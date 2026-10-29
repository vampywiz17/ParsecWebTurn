//! Session coordination; desktop effects are supplied by a platform adapter.
use super::{
    input, pointer, profile,
    session_model::{self, Event, Notification, Status, View},
    session_video, signaling, transport,
};
use anyhow::{ensure, Result};
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) trait Desktop: View {
    fn running(&self) -> bool;
    fn stop(&self);
    fn fullscreen(&self, active: bool);
    fn input_state(&self, state: crate::input_state::State);
    fn awake(&self);
    fn refresh_overlay(&self);
    fn clipboard_allowed(&self) -> bool;
    fn acknowledge_clipboard(&self);
    fn clipboard(&self) -> &crate::platform::Services;
}

/// Existing backend operations, statically dispatched. This boundary does not
/// replace the transport, HID/cursor workers or Windows audio/video backends.
pub(super) trait Runtime: Desktop {
    fn start_attempt(
        &self,
        id: &str,
        peer: &str,
        writer: crate::bud_attempt::SignalingWrite,
        manager: Arc<crate::connection_settings::Manager>,
        policy: transport::Policy,
    ) -> Result<transport::Attempt>;
    fn register_consumer(&self);
    fn take_events(&self, batch: &mut VecDeque<Event>);
    fn receive_pointer(&self, pointer: &mut pointer::Pointer, frame: &[u8]) -> Result<()>;
    fn send_input(
        &self,
        input: &mut input::Input,
        attempt: &transport::Attempt,
        event: &Event,
    ) -> Result<()>;
    fn create_audio(
        &self,
        layout: crate::audio_layout::Layout,
    ) -> Result<Arc<crate::audio_platform::Playback>>;
    fn host_error(&self, code: i32) -> transport::HostStatus;
}

struct PendingClipboard {
    text: String,
    expires: Instant,
    next_try: Instant,
    attempts: u8,
}

