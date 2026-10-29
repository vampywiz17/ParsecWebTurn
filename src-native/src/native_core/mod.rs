//! EvoParseq native account and session entry point.
//! The production media, carrier, route retry and HID workers are reused.
mod account;
#[cfg(windows)]
mod controls;
mod display_timing;
mod display_windows;
pub(crate) mod host_displays;
mod host_video;
mod input;
mod picker;
mod pointer;
pub(crate) mod profile;
mod session;
#[cfg(windows)]
mod session_badge;
pub(crate) mod session_model;
#[cfg(windows)]
mod session_platform;
#[cfg(target_os = "macos")]
#[path = "session_platform_macos.rs"]
mod session_platform;
#[cfg(windows)]
pub(crate) mod session_ui;
#[cfg(target_os = "macos")]
#[path = "session_ui_macos.rs"]
mod session_ui;
mod session_video;
mod sign_in;
mod signaling;
mod transport;
#[cfg(windows)]
pub(crate) mod ui;
#[cfg(target_os = "macos")]
#[path = "ui_macos.rs"]
pub(crate) mod ui;

use anyhow::{Context, Result};

#[cfg(target_os = "macos")]
pub(crate) fn open_stream_settings(
    window: std::sync::Arc<crate::window::Window>,
    manager: std::sync::Arc<crate::connection_settings::Manager>,
) {
    session_ui::open(window, manager);
}

#[cfg(target_os = "macos")]
pub fn main() {
    // AppKit owns the main thread; the account/session loop below blocks.
    ui::run_application(|| {
        if let Err(error) = run() {
            unsafe {
                ui::message_box(std::ptr::null_mut(), &error.to_string());
            }
        }
    });
}

#[cfg(windows)]
pub fn main() {
    if let Err(error) = run() {
        // Every error crossing this entry is an application-authored message;
        // raw HTTP/WebSocket errors must never display their credential URL.
        unsafe {
            ui::message_box(std::ptr::null_mut(), &error.to_string());
        }
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--version"] {
        println!(
            "EvoParseq {} / native Rust core (BUD/WebRTC)",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    let root = if args.is_empty() && cfg!(target_os = "macos") {
        // An app bundle may be signed or translocated read-only.
        let root =
            std::path::PathBuf::from(std::env::var_os("HOME").context("Home folder unavailable")?)
                .join("Library/Application Support/EvoParseq");
        std::fs::create_dir_all(&root)?;
        root
    } else if args.is_empty() {
        std::env::current_exe()?
            .parent()
            .context("Application folder unavailable")?
            .to_path_buf()
    } else if args.len() == 2 && args[0] == "--data-dir" {
        std::path::PathBuf::from(&args[1])
    } else {
        anyhow::bail!("Unsupported native core argument");
    };
    let settings = crate::connection_settings::Manager::open(root.join("settings.json"));
    crate::event_log::initialize(
        root.join("client-events.jsonl"),
        settings.view().0.event_logging,
    )?;
    let _log = crate::event_log::Guard;
    evoparseq_socket_qos::observe(|transport, status| {
        crate::event_log::record(
            if status.applied { "INFO" } else { "WARN" },
            "turn_tcp_qos",
            serde_json::json!({"transport":transport,"requested":status.requested,"applied":status.applied,
                "configured_dscp":status.configured_dscp,"configured_l2":status.configured_l2,"os_error":status.error}),
        );
    });
    crate::event_log::record(
        "INFO",
        "native_core_started",
        serde_json::json!({"transport":if settings.view().0.bud {"BUD"} else {"WebRTC"}}),
    );
    #[cfg(windows)]
    prepare_vsr_runtime(&settings);
    let mut session = None;
    loop {
        let Some(selection) = ui::choose(settings.clone(), session.take())? else {
            return Ok(());
        };
        let preferences = settings.view().0;
        let requested_main10 = preferences.prefer_hevc && preferences.experimental_ten_bit;
        let policy = transport::Policy {
            bud: preferences.bud,
        };
        let result = (|| {
            let socket = signaling::Socket::open(&selection.session)?;
            let platform = session_platform::Platform::create(settings.clone())?;
            session::run(
                &selection.peer,
                &socket,
                &platform,
                settings.clone(),
                policy,
            )
        })();
        if let Err(error) = result {
            crate::event_log::record(
                "ERROR",
                "native_core_failed",
                serde_json::json!({"stage":"signaling_or_session"}),
            );
            unsafe {
                let message = error
                    .downcast_ref::<transport::HostStatus>()
                    .map(|status| status.user_message(requested_main10))
                    .unwrap_or_else(|| error.to_string());
                ui::message_box(std::ptr::null_mut(), &message);
            }
        }
        session = Some(selection.session);
    }
}

#[cfg(windows)]
fn prepare_vsr_runtime(settings: &crate::connection_settings::Manager) {
    use crate::connection_settings::VsrRuntimeConsent;
    if !cfg!(nvidia_vsr_sdk) || settings.view().1.is_some() {
        return;
    }
    let data = settings.directory();
    if crate::vsr_runtime::reuse_previous(&data).unwrap_or(false) {
        return;
    }
    if !crate::gpu_windows::enumerate()
        .is_ok_and(|gpus| gpus.iter().any(|g| g.preference.vendor_id == 0x10de))
    {
        return;
    }
    let mut preferences = settings.view().0;
    if preferences.vsr_runtime_consent == VsrRuntimeConsent::Ask {
        let text: Vec<u16> = crate::vsr_runtime::PROMPT
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let title: Vec<u16> = "EvoParseq - optional NVIDIA VSR"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let accepted = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                windows_sys::Win32::UI::WindowsAndMessaging::MB_YESNO
                    | windows_sys::Win32::UI::WindowsAndMessaging::MB_DEFBUTTON2
                    | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONQUESTION,
            )
        } == windows_sys::Win32::UI::WindowsAndMessaging::IDYES;
        preferences.vsr_runtime_consent = if accepted {
            VsrRuntimeConsent::Allowed
        } else {
            VsrRuntimeConsent::Declined
        };
        if settings
            .update_vsr_consent(preferences.vsr_runtime_consent)
            .is_err()
        {
            crate::event_log::record(
                "WARN",
                "vsr_runtime_consent_save_failed",
                serde_json::json!({}),
            );
            unsafe {
                ui::message_box(std::ptr::null_mut(), "The VSR download permission could not be saved beside settings.json. No download was started; normal GPU video remains available.");
            }
            return;
        }
    }
    if preferences.vsr_runtime_consent == VsrRuntimeConsent::Allowed {
        if let Err(error) = crate::vsr_runtime::install() {
            crate::event_log::record(
                "WARN",
                "vsr_runtime_install_failed",
                serde_json::json!({"reason":error.to_string()}),
            );
            unsafe {
                ui::message_box(std::ptr::null_mut(), "The optional NVIDIA VSR runtime could not be downloaded or verified. Normal GPU video remains available. See the event log.");
            }
        }
    }
}
