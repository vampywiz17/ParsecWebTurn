//! RFC 6716 reference decoder. The pinned Parsec channel carries raw Opus,
//! 48 kHz stereo; containers/RTP headers must not be invented around it.
use anyhow::{bail, Result};
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{mpsc, Arc, Mutex},
    thread::JoinHandle,
};
const MAX_SAMPLES: usize = 48_000; // 500 ms, interleaved stereo
#[derive(Clone, Default, Serialize)]
pub struct Snapshot {
    pub packets_decoded: u64,
    pub packets_dropped: u64,
    pub decode_errors: u64,
    pub pcm_frames_polled: u64,
    pub queued_samples: usize,
    pub worker_finished: bool,
}
#[derive(Default)]
struct State {
    report: Snapshot,
    pcm: VecDeque<Vec<i16>>,
}
pub struct Pipeline {
    tx: Option<mpsc::SyncSender<Vec<u8>>>,
    state: Arc<Mutex<State>>,
    worker: Option<JoinHandle<()>>,
}
struct Decoder(*mut libopus_sys::OpusDecoder);
impl Decoder {
    fn new() -> Result<Self> {
        let mut error = 0;
        // SAFETY: libopus owns this opaque state; this worker alone uses it.
        let pointer = unsafe { libopus_sys::opus_decoder_create(48_000, 2, &mut error) };
        if pointer.is_null() || error != 0 {
            bail!("Opus decoder initialization failed");
        }
        Ok(Self(pointer))
    }
    fn decode(&mut self, packet: &[u8]) -> Result<Vec<i16>> {
        if packet.is_empty() || packet.len() > 65_536 {
            bail!("invalid Opus packet boundary");
        }
        let mut pcm = vec![0i16; 5760 * 2]; // RFC maximum 120 ms at 48 kHz
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
        Ok(pcm)
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { libopus_sys::opus_decoder_destroy(self.0) }
    }
}
impl Pipeline {
    pub fn start() -> Self {
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        let worker = std::thread::spawn(move || {
            if let Ok(mut decoder) = Decoder::new() {
                while let Ok(packet) = rx.recv() {
                    let decoded = decoder.decode(&packet);
                    let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                    match decoded {
                        Ok(pcm) => {
                            state.report.packets_decoded += 1;
                            // Drop oldest decoded audio on backpressure; never grow latency indefinitely.
                            while state.report.queued_samples + pcm.len() > MAX_SAMPLES {
                                if let Some(old) = state.pcm.pop_front() {
                                    state.report.queued_samples -= old.len();
                                    state.report.packets_dropped += 1;
                                } else {
                                    break;
                                }
                            }
                            state.report.queued_samples += pcm.len();
                            state.pcm.push_back(pcm);
                        }
                        Err(_) => state.report.decode_errors += 1,
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
        }
    }
    pub fn submit(&self, bytes: &[u8]) -> bool {
        let accepted = !bytes.is_empty()
            && bytes.len() <= 65_536
            && self
                .tx
                .as_ref()
                .is_some_and(|tx| tx.try_send(bytes.to_vec()).is_ok());
        if !accepted {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .report
                .packets_dropped += 1;
        }
        accepted
    }
    pub fn poll(
        &self,
        memory: &crate::memory::GuestMemory,
        pointer: u32,
        capacity: usize,
    ) -> Result<usize> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(pcm) = state.pcm.front() else {
            return Ok(0);
        };
        if pcm.len() > capacity {
            bail!("guest audio buffer too small");
        }
        let bytes: Vec<u8> = pcm.iter().flat_map(|v| v.to_le_bytes()).collect();
        memory.write(pointer, &bytes)?;
        let samples = pcm.len();
        state.pcm.pop_front();
        state.report.queued_samples -= samples;
        state.report.pcm_frames_polled += samples as u64 / 2;
        Ok(samples / 2)
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .report
            .clone()
    }
    pub fn stop(&mut self) {
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.pcm.clear();
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
    fn reference_decoder_accepts_standard_silence_and_rejects_invalid_packet() {
        let mut decoder = Decoder::new().unwrap();
        let samples = decoder.decode(&[0xf8, 0xff, 0xfe]).unwrap();
        assert_eq!(samples.len(), 960 * 2);
        assert!(samples.iter().all(|v| v.abs() < 4));
        assert!(decoder.decode(&[]).is_err());
        assert!(decoder.decode(&[3]).is_err());
    }
    #[test]
    fn decoded_audio_is_retained_until_a_valid_guest_copy_and_released_on_stop() {
        let mut config = wasmtime::Config::new();
        config.wasm_threads(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        let memory = crate::memory::GuestMemory(
            wasmtime::SharedMemory::new(&engine, wasmtime::MemoryType::shared(1, 1)).unwrap(),
        );
        let mut pipeline = Pipeline::start();
        assert!(pipeline.submit(&[0xf8, 0xff, 0xfe]));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while pipeline.snapshot().packets_decoded == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(pipeline.snapshot().queued_samples, 1920);
        assert!(pipeline.poll(&memory, 0, 1919).is_err());
        assert!(pipeline.poll(&memory, u32::MAX, 1920).is_err());
        assert_eq!(pipeline.snapshot().queued_samples, 1920);
        assert_eq!(pipeline.poll(&memory, 0, 1920).unwrap(), 960);
        assert_eq!(pipeline.poll(&memory, 0, 1920).unwrap(), 0);
        pipeline.stop();
        assert!(pipeline.snapshot().worker_finished);
        assert_eq!(pipeline.snapshot().queued_samples, 0);
    }
}
