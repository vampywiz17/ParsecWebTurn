//! Bounded WASI preview1 clock subscription used by libc nanosleep.
//! Layout verified against WebAssembly/wasi-libc's public wasi/wasip1.h.
use crate::memory::GuestMemory;
use anyhow::Result;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub fn clock_poll(
    m: &GuestMemory,
    started: Instant,
    input: u32,
    output: u32,
    count: u32,
    returned: u32,
) -> Result<i32> {
    if count == 0 {
        return Ok(28);
    } // INVAL
    if count != 1 {
        return Ok(52);
    } // NOSYS: multi-subscription polling not implemented.
    let subscription = m.read(input, 48)?;
    m.read(output, 32)?;
    m.read(returned, 4)?;
    let tag = subscription[8];
    if tag != 0 {
        return Ok(if tag <= 2 { 52 } else { 28 });
    }
    let id = u32::from_le_bytes(subscription[16..20].try_into().unwrap());
    let timestamp = u64::from_le_bytes(subscription[24..32].try_into().unwrap());
    let flags = u16::from_le_bytes(subscription[40..42].try_into().unwrap());
    if flags & !1 != 0 || id > 1 {
        return Ok(28);
    }
    let now = if id == 0 {
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    } else {
        started.elapsed().as_nanos()
    };
    let ns = if flags == 1 {
        u128::from(timestamp).saturating_sub(now)
    } else {
        u128::from(timestamp)
    };
    if ns > 1_000_000_000 {
        return Ok(52);
    } // No unbounded host waits in the bootstrap.
    let deadline = Instant::now() + Duration::from_nanos(ns as u64);
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        std::thread::sleep(remaining);
    }
    let mut event = [0u8; 32];
    event[..8].copy_from_slice(&subscription[..8]); // Userdata, unchanged.
                                                    // errno at 8 and type at 10 are zero: successful CLOCK, not FD readiness.
    m.write(output, &event)?;
    m.set_u32(returned, 1)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime::{Config, Engine, MemoryType, SharedMemory};

    #[test]
    fn wasi_clock_poll_obeys_deadline_userdata_and_subscription_layout() {
        let mut config = Config::new();
        config.wasm_threads(true);
        let engine = Engine::new(&config).unwrap();
        let m = GuestMemory(SharedMemory::new(&engine, MemoryType::shared(1, 1)).unwrap());
        let mut sub = [0u8; 48];
        sub[..8].copy_from_slice(&0x123456789abcdeffu64.to_le_bytes());
        sub[16..20].copy_from_slice(&1u32.to_le_bytes());
        sub[24..32].copy_from_slice(&5_000_000u64.to_le_bytes());
        m.write(0, &sub).unwrap();
        let started = Instant::now();
        assert_eq!(clock_poll(&m, started, 0, 64, 1, 100).unwrap(), 0);
        assert!(started.elapsed() >= Duration::from_millis(5));
        assert_eq!(m.read(64, 8).unwrap(), sub[..8]);
        assert_eq!(m.read(72, 24).unwrap(), vec![0; 24]);
        assert_eq!(m.u32(100).unwrap(), 1);
        // Absolute deadline in the past completes, not a fresh relative wait.
        sub[24..32].copy_from_slice(&0u64.to_le_bytes());
        sub[40..42].copy_from_slice(&1u16.to_le_bytes());
        m.write(0, &sub).unwrap();
        assert_eq!(clock_poll(&m, started, 0, 64, 1, 100).unwrap(), 0);
        sub[8] = 1; // Never fabricate readiness for an unsupported FD poll.
        m.write(0, &sub).unwrap();
        assert_eq!(clock_poll(&m, started, 0, 64, 1, 100).unwrap(), 52);
        assert!(clock_poll(&m, started, 0, u32::MAX, 1, 100).is_err());
    }
}
