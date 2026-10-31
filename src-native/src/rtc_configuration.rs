//! Per-platform settings access, resolved once before creating a peer.
use anyhow::Result;
use futures_util::future::BoxFuture;
use webrtc::peer_connection::configuration::RTCConfiguration;

pub trait NetworkSettings: Send + Sync {
    fn resolve(&self) -> BoxFuture<'_, Result<RTCConfiguration>>;
    fn tcp_qos(&self) -> bool;
    #[cfg(windows)]
    fn video_gpu(&self) -> Option<crate::connection_settings::GpuPreference>;
}

#[cfg(windows)]
impl NetworkSettings for crate::connection_settings::Manager {
    fn resolve(&self) -> BoxFuture<'_, Result<RTCConfiguration>> {
        Box::pin(self.resolve_configuration())
    }
    fn tcp_qos(&self) -> bool {
        self.view().0.experimental_tcp_qos
    }
    fn video_gpu(&self) -> Option<crate::connection_settings::GpuPreference> {
        self.view().0.video_gpu
    }
}

/// RTT only from the selected pair after a response, never from stale candidates.
pub fn selected_rtt(report: &webrtc::stats::StatsReport, local: &str, remote: &str) -> Option<f64> {
    let id = format!("{local}-{remote}");
    report.reports.values().find_map(|s| match s {
        webrtc::stats::StatsReportType::CandidatePair(p)
            if p.id == id
                && p.responses_received > 0
                && p.current_round_trip_time.is_finite()
                && p.current_round_trip_time >= 0. =>
        {
            Some(p.current_round_trip_time * 1000.)
        }
        _ => None,
    })
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

#[cfg(test)]
mod route_tests {
    use super::route;
    #[test]
    fn selected_pair_requires_positive_route_evidence() {
        assert_eq!(route("relay", "prflx"), "Relay — TURN in use");
        assert_eq!(route("host", "relay"), "Relay — TURN in use");
        assert_eq!(route("srflx", "host"), "Direct — no TURN");
        assert_eq!(route("host", "prflx"), "Unverified");
        assert_eq!(route("", ""), "Unverified");
    }
}
