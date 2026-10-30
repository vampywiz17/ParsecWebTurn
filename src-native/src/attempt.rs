//! Native WebRTC offer creation and managed connection lifecycle.
//! Host signaling translation is isolated from the ICE/DTLS/SCTP engine.
#[cfg(target_os = "android")]
use crate::video_android as native_video;
#[cfg(windows)]
use crate::video_windows as native_video;
use crate::{
    signaling::{Candidate, CandidateGate, Credentials, Description, LocalCandidateSync},
    transport::CHANNELS,
};
use anyhow::{bail, Context, Result};
use bytes::Bytes;
use std::collections::VecDeque;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    time::Duration,
};
pub(crate) const MAX_CHANNEL_MESSAGE: usize = crate::control::MAX_CHANNEL_MESSAGE;
static ACTIVE_WORKERS: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE_WORKERS.fetch_sub(1, Ordering::SeqCst);
    }
}
use webrtc::{
    api::{setting_engine::SettingEngine, APIBuilder},
    data_channel::data_channel_init::RTCDataChannelInit,
    data_channel::data_channel_state::RTCDataChannelState,
    ice::network_type::NetworkType,
    ice_transport::ice_server::RTCIceServer,
    ice_transport::{ice_candidate_type::RTCIceCandidateType, ice_protocol::RTCIceProtocol},
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription,
    },
};

type Publish = dyn Fn(Option<&Description>) -> Result<bool> + Send + Sync;

#[derive(Clone)]
pub struct Output(Arc<Publish>);
impl Output {
    pub fn new(publish: Arc<Publish>) -> Self {
        Self(publish)
    }
    pub fn finish(&self, credentials: Option<&Description>) -> Result<bool> {
        (self.0)(credentials)
    }
}

/// Static categories, never underlying errors, addresses or ICE credentials.
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureStage {
    Worker,
    InboundChannel,
    DataChannelDetach,
    DataChannelRead,
    DataChannelClosed,
    InboundQueueFull,
    ControlStartup,
    ChannelSend,
    IceTransport,
    DtlsTransport,
    PeerTransport,
    SetLocalDescription,
    SetRemoteDescription,
    CandidateGate,
    AddIceCandidate,
    AttemptMismatch,
    CommandQueueFull,
    CommandQueueClosed,
    Cancelled,
    Deadline,
}
impl std::fmt::Display for FailureStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for FailureStage {}

#[derive(Default)]
struct Progress {
    #[cfg(test)]
    relay_only: bool,
    #[cfg(test)]
    relay_only_connectivity: bool,
    #[cfg(test)]
    resolved_ice_servers: Option<usize>,
    ready: bool,
    closed: bool,
    failed: bool,
    failure_stage: Option<FailureStage>,
    channels: usize,
    mid: String,
    open_mask: u8,
    transport_connected: bool,
    connection_established: bool,
    local_candidates: usize,
    local_host_candidates: usize,
    local_srflx_candidates: usize,
    remote_candidates: usize,
    messages_received: usize,
    channel_messages_received: [u64; 3],
    channel_bytes_received: [u64; 3],
    channel_max_message_bytes: [usize; 3],
    empty_channel_messages: [u64; 3],
    failure_channel: Option<u16>,
    read_error: Option<&'static str>,
    failure_elapsed_ms: Option<u64>,
    last_send_elapsed_ms: Option<u64>,
    last_sent_control_kind: Option<u8>,
    control_ready: bool,
    local_description_set: bool,
    remote_description_set: bool,
    sync_received: bool,
    local_gathering_complete: bool,
    local_sync: LocalCandidateSync,
    local_sync_queued: bool,

    transport_states: serde_json::Value,
    transport_states_before_close: serde_json::Value,
    transport_states_at_failure: serde_json::Value,
    #[cfg(test)]
    stun_provider: StunProvider,
    #[cfg(test)]
    legacy_rsa_1024: bool,
}

struct Completion {
    #[cfg(windows)]
    telemetry: Option<(Arc<crate::stats::Shared>, u64)>,
    #[cfg(windows)]
    timing_active: bool,
    started_at: std::time::Instant,
    closing: bool,
    output: Option<Output>,
    progress: Progress,
    cancelled: bool,
    events: VecDeque<serde_json::Value>,
    messages: VecDeque<(u16, bool, Bytes)>,
    message_bytes: usize,
    // Native session media bypasses the receipt queue; transport probes retain it.
    discard_unavailable_media: bool,
    media_ingress: crate::media_ingress::Ingress,
    video_stream: Arc<Mutex<crate::video_stream::Inspector>>,
    audio_stream: Option<crate::audio_stream::Pipeline>,
    #[cfg(any(windows, target_os = "android"))]
    video_output: Option<Arc<native_video::Pipeline>>,
}

enum Delivery {
    Queued,
    EmptyMessage,
    #[cfg(windows)]
    Timing(Option<(Arc<crate::stats::Shared>, u64)>),
    Audio(Option<crate::audio_stream::Sender>),
    Video {
        inspector: Arc<Mutex<crate::video_stream::Inspector>>,
        #[cfg(any(windows, target_os = "android"))]
        pipeline: Option<Arc<native_video::Pipeline>>,
    },
}
impl Delivery {
    fn submit(self, bytes: &[u8]) -> Option<bool> {
        match self {
            Self::Queued => None,
            Self::EmptyMessage => Some(false),
            #[cfg(windows)]
            Self::Timing(link) => {
                if let Some((bus, generation)) = link {
                    bus.host_timing.receive(generation, bytes);
                }
                None
            }
            Self::Audio(audio) => Some(!audio.is_some_and(|sender| sender.submit(bytes))),
            Self::Video {
                inspector,
                #[cfg(any(windows, target_os = "android"))]
                pipeline,
            } => {
                let info = inspector
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .receive(bytes);
                #[cfg(any(windows, target_os = "android"))]
                if let (Some(pipeline), Some(info)) = (pipeline, info) {
                    return Some(!pipeline.submit(bytes, info));
                }
                #[cfg(not(any(windows, target_os = "android")))]
                let _ = info;
                Some(true)
            }
        }
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some((bus, generation)) = &self.telemetry {
            bus.finish(*generation);
        }
    }
}
impl Completion {
    #[cfg(any(target_os = "android", test))]
    fn network_diagnostics(&self) -> serde_json::Value {
        let p = &self.progress;
        let states = if p.failed && p.transport_states_at_failure.is_object() {
            &p.transport_states_at_failure
        } else {
            &p.transport_states
        };
        serde_json::json!({
            "failure_stage":p.failed.then_some(p.failure_stage.unwrap_or(FailureStage::Worker)),
            "failure_channel":p.failure_channel,"read_error":p.read_error,
            "failure_elapsed_ms":p.failure_elapsed_ms,"states":states,
            "received_bytes":p.channel_bytes_received,"received_messages":p.channel_messages_received,
            "max_message_bytes":p.channel_max_message_bytes,
            "empty_messages":p.empty_channel_messages,
            "queued_control_messages":self.messages.len(),"queued_control_bytes":self.message_bytes,
        })
    }
    fn queue_local_sync(&mut self, marker: Option<serde_json::Value>) {
        if let Some(marker) = marker {
            if self.events.len() >= 64 {
                self.progress.failed = true;
            } else {
                self.events.push_back(marker);
                self.progress.local_sync_queued = true;
            }
        }
    }

    fn relay_readiness_sync(&mut self, id: &str) {
        if self.cancelled || self.closing {
            return;
        }
        let relay_candidates = self.progress.local_candidates.saturating_sub(
            self.progress.local_host_candidates + self.progress.local_srflx_candidates,
        );
        let marker = self.progress.local_sync.relay_ready(
            id,
            self.progress.remote_description_set,
            relay_candidates,
        );
        self.queue_local_sync(marker);
    }

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn fail_channel(&mut self, stage: FailureStage, channel: u16) {
        if self.cancelled || self.closing {
            return;
        }
        self.progress.failed = true;
        if self.progress.failure_stage.is_none() {
            self.progress.transport_states_at_failure = self.progress.transport_states.clone();
            self.progress.failure_stage = Some(stage);
            self.progress.failure_channel = Some(channel);
            self.progress.failure_elapsed_ms = Some(self.elapsed_ms());
        }
        if channel < 3 {
            self.progress.open_mask &= !(1 << channel);
        }
    }

