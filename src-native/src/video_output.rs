//! Bounded compressed-video queue; decoder resources belong to one worker.
// Parts of this shared model serve only the Windows decoder and controls.
#![cfg_attr(not(windows), allow(dead_code))]
use serde::Serialize;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

// Allow bounded headroom while Media Foundation initializes its first device.
pub const MAX_FRAMES: usize = 32;
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// Describe original YUV surfaces without inferring compositor/scanout behavior.
pub fn original_surface_status(ten_bit: bool) -> &'static str {
    if ten_bit {
        "Original P010 surfaces; no app conversion pass. Scanout and final output depth not verified."
    } else {
        "Original NV12 surfaces; no app conversion pass. Scanout not verified."
    }
}

/// Preserve optional effect/device state only for known non-driver resets.
/// A genuine decoder error must retain the original full recreation path.
pub fn can_preserve_effect(reason: Option<&str>) -> bool {
    matches!(
        reason,
        None | Some("optional_video_setup_backlog" | "compressed_queue_full")
    )
}

/// Caller-side deadline, not cancellation of a native driver operation.
#[derive(Clone, Copy)]
pub enum NativeOperation {
    Setup,
    Feed,
    Present,
    Release,
}
impl NativeOperation {
    fn timeout(self) -> Duration {
        Duration::from_secs(if matches!(self, Self::Release) { 2 } else { 10 })
    }
    fn failure(self) -> &'static str {
        match self {
            Self::Setup => "video-native-setup-timeout-restart-required",
            Self::Feed => "video-native-feed-timeout-restart-required",
            Self::Present => "video-native-present-timeout-restart-required",
            Self::Release => "video-native-release-timeout-restart-required",
        }
    }
}

/// CPU wall-clock durations only. No inference about GPU completion/scanout.
#[derive(Clone, Default, Serialize)]
pub struct Timing {
    pub count: u64,
    pub latest_ms: f64,
    pub average_ms: f64,
    pub peak_ms: f64,
    pub total_ns: u128,
}
impl Timing {
    pub fn record(&mut self, elapsed: Duration) {
        let ms = elapsed.as_secs_f64() * 1000.;
        self.count = self.count.saturating_add(1);
        self.latest_ms = ms;
        self.average_ms += (ms - self.average_ms) / self.count as f64;
        self.peak_ms = self.peak_ms.max(ms);
        self.total_ns = self.total_ns.saturating_add(elapsed.as_nanos());
    }
}

#[derive(Clone, Default, Serialize)]
pub struct LocalTiming {
    pub decoder_setup: Timing,
    pub compressed_wait: Timing,
    pub decoder_service: Timing,
    pub decoder_residence: Timing,
    pub surface_dispatch: Timing,
    pub presentation_prepare: Timing,
    pub present_call: Timing,
    /// Interval between accepted Present calls; not GPU/scanout timing.
    pub present_interval: Timing,
    /// Time spent servicing decoded output inside the decoder feed callback.
    pub output_service_ns: u128,
}

impl LocalTiming {
    /// Stable presentation order, shared by sampled diagnostics and its view.
    /// Constructed only by telemetry consumers, never by the decoder callback.
    pub fn stages(&self) -> [(&'static str, &Timing); 8] {
        [
            ("Decoder/device setup", &self.decoder_setup),
            ("Compressed queue wait", &self.compressed_wait),
            ("Decoder API service", &self.decoder_service),
            ("Decoder input to output", &self.decoder_residence),
            ("Decoded surface handoff", &self.surface_dispatch),
            ("Presentation preparation", &self.presentation_prepare),
            ("Present API call", &self.present_call),
            ("Present submission interval", &self.present_interval),
        ]
    }
}

#[derive(Clone, Copy)]
struct PendingInput {
    sequence: usize,
    timestamp: i64,
    at: Instant,
}
/// Correlate native decoder outputs without retaining compressed bytes/surfaces.
/// Missing/evicted/ambiguous timestamps remain unknown, never FIFO guesses.
pub struct InFlight {
    slots: [Option<PendingInput>; 64],
    next: usize,
}
impl Default for InFlight {
    fn default() -> Self {
        Self {
            slots: [None; 64],
            next: 0,
        }
    }
}
impl InFlight {
    pub fn begin(&mut self, timestamp: i64) {
        self.slots[self.next % 64] = Some(PendingInput {
            sequence: self.next,
            timestamp,
            at: Instant::now(),
        });
        self.next = self.next.wrapping_add(1);
    }
    pub fn ready(
        &mut self,
        sequence: Option<usize>,
        timestamp: Option<i64>,
        now: Instant,
    ) -> Option<Duration> {
        if let Some(sequence) = sequence {
            let slot = &mut self.slots[sequence % 64];
            if slot.is_some_and(|p| p.sequence == sequence) {
                return now.checked_duration_since(slot.take()?.at);
            }
            return None;
        }
        let mut found = None;
        for (index, value) in self.slots.iter().enumerate() {
            if value.is_some_and(|p| timestamp.is_some_and(|t| p.timestamp == t)) {
                if found.is_some() {
                    return None;
                }
                found = Some(index);
            }
        }
        let input = self.slots[found?].take()?;
        now.checked_duration_since(input.at)
    }
}

/// Baseline fixed policy: only omit obsolete display updates when at least
/// two later compressed messages already await decoding. Never wait for input
/// or discard references. The adaptive policy can tighten this after a late
/// backlog episode; both policies use the existing bounded queue and worker.
pub fn skip_display_for_backlog(queued: usize) -> bool {
    queued >= 2
}

/// Shared display policy; selecting it never changes transport or decoder input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisplayCatchUpPolicy {
    #[default]
    Fixed,
    Adaptive,
}

