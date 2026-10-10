//! Bounded compressed-video queue; decoder resources belong to one worker.
use bytes::Bytes;
use serde::Serialize;
use std::collections::VecDeque;

// Allow bounded headroom while Media Foundation initializes its first device.
pub const MAX_FRAMES: usize = 32;
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Default, Serialize)]
pub struct Snapshot {
    pub enabled: bool,
    pub decoder_initialized: bool,
    pub low_latency_request_accepted: bool,
    pub mmcss_registered: bool,
    pub decoder: Option<&'static str>,
    pub renderer: Option<&'static str>,
    pub adapter: Option<String>,
    pub gpu_surface_output: bool,
    pub synthetic_pixel_variation_verified: bool,
    pub hardware_decode: Option<bool>,
    /// Observed on the latest decoded output, not a capability/configuration flag.
    pub d3d11_decoder_surface: Option<bool>,
    pub frames_queued: u64,
    pub frames_dropped: u64,
    pub frames_submitted: u64,
    pub frames_decoded: u64,
    pub frames_presented: u64,
    pub input_views_created: u64,
    pub output_views_created: u64,
    pub auto_processing_disabled: bool,
    pub presentations_busy: u64,
    pub presentations_not_visible: u64,
    pub present_max_us: u64,
    pub decode_feed_max_us: u64,
    pub queue_peak_depth: usize,
    pub queue_depth: usize,
    pub queue_bytes: usize,
    pub waiting_for_idr: bool,
    pub color_matrix: Option<u32>,
    pub nominal_range: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub failure_stage: Option<&'static str>,
    pub failure_hresult: Option<String>,
    pub worker_finished: bool,
    pub resources_released: bool,
}

impl Snapshot {
    pub fn hardware_decode_status(&self) -> &'static str {
        match (self.hardware_decode, self.d3d11_decoder_surface) {
            (Some(true), _) => "Yes",
            (Some(false), _) => "No",
            (None, Some(true)) => "Likely (D3D11 decoder surface)",
            _ => "Not reported",
        }
    }
}

pub struct Frame {
    pub bytes: Bytes,
    pub idr: bool,
    pub timestamp_100ns: i64,
}

pub struct Queue {
    pub report: Snapshot,
    pub stopped: bool,
    frames: VecDeque<Frame>,
}

impl Default for Queue {
    fn default() -> Self {
        Self {
            report: Snapshot {
                waiting_for_idr: true,
                ..Default::default()
            },
            stopped: false,
            frames: VecDeque::new(),
        }
    }
}

impl Queue {
    pub fn submit(
        &mut self,
        bytes: &[u8],
        idr: bool,
        parameters: bool,
        timestamp_100ns: i64,
    ) -> bool {
        if self.stopped
            || self.report.failure_stage.is_some()
            || (self.report.waiting_for_idr && !idr && !parameters)
        {
            self.report.frames_dropped = self.report.frames_dropped.saturating_add(1);
            return false;
        }
        if bytes.len() > crate::attempt::MAX_CHANNEL_MESSAGE
            || self.frames.len() >= MAX_FRAMES
            || self.report.queue_bytes.saturating_add(bytes.len()) > MAX_BYTES
        {
            // Dropping an interdependent H.264 frame invalidates following delta
            // pictures. Stop this video attempt rather than silently corrupting it.
            self.fail("video-input-queue-full", None);
            self.report.frames_dropped = self.report.frames_dropped.saturating_add(1);
            return false;
        }
        if idr {
            self.report.waiting_for_idr = false;
        }
        self.report.frames_queued = self.report.frames_queued.saturating_add(1);
        self.report.queue_bytes += bytes.len();
        self.frames.push_back(Frame {
            bytes: Bytes::copy_from_slice(bytes),
            idr,
            timestamp_100ns,
        });
        self.report.queue_depth = self.frames.len();
        self.report.queue_peak_depth = self.report.queue_peak_depth.max(self.frames.len());
        true
    }

    pub fn pop(&mut self) -> Option<Frame> {
        let frame = self.frames.pop_front()?;
        self.report.queue_bytes -= frame.bytes.len();
        self.report.queue_depth = self.frames.len();
        Some(frame)
    }

    pub fn fail(&mut self, stage: &'static str, hresult: Option<String>) {
        if self.report.failure_stage.is_none() {
            self.report.failure_stage = Some(stage);
            self.report.failure_hresult = hresult;
        }
        self.clear();
    }

    pub fn clear(&mut self) {
        self.report.frames_dropped = self
            .report
            .frames_dropped
            .saturating_add(self.frames.len() as u64);
        self.frames.clear();
        self.report.queue_depth = 0;
        self.report.queue_bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gpu_output_alone_does_not_establish_hardware_decoding() {
        let mut snapshot = Snapshot {
            decoder_initialized: true,
            gpu_surface_output: true,
            ..Default::default()
        };
        assert_eq!(snapshot.hardware_decode_status(), "Not reported");
        snapshot.d3d11_decoder_surface = Some(false);
        assert_eq!(snapshot.hardware_decode_status(), "Not reported");
        snapshot.d3d11_decoder_surface = Some(true);
        assert_eq!(
            snapshot.hardware_decode_status(),
            "Likely (D3D11 decoder surface)"
        );
        assert_eq!(snapshot.hardware_decode, None);
        snapshot.hardware_decode = Some(false);
        assert_eq!(snapshot.hardware_decode_status(), "No");
    }
    #[test]
    fn queue_is_bounded_and_overflow_only_disables_video() {
        let mut q = Queue::default();
        for _ in 0..MAX_FRAMES {
            assert!(q.submit(&[1, 2, 3], true, false, 0));
        }
        assert!(!q.submit(&[4], false, false, 0));
        assert_eq!(q.report.failure_stage, Some("video-input-queue-full"));
        assert_eq!(q.report.queue_depth, 0);
        assert_eq!(q.report.queue_bytes, 0);
        assert_eq!(q.report.frames_dropped, MAX_FRAMES as u64 + 1);
        q.fail("later", Some("0x80000000".into()));
        assert_eq!(q.report.failure_stage, Some("video-input-queue-full"));
    }
    #[test]
    fn parameter_sets_can_precede_idr_but_delta_cannot_start_decoder() {
        let mut q = Queue::default();
        assert!(!q.submit(&[1], false, false, 0));
        assert!(q.submit(&[2], false, true, 1));
        assert!(q.report.waiting_for_idr);
        assert!(q.submit(&[3], true, false, 2));
        assert!(!q.report.waiting_for_idr);
        assert!(q.submit(&[4], false, false, 3));
        assert_eq!(q.pop().unwrap().bytes.as_ref(), &[2]);
        let frame = q.pop().unwrap();
        assert!(frame.idr);
        assert_eq!(frame.timestamp_100ns, 2);
        q.clear();
        assert_eq!(q.report.queue_bytes, 0);
    }
    #[test]
    fn stopped_and_oversized_input_are_never_retained() {
        let mut q = Queue {
            stopped: true,
            ..Default::default()
        };
        assert!(!q.submit(b"private-video", true, false, 0));
        assert_eq!(q.report.frames_queued, 0);
        let mut q = Queue::default();
        assert!(!q.submit(
            &vec![0; crate::attempt::MAX_CHANNEL_MESSAGE + 1],
            true,
            false,
            0
        ));
        assert_eq!(q.report.queue_bytes, 0);
        assert!(!serde_json::to_string(&q.report)
            .unwrap()
            .contains("private-video"));
    }
}
