use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionStats {
    pub state: String,
    pub route: Option<String>,
    pub protocol: Option<String>,
    pub rtt_ms: Option<f64>,
    pub inbound_mbps: Option<f64>,
    pub outbound_mbps: Option<f64>,
    pub fps: Option<f64>,
    pub codec: Option<String>,
    pub decoder: Option<String>,
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
            || [&self.codec, &self.decoder]
                .into_iter()
                .flatten()
                .any(|s| s.len() > 128)
            || self.peer_connections > 1000
        {
            return Err("Invalid connection statistics".into());
        }
        for value in [self.rtt_ms, self.inbound_mbps, self.outbound_mbps, self.fps]
            .into_iter()
            .flatten()
        {
            if !value.is_finite() || !(0.0..=1_000_000.0).contains(&value) {
                return Err("Invalid connection statistics".into());
            }
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
}
