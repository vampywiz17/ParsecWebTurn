//! Documented Windows media capability probe. It configures the exact native
//! backends intended for the live pipeline without retaining compressed media.
use anyhow::{bail, Result};
use serde::Serialize;

#[derive(Clone, Default, Serialize)]
pub struct Stats {
    pub video_ready: bool,
    pub audio_ready: bool,
    pub video_packets_submitted: u64,
    pub video_frames_decoded: u64,
    pub video_frames_presented: u64,
    pub video_frames_dropped: u64,
    pub video_width: u32,
    pub video_height: u32,
    pub audio_packets_submitted: u64,
    pub audio_pcm_frames_decoded: u64,
    pub audio_pcm_frames_rendered: u64,
    pub audio_frames_dropped: u64,
    pub audio_underflows: u64,
    pub worker_packets_dropped: u64,
    pub last_hresult: i32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub unavailable_reason: String,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct NativeStats {
    video_ready: i32,
    audio_ready: i32,
    video_packets_submitted: u64,
    video_frames_decoded: u64,
    video_frames_presented: u64,
    video_frames_dropped: u64,
    video_width: u32,
    video_height: u32,
    audio_packets_submitted: u64,
    audio_pcm_frames_decoded: u64,
    audio_pcm_frames_rendered: u64,
    audio_frames_dropped: u64,
    audio_underflows: u64,
    last_hresult: i32,
    unavailable_reason: [std::ffi::c_char; 256],
}

#[cfg(windows)]
impl Default for NativeStats {
    fn default() -> Self {
        // SAFETY: this repr(C) structure consists only of integer fields and
        // fixed integer arrays, for which an all-zero value is valid.
        unsafe { std::mem::zeroed() }
    }
}

#[cfg(windows)]
impl From<NativeStats> for Stats {
    fn from(value: NativeStats) -> Self {
        Self {
            video_ready: value.video_ready != 0,
            audio_ready: value.audio_ready != 0,
            video_packets_submitted: value.video_packets_submitted,
            video_frames_decoded: value.video_frames_decoded,
            video_frames_presented: value.video_frames_presented,
            video_frames_dropped: value.video_frames_dropped,
            video_width: value.video_width,
            video_height: value.video_height,
            audio_packets_submitted: value.audio_packets_submitted,
            audio_pcm_frames_decoded: value.audio_pcm_frames_decoded,
            audio_pcm_frames_rendered: value.audio_pcm_frames_rendered,
            audio_frames_dropped: value.audio_frames_dropped,
            audio_underflows: value.audio_underflows,
            last_hresult: value.last_hresult,
            unavailable_reason: text(&value.unavailable_reason),
            ..Default::default()
        }
    }
}

#[cfg(windows)]
unsafe extern "C" {
    fn parsec_native_media_session_create(
        hwnd: *mut std::ffi::c_void,
        session: *mut *mut std::ffi::c_void,
        stats: *mut NativeStats,
    ) -> i32;
    fn parsec_native_media_session_submit_video(
        session: *mut std::ffi::c_void,
        data: *const u8,
        size: u32,
        keyframe: i32,
    ) -> i32;
    fn parsec_native_media_session_submit_audio(
        session: *mut std::ffi::c_void,
        data: *const u8,
        size: u32,
    ) -> i32;
    fn parsec_native_media_session_stats(session: *mut std::ffi::c_void, stats: *mut NativeStats);
    fn parsec_native_media_session_destroy(session: *mut std::ffi::c_void);
}

#[cfg(windows)]
pub struct Pipeline {
    tx: Option<std::sync::mpsc::SyncSender<crate::media_ingress::Packet>>,
    stats: std::sync::Arc<std::sync::Mutex<Stats>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[cfg(windows)]
impl Pipeline {
    pub fn new(window: std::sync::Arc<crate::window::Window>) -> Result<Self> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<crate::media_ingress::Packet>(8);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let stats = std::sync::Arc::new(std::sync::Mutex::new(Stats::default()));
        let shared = stats.clone();
        let worker = std::thread::Builder::new()
            .name("native-media".into())
            .spawn(move || {
                let mut handle = std::ptr::null_mut();
                let mut native = NativeStats::default();
                // SAFETY: HWND is process-local and valid while `window` is held;
                // all opaque session calls remain on this one worker thread.
                let status = unsafe {
                    parsec_native_media_session_create(
                        window.handle().cast(),
                        &mut handle,
                        &mut native,
                    )
                };
                *shared.lock().unwrap_or_else(|e| e.into_inner()) = native.into();
                let _ = ready_tx.send(status);
                if status != 0 || handle.is_null() {
                    return;
                }
                while let Ok(packet) = rx.recv() {
                    let status = unsafe {
                        match packet.channel {
                            1 => parsec_native_media_session_submit_video(
                                handle,
                                packet.bytes.as_ptr(),
                                packet.bytes.len() as u32,
                                i32::from(packet.keyframe),
                            ),
                            2 => parsec_native_media_session_submit_audio(
                                handle,
                                packet.bytes.as_ptr(),
                                packet.bytes.len() as u32,
                            ),
                            _ => 0,
                        }
                    };
                    unsafe { parsec_native_media_session_stats(handle, &mut native) };
                    if status != 0 {
                        native.last_hresult = status;
                    }
                    let mut value: Stats = native.into();
                    if value.video_frames_presented != 0 {
                        window
                            .remote_video_active
                            .store(true, std::sync::atomic::Ordering::Release);
                    }
                    let mut current = shared.lock().unwrap_or_else(|e| e.into_inner());
                    value.worker_packets_dropped = current.worker_packets_dropped;
                    *current = value;
                }
                unsafe { parsec_native_media_session_destroy(handle) };
                window
                    .remote_video_active
                    .store(false, std::sync::atomic::Ordering::Release);
            })?;
        let status = ready_rx.recv()?;
        if status != 0 {
            let _ = worker.join();
            bail!("native media session failed ({status:#x})");
        }
        Ok(Self {
            tx: Some(tx),
            stats,
            worker: Some(worker),
        })
    }

