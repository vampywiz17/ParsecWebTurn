//! macOS implementation of the session's desktop and existing media services.
use super::{
    input::Input,
    pointer::Pointer,
    session::{Desktop, Runtime},
    session_model::{Event, Notification, State, View},
    transport,
};
use crate::{connection_settings::Manager, input_state, platform::Services, window::Window};
use anyhow::Result;
use std::{
    collections::VecDeque,
    sync::{atomic::Ordering, Arc},
};

pub struct Platform {
    window: Arc<Window>,
    clipboard: Services,
}
impl Platform {
    pub fn create(manager: Arc<Manager>) -> Result<Self> {
        let window = Window::create_native()?;
        window.install_connection_settings(manager);
        let clipboard = Services::new(Box::new(crate::platform_macos::NativeDesktop(
            Arc::downgrade(&window),
        )));
        Ok(Self { window, clipboard })
    }
}
impl Drop for Platform {
    fn drop(&mut self) {
        // Same teardown order as Windows, including setup failures.
        super::session_ui::close();
        self.window.end_session();
        self.window.request_stop();
        self.window.close();
    }
}
impl View for Platform {
    fn session(&self) -> &State {
        &self.window.session
    }
    fn display_size(&self) -> (u32, u32) {
        self.window.native_display_size()
    }
    fn notify(&self, notification: Notification) {
        match notification {
            Notification::Status(status) => super::session_ui::status(status),
            Notification::DisplaysChanged => super::session_ui::displays(false),
            Notification::DisplaySelected => super::session_ui::displays(true),
        }
    }
}
impl Desktop for Platform {
    fn running(&self) -> bool {
        !self.window.closing.load(Ordering::Acquire) && !self.window.stop.stopped()
    }
    fn stop(&self) {
        self.window.request_stop();
    }
    fn fullscreen(&self, active: bool) {
        self.window.set_fullscreen(active);
    }
    fn input_state(&self, state: input_state::State) {
        self.window.set_input_state(state);
    }
    fn awake(&self) {
        self.window.set_wake_lock(true);
    }
    fn refresh_overlay(&self) {}
    fn clipboard_allowed(&self) -> bool {
        self.window.clipboard_allowed()
    }
    fn acknowledge_clipboard(&self) {
        self.window.take_clipboard_event();
    }
    fn clipboard(&self) -> &Services {
        &self.clipboard
    }
}
impl Runtime for Platform {
    fn start_attempt(
        &self,
        id: &str,
        peer: &str,
        writer: crate::bud_attempt::SignalingWrite,
        manager: Arc<Manager>,
        policy: transport::Policy,
    ) -> Result<transport::Attempt> {
        transport::Attempt::spawn_native(id, peer, writer, self.window.clone(), manager, policy)
    }
    fn register_consumer(&self) {
        self.window.events.set_consumer(std::thread::current());
    }
    fn take_events(&self, batch: &mut VecDeque<Event>) {
        self.window.events.take_batch(batch);
    }
    fn receive_pointer(&self, pointer: &mut Pointer, frame: &[u8]) -> Result<()> {
        pointer.receive(frame, &self.window)
    }
    fn send_input(
        &self,
        input: &mut Input,
        attempt: &transport::Attempt,
        event: &Event,
    ) -> Result<()> {
        input.send(attempt, &self.window, event)
    }
    fn create_audio(
        &self,
        layout: crate::audio_layout::Layout,
    ) -> Result<Arc<crate::audio_platform::Playback>> {
        let playback = Arc::new(crate::audio_platform::Playback::create_layout(
            self.window.clone(),
            75,
            150,
            layout,
        )?);
        self.window.stats.set_audio(Arc::downgrade(&playback));
        Ok(playback)
    }
    fn host_error(&self, code: i32) -> transport::HostStatus {
        transport::HostStatus::from_window(code, &self.window)
    }
}