// A bounded history of one continuous late-backlog episode, not a frame queue.
// A short burst uses the existing two-update tail. Only persistent, already-late
// input may reduce that tail to one; nothing is held waiting for this interval.
const CATCH_UP_HISTORY: Duration = Duration::from_millis(100);
const CATCH_UP_LATE_WAIT: Duration = Duration::from_millis(8);
#[derive(Default)]
struct DisplayCatchUp {
    policy: DisplayCatchUpPolicy,
    late_since: Option<Instant>,
    last_late: Option<Instant>,
    observations: u8,
    aggressive: bool,
}
impl DisplayCatchUp {
    fn reset(&mut self) {
        self.late_since = None;
        self.last_late = None;
        self.observations = 0;
        self.aggressive = false;
    }
    /// One observation per dequeued input, sharing its existing timing sample.
    /// Return true only when a late episode first enters stricter catch-up.
    fn observe(&mut self, queued: usize, waited: Duration, now: Instant) -> bool {
        if self.policy == DisplayCatchUpPolicy::Fixed {
            return false;
        }
        if queued == 0 || waited < CATCH_UP_LATE_WAIT {
            self.reset();
            return false;
        }
        if self
            .last_late
            .is_some_and(|last| now.saturating_duration_since(last) > CATCH_UP_HISTORY)
        {
            self.reset();
        }
        self.last_late = Some(now);
        let since = *self.late_since.get_or_insert(now);
        self.observations = self.observations.saturating_add(1);
        let previous = self.aggressive;
        self.aggressive =
            self.observations >= 3 && now.saturating_duration_since(since) >= CATCH_UP_HISTORY;
        self.aggressive && !previous
    }
}

#[derive(Clone, Default, Serialize)]
pub struct Snapshot {
    pub local_timing: LocalTiming,
    pub presentation_mode: Option<&'static str>,
    pub display_updates_skipped: u64,
    pub adaptive_display_catch_up: bool,
    pub adaptive_display_catch_up_activations: u64,
    pub decoded_bit_depth: Option<u8>,
    pub decoded_chroma: Option<&'static str>,
    /// RGB swap-chain precision, not the monitor/link's physical bit depth.
    pub display_rgb_bit_depth: Option<u8>,
    pub display_output_fallback: Option<String>,
    pub vsr_status: Option<&'static str>,
    pub recovery_reason: Option<&'static str>,
    pub codec: Option<crate::video_stream::Codec>,
    pub enabled: bool,
    pub decoder_initialized: bool,
    pub low_latency_request_accepted: bool,
    pub mmcss_registered: bool,
    pub decoder: Option<&'static str>,
    pub decoder_fallback: Option<String>,
    pub zero_copy_status: Option<String>,
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
    pub recovering: bool,
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
    pub picture: bool,
    /// Clockwise display quarter-turns, carried with compressed-frame identity.
    pub rotation: u8,
    pub queued_at: Instant,
    pub codec: crate::video_stream::Codec,
    pub bytes: Vec<u8>,
    pub idr: bool,
    pub parameters: bool,
    pub timestamp_100ns: i64,
}

pub struct Queue {
    pub mf_420_fallback: bool,
    native_call: Option<(NativeOperation, Instant)>,
    pub native_stalled: bool,
    display_catch_up: DisplayCatchUp,
    pub report: Snapshot,
    pub stopped: bool,
    frames: VecDeque<Frame>,
    free_buffers: Vec<Vec<u8>>,
    free_bytes: usize,
    pub reset_generation: u64,
    pub stream_generation: u64,
    recovery_started: Option<Instant>,
    recovery_attempts: u8,
    optional_setup: bool,
    request_pending: bool,
    last_request: Option<Instant>,
}

impl Default for Queue {
    fn default() -> Self {
        Self {
            mf_420_fallback: false,
            native_call: None,
            native_stalled: false,
            display_catch_up: Default::default(),
            report: Snapshot {
                waiting_for_idr: true,
                ..Default::default()
            },
            stopped: false,
            frames: VecDeque::new(),
            free_buffers: Vec::new(),
            free_bytes: 0,
            reset_generation: 0,
            stream_generation: 0,
            recovery_started: None,
            recovery_attempts: 0,
            optional_setup: false,
            request_pending: false,
            last_request: None,
        }
    }
}

impl Queue {
    pub fn begin_native_call(&mut self, operation: NativeOperation, now: Instant) {
        self.check_native_call(now);
        if !self.native_stalled {
            self.native_call = Some((operation, now));
        }
    }

    pub fn finish_native_call(&mut self, now: Instant) {
        self.check_native_call(now);
        self.native_call = None;
    }

    pub fn check_native_call(&mut self, now: Instant) {
        if !self.native_stalled {
            if let Some((operation, began)) = self.native_call {
                if now.saturating_duration_since(began) >= operation.timeout() {
                    self.native_stalled = true;
                    self.stopped = true;
                    self.fail(operation.failure(), None);
                }
            }
        }
    }

    pub fn with_display_policy(policy: DisplayCatchUpPolicy) -> Self {
        let mut queue = Self::default();
        queue.display_catch_up.policy = policy;
        queue.report.adaptive_display_catch_up = policy == DisplayCatchUpPolicy::Adaptive;
        queue
    }

    /// Read-only at every output/retry point. History advances only in pop().
    pub fn should_skip_display(&self) -> bool {
        skip_display_for_backlog(self.frames.len())
            || (self.display_catch_up.aggressive
                && self.frames.front().is_some_and(|next| next.picture))
    }

