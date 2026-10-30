//! RFC 6716 reference decoder. The pinned Parsec channel carries raw Opus,
//! 48 kHz stereo; containers/RTP headers must not be invented around it.
use crate::audio_layout::Layout;
use anyhow::{bail, Result};
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{mpsc, Arc, Mutex},
    thread::JoinHandle,
};
const MAX_SAMPLES: usize = 48_000; // 500 ms, interleaved stereo
const PCM_SAMPLES: usize = 5760 * 2; // RFC maximum 120 ms, stereo
const RECYCLED_BUFFERS: usize = 32;
#[derive(Clone, Default, Serialize)]
pub struct Snapshot {
    pub layout: Option<Layout>,
    pub queued_frames: usize,
    pub codec: Option<&'static str>,
    pub packets_decoded: u64,
    pub packets_dropped: u64,
    pub decode_errors: u64,
    pub pcm_frames_consumed: u64,
    pub queued_samples: usize,
    pub worker_finished: bool,
}
#[derive(Default)]
struct State {
    report: Snapshot,
    pcm: VecDeque<Vec<i16>>,
    free_pcm: Vec<Vec<i16>>,
    free_packets: Vec<Vec<u8>>,
    stopped: bool,
    generation: u64,

    native_consumer: Option<std::thread::Thread>,
}
impl State {
    fn discard_pending(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        while let Some(pcm) = self.pcm.pop_front() {
            self.report.packets_dropped += 1;
            if self.free_pcm.len() < RECYCLED_BUFFERS {
                self.free_pcm.push(pcm);
            }
        }
        self.report.queued_samples = 0;
    }
}

pub struct Handoff {
    pub layout: Layout,
    pub pcm: Vec<i16>,
}
fn stage_pcm(state: &mut State, mut pcm: Vec<i16>, max_samples: usize) {
    if pcm.len() > max_samples {
        pcm.drain(..pcm.len() - max_samples);
        state.report.packets_dropped += 1;
    }
    while state.report.queued_samples + pcm.len() > max_samples {
        if let Some(old) = state.pcm.pop_front() {
            state.report.queued_samples -= old.len();
            state.report.packets_dropped += 1;
            if state.free_pcm.len() < RECYCLED_BUFFERS {
                state.free_pcm.push(old);
            }
        } else {
            break;
        }
    }
    state.report.queued_samples += pcm.len();
    state.pcm.push_back(pcm);

    if let Some(consumer) = &state.native_consumer {
        consumer.unpark();
    }
}
#[derive(Clone)]
pub(crate) struct Sender {
    tx: mpsc::SyncSender<Packet>,
    state: Arc<Mutex<State>>,
    allowed: Layout,
}
impl Sender {
    pub fn submit(&self, bytes: &[u8]) -> bool {
        self.submit_format(bytes, false)
    }
    #[cfg(test)]
    pub fn submit_pcm(&self, bytes: &[u8]) -> bool {
        if bytes.is_empty() || bytes.len() > 32_000 || !bytes.len().is_multiple_of(4) {
            return false;
        }
        self.submit_format(bytes, true)
    }
    fn submit_format(&self, bytes: &[u8], raw: bool) -> bool {
        self.submit_layout(bytes, raw, Layout::STEREO)
    }
    pub fn submit_layout(&self, bytes: &[u8], raw: bool, layout: Layout) -> bool {
        if (layout != Layout::STEREO && layout != self.allowed)
            || (raw && (bytes.len() > 32_000 || !bytes.len().is_multiple_of(layout.samples(1) * 2)))
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.report.packets_dropped += 1;
            if state.report.packets_dropped.is_power_of_two() {
                crate::event_log::record(
                    "ERROR",
                    "audio_packet_rejected",
                    serde_json::json!({"stage":"layout_or_pcm_framing","channels":layout.channels,"channel_mask":layout.mask}),
                );
            }
            return false;
        }
        let (mut packet, generation) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.stopped || bytes.is_empty() || bytes.len() > 65_536 {
                state.report.packets_dropped += 1;
                return false;
            }
            (
                state.free_packets.pop().unwrap_or_default(),
                state.generation,
            )
        };
        packet.clear();
        packet.extend_from_slice(bytes);
        match self.tx.try_send(Packet {
            generation,
            bytes: packet,
            raw,
            layout,
        }) {
            Ok(()) => true,
            Err(error) => {
                let packet = match error {
                    mpsc::TrySendError::Full(packet) | mpsc::TrySendError::Disconnected(packet) => {
                        packet
                    }
                };
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                state.report.packets_dropped += 1;
                if !state.stopped && state.free_packets.len() < RECYCLED_BUFFERS {
                    state.free_packets.push(packet.bytes);
                }
                false
            }
        }
    }
}
struct Packet {
    generation: u64,
    bytes: Vec<u8>,
    raw: bool,
    layout: Layout,
}

