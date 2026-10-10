//! Shared-mode, event-driven WASAPI PCM output. COM objects stay on the worker.
use anyhow::{bail, Result};
use serde::Serialize;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{mpsc, Arc, Mutex},
    thread::JoinHandle,
};
use windows::{
    core::{Interface, PCWSTR},
    Win32::{
        Foundation::*,
        Media::Audio::*,
        System::{Com::*, Threading::*},
    },
};
#[derive(Clone, Default, Serialize)]
pub struct Snapshot {
    pub device_opened: bool,
    pub low_latency_shared_mode: bool,
    pub engine_period_frames: Option<u32>,
    pub device_buffer_frames: u32,
    pub mmcss_registered: bool,
    pub frames_queued: u64,
    pub frames_written: u64,
    pub frames_dropped: u64,
    pub underruns: u64,
    pub queued_frames: usize,
    pub device_padding: u32,
    pub failure_hresult: Option<String>,
    pub resources_released: bool,
}
#[derive(Default)]
pub struct Registry {
    next: u32,
    outputs: BTreeMap<u32, Arc<Playback>>,
    completed: VecDeque<Snapshot>,
}
impl Registry {
    #[cfg(feature = "diagnostics")]
    pub fn snapshots(&self) -> Vec<Snapshot> {
        self.completed
            .iter()
            .cloned()
            .chain(self.outputs.values().map(|v| v.snapshot()))
            .collect()
    }
    pub fn retain_snapshot(&mut self, snapshot: Snapshot) {
        if self.completed.len() == 8 {
            self.completed.pop_front();
        }
        self.completed.push_back(snapshot);
    }
    pub fn insert(&mut self, output: Playback) -> Result<u32> {
        if self.outputs.len() >= 4 {
            bail!("audio context limit");
        }
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("audio handle overflow"))?;
        self.outputs.insert(self.next, Arc::new(output));
        Ok(self.next)
    }
    pub fn get(&self, handle: u32) -> Result<Arc<Playback>> {
        self.outputs
            .get(&handle)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("invalid audio context"))
    }
    pub fn remove(&mut self, handle: u32) -> Result<Arc<Playback>> {
        self.outputs
            .remove(&handle)
            .ok_or_else(|| anyhow::anyhow!("invalid audio context"))
    }
}
#[derive(Default)]
struct State {
    report: Snapshot,
    pcm: VecDeque<i16>,
    stopped: bool,
    reset: bool,
}
pub struct Playback {
    shared: Arc<Mutex<State>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    max_frames: usize,
}
impl Playback {
    pub fn create(
        window: Arc<crate::window::Window>,
        min_buffer: u32,
        max_buffer: u32,
    ) -> Result<Self> {
        if min_buffer > max_buffer || max_buffer == 0 || max_buffer > 500 {
            bail!("invalid audio latency limits");
        }
        // Pinned AudioWorklet converts its latency arguments from milliseconds to frames.
        let max_frames = max_buffer as usize * 48;
        let min_frames = min_buffer as usize * 48;
        let shared = Arc::new(Mutex::new(State::default()));
        let state = shared.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        window
            .active_contexts
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let worker = std::thread::spawn(move || {
            let result = unsafe { render(&state, &window, min_frames, &tx) };
            if let Err(error) = result {
                state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .report
                    .failure_hresult = Some(format!("0x{:08X}", error.code().0 as u32));
                let _ = tx.try_send(false);
            }
            state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .report
                .resources_released = true;
            window
                .active_contexts
                .fetch_sub(1, std::sync::atomic::Ordering::Release);
        });
        let output = Self {
            shared,
            worker: Mutex::new(Some(worker)),
            max_frames,
        };
        if rx.recv_timeout(std::time::Duration::from_secs(3)).ok() != Some(true) {
            output.stop();
            bail!("WASAPI output unavailable");
        }
        Ok(output)
    }
    pub fn queue(&self, pcm: &[i16]) {
        let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let frames = pcm.len() / 2;
        if s.stopped
            || s.report.failure_hresult.is_some()
            || s.pcm.len() / 2 + frames > self.max_frames
        {
            s.report.frames_dropped += frames as u64;
            return;
        }
        s.pcm.extend(pcm);
        s.report.frames_queued += frames as u64;
        s.report.queued_frames = s.pcm.len() / 2;
    }
    pub fn queued_frames(&self) -> u32 {
        let s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        (s.pcm.len() / 2 + s.report.device_padding as usize) as u32
    }
    pub fn reset(&self) {
        let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        s.pcm.clear();
        s.report.queued_frames = 0;
        s.reset = true;
    }
    pub fn snapshot(&self) -> Snapshot {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .report
            .clone()
    }
    pub fn stop(&self) {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stopped = true;
        if let Some(worker) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = worker.join();
        }
    }
}
impl Drop for Playback {
    fn drop(&mut self) {
        self.stop();
    }
}
struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
struct Event(HANDLE);
impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
struct Started(IAudioClient);
impl Drop for Started {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.Stop();
            let _ = self.0.Reset();
        }
    }
}
unsafe fn render(
    shared: &Mutex<State>,
    window: &crate::window::Window,
    min_frames: usize,
    ready: &mpsc::SyncSender<bool>,
) -> windows::core::Result<()> {
    CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
    let _com = Apartment;
    let scheduling = crate::media_scheduling::Registration::audio();
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
    let format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: 2,
        nSamplesPerSec: 48_000,
        nAvgBytesPerSec: 192_000,
        nBlockAlign: 4,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    let (client, period) = initialize_client(&device, &format)?;
    let event = Event(CreateEventW(None, false, false, PCWSTR::null())?);
    client.SetEventHandle(event.0)?;
    let render: IAudioRenderClient = client.GetService()?;
    let capacity = client.GetBufferSize()?;
    {
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        state.report.low_latency_shared_mode = period.is_some();
        state.report.engine_period_frames = period;
        state.report.device_buffer_frames = capacity;
        state.report.mmcss_registered = scheduling.is_some();
    }
    // Prime with silence, without claiming real decoded audio was played.
    let _ = render.GetBuffer(capacity)?;
    render.ReleaseBuffer(capacity, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)?;
    client.Start()?;
    let _started = Started(client.clone());
    shared
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .report
        .device_opened = true;
    let _ = ready.send(true);
    let mut buffering = true;
    loop {
        WaitForSingleObject(event.0, 50);
        if window.closing.load(std::sync::atomic::Ordering::Acquire) {
            break;
        }
        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
        if s.stopped {
            break;
        }
        if s.reset {
            client.Stop()?;
            client.Reset()?;
            client.Start()?;
            s.reset = false;
            buffering = true;
        }
        let padding = client.GetCurrentPadding()?;
        s.report.device_padding = padding;
        let available = capacity.saturating_sub(padding);
        if available == 0 {
            continue;
        }
        if buffering && s.pcm.len() / 2 >= min_frames.max(1) {
            buffering = false;
        }
        let count = if buffering {
            0
        } else {
            available.min((s.pcm.len() / 2) as u32)
        };
        let pointer = render.GetBuffer(available)?;
        // SAFETY: WASAPI grants exactly available interleaved 16-bit stereo frames.
        let buffer = std::slice::from_raw_parts_mut(pointer.cast::<i16>(), available as usize * 2);
        buffer.fill(0);
        for sample in buffer.iter_mut().take(count as usize * 2) {
            *sample = s.pcm.pop_front().unwrap();
        }
        render.ReleaseBuffer(
            available,
            if count == 0 {
                AUDCLNT_BUFFERFLAGS_SILENT.0 as u32
            } else {
                0
            },
        )?;
        s.report.frames_written += count as u64;
        s.report.queued_frames = s.pcm.len() / 2;
        if count < available && !buffering {
            s.report.underruns += 1;
            buffering = true;
        }
    }
    Ok(())
}

