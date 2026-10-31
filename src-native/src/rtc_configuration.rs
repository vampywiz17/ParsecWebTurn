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
