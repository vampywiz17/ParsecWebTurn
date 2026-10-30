//! Android lifecycle adapter around the existing authenticated WebRTC engine.
use crate::{
    account::Session,
    account_signaling::Socket,
    audio_android, native_wire,
    preferences::Preferences,
    transport_android::Transport,
    video_android::{Choices, NativeWindow, Pipeline},
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Running {
    stop: Arc<AtomicBool>,
    thread: thread::Thread,
    status: Arc<Mutex<Value>>,
}
static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
struct Request {
    session: Arc<Session>,
    peer: String,
    prefs: Preferences,
    window: NativeWindow,
    choices: Choices,
    size: (u32, u32),
}

pub fn start(
    session: Arc<Session>,
    peer: String,
    prefs: Preferences,
    window: NativeWindow,
    choices: Choices,
    size: (u32, u32),
) -> Result<()> {
    ensure!(
        !peer.is_empty() && peer.len() <= 256 && !peer.chars().any(char::is_control),
        "Invalid computer ID"
    );
    ensure!(
        prefs.video == 0 || choices.hevc.is_some(),
        "This device has no declared hardware HEVC decoder"
    );
    ensure!(
        prefs.video != 2 || choices.main10,
        "This device has no declared Main 10 decoder"
    );
    let mut slot = RUNNING
        .lock()
        .map_err(|_| anyhow::anyhow!("Stream unavailable"))?;
    if let Some(old) = slot.as_ref() {
        ensure!(
            old.stop.load(Ordering::Acquire)
                && old.status.lock().unwrap_or_else(|e| e.into_inner())["ended"] == true,
            "The previous session is still closing. Retry shortly."
        );
    }
    let stop = Arc::new(AtomicBool::new(false));
    let status = Arc::new(Mutex::new(json!({"state":"Connecting…","ended":false})));
    let worker_stop = stop.clone();
    let worker_status = status.clone();
    let handle = thread::Builder::new()
        .name("android-session".into())
        .spawn(move || {
            let result = run(
                Request {
                    session,
                    peer,
                    prefs,
                    window,
                    choices,
                    size,
                },
                &worker_stop,
                &worker_status,
            );
            worker_stop.store(true, Ordering::Release);
            let mut report = worker_status.lock().unwrap_or_else(|e| e.into_inner());
            report["ended"] = true.into();
            match result {
                Ok(()) => report["state"] = "Disconnected".into(),
                Err(error) => {
                    report["state"] = "Stream stopped".into();
                    report["error"] = error.to_string().into();
                }
            }
        })?;
    *slot = Some(Running {
        stop,
        thread: handle.thread().clone(),
        status,
    });
    Ok(())
}
pub fn stop() {
    if let Ok(slot) = RUNNING.lock() {
        if let Some(r) = slot.as_ref() {
            r.stop.store(true, Ordering::Release);
            r.thread.unpark();
        }
    }
}
pub fn status() -> Value {
    RUNNING
        .lock()
        .ok()
        .and_then(|s| {
            s.as_ref()
                .map(|r| r.status.lock().unwrap_or_else(|e| e.into_inner()).clone())
        })
        .unwrap_or_else(|| json!({"state":"Disconnected","ended":true}))
}
struct Connection {
    attempt: Transport,
    socket: Socket,
    peer: String,
    id: String,
    video: Arc<Pipeline>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        (self.socket.writer)(
            native_wire::envelope("offer_cancel", &self.peer, &self.id, json!({})).to_string(),
        );
        self.attempt.disconnect();
        self.video.stop();
    }
}
fn run(request: Request, stop: &AtomicBool, status: &Mutex<Value>) -> Result<()> {
    let Request {
        session,
        peer,
        prefs,
        window,
        choices,
        size,
    } = request;
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    let socket = Socket::open(&session)?;
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    let id = format!(
        "tv-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let config = crate::control::Config {
        video_protocol: crate::video_stream::VideoProtocol {
            version: 1,
            message_size: 11,
            version_offset: 0,
            flag_offset: 4,
        },
        allow_hevc: prefs.video > 0,
        ten_bit: prefs.video == 2,
        chroma_444: false,
    };
    let video = Pipeline::start(window, choices)?;
    let attempt = Transport::start(
        &id,
        &peer,
        socket.writer.clone(),
        &config,
        prefs.transport == 1,
        video.clone(),
    )?;
    let mut connection = Connection {
        attempt,
        socket,
        peer,
        id,
        video,
    };
    let receiver = connection
        .attempt
        .audio_receiver()
        .context("Audio pipeline unavailable")?;
    let audio = audio_android::Output::open(receiver.clone())?;
    let mut configured = false;
    let mut queried = false;
    let mut host_config = crate::host_video::Config::default();
    let mut confirmation = None;
    let mut settings_confirmed = false;
    let mut signaling_closed = false;
    let started = Instant::now();
    let mut update = Instant::now();
    let mut previous_received_bytes = 0u64;
    let mut received_mbps = 0.;
    while !stop.load(Ordering::Acquire) {
        if !signaling_closed {
            for _ in 0..64 {
                match connection.socket.next() {
                    Ok(Some(text)) => connection.attempt.receive(&text, &connection.id)?,
                    Ok(None) => break,
                    Err(error) => {
                        if !connection.attempt.control_ready() {
                            return Err(error);
                        }
                        signaling_closed = true;
                        break;
                    }
                }
            }
        }
        connection
            .attempt
            .poll(&connection.socket.writer, &connection.peer, &connection.id)?;
        // Reuse the Windows host-settings model: query, preserve other fields,
        // write once after the reply, then verify with bounded read-only polls.
        for _ in 0..64 {
            let Some((channel, text, frame)) = connection.attempt.pop_binary() else {
                break;
            };
            ensure!(channel == 0 && !text, "Invalid session control channel");
            match crate::control::decode(&frame)? {
                crate::control::Message::Status(code) if code != 0 => {
                    anyhow::bail!("The host ended the stream (status {code}). Check the host encoder and requested video format.");
                }
                crate::control::Message::Buffer {
                    event,
                    payload: Some(range),
                } if event["type"] == 3 && event["id"] == 11 => {
                    let bytes = &frame[range];
                    host_config.receive(bytes.strip_suffix(&[0]).unwrap_or(bytes))?;
                }
                _ => {}
            }
        }
        if let Some(code) = connection.attempt.host_status().filter(|c| *c != 0) {
            anyhow::bail!("The host ended the stream (status {code}). Check the host encoder and requested video format.");
        }
        if let Some(stage) = connection.attempt.failure_stage() {
            let mut network = connection.attempt.network_diagnostics();
            network["received_mbps"] = received_mbps.into();
            // Retain the failure evidence before Connection::drop closes the
            // peer and changes its native state enums to Closed.
            let mut report = status.lock().unwrap_or_else(|e| e.into_inner());
            report["network"] = network;
            report["video"] = serde_json::to_value(connection.video.snapshot())?;
            report["audio"] = serde_json::to_value(receiver.snapshot())?;
            report["audio_output"] = serde_json::to_value(audio.snapshot())?;
            anyhow::bail!("WebRTC connection failed: {stage}");
        }
        if connection.attempt.control_ready() && !queried {
            connection
                .attempt
                .send_binary(0, crate::host_video::query()?)?;
            queried = true;
        }
        if connection.attempt.control_ready() && host_config.ready() && !configured {
            let request = host_config.request(&prefs.host_request()?, size)?;
            confirmation = Some(crate::host_video::Confirmation::new(
                &request,
                Instant::now(),
            )?);
            connection.attempt.send_binary(0, request)?;
            configured = true;
        }
        if let Some(check) = confirmation.as_mut() {
            match check.poll(&host_config, Instant::now()) {
                crate::host_video::Verification::Query => connection
                    .attempt
                    .send_binary(0, crate::host_video::query()?)?,
                crate::host_video::Verification::Confirmed => {
                    settings_confirmed = true;
                    confirmation = None;
                }
                crate::host_video::Verification::TimedOut => {
                    confirmation = None;
                }
                crate::host_video::Verification::Waiting => {}
            }
        }
        if configured && connection.video.take_keyframe_request() {
            connection.attempt.keyframe(&config)?;
        }
        audio.check()?;
        if update.elapsed() >= Duration::from_millis(500) {
            let snapshot = connection.video.snapshot();
            let mut network = connection.attempt.network_diagnostics();
            let bytes = network["received_bytes"]
                .as_array()
                .map(|v| v.iter().filter_map(Value::as_u64).sum::<u64>())
                .unwrap_or(0);
            received_mbps = bytes.saturating_sub(previous_received_bytes) as f64 * 8.
                / update.elapsed().as_secs_f64()
                / 1_000_000.;
            previous_received_bytes = bytes;
            network["received_mbps"] = received_mbps.into();
            *status.lock().unwrap_or_else(|e| e.into_inner()) = json!({"state":if snapshot.submitted>0 {"Streaming"} else {"Connecting…"},"ended":false,"video":snapshot,"settings_confirmed":settings_confirmed,
                "audio":receiver.snapshot(),"audio_output":audio.snapshot(),"network":network});
            if let Some(error) = snapshot.error.as_ref() {
                anyhow::bail!("{error}");
            }
            ensure!(
                snapshot.submitted > 0 || started.elapsed() < Duration::from_secs(30),
                "No video output received within 30 seconds"
            );
            update = Instant::now();
        }
        thread::park_timeout(Duration::from_millis(25));
    }
    Ok(())
}
