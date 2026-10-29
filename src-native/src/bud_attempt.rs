//! Explicit BUD lifecycle. RTC attempts never enter this adapter.
use anyhow::{ensure, Context, Result};
use evoparseq_bud::{
    bud::{Event, Outcome, Route, Session},
    signaling::Signaling,
};
use serde_json::{json, Value};
pub type SignalingWrite = Arc<dyn Fn(String) -> bool + Send + Sync>;

type SharedAudio = Arc<Mutex<Option<Arc<crate::audio_stream::Pipeline>>>>;

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Default)]
struct State {
    output_finished: bool,
    wire_id: String,
    retired_wires: Vec<String>,
    template: Option<Value>,
    writer: Option<SignalingWrite>,
    established: bool,
    events: VecDeque<Value>,
    controls: VecDeque<bytes::Bytes>,
    ingress: crate::media_ingress::Ingress,
    failure: Option<&'static str>,
    host_status: Option<i32>,
}

// Keep BUD identity through connecting/disconnected states too. An old worker
// finishing after reconnect may only publish into its own stats generation.
struct Statistics {
    shared: Arc<crate::stats::Shared>,
    generation: u64,
}
impl Statistics {
    fn consume_timing(&self, event: &Event) -> bool {
        if let Event::ExternalControl { bytes } = event {
            self.shared.host_timing.receive(self.generation, bytes)
        } else {
            false
        }
    }
    fn start(shared: Arc<crate::stats::Shared>) -> Self {
        let generation = shared.begin();
        shared.publish(
            generation,
            crate::stats::Sample {
                bud: Some(Default::default()),
                state: "Connecting (BUD)".into(),
                ..Default::default()
            },
        );
        Self { shared, generation }
    }
}
impl Drop for Statistics {
    fn drop(&mut self) {
        self.shared.host_timing.end(self.generation);
        self.shared.publish(
            self.generation,
            crate::stats::Sample {
                bud: Some(Default::default()),
                state: "Disconnected (BUD)".into(),
                ..Default::default()
            },
        );
    }
}

pub struct Attempt {
    id: String,
    state: Arc<Mutex<State>>,
    session: Arc<Mutex<Option<Session>>>,
    audio: SharedAudio,
    commands: mpsc::SyncSender<String>,
    cancel: Arc<AtomicBool>,
    thread: std::thread::Thread,
    hid_input: (bool, bool),

    worker: Option<std::thread::JoinHandle<()>>,
}

impl Attempt {
    pub fn spawn_native(
        id: &str,
        peer: &str,
        writer: SignalingWrite,
        window: Arc<crate::window::Window>,
        manager: Arc<crate::connection_settings::Manager>,
    ) -> Result<Self> {
        // Account signaling and media share one offer/candidate/retry worker.
        ensure!(
            !peer.is_empty() && peer.len() <= 256,
            "invalid native peer id"
        );
        let template = json!({"version":1,"action":"offer","payload":{
            "to":peer,"attempt_id":id,"secret":"","data":{}}});
        Self::spawn_inner(
            id,
            window,
            manager,
            State {
                template: Some(template),
                writer: Some(writer),
                ..Default::default()
            },
        )
    }

    fn spawn_inner(
        id: &str,
        window: Arc<crate::window::Window>,
        manager: Arc<crate::connection_settings::Manager>,
        initial: State,
    ) -> Result<Self> {
        let settings = manager.view().0;
        let hid_input = (
            settings.hid_mouse_enabled(),
            settings.hid_keyboard_enabled(),
        );
        let state = Arc::new(Mutex::new(initial));
        let session = Arc::new(Mutex::new(None));
        let audio = Arc::new(Mutex::new(None));
        let cancel = Arc::new(AtomicBool::new(false));
        let (commands, incoming) = mpsc::sync_channel(64);
        let (worker_state, worker_session, worker_audio, worker_cancel) = (
            state.clone(),
            session.clone(),
            audio.clone(),
            cancel.clone(),
        );
        let attempt_id = id.to_owned();
        let worker = std::thread::Builder::new().name("evoparseq-bud-media".into()).spawn(move || {
            let mut stage = "start";
            let result = run(&attempt_id, &window, &manager, &worker_state, &worker_session, &worker_audio, &worker_cancel, incoming, &mut stage, hid_input);

            if result.is_err() {
                if !worker_cancel.load(Ordering::Acquire) {
                    crate::event_log::record("ERROR", "connection_failed", json!({"transport":"BUD","stage":stage,"route":window.stats.read().map(|s|s.route)}));
                }
                let mut state = worker_state.lock().unwrap_or_else(|e| e.into_inner());
                if !worker_cancel.load(Ordering::Acquire) {
                state.failure.get_or_insert("BUD connection or media failed");
                // No tokens, AES keys, addresses or server responses in diagnostics.
                state.events.clear();
                state.controls.clear();
                state.events.push_back(json!({"type":7,"status":-6200,"state":4,"attemptID":attempt_id,"duration":0}));
                state.output_finished = true;
                }
            }
            withdraw_pending(&worker_state);
            let mut retired = worker_session.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(session) = &mut retired { session.close(); }

        })?;
        let thread = worker.thread().clone();
        // The worker owns its shutdown. Never join it under Backend/UI locks.

        Ok(Self {
            id: id.into(),
            state,
            session,
            audio,
            commands,
            cancel,
            thread,
            hid_input,

            worker: Some(worker),
        })
    }

    pub fn hid_mouse_enabled(&self) -> bool {
        self.hid_input.0
    }

    pub fn native_connected(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .established
    }