    pub fn submit(&mut self, packet: crate::media_ingress::Packet) {
        if self
            .tx
            .as_ref()
            .is_some_and(|tx| tx.try_send(packet).is_err())
        {
            let mut stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
            stats.worker_packets_dropped = stats.worker_packets_dropped.saturating_add(1);
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[cfg(windows)]
impl Drop for Pipeline {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Serialize)]
pub struct Probe {
    pub schema: u32,
    pub video_codec: &'static str,
    pub video_backend: &'static str,
    pub video_d3d11_path_ready: bool,
    pub video_low_latency_enabled: bool,
    pub audio_codec: &'static str,
    pub audio_decoder_ready: bool,
    pub audio_backend: &'static str,
    pub wasapi_low_latency_ready: bool,
    pub audio_period_frames: u32,
    pub audio_period_microseconds: u32,
    pub adapter: String,
    pub video_decoder: String,
    pub audio_decoder: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub unavailable_reason: String,
}

#[cfg(windows)]
#[repr(C)]
struct NativeProbe {
    video_d3d11_path_ready: i32,
    video_low_latency_enabled: i32,
    audio_decoder_ready: i32,
    wasapi_low_latency_ready: i32,
    audio_period_frames: u32,
    audio_period_microseconds: u32,
    adapter: [std::ffi::c_char; 128],
    video_decoder: [std::ffi::c_char; 128],
    audio_decoder: [std::ffi::c_char; 128],
    unavailable_reason: [std::ffi::c_char; 256],
}

#[cfg(windows)]
unsafe extern "C" {
    fn parsec_native_media_probe(report: *mut NativeProbe) -> i32;
}

#[cfg(windows)]
fn text(value: &[std::ffi::c_char]) -> String {
    let len = value.iter().position(|c| *c == 0).unwrap_or(value.len());
    let bytes = value[..len].iter().map(|c| *c as u8).collect::<Vec<_>>();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(windows)]
pub fn probe() -> Result<Probe> {
    let mut native = NativeProbe {
        video_d3d11_path_ready: 0,
        video_low_latency_enabled: 0,
        audio_decoder_ready: 0,
        wasapi_low_latency_ready: 0,
        audio_period_frames: 0,
        audio_period_microseconds: 0,
        adapter: [0; 128],
        video_decoder: [0; 128],
        audio_decoder: [0; 128],
        unavailable_reason: [0; 256],
    };
    // SAFETY: `native` has the same repr(C) layout as the private C++ POD and
    // remains exclusively borrowed for the complete synchronous call.
    let status = unsafe { parsec_native_media_probe(&mut native) };
    if status != 0 {
        bail!("native media capability probe failed ({status:#x})");
    }
    Ok(Probe {
        schema: 1,
        video_codec: "H.264 Annex-B",
        video_backend: "Media Foundation + D3D11/DXVA",
        video_d3d11_path_ready: native.video_d3d11_path_ready != 0,
        video_low_latency_enabled: native.video_low_latency_enabled != 0,
        audio_codec: "Opus 48 kHz stereo",
        audio_decoder_ready: native.audio_decoder_ready != 0,
        audio_backend: "Media Foundation + event-driven WASAPI shared mode",
        wasapi_low_latency_ready: native.wasapi_low_latency_ready != 0,
        audio_period_frames: native.audio_period_frames,
        audio_period_microseconds: native.audio_period_microseconds,
        adapter: text(&native.adapter),
        video_decoder: text(&native.video_decoder),
        audio_decoder: text(&native.audio_decoder),
        unavailable_reason: text(&native.unavailable_reason),
    })
}

#[cfg(not(windows))]
pub fn probe() -> Result<Probe> {
    bail!("native media probe requires Windows")
}
