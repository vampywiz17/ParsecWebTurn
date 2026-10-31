//! Native telemetry; detailed host reports follow diagnostic visibility/logging.
//! No payloads, IP addresses, credentials or certificates are recorded here.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Instant,
};
use webrtc::{peer_connection::RTCPeerConnection, stats::StatsReportType};
/// Existing resource sampler output, separate from platform query APIs.
#[derive(Clone, Copy, Default, serde::Serialize)]
pub struct MemoryUsage {
    pub working_set_bytes: usize,
    pub private_committed_bytes: usize,
    pub private_working_set_bytes: Option<usize>,
}
#[derive(Clone)]
pub struct Sample {
    #[cfg(feature = "bud")]
    pub bud: Option<evoparseq_bud::bud::Snapshot>,
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
    pub audio_codec: Option<&'static str>,
    pub audio_layout: Option<crate::audio_layout::Layout>,
    pub audio_decoded_queue_ms: Option<f64>,
    #[cfg(windows)]
    pub audio_playback: Option<crate::audio_windows::Snapshot>,
    pub video: Option<crate::video_output::Snapshot>,
    pub profile: Option<u8>,
    pub codec: Option<crate::video_stream::Codec>,
    pub hevc_advertised: bool,
}
impl Default for Sample {
    fn default() -> Self {
        Self {
            #[cfg(feature = "bud")]
            bud: None,
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
            audio_codec: None,
            audio_layout: None,
            audio_decoded_queue_ms: None,
            #[cfg(windows)]
            audio_playback: None,
            video: None,
            profile: None,
            codec: None,
            hevc_advertised: false,
        }
    }
}
#[derive(Default)]
pub struct Shared {
    pub overlay_visible: AtomicBool,
    #[cfg(windows)]
    pub host_timing: crate::frame_timing::Shared,
    #[cfg(windows)]
    audio: Mutex<Option<std::sync::Weak<crate::audio_windows::Playback>>>,
    pub visible: AtomicBool,
    latest: Mutex<(u64, Option<Sample>)>,
}
impl Shared {
    #[cfg(windows)]
    pub fn set_audio(&self, audio: std::sync::Weak<crate::audio_windows::Playback>) {
        *self.audio.lock().unwrap_or_else(|e| e.into_inner()) = Some(audio);
    }
    pub fn requested(&self) -> bool {
        self.visible.load(Ordering::Acquire) || crate::event_log::enabled()
    }
    pub fn sample_requested(&self) -> bool {
        if self.overlay_visible.load(Ordering::Acquire) {
            return true;
        }
        self.requested()
    }
    pub fn begin(&self) -> u64 {
        let mut data = self.latest.lock().unwrap_or_else(|e| e.into_inner());
        data.0 += 1;
        #[cfg(windows)]
        self.host_timing.begin(data.0);
        data.1 = Some(Sample {
            generation: data.0,
            state: "Connecting".into(),
            ..Default::default()
        });
        crate::event_log::record(
            "INFO",
            "connection_started",
            serde_json::json!({"generation":data.0}),
        );
        data.0
    }
    pub fn publish(&self, generation: u64, mut sample: Sample) {
        #[cfg(windows)]
        {
            sample.audio_playback = self
                .audio
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .and_then(|a| a.upgrade())
                .map(|a| a.snapshot());
        }
        let mut data = self.latest.lock().unwrap_or_else(|e| e.into_inner());
        if data.0 == generation {
            sample.generation = generation;
            #[cfg(windows)]
            if crate::event_log::enabled()
                && sample.audio_playback.as_ref().is_some_and(|audio| {
                    data.1
                        .as_ref()
                        .and_then(|old| old.audio_playback.as_ref())
                        .is_none_or(|old| {
                            old.layout != audio.layout
                                || old.recovering != audio.recovering
                                || old.recoveries != audio.recoveries
                                || old.failure_hresult != audio.failure_hresult
                                || old.last_device_error != audio.last_device_error
                                || (!audio.recovering
                                    && (old.underruns != audio.underruns
                                        || old.frames_dropped != audio.frames_dropped))
                        })
                        || data
                            .1
                            .as_ref()
                            .is_some_and(|old| old.audio_codec != sample.audio_codec)
                })
            {
                let audio = sample.audio_playback.as_ref().unwrap();
                crate::event_log::record(
                    "INFO",
                    "audio_playback_state",
                    serde_json::json!({
                        "codec":sample.audio_codec,"queued_frames":audio.queued_frames,
                        "channels":audio.layout.map(|v| v.channels),"channel_mask":audio.layout.map(|v| v.mask),
                        "device_padding":audio.device_padding,"device_capacity":audio.device_capacity_frames,
                        "minimum_ms":audio.minimum_ms,"maximum_ms":audio.maximum_ms,
                        "custom":audio.custom_buffer,"low_latency_device":audio.low_latency_device,
                        "underruns":audio.underruns,"dropped_frames":audio.frames_dropped,
                        "hresult":audio.failure_hresult,"decoded_queue_ms":sample.audio_decoded_queue_ms,
                        "recovering":audio.recovering,"recoveries":audio.recoveries,"recovery_attempts":audio.recovery_attempts,
                        "last_device_error":audio.last_device_error
                    }),
                );
            }
            if crate::event_log::enabled()
                && data.1.as_ref().is_none_or(|old| {
                    old.state != sample.state
                        || old.route != sample.route
                        || old.protocol != sample.protocol
                })
            {
                crate::event_log::record(
                    "INFO",
                    "connection_state_changed",
                    serde_json::json!({
                        "generation":generation,"state":sample.state,"route":sample.route,
                        "previous_route":data.1.as_ref().map(|old|old.route),"transport":sample.protocol,
                        "turn_transport":sample.turn_protocol,"dtls":sample.dtls,
                    }),
                );
            }
            if crate::event_log::enabled()
                && sample.video.as_ref().is_some_and(|v| {
                    data.1
                        .as_ref()
                        .and_then(|old| old.video.as_ref())
                        .is_none_or(|old| {
                            old.decoder != v.decoder
                                || old.adapter != v.adapter
                                || old.decoded_chroma != v.decoded_chroma
                                || old.decoded_bit_depth != v.decoded_bit_depth
                                || old.decoder_fallback != v.decoder_fallback
                                || old.display_rgb_bit_depth != v.display_rgb_bit_depth
                                || old.display_output_fallback != v.display_output_fallback
                                || old.failure_stage != v.failure_stage
                        })
                })
            {
                let v = sample.video.as_ref().unwrap();
                crate::event_log::record(
                    if v.failure_stage.is_some() {
                        "ERROR"
                    } else {
                        "INFO"
                    },
                    "decoder_state_changed",
                    serde_json::json!({
                        "generation":generation,"decoder":v.decoder,"fallback":v.decoder_fallback.is_some(),
                        "gpu_adapter":v.adapter,"decoded_chroma":v.decoded_chroma,
                        "fallback_reason":v.decoder_fallback,
                        "stage":v.failure_stage,"hresult":v.failure_hresult,"decoded_bit_depth":v.decoded_bit_depth,
                        "display_rgb_bit_depth":v.display_rgb_bit_depth,"display_output_fallback":v.display_output_fallback,
                    }),
                );
            }
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
        #[cfg(windows)]
        self.host_timing.end(generation);
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
    sample.rtt_ms =
        crate::rtc_configuration::selected_rtt(&report, &local.stats_id, &remote.stats_id);
    for stats in report.reports.values() {
        match stats {
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
impl Sample {
    fn connected(&self) -> bool {
        #[cfg(feature = "bud")]
        if let Some(bud) = &self.bud {
            return bud.connected;
        }
        self.state == "connected"
    }
}
#[derive(Default)]
pub struct Rates {
    previous: Option<Sample>,
    recent: crate::stats_recent::Recent,
}
#[derive(Default)]
pub struct Values {
    pub incoming: Option<f64>,
    pub outgoing: Option<f64>,
    pub fps: Option<f64>,
    pub present_fps: Option<f64>,
    pub recent_timing_ms: [Option<f64>; 8],
    pub audio: Option<f64>,
}
impl Rates {
    pub fn update(&mut self, sample: &Sample) -> Values {
        let mut result = Values {
            recent_timing_ms: self.recent.update(
                sample,
                sample.connected() && sample.at.elapsed().as_secs() <= 4,
            ),
            ..Default::default()
        };
        if let Some(old) = &self.previous {
            let seconds = sample.at.saturating_duration_since(old.at).as_secs_f64();
            if sample.generation == old.generation
                && sample.connected()
                && old.connected()
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
                result.present_fps = rate(
                    sample
                        .video
                        .as_ref()
                        .filter(|v| v.decoder_initialized)
                        .map(|v| v.frames_presented),
                    old.video
                        .as_ref()
                        .filter(|v| v.decoder_initialized)
                        .map(|v| v.frames_presented),
                    1.0,
                );
            }
        }
        // Don't discard the baseline if the UI timer reads the same cached sample twice.
        if self
            .previous
            .as_ref()
            .is_none_or(|p| p.at != sample.at || p.generation != sample.generation)
        {
            self.previous = Some(sample.clone());
        }
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decode_and_submission_rates_are_distinct_and_reset_independently() {
        let mut rates = Rates::default();
        let mut sample = Sample {
            generation: 1,
            state: "connected".into(),
            video: Some(crate::video_output::Snapshot {
                decoder_initialized: true,
                frames_decoded: 10,
                frames_presented: 5,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(rates.update(&sample).present_fps.is_none());
        sample.at += std::time::Duration::from_secs(1);
        sample.video.as_mut().unwrap().frames_decoded += 60;
        sample.video.as_mut().unwrap().frames_presented += 30;
        let values = rates.update(&sample);
        assert_eq!(values.fps, Some(60.0));
        assert_eq!(values.present_fps, Some(30.0));
        sample.at += std::time::Duration::from_secs(1);
        sample.video.as_mut().unwrap().frames_presented = 0;
        assert!(rates.update(&sample).present_fps.is_none());
        sample.generation += 1;
        assert!(rates.update(&sample).fps.is_none());
        sample.at += std::time::Duration::from_secs(1);
        sample.video.as_mut().unwrap().frames_decoded += 5;
        assert_eq!(rates.update(&sample).fps, Some(5.0));
        sample.state = "disconnected".into();
        sample.at += std::time::Duration::from_secs(1);
        let values = rates.update(&sample);
        assert!(values.present_fps.is_none() && values.fps.is_none());
        assert_eq!(values.recent_timing_ms, [None; 8]);
    }
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
    #[cfg(feature = "bud")]
    #[test]
    fn bud_rates_use_native_connection_state_and_existing_decoder_counters() {
        let mut rates = Rates::default();
        let mut sample = Sample {
            generation: 1,
            state: "Connected (BUD)".into(),
            bud: Some(evoparseq_bud::bud::Snapshot {
                connected: true,
                ..Default::default()
            }),
            received: Some(100),
            sent: Some(200),
            audio_bytes: 10,
            video: Some(crate::video_output::Snapshot {
                decoder_initialized: true,
                frames_decoded: 10,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(rates.update(&sample).fps.is_none());
        sample.at += std::time::Duration::from_secs(2);
        sample.received = Some(250_100);
        sample.sent = Some(25_200);
        sample.audio_bytes = 32_010;
        sample.video.as_mut().unwrap().frames_decoded = 130;
        let values = rates.update(&sample);
        assert_eq!(values.fps, Some(60.0));
        assert_eq!(values.incoming, Some(1.0));
        assert_eq!(values.audio, Some(128.0));
        assert!((values.outgoing.unwrap() - 0.1).abs() < 1e-12);
        sample.at += std::time::Duration::from_secs(1);
        sample.bud.as_mut().unwrap().connected = false;
        let stopped = rates.update(&sample);
        assert!(stopped.fps.is_none() && stopped.incoming.is_none());
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
