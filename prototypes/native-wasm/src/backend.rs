//! Snapshot-specific Parsec ABI state. Transport and decoding are separate:
//! initializing this object must never imply an established session.
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::VecDeque;

#[derive(Default, Serialize)]
pub struct Backend {
    pub initialized: bool,
    pub cloudflare_stun_enabled: bool,
    pub legacy_rsa_1024_enabled: bool,
    pub generation: u64,
    pub status: Option<i32>,
    pub video_protocol: Option<VideoProtocol>,
    pub idle_messages_discarded: u64,
    pub host_mode: i32,
    pub encode_latency: f32,
    pub control_frames_received: u64,
    pub media_ingress: crate::media_ingress::Ingress,
    pub attempt_failure: Option<crate::attempt::FailureStage>,
    pub attempt_diagnostic: Option<Value>,
    pub remote_begin_diagnostic: Option<Value>,
    pub previous_attempt_diagnostics: VecDeque<Value>,
    pub previous_attempts_omitted: u64,
    #[serde(skip)]
    pub guests: Vec<Value>,
    #[serde(skip)]
    pub me: Value,
    #[serde(skip)]
    pub attempt_id: String,
    #[serde(skip)]
    pub attempt_started: Option<std::time::Instant>,
    #[serde(skip)]
    pub native_attempt: Option<crate::attempt::Attempt>,
    #[serde(skip)]
    pub buffers: crate::buffers::Buffers,
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
        self.retain_attempt_diagnostic();
        self.initialized = false;
        self.status = None;
        self.video_protocol = None;
        self.events.clear();
        self.buffers.clear();
        self.idle_messages_discarded = 0;
        self.host_mode = 0;
        self.encode_latency = 0.0;
        self.control_frames_received = 0;
        self.media_ingress = Default::default();
        self.guests.clear();
        self.me = Value::Null;
        self.attempt_id.clear();
        self.attempt_started = None;
    }

    pub fn require_initialized(&self) -> Result<()> {
        if !self.initialized {
            bail!("Parsec backend has not been initialized");
        }
        Ok(())
    }

    pub fn prepare_attempt(&mut self) {
        if self.attempt_diagnostic.is_some()
            || self.remote_begin_diagnostic.is_some()
            || self.attempt_failure.is_some()
        {
            if self.previous_attempt_diagnostics.len() == 4 {
                self.previous_attempt_diagnostics.pop_front();
                self.previous_attempts_omitted = self.previous_attempts_omitted.saturating_add(1);
            }
            self.previous_attempt_diagnostics.push_back(json!({
                "attempt":self.attempt_diagnostic,
                "failure":self.attempt_failure,
                "remote_begin":self.remote_begin_diagnostic
            }));
        }
        self.attempt_failure = None;
        self.attempt_diagnostic = None;
        self.remote_begin_diagnostic = None;
        self.events.clear();
        self.buffers.clear();
        self.guests.clear();
        self.me = Value::Null;
        self.host_mode = 0;
        self.encode_latency = 0.0;
        self.control_frames_received = 0;
        self.media_ingress = Default::default();
    }

    /// Include a live attempt without consuming, cancelling or pumping it.
    pub fn diagnostic(&self) -> Result<Value> {
        let mut value = serde_json::to_value(self)?;
        value["active_attempt_diagnostic"] = self
            .native_attempt
            .as_ref()
            .map(|a| a.snapshot())
            .unwrap_or(Value::Null);
        Ok(value)
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
        let event = if state == 4 && self.status != Some(0) {
            Some(json!({
                "type": 7, "status": status, "state": state,
                "attemptID": self.attempt_id, "duration": 0
            }))
        } else if state == 8 && self.status == Some(0) {
            Some(
                json!({"type":7,"status":0,"state":state,"attemptID":self.attempt_id,
                "duration":self.attempt_started.map_or(0,|t|t.elapsed().as_secs())}),
            )
        } else {
            None
        };
        self.retain_attempt_diagnostic();
        self.events
            .retain(|event| !matches!(event["type"].as_i64(), Some(1 | 3)));
        self.buffers.clear();
        self.status = Some(status);
        self.guests.clear();
        self.me = Value::Null;
        self.host_mode = 0;
        if let Some(event) = event {
            if self.events.len() >= 32 {
                bail!("backend event queue limit reached after disconnect cleanup");
            }
            self.events.push_back(event);
        }
        Ok(())
    }

    pub fn poll_event(&mut self) -> Option<Value> {
        self.events.pop_front()
    }

    fn retain_attempt_diagnostic(&mut self) {
        if let Some(attempt) = self.native_attempt.take() {
            self.attempt_diagnostic = Some(attempt.snapshot());
        }
    }

    /// Expected connection failure is an app event, not a Wasmtime trap.
    pub fn fail_native_attempt(&mut self, stage: crate::attempt::FailureStage) -> Result<()> {
        self.attempt_failure = Some(stage);
        let connected = self.status == Some(0);
        // Reserve space for the single failure event even if ICE events filled
        // the queue. No success or previously queued media can survive failure.
        self.events.clear();
        self.disconnect(-6200, 4)?;
        if connected {
            self.events.push_back(json!({"type":7,"status":-6200,"state":8,"attemptID":self.attempt_id,"duration":self.attempt_started.map_or(0,|t|t.elapsed().as_secs())}));
        }
        Ok(())
    }

    pub fn pump_native_events(&mut self) -> Result<()> {
        if let Err(error) = self.pump() {
            let stage = error
                .downcast_ref::<crate::attempt::FailureStage>()
                .copied()
                .unwrap_or(crate::attempt::FailureStage::InboundControl);
            // Malformed/unsupported inbound traffic fails this connection,
            // preserving its snapshot and notifying the guest, not trapping WASM.
            self.fail_native_attempt(stage)?;
        }
        Ok(())
    }

    fn pump(&mut self) -> Result<()> {
        if let Some(attempt) = &self.native_attempt {
            while self.events.len() < 32 {
                let Some(event) = attempt.pop_event() else {
                    break;
                };
                if event["type"] == 7 {
                    self.status =
                        Some(i32::try_from(event["status"].as_i64().ok_or_else(
                            || anyhow::anyhow!("native status event missing value"),
                        )?)?);
                }
                self.events.push_back(event);
            }
        }
        if self
            .native_attempt
            .as_ref()
            .is_some_and(|a| a.control_ready())
        {
            // Media packets do not enqueue UI events. Bound work even while
            // their producer is faster than the UI's polling loop.
            for _ in 0..32 {
                if self.events.len() >= 32 {
                    break;
                }
                let Some((channel, text, bytes)) =
                    self.native_attempt.as_ref().and_then(|a| a.pop_binary())
                else {
                    break;
                };
                self.handle_message(channel, text, bytes)?;
            }
        }
        Ok(())
    }

    pub(crate) fn handle_message(
        &mut self,
        channel: u16,
        text: bool,
        bytes: bytes::Bytes,
    ) -> Result<()> {
        if text || channel > 2 {
            return Err(crate::attempt::FailureStage::InboundChannel.into());
        }
        if channel != 0 {
            self.media_ingress.unavailable(channel, bytes.len());
            return Ok(());
        }
        let decoded =
            crate::control::decode(&bytes).context(crate::attempt::FailureStage::InboundControl)?;
        self.control_frames_received = self.control_frames_received.saturating_add(1);
        match decoded {
            crate::control::Message::Status(status) => {
                self.status = Some(status);
                self.events.push_back(json!({"type":7,"status":status,"state":8,"attemptID":self.attempt_id,"duration":self.attempt_started.map_or(0,|t|t.elapsed().as_secs())}));
            }
            crate::control::Message::EncodeLatency(value) => self.encode_latency = value,
            crate::control::Message::Event(event) => self.events.push_back(event),
            crate::control::Message::HostMode(value) => self.host_mode = value,
            crate::control::Message::Guests { list, me } => {
                self.guests = list;
                self.me = me;
            }
            crate::control::Message::Buffer { mut event, payload } => {
                let key = if let Some(range) = payload {
                    self.buffers.insert(bytes, range)?
                } else {
                    0
                };
                event["key"] = json!(key);
                self.events.push_back(event);
            }
            crate::control::Message::Ignored => {}
        }
        Ok(())
    }

    pub fn peek_event(&self) -> Option<&Value> {
        self.events.front()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn retry_history_keeps_failure_and_is_bounded_without_ids() {
        let mut b = Backend::default();
        b.init();
        b.attempt_id = "private-attempt-sentinel".into();
        for index in 0..6 {
            b.attempt_diagnostic = Some(json!({"remote_candidates":index}));
            b.attempt_failure = Some(crate::attempt::FailureStage::Deadline);
            b.prepare_attempt();
        }
        let value = b.diagnostic().unwrap();
        assert_eq!(
            value["previous_attempt_diagnostics"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        assert_eq!(value["previous_attempts_omitted"], 2);
        assert_eq!(
            value["previous_attempt_diagnostics"][0]["attempt"]["remote_candidates"],
            2
        );
        assert_eq!(
            value["previous_attempt_diagnostics"][3]["failure"],
            "deadline"
        );
        assert!(value["active_attempt_diagnostic"].is_null());
        assert!(!value.to_string().contains("private-attempt-sentinel"));
    }
    #[test]
    fn transport_failure_replaces_pending_events_and_cleans_buffers() {
        for status in [20, 0] {
            let mut b = Backend::default();
            b.init();
            b.status = Some(status);
            let key = b
                .buffers
                .insert(bytes::Bytes::from_static(b"unread"), 0..6)
                .unwrap();
            for _ in 0..32 {
                b.events.push_back(json!({"type":2}));
            }
            b.fail_native_attempt(crate::attempt::FailureStage::RemoteCandidate)
                .unwrap();
            assert_eq!(b.status, Some(-6200));
            assert_eq!(b.buffers.size(key), 0);
            let event = b.poll_event().unwrap();
            assert_eq!(event["status"], -6200);
            assert_eq!(event["state"], if status == 0 { 8 } else { 4 });
            assert!(b.poll_event().is_none());
            assert_eq!(
                serde_json::to_value(&b).unwrap()["attempt_failure"],
                "remote-candidate"
            );
            b.prepare_attempt();
            assert!(b.attempt_failure.is_none());
        }
    }
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
    #[test]
    fn guest_handles_expire_across_attempt_and_backend_lifecycles() {
        let mut b = Backend::default();
        b.init();
        let first = b
            .buffers
            .insert(bytes::Bytes::from_static(b"one"), 0..3)
            .unwrap();
        b.prepare_attempt();
        assert_eq!(b.buffers.size(first), 0);
        let second = b
            .buffers
            .insert(bytes::Bytes::from_static(b"two"), 0..3)
            .unwrap();
        b.disconnect(-3, 4).unwrap();
        assert_eq!(b.buffers.size(second), 0);
        b.destroy();
        b.init();
        let third = b
            .buffers
            .insert(bytes::Bytes::from_static(b"three"), 0..5)
            .unwrap();
        assert!(first < second && second < third);
        assert_eq!(b.buffers.size(first), 0);
    }
    #[test]
    fn full_event_queue_cannot_prevent_disconnect_buffer_cleanup() {
        let mut b = Backend::default();
        b.init();
        let key = b
            .buffers
            .insert(bytes::Bytes::from_static(b"unread"), 0..6)
            .unwrap();
        for _ in 0..32 {
            b.events.push_back(json!({"type":2}));
        }
        assert!(b.disconnect(-3, 4).is_err());
        assert_eq!(b.buffers.size(key), 0);
        assert_eq!(b.status, Some(-3));
    }
}
