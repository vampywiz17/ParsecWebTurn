mod backend;
mod filesystem;
mod host;
mod memory;
mod poll;
mod threads;
mod transport;

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
    fn start() -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(15));
            if !flag.load(Ordering::SeqCst) {
                // Epoch interruption does not interrupt every blocking atomic
                // wait. This standalone CLI has no persistent writes/session
                // to clean up, so enforce a hard execution limit as well.
                eprintln!("WASM execution exceeded the 15-second process deadline");
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
    wasm_sha256: String,
    mode: String,
    imports: Vec<ImportInfo>,
    exports: Vec<String>,
    instantiated: bool,
    allocator_roundtrip: bool,
    start_returned: bool,
    start_error: Option<String>,
    network_enabled: bool,
    video_rendered: bool,
    host: Option<HostState>,
    threads: Vec<threads::ThreadRecord>,
    native_backend: Option<serde_json::Value>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "help".into());
    if mode == "transport-probe" {
        let path = args.next().map(PathBuf::from);
        if args.next().is_some() {
            bail!("too many arguments");
        }
        let report = transport::probe()?;
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
                  parsec-native-wasm transport-probe [report.json]\n\
                  Offline WASM host prototype. No browser, login, network or video renderer.\n\
                  boot reports the first unimplemented bridge; it is not a connected client."
        );
        return Ok(());
    }
    if !matches!(mode.as_str(), "inspect" | "allocator" | "boot") {
        bail!("unknown mode: {mode}");
    }
    let path = PathBuf::from(args.next().context("WASM path required")?);
    let report_path = args.next().map(PathBuf::from);
    if args.next().is_some() {
        bail!("too many arguments");
    }
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
        schema: 1,
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
                } else if host::disabled_web_stub(i.module(), i.name()) {
                    "unavailable-as-in-web-client"
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
        network_enabled: false,
        video_rendered: false,
        host: None,
        threads: Vec::new(),
        native_backend: None,
    };
    if mode != "inspect" {
        let _deadline = ExecutionDeadline::start();
        let (mut store, instance) = instantiate(&engine, &module)?;
        report.instantiated = true;
        allocator_roundtrip(&mut store, &instance)?;
        report.allocator_roundtrip = true;
        if mode == "boot" {
            let start = instance.get_typed_func::<(), ()>(&mut store, "_start")?;
            let outcome = start.call(&mut store, ());
            if outcome.is_err() {
                engine.increment_epoch();
            }
            report.start_returned = outcome.is_ok();
            report.start_error = outcome.err().map(|e| format!("{e:#}"));
        }
        if let Some(runtime) = &store.data().threads {
            // Let workers already at their boundary publish their records.
            // This does not wait indefinitely for guest threads.
            std::thread::sleep(Duration::from_millis(100));
            report.threads = runtime.snapshot();
        }
        report.native_backend = Some(serde_json::to_value(
            &*store
                .data()
                .backend
                .lock()
                .map_err(|_| anyhow::anyhow!("backend lock poisoned"))?,
        )?);
        report.host = Some(store.into_data());
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

fn instantiate(engine: &Engine, module: &Module) -> Result<(Store<HostState>, Instance)> {
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
    let runtime = Arc::new(threads::ThreadRuntime::new(
        engine.clone(),
        module.clone(),
        GuestMemory(memory),
    ));
    instantiate_with_runtime(runtime)
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
    host.started = runtime.started;
    let mut store = Store::new(engine, host);
    store.set_fuel(50_000_000)?;
    store.set_epoch_deadline(1);
    // Engine watchdog also bounds guest code that spends no fuel between epochs.
    let watchdog = engine.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(5));
        watchdog.increment_epoch();
    });
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
            (import "env" "parsec_web_get_self" (func $self (param i32 i32)))
            (import "env" "parsec_web_get_metrics" (func $metrics (param i32 i32 i32 i32 i32 i32 i32)))
            (func (export "init") call $init)
            (func (export "destroy") call $destroy)
            (func (export "status") (result i32) call $status)
            (func (export "guests") i32.const 100 i32.const 3 call $guests)
            (func (export "self") i32.const 104 i32.const 108 call $self)
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
        for name in ["guests", "self", "metrics"] {
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
            (import "env" "parsec_web_begin_p2p" (func $connect (result i32)))
            (func (export "probe") (result i32) call $connect))"#,
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
            Some("env::parsec_web_begin_p2p")
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
}
