//! Pinned Matoya 0/1 latch using Wasmtime's public WebAssembly atomic wait API.
use crate::{lifecycle::StopSignal, memory::GuestMemory};
use anyhow::{bail, Result};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

pub fn wait(
    memory: &GuestMemory,
    ptr: u32,
    stop: Option<&StopSignal>,
    timeout: Option<Duration>,
) -> Result<()> {
    let word = memory.sync_word(ptr)?;
    let deadline = timeout.map(|t| Instant::now() + t);
    let check = || -> Result<()> {
        if stop.is_some_and(StopSignal::stopped) {
            return Err(crate::lifecycle::cancellation_error());
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            bail!("bounded guest synchronization deadline reached");
        }
        Ok(())
    };
    check()?;
    match word.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst) {
        Err(1) => {
            // Completion arrived before the wait: consume the latch.
            word.store(0, Ordering::SeqCst);
            return Ok(());
        }
        Err(_) => bail!("invalid guest synchronization state"),
        Ok(_) => {}
    }
    loop {
        check()?;
        // Keep the registered 1 across polling timeouts; a timeout is NOT
        // completion. The pinned signal protocol retries notify until a waiter
        // registers (GuestMemory::signal), covering gaps between these waits.
        let slice = deadline.map_or(Duration::from_millis(25), |d| {
            d.saturating_duration_since(Instant::now())
                .min(Duration::from_millis(25))
        });
        match memory.0.atomic_wait32(u64::from(ptr), 1, Some(slice))? {
            wasmtime::WaitResult::Ok | wasmtime::WaitResult::Mismatch => {
                check()?;
                word.store(0, Ordering::SeqCst);
                return Ok(());
            }
            wasmtime::WaitResult::TimedOut => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    fn memory() -> GuestMemory {
        let mut config = wasmtime::Config::new();
        config.wasm_threads(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        GuestMemory(
            wasmtime::SharedMemory::new(&engine, wasmtime::MemoryType::shared(1, 1)).unwrap(),
        )
    }
    #[test]
    fn completion_before_wait_is_consumed_and_reusable() {
        let m = memory();
        for _ in 0..80 {
            m.signal(64).unwrap();
            wait(&m, 64, None, Some(Duration::from_secs(1))).unwrap();
            assert_eq!(m.sync_word(64).unwrap().load(Ordering::SeqCst), 0);
        }
    }
    #[test]
    fn timeout_slices_are_not_fabricated_completion() {
        let m = memory();
        let notifier = m.clone();
        let thread = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            while notifier.sync_word(64).unwrap().load(Ordering::SeqCst) == 0 {
                assert!(Instant::now() < deadline, "waiter did not register");
                std::thread::yield_now();
            }
            std::thread::sleep(Duration::from_millis(100));
            notifier.signal(64).unwrap();
        });
        let started = Instant::now();
        wait(&m, 64, None, Some(Duration::from_secs(2))).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(100));
        thread.join().unwrap();
        assert_eq!(m.sync_word(64).unwrap().load(Ordering::SeqCst), 0);
    }
    #[test]
    fn cancellation_and_diagnostic_deadline_do_not_claim_completion() {
        let m = memory();
        assert!(wait(&m, 64, None, Some(Duration::from_millis(40))).is_err());
        assert_eq!(m.sync_word(64).unwrap().load(Ordering::SeqCst), 1);
        m.sync_word(64).unwrap().store(0, Ordering::SeqCst);
        let stop = Arc::new(StopSignal::default());
        let other = stop.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            other.stop();
        });
        let error = wait(&m, 64, Some(&stop), None).unwrap_err();
        assert!(crate::lifecycle::is_cancellation(&error));
        thread.join().unwrap();
        assert_eq!(m.sync_word(64).unwrap().load(Ordering::SeqCst), 1);
    }
    #[test]
    fn invalid_sync_pointers_and_values_fail() {
        let m = memory();
        for ptr in [1, 65536, u32::MAX] {
            assert!(wait(&m, ptr, None, Some(Duration::from_millis(1))).is_err());
        }
        m.sync_word(64).unwrap().store(2, Ordering::SeqCst);
        assert!(wait(&m, 64, None, Some(Duration::from_secs(1))).is_err());
        assert_eq!(m.sync_word(64).unwrap().load(Ordering::SeqCst), 2);
    }
}