    #[cfg(test)]
    pub fn submit(
        &mut self,
        bytes: &[u8],
        idr: bool,
        parameters: bool,
        timestamp_100ns: i64,
    ) -> bool {
        let accepted = self.submit_codec(
            bytes.to_vec(),
            idr,
            parameters,
            timestamp_100ns,
            crate::video_stream::Codec::H264,
        );
        if accepted {
            self.set_last_packet(0, idr || !parameters);
        }
        accepted
    }

    pub fn submit_codec(
        &mut self,
        bytes: Vec<u8>,
        idr: bool,
        parameters: bool,
        timestamp_100ns: i64,
        codec: crate::video_stream::Codec,
    ) -> bool {
        if self.report.codec.is_some_and(|previous| previous != codec) {
            self.fail("video-unexpected-codec-change", None);
            return false;
        }
        if self.stopped
            || self.report.failure_stage.is_some()
            || (self.report.waiting_for_idr && !idr && !parameters)
        {
            self.report.frames_dropped = self.report.frames_dropped.saturating_add(1);
            self.recycle(bytes);
            return false;
        }
        if bytes.len() > crate::control::MAX_CHANNEL_MESSAGE {
            self.fail("video-input-too-large", None);
            self.report.frames_dropped += 1;
            return false;
        }
        if self.frames.len() >= MAX_FRAMES
            || self.report.queue_bytes.saturating_add(bytes.len()) > MAX_BYTES
        {
            // Following deltas are unusable after a missing reference picture.
            self.recover_for(Instant::now(), "compressed_queue_full");
            self.report.frames_dropped = self.report.frames_dropped.saturating_add(1);
            return false;
        }
        if idr {
            // A BUD rebuild already supplied the requested key. Keep the
            // existing WebRTC recovery/request policy (stream_generation=0).
            if self.stream_generation != 0 {
                self.request_pending = false;
            }
            self.report.waiting_for_idr = false;
        }
        self.report.frames_queued = self.report.frames_queued.saturating_add(1);
        self.report.codec = Some(codec);
        self.report.queue_bytes += bytes.len();
        self.frames.push_back(Frame {
            picture: false,
            rotation: 0,
            queued_at: Instant::now(),
            codec,
            bytes,
            idr,
            parameters,
            timestamp_100ns,
        });
        self.report.queue_depth = self.frames.len();
        self.report.queue_peak_depth = self.report.queue_peak_depth.max(self.frames.len());
        true
    }

    pub fn set_last_packet(&mut self, rotation: u8, picture: bool) {
        if let Some(frame) = self.frames.back_mut() {
            frame.rotation = rotation;
            frame.picture = picture;
        }
    }

    pub fn pop(&mut self) -> Option<Frame> {
        if self.frames.is_empty() {
            return None;
        }
        self.pop_at(Instant::now())
    }

    fn pop_at(&mut self, now: Instant) -> Option<Frame> {
        let frame = self.frames.pop_front()?;
        self.report.queue_bytes -= frame.bytes.len();
        self.report.queue_depth = self.frames.len();
        let waited = now.saturating_duration_since(frame.queued_at);
        self.report.local_timing.compressed_wait.record(waited);
        if self
            .display_catch_up
            .observe(self.frames.len(), waited, now)
        {
            self.report.adaptive_display_catch_up_activations = self
                .report
                .adaptive_display_catch_up_activations
                .saturating_add(1);
        }
        Some(frame)
    }

    pub fn take_buffer(&mut self) -> Vec<u8> {
        let buffer = self.free_buffers.pop().unwrap_or_default();
        self.free_bytes -= buffer.capacity();
        buffer
    }

    pub fn recycle(&mut self, mut buffer: Vec<u8>) {
        if !self.stopped
            && self.report.failure_stage.is_none()
            && self.free_buffers.len() < MAX_FRAMES
            && self.free_bytes.saturating_add(buffer.capacity()) <= MAX_BYTES
        {
            buffer.clear();
            self.free_bytes += buffer.capacity();
            self.free_buffers.push(buffer);
        }
    }

    /// An explicit encoder rebuild starts a new reference chain and may use a
    /// different codec. Retire old compressed work before any new payload is
    /// queued; this is not a decoder failure and consumes no recovery attempt.
    pub fn encoder_rebuilt(&mut self, now: Instant) -> bool {
        if self.stopped || self.report.failure_stage.is_some() {
            return false;
        }
        self.clear();
        self.stream_generation = self.stream_generation.wrapping_add(1);
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.report.codec = None;
        self.report.waiting_for_idr = true;
        self.begin_format_change(now, true);
        true
    }

    pub fn optional_setup(&mut self, active: bool) {
        self.optional_setup = active;
        if !active && self.report.recovering {
            self.recovery_started = Some(Instant::now());
        }
    }
    pub fn recover(&mut self, now: Instant) -> bool {
        self.recover_for(now, "decoder_recovery")
    }
    pub fn fallback_to_mf_420(&mut self, now: Instant, detail: &str) -> bool {
        if self.mf_420_fallback {
            return true;
        }
        self.mf_420_fallback = true;
        self.report.decoder_fallback = Some(format!(
            "Codec compatibility: requesting 4:2:0 and Media Foundation: {detail}"
        ));
        crate::event_log::record(
            "WARN",
            "video_chroma_fallback_requested",
            serde_json::json!({"reason":"decoder_compatibility","requested_chroma":"4:4:4",
                "fallback_chroma":"4:2:0","fallback_decoder":"Media Foundation","detail":detail}),
        );
        self.recover_for(now, "codec_compatibility_420_mf")
    }
    fn recover_for(&mut self, now: Instant, reason: &'static str) -> bool {
        if self.stopped || self.report.failure_stage.is_some() {
            return false;
        }
        let warmup = self.optional_setup && reason == "compressed_queue_full";
        if !warmup && self.recovery_attempts >= 3 {
            self.fail("video-recovery-exhausted", None);
            return false;
        }
        if !warmup {
            self.recovery_attempts += 1;
        }
        self.report.recovery_reason = Some(if warmup {
            "optional_video_setup_backlog"
        } else {
            reason
        });
        crate::event_log::record(
            "WARN",
            "video_recovery_requested",
            serde_json::json!({
                "reason":self.report.recovery_reason,"queue_depth":self.report.queue_depth,
                "queue_bytes":self.report.queue_bytes,"attempt":self.recovery_attempts,
            }),
        );
        self.reset_generation += 1;
        self.clear();
        self.report.waiting_for_idr = true;
        self.report.decoder_initialized = false;
        self.report.recovering = true;
        self.recovery_started = Some(now);
        self.request_pending = true;
        true
    }

