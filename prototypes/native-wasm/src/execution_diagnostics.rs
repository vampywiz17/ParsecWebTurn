//! Public Wasmtime diagnostics only: fixed stages and numeric WASM locations.
use serde::Serialize;
#[derive(Clone, Serialize)]
pub struct Frame {
    function_index: u32,
    module_offset: Option<usize>,
    function_offset: Option<usize>,
}
#[derive(Clone, Serialize)]
pub struct Failure {
    kind: String,
    stage: Option<&'static str>,
    frames: Vec<Frame>,
    omitted_frames: usize,
}
impl Failure {
    pub fn capture(error: &anyhow::Error, stage: Option<&'static str>) -> Self {
        // Trap's Debug is a runtime enum, never the guest/host error message.
        let kind = if crate::lifecycle::is_cancellation(error) {
            "session-cancelled".into()
        } else if let Some(trap) = error.downcast_ref::<wasmtime::Trap>() {
            format!("{trap:?}")
        } else {
            "host-or-api-error".into()
        };
        let frames = error
            .downcast_ref::<wasmtime::WasmBacktrace>()
            .map_or(&[][..], |trace| trace.frames());
        Self {
            kind,
            stage,
            omitted_frames: frames.len().saturating_sub(12),
            frames: frames
                .iter()
                .take(12)
                .map(|frame| Frame {
                    function_index: frame.func_index(),
                    module_offset: frame.module_offset(),
                    function_offset: frame.func_offset(),
                })
                .collect(),
        }
    }
}

pub fn probe() -> anyhow::Result<serde_json::Value> {
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(
        &engine,
        r#"(module
        (func $private_account_name (export "run") unreachable))"#,
    )?;
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = wasmtime::Instance::new(&mut store, &module, &[])?;
    let error = instance
        .get_typed_func::<(), ()>(&mut store, "run")?
        .call(&mut store, ())
        .unwrap_err();
    let failure = Failure::capture(&error, Some("event-loop-callback"));
    anyhow::ensure!(
        failure.kind == "UnreachableCodeReached" && failure.frames.len() == 1,
        "structured WASM trap missing"
    );
    let text = serde_json::to_string(&failure)?;
    anyhow::ensure!(
        !text.contains("private_account_name"),
        "guest symbol leaked"
    );
    let secret = Failure::capture(
        &anyhow::anyhow!("private-password-token-url"),
        Some("event-export"),
    );
    anyhow::ensure!(
        !serde_json::to_string(&secret)?.contains("private-password"),
        "host error leaked"
    );
    Ok(
        serde_json::json!({"trap_classification_verified":true,"numeric_frames_verified":true,"error_redaction_verified":true,"real_account_used":false,"failure":failure}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numeric_backtrace_omits_guest_symbols_and_raw_messages() {
        probe().unwrap();
    }
    #[test]
    fn api_errors_remain_unknown_and_cancellation_is_separate() {
        assert_eq!(
            Failure::capture(&anyhow::anyhow!("secret"), None).kind,
            "host-or-api-error"
        );
        assert_eq!(
            Failure::capture(&crate::lifecycle::cancellation_error(), None).kind,
            "session-cancelled"
        );
    }
    #[test]
    fn frame_history_is_bounded() {
        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(
            &engine,
            r#"(module
            (func $recurse (export "run") (param i32)
                local.get 0 i32.eqz if unreachable end
                local.get 0 i32.const 1 i32.sub call $recurse))"#,
        )
        .unwrap();
        let mut store = wasmtime::Store::new(&engine, ());
        let instance = wasmtime::Instance::new(&mut store, &module, &[]).unwrap();
        let error = instance
            .get_typed_func::<i32, ()>(&mut store, "run")
            .unwrap()
            .call(&mut store, 20)
            .unwrap_err();
        let failure = Failure::capture(&error, None);
        assert_eq!(failure.frames.len(), 12);
        assert!(failure.omitted_frames > 0);
    }
    #[test]
    fn fuel_exhaustion_is_classified_without_guessing() {
        let mut config = wasmtime::Config::new();
        config.consume_fuel(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        let module =
            wasmtime::Module::new(&engine, "(module (func (export \"run\") (loop br 0)))").unwrap();
        let mut store = wasmtime::Store::new(&engine, ());
        store.set_fuel(10).unwrap();
        let instance = wasmtime::Instance::new(&mut store, &module, &[]).unwrap();
        let error = instance
            .get_typed_func::<(), ()>(&mut store, "run")
            .unwrap()
            .call(&mut store, ())
            .unwrap_err();
        assert_eq!(
            Failure::capture(&error, Some("event-loop-callback")).kind,
            "OutOfFuel"
        );
    }
}
