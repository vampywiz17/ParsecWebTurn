//! BUD lifecycle below native account signaling.
use std::net::SocketAddr;

use anyhow::{Result, anyhow, ensure};
pub use lowlat_client::config::RelayTransport;
use lowlat_client::config::{Port, Relay};
#[cfg(feature = "native-core")]
pub use lowlat_client::cursor::{Cache as CursorCache, Shape as CursorShape};
use lowlat_client::driver::Units;
use lowlat_client::{Client, Config, Peer, Transport};
pub use lowlat_client::{Event, Outcome};
use lowlat_core::conn::Kind;
use lowlat_crypto::Credentials;

/// Sanitized native setup failure. No credentials, addresses or peer text.
#[derive(Debug)]
pub struct AnswerFailure(lowlat_client::event::Error);
impl AnswerFailure {
    pub fn code(&self) -> &'static str {
        use lowlat_client::event::Error;
        match self.0 {
            Error::Busy => "busy",
            Error::UnknownAttempt => "unknown_attempt",
            Error::AlreadyBegun => "already_begun",
            Error::Transport => "transport",
            Error::Credentials => "credentials",
            Error::Crypto => "crypto",
            Error::Io { .. } => "io",
            Error::NoSession => "no_session",
            Error::Unsupported => "unsupported",
        }
    }
    pub fn io_details(&self) -> Option<(&'static str, std::io::ErrorKind, Option<i32>)> {
        match self.0 {
            lowlat_client::event::Error::Io {
                stage,
                kind,
                os_error,
            } => Some((stage.name(), kind, os_error)),
            _ => None,
        }
    }
}
impl std::fmt::Display for AnswerFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for AnswerFailure {}

/// One explicit route per attempt. No implicit transport or provider fallback.
pub enum Route {
    Direct { stun: Vec<SocketAddr> },
    Turn(Relay),
}

impl Route {
    pub fn turn_udp(server: SocketAddr, username: &str, password: &str) -> Self {
        Self::turn(server, username, password, RelayTransport::Udp)
    }
    pub fn turn(
        server: SocketAddr,
        username: &str,
        password: &str,
        transport: RelayTransport,
    ) -> Self {
        let mut relay = Relay::new(server, username, password);
        relay.transport = transport;
        Self::Turn(relay)
    }
    pub fn with_tcp_qos(mut self, enabled: bool) -> Self {
        if let Self::Turn(relay) = &mut self {
            relay.tcp_qos = enabled;
        }
        self
    }
    fn configure(self) -> Config {
        match self {
            Self::Direct { stun } => Config {
                servers: stun,
                ..Config::default()
            },
            Self::Turn(relay) => Config {
                relay: Some(relay),
                ..Config::default()
            },
        }
    }
}

/// Application-owned native session, with a separate compressed-media consumer.
pub struct Session {
    client: Client,
    attempt: Option<String>,
    external_units: Option<Units>,
    prefer_hevc: bool,
    ten_bit: bool,
    chroma_444: bool,
    raw_audio: bool,
    audio_layout: Option<(u8, u32)>,
    hid_input: (bool, bool),
    cipher: Option<&'static str>,
}

impl Session {
    /// Native transport with application-owned video decode. The caller must
    /// have verified its existing MF decoder's HEVC support before setting it.
    /// This entry advertises 8-bit 4:2:0 only. No SDK video decoder is created.
    pub fn external_video(hevc_available: bool) -> Result<Self> {
        Self::external_video_depth(hevc_available, false)
    }

    pub fn external_video_depth(hevc_available: bool, ten_bit: bool) -> Result<Self> {
        Self::external_video_format(hevc_available, ten_bit, false)
    }
    pub fn external_video_format(
        hevc_available: bool,
        ten_bit: bool,
        chroma_444: bool,
    ) -> Result<Self> {
        let client = Client::new_external_video(
            lowlat_client::config::Caps {
                h264: true,
                hevc: hevc_available,
                hevc_10: hevc_available && ten_bit,
                hevc_444: hevc_available && chroma_444,
                hevc_444_10: hevc_available && chroma_444 && ten_bit,
            },
            Port::Any,
        )
        .map_err(|e| anyhow!("BUD external video: {e:?}"))?;
        let units = client.units();
        Ok(Self {
            client,
            attempt: None,
            external_units: Some(units),
            prefer_hevc: hevc_available,
            ten_bit: hevc_available && ten_bit,
            chroma_444: hevc_available && chroma_444,
            raw_audio: false,
            audio_layout: None,
            hid_input: (false, false),
            cipher: None,
        })
    }

