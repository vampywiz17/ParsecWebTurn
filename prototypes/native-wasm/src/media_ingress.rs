//! Pinned Parsec channels: 1 video, 2 audio. No decoder or buffered sink.
use serde::Serialize;
#[derive(Clone, Default, Serialize)]
pub struct Ingress {
    pub video_stream: crate::video_stream::Snapshot,
    pub audio_output: Option<crate::audio_stream::Snapshot>,
    pub video_output: Option<crate::video_output::Snapshot>,
    pub video_packets_received: u64,
    pub video_bytes_received: u64,
    pub audio_packets_received: u64,
    pub audio_bytes_received: u64,
    pub packets_discarded_decoder_unavailable: u64,
    pub video_decoder_available: bool,
    pub audio_decoder_available: bool,
}
impl Ingress {
    pub fn unavailable(&mut self, channel: u16, size: usize) {
        self.record(channel, size, true);
    }
    pub fn record(&mut self, channel: u16, size: usize, discarded: bool) {
        let (packets, bytes) = match channel {
            1 => (
                &mut self.video_packets_received,
                &mut self.video_bytes_received,
            ),
            2 => (
                &mut self.audio_packets_received,
                &mut self.audio_bytes_received,
            ),
            _ => return,
        };
        *packets = packets.saturating_add(1);
        *bytes = bytes.saturating_add(size as u64);
        if discarded {
            self.packets_discarded_decoder_unavailable =
                self.packets_discarded_decoder_unavailable.saturating_add(1);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_media_is_counted_without_retaining_payload_or_claiming_decoding() {
        let mut backend = crate::backend::Backend::default();
        backend.init();
        backend.status = Some(0);
        backend
            .handle_message(
                1,
                false,
                bytes::Bytes::from_static(b"private-video-payload"),
            )
            .unwrap();
        backend
            .handle_message(
                2,
                false,
                bytes::Bytes::from_static(b"private-audio-payload"),
            )
            .unwrap();
        backend
            .handle_message(0, false, crate::control::header(28, 3, 0, 0))
            .unwrap();
        assert_eq!(backend.status, Some(0));
        assert_eq!(backend.host_mode, 3);
        assert_eq!(backend.control_frames_received, 1);
        assert_eq!(
            backend.media_ingress.packets_discarded_decoder_unavailable,
            2
        );
        assert!(
            !backend.media_ingress.video_decoder_available
                && !backend.media_ingress.audio_decoder_available
        );
        assert!(!backend
            .diagnostic()
            .unwrap()
            .to_string()
            .contains("private-"));
        backend.prepare_attempt();
        assert_eq!(backend.media_ingress.video_packets_received, 0);
    }
    #[test]
    fn invalid_channel_and_control_are_distinguished_and_remain_connection_errors() {
        let mut backend = crate::backend::Backend::default();
        backend.init();
        for (channel, text, stage) in [
            (3, false, "inbound-channel"),
            (0, true, "inbound-channel"),
            (0, false, "inbound-control"),
        ] {
            let error = backend
                .handle_message(channel, text, bytes::Bytes::new())
                .unwrap_err();
            let failure = *error
                .downcast_ref::<crate::attempt::FailureStage>()
                .unwrap();
            assert_eq!(serde_json::to_value(failure).unwrap(), stage);
            backend.fail_native_attempt(failure).unwrap();
            assert_eq!(backend.status, Some(-6200));
            assert!(backend.poll_event().is_some());
        }
    }
    #[test]
    fn accounting_saturates_and_never_allocates_a_media_queue() {
        let mut ingress = Ingress {
            video_packets_received: u64::MAX,
            video_bytes_received: u64::MAX,
            ..Default::default()
        };
        ingress.unavailable(1, 100);
        ingress.unavailable(99, 100);
        assert_eq!(ingress.video_packets_received, u64::MAX);
        assert_eq!(ingress.video_bytes_received, u64::MAX);
        assert_eq!(ingress.packets_discarded_decoder_unavailable, 1);
    }
}
