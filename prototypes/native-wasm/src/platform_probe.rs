//! Actual guest imports with a synthetic desktop. Never accesses the OS clipboard
//! or launches a browser, even when run on a signed-in user's computer.
use anyhow::{bail, Result};
use std::sync::{Arc, Mutex};
use wasmtime::{Config, Engine, Module};

struct Fixture(Mutex<String>);
impl crate::platform::Desktop for Fixture {
    fn read_text(&self) -> Option<String> {
        Some(self.0.lock().unwrap().clone())
    }
    fn write_text(&self, text: &str) -> bool {
        *self.0.lock().unwrap() = text.into();
        true
    }
    fn open_url(&self, _: &str) -> bool {
        true
    }
    fn alert(&self, _: &str, _: &str) -> bool {
        true
    }
}

pub fn probe() -> Result<serde_json::Value> {
    let mut config = Config::new();
    config
        .wasm_threads(true)
        .consume_fuel(true)
        .epoch_interruption(true);
    let engine = Engine::new(&config)?;
    let module = Module::new(
        &engine,
        r#"(module
        (import "env" "memory" (memory 2 2 shared))
        (import "env" "web_get_clipboard" (func $read (result i32)))
        (import "env" "web_set_clipboard" (func $write (param i32)))
        (import "env" "MTY_HandleProtocol" (func $link (param i32 i32)))
        (import "env" "web_alert" (func $alert (param i32 i32)))
        (import "env" "web_set_key" (func $key (param i32 i32 i32)))
        (global $heap (mut i32) (i32.const 4096))
        (func (export "mty_system_alloc") (param $size i32) (param i32) (result i32)
            (local $old i32) global.get $heap local.tee $old local.get $size i32.add
            global.set $heap local.get $old)
        (func (export "read") (result i32) call $read)
        (func (export "write") i32.const 64 call $write)
        (func (export "link") i32.const 128 i32.const -1 call $link)
        (func (export "alert") i32.const 64 i32.const 64 call $alert)
        (func (export "register") i32.const 0 i32.const 256 i32.const 99 call $key))"#,
    )?;
    let (mut store, instance) = crate::instantiate(&engine, &module)?;
    let services = Arc::new(crate::platform::Services::new(Box::new(Fixture(
        Mutex::new("Árvíztűrő 🦀 secret-sentinel".into()),
    ))));
    store.data_mut().platform = services.clone();
    let memory = store.data().memory.clone();
    let read = instance.get_typed_func::<(), i32>(&mut store, "read")?;
    let first = read.call(&mut store, ())? as u32;
    if first == 0 || memory.string(first, 128)? != "Árvíztűrő 🦀 secret-sentinel" {
        bail!("Unicode guest clipboard read failed");
    }
    memory.c_string(64, 64, "Másolás 🦀 secret-sentinel")?;
    instance
        .get_typed_func::<(), ()>(&mut store, "write")?
        .call(&mut store, ())?;
    let second = read.call(&mut store, ())? as u32;
    if second == first || memory.string(second, 128)? != "Másolás 🦀 secret-sentinel" {
        bail!("Guest clipboard ownership/write failed");
    }
    let link = instance.get_typed_func::<(), ()>(&mut store, "link")?;
    memory.c_string(128, 128, "https://example.invalid/?secret-sentinel=1")?;
    link.call(&mut store, ())?;
    memory.c_string(128, 128, "file:///C:/Windows/notepad.exe")?;
    link.call(&mut store, ())?;
    instance
        .get_typed_func::<(), ()>(&mut store, "alert")?
        .call(&mut store, ())?;
    memory.c_string(256, 32, "Tab")?;
    instance
        .get_typed_func::<(), ()>(&mut store, "register")?
        .call(&mut store, ())?;
    if store.data().key_codes.get("Tab") != Some(&99) || store.data().keys.contains_key(&99) {
        bail!("Forward-only key alias lost");
    }
    let stats = services.snapshot();
    if stats.clipboard_reads != 2
        || stats.clipboard_writes != 1
        || stats.links_opened != 1
        || stats.alerts_shown != 1
        || stats.unavailable_or_rejected != 1
    {
        bail!("Platform counter mismatch");
    }
    store.data_mut().platform = Default::default();
    let empty = read.call(&mut store, ())? as u32;
    if empty == 0 || memory.string(empty, 1)? != "" || store.data().boundary.is_some() {
        bail!("Disabled clipboard did not return guest-owned empty text");
    }
    let report = serde_json::json!({
        "schema": 1, "unicode_clipboard_imports_verified": true,
        "guest_owned_clipboard_buffers_verified": true,
        "https_link_import_verified": true, "unsafe_link_rejected_verified": true,
        "alert_import_verified": true, "forward_key_alias_verified": true,
        "disabled_clipboard_verified": true, "local_platform": stats,
        "real_clipboard_accessed": false, "browser_launched": false,
        "external_requests_enabled": false, "real_account_used": false
    });
    if serde_json::to_string(&report)?.contains("secret-sentinel") {
        bail!("Private desktop data escaped");
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    #[test]
    fn actual_guest_desktop_imports_are_bounded_and_private() {
        super::probe().unwrap();
    }
}
