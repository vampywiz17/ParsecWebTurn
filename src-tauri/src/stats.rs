use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionStats {
    pub state: String,
    pub route: Option<String>,
    pub protocol: Option<String>,
    pub dtls_state: Option<String>,
    pub tls_version: Option<String>,
    pub dtls_cipher: Option<String>,
    pub srtp_cipher: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_bitrate_kbps: Option<f64>,
    pub audio_sample_rate: Option<u32>,
    pub audio_channels: Option<u32>,
    pub audio_source: Option<String>,
    pub app_cpu_percent: Option<f64>,
    pub app_gpu_percent: Option<f64>,
    pub app_gpu_decode_percent: Option<f64>,
    pub route_evidence: Option<String>,
    pub turn_protocol: Option<String>,
    pub configured_turn_used: Option<bool>,
    pub turn_server: Option<String>,
    pub local_candidate_type: Option<String>,
    pub remote_candidate_type: Option<String>,
    pub rtt_ms: Option<f64>,
    pub inbound_mbps: Option<f64>,
    pub outbound_mbps: Option<f64>,
    pub fps: Option<f64>,
    pub fps_source: Option<String>,
    pub codec: Option<String>,
    pub decoder: Option<String>,
    pub video_profile: Option<String>,
    pub decoder_backend: Option<String>,
    pub hardware_decode: Option<bool>,
    #[serde(default)]
    pub media_diagnostics_enabled: bool,
    pub video_source: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub packets_lost: Option<u64>,
    pub peer_connections: u32,
    #[serde(default)]
    pub stale: bool,
}

impl ConnectionStats {
    pub fn validate(&self) -> Result<(), String> {
        if ![
            "waiting",
            "new",
            "connecting",
            "connected",
            "disconnected",
            "failed",
            "closed",
        ]
        .contains(&self.state.as_str())
            || self
                .route
                .as_ref()
                .is_some_and(|s| s != "direct" && s != "relay")
            || self
                .protocol
                .as_ref()
                .is_some_and(|s| !["udp", "tcp", "tls"].contains(&s.as_str()))
            || self
                .turn_protocol
                .as_ref()
                .is_some_and(|s| !["udp", "tcp", "tls"].contains(&s.as_str()))
            || self
                .turn_server
                .as_ref()
                .is_some_and(|s| s.len() > 512 || s.chars().any(char::is_control))
            || [
                &self.codec,
                &self.decoder,
                &self.video_profile,
                &self.decoder_backend,
                &self.video_source,
                &self.fps_source,
                &self.route_evidence,
                &self.dtls_cipher,
                &self.srtp_cipher,
                &self.audio_codec,
                &self.audio_source,
            ]
            .into_iter()
            .flatten()
            .any(|s| s.len() > 128)
            || self.peer_connections > 1000
            || [&self.local_candidate_type, &self.remote_candidate_type]
                .into_iter()
                .flatten()
                .any(|s| !["host", "srflx", "prflx", "relay"].contains(&s.as_str()))
        {
            return Err("Invalid connection statistics".into());
        }
        if self.dtls_state.as_ref().is_some_and(|s| {
            !["new", "connecting", "connected", "closed", "failed"].contains(&s.as_str())
        }) || self.tls_version.as_ref().is_some_and(|s| {
            s.len() != 4
                || !s
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(&c))
        }) || [&self.dtls_cipher, &self.srtp_cipher]
            .into_iter()
            .flatten()
            .any(|s| {
                s.is_empty()
                    || !s
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
            })
        {
            return Err("Invalid encryption statistics".into());
        }
        for value in [
            self.rtt_ms,
            self.inbound_mbps,
            self.outbound_mbps,
            self.fps,
            self.audio_bitrate_kbps,
        ]
        .into_iter()
        .flatten()
        {
            if !value.is_finite() || !(0.0..=1_000_000.0).contains(&value) {
                return Err("Invalid connection statistics".into());
            }
        }
        if [
            self.app_cpu_percent,
            self.app_gpu_percent,
            self.app_gpu_decode_percent,
        ]
        .into_iter()
        .flatten()
        .any(|value| !value.is_finite() || !(0.0..=100.0).contains(&value))
            || self
                .audio_sample_rate
                .is_some_and(|value| value == 0 || value > 768000)
            || self
                .audio_channels
                .is_some_and(|value| value == 0 || value > 32)
        {
            return Err("Invalid audio or performance statistics".into());
        }
        Ok(())
    }
}

pub struct LatestStats {
    pub value: ConnectionStats,
    pub received: Option<Instant>,
}

impl Default for LatestStats {
    fn default() -> Self {
        Self {
            value: ConnectionStats {
                state: "waiting".into(),
                ..Default::default()
            },
            received: None,
        }
    }
}

impl LatestStats {
    pub fn snapshot(&self) -> ConnectionStats {
        let mut value = self.value.clone();
        value.stale = self
            .received
            .is_some_and(|time| time.elapsed().as_secs() > 5);
        if value.stale {
            value.inbound_mbps = None;
            value.outbound_mbps = None;
            value.rtt_ms = None;
            value.fps = None;
            value.fps_source = None;
            value.dtls_state = None;
            value.tls_version = None;
            value.dtls_cipher = None;
            value.srtp_cipher = None;
            value.audio_codec = None;
            value.audio_bitrate_kbps = None;
            value.audio_sample_rate = None;
            value.audio_channels = None;
            value.audio_source = None;
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_telemetry_and_expires_old_samples() {
        let mut value = ConnectionStats {
            state: "connected".into(),
            rtt_ms: Some(12.0),
            ..Default::default()
        };
        value.validate().unwrap();
        value.rtt_ms = Some(f64::NAN);
        assert!(value.validate().is_err());
        value.rtt_ms = Some(12.0);
        let latest = LatestStats {
            value,
            received: Some(Instant::now() - std::time::Duration::from_secs(6)),
        };
        assert!(latest.snapshot().stale);
        assert_eq!(latest.snapshot().rtt_ms, None);
    }

    #[test]
    fn encryption_and_audio_validate_and_expire_without_claiming_current_security() {
        let mut value = ConnectionStats {
            state: "connected".into(),
            dtls_state: Some("connected".into()),
            tls_version: Some("FEFD".into()),
            dtls_cipher: Some("TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256".into()),
            audio_codec: Some("opus".into()),
            audio_bitrate_kbps: Some(64.0),
            audio_sample_rate: Some(48000),
            audio_channels: Some(2),
            ..Default::default()
        };
        value.validate().unwrap();
        value.dtls_cipher = Some("bad\nvalue".into());
        assert!(value.validate().is_err());
        value.dtls_cipher = None;
        value.audio_channels = Some(100);
        assert!(value.validate().is_err());
        value.audio_channels = Some(2);
        let latest = LatestStats {
            value,
            received: Some(Instant::now() - std::time::Duration::from_secs(6)),
        };
        let sample = latest.snapshot();
        assert!(sample.stale);
        assert!(sample.dtls_state.is_none());
        assert!(sample.tls_version.is_none());
        assert!(sample.audio_bitrate_kbps.is_none());
    }
}