    pub fn request_keyframe_format(&self, fallback_420: bool) -> bool {
        self.client.request_external_keyframe_format(fallback_420)
    }
    pub fn hid_input_state(&self) -> [(bool, u32); 2] {
        use std::sync::atomic::Ordering::Acquire;
        let t = self.client.telemetry();
        let connected = t.state.load(Acquire) == 1;
        [
            (
                connected && t.hid_mouse_active.load(Acquire),
                t.hid_mouse_error.load(Acquire),
            ),
            (
                connected && t.hid_keyboard_active.load(Acquire),
                t.hid_keyboard_error.load(Acquire),
            ),
        ]
    }
    pub fn enable_hid_input(&mut self, mouse: bool, keyboard: bool) {
        self.hid_input = (mouse, keyboard);
    }
    pub fn allow_raw_audio(&mut self, enabled: bool) {
        self.raw_audio = enabled;
    }
    pub fn set_audio_layout(&mut self, channels: u8, mask: u32) -> Result<()> {
        ensure!(
            matches!((channels, mask), (2, 3) | (6, 0x3f | 0x60f) | (8, 0x63f)),
            "unsupported BUD playback layout"
        );
        self.audio_layout = Some((channels, mask));
        Ok(())
    }

    pub fn take_media(&mut self) -> Result<MediaSource> {
        let units = self
            .external_units
            .take()
            .ok_or_else(|| anyhow!("BUD video consumer already taken"))?;
        let audio = self
            .client
            .take_external_audio_source()
            .map_err(|_| anyhow!("BUD audio consumer already taken"))?;
        Ok(MediaSource { units, audio })
    }

    pub fn wake_consumer(&mut self, thread: std::thread::Thread) {
        self.client.wake_external_consumer(thread);
    }

    pub fn send_control(&mut self, bytes: &[u8]) -> Result<()> {
        ensure!(
            self.client.send_external_control(bytes),
            "BUD control refused"
        );
        Ok(())
    }

    /// Route application text through BUD's variable-size user-data queue,
    /// preserving the short input/control queue and its existing size limit.
    pub fn send_frame(&mut self, bytes: &[u8]) -> Result<()> {
        use lowlat_core::control::{self, op};
        if bytes.get(12) == Some(&op::USER_DATA) {
            let message =
                control::parse(bytes).map_err(|_| anyhow!("Invalid BUD control frame"))?;
            ensure!(
                message.body.len() <= control::USER_DATA_MAX,
                "BUD user data exceeds limit"
            );
            let body = message
                .body
                .strip_suffix(&[0])
                .ok_or_else(|| anyhow!("BUD user data lacks terminator"))?;
            ensure!(
                self.send_user_data(message.a1, body),
                "BUD user data refused"
            );
            return Ok(());
        }
        self.send_control(bytes)
    }

    pub fn send_user_data(&mut self, id: u32, text: &[u8]) -> bool {
        self.client.send_user_data(id, text)
    }

    pub fn offer(&mut self, id: &str, route: Route) -> Result<Credentials> {
        ensure!(!id.is_empty() && id.len() <= 256, "invalid BUD attempt id");
        ensure!(self.attempt.is_none(), "BUD attempt already active");
        let mut config = route.configure();
        config.video.hevc = self.prefer_hevc;
        config.video.ten_bit = self.ten_bit;
        config.video.chroma_444 = self.chroma_444;
        config.video.full_range = true;
        config.raw_audio = self.raw_audio;
        config.audio_layout = self.audio_layout;
        config.hid_mouse = self.hid_input.0;
        config.hid_keyboard = self.hid_input.1;
        let ours = self
            .client
            .new_attempt(id, config, Transport::Bud)
            .map_err(|e| anyhow!("BUD offer: {e:?}"))?;
        self.attempt = Some(id.to_owned());
        Ok(ours)
    }

    fn matches(&self, id: &str) -> Result<()> {
        ensure!(self.attempt.as_deref() == Some(id), "stale BUD attempt");
        Ok(())
    }

    pub fn answer(&mut self, id: &str, peer: Peer) -> Result<()> {
        let cipher = if peer.aes256.as_ref().is_some_and(|v| !v.is_empty()) {
            "AES-256-GCM"
        } else {
            "AES-128-GCM"
        };
        self.matches(id)?;
        self.client.begin_p2p(id, &peer).map_err(AnswerFailure)?;
        self.cipher = Some(cipher);
        Ok(())
    }

    pub fn candidate(&mut self, id: &str, addr: SocketAddr, kind: Kind) -> Result<()> {
        self.matches(id)?;
        ensure!(
            addr.port() != 0 && !addr.ip().is_unspecified(),
            "invalid BUD candidate"
        );
        self.client.add_candidate(id, addr, false, kind);
        Ok(())
    }