    fn prepare_receive(&mut self, channel: u16, text: bool, bytes: &[u8]) -> Option<Delivery> {
        if self.cancelled || self.closing || self.progress.failed {
            return None;
        }
        if channel > 2 || text || bytes.len() > MAX_CHANNEL_MESSAGE {
            self.fail_channel(FailureStage::InboundChannel, channel);
            return None;
        }
        let i = usize::from(channel);
        self.progress.messages_received = self.progress.messages_received.saturating_add(1);
        self.progress.channel_messages_received[i] =
            self.progress.channel_messages_received[i].saturating_add(1);
        self.progress.channel_bytes_received[i] =
            self.progress.channel_bytes_received[i].saturating_add(bytes.len() as u64);
        self.progress.channel_max_message_bytes[i] =
            self.progress.channel_max_message_bytes[i].max(bytes.len());
        if bytes.is_empty() {
            self.progress.empty_channel_messages[i] =
                self.progress.empty_channel_messages[i].saturating_add(1);
            if self.discard_unavailable_media {
                return Some(Delivery::EmptyMessage);
            }
        }
        #[cfg(windows)]
        if channel == 0 && self.discard_unavailable_media && crate::frame_timing::is_report(bytes) {
            return Some(Delivery::Timing(self.telemetry.clone()));
        }
        if self.discard_unavailable_media {
            if channel == 1 {
                return Some(Delivery::Video {
                    inspector: self.video_stream.clone(),
                    #[cfg(any(windows, target_os = "android"))]
                    pipeline: self.video_output.clone(),
                });
            }
            return Some(Delivery::Audio(
                self.audio_stream.as_ref().and_then(|p| p.sender()),
            ));
        }
        if self.messages.len() >= 16 || self.message_bytes + bytes.len() > 4 * MAX_CHANNEL_MESSAGE {
            self.fail_channel(FailureStage::InboundQueueFull, channel);
            return None;
        }
        Some(Delivery::Queued)
    }

    fn queue_received(&mut self, channel: u16, text: bool, bytes: Bytes) -> bool {
        // Own the already copied payload; never copy a detached read buffer
        // while holding the connection lock.
        if self.cancelled || self.closing || self.progress.failed {
            return false;
        }
        if self.messages.len() >= 16 || self.message_bytes + bytes.len() > 4 * MAX_CHANNEL_MESSAGE {
            self.fail_channel(FailureStage::InboundQueueFull, channel);
            return false;
        }
        self.message_bytes += bytes.len();
        self.messages.push_back((channel, text, bytes));
        true
    }

    #[cfg(test)]
    fn receive(&mut self, channel: u16, text: bool, bytes: &[u8]) -> bool {
        let Some(delivery) = self.prepare_receive(channel, text, bytes) else {
            return false;
        };
        if matches!(delivery, Delivery::Queued) {
            return self.queue_received(channel, text, Bytes::copy_from_slice(bytes));
        }
        if let Some(discarded) = delivery.submit(bytes) {
            self.media_ingress.record(channel, bytes.len(), discarded);
        }
        true
    }
}

// Public detached API preserves complete SCTP messages beyond the library's
// fixed 65,535-byte callback buffer. All three channels use the same API.
async fn read_channel(
    shared: Arc<Mutex<Completion>>,
    id: u16,
    channel: Arc<webrtc::data::data_channel::DataChannel>,
) {
    let mut buffer = vec![0; MAX_CHANNEL_MESSAGE];
    // This session mode is immutable. Native media goes straight to its sink;
    // only control/probe receipts need an owned copy for the control queue.
    let native_mode = shared
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .discard_unavailable_media;
    let queue_receipts = id == 0 || !native_mode;
    loop {
        let read = read_data_message(&channel, &mut buffer).await;
        let len = read
            .as_ref()
            .ok()
            .and_then(|m| *m)
            .map_or(0, |(len, _)| len);
        let delivery = match read {
            Ok(None) => {
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                state.fail_channel(FailureStage::DataChannelClosed, id);
                None
            }
            Ok(Some((len, text))) => {
                #[cfg(windows)]
                let timing =
                    native_mode && id == 0 && crate::frame_timing::is_report(&buffer[..len]);
                #[cfg(not(windows))]
                let timing = false;
                let payload = (queue_receipts && !timing && !text && len <= MAX_CHANNEL_MESSAGE)
                    .then(|| Bytes::copy_from_slice(&buffer[..len]));
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                match state.prepare_receive(id, text, &buffer[..len]) {
                    Some(Delivery::Queued) => payload.and_then(|payload| {
                        state
                            .queue_received(id, text, payload)
                            .then_some(Delivery::Queued)
                    }),
                    delivery => delivery,
                }
            }
            Err(error) => {
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                if !state.cancelled && !state.closing && state.progress.failure_stage.is_none() {
                    state.progress.read_error = Some(classify_channel_read_error(&error));
                }
                state.fail_channel(FailureStage::DataChannelRead, id);
                None
            }
        };
        let keep_reading = delivery.is_some();
        if let Some(delivery) = delivery {
            // Copy/submit outside the connection lock. Consume sender handles
            // before bookkeeping, so cancellation never joins a blocked reader.
            if let Some(discarded) = delivery.submit(&buffer[..len]) {
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                if !state.cancelled && !state.closing {
                    state.media_ingress.record(id, len, discarded);
                }
            }
        }
        if !keep_reading {
            break;
        }
    }
}

/// The detached API returns zero for both a valid empty message (RFC 8831)
/// and a reset stream. Its public message counter advances only for a message.
/// This channel has one reader, so an empty message is distinguishable from EOF
/// without private SCTP state, extra buffering, retries or changing ordering.
pub(crate) async fn read_data_message(
    channel: &webrtc::data::data_channel::DataChannel,
    buffer: &mut [u8],
) -> std::result::Result<Option<(usize, bool)>, webrtc::data::Error> {
    let before = channel.messages_received();
    let message = channel.read_data_channel(buffer).await?;
    Ok((message.0 != 0 || channel.messages_received() != before).then_some(message))
}

// Fixed categories only: library error strings can contain remote text or
// addresses. Do not serialize those strings or private connection credentials.
fn classify_channel_read_error(error: &webrtc::data::Error) -> &'static str {
    use webrtc::{data::Error as D, sctp::Error as S};
    match error {
        D::Sctp(S::ErrShortBuffer { .. }) => "message-exceeds-read-buffer",
        D::Sctp(S::ErrNetConnReadError) => "sctp-network-read",
        D::Sctp(S::ErrEof) => "sctp-eof",
        D::Sctp(S::ErrStreamClosed) | D::ErrStreamClosed => "stream-closed",
        D::Sctp(_) => "sctp-error",
        D::Util(_) => "transport-io",
        _ => "datachannel-error",
    }
}

enum Command {
    Begin(Credentials),
    Candidate(Candidate),
    Sync,
    Send(u16, Bytes),
    Disconnect,
}

/// Default web-client STUN, with offline/test alternatives kept explicit.
#[derive(Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StunProvider {
    #[default]
    None,

    Parsec,
    #[cfg(test)]
    Cloudflare,
}
impl StunProvider {
    fn server(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::None => None,
            // Audited constructor in the pinned public parsec.js (150-104a).
            Self::Parsec => Some(("parsec", "stun:stun.parsec.gg:3478")),
            #[cfg(test)]
            Self::Cloudflare => Some(("cloudflare", "stun:stun.cloudflare.com:3478")),
        }
    }
}

pub struct Attempt {
    commands: Option<mpsc::SyncSender<Command>>,
    id: String,
    completion: Arc<Mutex<Completion>>,
    finished: Arc<(Mutex<bool>, Condvar)>,
}

