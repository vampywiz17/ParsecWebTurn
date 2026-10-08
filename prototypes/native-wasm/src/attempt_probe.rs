//! Controlled WAT guest calling the actual native import. This validates the
//! pinned ABI shape, not authentication or a call from the original Parsec UI.
use crate::{memory::GuestMemory, signaling::Credentials};
use anyhow::{bail, Result};
use std::time::Duration;
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
        (import "env" "memory" (memory 1 1 shared))
        (import "env" "parsec_web_init" (func $init))
        (import "env" "parsec_web_new_attempt" (func $offer (param i32 i32 i32 i32 i32 i32 i32)))
        (import "env" "parsec_web_destroy" (func $destroy))
        (data (i32.const 16) "local-offer-test\00")
        (func (export "offer") (result i32)
            call $init
            i32.const 16 i32.const 100 i32.const 400 i32.const 700 i32.const 256 i32.const 64 i32.const 80 call $offer
            i32.const 64 i32.const 0 i32.const 1 i32.atomic.rmw.cmpxchg
            i32.eqz
            if
                i32.const 64 i32.const 1 i64.const 4000000000 memory.atomic.wait32
                i32.const 2 i32.eq if unreachable end
            end
            i32.const 64 i32.const 0 i32.atomic.store
            i32.const 80 i32.load)
        (func (export "destroy") call $destroy))"#,
    )?;
    let (mut store, instance) = crate::instantiate(&engine, &module)?;
    let error = instance
        .get_typed_func::<(), i32>(&mut store, "offer")?
        .call(&mut store, ())?;
    let memory: GuestMemory = store.data().memory.clone();
    let backend = store.data().backend.clone();
    let mut attempt = backend
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .native_attempt
        .take()
        .ok_or_else(|| anyhow::anyhow!("guest did not create a native attempt"))?;
    let checked = (|| -> Result<()> {
        if error != 0 {
            bail!("native offer completion reported failure");
        }
        Credentials {
            ufrag: memory.string(100, 256)?,
            password: memory.string(400, 256)?,
            fingerprint: memory.string(700, 256)?,
        }
        .validate()?;
        let state = attempt.snapshot();
        if state["offer_ready"] != true
            || state["negotiated_channels"] != 3
            || backend.lock().unwrap_or_else(|e| e.into_inner()).status != Some(20)
        {
            bail!("incorrect native pending-offer state");
        }
        Ok(())
    })();
    attempt.cancel();
    attempt.wait_finished(Duration::from_secs(12))?;
    checked?;
    let state = attempt.snapshot();
    if state["peer_closed"] != true || state["failed"] != false {
        bail!("native offer resource cleanup failed");
    }
    instance
        .get_typed_func::<(), ()>(&mut store, "destroy")?
        .call(&mut store, ())?;
    Ok(
        serde_json::json!({ "schema":1, "scope":"controlled-wasm-guest-native-offer-import", "original_parsec_guest_attempt_exercised":false, "guest_completion_verified":true, "credentials_validated":true, "native_attempt":state, "parsec_host_connected":false, "video_decoded":false }),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn guest_import_returns_real_credentials_wakes_guest_and_closes_peer() {
        let report = super::probe().unwrap();
        assert_eq!(report["guest_completion_verified"], true);
        assert_eq!(report["native_attempt"]["worker_finished"], true);
        assert_eq!(report["native_attempt"]["peer_closed"], true);
        assert_eq!(report["parsec_host_connected"], false);
    }
}
