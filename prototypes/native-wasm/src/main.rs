mod attempt;
mod attempt_probe;
mod audio;
mod audit_probe;
mod backend;
mod buffers;
mod control;
#[cfg(test)]
mod crypto_policy_tests;
#[cfg(windows)]
mod cursor;
#[cfg(test)]
mod data_only_policy_tests;
#[cfg(windows)]
mod desktop;
mod execution_diagnostics;
mod filesystem;
#[cfg(windows)]
mod fullscreen;
#[cfg(windows)]
mod graphics;
mod host;
mod http;
mod http_probe;
mod input;
mod lifecycle;
mod media_ingress;
mod memory;
mod native_media;
mod network_audit;
mod network_policy;
mod platform;
mod platform_probe;
#[cfg(windows)]
mod platform_windows;
mod poll;
mod session_probe;
mod signaling;
mod thread_probe;
mod threads;
mod tls_probe;
mod transport;
mod transport_diagnostic_errors;
mod transport_diagnostics;
mod wait;
#[cfg(windows)]
mod wake_lock;
mod websocket;
mod websocket_probe;
#[cfg(windows)]
mod window;

use anyhow::{bail, Context, Result};
use host::HostState;
use memory::GuestMemory;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use wasmtime::{Config, Engine, ExternType, Func, Instance, Linker, Module, SharedMemory, Store};

const PINNED_SHA256: &str = "d663dd96df477c65479fb93eb88756c7fcafff581cc93be563625cd195a4b4a6";

