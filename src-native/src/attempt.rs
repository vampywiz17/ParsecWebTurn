//! Managed native offer creation behind parsec_web_new_attempt. The guest's
//! private ABI is isolated; no account, signaling server or remote host is used.
use crate::{
    memory::GuestMemory,
    signaling::{Candidate, CandidateGate, Credentials, Description},
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
pub(crate) const MAX_CHANNEL_MESSAGE: usize = 1024 * 1024;
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

#[derive(Clone)]
pub struct Output {
    memory: GuestMemory,
    strings: [u32; 3],
    capacity: usize,
    sync: u32,
    error: u32,
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
    InboundControl,
    IceTransport,
    DtlsTransport,
    PeerTransport,
    SetLocalDescription,
    SetRemoteDescription,
    CandidateGate,
    AddIceCandidate,
    RemoteBegin,
    AttemptMismatch,
    RemoteCandidate,
    CandidateAddress,
    CandidatePort,
    CandidateEndpoint,
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

impl Output {
    pub fn new(
        memory: GuestMemory,
        strings: [u32; 3],
        capacity: usize,
        sync: u32,
        error: u32,
    ) -> Result<Self> {
        if !(2..=4096).contains(&capacity) {
            bail!("invalid attempt output capacity");
        }
        memory.sync_word(sync)?;
        let mut ranges = Vec::new();
        for ptr in strings {
            ranges.push(memory.range(ptr, capacity)?);
        }
        ranges.push(memory.range(sync, 4)?);
        ranges.push(memory.range(error, 4)?);
        for (i, a) in ranges.iter().enumerate() {
            for b in &ranges[i + 1..] {
                if a.start < b.end && b.start < a.end {
                    bail!("overlapping attempt output buffers");
                }
            }
        }
        Ok(Self {
            memory,
            strings,
            capacity,
            sync,
            error,
        })
    }
    pub fn finish(&self, credentials: Option<&Description>) -> Result<bool> {
        // Validate every size before writing any credential. Failure leaves the
        // buffers empty, so stale credentials cannot look like a valid offer.
        let values = credentials.map(|d| {
            [
                &d.credentials.ufrag,
                &d.credentials.password,
                &d.credentials.fingerprint,
            ]
        });
        let fits = values
            .as_ref()
            .is_some_and(|values| values.iter().all(|v| v.len() < self.capacity));
        for (i, ptr) in self.strings.iter().enumerate() {
            self.memory.c_string(
                *ptr,
                self.capacity,
                if fits {
                    values.as_ref().unwrap()[i]
                } else {
                    ""
                },
            )?;
        }
        self.memory.set_u32(self.error, u32::from(!fits))?;
        self.memory.signal(self.sync)?;
        Ok(fits)
    }
}

#[derive(Default)]
struct Progress {
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
    failure_channel: Option<u16>,
    failure_elapsed_ms: Option<u64>,
    last_send_elapsed_ms: Option<u64>,
    last_sent_control_kind: Option<u8>,
    control_ready: bool,
    local_description_set: bool,
    remote_description_set: bool,
    sync_received: bool,
    transport_states: serde_json::Value,
    transport_states_before_close: serde_json::Value,
    transport_states_at_failure: serde_json::Value,
    #[cfg(any(test, feature = "diagnostics"))]
    stun_provider: StunProvider,
    #[cfg(any(test, feature = "diagnostics"))]
    legacy_rsa_1024: bool,
}

struct Completion {
    #[cfg(windows)]
    telemetry: Option<(Arc<crate::stats::Shared>, u64)>,
    started_at: std::time::Instant,
    closing: bool,
    output: Option<Output>,
    progress: Progress,
    cancelled: bool,
    events: VecDeque<serde_json::Value>,
    messages: VecDeque<(u16, bool, Bytes)>,
    message_bytes: usize,
    // Session media has no decoder yet; raw transport probes retain receipts.
    discard_unavailable_media: bool,
    media_ingress: crate::media_ingress::Ingress,
    video_stream: crate::video_stream::Inspector,
    audio_stream: Option<crate::audio_stream::Pipeline>,
    #[cfg(windows)]
    video_output: Option<crate::video_windows::Pipeline>,
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
    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn fail_channel(&mut self, stage: FailureStage, channel: u16) {
        if self.cancelled || self.closing {
            return;
        }
        self.progress.failed = true;
        if self.progress.failure_stage.is_none() {
            self.progress.failure_stage = Some(stage);
            self.progress.failure_channel = Some(channel);
            self.progress.failure_elapsed_ms = Some(self.elapsed_ms());
        }
        if channel < 3 {
            self.progress.open_mask &= !(1 << channel);
        }
    }

    fn receive(&mut self, channel: u16, text: bool, bytes: &[u8]) -> bool {
        if self.cancelled || self.closing || self.progress.failed {
            return false;
        }
        if channel > 2 || text || bytes.len() > MAX_CHANNEL_MESSAGE {
            self.fail_channel(FailureStage::InboundChannel, channel);
            return false;
        }
        let i = usize::from(channel);
        self.progress.messages_received = self.progress.messages_received.saturating_add(1);
        self.progress.channel_messages_received[i] =
            self.progress.channel_messages_received[i].saturating_add(1);
        self.progress.channel_bytes_received[i] =
            self.progress.channel_bytes_received[i].saturating_add(bytes.len() as u64);
        self.progress.channel_max_message_bytes[i] =
            self.progress.channel_max_message_bytes[i].max(bytes.len());
        if channel != 0 && self.discard_unavailable_media {
            let mut discarded = true;
            if channel == 1 {
                let info = self.video_stream.receive(bytes);
                #[cfg(windows)]
                if let (Some(pipeline), Some(info)) = (&self.video_output, info) {
                    discarded = !pipeline.submit(bytes, info);
                }
                #[cfg(not(windows))]
                let _ = info;
            }
            if channel == 2 {
                if let Some(pipeline) = &self.audio_stream {
                    discarded = !pipeline.submit(bytes);
                }
            }
            self.media_ingress.record(channel, bytes.len(), discarded);
            return true;
        }
        if self.messages.len() >= 16 || self.message_bytes + bytes.len() > 4 * MAX_CHANNEL_MESSAGE {
            self.fail_channel(FailureStage::InboundQueueFull, channel);
            return false;
        }
        self.message_bytes += bytes.len();
        self.messages
            .push_back((channel, text, Bytes::copy_from_slice(bytes)));
        true
    }
}

// Public detached API preserves complete SCTP messages beyond the library's
// fixed 65,535-byte callback buffer. All three channels use the same API.
async fn read_channel(
    shared: Arc<Mutex<Completion>>,
    wake: Arc<Condvar>,
    id: u16,
    channel: Arc<webrtc::data::data_channel::DataChannel>,
) {
    let mut buffer = vec![0; MAX_CHANNEL_MESSAGE];
    loop {
        let read = channel.read_data_channel(&mut buffer).await;
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        let keep_reading = match read {
            Ok((0, _)) => {
                state.fail_channel(FailureStage::DataChannelClosed, id);
                false
            }
            Ok((len, text)) => state.receive(id, text, &buffer[..len]),
            Err(_) => {
                state.fail_channel(FailureStage::DataChannelRead, id);
                false
            }
        };
        wake.notify_all();
        if !keep_reading {
            break;
        }
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
    #[cfg(any(test, not(feature = "diagnostics")))]
    Parsec,
    #[cfg(any(test, feature = "diagnostics"))]
    Cloudflare,
}
impl StunProvider {
    fn server(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::None => None,
            // Audited constructor in the pinned public parsec.js (150-104a).
            #[cfg(any(test, not(feature = "diagnostics")))]
            Self::Parsec => Some(("parsec", "stun:stun.parsec.gg:3478")),
            #[cfg(any(test, feature = "diagnostics"))]
            Self::Cloudflare => Some(("cloudflare", "stun:stun.cloudflare.com:3478")),
        }
    }
}

pub struct Attempt {
    commands: Option<mpsc::SyncSender<Command>>,
    id: String,
    completion: Arc<Mutex<Completion>>,
    finished: Arc<(Mutex<bool>, Condvar)>,
    wake: Arc<Condvar>,
}

impl Attempt {
    #[cfg(test)]
    pub fn spawn(output: Output) -> Result<Self> {
        Self::spawn_named("local-offer-test", output)
    }

    #[cfg(test)]
    pub fn spawn_named(id: &str, output: Output) -> Result<Self> {
        Self::spawn_configured(id, output, None, StunProvider::None, false, None)
    }

    pub fn spawn_configured(
        id: &str,
        output: Output,
        config: Option<crate::control::Config>,
        stun_provider: StunProvider,
        legacy_rsa_1024: bool,
        connection_settings: Option<Arc<crate::connection_settings::Manager>>,
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
            started_at: std::time::Instant::now(),
            closing: false,
            output: Some(output),
            progress: Progress {
                #[cfg(any(test, feature = "diagnostics"))]
                stun_provider,
                #[cfg(any(test, feature = "diagnostics"))]
                legacy_rsa_1024,
                ..Default::default()
            },
            cancelled: false,
            events: Default::default(),
            messages: Default::default(),
            message_bytes: 0,
            discard_unavailable_media: config.is_some(),
            media_ingress: Default::default(),
            video_stream,
            #[cfg(windows)]
            video_output: None,
            audio_stream: None,
        }));
        let finished = Arc::new((Mutex::new(false), Condvar::new()));
        let shared = completion.clone();
        let done = finished.clone();
        let wake = Arc::new(Condvar::new());
        let notify = wake.clone();
        let attempt_id = id.to_owned();
        std::thread::Builder::new()
            .name("parsec-native-offer".into())
            .spawn(move || {
                let _permit = permit;
                let result = worker(
                    &shared,
                    &notify,
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
                notify.notify_all();
            })
            .context("starting native offer worker")?;
        Ok(Self {
            commands: Some(tx),
            id: id.into(),
            completion,
            finished,
            wake,
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
        // prevents the old worker from writing into reused guest buffers.
        let mut state = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        state.cancelled = true;
        #[cfg(windows)]
        if let Some(pipeline) = &state.video_output {
            pipeline.stop();
        }
        if let Some(mut audio) = state.audio_stream.take() {
            audio.stop();
        }
        state.events.clear();
        state.messages.clear();
        state.message_bytes = 0;
        if let Some(output) = state.output.take() {
            if output.finish(None).is_err() {
                state.progress.failed = true;
            }
        }
        drop(state);
        self.commands.take();
        self.wake.notify_all();
    }

    pub fn failure_stage(&self) -> Option<FailureStage> {
        let state = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        state
            .progress
            .failed
            .then_some(state.progress.failure_stage.unwrap_or(FailureStage::Worker))
    }

    pub fn control_ready(&self) -> bool {
        self.completion
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .progress
            .control_ready
    }

    pub fn media_ingress(&self) -> crate::media_ingress::Ingress {
        let state = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        let mut ingress = state.media_ingress.clone();
        ingress.video_stream = state.video_stream.snapshot();
        if let Some(audio) = &state.audio_stream {
            let report = audio.snapshot();
            ingress.audio_decoder_available = report.packets_decoded > 0;
            ingress.audio_output = Some(report);
        }
        #[cfg(windows)]
        if let Some(pipeline) = &state.video_output {
            let output = pipeline.snapshot();
            ingress.video_decoder_available = output.frames_decoded > 0;
            ingress.video_output = Some(output);
        }
        ingress
    }

    #[cfg(windows)]
    pub fn start_video(&self, window: Arc<crate::window::Window>) {
        let generation = window.stats.begin();
        self.completion
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .telemetry = Some((window.stats.clone(), generation));
        self.completion
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .audio_stream = Some(crate::audio_stream::Pipeline::start());
        self.completion
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .video_output = Some(crate::video_windows::Pipeline::start(window));
    }

    pub fn poll_audio(&self, memory: &GuestMemory, pointer: u32, capacity: usize) -> Result<usize> {
        let state = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        match &state.audio_stream {
            Some(audio) => audio.poll(memory, pointer, capacity),
            None => Ok(0),
        }
    }

    pub fn configure_video_protocol(&self, protocol: crate::backend::VideoProtocol) {
        self.completion
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .video_stream
            .configure(protocol);
    }

    pub fn pop_binary(&self) -> Option<(u16, bool, Bytes)> {
        let mut state = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        let message = state.messages.pop_front()?;
        state.message_bytes -= message.2.len();
        Some(message)
    }

    #[cfg(any(test, feature = "diagnostics"))]
    pub fn snapshot(&self) -> serde_json::Value {
        let state = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        serde_json::json!({ "offer_ready": state.progress.ready, "mid":state.progress.mid, "peer_closed": state.progress.closed, "failed": state.progress.failed, "failure_stage": state.progress.failure_stage, "negotiated_channels": state.progress.channels, "channels_open": state.progress.open_mask.count_ones(), "transport_connected":state.progress.transport_connected, "local_candidates":state.progress.local_candidates, "remote_candidates":state.progress.remote_candidates, "messages_received":state.progress.messages_received, "worker_finished": *self.finished.0.lock().unwrap_or_else(|e| e.into_inner()), "host_connected": false,
        "local_description_set":state.progress.local_description_set,"remote_description_set":state.progress.remote_description_set,"sync_received":state.progress.sync_received,"transport_states":state.progress.transport_states,"transport_states_before_close":state.progress.transport_states_before_close,"transport_states_at_failure":state.progress.transport_states_at_failure,
        "local_host_candidates":state.progress.local_host_candidates,"local_srflx_candidates":state.progress.local_srflx_candidates,
        "data_channel_only":true,"legacy_rsa_1024_enabled":state.progress.legacy_rsa_1024,"ice_servers_configured":state.progress.stun_provider.server().is_some(),"stun_provider":state.progress.stun_provider.server().map(|(name, _)| name),"network_types":["udp4"],"connection_deadline_seconds":30,"connection_deadline_scope":"establishment-only","connection_established":state.progress.connection_established,
        "channel_receive_api":"detached","unavailable_media_bypasses_control_queue":state.discard_unavailable_media,"queued_messages":state.messages.len(),"queued_message_bytes":state.message_bytes,"channel_message_limit_bytes":MAX_CHANNEL_MESSAGE,
        "channel_messages_received":state.progress.channel_messages_received,
        "channel_bytes_received":state.progress.channel_bytes_received,
        "channel_max_message_bytes":state.progress.channel_max_message_bytes,
        "failure_channel":state.progress.failure_channel,"failure_elapsed_ms":state.progress.failure_elapsed_ms,
        "last_send_elapsed_ms":state.progress.last_send_elapsed_ms,"last_sent_control_kind":state.progress.last_sent_control_kind })
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
    #[cfg(any(test, feature = "diagnostics"))]
    pub fn receive_binary(&self, duration: Duration) -> Result<(u16, bool, Bytes)> {
        let state = self
            .completion
            .lock()
            .map_err(|_| anyhow::anyhow!("native receipt lock poisoned"))?;
        let (mut state, _) = self
            .wake
            .wait_timeout_while(state, duration, |s| {
                s.messages.is_empty() && !s.progress.failed && !s.cancelled
            })
            .map_err(|_| anyhow::anyhow!("native receipt wait failed"))?;
        let message = state
            .messages
            .pop_front()
            .context("no native receipt before deadline")?;
        state.message_bytes -= message.2.len();
        Ok(message)
    }
    #[cfg(any(test, feature = "diagnostics"))]
    pub fn wait_transport(&self, duration: Duration) -> Result<()> {
        let state = self
            .completion
            .lock()
            .map_err(|_| anyhow::anyhow!("native connection lock poisoned"))?;
        let (state, _) = self
            .wake
            .wait_timeout_while(state, duration, |s| {
                (s.progress.open_mask != 7 || !s.progress.transport_connected)
                    && !s.progress.failed
                    && !s.cancelled
            })
            .map_err(|_| anyhow::anyhow!("native connection wait failed"))?;
        if state.progress.failed
            || state.cancelled
            || state.progress.open_mask != 7
            || !state.progress.transport_connected
        {
            bail!("native channels did not connect before deadline");
        }
        Ok(())
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
    connection_settings: Option<Arc<crate::connection_settings::Manager>>,
}

fn worker(
    shared: &Arc<Mutex<Completion>>,
    wake: &Arc<Condvar>,
    id: &str,
    commands: mpsc::Receiver<Command>,
    config: Option<crate::control::Config>,
    network: NetworkOptions,
) -> Result<()> {
    let NetworkOptions {
        stun_provider,
        legacy_rsa_1024,
        connection_settings,
    } = network;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let mut settings = SettingEngine::default();
    settings.set_network_types(vec![NetworkType::Udp4]);
    settings.set_data_channel_only(true);
    settings.detach_data_channels();
    settings.set_sctp_max_message_size_can_send(
        webrtc::api::setting_engine::SctpMaxMessageSize::Bounded(MAX_CHANNEL_MESSAGE as u32),
    );
    // Explicit legacy key-size compatibility only. Signature and SDP fingerprint
    // verification remain enabled; insecure hashes are not enabled.
    settings.allow_insecure_verification_algorithm(legacy_rsa_1024);
    let api = APIBuilder::new().with_setting_engine(settings).build();
    // Resolve short-lived credentials on the async worker, never the UI/WASM
    // thread. Snapshot settings once; a save affects only the next attempt.
    let mut ice = ice_configuration(stun_provider);
    if let Some(manager) = connection_settings {
        ice.ice_servers = runtime.block_on(async {
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
                if !bus.requested() {
                    continue;
                }
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
                    sample.audio_bytes = s.progress.channel_bytes_received[2];
                    sample.audio_ready = s.audio_stream.is_some();
                    sample.video = s.video_output.as_ref().map(|v| v.snapshot());
                    sample.profile = s.video_stream.snapshot().sps_profile_idc;
                }
                bus.publish(generation, sample);
            }
        });
    }
    let callback_wake = wake.clone();
    let attempt_id = id.to_owned();
    peer.on_ice_candidate(Box::new(move |candidate| {
        let shared=callback_state.clone(); let wake=callback_wake.clone(); let id=attempt_id.clone();
        Box::pin(async move {
            let Some(candidate)=candidate else { return; };
            if candidate.protocol != RTCIceProtocol::Udp || candidate.component != 1 { return; }
            let mut state=shared.lock().unwrap_or_else(|e|e.into_inner());
            if state.cancelled { return; }
            if state.events.len() >= 64 || !matches!(candidate.typ,RTCIceCandidateType::Host|RTCIceCandidateType::Srflx|RTCIceCandidateType::Relay) { state.progress.failed=true; }
            else {
                state.progress.local_candidates+=1;
                if candidate.typ==RTCIceCandidateType::Host {state.progress.local_host_candidates+=1;} else if candidate.typ==RTCIceCandidateType::Srflx {state.progress.local_srflx_candidates+=1;}
                state.events.push_back(serde_json::json!({"type":8,"attemptID":id,"ip":candidate.address,"port":candidate.port,"lan":candidate.typ==RTCIceCandidateType::Host,"fromStun":candidate.typ==RTCIceCandidateType::Srflx,"sync":false}));
            }
            wake.notify_all();
        })
    }));
    let callback_state = shared.clone();
    let callback_wake = wake.clone();
    // A weak reference avoids a peer -> callback -> peer ownership cycle.
    let callback_peer = Arc::downgrade(&peer);
    peer.on_peer_connection_state_change(Box::new(move |connection_state| {
        let shared = callback_state.clone();
        let wake = callback_wake.clone();
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
            wake.notify_all();
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
                let callback_wake = wake.clone();
                let startup_config = if id == 0 { config.clone() } else { None };
                let weak_channel = Arc::downgrade(&channel);
                let attempt_id = control_attempt_id.clone();
                channel.on_open(Box::new(move || {
                    let shared = callback_state.clone();
                    let wake = callback_wake.clone();
                    Box::pin(async move {
                        if shared.lock().unwrap_or_else(|e| e.into_inner()).cancelled { return; }
                        let Some(channel) = weak_channel.upgrade() else { return; };
                        let detached = match channel.detach().await {
                            Ok(detached) => detached,
                            Err(_) => {
                                shared.lock().unwrap_or_else(|e| e.into_inner()).fail_channel(FailureStage::DataChannelDetach, id);
                                wake.notify_all();
                                return;
                            }
                        };
                        tokio::spawn(read_channel(shared.clone(), wake.clone(), id, detached));
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
                        wake.notify_all();
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
        let mut deadline = EstablishmentDeadline::new(std::time::Instant::now());
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
            let command = match commands.recv_timeout(wait) {
                Ok(command) => command,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
            };
            if shared.lock().unwrap_or_else(|e| e.into_inner()).cancelled {
                break;
            }
            if matches!(command, Command::Disconnect) {
                let _ = runtime.block_on(send_disconnect(&channels[0]));
                break;
            }
            runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(5),async {
                    match command {
                        Command::Begin(remote)=>{
                            let local=offer.take().context("remote begin already supplied")?;
                            peer.set_local_description(local).await.context(FailureStage::SetLocalDescription)?;
                            shared.lock().unwrap_or_else(|e|e.into_inner()).progress.local_description_set=true;
                            peer.set_remote_description(RTCSessionDescription::answer(description.answer(&remote)?)?).await.context(FailureStage::SetRemoteDescription)?;
                            shared.lock().unwrap_or_else(|e|e.into_inner()).progress.remote_description_set=true;
                            gate.remote_ready(id,&description.mid,&remote.ufrag).context(FailureStage::CandidateGate)?;
                        }
                        Command::Candidate(candidate)=>gate.push(id,candidate).context(FailureStage::CandidateGate)?,
                        Command::Sync=>{
                            gate.sync(id)?;
                            shared.lock().unwrap_or_else(|e|e.into_inner()).progress.sync_received=true;
                            if !sync_ack_scheduled {
                                sync_ack_scheduled=true;
                                let shared=shared.clone(); let wake=wake.clone(); let id=id.to_owned();
                                tokio::spawn(async move {
                                    // ka() in the pinned JS returns this marker after 500 ms.
                                    tokio::time::sleep(Duration::from_millis(500)).await;
                                    let mut state=shared.lock().unwrap_or_else(|e|e.into_inner());
                                    if !state.cancelled {
                                        if state.events.len()>=64 { state.progress.failed=true; }
                                        else { state.events.push_back(serde_json::json!({"type":8,"attemptID":id,"ip":"1.2.3.4","port":1234,"lan":false,"fromStun":false,"sync":true})); }
                                        wake.notify_all();
                                    }
                                });
                            }
                        }
                        Command::Send(id,payload)=>{
                            let channel=&channels[usize::from(id)];
                            if channel.ready_state()!=RTCDataChannelState::Open { return Err(FailureStage::ChannelSend.into()); }
                            channel.send(&payload).await.context(FailureStage::ChannelSend)?;
                            let mut state=shared.lock().unwrap_or_else(|e|e.into_inner());
                            state.progress.last_send_elapsed_ms=Some(state.elapsed_ms());
                            state.progress.last_sent_control_kind=if id==0 { payload.get(12).copied() } else { None };
                        }
                        Command::Disconnect => unreachable!("handled before command dispatch"),
                    }
                    while let Some(candidate)=gate.pop_ready() { peer.add_ice_candidate(candidate).await.context(FailureStage::AddIceCandidate)?; shared.lock().unwrap_or_else(|e|e.into_inner()).progress.remote_candidates+=1; }
                    Ok::<(),anyhow::Error>(())
                }).await
            })??;
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
    use crate::signaling::Credentials;
    use wasmtime::{Config, Engine, MemoryType, SharedMemory};
    fn empty_completion() -> Completion {
        Completion {
            #[cfg(windows)]
            telemetry: None,
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
            #[cfg(windows)]
            video_output: None,
            audio_stream: None,
        }
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

    fn memory() -> GuestMemory {
        let mut config = Config::new();
        config.wasm_threads(true);
        GuestMemory(
            SharedMemory::new(&Engine::new(&config).unwrap(), MemoryType::shared(1, 1)).unwrap(),
        )
    }
    #[test]
    fn outputs_reject_aliases_and_incomplete_credentials_before_publication() {
        let m = memory();
        assert!(Output::new(m.clone(), [100, 200, 700], 256, 64, 80).is_err());
        assert!(Output::new(m.clone(), [100, 400, 700], 256, 65, 80).is_err());
        assert!(Output::new(m.clone(), [100, 400, 700], 256, 64, 702).is_err());
        let output = Output::new(m.clone(), [100, 400, 700], 128, 64, 80).unwrap();
        let d = Description {
            credentials: Credentials {
                ufrag: "abcd".into(),
                password: "x".repeat(256),
                fingerprint: format!("sha-256 {}", ["AB"; 32].join(":")),
            },
            mid: "0".into(),
        };
        assert!(!output.finish(Some(&d)).unwrap());
        assert_eq!(m.u32(80).unwrap(), 1);
        assert_eq!(m.string(100, 128).unwrap(), "");
        assert_eq!(m.string(400, 128).unwrap(), "");
        assert_eq!(m.string(700, 128).unwrap(), "");
    }
    #[test]
    fn cancellation_prevents_writes_into_reused_guest_buffers() {
        let m = memory();
        let output = Output::new(m.clone(), [100, 400, 700], 256, 64, 80).unwrap();
        let mut attempt = Attempt::spawn(output).unwrap();
        assert!(attempt.sync("stale-attempt").is_err());
        assert!(attempt
            .candidate(
                "stale-attempt",
                Candidate::new("127.0.0.1", 1234, false).unwrap()
            )
            .is_err());
        attempt.cancel();
        assert!(attempt.sync("local-offer-test").is_err());
        m.sync_word(64).unwrap().store(0, Ordering::SeqCst);
        for ptr in [100, 400, 700] {
            m.c_string(ptr, 256, "reused-buffer").unwrap();
        }
        m.set_u32(80, 12345).unwrap();
        attempt.wait_finished(Duration::from_secs(12)).unwrap();
        for ptr in [100, 400, 700] {
            assert_eq!(m.string(ptr, 256).unwrap(), "reused-buffer");
        }
        assert_eq!(m.u32(80).unwrap(), 12345);
        assert_eq!(attempt.snapshot()["peer_closed"], true);
        assert_eq!(attempt.snapshot()["host_connected"], false);
    }
}
