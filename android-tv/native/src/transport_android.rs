//! Android transport lifecycle; codecs and host settings stay shared.
use crate::{
    attempt::{Attempt, Output, StunProvider},
    audio_stream, control, native_wire,
    native_wire::SignalingWrite,
    video_android, video_stream,
};
use anyhow::{ensure, Result};
use bytes::Bytes;
use evoparseq_bud::{
    bud::{Event, MediaSource, Route, Session},
    media,
    signaling::Signaling,
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    net::ToSocketAddrs,
    sync::Arc,
    time::{Duration, Instant},
};

pub enum Transport {
    WebRtc(Box<Attempt>),
    Bud(Box<Bud>),
}
pub struct Bud {
    session: Session,
    signaling: Signaling,
    source: MediaSource,
    audio: audio_stream::Pipeline,
    sender: audio_stream::Sender,
    video: Arc<video_android::Pipeline>,
    inspector: video_stream::Inspector,
    controls: VecDeque<Bytes>,
    started: Instant,
    established: bool,
    waiting_for_keyframe: bool,
}
impl Transport {
    pub fn start(
        id: &str,
        peer: &str,
        writer: SignalingWrite,
        config: &control::Config,
        bud: bool,
        video: Arc<video_android::Pipeline>,
    ) -> Result<Self> {
        if !bud {
            let target = peer.to_owned();
            let offer_id = id.to_owned();
            let output = Output::new(Arc::new(move |description| {
                Ok(description.is_some_and(|d| {
                    writer(native_wire::offer(&target, &offer_id, &d.credentials))
                }))
            }));
            let attempt = Attempt::spawn_configured(
                id,
                output,
                Some(config.clone()),
                StunProvider::Parsec,
                true,
                None,
            )?;
            attempt.start_android_media(video);
            return Ok(Self::WebRtc(Box::new(attempt)));
        }
        let mut session = Session::external_video_depth(config.allow_hevc, config.ten_bit)?;
        session.wake_consumer(std::thread::current());
        let stun = ("stun.parsec.gg", 3478).to_socket_addrs()?.collect();
        let credentials = session.offer(id, Route::Direct { stun })?;
        let source = session.take_media()?;
        let signaling = Signaling::new(peer, id)?;
        ensure!(
            writer(signaling.offer(&credentials).to_string()),
            "Could not send BUD offer"
        );
        let audio = audio_stream::Pipeline::start_with_budget(Some(100));
        let sender = audio
            .sender()
            .ok_or_else(|| anyhow::anyhow!("Audio unavailable"))?;
        let mut inspector = video_stream::Inspector::default();
        inspector.enable_hevc(config.allow_hevc);
        Ok(Self::Bud(Box::new(Bud {
            session,
            signaling,
            source,
            audio,
            sender,
            video,
            inspector,
            controls: VecDeque::new(),
            started: Instant::now(),
            established: false,
            waiting_for_keyframe: true,
        })))
    }
    pub fn receive(&mut self, text: &str, id: &str) -> Result<()> {
        match self {
            Self::WebRtc(attempt) => ensure!(
                !native_wire::incoming(text, id, attempt)?,
                "The host closed the connection"
            ),
            Self::Bud(bud) => bud.signaling.receive(text, &mut bud.session)?,
        }
        Ok(())
    }
    pub fn poll(&mut self, writer: &SignalingWrite, peer: &str, id: &str) -> Result<()> {
        match self {
            Self::WebRtc(attempt) => {
                for _ in 0..64 {
                    let Some(event) = attempt.pop_event() else {
                        break;
                    };
                    if event["type"] == 8 {
                        ensure!(
                            writer(native_wire::candidate(&event, peer, id)),
                            "Could not send network candidate"
                        );
                    }
                }
            }
            Self::Bud(bud) => {
                for _ in 0..64 {
                    let Some(event) = bud.session.poll()? else {
                        break;
                    };
                    if let Some(candidate) = bud.signaling.candidate(&event) {
                        ensure!(
                            writer(candidate.to_string()),
                            "Could not send BUD candidate"
                        );
                    }
                    match event {
                        Event::Established { .. } => bud.established = true,
                        Event::ExternalControl { bytes } => {
                            ensure!(bud.controls.len() < 64, "BUD control queue full");
                            bud.controls.push_back(Bytes::from(bytes));
                        }
                        Event::Ended { outcome } => {
                            anyhow::bail!("BUD connection ended: {outcome:?}")
                        }
                        Event::IoFailure { stage, kind, .. } => {
                            anyhow::bail!("BUD IO failed: {} ({kind:?})", stage.name())
                        }
                        _ => {}
                    }
                }
                ensure!(
                    bud.established || bud.started.elapsed() < Duration::from_secs(30),
                    "BUD connection timed out"
                );
                for _ in 0..8 {
                    let inspector = &mut bud.inspector;
                    let waiting = &mut bud.waiting_for_keyframe;
                    let video = &bud.video;
                    if bud
                        .source
                        .with_video(|bytes| {
                            match media::video(bytes)? {
                                media::Video::Metadata(metadata) => {
                                    if metadata.rebuilt {
                                        *waiting = true;
                                    }
                                }
                                media::Video::AccessUnit { annex_b, header } => {
                                    ensure!(
                                        matches!(
                                            header.rotation,
                                            media::Rotation::None | media::Rotation::Unknown
                                        ),
                                        "Rotated BUD video is not supported on Android TV yet"
                                    );
                                    if let Some(info) = inspector.receive(annex_b) {
                                        if !*waiting || info.idr || info.parameters {
                                            let idr = info.idr;
                                            if video.submit(annex_b, info) {
                                                if idr {
                                                    *waiting = false;
                                                }
                                            } else {
                                                *waiting = true;
                                            }
                                        }
                                    }
                                }
                            }
                            Ok(())
                        })?
                        .is_none()
                    {
                        break;
                    }
                }
                for _ in 0..8 {
                    let sender = &bud.sender;
                    if bud
                        .source
                        .with_audio(|bytes| {
                            sender.submit(media::opus(bytes)?);
                            Ok(())
                        })?
                        .is_none()
                    {
                        break;
                    }
                }
            }
        }
        Ok(())
    }
    pub fn audio_receiver(&self) -> Option<audio_stream::NativeReceiver> {
        match self {
            Self::WebRtc(a) => a.audio_receiver(),
            Self::Bud(b) => Some(b.audio.native_receiver()),
        }
    }
    pub fn control_ready(&self) -> bool {
        match self {
            Self::WebRtc(a) => a.control_ready(),
            Self::Bud(b) => b.established,
        }
    }
    pub fn pop_binary(&mut self) -> Option<(u16, bool, Bytes)> {
        match self {
            Self::WebRtc(a) => a.pop_binary(),
            Self::Bud(b) => b.controls.pop_front().map(|b| (0, false, b)),
        }
    }
    pub fn send_binary(&mut self, channel: u16, bytes: Bytes) -> Result<()> {
        match self {
            Self::WebRtc(a) => a.send_binary(channel, bytes),
            Self::Bud(b) => {
                ensure!(channel == 0, "Invalid BUD control channel");
                b.session.send_frame(&bytes)
            }
        }
    }
    pub fn keyframe(&self, config: &control::Config) -> Result<()> {
        match self {
            Self::WebRtc(a) => a.send_binary(0, config.keyframe_request()),
            Self::Bud(b) => {
                ensure!(
                    b.session.request_keyframe_format(false),
                    "BUD keyframe request failed"
                );
                Ok(())
            }
        }
    }
    pub fn host_status(&self) -> Option<i32> {
        match self {
            Self::WebRtc(a) => a.host_status(),
            Self::Bud(_) => None,
        }
    }
    pub fn failure_stage(&self) -> Option<crate::attempt::FailureStage> {
        match self {
            Self::WebRtc(a) => a.failure_stage(),
            Self::Bud(_) => None,
        }
    }
    pub fn network_diagnostics(&self) -> Value {
        match self {
            Self::WebRtc(a) => a.network_diagnostics(),
            Self::Bud(b) => {
                let s = b.session.snapshot();
                json!({"transport":"BUD / UDP", "state":if s.connected {"Connected"} else {"Connecting"}, "received_bytes":[s.udp_received], "rtt_ms":s.rtt_ms})
            }
        }
    }
    pub fn disconnect(&mut self) {
        match self {
            Self::WebRtc(a) => a.disconnect(),
            Self::Bud(b) => b.session.close(),
        }
    }
}