    pub fn host_status(&self) -> Option<i32> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .host_status
    }

    pub fn incoming(&self, text: &str) -> Result<()> {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return Ok(());
        };
        let wire = value["payload"]["attempt_id"].as_str().unwrap_or("");
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.retired_wires.iter().any(|id| id == wire) {
            return Ok(());
        }
        let current = if state.wire_id.is_empty() {
            self.id.as_str()
        } else {
            &state.wire_id
        };
        if wire != current
            || !matches!(
                value["action"].as_str(),
                Some("answer_relay" | "candex_relay" | "close")
            )
        {
            return Ok(());
        }
        drop(state);
        ensure!(text.len() <= 16 * 1024, "BUD signaling exceeds limit");
        if value["action"] == "answer_relay" && value["payload"]["approved"] == true {
            ensure!(
                value["payload"]["data"].is_object()
                    && value["payload"]["data"]["creds"].is_object(),
                "malformed BUD answer"
            );
        }
        // The BUD worker owns termination, credentials and direct-to-relay retry.
        self.commands
            .try_send(text.to_owned())
            .context("BUD signaling queue unavailable")?;
        self.thread.unpark();
        Ok(())
    }

    pub fn send(&self, frame: &[u8]) -> Result<()> {
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let session = session.as_mut().context("BUD session unavailable")?;
        if frame.len() >= 14 && frame[12] == 17 {
            let id = u32::from_be_bytes(frame[4..8].try_into()?);
            let body = &frame[13..];
            ensure!(body.last() == Some(&0), "BUD user data lacks terminator");
            ensure!(
                session.send_user_data(id, &body[..body.len() - 1]),
                "BUD user data refused"
            );
            return Ok(());
        }
        session.send_control(frame)
    }

    pub fn native_input(
        &self,
        packet: &[u8],
        viewport: Option<crate::viewport::Viewport>,
    ) -> Result<()> {
        let absolute = if self.hid_mouse_enabled() {
            crate::hid_input::absolute_motion(packet, viewport)?
        } else {
            None
        };
        self.send(absolute.as_deref().unwrap_or(packet))
    }

    pub fn audio_pipeline(&self) -> Option<Arc<crate::audio_stream::Pipeline>> {
        self.audio.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn pop_event(&self) -> Option<Value> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .events
            .pop_front()
    }
    pub fn pop_control(&self) -> Option<bytes::Bytes> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .controls
            .pop_front()
    }

    pub fn failure(&self) -> Option<&'static str> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.failure.or_else(|| {
            state
                .ingress
                .video_output
                .as_ref()
                .and_then(|video| video.failure_stage)
                .filter(|stage| stage.starts_with("video-native-"))
        })
    }
}
impl Drop for Attempt {
    fn drop(&mut self) {
        {
            let _state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            self.cancel.store(true, Ordering::Release);
            self.thread.unpark();
        }

        if let Some(worker) = self.worker.take() {
            // Complete authenticated departure before process exit when possible.
            // Allow authenticated departure/HID removal before process exit,
            // outside every state/session lock, with a bounded startup-cancel wait.
            let until = Instant::now() + Duration::from_secs(2);
            while !worker.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                crate::event_log::record("WARN", "native_bud_shutdown_pending", json!({}));
            }
        }
    }
}

fn send_candidate(state: &Mutex<State>, event: &Event) -> Result<()> {
    let (writer, message) = {
        let state = state.lock().unwrap_or_else(|e| e.into_inner());
        let peer = state
            .template
            .as_ref()
            .and_then(|v| v["payload"]["to"].as_str())
            .context("BUD account destination missing")?;
        let message = Signaling::new(peer, &state.wire_id)?
            .candidate(event)
            .context("not a BUD candidate")?;
        (
            state.writer.clone().context("BUD account writer missing")?,
            serde_json::to_string(&message)?,
        )
    };
    ensure!(writer(message), "BUD candidate send failed");
    Ok(())
}

fn switch_offer(state: &Mutex<State>, cancel: &AtomicBool, id: &str, data: Value) -> Result<bool> {
    let (writer, previous, next) = {
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        if cancel.load(Ordering::Acquire) {
            return Ok(false);
        }
        let mut next = state
            .template
            .clone()
            .context("BUD account offer missing")?;
        let mut previous = next.clone();
        previous["action"] = json!("offer_cancel");
        previous["payload"]["attempt_id"] = json!(state.wire_id);
        previous["payload"]["data"] = json!({});
        next["payload"]["attempt_id"] = json!(id);
        next["payload"]["data"] = data.clone();
        ensure!(state.retired_wires.len() < 32, "BUD relay retry limit");
        let previous_id = std::mem::replace(&mut state.wire_id, id.into());
        state.retired_wires.push(previous_id);
        state.events.clear();
        state.controls.clear();
        (
            state.writer.clone().context("BUD account writer missing")?,
            serde_json::to_string(&previous)?,
            serde_json::to_string(&next)?,
        )
    };
    ensure!(
        writer(previous) && writer(next),
        "BUD relay offer send failed"
    );
    Ok(true)
}

// A cancelled relay retry may not have reached the signaling consumer yet. Retire
// its native account offer here, outside the UI/session locks. Established
// sessions leave through the SDK's existing authenticated teardown instead.
fn withdraw_pending(state: &Mutex<State>) {
    let pending = {
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        if state.established {
            return;
        }
        state.template.take().and_then(|mut message| {
            let writer = state.writer.clone()?;
            message["action"] = json!("offer_cancel");
            message["payload"]["attempt_id"] = json!(state.wire_id);
            message["payload"]["data"] = json!({});
            Some((writer, message.to_string()))
        })
    };
    if let Some((writer, message)) = pending {
        let _ = writer(message);
    }
}

// Serialize account offer completion against retirement and reconnect.
fn complete_offer(state: &Mutex<State>, cancel: &AtomicBool, offer: Value) -> Result<bool> {
    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
    if cancel.load(Ordering::Acquire) {
        return Ok(false);
    }
    ensure!(!state.output_finished, "BUD offer already completed");
    state.output_finished = true;
    let mut message = state
        .template
        .clone()
        .context("native account destination missing")?;
    message["payload"]["data"] = offer.clone();
    state.template = Some(message.clone());
    let writer = state
        .writer
        .clone()
        .context("native account writer missing")?;
    drop(state);
    ensure!(
        writer(message.to_string()),
        "native account offer send failed"
    );
    Ok(true)
}