    pub fn ready(&mut self, id: &str) -> Result<()> {
        self.matches(id)?;
        self.client
            .add_candidate(id, "0.0.0.0:0".parse()?, true, Kind::Direct);
        Ok(())
    }

    pub fn poll(&mut self) -> Result<Option<Event>> {
        let Some(received) = self.client.poll_event() else {
            return Ok(None);
        };
        ensure!(
            received.dropped == 0,
            "BUD signaling events lost; end attempt"
        );
        Ok(Some(received.event))
    }

    /// Per-channel counters describe delivered messages. Transport totals count
    /// UDP payload or TURN stream plaintext, including framing/protocol overhead,
    /// excluding TLS and IP/TCP/UDP headers. Historical field names use udp_*.
    pub fn snapshot(&self) -> Snapshot {
        use std::sync::atomic::Ordering::Relaxed;
        let t = self.client.telemetry();
        let connected = t.state.load(Relaxed) == 1;
        let rtt = t.rtt_ms.load(Relaxed);
        let encode = t.encode_us.load(Relaxed);
        Snapshot {
            udp_received: t.udp_received.load(Relaxed),
            udp_sent: t.udp_sent.load(Relaxed),
            connected,
            relay: connected && t.path_relayed.load(Relaxed),
            relay_transport: (connected && t.path_relayed.load(Relaxed)).then(|| {
                match t.relay_transport.load(Relaxed) {
                    1 => "tcp",
                    2 => "tls",
                    _ => "udp",
                }
            }),
            cipher: connected.then_some(self.cipher).flatten(),
            rtt_ms: (connected && rtt > 0).then_some(f64::from(rtt)),
            encode_ms: (connected && encode > 0).then_some(f64::from(encode) / 1000.0),
            channels: std::array::from_fn(|i| Channel {
                fragments: t.fragments[i].load(Relaxed),
                late: t.late[i].load(Relaxed),
                duplicates: t.duplicates[i].load(Relaxed),
                window_drops: t.out_of_window[i].load(Relaxed),
                nacks: t.nacks_sent[i].load(Relaxed),
                bytes: t.bytes[i].load(Relaxed),
            }),
            video_backlog: t.behind.load(Relaxed),
            video_backlog_ms: t.behind_ms.load(Relaxed),
            skipped_video: t.skipped.load(Relaxed),
            audio_pool_drops: t.audio_dropped.load(Relaxed),
            input_drops: t.input_dropped.load(Relaxed),
        }
    }