struct ExecutionDeadline(Arc<AtomicBool>);
impl ExecutionDeadline {
    fn start(seconds: u64) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(seconds));
            if !flag.load(Ordering::SeqCst) {
                // Epoch interruption does not interrupt every blocking atomic
                // wait. This standalone CLI has no persistent writes/session
                // to clean up, so enforce a hard execution limit as well.
                eprintln!("WASM execution exceeded the bounded process deadline");
                std::process::exit(124);
            }
        });
        Self(done)
    }
}
impl Drop for ExecutionDeadline {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[derive(Serialize)]
struct ImportInfo {
    module: String,
    name: String,
    signature: String,
    bridge: &'static str,
}
#[derive(Serialize)]
struct Report {
    schema: u32,
    prototype_version: &'static str,
    wasm_sha256: String,
    mode: String,
    imports: Vec<ImportInfo>,
    exports: Vec<String>,
    instantiated: bool,
    allocator_roundtrip: bool,
    start_returned: bool,
    start_error: Option<String>,
    network_enabled: bool,
    live_session: bool,
    shutdown_requested: bool,
    native_window_released: bool,
    #[cfg(windows)]
    native_wake_lock: Option<wake_lock::State>,
    video_rendered: bool,
    host: Option<HostState>,
    threads: Vec<threads::ThreadRecord>,
    thread_runtime: Option<threads::ThreadSummary>,
    native_backend: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    native_transport_diagnostics: Option<transport_diagnostics::Snapshot>,
    network_audit: Option<network_audit::Snapshot>,
    local_platform: Option<platform::Snapshot>,
    synthetic_login_steps: usize,
    #[cfg(windows)]
    graphics: Option<graphics::GraphicsReport>,
}

fn main() {
    // The native compiler can require more stack in diagnostic Rust builds.
    // Reserve a bounded runtime-thread stack; the main thread only waits.
    // Wasmtime's guest stack/fuel/epoch limits remain independently enforced.
    let outcome = std::thread::Builder::new()
        .name("parsec-native-runtime".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(run)
        .map_err(anyhow::Error::from)
        .and_then(|thread| {
            thread
                .join()
                .map_err(|_| anyhow::anyhow!("Native runtime thread panicked"))
        })
        .and_then(|result| result);
    if let Err(error) = outcome {
        if matches!(
            env::args().nth(1).as_deref(),
            Some("account" | "account-network-audit" | "session-audit")
        ) {
            eprintln!("Native account runtime failed; guest content is not logged");
        } else {
            eprintln!("{error:#}");
        }
        std::process::exit(1);
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
struct AccountOptions {
    cloudflare_stun: bool,
    legacy_rsa_1024: bool,
}

fn parse_account_options(flags: impl Iterator<Item = String>) -> Result<AccountOptions> {
    let mut options = AccountOptions::default();
    for flag in flags {
        match flag.as_str() {
            "--cloudflare-stun" if !options.cloudflare_stun => options.cloudflare_stun = true,
            "--legacy-rsa-1024" if !options.legacy_rsa_1024 => options.legacy_rsa_1024 = true,
            _ => bail!("unsupported or duplicate account option"),
        }
    }
    Ok(options)
}

#[test]
fn account_options_require_explicit_unique_selection() {
    let parse = |flags: &[&str]| parse_account_options(flags.iter().map(|s| s.to_string()));
    assert_eq!(parse(&[]).unwrap(), AccountOptions::default());
    assert!(parse(&["--cloudflare-stun"]).unwrap().cloudflare_stun);
    assert!(!parse(&["--cloudflare-stun"]).unwrap().legacy_rsa_1024);
    assert!(parse(&["--legacy-rsa-1024"]).unwrap().legacy_rsa_1024);
    assert!(!parse(&["--legacy-rsa-1024"]).unwrap().cloudflare_stun);
    assert_eq!(
        parse(&["--cloudflare-stun", "--legacy-rsa-1024"]).unwrap(),
        parse(&["--legacy-rsa-1024", "--cloudflare-stun"]).unwrap()
    );
    assert!(parse(&["--turn"]).is_err());
    assert!(parse(&["--legacy-rsa-1024", "--legacy-rsa-1024"]).is_err());
    assert!(parse(&["--cloudflare-stun", "--cloudflare-stun"]).is_err());
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "help".into());
    let account_mode = matches!(mode.as_str(), "account" | "account-network-audit");
    let live_mode = account_mode || mode == "session-audit";
    let window_mode =
        live_mode || matches!(mode.as_str(), "window" | "window-audit" | "login-audit");
    let audit_mode = live_mode || matches!(mode.as_str(), "window-audit" | "login-audit");
    if mode == "media-probe" {
        let path = args.next().map(PathBuf::from);
        if args.next().is_some() {
            bail!("too many arguments");
        }
        let json = serde_json::to_string_pretty(&native_media::probe()?)?;
        if let Some(path) = path {
            fs::write(path, &json)?;
        }
        println!("{json}");
        return Ok(());
    }
    if matches!(
        mode.as_str(),
        "guest-offer-probe"
            | "guest-session-probe"
            | "guest-dtls-failure-probe"
            | "guest-legacy-dtls-failure-probe"
            | "guest-control-probe"
            | "guest-buffer-probe"
            | "guest-http-probe"
            | "guest-websocket-probe"
            | "guest-tls-probe"
            | "guest-audit-probe"
            | "guest-thread-probe"
            | "guest-audio-unavailable-probe"
            | "guest-platform-probe"
            | "guest-window-probe"
            | "guest-wake-lock-probe"
            | "guest-execution-diagnostic-probe"
    ) {
        let path = args.next().map(PathBuf::from);
        if args.next().is_some() {
            bail!("too many arguments");
        }
        let report = if mode == "guest-execution-diagnostic-probe" {
            execution_diagnostics::probe()?
        } else if mode == "guest-wake-lock-probe" {
            #[cfg(windows)]
            {
                wake_lock::probe()?
            }
            #[cfg(not(windows))]
            {
                bail!("wake lock probe requires Windows");
            }
        } else if mode == "guest-window-probe" {
            #[cfg(windows)]
            {
                fullscreen::probe()?
            }
            #[cfg(not(windows))]
            {
                bail!("native window probe requires Windows");
            }
        } else if mode == "guest-audio-unavailable-probe" {
            audio::probe()?
        } else if mode == "guest-platform-probe" {
            platform_probe::probe()?
        } else if mode == "guest-thread-probe" {
            thread_probe::probe()?
        } else if mode == "guest-audit-probe" {
            audit_probe::probe()?
        } else if mode == "guest-tls-probe" {
            tls_probe::probe()?
        } else if mode == "guest-websocket-probe" {
            websocket_probe::probe()?
        } else if mode == "guest-http-probe" {
            http_probe::probe()?
        } else if mode == "guest-buffer-probe" {
            session_probe::buffer_probe()?
        } else if mode == "guest-control-probe" {
            session_probe::control_probe()?
        } else if matches!(
            mode.as_str(),
            "guest-dtls-failure-probe" | "guest-legacy-dtls-failure-probe"
        ) {
            session_probe::dtls_failure_probe(mode == "guest-legacy-dtls-failure-probe")?
        } else if mode == "guest-session-probe" {
            session_probe::probe()?
        } else {
            attempt_probe::probe()?
        };
        let json = serde_json::to_string_pretty(&report)?;
        if let Some(path) = path {
            fs::write(path, &json)?;
        }
        println!("{json}");
        return Ok(());
    }
    if matches!(mode.as_str(), "transport-probe" | "signaling-probe") {
        let path = args.next().map(PathBuf::from);
        if args.next().is_some() {
            bail!("too many arguments");
        }
        let report = if mode == "signaling-probe" {
            transport::signaling_probe()?
        } else {
            transport::probe()?
        };
        let json = serde_json::to_string_pretty(&report)?;
        if let Some(path) = path {
            fs::write(path, &json)?;
        }
        println!("{json}");
        return Ok(());
    }
    if mode == "help" || mode == "--help" {
        println!(
            "parsec-native-wasm <inspect|allocator|boot> <parsecd.wasm> [report.json]\n\
                  parsec-native-wasm window <parsecd.wasm> [report.json]\n\
                  parsec-native-wasm transport-probe [report.json]\n\
                  parsec-native-wasm signaling-probe [report.json]\n\
                  parsec-native-wasm media-probe [report.json]\n\
                  parsec-native-wasm guest-offer-probe [report.json]\n\
                  parsec-native-wasm guest-window-probe [report.json]\n\
                  parsec-native-wasm guest-session-probe [report.json]\n\
                  parsec-native-wasm guest-dtls-failure-probe [report.json]\n\
                  parsec-native-wasm guest-control-probe [report.json]\n\
                  parsec-native-wasm guest-buffer-probe [report.json]\n\
                  parsec-native-wasm guest-http-probe [report.json]\n\
                  parsec-native-wasm guest-websocket-probe [report.json]\n\
                  parsec-native-wasm guest-tls-probe [report.json]\n\
                  parsec-native-wasm guest-audit-probe [report.json]\n\
                  parsec-native-wasm guest-thread-probe [report.json]\n\
                  parsec-native-wasm guest-platform-probe [report.json]\n\
                  parsec-native-wasm window-audit <parsecd.wasm> [report.json]\n\
                  parsec-native-wasm login-audit <parsecd.wasm> [report.json]\n\
                  parsec-native-wasm account <parsecd.wasm> [report.json] [--cloudflare-stun] [--legacy-rsa-1024]\n\
                  parsec-native-wasm account-network-audit <parsecd.wasm> [report.json] [--cloudflare-stun] [--legacy-rsa-1024]\n\
                  parsec-native-wasm session-audit <parsecd.wasm> [report.json]\n\
                  Account modes enable exact HTTPS/WSS origins and run until close. No decoded remote video.\n\
                  account-network-audit also reports destination origins (no URL tokens).\n\
                  Account modes accept --cloudflare-stun after the report path (STUN only).\n\
                  boot reports the first unimplemented bridge; it is not a connected client."
        );
        return Ok(());
    }
    if !matches!(
        mode.as_str(),
        "inspect"
            | "allocator"
            | "boot"
            | "window"
            | "window-audit"
            | "login-audit"
            | "account"
            | "account-network-audit"
            | "session-audit"
    ) {
        bail!("unknown mode: {mode}");
    }
    #[cfg(not(windows))]
    if window_mode {
        bail!("The native window prototype currently requires Windows");
    }
    let path = PathBuf::from(args.next().context("WASM path required")?);
    let report_path = args.next().map(PathBuf::from);
    let account_options = if account_mode {
        parse_account_options(args.by_ref())?
    } else {
        AccountOptions::default()
    };
    #[cfg(windows)]
    let capture_path = if mode == "window" {
        args.next().map(PathBuf::from)
    } else {
        None
    };
    if args.next().is_some() {
        bail!("too many arguments");
    }
    // Supplementary pinned-library diagnostics are explicit opt-in only.
    let transport_logger = (mode == "account-network-audit").then(transport_diagnostics::enable);
    let bytes = fs::read(&path).context("reading WASM")?;
    let hash = format!("{:x}", Sha256::digest(&bytes));
    if hash != PINNED_SHA256 {
        bail!("WASM SHA-256 mismatch; audit a new binary before changing the pin");
    }
    let mut config = Config::new();
    config
        .wasm_threads(true)
        .consume_fuel(true)
        .epoch_interruption(true);
    let engine = Engine::new(&config)?;
    let module = Module::new(&engine, &bytes).context("compiling the original WASM")?;
    let mut report = Report {
        schema: 3,
        prototype_version: env!("CARGO_PKG_VERSION"),
        wasm_sha256: hash,
        mode: mode.clone(),
        imports: module
            .imports()
            .map(|i| ImportInfo {
                module: i.module().into(),
                name: i.name().into(),
                signature: format!("{:?}", i.ty()),
                bridge: if matches!(i.ty(), ExternType::Memory(_)) {
                    "shared-memory"
                } else if window_mode && native_window_import(i.module(), i.name()) {
                    "native-window"
                } else if host::disabled_web_stub(i.module(), i.name()) {
                    "unavailable-as-in-web-client"
                } else if i.module() == "env"
                    && matches!(i.name(), "web_set_pointer_lock" | "web_set_kb_grab")
                {
                    "inactive-release-only"
                } else if i.module() == "env" && audio::handles(i.name()) {
                    "audio-output-unavailable"
                } else if host::implemented(i.module(), i.name()) {
                    "implemented"
                } else {
                    "trap-on-call"
                },
            })
            .collect(),
        exports: module
            .exports()
            .map(|e| format!("{}: {:?}", e.name(), e.ty()))
            .collect(),
        instantiated: false,
        allocator_roundtrip: false,
        start_returned: false,
        start_error: None,
        network_enabled: account_mode,
        live_session: live_mode,
        shutdown_requested: false,
        native_window_released: false,
        #[cfg(windows)]
        native_wake_lock: None,
        video_rendered: false,
        host: None,
        threads: Vec::new(),
        thread_runtime: None,
        native_backend: None,
        native_transport_diagnostics: None,
        network_audit: None,
        local_platform: None,
        synthetic_login_steps: 0,
        #[cfg(windows)]
        graphics: None,
    };
    if mode != "inspect" {
        let _deadline = if live_mode {
            None
        } else {
            Some(ExecutionDeadline::start(if mode == "login-audit" {
                30
            } else {
                15
            }))
        };
        #[cfg(windows)]
        let native_window = if window_mode {
            Some(window::Window::create(
                mode == "login-audit",
                live_mode,
                account_mode,
                mode == "account-network-audit",
            )?)
        } else {
            None
        };
        #[cfg(windows)]
        if let Some(window) = &native_window {
            *window.capture.lock().unwrap_or_else(|e| e.into_inner()) = capture_path;
            if live_mode {
                let stop = window.stop.clone();
                // Blocking guest atomic waits cannot always be interrupted.
                // Bound shutdown only, never the interactive account lifetime.
                std::thread::spawn(move || {
                    stop.wait();
                    std::thread::sleep(Duration::from_secs(10));
                    eprintln!("Native session shutdown deadline reached");
                    std::process::exit(124);
                });
                if mode == "session-audit" {
                    let window = window.clone();
                    std::thread::spawn(move || {
                        if !window.stop.wait_timeout(Duration::from_secs(35)) {
                            window.request_stop();
                        }
                    });
                }
            }
        }
        #[cfg(windows)]
        let (mut store, instance) = instantiate_mode(&engine, &module, native_window.clone())?;
        #[cfg(not(windows))]
        let (mut store, instance) = instantiate(&engine, &module)?;
        report.instantiated = true;
        allocator_roundtrip(&mut store, &instance)?;
        report.allocator_roundtrip = true;
        {
            let mut backend = store
                .data()
                .backend
                .lock()
                .map_err(|_| anyhow::anyhow!("backend lock poisoned"))?;
            backend.cloudflare_stun_enabled = account_options.cloudflare_stun;
            backend.legacy_rsa_1024_enabled = account_options.legacy_rsa_1024;
        }
        if mode == "boot" || window_mode {
            let start = instance.get_typed_func::<(), ()>(&mut store, "_start")?;
            store.data_mut().execution_stage = Some("guest-start");
            let outcome = start.call(&mut store, ());
            #[cfg(windows)]
            let handed_off = window_mode && store.data().event_loop.is_some();
            #[cfg(not(windows))]
            let handed_off = false;
            if outcome.is_err() && !handed_off {
                engine.increment_epoch();
            }
            report.start_returned = outcome.is_ok();
            if !handed_off {
                if let Err(error) = &outcome {
                    store.data_mut().execution_failure =
                        Some(execution_diagnostics::Failure::capture(
                            error,
                            store.data().execution_stage,
                        ));
                }
            }
            report.start_error = outcome
                .err()
                .filter(|e| !lifecycle::is_cancellation(e))
                .map(|e| format!("{e:#}"));
            #[cfg(windows)]
            if handed_off {
                let result = desktop::run(&mut store, &instance);
                if let Err(error) = &result {
                    store.data_mut().execution_failure =
                        Some(execution_diagnostics::Failure::capture(
                            error,
                            store.data().execution_stage,
                        ));
                }
                report.start_error = result
                    .err()
                    .filter(|e| !lifecycle::is_cancellation(e))
                    .map(|e| format!("{e:#}"));
                engine.increment_epoch();
            }
        }
        #[cfg(windows)]
        if let Some(window) = &native_window {
            // The render worker owns its WGL context and releases it before
            // the HWND is destroyed. Never destroy a live context's window.
            window.request_stop();
            engine.increment_epoch();
            report.shutdown_requested = true;
            if let Some(runtime) = &store.data().threads {
                runtime.http.shutdown();
                runtime.websocket.shutdown();
            }
            let until = std::time::Instant::now() + Duration::from_secs(2);
            while window.active_contexts.load(Ordering::Acquire) != 0
                && std::time::Instant::now() < until
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            report.graphics = window
                .graphics
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            report.synthetic_login_steps = window.script_steps.load(Ordering::Acquire);
            if window.active_contexts.load(Ordering::Acquire) == 0 {
                window.close();
                let until = std::time::Instant::now() + Duration::from_secs(1);
                while !window.handle().is_null() && std::time::Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(10));
                }
                report.native_window_released = window.handle().is_null();
                report.native_wake_lock = Some(window.wake_lock_snapshot());
            }
        }
        if let Some(runtime) = &store.data().threads {
            // Let workers already at their boundary publish their records.
            // This does not wait indefinitely for guest threads.
            std::thread::sleep(Duration::from_millis(100));
            report.threads = runtime.snapshot();
            report.thread_runtime = Some(runtime.summary());
            report.network_audit = Some(runtime.audit.snapshot());
            report.local_platform = Some(runtime.platform.snapshot());
        }
        report.native_backend = Some(
            store
                .data()
                .backend
                .lock()
                .map_err(|_| anyhow::anyhow!("backend lock poisoned"))?
                .diagnostic()?,
        );
        report.host = Some(store.into_data());
    }
    report.native_transport_diagnostics = transport_logger.map(transport_diagnostics::snapshot);
    if audit_mode {
        // Keep the dedicated account-flow audit independent of guest strings,
        // window titles, filenames and potentially sensitive exception details.
        if report.start_error.is_some() {
            report.start_error = Some("guest-execution-failed".into());
        }
        for thread in &mut report.threads {
            if thread.error.is_some() {
                thread.error = Some("guest-worker-failed".into());
            }
        }
        if let Some(host) = &mut report.host {
            host.title = None;
            host.filesystem_requests.clear();
        }
    }
    let json = serde_json::to_string_pretty(&report)?;
    if let Some(path) = report_path {
        fs::write(path, &json)?;
    }
    println!("{json}");
    // A boot boundary is a successful diagnostic, not a successful app launch.
    // Consumers must check start_error, video_rendered and the bridge statuses.
    Ok(())
}

fn native_window_import(module: &str, name: &str) -> bool {
    #[cfg(windows)]
    {
        module == "env" && desktop::handles(name)
    }
    #[cfg(not(windows))]
    {
        let _ = (module, name);
        false
    }
}

fn instantiate(engine: &Engine, module: &Module) -> Result<(Store<HostState>, Instance)> {
    #[cfg(windows)]
    {
        instantiate_mode(engine, module, None)
    }
    #[cfg(not(windows))]
    {
        instantiate_base_with_window(engine, module)
    }
}

#[cfg(windows)]
fn instantiate_mode(
    engine: &Engine,
    module: &Module,
    window: Option<Arc<window::Window>>,
) -> Result<(Store<HostState>, Instance)> {
    instantiate_base_with_window(engine, module, window)
}

fn instantiate_base_with_window(
    engine: &Engine,
    module: &Module,
    #[cfg(windows)] window: Option<Arc<window::Window>>,
) -> Result<(Store<HostState>, Instance)> {
    let ty = module
        .imports()
        .find_map(|i| match i.ty() {
            ExternType::Memory(m) => Some(m),
            _ => None,
        })
        .context("shared memory import missing")?;
    if !ty.is_shared() {
        bail!("expected shared memory");
    }
    let memory = SharedMemory::new(engine, ty)?;
    let runtime = threads::ThreadRuntime::new(engine.clone(), module.clone(), GuestMemory(memory));
    #[cfg(windows)]
    let runtime = {
        let mut runtime = threads::ThreadRuntime { window, ..runtime };
        if runtime.window.as_ref().is_some_and(|w| w.synthetic_login) {
            runtime.platform = Arc::new(platform_probe::offline_login_fixture());
        }
        if runtime.window.as_ref().is_some_and(|w| w.online) {
            if runtime
                .window
                .as_ref()
                .is_some_and(|w| w.network_origin_audit)
            {
                runtime.audit = Arc::new(network_audit::Audit::with_destination_origins());
            }
            let owner = runtime.window.as_ref().unwrap().handle() as usize;
            runtime.platform = Arc::new(platform::Services::new(Box::new(
                platform_windows::NativeDesktop(owner),
            )));
            runtime.http = Arc::new(http::Network::account(runtime.audit.clone()));
            runtime.websocket = Arc::new(websocket::Network::account(runtime.audit.clone()));
        }
        runtime
    };
    instantiate_with_runtime(Arc::new(runtime))
}

fn instantiate_with_runtime(
    runtime: Arc<threads::ThreadRuntime>,
) -> Result<(Store<HostState>, Instance)> {
    let engine = &runtime.engine;
    let module = &runtime.module;
    let memory = runtime.memory.0.clone();
    let mut host = HostState::new(runtime.memory.clone());
    host.threads = Some(runtime.clone());
    host.filesystem = runtime.filesystem.clone();
    host.backend = runtime.backend.clone();
    host.http = runtime.http.clone();
    host.websocket = runtime.websocket.clone();
    host.platform = runtime.platform.clone();
    host.started = runtime.started;
    #[cfg(windows)]
    {
        host.window = runtime.window.clone();
    }
    let mut store = Store::new(engine, host);
    store.set_fuel(50_000_000)?;
    store.set_epoch_deadline(1);
    #[cfg(windows)]
    let live_window = runtime.window.as_ref().filter(|window| window.live);
    #[cfg(windows)]
    if let Some(window) = live_window {
        lifecycle::configure_store(&mut store, window.stop.clone());
        if !runtime.watchdog_started.swap(true, Ordering::AcqRel) {
            let watchdog = engine.clone();
            let stop = window.stop.clone();
            std::thread::spawn(move || {
                while !stop.wait_timeout(Duration::from_millis(100)) {
                    watchdog.increment_epoch();
                }
                watchdog.increment_epoch();
            });
        }
    }
    // Engine watchdog also bounds guest code that spends no fuel between epochs.
    let watchdog = engine.clone();
    #[cfg(windows)]
    let seconds = runtime
        .window
        .as_ref()
        .map_or(5, |window| window.run_seconds + 4);
    #[cfg(not(windows))]
    let seconds = 5;
    #[cfg(windows)]
    let bounded = live_window.is_none();
    #[cfg(not(windows))]
    let bounded = true;
    if bounded && !runtime.watchdog_started.swap(true, Ordering::AcqRel) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(seconds));
            watchdog.increment_epoch();
        });
    }
    let mut linker = Linker::new(engine);
    for import in module.imports() {
        match import.ty() {
            ExternType::Memory(_) => {
                linker.define(&store, import.module(), import.name(), memory.clone())?;
            }
            ExternType::Func(ty) => {
                let namespace = import.module().to_owned();
                let name = import.name().to_owned();
                let func = Func::new(&mut store, ty, move |caller, args, results| {
                    host::dispatch(caller, &namespace, &name, args, results)
                });
                linker.define(&store, import.module(), import.name(), func)?;
            }
            other => bail!("unsupported import type: {other:?}"),
        }
    }
    let instance = linker.instantiate(&mut store, module)?;
    Ok((store, instance))
}