// Force TURN starts on the selected relay carrier; otherwise try direct UDP first.
async fn route(
    manager: &crate::connection_settings::Manager,
) -> Result<(
    Route,
    Option<VecDeque<Route>>,
    crate::connection_settings::Settings,
)> {
    let (settings, servers) = manager.resolve_bud().await?;
    if settings.force_relay {
        let mut routes = turn_relays(servers).await?;
        let first = routes
            .pop_front()
            .context("BUD requires a supported TURN URL")?;
        return Ok((first, Some(routes), settings));
    }

    let mut stun = Vec::new();
    for server in servers {
        for raw in server.urls {
            let url = webrtc::ice::url::Url::parse_url(&raw)?;
            if url.scheme == webrtc::ice::url::SchemeType::Stun
                && url.proto == webrtc::ice::url::ProtoType::Udp
            {
                stun.extend(
                    tokio::net::lookup_host((url.host.as_str(), url.port))
                        .await?
                        .filter(|a| a.is_ipv4()),
                );
            }
        }
    }
    Ok((Route::Direct { stun }, None, settings))
}

async fn relay_route(
    manager: &crate::connection_settings::Manager,
    settings: &crate::connection_settings::Settings,
) -> Result<VecDeque<Route>> {
    if settings.stun_only || (settings.provider == "custom" && settings.turn_urls.is_empty()) {
        return Ok(VecDeque::new());
    }
    turn_relays(manager.resolve_bud_relay(settings).await?).await
}

fn relay_carrier(url: &webrtc::ice::url::Url) -> Option<evoparseq_bud::bud::RelayTransport> {
    use evoparseq_bud::bud::RelayTransport;
    use webrtc::ice::url::{ProtoType, SchemeType};
    match (url.scheme, url.proto) {
        (SchemeType::Turn, ProtoType::Udp) => Some(RelayTransport::Udp),
        (SchemeType::Turn, ProtoType::Tcp) => Some(RelayTransport::Tcp),
        (SchemeType::Turns, ProtoType::Tcp) => Some(RelayTransport::Tls {
            server_name: url.host.clone(),
        }),
        _ => None,
    }
}

async fn turn_relays(
    servers: Vec<webrtc::ice_transport::ice_server::RTCIceServer>,
) -> Result<VecDeque<Route>> {
    let mut urls = Vec::new();
    for server in &servers {
        for raw in &server.urls {
            let url = webrtc::ice::url::Url::parse_url(raw)?;
            if let Some(carrier) = relay_carrier(&url) {
                let priority = match carrier {
                    evoparseq_bud::bud::RelayTransport::Udp => 0,
                    evoparseq_bud::bud::RelayTransport::Tls { .. } => 1,
                    _ if url.port == 80 => 2,
                    _ => 3,
                };
                urls.push((priority, url, carrier, server));
            }
        }
    }
    urls.sort_by_key(|entry| entry.0);
    // Resolve concurrently so an unavailable DNS entry does not serially hold
    // up other routes. join_all preserves the explicitly sorted URL priority.
    let routes: VecDeque<_> = futures_util::future::join_all(urls.into_iter().take(32).map(
        |(_, url, carrier, server)| async move {
            let addresses = tokio::time::timeout(
                Duration::from_secs(3),
                tokio::net::lookup_host((url.host.as_str(), url.port)),
            )
            .await
            .ok()?
            .ok()?;
            let addr = addresses.into_iter().find(|a| a.is_ipv4())?;
            Some(Route::turn(
                addr,
                &server.username,
                &server.credential,
                carrier,
            ))
        },
    ))
    .await
    .into_iter()
    .flatten()
    .collect();
    ensure!(
        !routes.is_empty(),
        "BUD requires a reachable UDP, TLS or TCP TURN URL"
    );
    Ok(routes)
}

fn log_route_attempt(route: &Route) {
    use evoparseq_bud::bud::RelayTransport;
    let carrier = match route {
        Route::Direct { .. } => None,
        Route::Turn(relay) => Some(match &relay.transport {
            RelayTransport::Udp => "udp",
            RelayTransport::Tcp => "tcp",
            RelayTransport::Tls { .. } => "tls",
        }),
    };
    crate::event_log::record(
        "INFO",
        "bud_route_attempt",
        json!({"route":if carrier.is_some() {"TURN relay"} else {"Direct"}, "requested_turn_transport":carrier}),
    );
}

