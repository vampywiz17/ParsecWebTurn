//! TV platform adapters reuse the Windows client's account and WebRTC source.
#[path = "../../../src-native/src/native_core/account.rs"]
pub mod account;
#[path = "../../../src-native/src/native_core/signaling.rs"]
pub mod account_signaling;
#[path = "../../../src-native/src/attempt.rs"]
pub mod attempt;
#[path = "../../../src-native/src/audio_layout.rs"]
pub mod audio_layout;
#[path = "../../../src-native/src/audio_stream.rs"]
pub mod audio_stream;
#[path = "../../../src-native/src/codec_initialization.rs"]
pub mod codec_initialization;
#[path = "../../../src-native/src/control.rs"]
pub mod control;
#[cfg(any(target_os = "android", test))]
mod decode_state;
#[path = "../../../src-native/src/event_log.rs"]
mod event_log;
#[path = "../../../src-native/src/frame_timing.rs"]
pub mod frame_timing;
#[path = "../../../src-native/src/native_core/host_video.rs"]
pub mod host_video;
#[path = "../../../src-native/src/media_ingress.rs"]
pub mod media_ingress;
#[path = "../../../src-native/src/native_wire.rs"]
pub mod native_wire;
pub mod preferences;
#[path = "../../../src-native/src/rtc_configuration.rs"]
pub mod rtc_configuration;
#[path = "../../../src-native/src/session_preferences.rs"]
pub mod session_preferences;
#[path = "../../../src-native/src/signaling.rs"]
pub mod signaling;
#[path = "../../../src-native/src/transport.rs"]
pub mod transport;
#[path = "../../../src-native/src/video_output.rs"]
pub mod video_output;
#[path = "../../../src-native/src/video_stream.rs"]
pub mod video_stream;

#[cfg(target_os = "android")]
mod audio_android;
#[cfg(target_os = "android")]
mod stream;
#[cfg(target_os = "android")]
pub mod video_android;

#[cfg(target_os = "android")]
mod android;
