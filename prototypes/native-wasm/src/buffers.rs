//! Opaque guest handles. Payload slices retain their incoming frame without a
//! second Rust-side copy; accounting includes the whole retained frame.
use anyhow::{bail, Result};
use bytes::Bytes;
use std::{collections::BTreeMap, ops::Range};

struct Payload {
    bytes: Bytes,
    retained: usize,
}

pub struct Buffers {
    next: u32,
    retained: usize,
    entries: BTreeMap<u32, Payload>,
}

impl Default for Buffers {
    fn default() -> Self {
        Self {
            next: 1,
            retained: 0,
            entries: BTreeMap::new(),
        }
    }
}

impl Buffers {
    pub fn insert(&mut self, frame: Bytes, range: Range<usize>) -> Result<u32> {
        if range.start > range.end || range.end > frame.len() || frame.len() > 1024 * 1024 {
            bail!("invalid retained buffer range/size");
        }
        if self.entries.len() >= 16
            || self.retained + frame.len() > 4 * 1024 * 1024
            || self.next == u32::MAX
        {
            bail!("guest buffer budget/handle limit reached");
        }
        let key = self.next;
        self.next += 1;
        self.retained += frame.len();
        self.entries.insert(
            key,
            Payload {
                bytes: frame.slice(range),
                retained: frame.len(),
            },
        );
        Ok(key)
    }
    pub fn get(&self, key: u32) -> Option<&Bytes> {
        self.entries.get(&key).map(|p| &p.bytes)
    }
    pub fn size(&self, key: u32) -> u32 {
        self.get(key).map_or(0, |b| b.len() as u32)
    }
    pub fn remove(&mut self, key: u32) {
        if let Some(payload) = self.entries.remove(&key) {
            self.retained -= payload.retained;
        }
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.retained = 0;
        // Never recycle a handle across disconnect/destroy/reinitialization.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_copy_handles_are_one_shot_and_not_recycled_after_reset() {
        let mut store = Buffers::default();
        let frame = Bytes::from_static(b"header-payload");
        let key = store.insert(frame.clone(), 7..14).unwrap();
        assert_eq!(store.get(key).unwrap().as_ptr(), frame[7..].as_ptr());
        assert_eq!(store.size(key), 7);
        store.remove(key);
        assert_eq!(store.size(key), 0);
        let next = store.insert(frame.clone(), 7..14).unwrap();
        store.clear();
        assert!(store.get(next).is_none());
        assert!(store.insert(frame, 0..0).unwrap() > next);
        assert_eq!(store.size(0), 0);
    }
    #[test]
    fn budgets_include_unused_frame_bytes_and_empty_handles() {
        let mut store = Buffers::default();
        for _ in 0..4 {
            store
                .insert(Bytes::from(vec![0; 1024 * 1024]), 0..0)
                .unwrap();
        }
        assert!(store.insert(Bytes::from_static(b"x"), 0..0).is_err());
        assert_eq!(store.retained, 4 * 1024 * 1024);
        store.clear();
        for _ in 0..16 {
            store.insert(Bytes::new(), 0..0).unwrap();
        }
        assert!(store.insert(Bytes::new(), 0..0).is_err());
        assert!(store.insert(Bytes::new(), 0..1).is_err());
        store.clear();
        store.next = u32::MAX;
        assert!(store.insert(Bytes::new(), 0..0).is_err());
    }
}