impl Attempt {
    pub fn spawn_configured(
        id: &str,
        output: Output,
        config: Option<crate::control::Config>,
        stun_provider: StunProvider,
        legacy_rsa_1024: bool,
        connection_settings: Option<Arc<dyn crate::rtc_configuration::NetworkSettings>>,
    ) -> Result<Self> {
        CandidateGate::new(id)?;
        let mut active = ACTIVE_WORKERS.load(Ordering::SeqCst);
        loop {
            if active >= 8 {
                bail!("native offer worker limit reached");
            }
            match ACTIVE_WORKERS.compare_exchange(
                active,
                active + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(current) => active = current,
            }
        }
        let permit = Permit;
        let (tx, rx) = mpsc::sync_channel(64);
        let mut video_stream = crate::video_stream::Inspector::default();
        if let Some(config) = &config {
            video_stream.configure(config.video_protocol.clone());
        }
        let completion = Arc::new(Mutex::new(Completion {
            #[cfg(windows)]
            telemetry: None,
            #[cfg(windows)]
            timing_active: false,
            started_at: std::time::Instant::now(),
            closing: false,
            output: Some(output),
            progress: Progress {
                #[cfg(test)]
                stun_provider,
                #[cfg(test)]
                legacy_rsa_1024,
                ..Default::default()
            },
            cancelled: false,
            events: Default::default(),
            messages: Default::default(),
            message_bytes: 0,
            discard_unavailable_media: config.is_some(),
            media_ingress: Default::default(),
            video_stream: Arc::new(Mutex::new(video_stream)),
            #[cfg(any(windows, target_os = "android"))]
            video_output: None,
            audio_stream: None,
        }));
        let finished = Arc::new((Mutex::new(false), Condvar::new()));
        let shared = completion.clone();
        let done = finished.clone();
        let attempt_id = id.to_owned();
        std::thread::Builder::new()
            .name("evoparseq-offer".into())
            .spawn(move || {
                let _permit = permit;
                let result = worker(
                    &shared,
                    &attempt_id,
                    rx,
                    config,
                    NetworkOptions {
                        stun_provider,
                        legacy_rsa_1024,
                        connection_settings,
                    },
                );
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                #[cfg(windows)]
                if let Some((bus, generation)) = &state.telemetry {
                    bus.finish(*generation);
                }
                if result.is_err() {
                    state.progress.failed = true;
                    if state.progress.failure_elapsed_ms.is_none() {
                        state.progress.failure_elapsed_ms = Some(state.elapsed_ms());
                    }
                    let stage = result
                        .as_ref()
                        .err()
                        .and_then(|e| e.downcast_ref::<FailureStage>())
                        .copied()
                        .unwrap_or(FailureStage::Worker);
                    state.progress.failure_stage.get_or_insert(stage);
                    if let Some(output) = state.output.take() {
                        let _ = output.finish(None);
                    }
                }
                *done.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
                done.1.notify_all();
            })
            .context("starting native offer worker")?;
        Ok(Self {
            commands: Some(tx),
            id: id.into(),
            completion,
            finished,
        })
    }

    pub fn disconnect(&mut self) {
        // Let the transport worker send the pinned client's exit notification
        // before cancellation closes SCTP. Never wait indefinitely for a host.
        if let Some(commands) = &self.commands {
            if commands.try_send(Command::Disconnect).is_ok() {
                let _ = self.wait_finished(Duration::from_millis(900));
            }
        }
        self.cancel();
    }

    pub fn cancel(&mut self) {
        // Serialize publication and cancellation. Taking the output lease
        // prevents the old worker from writing into retired output callbacks.
        let mut state = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        state.cancelled = true;
        #[cfg(any(windows, target_os = "android"))]
        if let Some(pipeline) = &state.video_output {
            pipeline.stop();
        }
        let audio = state.audio_stream.take();
        state.events.clear();
        state.messages.clear();
        state.message_bytes = 0;
        if let Some(output) = state.output.take() {
            if output.finish(None).is_err() {
                state.progress.failed = true;
            }
        }
        drop(state);
        if let Some(mut audio) = audio {
            audio.stop();
        }
        self.commands.take();
    }

    pub fn failure_stage(&self) -> Option<FailureStage> {
        let state = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        state
            .progress
            .failed
            .then_some(state.progress.failure_stage.unwrap_or(FailureStage::Worker))
    }

    #[cfg(target_os = "android")]
    pub fn network_diagnostics(&self) -> serde_json::Value {
        self.completion
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .network_diagnostics()
    }

    #[cfg(windows)]
    pub fn native_video_failure(&self) -> Option<&'static str> {
        let pipeline = self
            .completion
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .video_output
            .clone();
        pipeline.and_then(|v| v.native_failure())
    }

    pub fn host_status(&self) -> Option<i32> {
        // A host status can precede SCTP closure in the same read batch. Keep
        // its authenticated numeric reason available before a generic failure.
        self.completion
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .messages
            .iter()
            .find_map(|(channel, text, bytes)| {
                if *channel != 0 || *text || bytes.len() < 13 || bytes[12] != 10 {
                    return None;
                }
                let status = i32::from_be_bytes(bytes[..4].try_into().ok()?);
                (status != 0).then_some(status)
            })
    }

    pub fn control_ready(&self) -> bool {
        self.completion
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .progress
            .control_ready
    }

    #[cfg(windows)]
    pub fn start_video(&self, window: Arc<crate::window::Window>) {
        {
            let state = self.completion.lock().unwrap_or_else(|e| e.into_inner());
            if state.cancelled || state.closing || state.video_output.is_some() {
                return;
            }
        }
        let generation = window.stats.begin();
        let audio = crate::audio_stream::Pipeline::start_with_budget(
            window.audio_buffer().map(|(_, max)| max),
        );
        let video = Arc::new(crate::video_windows::Pipeline::start(
            window.clone(),
            crate::video_output::DisplayCatchUpPolicy::Adaptive,
        ));
        let mut state = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        if state.cancelled || state.closing || state.video_output.is_some() {
            drop(state);
            video.stop();
            window.stats.finish(generation);
            return;
        }
        state.telemetry = Some((window.stats.clone(), generation));
        state.audio_stream = Some(audio);
        state.video_output = Some(video);
    }

    #[cfg(target_os = "android")]
    pub fn start_android_media(&self, video: Arc<crate::video_android::Pipeline>) {
        let mut state = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        state.audio_stream = Some(crate::audio_stream::Pipeline::start_with_budget(Some(100)));
        state.video_output = Some(video);
    }

    pub fn audio_receiver(&self) -> Option<crate::audio_stream::NativeReceiver> {
        self.completion
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .audio_stream
            .as_ref()
            .map(|audio| audio.native_receiver())
    }
    pub fn pop_binary(&self) -> Option<(u16, bool, Bytes)> {
        let mut state = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        let message = state.messages.pop_front()?;
        state.message_bytes -= message.2.len();
        Some(message)
    }

    #[cfg(test)]
    pub fn snapshot(&self) -> serde_json::Value {
        let state = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        let mut snapshot = serde_json::json!({ "offer_ready": state.progress.ready, "mid":state.progress.mid, "peer_closed": state.progress.closed, "failed": state.progress.failed, "failure_stage": state.progress.failure_stage, "negotiated_channels": state.progress.channels, "channels_open": state.progress.open_mask.count_ones(), "transport_connected":state.progress.transport_connected, "local_candidates":state.progress.local_candidates, "remote_candidates":state.progress.remote_candidates, "messages_received":state.progress.messages_received, "worker_finished": *self.finished.0.lock().unwrap_or_else(|e| e.into_inner()), "host_connected": false,
        "local_description_set":state.progress.local_description_set,"remote_description_set":state.progress.remote_description_set,"sync_received":state.progress.sync_received,"transport_states":state.progress.transport_states,"transport_states_before_close":state.progress.transport_states_before_close,"transport_states_at_failure":state.progress.transport_states_at_failure,
        "local_host_candidates":state.progress.local_host_candidates,"local_srflx_candidates":state.progress.local_srflx_candidates,
        "data_channel_only":true,"legacy_rsa_1024_enabled":state.progress.legacy_rsa_1024,"ice_servers_configured":state.progress.stun_provider.server().is_some(),"stun_provider":state.progress.stun_provider.server().map(|(name, _)| name),"network_types":["udp4"],"connection_deadline_seconds":30,"connection_deadline_scope":"establishment-only","connection_established":state.progress.connection_established,
        "channel_receive_api":"detached","unavailable_media_bypasses_control_queue":state.discard_unavailable_media,"queued_messages":state.messages.len(),"queued_message_bytes":state.message_bytes,"channel_message_limit_bytes":MAX_CHANNEL_MESSAGE,
        "channel_messages_received":state.progress.channel_messages_received,
        "channel_bytes_received":state.progress.channel_bytes_received,
        "channel_max_message_bytes":state.progress.channel_max_message_bytes,
        "failure_channel":state.progress.failure_channel,"failure_elapsed_ms":state.progress.failure_elapsed_ms,
        "last_send_elapsed_ms":state.progress.last_send_elapsed_ms,"last_sent_control_kind":state.progress.last_sent_control_kind });
        snapshot["local_relay_candidates"] =
            serde_json::json!(state.progress.local_candidates.saturating_sub(
                state.progress.local_host_candidates + state.progress.local_srflx_candidates
            ));
        snapshot["relay_only_policy"] = serde_json::json!(state.progress.relay_only);
        snapshot["relay_only_connectivity"] =
            serde_json::json!(state.progress.relay_only_connectivity);
        snapshot["ice_gathering_policy"] = serde_json::json!(if state.progress.relay_only {
            "relay"
        } else {
            "all"
        });
        snapshot["local_gathering_complete"] =
            serde_json::json!(state.progress.local_gathering_complete);
        snapshot["local_sync_queued"] = serde_json::json!(state.progress.local_sync_queued);

        snapshot["ice_servers_configured"] =
            serde_json::json!(state.progress.resolved_ice_servers.map(|count| count != 0));
        snapshot["resolved_ice_server_entries"] =
            serde_json::json!(state.progress.resolved_ice_servers);
        snapshot
    }

    fn command(&self, id: &str, command: Command) -> Result<()> {
        if id != self.id {
            return Err(FailureStage::AttemptMismatch.into());
        }
        self.commands
            .as_ref()
            .context(FailureStage::Cancelled)?
            .try_send(command)
            .map_err(|error| {
                anyhow::Error::new(match error {
                    mpsc::TrySendError::Full(_) => FailureStage::CommandQueueFull,
                    mpsc::TrySendError::Disconnected(_) => FailureStage::CommandQueueClosed,
                })
            })
    }
    pub fn begin(&self, id: &str, remote: Credentials) -> Result<()> {
        remote.validate_parsec_remote()?;
        self.command(id, Command::Begin(remote))
    }
    pub fn candidate(&self, id: &str, candidate: Candidate) -> Result<()> {
        self.command(id, Command::Candidate(candidate))
    }
    pub fn sync(&self, id: &str) -> Result<()> {
        self.command(id, Command::Sync)
    }
    pub fn send_binary(&self, channel: u16, payload: Bytes) -> Result<()> {
        if channel > 2 || payload.len() > MAX_CHANNEL_MESSAGE {
            bail!("invalid native channel/message size");
        }
        self.command(&self.id, Command::Send(channel, payload))
    }
    pub fn pop_event(&self) -> Option<serde_json::Value> {
        self.completion
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .events
            .pop_front()
    }
    pub fn wait_finished(&self, duration: Duration) -> Result<()> {
        let done = self
            .finished
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("offer completion lock poisoned"))?;
        let (done, _) = self
            .finished
            .1
            .wait_timeout_while(done, duration, |done| !*done)
            .map_err(|_| anyhow::anyhow!("offer completion wait failed"))?;
        if !*done {
            bail!("native offer worker did not finish within the deadline");
        }
        Ok(())
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct NetworkOptions {
    stun_provider: StunProvider,
    legacy_rsa_1024: bool,
    connection_settings: Option<Arc<dyn crate::rtc_configuration::NetworkSettings>>,
}

