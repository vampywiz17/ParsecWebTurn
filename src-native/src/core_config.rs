// Shared by the isolated AOT build and its runtime comparison. Keep all
// execution/safety settings identical; AOT changes loading, not guest policy.
pub fn config() -> wasmtime::Config {
    let mut config = wasmtime::Config::new();
    config
        .wasm_threads(true)
        .consume_fuel(true)
        .epoch_interruption(true);
    config
}