struct Local {
    video: profile::Video,
    fullscreen: bool,
    detached: bool,
    last_clipboard: String,
    pending_clipboard: Option<PendingClipboard>,
}
impl Local {
    fn new(video: profile::Video, desktop: &impl Desktop) -> Result<Self> {
        video.validate()?;
        desktop.fullscreen(video.fullscreen);
        desktop.input_state(crate::input_state::State {
            mode: video.mode(),
            ..Default::default()
        });
        Ok(Self {
            fullscreen: video.fullscreen,
            video,
            detached: false,
            last_clipboard: String::new(),
            pending_clipboard: None,
        })
    }
    fn set_video(&mut self, video: profile::Video) {
        self.fullscreen = video.fullscreen;
        self.video = video;
    }
    fn apply_input_and_fullscreen(&self, desktop: &impl Desktop) {
        desktop.input_state(crate::input_state::State {
            mode: self.video.mode(),
            detached: self.detached,
            ..Default::default()
        });
        desktop.fullscreen(self.fullscreen);
    }
    fn observed_fullscreen(&mut self, active: bool) {
        self.fullscreen = active;
        self.video.fullscreen = active;
    }
    fn shortcut(
        &mut self,
        event: &Event,
        established: bool,
        desktop: &impl Desktop,
        mut send: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<bool> {
        use crate::input::Shortcut;
        let Event::Shortcut(action) = event else {
            return Ok(false);
        };
        match action {
            Shortcut::Disconnect => desktop.stop(),
            Shortcut::Fullscreen => {
                self.fullscreen = !self.fullscreen;
                desktop.fullscreen(self.fullscreen);
            }
            Shortcut::ReleaseInput => {
                if established {
                    send(&crate::control::header(24, 0, 0, 0))?;
                }
                self.detached = !self.detached;
                desktop.input_state(crate::input_state::State {
                    detached: self.detached,
                    mode: self.video.mode(),
                    ..Default::default()
                });
            }
            // Menu, stats and secure attention are handled by the view adapter.
            _ => {}
        }
        Ok(true)
    }
    fn receive_clipboard(&mut self, text: &str, desktop: &impl Desktop) {
        // A newer remote value supersedes pending text, even if it matches the
        // last successful value. Never retain rejected payloads or permissions.
        self.pending_clipboard = None;
        if !desktop.clipboard_allowed()
            || text == self.last_clipboard
            || text.len() > crate::platform::MAX_TEXT
            || text.contains('\0')
        {
            return;
        }
        let now = Instant::now();
        self.pending_clipboard = Some(PendingClipboard {
            text: text.into(),
            expires: now + Duration::from_secs(1),
            next_try: now,
            attempts: 0,
        });
        self.retry_clipboard(desktop, now);
    }
    fn retry_clipboard(&mut self, desktop: &impl Desktop, now: Instant) {
        let Some(pending) = self.pending_clipboard.as_mut() else {
            return;
        };
        if !desktop.clipboard_allowed() || now >= pending.expires {
            self.pending_clipboard = None;
            return;
        }
        if now < pending.next_try {
            return;
        }
        pending.attempts += 1;
        if desktop.clipboard().write_text(&pending.text) {
            self.last_clipboard = self.pending_clipboard.take().unwrap().text;
        } else if pending.attempts >= 5 {
            self.pending_clipboard = None;
            crate::event_log::record(
                "WARN",
                "clipboard_write_unavailable",
                serde_json::json!({"attempts":5}),
            );
        } else {
            pending.next_try = now + Duration::from_millis(200);
        }
    }
    fn clipboard_event(
        &mut self,
        event: &Event,
        established: bool,
        desktop: &impl Desktop,
        mut send: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        if !established
            || !desktop.clipboard_allowed()
            || matches!(event, Event::Focus(false) | Event::Clipboard)
        {
            // Local clipboard changes win over a delayed remote write. Focus
            // loss must not leave text that could overwrite a later local copy.
            self.pending_clipboard = None;
        }
        if matches!(event, Event::Key(true, "KeyV", mods) if mods & 14 == 2) {
            self.retry_clipboard(desktop, Instant::now());
            if self.pending_clipboard.is_some() {
                return Ok(());
            }
        }
        if matches!(event, Event::Clipboard)
            || matches!(event, Event::Key(true, "KeyV", mods) if mods & 14 == 2)
        {
            desktop.acknowledge_clipboard();
            if established && desktop.clipboard_allowed() {
                if let Some(text) = desktop.clipboard().read_notification_text() {
                    if text != self.last_clipboard {
                        send(&crate::control::text(17, 7, &text)?)?;
                        self.last_clipboard = text;
                    }
                }
            }
        }
        Ok(())
    }
}

pub(super) fn run(
    peer: &str,
    socket: &signaling::Socket,
    platform: &impl Runtime,
    manager: Arc<crate::connection_settings::Manager>,
    policy: transport::Policy,
) -> Result<()> {
    let id = crate::bud_attempt::native_attempt_id()?;
    let attempt =
        platform.start_attempt(&id, peer, socket.writer.clone(), manager.clone(), policy)?;
    let mut batch = VecDeque::new();
    let mut pointer = pointer::Pointer::default();
    let mut input = input::Input::default();
    let mut output: Option<Arc<crate::audio_platform::Playback>> = None;
    let mut established = false;
    let mut audio_source = None;
    platform.register_consumer();
    let mut local = Local::new(manager.view().0.native_core.video, platform)?;
    let view = platform;
    let mut host_settings = session_video::HostSettings::new(&local.video, view);
    while platform.running() {
        // The service can close its signaling socket after the BUD path is up.
        for _ in 0..64 {
            match socket.next() {
                Ok(Some(text)) => {
                    attempt.incoming(&text)?;
                }
                Ok(None) => break,
                Err(_) if established || attempt.native_connected() => break,
                Err(_) => anyhow::bail!(
                    "Account signaling ended before {} connected.",
                    attempt.name()
                ),
            }
        }
        while let Some(event) = attempt.pop_event() {
            if event["type"] == 7 {
                if event["status"] != 0 {
                    if let Some(code) = attempt.host_status() {
                        return Err(platform.host_error(code).into());
                    }
                }
                ensure!(
                    event["status"] == 0,
                    "The {} connection ended.",
                    attempt.name()
                );
                if !established {
                    host_settings.connected(&attempt, view)?;
                }
                established = true;
                platform.awake();
            }
        }
        if let Some(failure) = attempt.failure() {
            if failure == "video-native-previous-worker-still-running" {
                anyhow::bail!("The previous video session is still closing. Retry in a moment; restart EvoParseq if it does not finish.");
            }
            if failure.starts_with("video-native-") {
                if established {
                    let _ = platform.send_input(&mut input, &attempt, &Event::Focus(false));
                }
                anyhow::bail!("The video driver did not finish its operation. Restart EvoParseq before reconnecting. See the event log.");
            }
            if let Some(code) = attempt.host_status() {
                return Err(platform.host_error(code).into());
            }
            anyhow::bail!(
                "{} connection failed. See the event log for its stage.",
                attempt.name()
            );
        }
        for _ in 0..64 {
            let Some(frame) = attempt.pop_control() else {
                break;
            };
            if frame.get(12) == Some(&9) {
                platform.receive_pointer(&mut pointer, &frame)?;
                continue;
            }
            match crate::control::decode(&frame)? {
                crate::control::Message::Buffer {
                    event,
                    payload: Some(range),
                } if event["type"] == 3 && event["id"] == 11 => {
                    let bytes = &frame[range];
                    let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
                    host_settings.receive(bytes, &attempt, view);
                }
                crate::control::Message::Buffer {
                    event,
                    payload: Some(range),
                } if event["type"] == 3 && event["id"] == 12 => {
                    let bytes = &frame[range];
                    host_settings.receive_outputs(
                        bytes.strip_suffix(&[0]).unwrap_or(bytes),
                        &attempt,
                        view,
                    );
                }
                crate::control::Message::Buffer {
                    event,
                    payload: Some(range),
                } if event["type"] == 3 && event["id"] == 7 => {
                    let bytes = &frame[range];
                    if let Ok(text) = std::str::from_utf8(bytes.strip_suffix(&[0]).unwrap_or(bytes))
                    {
                        local.receive_clipboard(text, platform);
                    }
                }
                crate::control::Message::Status(status) if status != 0 => {
                    crate::event_log::record(
                        "ERROR",
                        "native_host_status",
                        serde_json::json!({"transport":attempt.name(), "host_status":status}),
                    );
                    return Err(platform.host_error(status).into());
                }
                _ => {}
            }
        }
        host_settings.poll(&attempt, view)?;
        if audio_source.is_none() {
            audio_source = attempt.audio_pipeline();
            if let Some(audio) = &audio_source {
                audio.set_native_consumer(std::thread::current());
            }
        }
        if let Some(audio) = &audio_source {
            if output
                .as_ref()
                .is_some_and(|playback| playback.take_recovery_flush())
            {
                audio.discard_pending();
            }
            for index in 0..8 {
                if index == 7 {
                    std::thread::current().unpark();
                }
                let Some(block) = audio.take_native_pcm() else {
                    break;
                };
                if output.as_ref().is_none_or(|p| p.layout != block.layout) {
                    if let Some(old) = output.take() {
                        old.stop();
                    }
                    let playback = platform.create_audio(block.layout)?;
                    output = Some(playback);
                }
                if let Some(playback) = &output {
                    playback.queue(&block.pcm);
                }
                audio.recycle_handoff(block);
            }
        }
        platform.take_events(&mut batch);
        for event in batch.drain(..) {
            if matches!(event, Event::InputOverflow) {
                crate::event_log::record("ERROR", "input_queue_overflow", serde_json::json!({}));
                if established {
                    let _ = platform.send_input(&mut input, &attempt, &Event::Focus(false));
                }
                anyhow::bail!("Input queue overloaded. Input was released; reconnect to continue.");
            }
            if local.shortcut(&event, established, platform, |frame| attempt.send(frame))? {
                if matches!(event, Event::Shortcut(crate::input::Shortcut::ReleaseInput)) {
                    // The release command also clears host modifiers. Do not
                    // resurrect them when input is recaptured without key-ups.
                    input = Default::default();
                }
                continue;
            }
            local.clipboard_event(&event, established, platform, |frame| attempt.send(frame))?;
            if let Event::NativeSession(action) = &event {
                match action {
                    session_model::Command::Disconnect => platform.stop(),
                    session_model::Command::HideButton => {
                        if manager
                            .update_native(|p| p.hide_session_button = true)
                            .is_err()
                        {
                            crate::event_log::record(
                                "WARN",
                                "native_session_button_save_failed",
                                serde_json::json!({}),
                            );
                        }
                        platform.refresh_overlay();
                    }
                    session_model::Command::SecureAttention => {
                        // Same application request as the stock menu. The host
                        // authorizes it and needs its installed service for SAS.
                        let result = (|| -> Result<()> {
                            ensure!(established, "Transport is not connected yet");
                            attempt.send(&crate::control::header(24, 0, 0, 0))?;
                            attempt.send(&crate::control::text(17, 14, "")?)
                        })();
                        crate::event_log::record(
                            if result.is_ok() { "INFO" } else { "WARN" },
                            "native_secure_attention_requested",
                            serde_json::json!({"sent":result.is_ok()}),
                        );
                        view.notify(Notification::Status(if result.is_ok() {
                            Status::AttentionSent
                        } else {
                            Status::AttentionFailed
                        }));
                    }
                    session_model::Command::SelectDisplay(id) => {
                        let result = if established {
                            host_settings.select_output(&attempt, view, id)
                        } else {
                            Err(anyhow::anyhow!("Transport is not connected"))
                        };
                        if matches!(result, Ok(true)) {
                            // A monitor switch releases the previous monitor's
                            // forced resolution. Keep the live and saved UI
                            // preference consistent with that host request.
                            local.video.resolution = profile::Resolution::Host;
                            if manager
                                .update_native(|p| p.video.resolution = profile::Resolution::Host)
                                .is_err()
                            {
                                crate::event_log::record(
                                    "WARN",
                                    "native_display_preference_save_failed",
                                    serde_json::json!({}),
                                );
                            }
                            view.notify(Notification::DisplaySelected);
                        }
                        if result.is_err() {
                            crate::event_log::record(
                                "WARN",
                                "native_host_display_request_failed",
                                serde_json::json!({"hid":attempt.hid_mouse_enabled()}),
                            );
                        }
                        host_settings.result(view, result.is_ok());
                        view.notify(Notification::DisplaysChanged);
                    }
                    session_model::Command::RefreshDisplays => {
                        if established && host_settings.refresh_outputs(&attempt, view).is_err() {
                            view.result(false);
                        }
                    }
                    session_model::Command::Apply(next) => {
                        let result = (|| -> Result<()> {
                            ensure!(established, "Transport is not connected yet");
                            let size = host_settings.apply(&attempt, view, next)?;
                            manager.update_native(|p| p.video = next.clone())?;
                            local.set_video(next.clone());
                            host_settings.applied_locally(size);
                            attempt.send(&crate::control::header(24, 0, 0, 0))?;
                            local.apply_input_and_fullscreen(platform);
                            Ok(())
                        })();
                        if result.is_err() {
                            crate::event_log::record(
                                "WARN",
                                "native_session_settings_failed",
                                serde_json::json!({}),
                            );
                        }
                        host_settings.result(view, result.is_ok());
                    }
                }
                continue;
            }
            if let Event::Fullscreen(active) = &event {
                local.observed_fullscreen(*active);
                let _ = manager.update_native(|p| p.video.fullscreen = *active);
            }
            if matches!(event, Event::Size(..))
                && established
                && local.video.resolution == profile::Resolution::Client
            {
                host_settings.resized(&attempt, view, &local.video)?;
            }
            if established {
                platform.send_input(&mut input, &attempt, &event)?;
            }
        }
        local.retry_clipboard(platform, Instant::now());
        // Existing FIFO wakes immediately for input; media decoding and presentation
        // stay on their original workers. This interval only services control/audio.
        std::thread::park_timeout(Duration::from_millis(100));
    }
    if established {
        let _ = attempt.send(&crate::control::header(24, 0, 0, 0));
    }
    drop(attempt);
    if let Some(output) = output {
        output.stop();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        sync::Mutex,
    };

    #[derive(Debug, PartialEq)]
    enum Effect {
        Fullscreen(bool),
        Input(crate::input_state::State),
        Stop,
        Awake,
        Overlay,
        ClipboardAcknowledged,
    }
    struct Clipboard(Arc<Mutex<String>>, Arc<std::sync::atomic::AtomicBool>);
    impl crate::platform::Desktop for Clipboard {
        fn read_text(&self) -> Option<String> {
            Some(self.0.lock().unwrap().clone())
        }
        fn write_text(&self, text: &str) -> bool {
            if !self.1.load(std::sync::atomic::Ordering::Acquire) {
                return false;
            }
            *self.0.lock().unwrap() = text.into();
            true
        }
    }
    struct TestDesktop {
        state: session_model::State,
        effects: RefCell<Vec<Effect>>,
        allowed: Cell<bool>,
        clipboard: crate::platform::Services,
        text: Arc<Mutex<String>>,
        writable: Arc<std::sync::atomic::AtomicBool>,
    }
    impl Default for TestDesktop {
        fn default() -> Self {
            let text = Arc::new(Mutex::new(String::new()));
            let writable = Arc::new(std::sync::atomic::AtomicBool::new(true));
            Self {
                state: Default::default(),
                effects: Default::default(),
                allowed: Cell::new(true),
                clipboard: crate::platform::Services::new(Box::new(Clipboard(
                    text.clone(),
                    writable.clone(),
                ))),
                text,
                writable,
            }
        }
    }
    impl View for TestDesktop {
        fn session(&self) -> &session_model::State {
            &self.state
        }
        fn display_size(&self) -> (u32, u32) {
            (1920, 1080)
        }
        fn notify(&self, _: Notification) {}
    }
    impl Desktop for TestDesktop {
        fn running(&self) -> bool {
            !self.effects.borrow().contains(&Effect::Stop)
        }
        fn stop(&self) {
            self.effects.borrow_mut().push(Effect::Stop);
        }
        fn fullscreen(&self, active: bool) {
            self.effects.borrow_mut().push(Effect::Fullscreen(active));
        }
        fn input_state(&self, state: crate::input_state::State) {
            self.effects.borrow_mut().push(Effect::Input(state));
        }
        fn awake(&self) {
            self.effects.borrow_mut().push(Effect::Awake);
        }
        fn refresh_overlay(&self) {
            self.effects.borrow_mut().push(Effect::Overlay);
        }
        fn clipboard_allowed(&self) -> bool {
            self.allowed.get()
        }
        fn acknowledge_clipboard(&self) {
            self.effects
                .borrow_mut()
                .push(Effect::ClipboardAcknowledged);
        }
        fn clipboard(&self) -> &crate::platform::Services {
            &self.clipboard
        }
    }
    #[test]
    fn desktop_controls_use_routed_actions_and_preserve_immersive_state_order() {
        let desktop = TestDesktop::default();
        let video = profile::Video {
            fullscreen: true,
            immersive: 1,
            ..Default::default()
        };
        let mut local = Local::new(video.clone(), &desktop).unwrap();
        let mut sent = Vec::new();
        assert_eq!(
            &*desktop.effects.borrow(),
            &[
                Effect::Fullscreen(true),
                Effect::Input(crate::input_state::State {
                    mode: video.mode(),
                    ..Default::default()
                }),
            ]
        );
        assert!(local
            .shortcut(
                &Event::Shortcut(crate::input::Shortcut::Fullscreen),
                true,
                &desktop,
                |_| unreachable!()
            )
            .unwrap());
        assert!(!local.fullscreen);
        // A shortcut requests fullscreen; only its actual window event updates
        // the saved preference, exactly as in the previous controller.
        assert!(local.video.fullscreen);
        assert!(!local
            .shortcut(
                &Event::Key(true, "KeyW", 7),
                true,
                &desktop,
                |_| unreachable!()
            )
            .unwrap());
        assert!(!local
            .shortcut(
                &Event::Key(false, "KeyW", 0),
                true,
                &desktop,
                |_| unreachable!()
            )
            .unwrap());
        local.observed_fullscreen(false);
        assert!(!local.video.fullscreen);
        assert!(local
            .shortcut(
                &Event::Shortcut(crate::input::Shortcut::ReleaseInput),
                true,
                &desktop,
                |frame| {
                    sent.push(frame.to_vec());
                    Ok(())
                }
            )
            .unwrap());
        assert_eq!(sent, [crate::control::header(24, 0, 0, 0).to_vec()]);
        assert!(local.detached);
        local.set_video(profile::Video {
            immersive: 2,
            fullscreen: true,
            ..video
        });
        desktop.effects.borrow_mut().clear();
        local.apply_input_and_fullscreen(&desktop);
        assert_eq!(
            &*desktop.effects.borrow(),
            &[
                Effect::Input(crate::input_state::State {
                    mode: local.video.mode(),
                    detached: true,
                    ..Default::default()
                }),
                Effect::Fullscreen(true),
            ]
        );
        local
            .shortcut(
                &Event::Shortcut(crate::input::Shortcut::Disconnect),
                true,
                &desktop,
                |_| unreachable!(),
            )
            .unwrap();
        assert!(!desktop.running());
    }
    #[test]
    fn failed_unlock_does_not_change_capture_and_focus_camera_events_remain_forwardable() {
        let desktop = TestDesktop::default();
        assert!(Local::new(
            profile::Video {
                immersive: 9,
                ..Default::default()
            },
            &desktop
        )
        .is_err());
        assert!(desktop.effects.borrow().is_empty());
        let mut local = Local::new(Default::default(), &desktop).unwrap();
        desktop.effects.borrow_mut().clear();
        assert!(local
            .shortcut(
                &Event::Shortcut(crate::input::Shortcut::ReleaseInput),
                true,
                &desktop,
                |_| anyhow::bail!("offline-send-failed")
            )
            .is_err());
        assert!(!local.detached);
        assert!(desktop.effects.borrow().is_empty());
        for event in [
            Event::Focus(false),
            Event::Focus(true),
            Event::HidInputRelease,
            Event::RelativeMode(true),
            Event::RelativeMotion(3, -4),
            Event::CursorReady,
            Event::Button(false, 0, 12, 34),
            Event::Key(false, "KeyA", 0),
        ] {
            assert!(!local
                .shortcut(&event, true, &desktop, |_| unreachable!())
                .unwrap());
            local
                .clipboard_event(&event, true, &desktop, |_| unreachable!())
                .unwrap();
        }
        assert!(desktop.effects.borrow().is_empty());
        // Before connecting, unlocking stays local and never sends a packet.
        local
            .shortcut(
                &Event::Shortcut(crate::input::Shortcut::ReleaseInput),
                false,
                &desktop,
                |_| unreachable!(),
            )
            .unwrap();
        assert!(local.detached);
    }
    #[test]
    fn clipboard_retries_only_the_latest_value_and_commits_after_success() {
        let desktop = TestDesktop::default();
        let mut local = Local::new(Default::default(), &desktop).unwrap();
        local.receive_clipboard("old", &desktop);
        desktop
            .writable
            .store(false, std::sync::atomic::Ordering::Release);
        local.receive_clipboard("first pending", &desktop);
        assert_eq!(local.last_clipboard, "old");
        assert_eq!(desktop.text.lock().unwrap().as_str(), "old");
        local.receive_clipboard("latest pending", &desktop);
        let due = local.pending_clipboard.as_ref().unwrap().next_try;
        local.retry_clipboard(&desktop, due - Duration::from_millis(1));
        assert_eq!(desktop.clipboard.snapshot().unavailable_or_rejected, 2);
        desktop
            .writable
            .store(true, std::sync::atomic::Ordering::Release);
        local.retry_clipboard(&desktop, due);
        assert_eq!(local.last_clipboard, "latest pending");
        assert_eq!(desktop.text.lock().unwrap().as_str(), "latest pending");
        assert!(local.pending_clipboard.is_none());
        local
            .clipboard_event(&Event::Clipboard, true, &desktop, |_| unreachable!())
            .unwrap();
        assert_eq!(desktop.clipboard.snapshot().clipboard_writes, 2);
    }
    #[test]
    fn clipboard_retry_is_bounded_and_focus_permission_and_local_copy_cancel_it() {
        let desktop = TestDesktop::default();
        let mut local = Local::new(Default::default(), &desktop).unwrap();
        desktop
            .writable
            .store(false, std::sync::atomic::Ordering::Release);
        local.receive_clipboard("bounded", &desktop);
        for _ in 0..4 {
            let due = local.pending_clipboard.as_ref().unwrap().next_try;
            local.retry_clipboard(&desktop, due);
        }
        assert!(local.pending_clipboard.is_none());
        assert_eq!(desktop.clipboard.snapshot().unavailable_or_rejected, 5);
        assert_eq!(local.last_clipboard, "");
        local.receive_clipboard("expires", &desktop);
        let expires = local.pending_clipboard.as_ref().unwrap().expires;
        local.retry_clipboard(&desktop, expires);
        assert!(local.pending_clipboard.is_none());
        for event in [Event::Focus(false), Event::Clipboard] {
            local.receive_clipboard("cancelled", &desktop);
            local
                .clipboard_event(&event, true, &desktop, |_| unreachable!())
                .unwrap();
            assert!(local.pending_clipboard.is_none());
        }
        local.receive_clipboard("permission", &desktop);
        desktop.allowed.set(false);
        local.retry_clipboard(&desktop, Instant::now());
        assert!(local.pending_clipboard.is_none());
        desktop.allowed.set(true);
        local.receive_clipboard("rejected\0text", &desktop);
        assert!(local.pending_clipboard.is_none());
    }
    #[test]
    fn pending_remote_clipboard_does_not_echo_old_local_text_on_paste() {
        let desktop = TestDesktop::default();
        let mut local = Local::new(Default::default(), &desktop).unwrap();
        *desktop.text.lock().unwrap() = "local old text".into();
        desktop
            .writable
            .store(false, std::sync::atomic::Ordering::Release);
        local.receive_clipboard("remote pending", &desktop);
        local
            .clipboard_event(
                &Event::Key(true, "KeyV", 2),
                true,
                &desktop,
                |_| unreachable!(),
            )
            .unwrap();
        assert!(local.pending_clipboard.is_some());
        assert_eq!(desktop.clipboard.snapshot().clipboard_reads, 0);
        // A duplicate of the successful value must still retire a newer pending value.
        local.receive_clipboard("", &desktop);
        assert!(local.pending_clipboard.is_none());
    }
    #[test]
    fn clipboard_preserves_focus_gating_deduplication_and_retry_after_send_failure() {
        let desktop = TestDesktop::default();
        let mut local = Local::new(Default::default(), &desktop).unwrap();
        let mut sent = Vec::new();
        local.receive_clipboard("host text", &desktop);
        local.receive_clipboard("host text", &desktop);
        assert_eq!(desktop.clipboard.snapshot().clipboard_writes, 1);
        local
            .clipboard_event(&Event::Clipboard, true, &desktop, |_| unreachable!())
            .unwrap();
        *desktop.text.lock().unwrap() = "client text".into();
        desktop.allowed.set(false);
        local.receive_clipboard("blocked remote update", &desktop);
        local
            .clipboard_event(&Event::Clipboard, true, &desktop, |_| unreachable!())
            .unwrap();
        assert_eq!(desktop.text.lock().unwrap().as_str(), "client text");
        desktop.allowed.set(true);
        local
            .clipboard_event(&Event::Clipboard, false, &desktop, |_| unreachable!())
            .unwrap();
        assert_eq!(desktop.clipboard.snapshot().clipboard_reads, 1);
        assert!(local
            .clipboard_event(&Event::Clipboard, true, &desktop, |_| anyhow::bail!(
                "offline-send-failed"
            ))
            .is_err());
        assert_eq!(local.last_clipboard, "host text");
        local
            .clipboard_event(&Event::Key(true, "KeyV", 2), true, &desktop, |frame| {
                sent.push(frame.to_vec());
                Ok(())
            })
            .unwrap();
        assert_eq!(
            sent,
            [crate::control::text(17, 7, "client text").unwrap().to_vec()]
        );
        assert_eq!(local.last_clipboard, "client text");
        local
            .clipboard_event(&Event::Clipboard, true, &desktop, |_| unreachable!())
            .unwrap();
        assert_eq!(desktop.clipboard.snapshot().clipboard_writes, 1);
    }
}