#[allow(clippy::too_many_arguments)]
fn run(
    id: &str,
    window: &Arc<crate::window::Window>,
    manager: &crate::connection_settings::Manager,
    state: &Arc<Mutex<State>>,
    shared_session: &Arc<Mutex<Option<Session>>>,
    audio: &SharedAudio,
    cancel: &AtomicBool,
    incoming: mpsc::Receiver<String>,
    stage: &mut &'static str,
    hid_input: (bool, bool),
) -> Result<()> {
    let (hid_mouse, hid_keyboard) = hid_input;
    *stage = "initial_route_resolution";
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let (route, mut relay_routes, settings) = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(15), route(manager)).await
    })??;
    *stage = if settings.force_relay {
        "initial_relay_selected"
    } else {
        "initial_direct_selected"
    };
    if cancel.load(Ordering::Acquire) {
        return Ok(());
    }
    // Probe before the lifecycle lock: a slow driver must not stall Cancel.
    let hevc_available =
        settings.prefer_hevc && crate::video_platform::hevc_available(window.video_gpu().as_ref());
    let chroma_444 = true
        && hevc_available
        && settings.experimental_chroma_444
        && crate::video_platform::hevc_444_available(
            window.video_gpu().as_ref(),
            settings.experimental_ten_bit,
        );
    let ten_bit = hevc_available
        && settings.experimental_ten_bit
        && crate::video_platform::hevc_ten_bit_available(window.video_gpu().as_ref());
    crate::event_log::record(
        "INFO",
        "video_capabilities_declared",
        json!({"transport":"BUD","flags":8 | u32::from(hevc_available) | if ten_bit {16} else {0} | if chroma_444 {2} else {0},"hevc":hevc_available,"main10":ten_bit,"chroma_444":chroma_444,"chroma_444_requested":settings.experimental_chroma_444}),
    );
    let hid_generation;
    let (mut media, mut session, statistics) = {
        let _state = state.lock().unwrap_or_else(|e| e.into_inner());
        if cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        // Creation only starts the existing workers. Surface/stats generation
        // is serialized against retirement, so a cancelled old attempt cannot
        // hide the next attempt's video or reset its statistics.
        let (media, session) = crate::bud_media::Media::start_verified(
            window.clone(),
            hevc_available,
            ten_bit,
            chroma_444,
            settings.allow_raw_audio,
            settings.experimental_surround_audio,
        )?;
        {
            hid_generation = window.hid_input_generation();
        }
        (media, session, Statistics::start(window.stats.clone()))
    };
    let _hid_lifetime = crate::hid_input::Lifetime::new(window.clone(), hid_generation);
    session.enable_hid_input(hid_mouse, hid_keyboard);
    let mut hid_states = [(false, 0); 2];
    for (enabled, device) in [(hid_mouse, "mouse"), (hid_keyboard, "keyboard")] {
        if enabled {
            crate::event_log::record(
                "INFO",
                "hid_input_enabled",
                json!({"transport":"BUD","device":device,"experimental":true}),
            );
        }
    }
    state.lock().unwrap_or_else(|e| e.into_inner()).wire_id = id.into();
    session.wake_consumer(std::thread::current());
    *stage = "initial_native_offer";
    log_route_attempt(&route);
    let creds = session.offer(id, route.with_tcp_qos(settings.experimental_tcp_qos))?;
    let mut source = session.take_media()?;
    let mut signaling = Signaling::new("host", id)?;
    *audio.lock().unwrap_or_else(|e| e.into_inner()) = Some(media.audio_pipeline());
    *shared_session.lock().unwrap_or_else(|e| e.into_inner()) = Some(session);
    *stage = "initial_account_offer_completion";
    if !complete_offer(
        state,
        cancel,
        signaling.offer(&creds)["payload"]["data"].take(),
    )? {
        return Ok(());
    }
    let mut deadline = Instant::now() + Duration::from_secs(30);
    let mut established = false;
    let mut timing_requests = crate::frame_timing::Requests::default();
    let mut timing_active = false;
    let mut answered_at: Option<Instant> = None;
    let mut direct_attempt = !settings.force_relay;
    let mut wire_id = id.to_owned();
    let mut last_stats = Instant::now();
    let mut last_cursor_capture = None;
    *stage = "initial_waiting_for_host";
    while !cancel.load(Ordering::Acquire) {
        if !established && Instant::now() >= deadline {
            *stage = "establishment_deadline";
        }
        let mut guard = shared_session.lock().unwrap_or_else(|e| e.into_inner());
        let session = guard.as_mut().context("BUD session retired")?;
        let mut ended = !established && Instant::now() >= deadline;
        for _ in 0..64 {
            let Ok(text) = incoming.try_recv() else {
                break;
            };
            if native_close(&text, &wire_id) {
                *stage = "host_close_received";
                ended = true;
                break;
            }
            *stage = "native_signal_processing";
            if let Err(error) = signaling.receive(&text, session) {
                let native = error.downcast_ref::<evoparseq_bud::bud::AnswerFailure>();
                let io = native.and_then(|e| e.io_details());
                crate::event_log::record(
                    "ERROR",
                    "bud_signaling_failed",
                    json!({
                    "native_error":native.map(|e|e.code()), "io_stage":io.map(|e|e.0),
                    "io_kind":io.map(|e|format!("{:?}",e.1)), "os_error":io.and_then(|e|e.2)}),
                );
                if io.is_some() {
                    // A local carrier/thread setup failure is a failed route,
                    // just like the worker's TransportFailed outcome.
                    ended = true;
                    break;
                }
                return Err(error);
            }
            *stage = "native_signal_processed";
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                if value["action"] == "answer_relay"
                    && value["payload"]["attempt_id"].as_str() == Some(&wire_id)
                {
                    answered_at.get_or_insert_with(Instant::now);
                    *stage = "host_answer_accepted";
                } else if value["action"] == "candex_relay" {
                    *stage = if value["payload"]["data"]["sync"] == true {
                        "host_readiness_received"
                    } else {
                        "host_candidate_received"
                    };
                }
            }
        }
        let mut events = Vec::new();
        for _ in 0..64 {
            *stage = "native_event_poll";
            let Some(event) = session.poll()? else {
                break;
            };
            events.push(event);
        }
        let transport = session.snapshot();
        if established && statistics.shared.host_timing.current(statistics.generation) {
            let desired = statistics.shared.requested();
            if let Some(request) = timing_requests.update(desired) {
                if desired {
                    timing_active = true;
                }
                statistics
                    .shared
                    .host_timing
                    .set_requested(statistics.generation, desired);
                if session.send_control(&request).is_err() {
                    timing_requests.fail();
                    statistics.shared.host_timing.failed(statistics.generation);
                    crate::event_log::record(
                        "WARN",
                        "host_timing_request_failed",
                        json!({"transport":"BUD"}),
                    );
                } else if !desired {
                    timing_active = false;
                }
            }
        }
        let hid_current = session.hid_input_state();
        window.set_hid_input_active(hid_generation, hid_current[0].0, hid_current[1].0);
        for (index, (enabled, device)) in [(hid_mouse, "mouse"), (hid_keyboard, "keyboard")]
            .into_iter()
            .enumerate()
        {
            let current = hid_current[index];
            let previous = hid_states[index];
            if enabled && current != previous {
                if current.1 != 0 && current.1 != previous.1 {
                    let reason = match current.1 {
                        1 => "host_setup_timeout",
                        2 => "invalid_interrupt_request",
                        3 => "input_queue_limit",
                        4 => "transport_send_failed",
                        _ => "host_device_ended",
                    };
                    crate::event_log::record(
                        "ERROR",
                        "hid_input_failed",
                        json!({"device":device,"reason":reason,"normal_input_fallback":false}),
                    );
                }
                if current.0 != previous.0 {
                    crate::event_log::record(
                        "INFO",
                        "hid_input_state_changed",
                        json!({"device":device,"active":current.0}),
                    );
                }
                hid_states[index] = current;
            }
        }
        media.recover(session);
        drop(guard);
        *stage = "native_media_processing";
        let pumped = media.pump(&mut source)?;
        *stage = "native_media_processed";
        for event in &events {
            if matches!(event, Event::Candidate { .. } | Event::Ready) {
                *stage = if matches!(event, Event::Ready) {
                    "local_readiness_sending"
                } else {
                    "local_candidate_sending"
                };
                send_candidate(state, event)?;
                *stage = "local_signal_sent";
            }
        }
        let mut shared = state.lock().unwrap_or_else(|e| e.into_inner());
        let mut cursor_ready = false;
        for event in events {
            if statistics.consume_timing(&event) {
                continue;
            }
            ensure!(
                shared.events.len() < 64 && shared.controls.len() < 64,
                "BUD application queue full"
            );
            match event {
                Event::IoFailure {
                    stage,
                    kind,
                    os_error,
                } => {
                    crate::event_log::record(
                        "ERROR",
                        "bud_transport_failed",
                        json!({"stage":stage.name(), "io_kind":format!("{kind:?}"), "os_error":os_error}),
                    );
                }
                Event::Candidate { .. } | Event::Ready => {}
                Event::Established { .. } => {
                    *stage = "transport_established";
                    crate::event_log::record(
                        "INFO",
                        "bud_connection_established",
                        json!({"route":if transport.relay {"TURN relay"} else {"Direct"},
                            "turn_transport":transport.relay_transport}),
                    );
                    established = true;
                    shared.established = true;
                    shared.events.push_back(
                        json!({"type":7,"status":0,"state":4,"attemptID":id,"duration":0}),
                    );
                }
                Event::Ended { outcome } => {
                    let host_status = match &outcome {
                        Outcome::Disconnected(status) => Some(*status),
                        _ => None,
                    };
                    let reason = match outcome {
                        Outcome::ConnectivityFailed => "ended_connectivity_failed",
                        Outcome::PeerGone => "ended_peer_gone",
                        Outcome::Undeliverable => "ended_undeliverable",
                        Outcome::TransportFailed => "ended_transport_failed",
                        Outcome::Disconnected(_) => "ended_host_disconnected",
                        Outcome::Unreadable => "ended_unreadable",
                        Outcome::RelayUnreachable => "ended_relay_unreachable",
                        Outcome::RelayRefused => "ended_relay_refused",
                        Outcome::RelayLost => "ended_relay_lost",
                    };
                    state.lock().unwrap_or_else(|e| e.into_inner()).host_status = host_status;
                    *stage = reason;
                    crate::event_log::record(
                        "WARN",
                        "bud_connection_ended",
                        json!({"reason":reason,"host_status":host_status,"attempt_route":if direct_attempt {"Direct"} else {"TURN relay"},"turn_transport":transport.relay_transport}),
                    );
                    ended = true;
                }
                Event::ExternalControl { bytes } => {
                    if let Some(capture) = crate::control::cursor_capture(&bytes) {
                        cursor_ready |= last_cursor_capture != Some(capture);
                        last_cursor_capture = Some(capture);
                    }
                    shared.controls.push_back(bytes.into());
                }
            }
        }
        let mut ingress = media.ingress();
        ingress.video_packets_received =
            shared.ingress.video_packets_received + pumped.video_messages as u64;
        ingress.audio_packets_received =
            shared.ingress.audio_packets_received + pumped.audio_messages as u64;
        ingress.video_bytes_received =
            shared.ingress.video_bytes_received + pumped.video_bytes as u64;
        ingress.audio_bytes_received =
            shared.ingress.audio_bytes_received + pumped.audio_bytes as u64;
        shared.ingress = ingress;
        if window.stats.sample_requested() && last_stats.elapsed() >= Duration::from_millis(250) {
            window.stats.publish(
                statistics.generation,
                crate::stats::Sample {
                    state: if established {
                        "Connected (BUD)"
                    } else {
                        "Connecting (BUD)"
                    }
                    .into(),
                    bud: Some(transport.clone()),
                    rtt_ms: transport.rtt_ms,
                    received: Some(transport.udp_received),
                    sent: Some(transport.udp_sent),
                    dtls: String::new(),
                    protocol: "BUD / UDP".into(),
                    route: if transport.connected {
                        if transport.relay {
                            "TURN relay"
                        } else {
                            "Direct"
                        }
                    } else {
                        "Unknown"
                    },
                    turn_protocol: transport.relay_transport.map(str::to_owned),
                    audio_bytes: shared.ingress.audio_bytes_received,
                    audio_codec: shared.ingress.audio_output.as_ref().and_then(|a| a.codec),
                    audio_layout: shared.ingress.audio_output.as_ref().and_then(|a| a.layout),
                    audio_decoded_queue_ms: shared
                        .ingress
                        .audio_output
                        .as_ref()
                        .map(|a| a.queued_frames as f64 / 48.0),
                    video: shared.ingress.video_output.clone(),
                    codec: shared.ingress.video_stream.codec,
                    profile: shared.ingress.video_stream.sps_profile_idc,
                    hevc_advertised: shared.ingress.video_stream.hevc_advertised,
                    audio_ready: shared
                        .ingress
                        .audio_output
                        .as_ref()
                        .is_some_and(|a| a.packets_decoded > 0),
                    ..Default::default()
                },
            );
            last_stats = Instant::now();
        }

        let native_pending = !shared.controls.is_empty() || !shared.events.is_empty();
        drop(shared);

        if native_pending {
            window.events.wake_consumer();
        }
        if cursor_ready {
            // Wake only after the unchanged FIFO contains the host event and
            // after releasing the worker state lock. The session still consumes
            // that event and owns its visibility/capture decisions.
            window.events.push(crate::window::Event::CursorReady);
        }
        let fallback_due = !established
            && (ended
                || (direct_attempt
                    && answered_at.is_some_and(|t| t.elapsed() >= Duration::from_secs(4))));
        if fallback_due {
            *stage = "relay_retry_started";
            if relay_routes.is_none() {
                relay_routes = Some(runtime.block_on(async {
                    tokio::time::timeout(Duration::from_secs(15), relay_route(manager, &settings))
                        .await
                })??);
            }
            let relay = relay_routes
                .as_mut()
                .and_then(VecDeque::pop_front)
                .context("BUD configured routes exhausted")?;
            direct_attempt = false;
            timing_requests = Default::default();
            timing_active = false;
            statistics.shared.host_timing.begin(statistics.generation);
            drop(source);
            let mut old = shared_session
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            if let Some(old) = &mut old {
                old.close();
            }
            drop(old);
            *stage = "relay_credentials_ready";
            if cancel.load(Ordering::Acquire) {
                return Ok(());
            }
            // The native client uses six random 8-hex groups per attempt.
            // Do not append a diagnostic suffix to an account protocol ID:
            // local SDK acceptance does not establish service compatibility.
            wire_id = native_attempt_id()?;
            last_cursor_capture = None;
            let mut next = crate::bud_media::session(
                hevc_available,
                ten_bit,
                chroma_444,
                settings.allow_raw_audio,
                media.audio_layout(),
            )?;
            next.enable_hid_input(hid_mouse, hid_keyboard);
            next.wake_consumer(std::thread::current());
            *stage = "relay_native_offer";
            log_route_attempt(&relay);
            let creds = next.offer(&wire_id, relay.with_tcp_qos(settings.experimental_tcp_qos))?;
            source = next.take_media()?;
            signaling = Signaling::new("host", &wire_id)?;
            *shared_session.lock().unwrap_or_else(|e| e.into_inner()) = Some(next);
            *stage = "relay_account_offer_sending";
            if !switch_offer(
                state,
                cancel,
                &wire_id,
                signaling.offer(&creds)["payload"]["data"].take(),
            )? {
                return Ok(());
            }
            *stage = "relay_account_offer_sent";
            answered_at = None;
            // A fresh relay attempt needs the same establishment allowance as
            // Force relay, independent of time spent discovering a direct path.
            deadline = Instant::now() + Duration::from_secs(30);
            continue;
        }
        ensure!(!ended, "BUD transport ended");
        if pumped.video_messages >= 8 || pumped.audio_messages >= 8 {
            continue;
        }
        // Packet arrivals unpark this consumer; this timeout only checks lifecycle/stats.
        std::thread::park_timeout(Duration::from_millis(100));
    }
    if timing_active {
        if let Some(session) = shared_session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            let _ = session.send_control(&crate::frame_timing::request(false));
        }
    }
    Ok(())
}

