//! Android lifecycle adapter around the existing authenticated WebRTC engine.
use crate::{
    account::Session,
    account_signaling::Socket,
    attempt::{Attempt, Output, StunProvider},
    audio_android, native_wire,
    preferences::Preferences,
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
        prefs.transport == 0,
        "BUD streaming is not yet available on Android TV. Select WebRTC in Settings."
    );
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
    attempt: Attempt,
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
    let writer = socket.writer.clone();
    let target = peer.to_owned();
    let offer_id = id.clone();
    let output = Output::new(Arc::new(move |description| {
        Ok(description
            .is_some_and(|d| writer(native_wire::offer(&target, &offer_id, &d.credentials))))
    }));
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
    let attempt = Attempt::spawn_configured(
        &id,
        output,
        Some(config.clone()),
        StunProvider::Parsec,
        true,
        None,
    )?;
    attempt.start_android_media(video.clone());
    let connection = Connection {
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
    while !stop.load(Ordering::Acquire) {
        if !signaling_closed {
            for _ in 0..64 {
                match connection.socket.next() {
                    Ok(Some(text)) => ensure!(
                        !native_wire::incoming(&text, &connection.id, &connection.attempt)?,
                        "The host closed the connection"
                    ),
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
        for _ in 0..64 {
            let Some(event) = connection.attempt.pop_event() else {
                break;
            };
            if event["type"] == 8 {
                ensure!(
                    (connection.socket.writer)(native_wire::candidate(
                        &event,
                        &connection.peer,
                        &connection.id
                    )),
                    "Could not send a network candidate"
                );
            }
        }
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
        ensure!(
            connection.attempt.failure_stage().is_none(),
            "WebRTC connection failed"
        );
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
            connection
                .attempt
                .send_binary(0, config.keyframe_request())?;
        }
        audio.check()?;
        if update.elapsed() >= Duration::from_millis(500) {
            let snapshot = connection.video.snapshot();
            *status.lock().unwrap_or_else(|e| e.into_inner()) = json!({"state":if snapshot.submitted>0 {"Streaming"} else {"Connecting…"},"ended":false,"video":snapshot,"settings_confirmed":settings_confirmed,
                "audio":receiver.snapshot(),"audio_output":audio.snapshot()});
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
