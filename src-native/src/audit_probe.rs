//! Actual imports and two guest instances sharing one bounded offline audit.
use anyhow::{bail, Context, Result};
use wasmtime::{Config, Engine, Module};

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
        (import "env" "MTY_HttpRequest" (func $http (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
        (import "env" "MTY_WebSocketConnect" (func $ws (param i32 i32 i32 i32 i32) (result i32)))
        (import "wasi_snapshot_preview1" "fd_write" (func $log (param i32 i32 i32 i32) (result i32)))
        (func (export "http") (param i32) (result i32)
            i32.const 16 i32.const 1024 i32.const 1100 i32.const 2048 local.get 0 i32.const 0 i32.const 1000 i32.const 400 i32.const 404 i32.const 408 call $http)
        (func (export "ws") (result i32)
            i32.const 16 i32.const 0 i32.const 0 i32.const 1000 i32.const 408 call $ws)
        (func (export "log") (result i32)
            i32.const 1 i32.const 500 i32.const 1 i32.const 600 call $log))"#,
    )?;
    let (mut first, instance) = crate::instantiate(&engine, &module)?;
    let runtime = first
        .data()
        .threads
        .clone()
        .context("Shared runtime missing")?;
    let (mut second, second_instance) = crate::instantiate_with_runtime(runtime.clone())?;
    let memory = runtime.memory.clone();
    let http = instance.get_typed_func::<i32, i32>(&mut first, "http")?;
    let ws = second_instance.get_typed_func::<(), i32>(&mut second, "ws")?;
    let log = instance.get_typed_func::<(), i32>(&mut first, "log")?;
    let secret = "secret-sentinel";
    memory.c_string(
        16,
        1000,
        "https://kessel-api.parsec.app/v2/auth?token=secret-sentinel",
    )?;
    memory.c_string(1024, 64, "POST")?;
    memory.c_string(1100, 128, "Authorization: Bearer secret-sentinel\r\n")?;
    memory.c_string(2048, 64, secret)?;
    memory.write(400, &[255; 10])?;
    if http.call(&mut first, secret.len() as i32)? != 0 || memory.read(400, 10)? != vec![0; 10] {
        bail!("Offline HTTP contract failed");
    }
    memory.c_string(
        16,
        1000,
        "wss://kessel-ws-v2.parsec.app/secret-sentinel?token=secret-sentinel",
    )?;
    if ws.call(&mut second, ())? != 0 || memory.read(408, 2)? != vec![0; 2] {
        bail!("Offline WebSocket contract failed");
    }
    memory.set_u32(500, 2048)?;
    memory.set_u32(504, secret.len() as u32)?;
    if log.call(&mut first, ())? != 0
        || memory.u32(600)? != secret.len() as u32
        || first.data().stdout_bytes != secret.len() as u64
    {
        bail!("Guest stdout acknowledgment failed");
    }
    let snapshot = runtime.audit.snapshot();
    if snapshot.intents.len() != 2 || snapshot.omitted != 0 {
        bail!("Shared audit missing guest intents");
    }
    let json = serde_json::to_string(&snapshot)?;
    let host_json = serde_json::to_string(first.data())?;
    if json.contains(secret) || host_json.contains(secret) || host_json.contains("\"stdout\"") {
        bail!("Sensitive guest text retained in report");
    }
    if !json.contains("\"has_authorization\":true")
        || !json.contains("\"has_query\":true")
        || !json.contains("authentication")
        || !json.contains("signaling")
    {
        bail!("Audit metadata missing");
    }
    Ok(serde_json::json!({
        "schema":1,"scope":"controlled-wasm-shared-offline-network-audit",
        "guest_instances_verified":2,"intents_verified":2,
        "shared_audit_verified":true,"offline_failure_outputs_verified":true,
        "request_redaction_verified":true,"stdout_redaction_verified":true,
        "network_audit":snapshot,"external_requests_enabled":false,
        "authentication_integrated":false,"original_parsec_guest_auth_exercised":false,
        "parsec_host_connected":false,"video_decoded":false
    }))
}
#[cfg(test)]
mod tests {
    #[test]
    fn shared_guest_network_metadata_excludes_credentials_and_guest_logs() {
        let report = super::probe().unwrap();
        assert_eq!(report["guest_instances_verified"], 2);
        assert_eq!(report["stdout_redaction_verified"], true);
        assert_eq!(report["external_requests_enabled"], false);
    }
    #[cfg(windows)]
    #[test]
    fn window_http_uses_the_common_host_bridge() {
        assert!(!crate::desktop::handles("MTY_HttpRequest"));
        assert!(crate::host::implemented("env", "MTY_HttpRequest"));
    }
}
