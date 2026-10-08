//! Snapshot-specific Parsec ABI state. Transport and decoding are separate:
//! initializing this object must never imply an established session.
use anyhow::{bail, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::VecDeque;

#[derive(Default, Serialize)]
pub struct Backend {
    pub initialized: bool,
    pub generation: u64,
    pub status: Option<i32>,
    pub video_protocol: Option<VideoProtocol>,
    pub idle_messages_discarded: u64,
    #[serde(skip)]
    pub native_attempt: Option<crate::attempt::Attempt>,
    #[serde(skip)]
    events: VecDeque<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VideoProtocol {
    pub version: u32,
    pub message_size: u32,
    pub version_offset: u32,
    pub flag_offset: u32,
}

impl Backend {
    pub fn init(&mut self) {
        if !self.initialized {
            self.initialized = true;
            self.generation += 1;
            self.status = Some(-3); // X constructor in the audited parsec.js.
        }
    }

    pub fn destroy(&mut self) {
        self.native_attempt.take();
        self.initialized = false;
        self.status = None;
        self.video_protocol = None;
        self.events.clear();
        self.idle_messages_discarded = 0;
    }

    pub fn require_initialized(&self) -> Result<()> {
        if !self.initialized {
            bail!("Parsec backend has not been initialized");
        }
        Ok(())
    }

    pub fn discard_idle_message(&mut self) -> Result<()> {
        self.require_initialized()?;
        // Audited h.W / clientSendMessage sends only if a transport exists
        // and status == 0. Preserve its idle no-op, not a fake delivery.
        if self.status == Some(0) {
            bail!("Live message transport is not implemented");
        }
        self.idle_messages_discarded = self.idle_messages_discarded.saturating_add(1);
        Ok(())
    }

    pub fn set_video_protocol(&mut self, protocol: VideoProtocol) -> Result<()> {
        self.require_initialized()?;
        // These offsets are used for 32-bit little-endian header fields in
        // parsec.js. Reject invalid metadata before a future decoder reads it.
        if protocol.message_size > 1024 * 1024
            || protocol
                .version_offset
                .checked_add(4)
                .is_none_or(|n| n > protocol.message_size)
            || protocol
                .flag_offset
                .checked_add(4)
                .is_none_or(|n| n > protocol.message_size)
        {
            bail!("invalid video protocol header layout");
        }
        self.video_protocol = Some(protocol);
        Ok(())
    }

    pub fn disconnect(&mut self, status: i32, state: i32) -> Result<()> {
        self.require_initialized()?;
        if self.events.len() >= 32 {
            bail!("backend event queue limit reached");
        }
        // No live attempt exists at this milestone. Match the idle JS branch;
        // never produce a connected event or invented attempt identifier.
        if state == 4 && self.status != Some(0) {
            self.events.push_back(json!({
                "type": 7, "status": status, "state": state,
                "attemptID": "", "duration": 0
            }));
        }
        self.native_attempt.take();
        self.status = Some(status);
        Ok(())
    }

    pub fn poll_event(&mut self) -> Option<Value> {
        self.events.pop_front()
    }

    pub fn peek_event(&self) -> Option<&Value> {
        self.events.front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_input_never_claims_a_live_message_delivery() {
        let mut b = Backend::default();
        assert!(b.discard_idle_message().is_err());
        b.init();
        b.discard_idle_message().unwrap();
        assert_eq!(b.idle_messages_discarded, 1);
        assert_eq!(b.status, Some(-3));
        b.status = Some(0);
        assert!(b.discard_idle_message().is_err());
        assert_eq!(b.idle_messages_discarded, 1);
        b.destroy();
        assert_eq!(b.idle_messages_discarded, 0);
    }

    #[test]
    fn lifecycle_is_idempotent_and_reinitialization_resets_state() {
        let mut b = Backend::default();
        assert!(b.require_initialized().is_err());
        b.init();
        b.init();
        assert_eq!(b.generation, 1);
        assert_eq!(b.status, Some(-3));
        b.disconnect(12, 4).unwrap();
        assert_eq!(b.poll_event().unwrap()["status"], 12);
        b.destroy();
        assert!(b.poll_event().is_none());
        assert_eq!(b.status, None);
        b.init();
        assert_eq!(b.generation, 2);
        assert_eq!(b.status, Some(-3));
    }

    #[test]
    fn metadata_rejects_overflow_and_out_of_header_fields() {
        let mut b = Backend::default();
        b.init();
        let good = VideoProtocol {
            version: 1,
            message_size: 16,
            version_offset: 0,
            flag_offset: 12,
        };
        b.set_video_protocol(good.clone()).unwrap();
        let bad = VideoProtocol {
            flag_offset: u32::MAX,
            ..good.clone()
        };
        assert!(b.set_video_protocol(bad).is_err());
        assert_eq!(b.video_protocol, Some(good));
    }
}