/// Query the endpoint's supported period; never assume a fixed 1/2/5 ms quantum.
unsafe fn initialize_client(
    device: &IMMDevice,
    format: &WAVEFORMATEX,
) -> windows::core::Result<(IAudioClient, Option<u32>)> {
    let candidate: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
    if let Ok(client3) = candidate.cast::<IAudioClient3>() {
        let (mut default, mut fundamental, mut minimum, mut maximum) = (0, 0, 0, 0);
        if client3
            .GetSharedModeEnginePeriod(
                format,
                &mut default,
                &mut fundamental,
                &mut minimum,
                &mut maximum,
            )
            .is_ok()
        {
            if let Some(period) = supported_period(fundamental, minimum, maximum) {
                // IAudioClient3 documents EVENTCALLBACK only. AUTOCONVERTPCM
                // belongs to the legacy fallback, not this initialization call.
                if client3
                    .InitializeSharedAudioStream(
                        AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                        period,
                        format,
                        None,
                    )
                    .is_ok()
                {
                    return Ok((candidate, Some(period)));
                }
            }
        }
    }
    // A rejected format/period or an older endpoint must retain the working
    // converter path. Use a fresh COM client after any failed initialization.
    drop(candidate);
    let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
    client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
        200_000,
        0,
        format,
        None,
    )?;
    Ok((client, None))
}

fn supported_period(fundamental: u32, minimum: u32, maximum: u32) -> Option<u32> {
    if fundamental == 0 || minimum == 0 || minimum > maximum {
        return None;
    }
    let period = minimum.div_ceil(fundamental).checked_mul(fundamental)?;
    (period <= maximum).then_some(period)
}

/// Quiet local device smoke test: no account/network and no audible test tone.
#[cfg(feature = "diagnostics")]
pub fn probe() -> Result<serde_json::Value> {
    let window = crate::window::Window::create_video_probe()?;
    let mut snapshot = None;
    if let Ok(output) = Playback::create(window.clone(), 0, 200) {
        for _ in 0..10 {
            output.queue(&[0; 1920]);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        output.reset();
        output.stop();
        snapshot = Some(output.snapshot());
    }
    window.close();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while !window.handle().is_null() && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(
        serde_json::json!({"scope":"synthetic-silent-wasapi-output","audio":snapshot,"real_account_used":false,"external_requests_enabled":false,"native_window_released":window.handle().is_null()}),
    )
}

#[cfg(test)]
mod tests {
    use super::supported_period;

    #[test]
    fn engine_period_respects_device_granularity_bounds_and_overflow() {
        assert_eq!(supported_period(4, 48, 448), Some(48));
        assert_eq!(supported_period(48, 100, 480), Some(144));
        assert_eq!(supported_period(48, 100, 120), None);
        assert_eq!(supported_period(0, 48, 480), None);
        assert_eq!(supported_period(48, 0, 480), None);
        assert_eq!(supported_period(48, 480, 240), None);
        assert_eq!(supported_period(2, u32::MAX, u32::MAX), None);
    }
}
