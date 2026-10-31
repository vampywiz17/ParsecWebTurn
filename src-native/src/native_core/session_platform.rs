//! Windows implementation of the session's desktop and existing media services.
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

pub struct Windows {
    window: Arc<Window>,
    clipboard: Services,
}
impl Windows {
    pub fn create(manager: Arc<Manager>) -> Result<Self> {
        let window = Window::create_native()?;
        window.install_connection_settings(manager);
        let clipboard = Services::new(Box::new(crate::platform_windows::NativeDesktop(
            window.handle() as usize,
        )));
        Ok(Self { window, clipboard })
    }
}
impl Drop for Windows {
    fn drop(&mut self) {
        // Same teardown order after the controller and its attempt have ended,
        // including failures during signaling/media setup.
        self.window.end_session();
        self.window.request_stop();
        self.window.close();
    }
}
impl View for Windows {
    fn session(&self) -> &State {
        self.window.session()
    }
    fn display_size(&self) -> (u32, u32) {
        self.window.display_size()
    }
    fn notify(&self, notification: Notification) {
        self.window.notify(notification);
    }
}
impl Desktop for Windows {
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
    fn refresh_overlay(&self) {
        self.window.native_refresh_overlay();
    }
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
impl Runtime for Windows {
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
    fn controllers(&self) -> impl crate::gamepad::polling::Devices {
        crate::gamepad_windows::Devices::default()
    }
    fn controller_input_allowed(&self) -> bool {
        self.window.native_input_allowed()
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
    ) -> Result<Arc<crate::audio_windows::Playback>> {
        let playback = Arc::new(crate::audio_windows::Playback::create_layout(
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_adapter_closes_its_window_on_normal_and_early_error_returns() {
        for fail in [false, true] {
            let path = std::env::temp_dir().join(format!(
                "evoparseq-session-adapter-{}.json",
                crate::bud_attempt::native_attempt_id().unwrap(),
            ));
            assert!(!path.exists());
            let mut observed = None;
            let result = (|| -> Result<()> {
                let platform = Windows::create(Manager::open(path.clone()))?;
                observed = Some(platform.window.clone());
                assert!(platform.running());
                if fail {
                    anyhow::bail!("offline-session-setup-failure");
                }
                Ok(())
            })();
            assert_eq!(result.is_err(), fail);
            let window = observed.expect("The fixture window must have been created");
            assert!(window.closing.load(Ordering::Acquire));
            assert!(window.stop.stopped());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while window.hwnd.load(Ordering::Acquire) != 0 && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert_eq!(window.hwnd.load(Ordering::Acquire), 0);
            assert!(
                !path.exists(),
                "The adapter must not persist a fixture profile"
            );
        }
    }
}
