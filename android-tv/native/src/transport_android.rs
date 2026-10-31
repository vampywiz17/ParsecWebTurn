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
    network: Arc<crate::network::Network>,
    config: control::Config,
    writer: SignalingWrite,
    peer: String,
    base_id: String,
    wire_id: String,
    routes: Option<VecDeque<Route>>,
    retries: usize,
}
impl Transport {
    pub fn start(
        id: &str,
        peer: &str,
        writer: SignalingWrite,
        config: &control::Config,
        bud: bool,
        video: Arc<video_android::Pipeline>,
        network: Arc<crate::network::Network>,
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
                Some(network),
            )?;
            attempt.start_android_media(video);
            return Ok(Self::WebRtc(Box::new(attempt)));
        }
        let mut session = Session::external_video_depth(config.allow_hevc, config.ten_bit)?;
        session.wake_consumer(std::thread::current());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (route, routes) = if network.force_relay {
            let mut routes = runtime.block_on(async {
                crate::bud_routes::turn_relays(network.servers().await?).await
            })?;
            (
                routes
                    .pop_front()
                    .ok_or_else(|| anyhow::anyhow!("No TURN route"))?,
                Some(routes),
            )
        } else {
            let urls = network
                .stun_urls
                .iter()
                .map(|s| webrtc::ice::url::Url::parse_url(s))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let addresses = runtime.block_on(futures_util::future::join_all(urls.into_iter().map(
                |u| async move {
                    tokio::time::timeout(
                        Duration::from_secs(3),
                        tokio::net::lookup_host((u.host.as_str(), u.port)),
                    )
                    .await
                    .ok()
                    .and_then(|v| v.ok())
                    .map(|v| v.collect::<Vec<_>>())
                    .unwrap_or_default()
                },
            )));
            (
                Route::Direct {
                    stun: addresses.into_iter().flatten().collect(),
                },
                None,
            )
        };
        let credentials = session.offer(id, route)?;
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
            network,
            config: config.clone(),
            writer,
            peer: peer.into(),
            base_id: id.into(),
            wire_id: id.into(),
            routes,
            retries: 0,
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
                            if !bud.established && bud.retry()? {
                                break;
                            }
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
                json!({"transport":if s.connected { format!("BUD / {}",s.relay_transport.unwrap_or("udp").to_uppercase()) } else { "BUD / connecting".into() }, "state":if s.connected {"Connected"} else {"Connecting"}, "received_bytes":[s.udp_received], "rtt_ms":s.rtt_ms,"route":if !s.connected { "Unverified" } else if s.relay { "Relay — TURN in use" } else { "Direct — no TURN" }})
            }
        }
    }
    pub fn wire_id<'a>(&'a self, fallback: &'a str) -> &'a str {
        match self {
            Self::WebRtc(_) => fallback,
            Self::Bud(b) => &b.wire_id,
        }
    }
    pub fn disconnect(&mut self) {
        match self {
            Self::WebRtc(a) => a.disconnect(),
            Self::Bud(b) => b.session.close(),
        }
    }
}

impl Bud {
    fn retry(&mut self) -> Result<bool> {
        if self.network.provider == "direct" {
            return Ok(false);
        }
        if self.routes.is_none() {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            self.routes = Some(runtime.block_on(async {
                crate::bud_routes::turn_relays(self.network.servers().await?).await
            })?);
        }
        let Some(route) = self.routes.as_mut().and_then(|r| r.pop_front()) else {
            return Ok(false);
        };
        (self.writer)(
            native_wire::envelope("offer_cancel", &self.peer, &self.wire_id, json!({})).to_string(),
        );
        self.session.close();
        let mut session =
            Session::external_video_depth(self.config.allow_hevc, self.config.ten_bit)?;
        session.wake_consumer(std::thread::current());
        self.retries += 1;
        self.wire_id = format!("{}-relay-{}", self.base_id, self.retries);
        let credentials = session.offer(&self.wire_id, route)?;
        self.source = session.take_media()?;
        self.signaling = Signaling::new(&self.peer, &self.wire_id)?;
        ensure!(
            (self.writer)(self.signaling.offer(&credentials).to_string()),
            "Could not send relay offer"
        );
        self.session = session;
        self.started = Instant::now();
        self.controls.clear();
        self.waiting_for_keyframe = true;
        Ok(true)
    }
}