fn worker(
    shared: &Arc<Mutex<Completion>>,
    id: &str,
    commands: mpsc::Receiver<Command>,
    mut config: Option<crate::control::Config>,
    network: NetworkOptions,
) -> Result<()> {
    let NetworkOptions {
        stun_provider,
        legacy_rsa_1024,
        connection_settings,
    } = network;
    if let Some(config) = &mut config {
        let chroma_444_requested = config.chroma_444;
        // Capability work stays on this worker, outside the connection lock.
        #[cfg(windows)]
        {
            let gpu = connection_settings.as_ref().and_then(|m| m.video_gpu());
            config.allow_hevc =
                config.allow_hevc && crate::video_windows::hevc_available(gpu.as_ref());
            config.chroma_444 = config.allow_hevc
                && config.chroma_444
                && crate::video_windows::hevc_444_available(gpu.as_ref(), config.ten_bit);
            config.ten_bit = config.allow_hevc
                && config.ten_bit
                && crate::video_windows::hevc_ten_bit_available(gpu.as_ref());
        }
        #[cfg(not(any(windows, target_os = "android")))]
        {
            config.allow_hevc = false;
        }
        let inspector = shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .video_stream
            .clone();
        inspector
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .enable_hevc(config.allow_hevc);
        crate::event_log::record(
            "INFO",
            "video_capabilities_declared",
            serde_json::json!({"transport":"WebRTC","flags":config.video_flags(),"hevc":config.allow_hevc,"main10":config.ten_bit,"chroma_444":config.chroma_444,"chroma_444_requested":chroma_444_requested}),
        );
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let mut settings = SettingEngine::default();
    settings.set_network_types(vec![NetworkType::Udp4]);
    settings.set_data_channel_only(true);
    settings.set_prefer_turn_udp(true);
    settings.detach_data_channels();
    settings.set_sctp_max_message_size_can_send(
        webrtc::api::setting_engine::SctpMaxMessageSize::Bounded(MAX_CHANNEL_MESSAGE as u32),
    );
    // Explicit legacy key-size compatibility only. Signature and SDP fingerprint
    // verification remain enabled; insecure hashes are not enabled.
    settings.allow_insecure_verification_algorithm(legacy_rsa_1024);
    // Resolve short-lived credentials on the async worker, never the UI
    // thread. Snapshot settings once; a save affects only the next attempt.
    let mut ice = ice_configuration(stun_provider);
    if let Some(manager) = connection_settings {
        settings.set_turn_tcp_qos(manager.tcp_qos());
        ice = runtime.block_on(async {
            tokio::select! {
                result = manager.resolve() => result,
                _ = async {
                    loop {
                        if shared.lock().unwrap_or_else(|e| e.into_inner()).cancelled { break; }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                } => bail!("Connection cancelled during credential resolution"),
            }
        })?;
    }
    let relay_only = ice.ice_transport_policy
        == webrtc::peer_connection::policy::ice_transport_policy::RTCIceTransportPolicy::Relay;
    if relay_only {
        // Parsec still sees real host/srflx discovery. A separate ICE routing
        // policy rejects non-relay pairs and inbound data from the outset.
        ice.ice_transport_policy =
            webrtc::peer_connection::policy::ice_transport_policy::RTCIceTransportPolicy::All;
        settings.set_relay_only_connectivity(true);
    }
    let api = APIBuilder::new().with_setting_engine(settings).build();
    #[cfg(test)]
    {
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        state.progress.resolved_ice_servers = Some(ice.ice_servers.len());
        state.progress.relay_only = ice.ice_transport_policy
            == webrtc::peer_connection::policy::ice_transport_policy::RTCIceTransportPolicy::Relay;
        state.progress.relay_only_connectivity = relay_only;
    }

    let peer = Arc::new(runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), api.new_peer_connection(ice)).await
    })??);
    let callback_state = shared.clone();
    #[cfg(windows)]
    {
        let state = shared.clone();
        let weak = Arc::downgrade(&peer);
        runtime.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let link = {
                    let s = state.lock().unwrap_or_else(|e| e.into_inner());
                    if s.cancelled || s.closing {
                        break;
                    }
                    s.telemetry.clone()
                };
                let Some((bus, generation)) = link else {
                    continue;
                };
                let Some(peer) = weak.upgrade() else {
                    break;
                };
                let Ok(mut sample) =
                    tokio::time::timeout(Duration::from_millis(750), crate::stats::network(&peer))
                        .await
                else {
                    continue;
                };
                {
                    let s = state.lock().unwrap_or_else(|e| e.into_inner());
                    if s.cancelled || s.closing {
                        break;
                    }
                }
                // Reuse the existing 1 Hz statistics task.
                if !bus.requested() {
                    continue;
                }
                let (video, inspector) = {
                    let s = state.lock().unwrap_or_else(|e| e.into_inner());
                    if s.cancelled || s.closing {
                        break;
                    }
                    sample.audio_bytes = s.progress.channel_bytes_received[2];
                    sample.audio_ready = s.audio_stream.is_some();
                    if let Some(audio) = &s.audio_stream {
                        let report = audio.snapshot();
                        sample.audio_codec = report.codec;
                        sample.audio_layout = report.layout;
                        sample.audio_decoded_queue_ms = Some(report.queued_frames as f64 / 48.0);
                    }
                    (s.video_output.clone(), s.video_stream.clone())
                };
                let framing = inspector
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .snapshot();
                sample.profile = framing.sps_profile_idc;
                sample.codec = framing.codec;
                sample.hevc_advertised = framing.hevc_advertised;
                sample.video = video.map(|v| v.snapshot());
                bus.publish(generation, sample);
            }
        });
    }
    let attempt_id = id.to_owned();
    peer.on_ice_candidate(Box::new(move |candidate| {
        let shared=callback_state.clone(); let id=attempt_id.clone();
        Box::pin(async move {
            let Some(candidate)=candidate else {
                let mut state=shared.lock().unwrap_or_else(|e|e.into_inner());
                state.progress.local_gathering_complete=true;
                if relay_only { state.relay_readiness_sync(&id); }
                return;
            };
            if candidate.protocol != RTCIceProtocol::Udp || candidate.component != 1 { return; }
            let mut state=shared.lock().unwrap_or_else(|e|e.into_inner());
            if state.cancelled { return; }
            if state.events.len() >= 64 || !matches!(candidate.typ,RTCIceCandidateType::Host|RTCIceCandidateType::Srflx|RTCIceCandidateType::Relay) { state.progress.failed=true; }
            else {
                state.progress.local_candidates+=1;
                if candidate.typ==RTCIceCandidateType::Host {state.progress.local_host_candidates+=1;} else if candidate.typ==RTCIceCandidateType::Srflx {state.progress.local_srflx_candidates+=1;}
                state.events.push_back(serde_json::json!({"type":8,"attemptID":id,"ip":candidate.address,"port":candidate.port,"lan":candidate.typ==RTCIceCandidateType::Host,"fromStun":candidate.typ==RTCIceCandidateType::Srflx,"sync":false}));
                if relay_only && candidate.typ==RTCIceCandidateType::Relay { state.relay_readiness_sync(&id); }
            }
        })
    }));
    let callback_state = shared.clone();
    // A weak reference avoids a peer -> callback -> peer ownership cycle.
    let callback_peer = Arc::downgrade(&peer);
    peer.on_peer_connection_state_change(Box::new(move |connection_state| {
        let shared = callback_state.clone();
        let peer = callback_peer.upgrade();
        Box::pin(async move {
            let failure = if connection_state == RTCPeerConnectionState::Failed {
                peer.as_ref().map(|peer| {
                    let mut states = transport_states(peer);
                    states["peer"] = serde_json::json!(connection_state.to_string());
                    let stage = transport_failure_stage(
                        peer.ice_connection_state(),
                        peer.dtls_transport().state(),
                    );
                    (states, stage)
                })
            } else {
                None
            };
            let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
            if !state.cancelled {
                state.progress.transport_connected =
                    connection_state == RTCPeerConnectionState::Connected;
                if connection_state == RTCPeerConnectionState::Failed {
                    if let Some((states, stage)) = failure {
                        state.progress.transport_states = states.clone();
                        state.progress.transport_states_at_failure = states;
                        state.progress.failure_stage.get_or_insert(stage);
                    }
                    state.progress.failed = true;
                }
            }
        })
    }));
    let prepared = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut channels = Vec::new();
            let control_attempt_id=id.to_owned();
            for (id, label) in CHANNELS {
                let channel = peer
                    .create_data_channel(
                        label,
                        Some(RTCDataChannelInit {
                            negotiated: Some(id),
                            ordered: Some(true),
                            ..Default::default()
                        }),
                    )
                    .await?;
                let callback_state = shared.clone();
                let startup_config = if id == 0 { config.clone() } else { None };
                let weak_channel = Arc::downgrade(&channel);
                let attempt_id = control_attempt_id.clone();
                channel.on_open(Box::new(move || {
                    let shared = callback_state.clone();
                    Box::pin(async move {
                        if shared.lock().unwrap_or_else(|e| e.into_inner()).cancelled { return; }
                        let Some(channel) = weak_channel.upgrade() else { return; };
                        let detached = match channel.detach().await {
                            Ok(detached) => detached,
                            Err(_) => {
                                shared.lock().unwrap_or_else(|e| e.into_inner()).fail_channel(FailureStage::DataChannelDetach, id);
                                return;
                            }
                        };
                        tokio::spawn(read_channel(shared.clone(), id, detached));
                        if let Some(config) = startup_config {
                            let sent = async {
                                tokio::time::timeout(Duration::from_secs(5), channel.send(&config.startup()?)).await??;
                                Ok::<(), anyhow::Error>(())
                            }.await;
                            let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                            if state.cancelled || state.closing || state.progress.failed { return; }
                            if sent.is_err() || state.events.len() >= 64 {
                                state.fail_channel(FailureStage::ControlStartup, id);
                            } else {
                                state.progress.control_ready = true;
                                state.progress.last_send_elapsed_ms = Some(state.elapsed_ms());
                                state.progress.last_sent_control_kind = Some(11);
                                state.events.push_back(serde_json::json!({"type":7,"status":0,"state":4,"attemptID":attempt_id,"duration":0}));
                            }
                        }
                        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                        if !state.cancelled && !state.closing && !state.progress.failed {
                            state.progress.open_mask |= 1 << id;
                        }
                    })
                }));
                channels.push(channel);
            }
            let offer = peer.create_offer(None).await?;
            let description = Description::from_sdp(&offer.sdp)?;
            Ok::<_, anyhow::Error>((offer, description, channels))
        })
        .await
    });
    let outcome = (|| -> Result<()> {
        let (offer, description, channels) = prepared??;
        let mut offer = Some(offer);
        let mut gate = CandidateGate::new(id)?;
        if relay_only {
            gate.allow_relay_trickle();
        }
        let mut sync_ack_scheduled = false;
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(output) = state.output.take() {
            state.progress.ready = output.finish(Some(&description))?;
            state.progress.failed = !state.progress.ready;
            state.progress.channels = channels.len();
            state.progress.mid = description.mid.clone();
        }
        let ready = state.progress.ready;
        let cancelled = state.cancelled;
        drop(state);
        if cancelled {
            return Ok(());
        }
        if !ready {
            bail!("native offer publication failed or was cancelled");
        }
        #[cfg(windows)]
        let timing_task = runtime.spawn(collect_host_timing(shared.clone(), channels[0].clone()));
        let mut deadline = EstablishmentDeadline::new(std::time::Instant::now());
        #[cfg(windows)]
        let mut next_video_recovery_check = std::time::Instant::now();
        loop {
            // Read only documented native state enums; never serialize SDP,
            // addresses, certificates or underlying error strings.
            let wait = {
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                state.progress.transport_states = transport_states(&peer);
                state.progress.connection_established |=
                    state.progress.transport_connected && state.progress.open_mask == 7;
                deadline.poll_wait(
                    std::time::Instant::now(),
                    state.progress.connection_established,
                )?
            };
            #[cfg(windows)]
            if std::time::Instant::now() >= next_video_recovery_check {
                next_video_recovery_check = std::time::Instant::now() + Duration::from_millis(50);
                let video = {
                    let state = shared.lock().unwrap_or_else(|e| e.into_inner());
                    if state.cancelled || state.closing {
                        break;
                    }
                    state.video_output.clone()
                };
                if let (Some(video), Some(config)) = (video, &mut config) {
                    if let Some((generation, fallback_420)) = video.take_keyframe_request() {
                        if fallback_420 {
                            config.chroma_444 = false;
                        }
                        // A failed refresh disables video recovery, not audio or
                        // control. No change to SCTP reliability or ordering.
                        let channel = channels[0].clone();
                        let request = config.keyframe_request();
                        runtime.spawn(async move {
                            let sent = tokio::time::timeout(
                                Duration::from_secs(1),
                                channel.send(&request),
                            )
                            .await;
                            if !matches!(sent, Ok(Ok(_))) {
                                video.recovery_request_failed(generation);
                            }
                        });
                    }
                }
            }
            let command = match commands.recv_timeout(wait) {
                Ok(command) => command,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
            };
            if shared.lock().unwrap_or_else(|e| e.into_inner()).cancelled {
                break;
            }
            if matches!(command, Command::Disconnect) {
                #[cfg(windows)]
                {
                    timing_task.abort();
                    runtime.block_on(stop_host_timing(shared, &channels[0]));
                }
                let _ = runtime.block_on(send_disconnect(&channels[0]));
                break;
            }
            runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(5), async {
                    match command {
                        Command::Begin(remote) => {
                            let local = offer.take().context("remote begin already supplied")?;
                            peer.set_local_description(local)
                                .await
                                .context(FailureStage::SetLocalDescription)?;
                            shared
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .progress
                                .local_description_set = true;
                            peer.set_remote_description(RTCSessionDescription::answer(
                                description.answer(&remote)?,
                            )?)
                            .await
                            .context(FailureStage::SetRemoteDescription)?;
                            shared
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .progress
                                .remote_description_set = true;
                            gate.remote_ready(id, &description.mid, &remote.ufrag)
                                .context(FailureStage::CandidateGate)?;
                            if relay_only {
                                shared
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .relay_readiness_sync(id);
                            }
                        }
                        Command::Candidate(candidate) => gate
                            .push(id, candidate)
                            .context(FailureStage::CandidateGate)?,
                        Command::Sync => {
                            gate.sync(id)?;
                            shared
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .progress
                                .sync_received = true;
                            if !relay_only
                                && !sync_ack_scheduled
                                && !shared
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .progress
                                    .local_sync
                                    .queued()
                            {
                                sync_ack_scheduled = true;
                                let shared = shared.clone();
                                let id = id.to_owned();
                                tokio::spawn(async move {
                                    // ka() in the pinned JS returns this marker after 500 ms.
                                    tokio::time::sleep(Duration::from_millis(500)).await;
                                    let mut state =
                                        shared.lock().unwrap_or_else(|e| e.into_inner());
                                    if !state.cancelled && !state.closing {
                                        let marker = state.progress.local_sync.acknowledge(&id);
                                        state.queue_local_sync(marker);
                                    }
                                });
                            }
                        }
                        Command::Send(id, payload) => {
                            let payload = if id == 0 {
                                config.as_ref().map_or_else(
                                    || payload.clone(),
                                    |c| c.encoder_request(payload.clone()),
                                )
                            } else {
                                payload
                            };
                            let channel = &channels[usize::from(id)];
                            if channel.ready_state() != RTCDataChannelState::Open {
                                return Err(FailureStage::ChannelSend.into());
                            }
                            channel
                                .send(&payload)
                                .await
                                .context(FailureStage::ChannelSend)?;
                            let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                            state.progress.last_send_elapsed_ms = Some(state.elapsed_ms());
                            state.progress.last_sent_control_kind = if id == 0 {
                                payload.get(12).copied()
                            } else {
                                None
                            };
                        }
                        Command::Disconnect => unreachable!("handled before command dispatch"),
                    }
                    while let Some(candidate) = gate.pop_ready() {
                        peer.add_ice_candidate(candidate)
                            .await
                            .context(FailureStage::AddIceCandidate)?;
                        shared
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .progress
                            .remote_candidates += 1;
                    }
                    Ok::<(), anyhow::Error>(())
                })
                .await
            })??;
        }
        #[cfg(windows)]
        {
            timing_task.abort();
            runtime.block_on(async {
                let _ = timing_task.await;
                stop_host_timing(shared, &channels[0]).await;
            });
        }
        drop((offer, channels));
        Ok(())
    })();

    {
        let states = transport_states(&peer);
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        state.progress.transport_states = states.clone();
        if let Err(error) = &outcome {
            let stage = error
                .downcast_ref::<FailureStage>()
                .copied()
                .unwrap_or(FailureStage::Worker);
            state.progress.failure_stage.get_or_insert(stage);
            #[cfg(windows)]
            let logged_route = state
                .telemetry
                .as_ref()
                .and_then(|(bus, _)| bus.read())
                .map(|s| s.route);
            #[cfg(not(windows))]
            let logged_route: Option<&str> = None;
            if !state.cancelled && !matches!(stage, FailureStage::Cancelled) {
                crate::event_log::record(
                    "ERROR",
                    "connection_failed",
                    serde_json::json!({"transport":"WebRTC","stage":stage.to_string(),"route":logged_route}),
                );
            }
            if state.progress.failure_elapsed_ms.is_none() {
                state.progress.failure_elapsed_ms = Some(state.elapsed_ms());
            }
        }
        state.progress.transport_states_before_close = states;
        state.closing = true;
        #[cfg(windows)]
        if let Some((bus, generation)) = &state.telemetry {
            bus.finish(*generation);
        }
    }
    let closed = runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(2), peer.close()).await });
    let is_closed =
        matches!(closed, Ok(Ok(()))) && peer.connection_state() == RTCPeerConnectionState::Closed;
    {
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        state.progress.closed = is_closed;
        // Publish before dropping the command receiver: a producer observing
        // a closed queue must still be able to retain the worker's cause.
        if let Err(error) = &outcome {
            state.progress.failed = true;
            state.progress.failure_stage.get_or_insert(
                error
                    .downcast_ref::<FailureStage>()
                    .copied()
                    .unwrap_or(FailureStage::Worker),
            );
        }
        if is_closed {
            state.progress.transport_connected = false;
            state.progress.open_mask = 0;
            state.progress.control_ready = false;
        }
    }
    if !is_closed {
        bail!("native offer peer did not close within the deadline");
    }
    outcome
}

