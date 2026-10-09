use crate::{host::HostState, memory::GuestMemory};
use anyhow::{Context, Result};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use wasmtime::{Engine, Module};

const MAX_ACTIVE: usize = 16;
const MAX_HISTORY: usize = 64;
const TID_END: i32 = 1 << 29;

pub(crate) struct Records {
    entries: Vec<ThreadRecord>,
    next_id: i32,
    completed: u64,
    rejected: u64,
    omitted: u64,
    peak_active: usize,
}
impl Default for Records {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            next_id: 2,
            completed: 0,
            rejected: 0,
            omitted: 0,
            peak_active: 0,
        }
    }
}
impl Records {
    fn active(&self) -> usize {
        self.entries.iter().filter(|r| !r.finished).count()
    }
    fn reserve(&mut self) -> Option<i32> {
        let active = self.active();
        if active >= MAX_ACTIVE || self.next_id >= TID_END {
            self.rejected = self.rejected.saturating_add(1);
            return None;
        }
        if self.entries.len() == MAX_HISTORY {
            // Never evict a running worker: its eventual completion must still
            // release capacity and retain its failure/exit diagnostics.
            let oldest = self.entries.iter().position(|r| r.finished)?;
            self.entries.remove(oldest);
            self.omitted = self.omitted.saturating_add(1);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.peak_active = self.peak_active.max(active + 1);
        self.entries.push(ThreadRecord {
            id,
            finished: false,
            cancelled: false,
            error: None,
            boundary: None,
            audio_output: None,
            execution_failure: None,
            last_host_call: None,
            exit_code: None,
            calls: Default::default(),
        });
        Some(id)
    }
}

#[derive(Serialize)]
pub struct ThreadSummary {
    pub active: usize,
    pub active_limit: usize,
    pub peak_active: usize,
    pub completed: u64,
    pub spawn_rejected: u64,
    pub history_omitted: u64,
}

/// Legacy WASI-threads ABI used by this pinned module: thread-spawn(start_arg)
/// returns a positive ID, and a fresh instance sharing linear memory enters
/// wasi_thread_start(thread_id, start_arg). No JavaScript worker is involved.
pub struct ThreadRuntime {
    pub engine: Engine,
    pub module: Module,
    pub memory: GuestMemory,
    pub filesystem: Arc<Mutex<crate::filesystem::VirtualFs>>,
    pub backend: Arc<Mutex<crate::backend::Backend>>,
    pub http: Arc<crate::http::Network>,
    pub websocket: Arc<crate::websocket::Network>,
    pub audit: Arc<crate::network_audit::Audit>,
    pub platform: Arc<crate::platform::Services>,
    pub started: std::time::Instant,
    pub(crate) records: Mutex<Records>,
    pub watchdog_started: std::sync::atomic::AtomicBool,
    #[cfg(windows)]
    pub window: Option<Arc<crate::window::Window>>,
}

#[derive(Clone, Serialize)]
pub struct ThreadRecord {
    pub id: i32,
    pub finished: bool,
    pub cancelled: bool,
    pub error: Option<String>,
    pub execution_failure: Option<crate::execution_diagnostics::Failure>,
    pub last_host_call: Option<String>,
    pub boundary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_output: Option<crate::audio::Output>,
    pub exit_code: Option<u32>,
    pub calls: std::collections::BTreeMap<String, u64>,
}

impl ThreadRuntime {
    pub fn new(engine: Engine, module: Module, memory: GuestMemory) -> Self {
        let audit = Arc::new(crate::network_audit::Audit::default());
        let http = crate::http::Network::offline(audit.clone());
        let websocket = crate::websocket::Network::offline(audit.clone());
        Self {
            engine,
            module,
            memory,
            filesystem: Default::default(),
            backend: Default::default(),
            http: Arc::new(http),
            websocket: Arc::new(websocket),
            audit,
            platform: Default::default(),
            started: std::time::Instant::now(),
            records: Default::default(),
            watchdog_started: Default::default(),
            #[cfg(windows)]
            window: None,
        }
    }

    pub fn snapshot(&self) -> Vec<ThreadRecord> {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .clone()
    }

    pub fn summary(&self) -> ThreadSummary {
        let records = self.records.lock().unwrap_or_else(|e| e.into_inner());
        ThreadSummary {
            active: records.active(),
            active_limit: MAX_ACTIVE,
            peak_active: records.peak_active,
            completed: records.completed,
            spawn_rejected: records.rejected,
            history_omitted: records.omitted,
        }
    }

    pub fn spawn(self: &Arc<Self>, argument: u32) -> i32 {
        let id = {
            let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());
            let Some(id) = records.reserve() else {
                return -1;
            };
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
                    store.data_mut().execution_stage = Some("guest-thread-start");
                    let result = entry.call(&mut store, (id, argument as i32));
                    #[cfg(windows)]
                    let result =
                        if store.data().window.is_some() && store.data().event_loop.is_some() {
                            crate::desktop::run_worker(&mut store, &instance)
                        } else {
                            result
                        };
                    if let Err(error) = &result {
                        store.data_mut().execution_failure =
                            Some(crate::execution_diagnostics::Failure::capture(
                                error,
                                store.data().execution_stage,
                            ));
                    }
                    host = Some(store.into_data());
                    result
                })();
                let error = outcome.err();
                let cancelled = error
                    .as_ref()
                    .is_some_and(crate::lifecycle::is_cancellation);
                runtime.finish(
                    id,
                    error.filter(|_| !cancelled).map(|e| format!("{e:#}")),
                    host,
                    cancelled,
                );
            });
        if let Err(error) = spawned {
            self.finish(
                id,
                Some(format!("OS thread creation failed: {error}")),
                None,
                false,
            );
            -1
        } else {
            id
        }
    }

    fn finish(&self, id: i32, error: Option<String>, host: Option<HostState>, cancelled: bool) {
        let failed = error.is_some();
        {
            let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());
            let record = records
                .entries
                .iter_mut()
                .find(|r| r.id == id)
                .expect("active thread record retained until completion");
            record.finished = true;
            record.cancelled = cancelled;
            record.error = error;
            if let Some(host) = host {
                if host.audio_output.create_requests > 0 || host.audio_output.destroy_requests > 0 {
                    record.audio_output = Some(host.audio_output);
                }
                record.execution_failure = host.execution_failure;
                record.last_host_call = host.last_host_call;
                record.boundary = host.boundary;
                record.calls = host.calls;
                record.exit_code = host.guest_exit_code;
            }
            records.completed = records.completed.saturating_add(1);
        }
        // Abort other executing guest loops at an observable bridge boundary.
        // A process deadline remains necessary for blocking guest atomic waits.
        if failed {
            #[cfg(windows)]
            if let Some(window) = &self.window {
                if window.live {
                    window.request_stop();
                }
            }
            self.engine.increment_epoch();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_tid_range_never_wraps_or_reuses_an_id() {
        let mut records = Records {
            next_id: TID_END - 1,
            ..Default::default()
        };
        assert_eq!(records.reserve(), Some(TID_END - 1));
        assert_eq!(records.reserve(), None);
        assert_eq!(records.rejected, 1);
    }
}
