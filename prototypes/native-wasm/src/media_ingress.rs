//! Pinned Parsec channels: 1 H.264 video, 2 Opus audio. Live media uses an
//! independent bounded queue so a burst can never starve control channel 0.
use bytes::Bytes;
use serde::Serialize;
use std::collections::VecDeque;

const VIDEO_FRAMES: usize = 2;
const AUDIO_PACKETS: usize = 8;

#[derive(Clone, Default, Serialize)]
pub struct Ingress {
    pub video_packets_received: u64,
    pub video_bytes_received: u64,
    pub audio_packets_received: u64,
    pub audio_bytes_received: u64,
    pub packets_discarded_decoder_unavailable: u64,
    pub video_protocol_messages: u64,
    pub video_frames_queued: u64,
    pub audio_packets_queued: u64,
    pub video_frames_dequeued: u64,
    pub video_keyframes_dequeued: u64,
    pub audio_packets_dequeued: u64,
    pub media_bytes_dequeued: u64,
    pub video_frames_dropped: u64,
    pub audio_packets_dropped: u64,
    pub video_queue_depth: usize,
    pub audio_queue_depth: usize,
    pub awaiting_video_keyframe: bool,
    pub video_decoder_available: bool,
    pub audio_decoder_available: bool,
}

impl Ingress {
    fn received(&mut self, channel: u16, size: usize) {
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
    }

    pub fn unavailable(&mut self, channel: u16, size: usize) {
        self.received(channel, size);
        self.packets_discarded_decoder_unavailable =
            self.packets_discarded_decoder_unavailable.saturating_add(1);
    }
}

#[derive(Clone, Copy)]
pub struct VideoLayout {
    pub version: u32,
    pub message_size: u32,
    pub version_offset: u32,
    pub flag_offset: u32,
}

struct VideoClassifier {
    layout: VideoLayout,
    metadata_seen: bool,
    next_keyframe: bool,
}

impl VideoClassifier {
    fn classify(&mut self, bytes: &[u8]) -> Option<bool> {
        let layout = self.layout;
        if bytes.len() == layout.message_size as usize {
            let version = usize::try_from(layout.version_offset)
                .ok()
                .and_then(|offset| bytes.get(offset..offset + 4))
                .and_then(|value| value.try_into().ok())
                .map(u32::from_le_bytes);
            if version == Some(layout.version) {
                let flags = usize::try_from(layout.flag_offset)
                    .ok()
                    .and_then(|offset| bytes.get(offset..offset + 4))
                    .and_then(|value| value.try_into().ok())
                    .map(u32::from_le_bytes)
                    .unwrap_or(0);
                self.metadata_seen = true;
                self.next_keyframe = flags & 2 != 0;
                return None;
            }
        }
        if !self.metadata_seen {
            Some(true)
        } else {
            Some(std::mem::take(&mut self.next_keyframe))
        }
    }
}

pub struct Packet {
    pub channel: u16,
    pub keyframe: bool,
    pub bytes: Bytes,
}

pub struct Queue {
    classifier: VideoClassifier,
    video: VecDeque<Packet>,
    audio: VecDeque<Packet>,
    await_keyframe: bool,
}

impl Queue {
    pub fn new(layout: VideoLayout) -> Self {
        Self {
            classifier: VideoClassifier {
                layout,
                metadata_seen: false,
                next_keyframe: false,
            },
            video: VecDeque::new(),
            audio: VecDeque::new(),
            await_keyframe: false,
        }
    }

    pub fn push(&mut self, channel: u16, bytes: &[u8], ingress: &mut Ingress) {
        ingress.received(channel, bytes.len());
        match channel {
            1 => {
                let Some(keyframe) = self.classifier.classify(bytes) else {
                    ingress.video_protocol_messages =
                        ingress.video_protocol_messages.saturating_add(1);
                    return self.depths(ingress);
                };
                if self.await_keyframe && !keyframe {
                    ingress.video_frames_dropped = ingress.video_frames_dropped.saturating_add(1);
                    return self.depths(ingress);
                }
                if keyframe {
                    ingress.video_frames_dropped = ingress
                        .video_frames_dropped
                        .saturating_add(self.video.len() as u64);
                    self.video.clear();
                    self.await_keyframe = false;
                } else if self.video.len() == VIDEO_FRAMES {
                    ingress.video_frames_dropped = ingress
                        .video_frames_dropped
                        .saturating_add(self.video.len() as u64 + 1);
                    self.video.clear();
                    self.await_keyframe = true;
                    return self.depths(ingress);
                }
                self.video.push_back(Packet {
                    channel,
                    keyframe,
                    bytes: Bytes::copy_from_slice(bytes),
                });
                ingress.video_frames_queued = ingress.video_frames_queued.saturating_add(1);
            }
            2 => {
                if self.audio.len() == AUDIO_PACKETS {
                    self.audio.pop_front();
                    ingress.audio_packets_dropped = ingress.audio_packets_dropped.saturating_add(1);
                }
                self.audio.push_back(Packet {
                    channel,
                    keyframe: true,
                    bytes: Bytes::copy_from_slice(bytes),
                });
                ingress.audio_packets_queued = ingress.audio_packets_queued.saturating_add(1);
            }
            _ => {}
        }
        self.depths(ingress);
    }

