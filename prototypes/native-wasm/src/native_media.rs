//! Documented Windows media capability probe. It configures the exact native
//! backends intended for the live pipeline without retaining compressed media.
use anyhow::{bail, Result};
use serde::Serialize;

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
