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
    channels: usize,
    mid: String,
    open_mask: u8,
    transport_connected: bool,
    local_candidates: usize,
    remote_candidates: usize,
    messages_received: usize,
}

struct Completion {
    output: Option<Output>,
    progress: Progress,
    cancelled: bool,
    events: VecDeque<serde_json::Value>,
    messages: VecDeque<(u16, bool, Bytes)>,
    message_bytes: usize,
}

enum Command {
    Begin(Credentials),
    Candidate(Candidate),
    Sync,
    Send(u16, Bytes),
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

    pub fn spawn_named(id: &str, output: Output) -> Result<Self> {
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
        let completion = Arc::new(Mutex::new(Completion {
            output: Some(output),
            progress: Default::default(),
            cancelled: false,
            events: Default::default(),
            messages: Default::default(),
            message_bytes: 0,
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
                let result = worker(&shared, &notify, &attempt_id, rx);
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                if result.is_err() {
                    state.progress.failed = true;
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

    pub fn cancel(&mut self) {
        // Serialize publication and cancellation. Taking the output lease
        // prevents the old worker from writing into reused guest buffers.
        let mut state = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        state.cancelled = true;
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

    pub fn failed(&self) -> bool {
        self.completion
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .progress
            .failed
    }

    pub fn snapshot(&self) -> serde_json::Value {
        let state = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        serde_json::json!({ "offer_ready": state.progress.ready, "mid":state.progress.mid, "peer_closed": state.progress.closed, "failed": state.progress.failed, "negotiated_channels": state.progress.channels, "channels_open": state.progress.open_mask.count_ones(), "transport_connected":state.progress.transport_connected, "local_candidates":state.progress.local_candidates, "remote_candidates":state.progress.remote_candidates, "messages_received":state.progress.messages_received, "worker_finished": *self.finished.0.lock().unwrap_or_else(|e| e.into_inner()), "host_connected": false })
    }

    fn command(&self, id: &str, command: Command) -> Result<()> {
        if id != self.id {
            bail!("command belongs to a different attempt");
        }
        self.commands
            .as_ref()
            .context("native attempt is cancelled")?
            .try_send(command)
            .map_err(|_| anyhow::anyhow!("native attempt command queue is full or closed"))
    }
    pub fn begin(&self, id: &str, remote: Credentials) -> Result<()> {
        remote.validate()?;
        self.command(id, Command::Begin(remote))
    }
    pub fn candidate(&self, id: &str, candidate: Candidate) -> Result<()> {
        self.command(id, Command::Candidate(candidate))
    }
    pub fn sync(&self, id: &str) -> Result<()> {
        self.command(id, Command::Sync)
    }
    pub fn send_binary(&self, channel: u16, payload: Bytes) -> Result<()> {
        if channel > 2 || payload.len() > 1024 * 1024 {
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

fn worker(
    shared: &Arc<Mutex<Completion>>,
    wake: &Arc<Condvar>,
    id: &str,
    commands: mpsc::Receiver<Command>,
) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let mut settings = SettingEngine::default();
    settings.set_network_types(vec![NetworkType::Udp4]);
    let api = APIBuilder::new().with_setting_engine(settings).build();
    let peer = runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(5),
            api.new_peer_connection(RTCConfiguration::default()),
        )
        .await
    })??;
    let callback_state = shared.clone();
    let callback_wake = wake.clone();
    let attempt_id = id.to_owned();
    peer.on_ice_candidate(Box::new(move |candidate| {
        let shared=callback_state.clone(); let wake=callback_wake.clone(); let id=attempt_id.clone();
        Box::pin(async move {
            let Some(candidate)=candidate else { return; };
            if candidate.protocol != RTCIceProtocol::Udp || candidate.component != 1 { return; }
            let mut state=shared.lock().unwrap_or_else(|e|e.into_inner());
            if state.cancelled { return; }
            if state.events.len() >= 64 || !matches!(candidate.typ,RTCIceCandidateType::Host|RTCIceCandidateType::Srflx) { state.progress.failed=true; }
            else {
                state.progress.local_candidates+=1;
                state.events.push_back(serde_json::json!({"type":8,"attemptID":id,"ip":candidate.address,"port":candidate.port,"lan":candidate.typ==RTCIceCandidateType::Host,"fromStun":candidate.typ==RTCIceCandidateType::Srflx,"sync":false}));
            }
            wake.notify_all();
        })
    }));
    let callback_state = shared.clone();
    let callback_wake = wake.clone();
    peer.on_peer_connection_state_change(Box::new(move |connection_state| {
        let shared = callback_state.clone();
        let wake = callback_wake.clone();
        Box::pin(async move {
            let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
            if !state.cancelled {
                state.progress.transport_connected =
                    connection_state == RTCPeerConnectionState::Connected;
                if connection_state == RTCPeerConnectionState::Failed {
                    state.progress.failed = true;
                }
            }
            wake.notify_all();
        })
    }));
    let prepared = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut channels = Vec::new();
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
                channel.on_open(Box::new(move || {
                    let shared = callback_state.clone();
                    let wake = callback_wake.clone();
                    Box::pin(async move {
                        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                        if !state.cancelled {
                            state.progress.open_mask |= 1 << id;
                        }
                        wake.notify_all();
                    })
                }));
                let callback_state = shared.clone();
                let callback_wake = wake.clone();
                channel.on_message(Box::new(move |message| {
                    let shared = callback_state.clone();
                    let wake = callback_wake.clone();
                    Box::pin(async move {
                        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                        if state.cancelled {
                            return;
                        }
                        if message.data.len() > 1024 * 1024
                            || state.messages.len() >= 16
                            || state.message_bytes + message.data.len() > 4 * 1024 * 1024
                        {
                            state.progress.failed = true;
                        } else {
                            state.progress.messages_received += 1;
                            state.message_bytes += message.data.len();
                            state
                                .messages
                                .push_back((id, message.is_string, message.data));
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
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            if std::time::Instant::now() >= deadline {
                bail!("native diagnostic attempt expired");
            }
            let command = match commands
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            {
                Ok(command) => command,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => bail!("native diagnostic attempt expired"),
            };
            if shared.lock().unwrap_or_else(|e| e.into_inner()).cancelled {
                break;
            }
            runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(5),async {
                    match command {
                        Command::Begin(remote)=>{
                            let local=offer.take().context("remote begin already supplied")?;
                            peer.set_local_description(local).await?;
                            peer.set_remote_description(RTCSessionDescription::answer(description.answer(&remote)?)?).await?;
                            gate.remote_ready(id,&description.mid,&remote.ufrag)?;
                        }
                        Command::Candidate(candidate)=>gate.push(id,candidate)?,
                        Command::Sync=>{
                            gate.sync(id)?;
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
                        Command::Send(channel,payload)=>{
                            let channel=&channels[usize::from(channel)];
                            if channel.ready_state()!=RTCDataChannelState::Open { bail!("native data channel is not open"); }
                            channel.send(&payload).await?;
                        }
                    }
                    while let Some(candidate)=gate.pop_ready() { peer.add_ice_candidate(candidate).await?; shared.lock().unwrap_or_else(|e|e.into_inner()).progress.remote_candidates+=1; }
                    Ok::<(),anyhow::Error>(())
                }).await
            })??;
        }
        drop((offer, channels));
        Ok(())
    })();
    let closed = runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(2), peer.close()).await });
    let is_closed =
        matches!(closed, Ok(Ok(()))) && peer.connection_state() == RTCPeerConnectionState::Closed;
    {
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        state.progress.closed = is_closed;
        if is_closed {
            state.progress.transport_connected = false;
            state.progress.open_mask = 0;
        }
    }
    if !is_closed {
        bail!("native offer peer did not close within the deadline");
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signaling::Credentials;
    use wasmtime::{Config, Engine, MemoryType, SharedMemory};
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