#[derive(Clone)]
pub struct NativeReceiver {
    state: Arc<Mutex<State>>,
}
fn snapshot(shared: &Mutex<State>) -> Snapshot {
    let mut report = shared
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .report
        .clone();
    report.queued_frames =
        report.queued_samples / usize::from(report.layout.unwrap_or(Layout::STEREO).channels);
    report
}

impl NativeReceiver {
    #[cfg(not(windows))]
    pub fn snapshot(&self) -> Snapshot {
        snapshot(&self.state)
    }
    pub fn set_native_consumer(&self, thread: std::thread::Thread) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .native_consumer = Some(thread);
    }
    pub fn take_native_pcm(&self) -> Option<Handoff> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let pcm = state.pcm.pop_front()?;
        let layout = state.report.layout.unwrap_or(Layout::STEREO);
        state.report.queued_samples -= pcm.len();
        state.report.pcm_frames_consumed += (pcm.len() / usize::from(layout.channels)) as u64;
        Some(Handoff { layout, pcm })
    }
    pub fn recycle_handoff(&self, block: Handoff) {
        recycle_pcm(&self.state, block);
    }
    /// Retire staged and in-flight old audio at an output-recovery boundary.
    /// The decoder still consumes Opus packets in order to preserve its state.
    pub fn discard_pending(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.discard_pending();
    }
}
fn recycle_pcm(shared: &Mutex<State>, block: Handoff) {
    let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
    if !state.stopped && state.free_pcm.len() < RECYCLED_BUFFERS {
        state.free_pcm.push(block.pcm);
    }
}
pub struct Pipeline {
    tx: Option<mpsc::SyncSender<Packet>>,
    state: Arc<Mutex<State>>,
    worker: Option<JoinHandle<()>>,
    allowed: Layout,
}
struct MultistreamDecoder(*mut libopus_sys::OpusMSDecoder);
impl MultistreamDecoder {
    fn new(layout: Layout) -> Result<Self> {
        let (streams, coupled, mapping) = layout.opus();
        let mut error = 0;
        let pointer = unsafe {
            libopus_sys::opus_multistream_decoder_create(
                48_000,
                i32::from(layout.channels),
                streams,
                coupled,
                mapping.as_ptr(),
                &mut error,
            )
        };
        if pointer.is_null() || error != 0 {
            bail!("multichannel Opus initialization failed");
        }
        Ok(Self(pointer))
    }
    fn decode_into(&mut self, packet: &[u8], pcm: &mut Vec<i16>, layout: Layout) -> Result<()> {
        pcm.resize(layout.samples(5760), 0);
        let frames = unsafe {
            libopus_sys::opus_multistream_decode(
                self.0,
                packet.as_ptr(),
                packet.len() as i32,
                pcm.as_mut_ptr(),
                5760,
                0,
            )
        };
        if !(1..=5760).contains(&frames) {
            bail!("multichannel Opus packet rejected");
        }
        pcm.truncate(layout.samples(frames as usize));
        Ok(())
    }
}
impl Drop for MultistreamDecoder {
    fn drop(&mut self) {
        unsafe { libopus_sys::opus_multistream_decoder_destroy(self.0) };
    }
}
struct Decoder(*mut libopus_sys::OpusDecoder);
impl Decoder {
    fn reset(&mut self) -> Result<()> {
        // A PCM interval is a discontinuity in the compressed predictor history.
        let code =
            unsafe { libopus_sys::opus_decoder_ctl(self.0, libopus_sys::OPUS_RESET_STATE as i32) };
        if code != 0 {
            bail!("Opus decoder reset failed");
        }
        Ok(())
    }
    fn new() -> Result<Self> {
        let mut error = 0;
        // SAFETY: libopus owns this opaque state; this worker alone uses it.
        let pointer = unsafe { libopus_sys::opus_decoder_create(48_000, 2, &mut error) };
        if pointer.is_null() || error != 0 {
            bail!("Opus decoder initialization failed");
        }
        Ok(Self(pointer))
    }
    fn decode_into(&mut self, packet: &[u8], pcm: &mut Vec<i16>) -> Result<()> {
        if packet.is_empty() || packet.len() > 65_536 {
            bail!("invalid Opus packet boundary");
        }
        pcm.resize(PCM_SAMPLES, 0);
        // SAFETY: both slices remain alive, capacity matches frame_size*channels.
        let frames = unsafe {
            libopus_sys::opus_decode(
                self.0,
                packet.as_ptr(),
                packet.len() as i32,
                pcm.as_mut_ptr(),
                5760,
                0,
            )
        };
        if !(1..=5760).contains(&frames) {
            bail!("Opus packet rejected");
        }
        pcm.truncate(frames as usize * 2);
        Ok(())
    }
    #[cfg(test)]
    fn decode(&mut self, packet: &[u8]) -> Result<Vec<i16>> {
        let mut pcm = Vec::new();
        self.decode_into(packet, &mut pcm)?;
        Ok(pcm)
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { libopus_sys::opus_decoder_destroy(self.0) }
    }
}
impl Pipeline {
    pub fn allowed_layout(&self) -> Layout {
        self.allowed
    }
    #[cfg(test)]
    pub fn start() -> Self {
        Self::start_with_budget(None)
    }
    pub fn start_with_budget(maximum_ms: Option<u32>) -> Self {
        Self::start_with_layout(maximum_ms, Layout::STEREO)
    }
    pub fn start_with_layout(maximum_ms: Option<u32>, allowed: Layout) -> Self {
        let max_samples = maximum_ms
            .map(|ms| ms.clamp(1, 500) as usize * 96)
            .unwrap_or(MAX_SAMPLES);
        let (tx, rx) = mpsc::sync_channel::<Packet>(64);
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        let worker = std::thread::spawn(move || {
            #[cfg(windows)]
            let _scheduling = crate::media_scheduling::Registration::audio();
            if let Ok(mut decoder) = Decoder::new() {
                let mut previous_raw = false;
                let mut previous_layout = Layout::STEREO;
                let mut multistream: Option<(Layout, MultistreamDecoder)> = None;
                loop {
                    if shared.lock().unwrap_or_else(|e| e.into_inner()).stopped {
                        break;
                    }
                    let packet = match rx.recv_timeout(std::time::Duration::from_millis(20)) {
                        Ok(packet) => packet,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    let recycled = shared
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .free_pcm
                        .pop();
                    let mut pcm = recycled.unwrap_or_else(|| Vec::with_capacity(PCM_SAMPLES));
                    let decoded = if packet.raw {
                        pcm.clear();
                        pcm.extend(
                            packet
                                .bytes
                                .as_chunks::<2>()
                                .0
                                .iter()
                                .map(|b| i16::from_le_bytes([b[0], b[1]])),
                        );
                        Ok(())
                    } else if packet.layout != Layout::STEREO {
                        if previous_raw
                            || previous_layout != packet.layout
                            || multistream
                                .as_ref()
                                .is_none_or(|(layout, _)| *layout != packet.layout)
                        {
                            multistream = MultistreamDecoder::new(packet.layout)
                                .ok()
                                .map(|decoder| (packet.layout, decoder));
                        }
                        multistream
                            .as_mut()
                            .ok_or_else(|| anyhow::anyhow!("multichannel Opus unavailable"))
                            .and_then(|(_, decoder)| {
                                decoder.decode_into(&packet.bytes, &mut pcm, packet.layout)
                            })
                    } else if previous_raw || previous_layout != Layout::STEREO {
                        decoder
                            .reset()
                            .and_then(|()| decoder.decode_into(&packet.bytes, &mut pcm))
                    } else {
                        decoder.decode_into(&packet.bytes, &mut pcm)
                    };
                    previous_raw = packet.raw;
                    previous_layout = packet.layout;
                    let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                    if !state.stopped && state.free_packets.len() < RECYCLED_BUFFERS {
                        state.free_packets.push(packet.bytes);
                    }
                    if state.stopped {
                        continue;
                    }
                    match decoded {
                        Ok(()) if packet.generation != state.generation => {
                            state.report.packets_decoded += 1;
                            state.report.packets_dropped += 1;
                            if state.free_pcm.len() < RECYCLED_BUFFERS {
                                state.free_pcm.push(pcm);
                            }
                        }
                        Ok(()) => {
                            if state.report.layout != Some(packet.layout) {
                                state.pcm.clear();
                                state.report.queued_samples = 0;
                                state.report.layout = Some(packet.layout);
                            }
                            state.report.codec = Some(if packet.raw {
                                "PCM (uncompressed)"
                            } else {
                                "Opus"
                            });
                            state.report.packets_decoded += 1;
                            // PCM's wire ceiling exceeds Opus's maximum decoded
                            // block. Preserve the bounded PCM block boundary.
                            let limit = packet.layout.samples(5760);
                            let budget = max_samples / 2 * usize::from(packet.layout.channels);
                            if packet.raw && pcm.len() > limit {
                                let tail = pcm.split_off(limit);
                                stage_pcm(&mut state, pcm, budget);
                                stage_pcm(&mut state, tail, budget);
                            } else {
                                stage_pcm(&mut state, pcm, budget);
                            }
                        }
                        Err(_) => {
                            state.report.decode_errors += 1;
                            if state.report.decode_errors.is_power_of_two() {
                                crate::event_log::record(
                                    "ERROR",
                                    "audio_decode_failed",
                                    serde_json::json!({"channels":packet.layout.channels,"channel_mask":packet.layout.mask,"raw":packet.raw}),
                                );
                            }
                            if state.free_pcm.len() < RECYCLED_BUFFERS {
                                state.free_pcm.push(pcm);
                            }
                        }
                    }
                }
            } else {
                shared
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .report
                    .decode_errors += 1;
            }
            shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .report
                .worker_finished = true;
        });
        Self {
            tx: Some(tx),
            state,
            worker: Some(worker),
            allowed,
        }
    }
    pub(crate) fn sender(&self) -> Option<Sender> {
        self.tx.as_ref().map(|tx| Sender {
            tx: tx.clone(),
            state: self.state.clone(),
            allowed: self.allowed,
        })
    }

    #[cfg(test)]
    pub fn recycle_handoff(&self, block: Handoff) {
        recycle_pcm(&self.state, block);
    }

    pub fn native_receiver(&self) -> NativeReceiver {
        NativeReceiver {
            state: self.state.clone(),
        }
    }
    #[cfg(test)]
    pub fn set_native_consumer(&self, thread: std::thread::Thread) {
        self.native_receiver().set_native_consumer(thread);
    }
    #[cfg(test)]
    pub fn take_native_pcm(&self) -> Option<Handoff> {
        self.native_receiver().take_native_pcm()
    }
    pub fn snapshot(&self) -> Snapshot {
        snapshot(&self.state)
    }
    pub fn stop(&mut self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).stopped = true;
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.pcm.clear();
        state.free_pcm.clear();
        state.free_packets.clear();
        state.report.queued_samples = 0;
    }
}
impl Drop for Pipeline {
    fn drop(&mut self) {
        self.stop();
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_barrier_retires_staged_and_inflight_audio_without_stopping_decoder() {
        let pipeline = Pipeline::start_with_budget(Some(100));
        let receiver = pipeline.native_receiver();
        receiver.set_native_consumer(std::thread::current());
        {
            // Holding the existing state lock keeps the decoder from publishing
            // this packet before the same retirement operation used by recovery.
            let mut state = pipeline.state.lock().unwrap();
            stage_pcm(&mut state, vec![90, -90], MAX_SAMPLES);
            pipeline
                .tx
                .as_ref()
                .unwrap()
                .try_send(Packet {
                    generation: state.generation,
                    bytes: [111i16, -111]
                        .iter()
                        .flat_map(|v| v.to_le_bytes())
                        .collect(),
                    raw: true,
                    layout: Layout::STEREO,
                })
                .unwrap_or_else(|_| panic!("Fixture audio channel rejected its first packet"));
            state.discard_pending();
            assert_eq!(state.report.queued_samples, 0);
        }
        assert!(pipeline.sender().unwrap().submit_pcm(
            &[222i16, -222]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>()
        ));
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let block = loop {
            if let Some(block) = receiver.take_native_pcm() {
                break block;
            }
            assert!(
                std::time::Instant::now() < until,
                "Fresh PCM did not reach the consumer"
            );
            std::thread::park_timeout(std::time::Duration::from_millis(20));
        };
        assert_eq!(block.pcm, [222, -222]);
        assert_eq!(pipeline.snapshot().packets_decoded, 2);
        assert_eq!(pipeline.snapshot().decode_errors, 0);
        assert_eq!(pipeline.snapshot().packets_dropped, 2);
        receiver.recycle_handoff(block);
        assert!(receiver.take_native_pcm().is_none());
    }

    #[test]
    fn native_pcm_handoff_keeps_layout_and_recycles() {
        let layout = Layout::new(6, 0x60f).unwrap();
        let pipeline = Pipeline::start_with_layout(Some(100), layout);
        pipeline.set_native_consumer(std::thread::current());
        let samples: Vec<i16> = (0..60).collect();
        let bytes: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        assert!(pipeline
            .sender()
            .unwrap()
            .submit_layout(&bytes, true, layout));
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let block = loop {
            if let Some(block) = pipeline.take_native_pcm() {
                break block;
            }
            assert!(
                std::time::Instant::now() < until,
                "PCM decoder did not wake its consumer"
            );
            std::thread::park_timeout(std::time::Duration::from_millis(20));
        };
        assert_eq!(block.layout, layout);
        assert_eq!(block.pcm, samples);
        assert_eq!(pipeline.snapshot().queued_frames, 0);
        pipeline.recycle_handoff(block);
        assert!(pipeline.take_native_pcm().is_none());
    }

    #[test]
    fn multistream_decode_preserves_distinct_channel_signals() {
        for layout in [
            Layout::new(6, 0x3f).unwrap(),
            Layout::new(8, 0x63f).unwrap(),
        ] {
            let (streams, coupled, mapping) = layout.opus();
            let mut error = 0;
            let encoder = unsafe {
                libopus_sys::opus_multistream_encoder_create(
                    48_000,
                    i32::from(layout.channels),
                    streams,
                    coupled,
                    mapping.as_ptr(),
                    libopus_sys::OPUS_APPLICATION_AUDIO as i32,
                    &mut error,
                )
            };
            assert!(!encoder.is_null());
            assert_eq!(error, 0);
            let mut decoder = MultistreamDecoder::new(layout).unwrap();
            let mut decoded = Vec::new();
            let mut packet = vec![0; 16_384];
            for active in 0..usize::from(layout.channels) {
                let input: Vec<i16> = (0..960)
                    .flat_map(|frame| {
                        (0..usize::from(layout.channels)).map(move |channel| {
                            if channel == active {
                                ((frame as f32 * 0.08).sin() * 8000.) as i16
                            } else {
                                0
                            }
                        })
                    })
                    .collect();
                // Let the codec's previous block and lookahead pass first.
                for _ in 0..4 {
                    let len = unsafe {
                        libopus_sys::opus_multistream_encode(
                            encoder,
                            input.as_ptr(),
                            960,
                            packet.as_mut_ptr(),
                            packet.len() as i32,
                        )
                    };
                    assert!(len > 0);
                    decoder
                        .decode_into(&packet[..len as usize], &mut decoded, layout)
                        .unwrap();
                }
                let energy: Vec<i64> = (0..usize::from(layout.channels))
                    .map(|ch| {
                        decoded
                            .chunks_exact(usize::from(layout.channels))
                            .map(|f| i64::from(f[ch]).abs())
                            .sum()
                    })
                    .collect();
                assert!(energy[active] > 100_000);
                for (channel, &value) in energy.iter().enumerate() {
                    if channel != active {
                        assert!(
                            value < energy[active] / 4,
                            "unexpected cross-channel audio: {energy:?}"
                        );
                    }
                }
            }
            unsafe { libopus_sys::opus_multistream_encoder_destroy(encoder) };
        }
    }
    #[test]
    fn longest_raw_packet_is_split_at_the_existing_bounded_pcm_capacity() {
        let mut pipeline = super::Pipeline::start();
        assert!(pipeline.sender().unwrap().submit_pcm(&vec![0; 32000]));
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while pipeline.snapshot().packets_decoded == 0 && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let state = pipeline.state.lock().unwrap();
        assert_eq!(state.report.queued_samples, 16000);
        assert_eq!(state.pcm.len(), 2);
        assert!(state
            .pcm
            .iter()
            .all(|pcm| pcm.len() <= super::PCM_SAMPLES && pcm.len().is_multiple_of(2)));
        drop(state);
        pipeline.stop();
    }
    #[test]
    fn custom_decoded_backlog_keeps_latest_pcm_instead_of_accumulating_delay() {
        let mut pipeline = super::Pipeline::start_with_budget(Some(40));
        let sender = pipeline.sender().unwrap();
        for value in 0..10i16 {
            let mut packet = Vec::with_capacity(3840);
            for _ in 0..960 {
                packet.extend_from_slice(&value.to_le_bytes());
                packet.extend_from_slice(&(-value).to_le_bytes());
            }
            assert!(sender.submit_pcm(&packet));
        }
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while pipeline.snapshot().packets_decoded < 10 && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(pipeline.snapshot().packets_decoded, 10);
        let state = pipeline.state.lock().unwrap();
        assert_eq!(state.report.queued_samples, 3840);
        assert_eq!(state.pcm.front().unwrap()[..2], [8, -8]);
        assert_eq!(state.pcm.back().unwrap()[..2], [9, -9]);
        assert_eq!(state.report.packets_dropped, 8);
        drop(state);
        pipeline.stop();
    }
    #[test]
    fn pcm_extrema_and_opus_fallback_share_the_existing_poll_boundary() {
        let mut pipeline = super::Pipeline::start_with_budget(Some(40));
        let sender = pipeline.sender().unwrap();
        assert!(!sender.submit_pcm(&[0, 1, 2]));
        assert!(sender.submit_pcm(&[0, 128, 255, 127, 1, 0, 255, 255]));
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while pipeline.snapshot().packets_decoded < 1 && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            pipeline.state.lock().unwrap().pcm.front().unwrap(),
            &[i16::MIN, i16::MAX, 1, -1]
        );
        assert_eq!(pipeline.snapshot().codec, Some("PCM (uncompressed)"));
        assert!(sender.submit(&[0xf8, 0xff, 0xfe]));
        while pipeline.snapshot().packets_decoded < 2 && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(pipeline.snapshot().packets_decoded, 2);
        assert_eq!(pipeline.snapshot().codec, Some("Opus"));
        assert_eq!(pipeline.snapshot().decode_errors, 0);
        pipeline.stop();
        assert!(pipeline.snapshot().worker_finished);
    }
    #[test]
    fn repeated_decode_reuses_pcm_storage_and_stop_invalidates_sender_handles() {
        let mut decoder = Decoder::new().unwrap();
        let mut pcm = Vec::with_capacity(PCM_SAMPLES);
        let pointer = pcm.as_ptr();
        for _ in 0..20 {
            decoder.decode_into(&[0xf8, 0xff, 0xfe], &mut pcm).unwrap();
            assert_eq!(pcm.len(), 1920);
            assert_eq!(pcm.as_ptr(), pointer);
        }
        let mut pipeline = Pipeline::start();
        let sender = pipeline.sender().unwrap();
        pipeline.stop();
        assert!(pipeline.snapshot().worker_finished);
        assert!(!sender.submit(&[0xf8, 0xff, 0xfe]));
        let state = pipeline.state.lock().unwrap();
        assert!(state.free_packets.is_empty());
        assert!(state.free_pcm.is_empty());
    }
    #[test]
    fn reference_decoder_accepts_standard_silence_and_rejects_invalid_packet() {
        let mut decoder = Decoder::new().unwrap();
        let samples = decoder.decode(&[0xf8, 0xff, 0xfe]).unwrap();
        assert_eq!(samples.len(), 960 * 2);
        assert!(samples.iter().all(|v| v.abs() < 4));
        assert!(decoder.decode(&[]).is_err());
        assert!(decoder.decode(&[3]).is_err());
    }
    #[test]
    fn reference_opus_roundtrip_retains_stereo_pcm_signal() {
        let mut error = 0;
        let encoder = unsafe {
            libopus_sys::opus_encoder_create(
                48_000,
                2,
                libopus_sys::OPUS_APPLICATION_AUDIO as i32,
                &mut error,
            )
        };
        assert!(!encoder.is_null());
        assert_eq!(error, 0);
        let input: Vec<i16> = (0..960)
            .flat_map(|i| {
                let v = ((i as f32 * 0.08).sin() * 8000.) as i16;
                [v, -v]
            })
            .collect();
        let mut packet = vec![0; 4096];
        let size = unsafe {
            libopus_sys::opus_encode(
                encoder,
                input.as_ptr(),
                960,
                packet.as_mut_ptr(),
                packet.len() as i32,
            )
        };
        unsafe { libopus_sys::opus_encoder_destroy(encoder) };
        assert!(size > 0);
        packet.truncate(size as usize);
        let mut decoder = Decoder::new().unwrap();
        let pcm = decoder.decode(&packet).unwrap();
        assert_eq!(pcm.len(), 1920);
        assert!(pcm.iter().map(|v| i64::from(*v).abs()).sum::<i64>() > 100_000);
        decoder.decode(&packet).unwrap();
        decoder.reset().unwrap();
        assert_eq!(decoder.decode(&packet).unwrap(), pcm);
    }
}