#[cfg(windows)]
async fn stop_host_timing(
    state: &Mutex<Completion>,
    channel: &webrtc::data_channel::RTCDataChannel,
) {
    let (link, active) = {
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        (
            state.telemetry.clone(),
            std::mem::take(&mut state.timing_active),
        )
    };
    if active {
        let _ = tokio::time::timeout(
            Duration::from_millis(250),
            channel.send(&Bytes::copy_from_slice(&crate::frame_timing::request(
                false,
            ))),
        )
        .await;
    }
    if let Some((bus, generation)) = link {
        bus.host_timing.end(generation);
    }
}

#[cfg(windows)]
async fn collect_host_timing(
    state: Arc<Mutex<Completion>>,
    channel: Arc<webrtc::data_channel::RTCDataChannel>,
) {
    let mut requests = crate::frame_timing::Requests::default();
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let link = {
            let s = state.lock().unwrap_or_else(|e| e.into_inner());
            if s.cancelled || s.closing || s.progress.failed {
                break;
            }
            if !s.progress.control_ready {
                continue;
            }
            s.telemetry.clone()
        };
        let Some((bus, generation)) = link else {
            continue;
        };
        if !bus.host_timing.current(generation) {
            break;
        }
        let desired = bus.requested();
        let Some(payload) = requests.update(desired) else {
            continue;
        };
        if desired {
            state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .timing_active = true;
        }
        bus.host_timing.set_requested(generation, desired);
        if !matches!(
            tokio::time::timeout(
                Duration::from_secs(1),
                channel.send(&Bytes::copy_from_slice(&payload))
            )
            .await,
            Ok(Ok(_))
        ) {
            bus.host_timing.failed(generation);
            crate::event_log::record(
                "WARN",
                "host_timing_request_failed",
                serde_json::json!({"transport":"WebRTC"}),
            );
            break;
        }
        if !desired {
            state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .timing_active = false;
        }
    }
}

