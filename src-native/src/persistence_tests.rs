//! Exercise the actual WASI bridge and DPAPI with synthetic data only.
use crate::*;

#[test]
fn wasi_libc_style_open_can_save_and_restore_an_encrypted_session() {
    let mut config = Config::new();
    config
        .wasm_threads(true)
        .consume_fuel(true)
        .epoch_interruption(true);
    let engine = Engine::new(&config).unwrap();
    let module = Module::new(&engine, r#"(module
        (import "env" "memory" (memory 1 1 shared))
        (import "wasi_snapshot_preview1" "fd_fdstat_get" (func $stat (param i32 i32) (result i32)))
        (import "wasi_snapshot_preview1" "path_open" (func $open (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
        (import "wasi_snapshot_preview1" "fd_write" (func $write (param i32 i32 i32 i32) (result i32)))
        (import "wasi_snapshot_preview1" "fd_read" (func $read (param i32 i32 i32 i32) (result i32)))
        (import "wasi_snapshot_preview1" "fd_close" (func $close (param i32) (result i32)))
        (data (i32.const 256) "session.bin")
        (data (i32.const 320) "synthetic-session")
        (func $file (param $flags i32) (result i32)
            i32.const 3 i32.const 64 call $stat
            if i32.const 1 return end
            i32.const 3 i32.const 0 i32.const 256 i32.const 11 local.get $flags
            i32.const 80 i64.load i64.const 70 i64.and
            i64.const 0 i32.const 0 i32.const 160 call $open)
        (func (export "save") (result i32)
            i32.const 9 call $file
            if i32.const 2 return end
            i32.const 128 i32.const 320 i32.store
            i32.const 132 i32.const 17 i32.store
            i32.const 160 i32.load i32.const 128 i32.const 1 i32.const 164 call $write
            if i32.const 3 return end
            i32.const 160 i32.load call $close)
        (func (export "load") (result i32)
            i32.const 0 call $file
            if i32.const 4 return end
            i32.const 128 i32.const 400 i32.store
            i32.const 132 i32.const 17 i32.store
            i32.const 160 i32.load i32.const 128 i32.const 1 i32.const 164 call $read
            if i32.const 5 return end
            i32.const 160 i32.load call $close))"#).unwrap();
    let directory = std::env::temp_dir().join(format!(
        "parsec-wasi-save-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    {
        let profile = profile::Profile::open(directory.clone()).unwrap();
        let (mut store, instance) = instantiate(&engine, &module).unwrap();
        let mut fs = profile.load().unwrap();
        fs.profile = Some(profile.clone());
        *store.data().filesystem.lock().unwrap() = fs;
        assert_eq!(
            instance
                .get_typed_func::<(), i32>(&mut store, "save")
                .unwrap()
                .call(&mut store, ())
                .unwrap(),
            0
        );
        assert_eq!(
            profile.load().unwrap().files["/session.bin"],
            b"synthetic-session"
        );
    }
    {
        let profile = profile::Profile::open(directory.clone()).unwrap();
        let (mut store, instance) = instantiate(&engine, &module).unwrap();
        *store.data().filesystem.lock().unwrap() = profile.load().unwrap();
        assert_eq!(
            instance
                .get_typed_func::<(), i32>(&mut store, "load")
                .unwrap()
                .call(&mut store, ())
                .unwrap(),
            0
        );
        assert_eq!(
            store.data().memory.read(400, 17).unwrap(),
            b"synthetic-session"
        );
    }
    std::fs::remove_file(directory.join("profile.dpapi")).unwrap();
    std::fs::remove_file(directory.join("profile.lock")).unwrap();
    std::fs::remove_dir(directory).unwrap();
}
