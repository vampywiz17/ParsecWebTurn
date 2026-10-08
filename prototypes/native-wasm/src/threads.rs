use crate::{host::HostState, memory::GuestMemory};
use anyhow::{Context, Result};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use wasmtime::{Engine, Module};

/// Legacy WASI-threads ABI used by this pinned module: thread-spawn(start_arg)
/// returns a positive ID, and a fresh instance sharing linear memory enters
/// wasi_thread_start(thread_id, start_arg). No JavaScript worker is involved.
pub struct ThreadRuntime {
    pub engine: Engine,
    pub module: Module,
    pub memory: GuestMemory,
    pub filesystem: Arc<Mutex<crate::filesystem::VirtualFs>>,
    pub backend: Arc<Mutex<crate::backend::Backend>>,
    pub started: std::time::Instant,
    pub(crate) records: Mutex<Vec<ThreadRecord>>,
    #[cfg(windows)]
    pub window: Option<Arc<crate::window::Window>>,
}

#[derive(Clone, Serialize)]
pub struct ThreadRecord {
    pub id: i32,
    pub finished: bool,
    pub error: Option<String>,
    pub boundary: Option<String>,
    pub calls: std::collections::BTreeMap<String, u64>,
}

impl ThreadRuntime {
    pub fn new(engine: Engine, module: Module, memory: GuestMemory) -> Self {
        Self {
            engine,
            module,
            memory,
            filesystem: Default::default(),
            backend: Default::default(),
            started: std::time::Instant::now(),
            records: Mutex::new(Vec::new()),
            #[cfg(windows)]
            window: None,
        }
    }

    pub fn snapshot(&self) -> Vec<ThreadRecord> {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn spawn(self: &Arc<Self>, argument: u32) -> i32 {
        let id = {
            let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());
            // Includes finished threads: the offline bootstrap may create at
            // most eight threads in total, not an unbounded sequence of workers.
            if records.len() >= 8 {
                return -1;
            }
            // Match the audited Matoya loader: main is ID 1, children start at 2.
            let id = records.len() as i32 + 2;
            records.push(ThreadRecord {
                id,
                finished: false,
                error: None,
                boundary: None,
                calls: Default::default(),
            });
            id
        };
        let runtime = self.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("parsec-wasm-{id}"))
            .spawn(move || {
                let mut host = None;
                let outcome: Result<()> = (|| {
                    let (mut store, instance) = crate::instantiate_with_runtime(runtime.clone())?;
                    let entry = instance
                        .get_typed_func::<(i32, i32), ()>(&mut store, "wasi_thread_start")
                        .context("WASI thread entry export missing")?;
                    let result = entry.call(&mut store, (id, argument as i32));
                    #[cfg(windows)]
                    let result =
                        if store.data().window.is_some() && store.data().event_loop.is_some() {
                            crate::desktop::run_worker(&mut store, &instance)
                        } else {
                            result
                        };
                    host = Some(store.into_data());
                    result
                })();
                runtime.finish(id, outcome.err().map(|e| format!("{e:#}")), host);
            });
        if let Err(error) = spawned {
            self.finish(
                id,
                Some(format!("OS thread creation failed: {error}")),
                None,
            );
            -1
        } else {
            id
        }
    }

    fn finish(&self, id: i32, error: Option<String>, host: Option<HostState>) {
        let failed = error.is_some();
        {
            let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());
            let record = &mut records[id as usize - 2];
            record.finished = true;
            record.error = error;
            if let Some(host) = host {
                record.boundary = host.boundary;
                record.calls = host.calls;
            }
        }
        // Abort other executing guest loops at an observable bridge boundary.
        // A process deadline remains necessary for blocking guest atomic waits.
        if failed {
            self.engine.increment_epoch();
        }
    }
}
