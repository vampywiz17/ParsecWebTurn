//! Read-only native telemetry. No payloads, IP addresses, credentials or certificates.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Instant,
};
use webrtc::{peer_connection::RTCPeerConnection, stats::StatsReportType};
#[derive(Clone)]
pub struct Sample {
    pub at: Instant,
    pub generation: u64,
    pub state: String,
    pub dtls: String,
    pub route: &'static str,
    pub local: String,
    pub remote: String,
    pub protocol: String,
    pub turn_server: Option<String>,
    pub turn_protocol: Option<String>,
    pub rtt_ms: Option<f64>,
    pub received: Option<u64>,
    pub sent: Option<u64>,
    pub audio_bytes: u64,
    pub audio_ready: bool,
    pub video: Option<crate::video_output::Snapshot>,
    pub profile: Option<u8>,
}
impl Default for Sample {
    fn default() -> Self {
        Self {
            at: Instant::now(),
            generation: 0,
            state: "Waiting for a connection".into(),
            dtls: "Not reported".into(),
            route: "Unknown",
            local: String::new(),
            remote: String::new(),
            protocol: String::new(),
            turn_server: None,
            turn_protocol: None,
            rtt_ms: None,
            received: None,
            sent: None,
            audio_bytes: 0,
            audio_ready: false,
            video: None,
            profile: None,
        }
    }
}
#[derive(Default)]
pub struct Shared {
    pub visible: AtomicBool,
    latest: Mutex<(u64, Option<Sample>)>,
}
impl Shared {
    pub fn requested(&self) -> bool {
        self.visible.load(Ordering::Acquire)
    }
    pub fn begin(&self) -> u64 {
        let mut data = self.latest.lock().unwrap_or_else(|e| e.into_inner());
        data.0 += 1;
        data.1 = Some(Sample {
            generation: data.0,
            state: "Connecting".into(),
            ..Default::default()
        });
        data.0
    }
    pub fn publish(&self, generation: u64, mut sample: Sample) {
        let mut data = self.latest.lock().unwrap_or_else(|e| e.into_inner());
        if data.0 == generation {
            sample.generation = generation;
            data.1 = Some(sample);
        }
    }
    pub fn read(&self) -> Option<Sample> {
        self.latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .1
            .clone()
    }
    pub fn finish(&self, generation: u64) {
        self.publish(
            generation,
            Sample {
                state: "Disconnected".into(),
                ..Default::default()
            },
        );
    }
}
/// Remote prflx cannot establish whether a remote TURN allocation is hidden.
pub fn route(local: &str, remote: &str) -> &'static str {
    if local == "relay" || remote == "relay" {
        "Relay — TURN in use"
    } else if matches!(local, "host" | "srflx" | "prflx") && matches!(remote, "host" | "srflx") {
        "Direct — no TURN"
    } else {
        "Unverified"
    }
}
pub async fn network(peer: &RTCPeerConnection) -> Sample {
    let dtls = peer.sctp().transport();
    let mut sample = Sample {
        state: peer.connection_state().to_string(),
        dtls: dtls.state().to_string(),
        ..Default::default()
    };
    let Some(pair) = dtls.ice_transport().get_selected_candidate_pair().await else {
        return sample;
    };
    let local = pair.local();
    let remote = pair.remote();
    sample.local = local.typ.to_string();
    sample.remote = remote.typ.to_string();
    sample.protocol = local.protocol.to_string();
    sample.route = route(&sample.local, &sample.remote);
    let id = format!("{}-{}", local.stats_id, remote.stats_id);
    let report = peer.get_stats().await;
    // A route change during sampling must not label a previous pair as active.
    if dtls
        .ice_transport()
        .get_selected_candidate_pair()
        .await
        .as_ref()
        .is_none_or(|active| {
            active.local().stats_id != local.stats_id || active.remote().stats_id != remote.stats_id
        })
    {
        return Sample {
            state: sample.state,
            dtls: sample.dtls,
            ..Default::default()
        };
    }
    for stats in report.reports.values() {
        match stats {
            StatsReportType::CandidatePair(p) if p.id == id && p.responses_received > 0 => {
                if p.current_round_trip_time.is_finite() && p.current_round_trip_time >= 0.0 {
                    sample.rtt_ms = Some(p.current_round_trip_time * 1000.0);
                }
            }
            StatsReportType::SCTPTransport(t) => {
                sample.received = Some(t.bytes_received as u64);
                sample.sent = Some(t.bytes_sent as u64);
            }
            StatsReportType::LocalCandidate(c)
                if c.id == local.stats_id && sample.local == "relay" =>
            {
                // These fields are allocation provenance from our ICE gatherer, not DNS guesses.
                if !c.url.is_empty() {
                    sample.turn_server = Some(c.url.clone());
                }
                if !c.relay_protocol.is_empty() {
                    sample.turn_protocol = Some(c.relay_protocol.clone());
                }
            }
            _ => {}
        }
    }
    sample
}
#[derive(Default)]
pub struct Rates {
    previous: Option<Sample>,
}
#[derive(Default)]
pub struct Values {
    pub incoming: Option<f64>,
    pub outgoing: Option<f64>,
    pub fps: Option<f64>,
    pub audio: Option<f64>,
}
impl Rates {
    pub fn update(&mut self, sample: &Sample) -> Values {
        let mut result = Values::default();
        if let Some(old) = &self.previous {
            let seconds = sample.at.saturating_duration_since(old.at).as_secs_f64();
            if sample.generation == old.generation
                && sample.state == "connected"
                && old.state == "connected"
                && seconds > 0.0
            {
                let rate = |now: Option<u64>, before: Option<u64>, scale: f64| {
                    now.zip(before)
                        .and_then(|(n, b)| n.checked_sub(b))
                        .map(|delta| delta as f64 / seconds * scale)
                };
                result.incoming = rate(sample.received, old.received, 8.0 / 1e6);
                result.outgoing = rate(sample.sent, old.sent, 8.0 / 1e6);
                result.audio = rate(
                    Some(sample.audio_bytes),
                    Some(old.audio_bytes),
                    8.0 / 1000.0,
                );
                result.fps = rate(
                    sample
                        .video
                        .as_ref()
                        .filter(|v| v.decoder_initialized)
                        .map(|v| v.frames_decoded),
                    old.video
                        .as_ref()
                        .filter(|v| v.decoder_initialized)
                        .map(|v| v.frames_decoded),
                    1.0,
                );
            }
        }
        // Don't discard the baseline if the UI timer reads the same cached sample twice.
        if self.previous.as_ref().is_none_or(|p| p.at != sample.at) {
            self.previous = Some(sample.clone());
        }
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routes_require_positive_evidence() {
        assert_eq!(route("relay", "prflx"), "Relay — TURN in use");
        assert_eq!(route("host", "relay"), "Relay — TURN in use");
        assert_eq!(route("prflx", "host"), "Direct — no TURN");
        assert_eq!(route("srflx", "srflx"), "Direct — no TURN");
        assert_eq!(route("prflx", "prflx"), "Unverified");
        assert_eq!(route("", ""), "Unverified");
    }
    #[test]
    fn rates_reset_on_new_session_and_counter_reset() {
        let mut rates = Rates::default();
        let mut s = Sample {
            generation: 1,
            state: "connected".into(),
            received: Some(100),
            ..Default::default()
        };
        assert!(rates.update(&s).incoming.is_none());
        s.at += std::time::Duration::from_secs(2);
        s.received = Some(250100);
        assert_eq!(rates.update(&s).incoming, Some(1.0));
        s.at += std::time::Duration::from_secs(1);
        s.received = Some(0);
        assert!(rates.update(&s).incoming.is_none());
        s.at += std::time::Duration::from_secs(1);
        s.generation = 2;
        s.received = Some(1000);
        assert!(rates.update(&s).incoming.is_none());
    }
    #[test]
    fn late_old_session_never_overwrites_new_session() {
        let bus = Shared::default();
        let old = bus.begin();
        let current = bus.begin();
        bus.publish(
            current,
            Sample {
                state: "connected".into(),
                ..Default::default()
            },
        );
        bus.finish(old);
        assert_eq!(bus.read().unwrap().state, "connected");
        bus.finish(current);
        assert_eq!(bus.read().unwrap().state, "Disconnected");
        assert!(!bus.visible.load(Ordering::Relaxed));
    }
}
