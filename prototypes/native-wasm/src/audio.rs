//! Explicit unavailable-output contract, not an audio renderer or silent sink.
//! The pinned web ABI takes a format pointer; current upstream native Matoya
//! uses a different argument list. Only the documented null failure result
//! and null destroy semantics are shared. Never fabricate a usable context.
use anyhow::{bail, Context, Result};
use serde::Serialize;
use wasmtime::{Caller, Val};

#[derive(Clone, Default, Serialize)]
pub struct Output {
    pub create_requests: u64,
    pub destroy_requests: u64,
    pub output_available: bool,
    pub unavailable_reason: Option<&'static str>,
}

pub fn handles(name: &str) -> bool {
    matches!(name, "MTY_AudioCreate" | "MTY_AudioDestroy")
}

pub fn dispatch(
    caller: &mut Caller<'_, crate::host::HostState>,
    name: &str,
    args: &[Val],
    results: &mut [Val],
) -> Result<()> {
    let pointer = args
        .first()
        .and_then(Val::i32)
        .context("audio pointer missing")? as u32;
    match name {
        "MTY_AudioCreate" => {
            // No device, format, or PCM is read: an unavailable factory has no
            // output context to configure. Zero is the documented failure result.
            let output = &mut caller.data_mut().audio_output;
            output.create_requests = output.create_requests.saturating_add(1);
            output.unavailable_reason = Some("native-audio-output-not-implemented");
            *results
                .first_mut()
                .context("audio factory result missing")? = Val::I32(0);
        }
        "MTY_AudioDestroy" => {
            // Destroy takes a pointer-to-handle, not the context itself.
            // Null is idempotent. Non-null fabricated handles stay invalid.
            if pointer != 0 && caller.data().memory.u32(pointer)? != 0 {
                bail!("no native audio context was acquired");
            }
            let output = &mut caller.data_mut().audio_output;
            output.destroy_requests = output.destroy_requests.saturating_add(1);
        }
        _ => bail!("unsupported audio operation"),
    }
    Ok(())
}

pub fn probe() -> Result<serde_json::Value> {
    let mut config = wasmtime::Config::new();
    config
        .wasm_threads(true)
        .consume_fuel(true)
        .epoch_interruption(true);
    let engine = wasmtime::Engine::new(&config)?;
    let module = wasmtime::Module::new(
        &engine,
        r#"(module
        (import "env" "memory" (memory 1 1 shared))
        (import "env" "MTY_AudioCreate" (func $create (param i32 i32 i32 i32 i32) (result i32)))
        (import "env" "MTY_AudioDestroy" (func $destroy (param i32)))
        (data (i32.const 64) "\02\00\00\00\80\bb\00\00")
        (func (export "create") (result i32)
            i32.const 64 i32.const 20 i32.const 100 i32.const -1 i32.const 1 call $create)
        (func (export "destroy") i32.const 0 call $destroy i32.const 128 call $destroy)
        (func (export "still-running") (result i32) i32.const 42))"#,
    )?;
    let (mut store, instance) = crate::instantiate(&engine, &module)?;
    let create = instance.get_typed_func::<(), i32>(&mut store, "create")?;
    for _ in 0..2 {
        if create.call(&mut store, ())? != 0 {
            bail!("unavailable audio fabricated a handle");
        }
    }
    instance
        .get_typed_func::<(), ()>(&mut store, "destroy")?
        .call(&mut store, ())?;
    if instance
        .get_typed_func::<(), i32>(&mut store, "still-running")?
        .call(&mut store, ())?
        != 42
        || store.data().boundary.is_some()
    {
        bail!("unavailable audio trapped the guest");
    }
    store.data().memory.set_u32(128, 123)?;
    if instance
        .get_typed_func::<(), ()>(&mut store, "destroy")?
        .call(&mut store, ())
        .is_ok()
    {
        bail!("fabricated audio context accepted");
    }
    let snapshot = store.data().audio_output.clone();
    if snapshot.create_requests != 2 || snapshot.output_available {
        bail!("audio unavailability lost");
    }
    Ok(
        serde_json::json!({"scope":"synthetic-guest-audio-unavailability", "null_factory_result_verified":true,
        "null_destroy_verified":true,"guest_continues_verified":true,"fabricated_context_rejected":true,
        "audio_output":snapshot,"audio_played":false,"device_opened":false,
        "external_requests_enabled":false,"real_account_used":false}),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn unavailable_audio_is_a_factory_failure_not_a_guest_trap() {
        super::probe().unwrap();
    }
}