    /// A valid stream format change is not a decoder failure. Preserve queued
    /// compressed frames, especially an already received random-access picture,
    /// and keep the existing keyframe deadline/rate limit if one is needed.
    pub fn begin_format_change(&mut self, now: Instant, needs_keyframe: bool) {
        self.report.decoder_initialized = false;
        if needs_keyframe {
            self.report.waiting_for_idr = true;
            self.report.recovering = true;
            self.recovery_started = Some(now);
            self.request_pending = true;
        }
    }

    pub fn check_recovery(&mut self, now: Instant) {
        if !self.optional_setup
            && self
                .recovery_started
                .is_some_and(|start| now.duration_since(start) >= Duration::from_secs(5))
        {
            self.fail("video-recovery-timeout", None);
        }
    }

    pub fn take_keyframe_request(&mut self, now: Instant) -> Option<crate::video_stream::Codec> {
        self.check_recovery(now);
        if self.stopped
            || self.report.failure_stage.is_some()
            || !self.request_pending
            || self
                .last_request
                .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1))
        {
            return None;
        }
        let codec = self.report.codec?;
        self.request_pending = false;
        self.last_request = Some(now);
        Some(codec)
    }

    pub fn recovered(&mut self) {
        self.report.recovery_reason = None;
        self.report.waiting_for_idr = false;
        self.recovery_started = None;
        self.request_pending = false;
        self.report.recovering = false;
    }

    pub fn request_failed(&mut self, generation: u64) {
        if !self.stopped && self.report.recovering && self.reset_generation == generation {
            self.fail("video-recovery-request-failed", None);
        }
    }

    pub fn fail(&mut self, stage: &'static str, hresult: Option<String>) {
        if self.report.failure_stage.is_none() {
            crate::event_log::record(
                "ERROR",
                "video_stopped",
                serde_json::json!({"stage":stage,"hresult":hresult}),
            );
            self.report.failure_stage = Some(stage);
            self.report.failure_hresult = hresult;
        }
        self.clear();
        self.request_pending = false;
        self.recovery_started = None;
        self.report.recovering = false;
    }

    pub fn clear(&mut self) {
        self.display_catch_up.reset();
        self.report.frames_dropped = self
            .report
            .frames_dropped
            .saturating_add(self.frames.len() as u64);
        while let Some(frame) = self.frames.pop_front() {
            self.recycle(frame.bytes);
        }
        if self.stopped || self.report.failure_stage.is_some() {
            self.free_buffers.clear();
            self.free_bytes = 0;
        }
        self.report.queue_depth = 0;
        self.report.queue_bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chroma_fallback_is_sticky_bounded_and_requires_fresh_random_access() {
        let now = Instant::now();
        let mut q = Queue::default();
        assert!(q.submit(&[1], true, false, 0));
        assert!(q.fallback_to_mf_420(now, "unsupported output format"));
        let generation = q.reset_generation;
        assert!(q.mf_420_fallback && q.report.waiting_for_idr);
        assert_eq!(q.report.queue_depth, 0);
        assert!(q
            .report
            .decoder_fallback
            .as_deref()
            .unwrap()
            .contains("unsupported output format"));
        assert!(q.take_keyframe_request(now).is_some());
        assert!(q.fallback_to_mf_420(now, "repeated old frame"));
        assert_eq!(q.reset_generation, generation);
        assert!(q
            .take_keyframe_request(now + Duration::from_secs(1))
            .is_none());
        q.check_recovery(now + Duration::from_secs(6));
        assert_eq!(q.report.failure_stage, Some("video-recovery-timeout"));
        assert!(!Queue::default().mf_420_fallback);
    }

    #[test]
    fn native_deadlines_cover_only_active_calls_and_never_claim_resource_release() {
        let now = Instant::now();
        for operation in [
            NativeOperation::Setup,
            NativeOperation::Feed,
            NativeOperation::Present,
            NativeOperation::Release,
        ] {
            let mut queue = Queue::default();
            queue.check_native_call(now + Duration::from_secs(3600));
            assert!(
                !queue.native_stalled,
                "an idle stream is not a stuck driver"
            );
            queue.begin_native_call(operation, now);
            queue.check_native_call(now + operation.timeout() - Duration::from_nanos(1));
            assert!(!queue.native_stalled);
            queue.check_native_call(now + operation.timeout());
            assert!(queue.native_stalled && queue.stopped);
            assert_eq!(queue.report.failure_stage, Some(operation.failure()));
            assert!(!queue.report.worker_finished && !queue.report.resources_released);
            queue.finish_native_call(now + operation.timeout() + Duration::from_secs(1));
            queue.begin_native_call(NativeOperation::Feed, now + Duration::from_secs(100));
            assert!(
                queue.native_call.is_none(),
                "late completion must not restart a retired worker"
            );
            assert_eq!(queue.report.failure_stage, Some(operation.failure()));
        }
        let mut healthy = Queue::default();
        healthy.begin_native_call(NativeOperation::Feed, now);
        healthy.finish_native_call(now + Duration::from_millis(1));
        healthy.check_native_call(now + Duration::from_secs(3600));
        assert!(!healthy.native_stalled);
    }

    #[test]
    fn stalled_native_owner_keeps_resource_until_its_call_returns() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc, Mutex,
        };
        struct Resource(Arc<AtomicBool>, std::thread::ThreadId);
        impl Drop for Resource {
            fn drop(&mut self) {
                assert_eq!(self.1, std::thread::current().id());
                self.0.store(true, Ordering::Release);
            }
        }
        let queue = Arc::new(Mutex::new(Queue::default()));
        let released = Arc::new(AtomicBool::new(false));
        let (start, ready) = mpsc::channel();
        let (resume, blocked) = mpsc::channel();
        let q = queue.clone();
        let dropped = released.clone();
        let worker = std::thread::spawn(move || {
            let resource = Resource(dropped, std::thread::current().id());
            let began = Instant::now();
            q.lock()
                .unwrap()
                .begin_native_call(NativeOperation::Feed, began);
            start.send(began).unwrap();
            blocked.recv_timeout(Duration::from_secs(2)).unwrap();
            q.lock().unwrap().finish_native_call(Instant::now());
            drop(resource);
        });
        let began = ready.recv_timeout(Duration::from_secs(2)).unwrap();
        {
            let mut q = queue.lock().unwrap();
            q.check_native_call(began + Duration::from_secs(10));
            assert!(q.native_stalled && q.stopped);
            assert!(!q.report.resources_released);
            // Queue cancellation and compressed-buffer cleanup cannot free an
            // owner-thread resource still inside a native call.
            q.clear();
            assert!(!released.load(Ordering::Acquire));
        }
        resume.send(()).unwrap();
        worker.join().unwrap();
        assert!(released.load(Ordering::Acquire));
        assert!(queue.lock().unwrap().native_stalled);
    }
    #[test]
    fn announced_encoder_rebuild_retires_old_codec_without_spending_error_budget() {
        use crate::video_stream::Codec;
        let mut q = Queue::default();
        for turn in 0..8 {
            let old = if turn % 2 == 0 {
                Codec::Hevc
            } else {
                Codec::H264
            };
            assert!(q.submit_codec(vec![1], true, true, 0, old));
            let previous = q.reset_generation;
            assert!(q.encoder_rebuilt(Instant::now()));
            assert_eq!(q.reset_generation, previous + 1);
            assert_eq!(q.stream_generation, previous + 1);
            assert_eq!(q.report.queue_depth, 0);
            assert_eq!(q.report.codec, None);
            assert_eq!(q.recovery_attempts, 0);
            let next = if old == Codec::Hevc {
                Codec::H264
            } else {
                Codec::Hevc
            };
            let bytes = vec![2];
            let pointer = bytes.as_ptr();
            assert!(q.submit_codec(bytes, true, true, 1, next));
            assert_eq!(q.take_keyframe_request(Instant::now()), None);
            let frame = q.pop().unwrap();
            assert_eq!(frame.codec, next);
            assert_eq!(frame.bytes.as_ptr(), pointer);
            q.recovered();
        }
        q.stopped = true;
        assert!(!q.encoder_rebuilt(Instant::now()));
    }
    #[test]
    fn local_timing_is_bounded_and_measures_queue_age_without_sleeping() {
        let mut q = Queue::default();
        assert!(q.submit(&[1], true, false, 0));
        q.frames.front_mut().unwrap().queued_at = Instant::now() - Duration::from_millis(20);
        q.pop().unwrap();
        assert_eq!(q.report.local_timing.compressed_wait.count, 1);
        assert!(q.report.local_timing.compressed_wait.latest_ms >= 20.);
        let mut timing = Timing::default();
        timing.record(Duration::from_millis(2));
        timing.record(Duration::from_millis(6));
        assert_eq!(
            (timing.latest_ms, timing.average_ms, timing.peak_ms),
            (6., 4., 6.)
        );
        assert_eq!(timing.total_ns, 8_000_000);
        assert!(std::mem::size_of::<LocalTiming>() <= 512);
        assert!(std::mem::size_of::<InFlight>() <= 4096);
    }

    #[test]
    fn output_timing_matches_identity_not_fifo_and_forgets_old_slots() {
        let mut pending = InFlight::default();
        pending.begin(100);
        pending.begin(200);
        let now = Instant::now();
        assert!(pending.ready(None, Some(200), now).is_some());
        assert!(pending.ready(Some(0), None, now).is_some());
        assert!(pending.ready(Some(0), None, now).is_none());
        pending.begin(300);
        pending.begin(300);
        assert!(pending.ready(None, Some(300), Instant::now()).is_none());
        for n in 0..1000 {
            pending.begin(n);
        }
        assert!(pending.ready(None, Some(1), Instant::now()).is_none());
        assert!(pending.ready(None, Some(999), Instant::now()).is_some());
    }

    #[test]
    fn burst_catch_up_decodes_every_reference_but_reduces_display_work() {
        fn model(catch_up: bool, count: usize, render_us: u64) -> (Vec<i64>, u64, usize) {
            let mut q = Queue::default();
            for index in 0..count {
                assert!(q.submit(&[index as u8], index == 0, false, index as i64));
            }
            let mut decoded = Vec::new();
            let mut elapsed = 0;
            let mut displayed = 0;
            while let Some(frame) = q.pop() {
                decoded.push(frame.timestamp_100ns);
                elapsed += 2_000; // Same modeled decode cost for every reference.
                if !catch_up || !skip_display_for_backlog(q.report.queue_depth) {
                    elapsed += render_us;
                    displayed += 1;
                }
            }
            (decoded, elapsed, displayed)
        }
        let baseline = model(false, 24, 4_000);
        let catch_up = model(true, 24, 4_000);
        assert_eq!(baseline.0, catch_up.0);
        assert_eq!((baseline.1, baseline.2), (144_000, 24));
        assert_eq!((catch_up.1, catch_up.2), (56_000, 2));
        // No claimed improvement when display work is free or no burst exists.
        assert_eq!(model(false, 24, 0).1, model(true, 24, 0).1);
        assert_eq!(model(false, 1, 4_000), model(true, 1, 4_000));
        assert!(!skip_display_for_backlog(0));
        assert!(!skip_display_for_backlog(1));
        eprintln!("synthetic burst: baseline={}us/{} display submissions; catch-up={}us/{}; all {} references decoded", baseline.1, baseline.2, catch_up.1, catch_up.2, catch_up.0.len());
    }

    #[test]
    fn display_catch_up_drains_bursts_without_losing_the_tail_or_references() {
        let mut queue = Queue::with_display_policy(DisplayCatchUpPolicy::Adaptive);
        let mut identity = 0i64;
        for burst in [1, 2, 3, 12, 1, 24, 1] {
            let first = identity;
            for offset in 0..burst {
                // Include parameter-set and random-access updates in the
                // decoded input sequence, not just ordinary predicted pictures.
                let idr = offset == 0;
                let parameters = offset == 0 || offset == 1;
                assert!(queue.submit(&[identity as u8], idr, parameters, identity));
                identity += 1;
            }
            let mut decoded = Vec::new();
            let mut displayed = Vec::new();
            // This fixture represents an instantaneous burst drain, regardless
            // of descheduling on the test runner. Persistent-time cases below
            // advance an injected monotonic clock explicitly.
            let sample = Instant::now();
            while let Some(frame) = queue.pop_at(sample) {
                decoded.push((frame.timestamp_100ns, frame.idr, frame.parameters));
                if !queue.should_skip_display() {
                    displayed.push(frame.timestamp_100ns);
                }
                queue.recycle(frame.bytes);
            }
            let expected: Vec<_> = (first..identity)
                .enumerate()
                .map(|(offset, id)| (id, offset == 0, offset <= 1))
                .collect();
            assert_eq!(decoded, expected);
            assert_eq!(displayed.len(), burst.min(2));
            assert_eq!(displayed.last(), Some(&(identity - 1)));
            assert_eq!(queue.report.queue_depth, 0);
            assert_eq!(queue.report.queue_bytes, 0);
            assert_eq!(queue.report.frames_dropped, 0);
            assert_eq!(queue.reset_generation, 0);
            assert!(!queue.report.waiting_for_idr);
        }
    }

    #[test]
    fn adaptive_catch_up_distinguishes_short_and_persistent_late_bursts() {
        fn drain(
            policy: DisplayCatchUpPolicy,
            arrivals: &[u64],
            tail: Option<&[u8]>,
        ) -> (Vec<i64>, Vec<i64>, u64) {
            let now = Instant::now();
            let mut queue = Queue::with_display_policy(policy);
            for index in 0..arrivals.len() {
                if let Some(bytes) = tail.filter(|_| index == arrivals.len() - 1) {
                    let info = crate::video_stream::Inspector::default()
                        .receive(bytes)
                        .unwrap();
                    assert!(queue.submit(bytes, info.idr, info.parameters, index as i64));
                    queue.set_last_packet(0, info.picture);
                } else {
                    assert!(queue.submit(&[index as u8], index == 0, false, index as i64));
                }
            }
            for frame in &mut queue.frames {
                frame.queued_at = now - CATCH_UP_LATE_WAIT;
            }
            let mut decoded = Vec::new();
            let mut displayed = Vec::new();
            for elapsed in arrivals {
                let frame = queue.pop_at(now + Duration::from_millis(*elapsed)).unwrap();
                decoded.push(frame.timestamp_100ns);
                if !queue.should_skip_display() {
                    displayed.push(frame.timestamp_100ns);
                }
                // Output, retry and original-surface checks must not each add
                // a history observation or count an activation again.
                let activations = queue.report.adaptive_display_catch_up_activations;
                let decision = queue.should_skip_display();
                assert_eq!(queue.should_skip_display(), decision);
                assert_eq!(
                    queue.report.adaptive_display_catch_up_activations,
                    activations
                );
                queue.recycle(frame.bytes);
            }
            assert_eq!(queue.report.frames_dropped, 0);
            assert_eq!(queue.report.queue_bytes, 0);
            assert_eq!(queue.reset_generation, 0);
            assert!(!queue.display_catch_up.aggressive);
            (
                decoded,
                displayed,
                queue.report.adaptive_display_catch_up_activations,
            )
        }
        use DisplayCatchUpPolicy::{Adaptive, Fixed};
        assert_eq!(
            drain(Adaptive, &[0, 2, 4, 6], None),
            drain(Fixed, &[0, 2, 4, 6], None)
        );
        let fixed = drain(Fixed, &[0, 50, 100, 150], None);
        let adaptive = drain(Adaptive, &[0, 50, 100, 150], None);
        assert_eq!(fixed.0, adaptive.0);
        assert_eq!(fixed.1, [2, 3]);
        assert_eq!(adaptive.1, [3]);
        assert_eq!((fixed.2, adaptive.2), (0, 1));
        // Parameter, SEI and access-unit-delimiter tails are parsed through the
        // real inspector; none may justify losing the preceding decoded image.
        for metadata in [
            &[0, 0, 1, 0x68, 0x80][..],
            &[0, 0, 1, 0x06, 0x80],
            &[0, 0, 1, 0x09, 0x80],
        ] {
            let headers = drain(Adaptive, &[0, 50, 100, 150], Some(metadata));
            assert_eq!(headers.1, fixed.1);
            assert_eq!(headers.2, 1);
        }
        let combined = [0, 0, 1, 0x67, 100, 0, 31, 0, 0, 1, 0x41, 0x80];
        assert_eq!(drain(Adaptive, &[0, 50, 100, 150], Some(&combined)).1, [3]);
        assert_eq!(drain(Adaptive, &[0, 20, 200, 220], None).2, 0);
        assert_eq!(drain(Adaptive, &[4000], None).1, [0]);
        assert!(std::mem::size_of::<DisplayCatchUp>() <= 64);
    }

    #[test]
    fn adaptive_catch_up_does_not_escalate_fresh_input_at_varied_frame_rates() {
        let now = Instant::now();
        for period_ns in [4_166_667, 6_944_444, 16_666_667, 16_680_567, 33_333_333] {
            let mut queue = Queue::with_display_policy(DisplayCatchUpPolicy::Adaptive);
            assert!(queue.submit(&[0], true, false, 0));
            for index in 1..=120 {
                assert!(queue.submit(&[1], false, false, index));
                let sample = now + Duration::from_nanos(period_ns * index as u64);
                queue.frames.front_mut().unwrap().queued_at = sample - Duration::from_millis(4);
                let frame = queue.pop_at(sample).unwrap();
                assert!(!queue.should_skip_display());
                queue.recycle(frame.bytes);
            }
            assert_eq!(queue.report.adaptive_display_catch_up_activations, 0);
        }
    }

    #[test]
    fn adaptive_catch_up_resets_history_at_recovery_and_encoder_boundaries() {
        let now = Instant::now();
        for rebuild in [false, true] {
            let mut queue = Queue::with_display_policy(DisplayCatchUpPolicy::Adaptive);
            for index in 0..4 {
                assert!(queue.submit(&[1], index == 0, false, index));
            }
            for frame in &mut queue.frames {
                frame.queued_at = now - CATCH_UP_LATE_WAIT;
            }
            for elapsed in [0, 50, 100] {
                let frame = queue.pop_at(now + Duration::from_millis(elapsed)).unwrap();
                queue.recycle(frame.bytes);
            }
            assert!(queue.should_skip_display());
            assert_eq!(queue.report.adaptive_display_catch_up_activations, 1);
            if rebuild {
                assert!(queue.encoder_rebuilt(now));
            } else {
                assert!(queue.recover(now));
            }
            assert!(!queue.display_catch_up.aggressive);
            assert!(queue.display_catch_up.late_since.is_none());
            assert!(queue.submit(&[2], true, false, 4));
            assert!(queue.submit(&[3], false, false, 5));
            queue.frames.front_mut().unwrap().queued_at = now - Duration::from_secs(1);
            queue.pop_at(now + Duration::from_secs(1)).unwrap();
            assert!(!queue.should_skip_display());
            assert_eq!(queue.report.adaptive_display_catch_up_activations, 1);
        }
    }
    #[test]
    fn late_refresh_failures_cannot_stop_a_new_or_recovered_generation() {
        let now = Instant::now();
        let mut q = Queue::default();
        assert!(q.submit(&[1], true, false, 0));
        assert!(q.recover(now));
        let old = q.reset_generation;
        assert!(q.recover(now + Duration::from_secs(1)));
        q.request_failed(old);
        assert_eq!(q.report.failure_stage, None);
        q.recovered();
        q.request_failed(q.reset_generation);
        assert_eq!(q.report.failure_stage, None);
        assert!(q.recover(now + Duration::from_secs(2)));
        q.request_failed(q.reset_generation);
        assert_eq!(
            q.report.failure_stage,
            Some("video-recovery-request-failed")
        );
    }
    #[test]
    fn format_changes_keep_queued_keys_and_do_not_spend_error_retries() {
        let mut queue = Queue::default();
        let now = Instant::now();
        assert!(queue.submit(&[1], true, true, 0));
        for _ in 0..8 {
            queue.begin_format_change(now, false);
        }
        assert!(queue.pop().unwrap().idr);
        assert_eq!(queue.reset_generation, 0);
        assert_eq!(queue.recovery_attempts, 0);
        assert!(queue.submit(&[2], true, true, 1));
        queue.begin_format_change(now, true);
        assert!(queue.take_keyframe_request(now).is_some());
        assert!(queue.pop().unwrap().idr);
        queue.recovered();
        assert!(queue.submit(&[3], false, false, 2));
        queue.begin_format_change(now, true);
        // A second boundary still respects the existing keyframe rate limit.
        assert!(queue.take_keyframe_request(now).is_none());
        queue.check_recovery(now + Duration::from_secs(5));
        assert_eq!(queue.report.failure_stage, Some("video-recovery-timeout"));
    }

    #[test]
    fn recovery_is_rate_limited_bounded_and_stops_on_timeout_or_cancellation() {
        let now = Instant::now();
        let mut q = Queue::default();
        assert!(q.submit(&[1], true, false, 0));
        assert!(q.recover(now));
        assert_eq!(
            q.take_keyframe_request(now),
            Some(crate::video_stream::Codec::H264)
        );
        assert_eq!(q.take_keyframe_request(now), None);
        assert!(q.recover(now + Duration::from_millis(10)));
        assert_eq!(
            q.take_keyframe_request(now + Duration::from_millis(999)),
            None
        );
        assert!(q
            .take_keyframe_request(now + Duration::from_secs(1))
            .is_some());
        q.recovered();
        assert!(q.recover(now + Duration::from_secs(2)));
        assert!(!q.recover(now + Duration::from_secs(3)));
        assert_eq!(q.report.failure_stage, Some("video-recovery-exhausted"));
        assert_eq!(q.take_keyframe_request(now + Duration::from_secs(10)), None);

        let mut q = Queue::default();
        assert!(q.submit(&[1], true, false, 0));
        assert!(q.recover(now));
        q.check_recovery(now + Duration::from_secs(5));
        assert_eq!(q.report.failure_stage, Some("video-recovery-timeout"));
        assert!(!q.report.recovering);
        let mut q = Queue {
            stopped: true,
            ..Default::default()
        };
        assert!(!q.recover(now));
        assert_eq!(q.take_keyframe_request(now), None);
    }
    #[test]
    fn compressed_buffers_are_reused_only_after_the_frame_is_consumed() {
        let mut q = Queue::default();
        let data = vec![7; 1024];
        let pointer = data.as_ptr();
        assert!(q.submit_codec(data, true, false, 0, crate::video_stream::Codec::H264));
        assert!(q.take_buffer().is_empty());
        let frame = q.pop().unwrap();
        assert_eq!(frame.bytes.as_ptr(), pointer);
        q.recycle(frame.bytes);
        let reused = q.take_buffer();
        assert_eq!(reused.as_ptr(), pointer);
        assert!(reused.is_empty());
        q.recycle(reused);
        q.stopped = true;
        q.clear();
        assert!(q.free_buffers.is_empty());
        assert_eq!(q.free_bytes, 0);
    }
    #[test]
    fn queued_frames_preserve_codec_and_refuse_a_midstream_change() {
        use crate::video_stream::Codec;
        let mut q = Queue::default();
        assert!(q.submit_codec(vec![1], false, true, 0, Codec::Hevc));
        assert_eq!(q.pop().unwrap().codec, Codec::Hevc);
        assert!(q.submit_codec(vec![2], true, false, 1, Codec::Hevc));
        assert!(!q.submit_codec(vec![3], true, false, 2, Codec::H264));
        assert_eq!(
            q.report.failure_stage,
            Some("video-unexpected-codec-change")
        );
        assert_eq!(q.report.queue_depth, 0);
    }
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
    fn overflow_discards_dependencies_and_waits_for_a_new_keyframe() {
        let mut q = Queue::default();
        for _ in 0..MAX_FRAMES {
            assert!(q.submit(&[1, 2, 3], true, false, 0));
        }
        assert!(!q.submit(&[4], false, false, 0));
        assert_eq!(q.report.failure_stage, None);
        assert!(q.report.recovering);
        assert!(q.report.waiting_for_idr);
        assert_eq!(q.reset_generation, 1);
        assert_eq!(q.report.queue_depth, 0);
        assert_eq!(q.report.queue_bytes, 0);
        assert_eq!(q.report.frames_dropped, MAX_FRAMES as u64 + 1);
        assert!(!q.submit(&[5], false, false, 0));
        assert!(q.submit(&[6], true, false, 0));
        assert_eq!(q.pop().unwrap().bytes, vec![6]);
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
        assert_eq!(q.pop().unwrap().bytes.as_slice(), &[2]);
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
            &vec![0; crate::control::MAX_CHANNEL_MESSAGE + 1],
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

#[cfg(test)]
mod optional_warmup_tests {
    use super::*;
    #[test]
    fn optional_effect_reuse_does_not_mask_decoder_errors_or_unknown_resets() {
        assert!(can_preserve_effect(None)); // explicit encoder rebuild
        assert!(can_preserve_effect(Some("optional_video_setup_backlog")));
        assert!(can_preserve_effect(Some("compressed_queue_full")));
        assert!(!can_preserve_effect(Some("decoder_recovery")));
        assert!(!can_preserve_effect(Some("unknown_future_failure")));
    }
    #[test]
    fn bounded_optional_warmup_drops_stale_input_without_consuming_error_retries() {
        let mut queue = Queue::default();
        let start = Instant::now();
        queue.begin_native_call(NativeOperation::Setup, start);
        queue.optional_setup(true);
        for _ in 0..5 {
            for _ in 0..MAX_FRAMES {
                assert!(queue.submit(&[1], true, false, 0));
            }
            assert!(!queue.submit(&[1], true, false, 0));
            assert_eq!(queue.recovery_attempts, 0);
            assert_eq!(queue.report.queue_depth, 0);
            assert_eq!(
                queue.report.recovery_reason,
                Some("optional_video_setup_backlog")
            );
            assert!(queue.report.failure_stage.is_none());
        }
        queue.optional_setup(false);
        for _ in 0..MAX_FRAMES {
            assert!(queue.submit(&[1], true, false, 0));
        }
        assert!(!queue.submit(&[1], true, false, 0));
        assert_eq!(queue.recovery_attempts, 1);
        assert_eq!(queue.report.recovery_reason, Some("compressed_queue_full"));
        queue.check_native_call(start + Duration::from_secs(10));
        assert_eq!(
            queue.report.failure_stage,
            Some("video-native-setup-timeout-restart-required")
        );
    }
}
