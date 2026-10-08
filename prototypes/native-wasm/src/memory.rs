use anyhow::{bail, Context, Result};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use wasmtime::SharedMemory;

/// All host access uses atomic bytes: the guest imports shared linear memory.
/// No mutable slices are created, including in the single-threaded bootstrap.
#[derive(Clone)]
pub struct GuestMemory(pub SharedMemory);

impl GuestMemory {
    pub(crate) fn range(&self, ptr: u32, len: usize) -> Result<std::ops::Range<usize>> {
        let start = ptr as usize;
        let end = start.checked_add(len).context("guest pointer overflow")?;
        if end > self.0.data_size() {
            bail!("guest memory access out of bounds");
        }
        Ok(start..end)
    }

    pub fn read(&self, ptr: u32, len: usize) -> Result<Vec<u8>> {
        let data = self.0.data();
        self.range(ptr, len)?
            .map(|i| {
                // SAFETY: range is checked; AtomicU8 has byte alignment and the
                // same size as u8. SharedMemory keeps the allocation alive.
                Ok(unsafe { AtomicU8::from_ptr(data[i].get()) }.load(Ordering::SeqCst))
            })
            .collect()
    }

    pub fn write(&self, ptr: u32, bytes: &[u8]) -> Result<()> {
        let range = self.range(ptr, bytes.len())?;
        let data = self.0.data();
        for (i, byte) in range.zip(bytes) {
            // SAFETY: same invariant as read(). Never alias with non-atomic host access.
            unsafe { AtomicU8::from_ptr(data[i].get()) }.store(*byte, Ordering::SeqCst);
        }
        Ok(())
    }

    pub fn u32(&self, ptr: u32) -> Result<u32> {
        Ok(u32::from_le_bytes(self.read(ptr, 4)?.try_into().unwrap()))
    }

    pub fn set_u32(&self, ptr: u32, value: u32) -> Result<()> {
        self.write(ptr, &value.to_le_bytes())
    }

    pub fn sync_word(&self, ptr: u32) -> Result<&AtomicU32> {
        self.range(ptr, 4)?;
        if ptr % 4 != 0 {
            bail!("unaligned guest synchronization word");
        }
        let address = self.0.data()[ptr as usize].get().cast::<u32>();
        // SAFETY: shared memory base is page-aligned, range/alignment checked.
        // Dedicated synchronization words use ONLY 32-bit atomic host access
        // while active; never overlapping AtomicU8 access during publication.
        Ok(unsafe { AtomicU32::from_ptr(address) })
    }

    pub fn signal(&self, ptr: u32) -> Result<()> {
        let word = self.sync_word(ptr)?;
        match word.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => Ok(()), // Completion before the guest's wait: latched.
            Err(1) => {
                // Matoya's wait sets 1 BEFORE atomic.wait(1). A notification
                // before wait registration would be lost. Retry without a hot
                // spin, bounded rather than copying JS's unbounded notify loop.
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
                loop {
                    if self.0.atomic_notify(u64::from(ptr), 1)? == 1 {
                        return Ok(());
                    }
                    if std::time::Instant::now() >= deadline {
                        bail!("guest completion waiter did not register");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
            Err(_) => bail!("invalid guest synchronization state"),
        }
    }

    pub fn string(&self, ptr: u32, limit: usize) -> Result<String> {
        let mut bytes = Vec::new();
        for offset in 0..limit {
            let p = ptr
                .checked_add(offset as u32)
                .context("guest string overflow")?;
            let b = self.read(p, 1)?[0];
            if b == 0 {
                return Ok(String::from_utf8(bytes)?);
            }
            bytes.push(b);
        }
        bail!("unterminated guest string exceeds limit")
    }

    pub fn c_string(&self, ptr: u32, capacity: usize, value: &str) -> Result<()> {
        if value
            .len()
            .checked_add(1)
            .context("string length overflow")?
            > capacity
        {
            bail!("guest string buffer too small");
        }
        self.write(ptr, value.as_bytes())?;
        self.write(
            ptr.checked_add(value.len() as u32)
                .context("pointer overflow")?,
            &[0],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime::{Config, Engine, MemoryType};

    fn memory() -> GuestMemory {
        let mut config = Config::new();
        config.wasm_threads(true);
        let engine = Engine::new(&config).unwrap();
        GuestMemory(SharedMemory::new(&engine, MemoryType::shared(1, 1)).unwrap())
    }

    #[test]
    fn matoya_completion_handles_early_signal_and_wait_registration_race() {
        let m = memory();
        m.signal(64).unwrap();
        assert_eq!(
            m.sync_word(64)
                .unwrap()
                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst),
            Err(1)
        );
        m.sync_word(64).unwrap().store(0, Ordering::SeqCst);
        // Emulate the gap between the guest's CAS and memory.atomic.wait.
        m.sync_word(64).unwrap().store(1, Ordering::SeqCst);
        let waiter = m.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            let result = waiter
                .0
                .atomic_wait32(64, 1, Some(std::time::Duration::from_secs(2)))
                .unwrap();
            waiter.sync_word(64).unwrap().store(0, Ordering::SeqCst);
            format!("{result:?}")
        });
        m.signal(64).unwrap();
        assert_eq!(thread.join().unwrap(), "Ok");
        assert!(m.signal(65).is_err());
    }

    #[test]
    fn shared_memory_is_bounded_and_strings_are_terminated() {
        let m = memory();
        m.c_string(32, 6, "hello").unwrap();
        assert_eq!(m.string(32, 6).unwrap(), "hello");
        assert!(m.string(32, 5).is_err());
        assert!(m.c_string(32, 5, "hello").is_err());
        assert!(m.write(65535, &[1, 2]).is_err());
        assert!(m.read(u32::MAX, 4).is_err());
        m.set_u32(100, 0x12345678).unwrap();
        assert_eq!(m.u32(100).unwrap(), 0x12345678);
    }
}
