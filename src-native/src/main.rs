#![cfg_attr(
    all(windows, not(feature = "diagnostics")),
    windows_subsystem = "windows"
)]
mod attempt;
const APP_TITLE: &str = "ParsecWebTurn";
#[cfg(any(test, feature = "diagnostics"))]
mod attempt_probe;
mod audio;
mod audio_stream;
#[cfg(windows)]
mod audio_windows;
#[cfg(any(test, feature = "diagnostics"))]
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
#[cfg(any(test, feature = "diagnostics"))]
mod execution_diagnostics;
mod filesystem;
#[cfg(windows)]
mod fullscreen;
#[cfg(windows)]
mod graphics;
mod host;
mod http;
#[cfg(any(test, feature = "diagnostics"))]
mod http_probe;
mod input;
mod lifecycle;
mod media_ingress;
mod memory;
mod network_audit;
mod network_policy;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod overlay_windows;
#[cfg(all(test, windows))]
mod persistence_tests;
mod platform;
#[cfg(any(test, feature = "diagnostics"))]
mod platform_probe;
#[cfg(windows)]
mod platform_windows;
mod poll;
#[cfg(any(test, feature = "diagnostics"))]
mod session_probe;
mod signaling;
#[cfg(any(test, feature = "diagnostics"))]
mod thread_probe;
mod threads;
#[cfg(any(test, feature = "diagnostics"))]
mod tls_probe;
mod transport;
#[cfg(any(test, feature = "diagnostics"))]
mod transport_diagnostic_errors;
#[cfg(any(test, feature = "diagnostics"))]
mod transport_diagnostics;
mod video_output;
mod video_stream;
#[cfg(windows)]
mod video_windows;
mod viewport;
mod wait;
#[cfg(windows)]
mod wake_lock;
mod websocket;
#[cfg(any(test, feature = "diagnostics"))]
mod websocket_probe;
#[cfg(windows)]
mod window;

use anyhow::{bail, Context, Result};
use host::HostState;
use memory::GuestMemory;
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use wasmtime::{Config, Engine, ExternType, Func, Instance, Linker, Module, SharedMemory, Store};
#[cfg(feature = "diagnostics")]
use {
    serde::Serialize,
    sha2::{Digest, Sha256},
    std::{env, fs, path::PathBuf, sync::atomic::AtomicBool},
};
const PINNED_SHA256: &str = "d663dd96df477c65479fb93eb88756c7fcafff581cc93be563625cd195a4b4a6";
#[cfg(feature = "diagnostics")]
include!("diagnostic_entry.rs");
#[cfg(all(windows, not(feature = "diagnostics")))]
mod client;
#[cfg(all(windows, any(test, not(feature = "diagnostics"))))]
mod profile;
fn main() {
    #[cfg(feature = "diagnostics")]
    diagnostic_main();
    #[cfg(all(windows, not(feature = "diagnostics")))]
    client::main();
}
#[cfg(any(test, feature = "diagnostics"))]
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
        #[cfg(any(test, feature = "diagnostics"))]
        if runtime.window.as_ref().is_some_and(|w| w.synthetic_login) {
            runtime.platform = Arc::new(platform_probe::offline_login_fixture());
        }
        if runtime.window.as_ref().is_some_and(|w| w.online) {
            #[cfg(any(test, feature = "diagnostics"))]
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

#[cfg(feature = "diagnostics")]
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
