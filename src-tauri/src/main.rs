#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ice;
mod provider;
mod settings;
mod stats;
mod storage;

use fs2::FileExt;
use settings::{SaveRequest, Settings, SettingsView};
use stats::{ConnectionStats, LatestStats};
use std::{
    fs::{self, File, OpenOptions},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Instant,
};
use tauri::{
    menu::{Menu, MenuItem},
    Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

struct AppState {
    root: PathBuf,
    _instance_lock: File,
    operation: tokio::sync::Mutex<()>,
    stats: Mutex<LatestStats>,
    auto_connect: AtomicBool,
}

// A shared WebView2 environment avoids an extra browser process for settings.
// Every view using this data directory must have identical browser arguments.
const BROWSER_ARGS: &str = "--autoplay-policy=no-user-gesture-required --disable-features=msWebOOUI,msPdfOOUI --disable-background-timer-throttling --disable-renderer-backgrounding";

fn trusted_local(window: &WebviewWindow, label: &str) -> Result<(), String> {
    let url = window.url().map_err(|_| "Cannot check window origin")?;
    if window.label() != label || !local_url(&url) {
        return Err("This operation is only available from the app interface".into());
    }
    Ok(())
}

fn local_url(url: &url::Url) -> bool {
    url.scheme() == "tauri"
        || (matches!(url.scheme(), "http" | "https") && url.host_str() == Some("tauri.localhost"))
}

#[tauri::command]
fn get_configuration(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<SettingsView, String> {
    trusted_local(&window, "main")?;
    let (settings, error) = match Settings::load(&state.root) {
        Ok(settings) => {
            let error = settings.secret().and_then(|s| settings.validate(&s)).err();
            (settings, error)
        }
        Err(_) if !state.root.join("settings.json").exists() => (Settings::default(), None),
        Err(error) => (Settings::default(), Some(error)),
    };
    let auto = state.auto_connect.swap(false, Ordering::SeqCst)
        && error.is_none()
        && state.root.join("settings.json").exists();
    Ok(SettingsView::new(settings, error, auto))
}

#[tauri::command]
async fn save_configuration(
    window: WebviewWindow,
    state: State<'_, AppState>,
    input: SaveRequest,
) -> Result<(), String> {
    trusted_local(&window, "main")?;
    let _operation = state.operation.lock().await;
    settings::save(&state.root, input)?;
    Ok(())
}

#[tauri::command]
async fn connect_saved(
    app: tauri::AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), String> {
    trusted_local(&window, "main")?;
    let _operation = state.operation.lock().await;
    let settings = Settings::load(&state.root)?;
    let servers = provider::resolve(&state.root, &settings).await?;
    open_parsec(&app, &state, &servers).await
}

#[tauri::command]
async fn connect_fallback(
    app: tauri::AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), String> {
    trusted_local(&window, "main")?;
    let _operation = state.operation.lock().await;
    let servers = provider::fallback(&state.root)?;
    open_parsec(&app, &state, &servers).await
}

async fn open_parsec(
    app: &tauri::AppHandle,
    state: &AppState,
    servers: &[ice::IceServer],
) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window("parsec") {
        existing.close().map_err(|e| e.to_string())?;
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while app.get_webview_window("parsec").is_some() {
            if Instant::now() >= deadline {
                return Err("Close the current Parsec window before reconnecting".into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
    *state.stats.lock().map_err(|_| "Statistics lock failed")? = LatestStats::default();
    let injection = include_str!("../../web/inject.js")
        .replace("__STATS_HELPER__", include_str!("../../web/stats.js"))
        .replace(
            "__ICE_SERVERS__",
            &serde_json::to_string(servers).map_err(|_| "Cannot encode ICE servers")?,
        );
    let profile = state.root.join("WebView2Profile");
    fs::create_dir_all(&profile).map_err(|e| format!("Cannot create WebView2 profile: {e}"))?;
    WebviewWindowBuilder::new(
        app,
        "parsec",
        WebviewUrl::External("https://web.parsec.app/".parse().unwrap()),
    )
    .title("Parsec — ParsecWebTurn")
    .inner_size(1280.0, 800.0)
    .min_inner_size(800.0, 500.0)
    .data_directory(profile)
    .initialization_script(injection)
    .additional_browser_args(BROWSER_ARGS)
    .general_autofill_enabled(false)
    .on_navigation(|url| url.scheme() == "https")
    .build()
    .map_err(|e| {
        format!("Cannot open Parsec. Ensure Microsoft Edge WebView2 Runtime is installed. {e}")
    })?;
    if let Some(settings) = app.get_webview_window("main") {
        let _ = settings.hide();
    }
    Ok(())
}

#[tauri::command]
fn report_stats(
    window: WebviewWindow,
    state: State<'_, AppState>,
    sample: ConnectionStats,
) -> Result<(), String> {
    if window.label() != "parsec"
        || window
            .url()
            .map_err(|_| "Cannot check Parsec origin")?
            .origin()
            .ascii_serialization()
            != "https://web.parsec.app"
    {
        return Err("Statistics must come from the Parsec webview".into());
    }
    sample.validate()?;
    let mut latest = state.stats.lock().map_err(|_| "Statistics lock failed")?;
    latest.value = sample;
    latest.received = Some(Instant::now());
    Ok(())
}

#[tauri::command]
fn get_stats(window: WebviewWindow, state: State<'_, AppState>) -> Result<ConnectionStats, String> {
    trusted_local(&window, "stats")?;
    Ok(state
        .stats
        .lock()
        .map_err(|_| "Statistics lock failed")?
        .snapshot())
}

fn show_settings(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn show_stats(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("stats") {
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let state = app.state::<AppState>();
    let _ = WebviewWindowBuilder::new(app, "stats", WebviewUrl::App("stats.html".into()))
        .title("ParsecWebTurn — Connection stats")
        .inner_size(470.0, 680.0)
        .resizable(false)
        .data_directory(state.root.join("WebView2Profile"))
        .additional_browser_args(BROWSER_ARGS)
        .on_navigation(local_url)
        .build();
}

#[tauri::command]
fn open_stats(app: tauri::AppHandle, window: WebviewWindow) -> Result<(), String> {
    trusted_local(&window, "main")?;
    show_stats(&app);
    Ok(())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut root = std::env::current_exe()?
        .parent()
        .ok_or("Cannot locate application")?
        .to_path_buf();
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let mut force_settings = false;
    let mut args = arguments.iter();
    while let Some(arg) = args.next() {
        if arg.eq_ignore_ascii_case("--settings") || arg.eq_ignore_ascii_case("/settings") {
            force_settings = true;
        } else if arg == "--data-dir" {
            root = PathBuf::from(args.next().ok_or("--data-dir requires a directory")?);
        } else {
            return Err(format!("Unknown argument: {arg}").into());
        }
    }
    fs::create_dir_all(&root)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(".launcher.lock"))?;
    lock.try_lock_exclusive().map_err(|_| "ParsecWebTurn is already running in this directory. Open Connection settings from its menu.")?;
    let settings_profile = root.join("WebView2Profile");
    fs::create_dir_all(&settings_profile)?;
    tauri::Builder::default()
        .manage(AppState {
            root,
            _instance_lock: lock,
            operation: tokio::sync::Mutex::new(()),
            stats: Mutex::new(LatestStats::default()),
            auto_connect: AtomicBool::new(!force_settings),
        })
        .invoke_handler(tauri::generate_handler![
            get_configuration,
            save_configuration,
            connect_saved,
            connect_fallback,
            get_stats,
            report_stats,
            open_stats
        ])
        .setup(move |app| {
            let settings =
                MenuItem::with_id(app, "settings", "Connection settings", true, Some("Ctrl+,"))?;
            let stats =
                MenuItem::with_id(app, "stats", "Connection stats", true, Some("Ctrl+Shift+S"))?;
            let devtools =
                MenuItem::with_id(app, "devtools", "Developer tools", true, Some("F12"))?;
            let exit = MenuItem::with_id(app, "exit", "Exit", true, None::<&str>)?;
            app.set_menu(Menu::with_items(
                app,
                &[&settings, &stats, &devtools, &exit],
            )?)?;
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("ParsecWebTurn — Connection settings")
                .inner_size(780.0, 820.0)
                .min_inner_size(660.0, 640.0)
                .data_directory(settings_profile)
                .additional_browser_args(BROWSER_ARGS)
                .general_autofill_enabled(false)
                .on_navigation(local_url)
                .build()?;
            Ok(())
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "settings" => show_settings(app),
            "stats" => show_stats(app),
            "devtools" => {
                if let Some(window) = app.get_webview_window("parsec") {
                    window.open_devtools();
                }
            }
            "exit" => app.exit(0),
            _ => {}
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. }
                if window.label() == "main"
                    && window.app_handle().get_webview_window("parsec").is_some() =>
            {
                api.prevent_close();
                let _ = window.hide();
            }
            tauri::WindowEvent::Destroyed if window.label() == "parsec" => {
                show_settings(window.app_handle())
            }
            tauri::WindowEvent::Destroyed if window.label() == "main" => {
                window.app_handle().exit(0)
            }
            _ => {}
        })
        .run(tauri::generate_context!())?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        #[link(name = "user32")]
        extern "system" {
            fn MessageBoxW(hwnd: isize, text: *const u16, title: *const u16, kind: u32) -> i32;
        }
        let message: Vec<u16> = error.to_string().encode_utf16().chain(Some(0)).collect();
        let title: Vec<u16> = "ParsecWebTurn".encode_utf16().chain(Some(0)).collect();
        unsafe {
            MessageBoxW(0, message.as_ptr(), title.as_ptr(), 0x10);
        }
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_pages_are_not_trusted_settings_origins() {
        assert!(local_url(
            &"http://tauri.localhost/index.html".parse().unwrap()
        ));
        assert!(!local_url(&"https://web.parsec.app/".parse().unwrap()));
        assert!(!local_url(
            &"https://tauri.localhost.evil.invalid/".parse().unwrap()
        ));
    }
}
