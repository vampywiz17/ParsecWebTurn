//! Android account adapter. The account protocol is the Windows client's source,
//! not a second implementation. No media, SDK, host or browser runtime is linked.
#[path = "../../../src-native/src/native_core/account.rs"]
pub mod account;
#[allow(dead_code)]
#[path = "../../../src-native/src/event_log.rs"]
mod event_log;
pub mod preferences;
#[path = "../../../src-native/src/session_preferences.rs"]
pub mod session_preferences;

#[cfg(target_os = "android")]
mod android;
