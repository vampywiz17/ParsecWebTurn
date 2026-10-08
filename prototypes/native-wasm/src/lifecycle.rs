//! One cancellation signal shared by the UI, guest instances and watchdog.
use std::sync::{Condvar, Mutex};
use std::time::Duration;

#[derive(Debug)]
struct SessionStopped;
impl std::fmt::Display for SessionStopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("native-session-stopped")
    }
}
impl std::error::Error for SessionStopped {}
pub fn is_cancellation(error: &anyhow::Error) -> bool {
    error.downcast_ref::<SessionStopped>().is_some()
}

#[derive(Default)]
pub struct StopSignal {
    stopped: Mutex<bool>,
    changed: Condvar,
}
impl StopSignal {
    pub fn stop(&self) {
        *self.stopped.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.changed.notify_all();
    }
    pub fn stopped(&self) -> bool {
        *self.stopped.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        let guard = self.stopped.lock().unwrap_or_else(|e| e.into_inner());
        *self
            .changed
            .wait_timeout_while(guard, timeout, |stop| !*stop)
            .unwrap_or_else(|e| e.into_inner())
            .0
    }
    pub fn wait(&self) {
        let guard = self.stopped.lock().unwrap_or_else(|e| e.into_inner());
        drop(
            self.changed
                .wait_while(guard, |stop| !*stop)
                .unwrap_or_else(|e| e.into_inner()),
        );
    }
}

/// Documented Wasmtime epoch callback: replenish bounded fuel between ticks;
/// stop traps executing code, including instances sharing the same engine.
pub fn configure_store<T: 'static>(
    store: &mut wasmtime::Store<T>,
    stop: std::sync::Arc<StopSignal>,
) {
    store.epoch_deadline_callback(move |mut context| {
        if stop.stopped() {
            return Err(anyhow::Error::new(SessionStopped));
        }
        context.set_fuel(context.get_fuel()?.max(50_000_000))?;
        Ok(wasmtime::UpdateDeadline::Continue(1))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn cancellation_wakes_all_waiters_and_is_not_lost() {
        let signal = Arc::new(StopSignal::default());
        assert!(!signal.wait_timeout(Duration::from_millis(1)));
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let signal = signal.clone();
                std::thread::spawn(move || {
                    signal.wait();
                    assert!(signal.stopped());
                })
            })
            .collect();
        signal.stop();
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(signal.wait_timeout(Duration::from_secs(1)));
    }
    #[test]
    fn live_epochs_continue_then_cancel_guest_execution() {
        let mut config = wasmtime::Config::new();
        config.consume_fuel(true).epoch_interruption(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        let module = wasmtime::Module::new(
            &engine,
            r#"(module
            (func (export "tick") (result i32) (local $count i32)
                i32.const 20 local.set $count
                (loop $next
                    local.get $count i32.const 1 i32.sub local.tee $count
                    br_if $next)
                i32.const 7))"#,
        )
        .unwrap();
        let stop = Arc::new(StopSignal::default());
        let mut store = wasmtime::Store::new(&engine, ());
        // Leave enough fuel to reach the first loop's epoch checkpoint.
        // Fuel exhaustion can trap before epoch callbacks are invoked.
        store.set_fuel(10_000).unwrap();
        store.set_epoch_deadline(1);
        configure_store(&mut store, stop.clone());
        let instance = wasmtime::Instance::new(&mut store, &module, &[]).unwrap();
        let tick = instance
            .get_typed_func::<(), i32>(&mut store, "tick")
            .unwrap();
        for _ in 0..20 {
            engine.increment_epoch();
            assert_eq!(tick.call(&mut store, ()).unwrap(), 7);
        }
        assert!(store.get_fuel().unwrap() > 10_000);
        stop.stop();
        engine.increment_epoch();
        let error = tick.call(&mut store, ()).unwrap_err();
        assert!(is_cancellation(&error));
        assert!(!is_cancellation(&anyhow::anyhow!("actual guest failure")));
    }
}
