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
