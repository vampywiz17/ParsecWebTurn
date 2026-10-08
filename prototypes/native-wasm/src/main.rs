mod host;
mod memory;

use anyhow::{bail, Context, Result};
use host::HostState;
use memory::GuestMemory;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf, time::Duration};
use wasmtime::{Config, Engine, ExternType, Func, Instance, Linker, Module, SharedMemory, Store};

const PINNED_SHA256: &str = "d663dd96df477c65479fb93eb88756c7fcafff581cc93be563625cd195a4b4a6";

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
    if mode == "help" || mode == "--help" {
        println!(
            "parsec-native-wasm <inspect|allocator|boot> <parsecd.wasm> [report.json]\n\
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
    };
    if mode != "inspect" {
        let (mut store, instance) = instantiate(&engine, &module)?;
        report.instantiated = true;
        allocator_roundtrip(&mut store, &instance)?;
        report.allocator_roundtrip = true;
        if mode == "boot" {
            let start = instance.get_typed_func::<(), ()>(&mut store, "_start")?;
            let outcome = start.call(&mut store, ());
            report.start_returned = outcome.is_ok();
            report.start_error = outcome.err().map(|e| format!("{e:#}"));
        }
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
    let mut store = Store::new(engine, HostState::new(GuestMemory(memory.clone())));
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
            8
        );
    }
}
