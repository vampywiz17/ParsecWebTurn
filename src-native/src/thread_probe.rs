//! Isolated WASI-thread lifecycle regression, using synthetic shared-memory data.
use anyhow::{ensure, Result};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use wasmtime::{Config, Engine, Module};

fn wait_finished(runtime: &crate::threads::ThreadRuntime, ids: &[i32]) -> Result<()> {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        let records = runtime.snapshot();
        if ids
            .iter()
            .all(|id| records.iter().any(|r| r.id == *id && r.finished))
        {
            ensure!(
                records
                    .iter()
                    .filter(|r| ids.contains(&r.id))
                    .all(|r| r.error.is_none()),
                "Synthetic guest worker failed"
            );
            return Ok(());
        }
        ensure!(Instant::now() < until, "Synthetic worker did not finish");
        std::thread::sleep(Duration::from_millis(2));
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
        (import "env" "memory" (memory 1 1 shared))
        (import "wasi" "thread-spawn" (func $spawn (param i32) (result i32)))
        (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
        (func (export "spawn") (param i32) (result i32) local.get 0 call $spawn)
        (func (export "exit") i32.const 19 call $exit)
        (func (export "wasi_thread_start") (param $id i32) (param $arg i32)
            (block $ready (loop $waiting
                i32.const 0 i32.atomic.load br_if $ready
                i32.const 0 i32.const 0 i64.const 10000000 memory.atomic.wait32 drop
                br $waiting))
            local.get $arg local.get $id i32.atomic.store))"#,
    )?;
    let (mut store, instance) = crate::instantiate(&engine, &module)?;
    let runtime: Arc<crate::threads::ThreadRuntime> = store.data().threads.clone().unwrap();
    let spawn = instance.get_typed_func::<i32, i32>(&mut store, "spawn")?;
    let memory = store.data().memory.clone();
    memory.set_u32(0, 1)?;
    let mut last_id = 1;
    for index in 0..80 {
        let output = 16 + index * 4;
        let id = spawn.call(&mut store, output)?;
        ensure!(
            id > last_id,
            "Finished workers consumed lifetime capacity or ID reused"
        );
        wait_finished(&runtime, &[id])?;
        ensure!(
            memory.u32(output as u32)? == id as u32,
            "Worker did not share guest memory/TID"
        );
        last_id = id;
    }
    ensure!(
        runtime.snapshot().len() == 64 && runtime.summary().history_omitted == 16,
        "Thread history not bounded"
    );
    memory.set_u32(0, 0)?;
    let limit = runtime.summary().active_limit;
    let mut running = Vec::new();
    for index in 0..limit {
        let id = spawn.call(&mut store, 512 + index as i32 * 4)?;
        if id <= last_id {
            memory.set_u32(0, 1)?;
            anyhow::bail!("Concurrent workers rejected before limit");
        }
        running.push(id);
        last_id = id;
    }
    let at_limit = runtime.summary().active == limit;
    let overflow_rejected = spawn.call(&mut store, 640)? < 0;
    memory.set_u32(0, 1)?;
    ensure!(
        at_limit && overflow_rejected,
        "Concurrent limit not enforced"
    );
    wait_finished(&runtime, &running)?;
    for (index, id) in running.iter().enumerate() {
        ensure!(
            memory.u32(512 + index as u32 * 4)? == *id as u32,
            "Concurrent guest data lost"
        );
    }
    let next = spawn.call(&mut store, 644)?;
    ensure!(next > last_id, "Capacity not released after completion");
    wait_finished(&runtime, &[next])?;
    let summary = runtime.summary();
    ensure!(
        summary.active == 0
            && summary.completed == 97
            && summary.spawn_rejected == 1
            && summary.peak_active == 16
            && summary.history_omitted == 33,
        "Thread accounting mismatch"
    );
    let exit = instance.get_typed_func::<(), ()>(&mut store, "exit")?;
    ensure!(
        exit.call(&mut store, ()).is_err() && store.data().guest_exit_code == Some(19),
        "Guest process exit code not retained"
    );
    Ok(serde_json::json!({
        "schema": 1, "sequential_guest_workers_verified": 80,
        "completed_guest_workers_verified": summary.completed,
        "unique_guest_ids_verified": true, "shared_memory_verified": true,
        "concurrent_limit_verified": true, "capacity_released_verified": true,
        "bounded_history_verified": true, "numeric_guest_exit_code_verified": true,
        "thread_runtime": summary, "external_requests_enabled": false,
        "real_account_used": false, "parsec_host_connected": false
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn actual_guest_threads_reuse_capacity_and_bound_history() {
        let report = super::probe().unwrap();
        assert_eq!(report["completed_guest_workers_verified"], 97);
        assert_eq!(report["numeric_guest_exit_code_verified"], true);
    }
}
