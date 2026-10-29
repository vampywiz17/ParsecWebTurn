#![cfg_attr(windows, windows_subsystem = "windows")]
#![warn(unused_crate_dependencies)]
// Integration-only TLS fixtures are compiled as separate test crates.
#[cfg(test)]
use {rcgen as _, rustls as _, tokio_rustls as _};

const APP_TITLE: &str = "EvoParseq";
mod attempt;
#[cfg(windows)]
mod audio_endpoint_windows;
mod audio_layout;
#[cfg(target_os = "macos")]
mod audio_macos;
#[cfg(any(windows, test))]
mod audio_recovery;
mod audio_stream;
#[cfg(windows)]
mod audio_windows;
#[cfg(windows)]
mod branding;
#[cfg(all(any(windows, target_os = "macos"), feature = "bud"))]
mod bud_attempt;
#[cfg(all(any(windows, target_os = "macos"), feature = "bud"))]
mod bud_media;
mod connection_settings;
#[cfg(windows)]
mod connection_settings_ui;
#[cfg(target_os = "macos")]
mod connection_settings_ui_macos;
mod control;
#[cfg(test)]
mod crypto_policy_tests;
#[cfg(any(windows, target_os = "macos"))]
mod cursor;
#[cfg(test)]
mod data_only_policy_tests;
mod event_log;
mod event_queue;
#[cfg(any(windows, target_os = "macos"))]
mod frame_timing;
#[cfg(windows)]
mod fullscreen;
#[cfg(windows)]
mod gpu_windows;
#[cfg(all(any(windows, target_os = "macos"), feature = "bud"))]
mod hid_input;
mod input;
mod input_state;
#[cfg(windows)]
mod input_windows;
#[cfg(windows)]
mod keyboard_grab;
mod lifecycle;
mod media_ingress;
#[cfg(windows)]
mod media_scheduling;
#[cfg(any(windows, target_os = "macos"))]
mod native_core;
#[cfg(windows)]
mod nvidia_vsr;
#[cfg(windows)]
mod performance;
mod platform;
#[cfg(target_os = "macos")]
mod platform_macos;
#[cfg(windows)]
mod platform_windows;
mod secure_storage;
#[cfg(any(windows, target_os = "macos"))]
mod settings_model;
mod signaling;
#[cfg(any(windows, target_os = "macos"))]
mod stats;
mod stats_recent;
#[cfg(windows)]
mod stats_ui;
#[cfg(any(windows, target_os = "macos"))]
mod stats_view;
mod transport;
#[cfg(windows)]
mod updater;
#[cfg(windows)]
mod updater_ui;
#[cfg(any(windows, test))]
mod video_color;
#[cfg(target_os = "macos")]
mod video_macos;
mod video_output;
mod video_stream;
#[cfg(windows)]
mod video_upscale;
#[cfg(windows)]
mod video_windows;
mod viewport;
#[cfg(windows)]
mod vsr_runtime;
#[cfg(any(windows, target_os = "macos"))]
mod wake_lock;
#[cfg(windows)]
mod window;
#[cfg(target_os = "macos")]
mod window_macos;
// Each platform's media and window backends behind one static name.
#[cfg(target_os = "macos")]
use {audio_macos as audio_platform, video_macos as video_platform, window_macos as window};
#[cfg(windows)]
use {audio_windows as audio_platform, video_windows as video_platform};
fn main() {
    #[cfg(any(windows, target_os = "macos"))]
    native_core::main();
    #[cfg(not(any(windows, target_os = "macos")))]
    eprintln!("EvoParseq currently requires Windows or macOS.");
}