    pub fn pop(&mut self, ingress: &mut Ingress) -> Option<Packet> {
        let packet = self.video.pop_front().or_else(|| self.audio.pop_front());
        if let Some(packet) = &packet {
            ingress.media_bytes_dequeued = ingress
                .media_bytes_dequeued
                .saturating_add(packet.bytes.len() as u64);
            match packet.channel {
                1 => {
                    ingress.video_frames_dequeued = ingress.video_frames_dequeued.saturating_add(1);
                    if packet.keyframe {
                        ingress.video_keyframes_dequeued =
                            ingress.video_keyframes_dequeued.saturating_add(1);
                    }
                }
                2 => {
                    ingress.audio_packets_dequeued =
                        ingress.audio_packets_dequeued.saturating_add(1);
                }
                _ => {}
            }
        }
        self.depths(ingress);
        packet
    }

    pub fn clear(&mut self, ingress: &mut Ingress) {
        self.video.clear();
        self.audio.clear();
        self.await_keyframe = false;
        self.depths(ingress);
    }

    fn depths(&self, ingress: &mut Ingress) {
        ingress.video_queue_depth = self.video.len();
        ingress.audio_queue_depth = self.audio.len();
        ingress.awaiting_video_keyframe = self.await_keyframe;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue() -> Queue {
        Queue::new(VideoLayout {
            version: 0,
            message_size: 11,
            version_offset: 0,
            flag_offset: 4,
        })
    }

    fn metadata(flags: u32) -> [u8; 11] {
        let mut value = [0; 11];
        value[4..8].copy_from_slice(&flags.to_le_bytes());
        value
    }

    #[test]
    fn parsec_metadata_marks_the_next_annex_b_frame() {
        let mut queue = queue();
        let mut ingress = Ingress::default();
        queue.push(1, &metadata(2), &mut ingress);
        queue.push(1, b"\0\0\0\x01\x65-key", &mut ingress);
        queue.push(1, b"\0\0\0\x01\x41-delta", &mut ingress);
        assert_eq!(ingress.video_protocol_messages, 1);
        assert!(queue.pop(&mut ingress).unwrap().keyframe);
        assert!(!queue.pop(&mut ingress).unwrap().keyframe);
    }

    #[test]
    fn overflow_drops_a_gop_and_waits_for_a_keyframe() {
        let mut queue = queue();
        let mut ingress = Ingress::default();
        queue.push(1, &metadata(2), &mut ingress);
        queue.push(1, b"key", &mut ingress);
        queue.push(1, b"delta-1", &mut ingress);
        queue.push(1, b"delta-2", &mut ingress);
        assert!(ingress.awaiting_video_keyframe);
        assert_eq!(ingress.video_queue_depth, 0);
        queue.push(1, b"ignored-delta", &mut ingress);
        queue.push(1, &metadata(2), &mut ingress);
        queue.push(1, b"new-key", &mut ingress);
        assert!(!ingress.awaiting_video_keyframe);
        assert!(queue.pop(&mut ingress).unwrap().keyframe);
    }

    #[test]
    fn audio_keeps_only_the_newest_bounded_packets() {
        let mut queue = queue();
        let mut ingress = Ingress::default();
        for value in 0..10 {
            queue.push(2, &[value], &mut ingress);
        }
        assert_eq!(ingress.audio_queue_depth, AUDIO_PACKETS);
        assert_eq!(ingress.audio_packets_dropped, 2);
        assert_eq!(queue.pop(&mut ingress).unwrap().bytes[0], 2);
    }
}