pub(crate) async fn send_disconnect(channel: &webrtc::data_channel::RTCDataChannel) -> Result<()> {
    if channel.ready_state() != RTCDataChannelState::Open {
        return Ok(());
    }
    // Z() in the audited parsec.js sends P(10,0,0,0) on control before close.
    // This private Parsec frame travels over the normal DTLS/SCTP channel.
    tokio::time::timeout(Duration::from_millis(500), async {
        channel.send(&crate::control::header(10, 0, 0, 0)).await?;
        while channel.buffered_amount().await != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await??;
    Ok(())
}

// This timer bounds initial ICE/DTLS/SCTP establishment, never session lifetime.
// Once established, a transient disconnect must not resurrect the old timer.
struct EstablishmentDeadline(Option<std::time::Instant>);

impl EstablishmentDeadline {
    fn new(now: std::time::Instant) -> Self {
        Self(Some(now + Duration::from_secs(30)))
    }

    fn poll_wait(&mut self, now: std::time::Instant, established: bool) -> Result<Duration> {
        if established {
            self.0 = None;
        }
        let poll = Duration::from_millis(250);
        match self.0 {
            Some(deadline) if now >= deadline => Err(FailureStage::Deadline.into()),
            Some(deadline) => Ok(poll.min(deadline.saturating_duration_since(now))),
            None => Ok(poll),
        }
    }
}

fn transport_states(peer: &webrtc::peer_connection::RTCPeerConnection) -> serde_json::Value {
    serde_json::json!({
        "peer":peer.connection_state().to_string(),
        "ice":peer.ice_connection_state().to_string(),
        "gathering":peer.ice_gathering_state().to_string(),
        "signaling":peer.signaling_state().to_string(),
        "dtls":peer.dtls_transport().state().to_string()
    })
}

fn transport_failure_stage(
    ice: webrtc::ice_transport::ice_connection_state::RTCIceConnectionState,
    dtls: webrtc::dtls_transport::dtls_transport_state::RTCDtlsTransportState,
) -> FailureStage {
    use webrtc::dtls_transport::dtls_transport_state::RTCDtlsTransportState;
    use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
    if dtls == RTCDtlsTransportState::Failed {
        FailureStage::DtlsTransport
    } else if ice == RTCIceConnectionState::Failed {
        FailureStage::IceTransport
    } else {
        FailureStage::PeerTransport
    }
}

fn ice_configuration(stun_provider: StunProvider) -> RTCConfiguration {
    RTCConfiguration {
        ice_transport_policy:
            webrtc::peer_connection::policy::ice_transport_policy::RTCIceTransportPolicy::All,
        ice_servers: stun_provider
            .server()
            .map(|(_, url)| RTCIceServer {
                urls: vec![url.into()],
                ..Default::default()
            })
            .into_iter()
            .collect(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn establishment_timeout_still_bounds_an_unconnected_attempt() {
        let start = std::time::Instant::now();
        let mut deadline = super::EstablishmentDeadline::new(start);
        assert_eq!(
            deadline.poll_wait(start, false).unwrap(),
            std::time::Duration::from_millis(250)
        );
        assert_eq!(
            deadline
                .poll_wait(start + std::time::Duration::from_millis(29999), false)
                .unwrap(),
            std::time::Duration::from_millis(1)
        );
        let error = deadline
            .poll_wait(start + std::time::Duration::from_secs(30), false)
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<super::FailureStage>(),
            Some(super::FailureStage::Deadline)
        ));
    }

    #[test]
    fn established_session_has_no_lifetime_timeout_or_busy_polling() {
        let start = std::time::Instant::now();
        let mut deadline = super::EstablishmentDeadline::new(start);
        deadline
            .poll_wait(start + std::time::Duration::from_secs(5), true)
            .unwrap();
        // A later transient disconnect must not re-arm the initial deadline.
        for seconds in [30, 60, 3600, 86400] {
            assert_eq!(
                deadline
                    .poll_wait(start + std::time::Duration::from_secs(seconds), false)
                    .unwrap(),
                std::time::Duration::from_millis(250)
            );
        }
    }

    #[test]
    fn failure_stage_uses_current_transport_states() {
        use webrtc::dtls_transport::dtls_transport_state::RTCDtlsTransportState as D;
        use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState as I;
        assert!(matches!(
            super::transport_failure_stage(I::Connected, D::Failed),
            super::FailureStage::DtlsTransport
        ));
        assert!(matches!(
            super::transport_failure_stage(I::Failed, D::Connecting),
            super::FailureStage::IceTransport
        ));
        assert!(matches!(
            super::transport_failure_stage(I::Connected, D::Connecting),
            super::FailureStage::PeerTransport
        ));
    }
    #[test]
    fn cloudflare_stun_is_opt_in_without_turn_or_credentials() {
        assert!(super::ice_configuration(StunProvider::None)
            .ice_servers
            .is_empty());
        let config = super::ice_configuration(StunProvider::Cloudflare);
        assert_eq!(config.ice_servers.len(), 1);
        assert_eq!(
            config.ice_servers[0].urls,
            vec!["stun:stun.cloudflare.com:3478"]
        );
        let default_client = super::ice_configuration(StunProvider::Parsec);
        assert_eq!(
            default_client.ice_servers[0].urls,
            vec!["stun:stun.parsec.gg:3478"]
        );
        assert!(default_client.ice_servers[0].username.is_empty());
        assert!(default_client.ice_servers[0].credential.is_empty());
        assert!(config.ice_servers[0].username.is_empty());
        assert!(config.ice_servers[0].credential.is_empty());
        assert_eq!(
            config.ice_transport_policy,
            webrtc::peer_connection::policy::ice_transport_policy::RTCIceTransportPolicy::All
        );
    }
    use super::*;

    #[test]
    fn relay_sync_remote_first_waits_for_relay_and_later_candidates_stay_after_marker() {
        let mut state = empty_completion();
        state.progress.remote_description_set = true;
        state.progress.local_candidates = 2;
        state.progress.local_host_candidates = 1;
        state.progress.local_srflx_candidates = 1;
        state.relay_readiness_sync("fixture");
        assert!(state.events.is_empty());
        state.progress.local_candidates += 1;
        state.events.push_back(serde_json::json!({"sync":false}));
        state.relay_readiness_sync("fixture");
        assert!(!state.progress.local_gathering_complete);
        assert_eq!(state.events.len(), 2);
        state.progress.local_candidates += 1;
        state.events.push_back(serde_json::json!({"sync":false}));
        state.relay_readiness_sync("fixture");
        assert_eq!(state.events.len(), 3);
        assert_eq!(state.events[1]["sync"], true);
        assert_eq!(state.events[2]["sync"], false);
    }

    #[test]
    fn relay_sync_precedes_gathering_completion_is_bounded_and_retired() {
        let mut state = empty_completion();
        state.progress.local_candidates = 1;
        state.events.push_back(serde_json::json!({"sync":false}));
        state.relay_readiness_sync("fixture");
        assert_eq!(state.events.len(), 1);
        assert!(!state.progress.local_gathering_complete);
        state.progress.remote_description_set = true;
        state.relay_readiness_sync("fixture");
        assert_eq!(state.events.len(), 2);
        assert!(state.progress.local_sync_queued);
        assert_eq!(state.events.front().unwrap()["sync"], false);
        assert_eq!(state.events.back().unwrap()["sync"], true);
        state.progress.local_gathering_complete = true;
        state.relay_readiness_sync("fixture");
        let ack = state.progress.local_sync.acknowledge("fixture");
        state.queue_local_sync(ack);
        assert_eq!(state.events.len(), 2);
        for cancelled in [false, true] {
            let mut retired = empty_completion();
            retired.cancelled = cancelled;
            retired.closing = !cancelled;
            retired.progress.local_candidates = 1;
            retired.progress.local_gathering_complete = true;
            retired.progress.remote_description_set = true;
            retired.relay_readiness_sync("retired");
            assert!(retired.events.is_empty());
        }
        let mut full = empty_completion();
        full.events = (0..64).map(|_| serde_json::Value::Null).collect();
        full.progress.local_candidates = 1;
        full.progress.local_gathering_complete = true;
        full.progress.remote_description_set = true;
        full.relay_readiness_sync("full");
        assert!(full.progress.failed);
        assert!(!full.progress.local_sync_queued);
        assert_eq!(full.events.len(), 64);
    }

    #[test]
    fn network_diagnostics_keep_first_failure_states_and_exclude_control_payloads() {
        let mut state = empty_completion();
        state.progress.transport_states = serde_json::json!({"ice":"connected","dtls":"connected"});
        state.progress.channel_bytes_received = [10, 1_000_000, 50_000];
        state.progress.channel_max_message_bytes = [10, 900_000, 200];
        state.messages.push_back((
            0,
            false,
            Bytes::from_static(b"private-fixture-control-text"),
        ));
        state.fail_channel(FailureStage::DataChannelRead, 1);
        state.progress.read_error = Some("message-exceeds-read-buffer");
        state.progress.transport_states = serde_json::json!({"ice":"closed","dtls":"closed"});
        state.fail_channel(FailureStage::DataChannelClosed, 2);
        let report = state.network_diagnostics();
        assert_eq!(report["failure_stage"], "data-channel-read");
        assert_eq!(report["failure_channel"], 1);
        assert_eq!(report["states"]["ice"], "connected");
        assert_eq!(report["max_message_bytes"][1], 900_000);
        assert_eq!(report["queued_control_messages"], 1);
        assert!(!report.to_string().contains("private-fixture-control-text"));
    }

    #[test]
    fn channel_read_categories_do_not_expose_library_error_strings() {
        use webrtc::{data::Error as D, sctp::Error as S};
        assert_eq!(
            classify_channel_read_error(&D::Sctp(S::ErrShortBuffer { size: 1_048_576 })),
            "message-exceeds-read-buffer"
        );
        assert_eq!(
            classify_channel_read_error(&D::Sctp(S::ErrNetConnReadError)),
            "sctp-network-read"
        );
        assert_eq!(classify_channel_read_error(&D::Sctp(S::ErrEof)), "sctp-eof");
        assert_eq!(
            classify_channel_read_error(&D::new("private-fixture-error-text".into())),
            "datachannel-error"
        );
    }

    fn empty_completion() -> Completion {
        Completion {
            #[cfg(windows)]
            telemetry: None,
            #[cfg(windows)]
            timing_active: false,
            started_at: std::time::Instant::now(),
            closing: false,
            output: None,
            progress: Progress::default(),
            cancelled: false,
            events: VecDeque::new(),
            messages: VecDeque::new(),
            message_bytes: 0,
            discard_unavailable_media: false,
            media_ingress: Default::default(),
            video_stream: Default::default(),
            #[cfg(any(windows, target_os = "android"))]
            video_output: None,
            audio_stream: None,
        }
    }

    #[cfg(windows)]
    #[test]
    fn host_timing_bursts_bypass_the_control_queue_without_changing_normal_delivery() {
        let bus = Arc::new(crate::stats::Shared::default());
        let generation = bus.begin();
        bus.host_timing.set_requested(generation, true);
        let mut state = empty_completion();
        state.telemetry = Some((bus.clone(), generation));
        state.discard_unavailable_media = true;
        let frame = crate::frame_timing::tests::packet(0, 0, &[12., 2., 16., 4.]);
        for _ in 0..10_000 {
            assert!(state.receive(0, false, &frame));
        }
        assert!(!state.progress.failed);
        assert!(state.messages.is_empty());
        assert_eq!(state.message_bytes, 0);
        assert_eq!(
            bus.host_timing.snapshot(generation).video[0]
                .as_ref()
                .unwrap()
                .count,
            10_000
        );
        assert!(state.receive(0, false, &[0; 13]));
        assert_eq!(state.messages.len(), 1);
        // Synthetic transport probes still retain arbitrary control receipts.
        let mut probe = empty_completion();
        assert!(probe.receive(0, false, &frame));
        assert_eq!(probe.messages.len(), 1);
    }

    #[test]
    fn captured_audio_delivery_is_safe_after_cancellation_without_holding_connection_lock() {
        let mut state = empty_completion();
        state.discard_unavailable_media = true;
        state.audio_stream = Some(crate::audio_stream::Pipeline::start());
        let delivery = state
            .prepare_receive(2, false, &[0xf8, 0xff, 0xfe])
            .unwrap();
        let mut audio = state.audio_stream.take().unwrap();
        state.cancelled = true;
        audio.stop();
        assert_eq!(delivery.submit(&[0xf8, 0xff, 0xfe]), Some(true));
        assert!(audio.snapshot().worker_finished);
        assert_eq!(audio.snapshot().queued_samples, 0);
    }

    #[test]
    fn receipt_queue_takes_ownership_and_rejects_retired_deliveries() {
        let mut state = empty_completion();
        let payload = Bytes::from(vec![0x5a; MAX_CHANNEL_MESSAGE]);
        let pointer = payload.as_ptr();
        assert!(state.queue_received(0, false, payload));
        assert_eq!(state.messages.front().unwrap().2.as_ptr(), pointer);
        assert_eq!(state.message_bytes, MAX_CHANNEL_MESSAGE);
        for closing in [false, true] {
            let mut retired = empty_completion();
            assert!(matches!(
                retired.prepare_receive(0, false, b"control"),
                Some(Delivery::Queued)
            ));
            retired.closing = closing;
            retired.cancelled = !closing;
            assert!(!retired.queue_received(0, false, Bytes::from_static(b"control")));
            assert!(retired.messages.is_empty());
            assert_eq!(retired.message_bytes, 0);
        }
    }

    #[test]
    fn prepared_receipts_still_enforce_the_shared_queue_budget() {
        let mut state = empty_completion();
        for _ in 0..2 {
            assert!(matches!(
                state.prepare_receive(0, false, b"control"),
                Some(Delivery::Queued)
            ));
        }
        for _ in 0..16 {
            assert!(state.queue_received(0, false, Bytes::from_static(b"control")));
        }
        assert!(!state.queue_received(0, false, Bytes::from_static(b"overflow")));
        assert_eq!(state.messages.len(), 16);
        assert_eq!(state.message_bytes, 16 * 7);
        assert!(matches!(
            state.progress.failure_stage,
            Some(FailureStage::InboundQueueFull)
        ));
    }

    #[test]
    fn complete_large_messages_are_bounded_and_counted_before_queue_overflow() {
        let mut state = empty_completion();
        let payload = vec![0x5a; MAX_CHANNEL_MESSAGE];
        for _ in 0..4 {
            assert!(state.receive(1, false, &payload));
        }
        assert_eq!(state.message_bytes, 4 * MAX_CHANNEL_MESSAGE);
        assert_eq!(state.messages[0].2.as_ref(), payload.as_slice());
        assert!(!state.receive(1, false, &payload));
        assert_eq!(state.messages.len(), 4);
        assert_eq!(state.progress.channel_messages_received, [0, 5, 0]);
        assert_eq!(
            state.progress.channel_max_message_bytes[1],
            MAX_CHANNEL_MESSAGE
        );
        assert!(matches!(
            state.progress.failure_stage,
            Some(FailureStage::InboundQueueFull)
        ));
        assert_eq!(state.progress.failure_channel, Some(1));
        assert!(state.progress.failure_elapsed_ms.is_some());
    }

    #[test]
    fn media_burst_without_ui_polling_preserves_control_and_constant_queue_memory() {
        let mut state = empty_completion();
        state.discard_unavailable_media = true;
        assert!(state.receive(0, false, b"control"));
        let payload = vec![0x5a; MAX_CHANNEL_MESSAGE];
        for _ in 0..128 {
            assert!(state.receive(1, false, &payload));
            assert!(state.receive(2, false, b"audio"));
        }
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.message_bytes, 7);
        assert_eq!(state.messages[0].2.as_ref(), b"control");
        assert_eq!(state.media_ingress.video_packets_received, 128);
        assert_eq!(
            state.media_ingress.video_bytes_received,
            128 * MAX_CHANNEL_MESSAGE as u64
        );
        assert_eq!(state.media_ingress.audio_packets_received, 128);
        assert_eq!(
            state.media_ingress.packets_discarded_decoder_unavailable,
            256
        );
        assert!(!state.progress.failed);
        for _ in 0..15 {
            assert!(state.receive(0, false, b"control"));
        }
        assert!(!state.receive(0, false, b"overflow"));
        assert!(matches!(
            state.progress.failure_stage,
            Some(FailureStage::InboundQueueFull)
        ));
        assert_eq!(state.progress.failure_channel, Some(0));
    }

    #[test]
    fn invalid_or_oversized_channel_messages_never_enter_queue() {
        for (channel, text, size) in [
            (1, false, MAX_CHANNEL_MESSAGE + 1),
            (1, true, 16),
            (99, false, 1),
        ] {
            let mut state = empty_completion();
            assert!(!state.receive(channel, text, &vec![0; size]));
            assert!(state.messages.is_empty());
            assert_eq!(state.message_bytes, 0);
            assert!(matches!(
                state.progress.failure_stage,
                Some(FailureStage::InboundChannel)
            ));
            assert_eq!(state.progress.failure_channel, Some(channel));
        }
    }

    #[test]
    fn empty_native_media_messages_are_accounted_without_codec_submission_or_close() {
        let mut state = empty_completion();
        state.discard_unavailable_media = true;
        for channel in [0, 1, 2] {
            assert!(matches!(
                state.prepare_receive(channel, false, &[]),
                Some(Delivery::EmptyMessage)
            ));
        }
        assert!(!state.progress.failed);
        assert_eq!(state.progress.empty_channel_messages, [1, 1, 1]);
        assert!(state.messages.is_empty());
        assert!(state.receive(1, false, b"nonempty-media-after-empty"));
        assert!(!state.progress.failed);
        assert_eq!(state.progress.channel_messages_received, [1, 2, 1]);
    }

    #[test]
    fn first_channel_failure_survives_later_errors_and_local_shutdown_is_not_failure() {
        let mut state = empty_completion();
        state.fail_channel(FailureStage::DataChannelRead, 1);
        let first = state.progress.failure_elapsed_ms;
        state.fail_channel(FailureStage::DataChannelClosed, 0);
        assert!(matches!(
            state.progress.failure_stage,
            Some(FailureStage::DataChannelRead)
        ));
        assert_eq!(state.progress.failure_channel, Some(1));
        assert_eq!(state.progress.failure_elapsed_ms, first);
        for cancelled in [true, false] {
            let mut state = empty_completion();
            state.cancelled = cancelled;
            state.closing = !cancelled;
            state.fail_channel(FailureStage::DataChannelRead, 1);
            assert!(!state.progress.failed);
            assert!(!state.receive(1, false, b"private-video-payload"));
            assert!(state.messages.is_empty());
        }
    }

    #[test]
    fn native_offer_completion_needs_no_account_connection() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let output = Output::new(Arc::new(move |description| {
            let Some(description) = description else {
                return Ok(false);
            };
            Ok(sender
                .try_send((
                    description.credentials.validate().is_ok(),
                    !description.mid.is_empty(),
                ))
                .is_ok())
        }));
        let mut attempt = Attempt::spawn_configured(
            "native-offer-fixture",
            output,
            None,
            StunProvider::None,
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(12)).unwrap(),
            (true, true)
        );
        assert!(attempt.snapshot()["offer_ready"].as_bool().unwrap());
        assert_eq!(attempt.snapshot()["negotiated_channels"], 3);
        attempt.cancel();
        attempt.wait_finished(Duration::from_secs(12)).unwrap();
    }
}