    pub fn close(&mut self) {
        if let Some(id) = self.attempt.take() {
            // No GUI or WebRTC lock held while SDK joins its own worker threads.
            self.client.end_connection(&id);
            if let Some(units) = &self.external_units {
                units.clear();
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

/// Sole media consumer; cannot clone or outlive borrowed pool leases.
/// Decode/inspection runs without locking the input/session command path.
pub struct MediaSource {
    units: Units,
    audio: lowlat_client::sound::EncodedAudioSource,
}
impl MediaSource {
    pub fn with_video<T>(&mut self, sink: impl FnOnce(&[u8]) -> Result<T>) -> Result<Option<T>> {
        let Some(unit) = self.units.take() else {
            return Ok(None);
        };
        sink(unit.bytes()).map(Some)
    }
    pub fn with_audio<T>(&mut self, sink: impl FnOnce(&[u8]) -> Result<T>) -> Result<Option<T>> {
        let Some(packet) = self.audio.take() else {
            return Ok(None);
        };
        sink(packet.bytes()).map(Some)
    }
}

#[derive(Clone, Default)]
pub struct Channel {
    pub fragments: u64,
    pub late: u64,
    pub duplicates: u64,
    pub window_drops: u64,
    pub nacks: u64,
    pub bytes: u64,
}
#[derive(Clone, Default)]
pub struct Snapshot {
    pub udp_received: u64,
    pub udp_sent: u64,
    pub connected: bool,
    pub relay: bool,
    pub relay_transport: Option<&'static str>,
    pub cipher: Option<&'static str>,
    pub rtt_ms: Option<f64>,
    pub encode_ms: Option<f64>,
    pub channels: [Channel; 3],
    pub video_backlog: u32,
    pub video_backlog_ms: u32,
    pub skipped_video: u64,
    pub audio_pool_drops: u32,
    pub input_drops: u32,
}

#[cfg(feature = "native-core")]
pub fn cursor_message(bytes: &[u8]) -> Result<lowlat_core::cursor::Message<'_>> {
    let control =
        lowlat_core::control::parse(bytes).map_err(|_| anyhow!("Invalid cursor control"))?;
    lowlat_core::cursor::parse(&control).map_err(|_| anyhow!("Invalid host cursor"))
}
#[cfg(feature = "native-core")]
pub use lowlat_core::cursor::Flags as CursorFlags;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn application_query_and_large_settings_use_user_data_not_the_short_input_queue() {
        fn text(id: u32, body: &[u8]) -> Vec<u8> {
            use lowlat_core::control::{CONTROL_HEADER_LEN, Control, encode_header, op};
            let mut frame = vec![0; CONTROL_HEADER_LEN];
            encode_header(
                &mut frame,
                &Control {
                    a0: (body.len() + 1) as u32,
                    a1: id,
                    a2: 0,
                    opcode: op::USER_DATA,
                    body: &[],
                },
            )
            .unwrap();
            frame.extend_from_slice(body);
            frame.push(0);
            frame
        }
        let mut session = Session::external_video(false).unwrap();
        session
            .offer("user-data-routing", Route::Direct { stun: vec![] })
            .unwrap();
        let query = text(9, b"");
        assert!(session.send_frame(&query).is_ok());
        let settings = text(
            10,
            serde_json::json!({"video":[{"output":"DISPLAY1"}],"fixture": "x".repeat(512)})
                .to_string()
                .as_bytes(),
        );
        assert!(settings.len() > 256);
        assert!(session.send_control(&settings).is_err());
        assert!(session.send_frame(&settings).is_ok());
        let mut missing_terminator = query.clone();
        missing_terminator.pop();
        assert!(session.send_frame(&missing_terminator).is_err());
        assert!(
            session
                .send_frame(&text(10, &vec![0; lowlat_core::control::USER_DATA_MAX]))
                .is_err()
        );
        // Ordinary control and lifecycle restrictions stay in the old path.
        let mut short = [0; 13];
        short[12] = 28;
        assert!(session.send_frame(&short).is_ok());
        short[12] = 11;
        assert!(session.send_frame(&short).is_err());
        assert!(session.send_frame(&[0; 12]).is_err());
        session.close();
        assert!(session.send_frame(&query).is_err());
    }
    #[test]
    fn answer_io_failure_exposes_only_safe_stage_kind_and_numeric_code() {
        let failure = AnswerFailure(lowlat_client::event::Error::Io {
            stage: lowlat_client::event::IoStage::RelayCarrier,
            kind: std::io::ErrorKind::PermissionDenied,
            os_error: Some(10013),
        });
        assert_eq!(failure.code(), "io");
        assert_eq!(
            failure.io_details(),
            Some((
                "relay_carrier_create",
                std::io::ErrorKind::PermissionDenied,
                Some(10013)
            ))
        );
        let credentials = AnswerFailure(lowlat_client::event::Error::Credentials);
        assert_eq!(credentials.code(), "credentials");
        assert!(credentials.io_details().is_none());
    }
    #[test]
    fn routes_do_not_mix_direct_candidates_and_relay() {
        let endpoint = "127.0.0.1:3478".parse().unwrap();
        let direct = Route::Direct {
            stun: vec![endpoint],
        }
        .configure();
        assert!(direct.relay.is_none());
        assert_eq!(direct.servers, [endpoint]);
        let relay = Route::turn_udp(endpoint, "isolated", "not-a-real-secret").configure();
        assert!(relay.servers.is_empty());
        assert_eq!(relay.relay.unwrap().server, endpoint);
    }

    #[test]
    fn media_consumer_is_exclusive_and_separate_from_session_commands() {
        let mut session = Session::external_video(false).unwrap();
        session
            .offer("media-source", Route::Direct { stun: vec![] })
            .unwrap();
        let mut source = session.take_media().unwrap();
        assert!(session.take_media().is_err());
        assert!(source.with_video(|_| Ok(())).unwrap().is_none());
        assert!(source.with_audio(|_| Ok(())).unwrap().is_none());
        drop(source);
        session.close();
    }

    #[test]
    fn native_credentials_lifecycle_and_stale_attempts() {
        let mut session = Session::external_video(false).unwrap();
        let ours = session
            .offer("isolated-test", Route::Direct { stun: vec![] })
            .unwrap();
        assert_eq!(ours.ufrag.len(), 8);
        assert_eq!(ours.pwd.len(), 32);
        assert_eq!(ours.fingerprint.len(), 64);
        assert_eq!(ours.aes256.len(), 254);
        assert!(session.ready("stale").is_err());
        assert!(
            session
                .offer("duplicate", Route::Direct { stun: vec![] })
                .is_err()
        );
        session.close();
        assert!(session.ready("isolated-test").is_err());
        assert!(
            session
                .offer("second", Route::Direct { stun: vec![] })
                .is_ok()
        );
    }
}