fn allocator_roundtrip(store: &mut Store<HostState>, instance: &Instance) -> Result<()> {
    let alloc = instance.get_typed_func::<(i32, i32), i32>(&mut *store, "mty_system_alloc")?;
    let free = instance.get_typed_func::<i32, ()>(&mut *store, "mty_system_free")?;
    let pointer = alloc.call(&mut *store, (32, 1))?;
    if pointer == 0 {
        bail!("guest allocation failed");
    }
    let memory = store.data().memory.clone();
    memory.c_string(pointer as u32, 32, "Rust host / Parsec WASM")?;
    let text = memory.string(pointer as u32, 32)?;
    free.call(&mut *store, pointer)?;
    if text != "Rust host / Parsec WASM" {
        bail!("guest allocation roundtrip mismatch");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_backend_bridge_is_shared_and_preserves_guest_abi() {
        let mut config = Config::new();
        config
            .wasm_threads(true)
            .consume_fuel(true)
            .epoch_interruption(true);
        let engine = Engine::new(&config).unwrap();
        let module = Module::new(&engine, r#"(module
            (import "env" "memory" (memory 1 1 shared))
            (import "env" "parsec_web_init" (func $init))
            (import "env" "parsec_web_destroy" (func $destroy))
            (import "env" "parsec_web_get_status" (func $status (result i32)))
            (import "env" "parsec_web_get_guests" (func $guests (param i32 i32)))
            (import "env" "parsec_web_get_self" (func $self (param i32 i32) (result i32)))
            (import "env" "parsec_web_get_metrics" (func $metrics (param i32 i32 i32 i32 i32 i32 i32)))
            (func (export "init") call $init)
            (func (export "destroy") call $destroy)
            (func (export "status") (result i32) call $status)
            (func (export "guests") i32.const 100 i32.const 3 call $guests)
            (func (export "self") (result i32) i32.const 104 i32.const 108 call $self)
            (func (export "metrics") i32.const 120 i32.const 124 i32.const 128 i32.const 129
                i32.const 132 i32.const 136 i32.const 140 call $metrics))"#).unwrap();
        let (mut main, instance) = instantiate(&engine, &module).unwrap();
        let runtime = main.data().threads.clone().unwrap();
        let (mut worker, other) = instantiate_with_runtime(runtime).unwrap();
        instance
            .get_typed_func::<(), ()>(&mut main, "init")
            .unwrap()
            .call(&mut main, ())
            .unwrap();
        assert_eq!(
            other
                .get_typed_func::<(), i32>(&mut worker, "status")
                .unwrap()
                .call(&mut worker, ())
                .unwrap(),
            -3
        );
        let m = main.data().memory.clone();
        m.write(100, &[255; 44]).unwrap();
        assert_eq!(
            instance
                .get_typed_func::<(), i32>(&mut main, "self")
                .unwrap()
                .call(&mut main, ())
                .unwrap(),
            0
        );
        for name in ["guests", "metrics"] {
            instance
                .get_typed_func::<(), ()>(&mut main, name)
                .unwrap()
                .call(&mut main, ())
                .unwrap();
        }
        assert_eq!(m.read(100, 4).unwrap(), b"[]\0\xff");
        assert_eq!(m.read(104, 4).unwrap(), vec![0, 255, 255, 255]);
        assert_eq!(m.u32(108).unwrap(), 0);
        assert_eq!(m.read(128, 4).unwrap(), vec![0, 0, 255, 255]);
        assert_eq!(m.u32(140).unwrap(), 0);
        other
            .get_typed_func::<(), ()>(&mut worker, "destroy")
            .unwrap()
            .call(&mut worker, ())
            .unwrap();
        assert!(instance
            .get_typed_func::<(), i32>(&mut main, "status")
            .unwrap()
            .call(&mut main, ())
            .is_err());
    }

    #[test]
    fn unsupported_bridges_trap_instead_of_claiming_success() {
        let mut config = Config::new();
        config
            .wasm_threads(true)
            .consume_fuel(true)
            .epoch_interruption(true);
        let engine = Engine::new(&config).unwrap();
        let module = Module::new(
            &engine,
            r#"(module
            (import "env" "memory" (memory 1 1 shared))
            (import "env" "MTY_DecompressImage" (func $connect (param i32 i32 i32 i32) (result i32)))
            (func (export "probe") (result i32)
                i32.const 0 i32.const 0 i32.const 0 i32.const 0 call $connect))"#,
        )
        .unwrap();
        let (mut store, instance) = instantiate(&engine, &module).unwrap();
        assert!(instance
            .get_typed_func::<(), i32>(&mut store, "probe")
            .unwrap()
            .call(&mut store, ())
            .is_err());
        assert_eq!(
            store.data().boundary.as_deref(),
            Some("env::MTY_DecompressImage")
        );
    }

    #[test]
    fn wasi_host_does_not_expose_local_environment_or_files() {
        let mut config = Config::new();
        config
            .wasm_threads(true)
            .consume_fuel(true)
            .epoch_interruption(true);
        let engine = Engine::new(&config).unwrap();
        let module = Module::new(&engine, r#"(module
            (import "env" "memory" (memory 1 1 shared))
            (import "wasi_snapshot_preview1" "environ_sizes_get" (func $env (param i32 i32) (result i32)))
            (import "wasi_snapshot_preview1" "fd_prestat_get" (func $file (param i32 i32) (result i32)))
            (func (export "env") (result i32) i32.const 0 i32.const 4 call $env)
            (func (export "file") (result i32) i32.const 3 i32.const 16 call $file))"#).unwrap();
        let (mut store, instance) = instantiate(&engine, &module).unwrap();
        assert_eq!(
            instance
                .get_typed_func::<(), i32>(&mut store, "env")
                .unwrap()
                .call(&mut store, ())
                .unwrap(),
            0
        );
        assert_eq!(store.data().memory.read(0, 8).unwrap(), vec![0; 8]);
        assert_eq!(
            instance
                .get_typed_func::<(), i32>(&mut store, "file")
                .unwrap()
                .call(&mut store, ())
                .unwrap(),
            0
        );
        assert_eq!(
            store.data().memory.read(16, 8).unwrap(),
            vec![0, 0, 0, 0, 1, 0, 0, 0]
        );
    }

    #[test]
    fn wasi_thread_enters_a_new_instance_with_shared_memory() {
        let mut config = Config::new();
        config
            .wasm_threads(true)
            .consume_fuel(true)
            .epoch_interruption(true);
        let engine = Engine::new(&config).unwrap();
        let module = Module::new(
            &engine,
            r#"(module
            (import "env" "memory" (memory 1 1 shared))
            (import "wasi" "thread-spawn" (func $spawn (param i32) (result i32)))
            (func (export "probe") (result i32) i32.const 100 call $spawn)
            (func (export "wasi_thread_start") (param $id i32) (param $arg i32)
                local.get $arg i32.const 42 i32.atomic.store))"#,
        )
        .unwrap();
        let (mut store, instance) = instantiate(&engine, &module).unwrap();
        let id = instance
            .get_typed_func::<(), i32>(&mut store, "probe")
            .unwrap()
            .call(&mut store, ())
            .unwrap();
        assert_eq!(id, 2);
        let runtime = store.data().threads.clone().unwrap();
        let limit = std::time::Instant::now() + Duration::from_secs(2);
        while !runtime.snapshot()[0].finished && std::time::Instant::now() < limit {
            std::thread::sleep(Duration::from_millis(5));
        }
        let record = runtime.snapshot().remove(0);
        assert!(record.finished);
        assert!(record.error.is_none(), "{:?}", record.error);
        assert_eq!(store.data().memory.u32(100).unwrap(), 42);
    }

    #[test]
    fn hostname_bridge_allocates_the_nul_terminator() {
        let mut config = Config::new();
        config
            .wasm_threads(true)
            .consume_fuel(true)
            .epoch_interruption(true);
        let engine = Engine::new(&config).unwrap();
        let module = Module::new(
            &engine,
            r#"(module
            (import "env" "memory" (memory 1 1 shared))
            (import "env" "web_get_hostname" (func $hostname (result i32)))
            (func (export "mty_system_alloc") (param $size i32) (param i32) (result i32)
                i32.const 200 local.get $size i32.store i32.const 256)
            (func (export "probe") (result i32) call $hostname))"#,
        )
        .unwrap();
        let (mut store, instance) = instantiate(&engine, &module).unwrap();
        let pointer = instance
            .get_typed_func::<(), i32>(&mut store, "probe")
            .unwrap()
            .call(&mut store, ())
            .unwrap();
        assert_eq!(pointer, 256);
        assert_eq!(store.data().memory.u32(200).unwrap(), 15);
        assert_eq!(
            store.data().memory.string(pointer as u32, 15).unwrap(),
            "web.parsec.app"
        );
    }

    #[test]
    fn optional_web_maintenance_returns_an_unavailable_handle() {
        let mut config = Config::new();
        config
            .wasm_threads(true)
            .consume_fuel(true)
            .epoch_interruption(true);
        let engine = Engine::new(&config).unwrap();
        let module = Module::new(
            &engine,
            r#"(module
            (import "env" "memory" (memory 1 1 shared))
            (import "env" "maintenance_create" (func $maintenance (result i32)))
            (func (export "probe") (result i32) call $maintenance))"#,
        )
        .unwrap();
        let (mut store, instance) = instantiate(&engine, &module).unwrap();
        assert_eq!(
            instance
                .get_typed_func::<(), i32>(&mut store, "probe")
                .unwrap()
                .call(&mut store, ())
                .unwrap(),
            0
        );
        assert!(store.data().boundary.is_none());
    }

    #[test]
    fn pinned_web_crypto_hash_is_void_and_leaves_memory_untouched() {
        let mut config = Config::new();
        config
            .wasm_threads(true)
            .consume_fuel(true)
            .epoch_interruption(true);
        let engine = Engine::new(&config).unwrap();
        let module = Module::new(
            &engine,
            r#"(module
            (import "env" "memory" (memory 1 1 shared))
            (import "env" "MTY_CryptoHash" (func $hash
                (param i32 i32 i32 i32 i32 i32 i32)))
            (func (export "probe")
                i32.const 1 i32.const 64 i32.const 4
                i32.const 80 i32.const 4 i32.const 96 i32.const 32 call $hash))"#,
        )
        .unwrap();
        let (mut store, instance) = instantiate(&engine, &module).unwrap();
        let memory = store.data().memory.clone();
        let before = vec![0xA5; 128];
        memory.write(0, &before).unwrap();
        instance
            .get_typed_func::<(), ()>(&mut store, "probe")
            .unwrap()
            .call(&mut store, ())
            .unwrap();
        assert_eq!(memory.read(0, before.len()).unwrap(), before);
        assert!(store.data().boundary.is_none());
        assert_eq!(store.data().calls["env::MTY_CryptoHash"], 1);
    }
}