fn native_close(text: &str, attempt: &str) -> bool {
    serde_json::from_str::<Value>(text).is_ok_and(|value| {
        value["action"] == "close" && value["payload"]["attempt_id"].as_str() == Some(attempt)
    })
}

pub(crate) fn native_attempt_id() -> Result<String> {
    let mut random = [0u8; 24];
    getrandom::fill(&mut random).map_err(|_| anyhow::anyhow!("BUD attempt entropy unavailable"))?;
    Ok(random
        .as_chunks::<4>()
        .0
        .iter()
        .map(|part| {
            format!(
                "{:02x}{:02x}{:02x}{:02x}",
                part[0], part[1], part[2], part[3]
            )
        })
        .collect::<Vec<_>>()
        .join("-"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn timing_reports_bypass_application_events_and_retire_with_their_generation() {
        let bus = std::sync::Arc::new(crate::stats::Shared::default());
        let statistics = super::Statistics::start(bus.clone());
        bus.host_timing.set_requested(statistics.generation, true);
        let event = evoparseq_bud::bud::Event::ExternalControl {
            bytes: crate::frame_timing::tests::packet(1, 0, &[8., 0., 20.]),
        };
        for _ in 0..10_000 {
            assert!(statistics.consume_timing(&event));
        }
        assert_eq!(
            bus.host_timing
                .snapshot(statistics.generation)
                .audio
                .unwrap()
                .count,
            10_000
        );
        assert!(!statistics
            .consume_timing(&evoparseq_bud::bud::Event::ExternalControl { bytes: vec![0; 13] }));
        let newer = super::Statistics::start(bus.clone());
        bus.host_timing.set_requested(newer.generation, true);
        assert!(statistics.consume_timing(&event));
        drop(statistics);
        let report = bus.host_timing.snapshot(newer.generation);
        assert!(report.requested);
        assert!(report.audio.is_none());
    }
    use super::*;
    #[test]
    fn turn_urls_prefer_udp_then_tls_then_tcp80_without_rewriting_ports() {
        use evoparseq_bud::bud::RelayTransport;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let routes = runtime
            .block_on(turn_relays(vec![
                webrtc::ice_transport::ice_server::RTCIceServer {
                    urls: vec![
                        "turn:127.0.0.1:3478?transport=tcp".into(),
                        "turn:127.0.0.1:80?transport=tcp".into(),
                        "turns:localhost:443?transport=tcp".into(),
                        "stun:127.0.0.1:3478".into(),
                        "turn:127.0.0.1:3478?transport=udp".into(),
                    ],
                    username: "isolated".into(),
                    credential: "fixture-only".into(),
                },
            ]))
            .unwrap();
        let relays: Vec<_> = routes
            .into_iter()
            .map(|route| match route {
                Route::Turn(relay) => relay,
                _ => panic!("TURN URL leaked a direct route"),
            })
            .collect();
        assert_eq!(relays.len(), 4);
        assert_eq!(
            relays.iter().map(|r| r.server.port()).collect::<Vec<_>>(),
            [3478, 443, 80, 3478]
        );
        assert_eq!(relays[0].transport, RelayTransport::Udp);
        assert_eq!(
            relays[1].transport,
            RelayTransport::Tls {
                server_name: "localhost".into()
            }
        );
        assert_eq!(relays[2].transport, RelayTransport::Tcp);
        assert_eq!(relays[3].transport, RelayTransport::Tcp);
    }
    fn prepare_offer(attempt: &Attempt, text: &str, writer: SignalingWrite) {
        let mut state = attempt.state.lock().unwrap();
        state.template = Some(serde_json::from_str(text).unwrap());
        state.writer = Some(writer);
    }
    fn isolated() -> (Attempt, mpsc::Receiver<String>) {
        let (commands, receiver) = mpsc::sync_channel(2);
        (
            Attempt {
                id: "isolated".into(),
                state: Arc::new(Mutex::new(State {
                    ..Default::default()
                })),
                session: Default::default(),
                audio: Default::default(),
                commands,
                cancel: Default::default(),
                thread: std::thread::current(),
                hid_input: (false, false),

                worker: None,
            },
            receiver,
        )
    }
    #[test]
    fn old_bud_shutdown_cannot_replace_new_stats_or_show_webrtc_fields() {
        let shared = Arc::new(crate::stats::Shared::default());
        let old = Statistics::start(shared.clone());
        let current = Statistics::start(shared.clone());
        let generation = current.generation;
        drop(old);
        let sample = shared.read().unwrap();
        assert_eq!(sample.generation, generation);
        assert!(sample.state.starts_with("Connecting"));
        assert!(sample.bud.is_some());
        drop(current);
        let sample = shared.read().unwrap();
        assert!(sample.state.starts_with("Disconnected"));
        assert!(!sample.bud.unwrap().connected);
        assert!(sample.video.is_none());
    }

    #[test]
    fn native_answer_queues_credentials_only_for_the_active_worker() {
        let (attempt, incoming) = isolated();
        let wire = json!({"action":"answer_relay","payload":{"attempt_id":"isolated","approved":true,"data":{"mode":1,"creds":{"aes256":"native-key-sentinel","ice_ufrag":"abc"}}}}).to_string();
        attempt.incoming(&wire).unwrap();
        assert_eq!(incoming.try_recv().unwrap(), wire);
        attempt
            .incoming(r#"{"action":"answer_relay","payload":{"attempt_id":"stale"}}"#)
            .unwrap();
        assert!(incoming.try_recv().is_err());
    }
    #[test]
    fn relay_retry_withdraws_old_offer_preserves_authorization_and_ignores_old_answer() {
        let (attempt, incoming) = isolated();
        attempt.state.lock().unwrap().wire_id = "isolated".into();
        let writes = Arc::new(Mutex::new(Vec::<Value>::new()));
        let collected = writes.clone();
        let writer: SignalingWrite = Arc::new(move |text| {
            collected
                .lock()
                .unwrap()
                .push(serde_json::from_str(&text).unwrap());
            true
        });
        let offer = json!({"action":"offer","payload":{"attempt_id":"isolated","to":"real-host","secret":"authorization-sentinel","data":{"mode":2}}}).to_string();
        prepare_offer(&attempt, &offer, writer);
        assert!(switch_offer(
            &attempt.state,
            &attempt.cancel,
            "isolated-turn-1",
            json!({"mode":1,"creds":{"aes256":"new-native-key"}})
        )
        .unwrap());
        let sent = writes.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0]["action"], "offer_cancel");
        assert_eq!(sent[0]["payload"]["attempt_id"], "isolated");
        assert_eq!(sent[1]["action"], "offer");
        assert_eq!(sent[1]["payload"]["attempt_id"], "isolated-turn-1");
        assert_eq!(sent[1]["payload"]["secret"], "authorization-sentinel");
        drop(sent);
        attempt.incoming(r#"{"action":"answer_relay","payload":{"attempt_id":"isolated","approved":true,"data":{}}}"#).unwrap();
        assert!(incoming.try_recv().is_err());
        let response = json!({"action":"answer_relay","payload":{"attempt_id":"isolated-turn-1","approved":true,"data":{"mode":1,"creds":{"aes256":"key"}}}}).to_string();
        attempt.incoming(&response).unwrap();
        assert_eq!(incoming.try_recv().unwrap(), response);
        attempt.state.lock().unwrap().established = true;
        withdraw_pending(&attempt.state);
        assert_eq!(writes.lock().unwrap().len(), 2);
        assert!(attempt.state.lock().unwrap().template.is_some());
        attempt.state.lock().unwrap().established = false;
        send_candidate(&attempt.state, &Event::Ready).unwrap();
        let sent = writes.lock().unwrap();
        assert_eq!(sent[2]["payload"]["to"], "real-host");
        assert_eq!(sent[2]["payload"]["attempt_id"], "isolated-turn-1");
        assert_eq!(sent[2]["payload"]["data"]["sync"], true);
        drop(sent);
        withdraw_pending(&attempt.state);
        withdraw_pending(&attempt.state);
        let sent = writes.lock().unwrap();
        assert_eq!(sent.len(), 4, "pending offer is withdrawn only once");
        assert_eq!(sent[3]["action"], "offer_cancel");
        assert_eq!(sent[3]["payload"]["attempt_id"], "isolated-turn-1");
    }

    #[test]
    fn direct_close_is_owned_by_worker_and_cannot_cancel_consumer_before_relay_retry() {
        let (attempt, incoming) = isolated();
        let close = r#"{"action":"close","payload":{"attempt_id":"isolated"}}"#;
        attempt.incoming(close).unwrap();
        let queued = incoming.try_recv().unwrap();
        assert!(native_close(&queued, "isolated"));
        assert!(!native_close(&queued, "isolated-turn-1"));
        assert!(!attempt.cancel.load(Ordering::Acquire));
        assert!(attempt.failure().is_none());
        // A late close from the retired direct route must not terminate relay.
        {
            let mut state = attempt.state.lock().unwrap();
            state.retired_wires.push("isolated".into());
            state.wire_id = "isolated-turn-1".into();
        }
        attempt.incoming(close).unwrap();
        assert!(incoming.try_recv().is_err());
        let relay_close = r#"{"action":"close","payload":{"attempt_id":"isolated-turn-1"}}"#;
        attempt.incoming(relay_close).unwrap();
        assert!(native_close(
            &incoming.try_recv().unwrap(),
            "isolated-turn-1"
        ));
    }

    #[test]
    fn late_signals_from_all_retired_carriers_are_suppressed() {
        let (attempt, incoming) = isolated();
        let offer = json!({"action":"offer","payload":{"attempt_id":"isolated","to":"fixture-host","data":{}}}).to_string();
        attempt.state.lock().unwrap().wire_id = "isolated".into();
        prepare_offer(&attempt, &offer, Arc::new(|_| true));
        for current in ["udp-attempt", "tls-attempt", "tcp-attempt"] {
            assert!(switch_offer(
                &attempt.state,
                &attempt.cancel,
                current,
                json!({"creds":{}})
            )
            .unwrap());
        }
        for retired in ["isolated", "udp-attempt", "tls-attempt"] {
            for action in ["close", "answer_relay", "candex_relay"] {
                let signal = json!({"action":action,"payload":{"attempt_id":retired}}).to_string();
                attempt.incoming(&signal).unwrap();
            }
        }
        assert!(incoming.try_recv().is_err());
        let state = attempt.state.lock().unwrap();
        assert_eq!(state.wire_id, "tcp-attempt");
        assert_eq!(state.retired_wires.len(), 3);
    }

    #[test]
    fn malformed_active_answer_fails_without_mutating_consumer_or_queue() {
        let (attempt, incoming) = isolated();
        assert!(attempt.incoming(r#"{"action":"answer_relay","payload":{"attempt_id":"isolated","approved":true,"data":42}}"#).is_err());
        assert!(incoming.try_recv().is_err());
    }

    #[test]
    fn bud_is_explicit_and_default_settings_remain_webrtc() {
        let settings: crate::connection_settings::Settings = serde_json::from_str("{}").unwrap();
        assert!(!settings.bud);
        let settings: crate::connection_settings::Settings =
            serde_json::from_str(r#"{"bud":true}"#).unwrap();
        assert!(settings.bud);
    }

    #[test]
    fn native_offer_uses_existing_account_envelope() {
        let writes = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured = writes.clone();
        let writer: SignalingWrite = Arc::new(move |text| {
            captured
                .lock()
                .unwrap()
                .push(serde_json::from_str(&text).unwrap());
            true
        });
        let state = Mutex::new(State {
            writer: Some(writer),
            template: Some(json!({"version":1,"action":"offer","payload":{
                "to":"fixture-host","attempt_id":"fixture-attempt","secret":"","data":{}}})),
            ..Default::default()
        });
        let cancel = AtomicBool::new(false);
        assert!(
            complete_offer(&state, &cancel, json!({"mode":1,"creds":{"fixture":true}})).unwrap()
        );
        let sent = writes.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["payload"]["to"], "fixture-host");
        assert_eq!(sent[0]["payload"]["data"]["mode"], 1);
        assert!(sent[0]["payload"]["data"]["creds"]["fixture"]
            .as_bool()
            .unwrap());
    }

    #[test]
    fn relay_retry_uses_fresh_native_account_identifier_shape() {
        let first = native_attempt_id().unwrap();
        let second = native_attempt_id().unwrap();
        assert_ne!(first, second);
        for id in [&first, &second] {
            assert_eq!(id.len(), 53);
            let groups: Vec<_> = id.split('-').collect();
            assert_eq!(groups.len(), 6);
            assert!(groups.iter().all(
                |group| group.len() == 8 && group.bytes().all(|byte| byte.is_ascii_hexdigit())
            ));
        }
        // Check the actual wire handoff, not just the random generator.
        let (attempt, _) = isolated();
        attempt.state.lock().unwrap().wire_id = "isolated".into();
        let writes = Arc::new(Mutex::new(Vec::<Value>::new()));
        let collected = writes.clone();
        let writer: SignalingWrite = Arc::new(move |text| {
            collected
                .lock()
                .unwrap()
                .push(serde_json::from_str(&text).unwrap());
            true
        });
        let offer = json!({"action":"offer","payload":{"attempt_id":"isolated","to":"host","secret":"authorization-sentinel","data":{"mode":2}}}).to_string();
        prepare_offer(&attempt, &offer, writer);
        switch_offer(&attempt.state, &attempt.cancel, &first, json!({"mode":1})).unwrap();
        send_candidate(&attempt.state, &Event::Ready).unwrap();
        let writes = writes.lock().unwrap();
        assert_eq!(writes[1]["payload"]["attempt_id"], first);
        assert_eq!(writes[2]["payload"]["attempt_id"], first);
        assert_eq!(writes[1]["payload"]["secret"], "authorization-sentinel");
        drop(writes);
        attempt
            .incoming(r#"{"action":"close","payload":{"attempt_id":"isolated"}}"#)
            .unwrap();
    }
}
