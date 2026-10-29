//! Native AppKit stream window. AppKit objects are created, used and released
//! on the main thread; other threads post work to the main dispatch queue.
pub use crate::native_core::session_model::Event;
use anyhow::Result;
use objc2::{rc::Retained, runtime::AnyObject, MainThreadMarker};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSColor, NSEvent, NSEventModifierFlags,
    NSEventType, NSPasteboard, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::sync::{
    atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering},
    Arc, Mutex, Weak,
};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    // Documented relative-pointer mode: the cursor stays put while deltas flow.
    fn CGAssociateMouseAndMouseCursorPosition(connected: u8) -> i32;
}

pub struct Window {
    pub(crate) session: crate::native_core::session_model::State,
    pub stats: Arc<crate::stats::Shared>,
    pub closing: AtomicBool,
    pub events: crate::event_queue::Queue<Event>,
    /// Content size in points; video and absolute input share this space.
    pub dimensions: Mutex<(i32, i32)>,
    pub video_report: Mutex<Option<crate::video_output::Snapshot>>,
    pub stop: Arc<crate::lifecycle::StopSignal>,
    pub has_focus: AtomicBool,
    pub(crate) pasteboard_seen: AtomicIsize,
    connection_settings: Mutex<Option<Arc<crate::connection_settings::Manager>>>,
    video_ready: AtomicBool,
    video_generation: Mutex<u64>,
    relative_mouse: AtomicBool,
    relative_requested: AtomicBool,
    host_cursor: Mutex<Option<(i32, i32, bool)>>,
    input_state: Mutex<crate::input_state::State>,
    pressed_buttons: AtomicUsize,
    local_shortcuts: Mutex<crate::input::Shortcuts>,
    cursor_pending: Mutex<crate::cursor::Pending>,
    fullscreen_active: AtomicBool,
    wake_requested: AtomicBool,
    wake_lock: Mutex<crate::wake_lock::State>,
    #[cfg(feature = "bud")]
    hid_mouse_active: AtomicBool,
    #[cfg(feature = "bud")]
    hid_keyboard_active: AtomicBool,
    #[cfg(feature = "bud")]
    hid_input_acquired: AtomicBool,
    ns_window: usize,
    video_layer: usize,
}

static ACTIVE: Mutex<Option<Weak<Window>>> = Mutex::new(None);
thread_local! {
    // Main thread only: NSCursor objects and fractional trackpad scrolling.
    static CURSOR: std::cell::RefCell<crate::cursor::State> = Default::default();
    static SCROLL: std::cell::Cell<(f64, f64)> = const { std::cell::Cell::new((0.0, 0.0)) };
}

pub fn active() -> Option<Arc<Window>> {
    ACTIVE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(Weak::upgrade)
}

/// Runs on the main thread, synchronously when already there.
pub(crate) fn on_main<R: Send>(f: impl FnOnce(MainThreadMarker) -> R + Send) -> R {
    match MainThreadMarker::new() {
        Some(mtm) => f(mtm),
        None => {
            let mut result = None;
            dispatch2::DispatchQueue::main().exec_sync(|| {
                result = Some(f(MainThreadMarker::new().expect("main dispatch queue")))
            });
            result.expect("main dispatch queue ran the work")
        }
    }
}
pub(crate) fn post(f: impl FnOnce(MainThreadMarker) + Send + 'static) {
    dispatch2::DispatchQueue::main()
        .exec_async(move || f(MainThreadMarker::new().expect("main dispatch queue")));
}

impl Window {
    pub fn create_native() -> Result<Arc<Self>> {
        let (ns_window, video_layer, size) = on_main(|mtm| {
            let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1280.0, 720.0));
            let style = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable;
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    mtm.alloc(),
                    rect,
                    style,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            window.setTitle(&NSString::from_str(crate::APP_TITLE));
            unsafe { window.setReleasedWhenClosed(false) };
            window.setBackgroundColor(Some(&NSColor::blackColor()));
            window.setAcceptsMouseMovedEvents(true);
            let view = NSView::initWithFrame(mtm.alloc(), rect);
            view.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            let layer = crate::video_platform::create_layer();
            if let Some(layer) = &layer {
                // Layer-hosting view: assign the layer before enabling it.
                let _: () = unsafe { objc2::msg_send![&*view, setLayer: &**layer] };
                view.setWantsLayer(true);
            }
            window.setContentView(Some(&view));
            window.center();
            window.makeKeyAndOrderFront(None);
            let size = window.contentRectForFrameRect(window.frame()).size;
            (
                Retained::into_raw(window) as usize,
                layer.map_or(0, |l| Retained::into_raw(l) as usize),
                (size.width as i32, size.height as i32),
            )
        });
        let window = Arc::new(Self {
            session: Default::default(),
            stats: Default::default(),
            closing: AtomicBool::new(false),
            events: crate::event_queue::Queue::input(),
            dimensions: Mutex::new(size),
            video_report: Default::default(),
            stop: Default::default(),
            has_focus: AtomicBool::new(false),
            pasteboard_seen: AtomicIsize::new(on_main(|_| {
                NSPasteboard::generalPasteboard().changeCount()
            })),
            connection_settings: Default::default(),
            video_ready: AtomicBool::new(false),
            video_generation: Mutex::new(0),
            relative_mouse: AtomicBool::new(false),
            relative_requested: AtomicBool::new(false),
            host_cursor: Default::default(),
            input_state: Default::default(),
            pressed_buttons: AtomicUsize::new(0),
            local_shortcuts: Default::default(),
            cursor_pending: Default::default(),
            fullscreen_active: AtomicBool::new(false),
            wake_requested: AtomicBool::new(false),
            wake_lock: Default::default(),
            #[cfg(feature = "bud")]
            hid_mouse_active: AtomicBool::new(false),
            #[cfg(feature = "bud")]
            hid_keyboard_active: AtomicBool::new(false),
            #[cfg(feature = "bud")]
            hid_input_acquired: AtomicBool::new(false),
            ns_window,
            video_layer,
        });
        window.events.push(Event::Size(size.0, size.1));
        *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::downgrade(&window));
        Ok(window)
    }

    /// Main thread only; the window retains its NSWindow until Drop.
    fn ns_window(&self, _mtm: MainThreadMarker) -> &NSWindow {
        unsafe { &*(self.ns_window as *const NSWindow) }
    }
    pub fn video_layer(&self) -> usize {
        self.video_layer
    }

    fn manager(&self) -> Option<Arc<crate::connection_settings::Manager>> {
        self.connection_settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn install_connection_settings(&self, manager: Arc<crate::connection_settings::Manager>) {
        *self
            .connection_settings
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(manager);
    }
    pub fn video_gpu(&self) -> Option<crate::connection_settings::GpuPreference> {
        self.manager().and_then(|m| m.view().0.video_gpu)
    }
    pub fn audio_buffer(&self) -> Option<(u32, u32)> {
        let s = self.manager()?.view().0;
        (s.custom_audio_buffer
            && s.audio_max_buffer_ms > 0
            && s.audio_max_buffer_ms <= 500
            && s.audio_min_buffer_ms <= s.audio_max_buffer_ms)
            .then_some((s.audio_min_buffer_ms, s.audio_max_buffer_ms))
    }

    pub fn native_input_allowed(&self) -> bool {
        self.has_focus.load(Ordering::Acquire)
            && self.video_ready.load(Ordering::Acquire)
            && !self.closing.load(Ordering::Acquire)
            && !self
                .input_state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .detached
    }

    fn set_video_ready(&self, ready: bool) {
        if self.video_ready.swap(ready, Ordering::AcqRel) != ready {
            self.apply_capture();
        }
    }
    // Same generation rules as Windows: a retired pipeline cannot touch a newer one.
    pub fn begin_video(&self) -> u64 {
        let mut generation = self
            .video_generation
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *generation = generation.wrapping_add(1);
        #[cfg(feature = "bud")]
        self.hid_mouse_active.store(false, Ordering::Release);
        #[cfg(feature = "bud")]
        self.hid_keyboard_active.store(false, Ordering::Release);
        self.set_video_ready(false);
        *generation
    }
    pub fn set_video_ready_for(&self, expected: u64, ready: bool) {
        let generation = self
            .video_generation
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *generation == expected {
            self.set_video_ready(ready);
        }
    }
    pub fn finish_video(&self, expected: u64) {
        let mut generation = self
            .video_generation
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *generation == expected {
            *generation = generation.wrapping_add(1);
            #[cfg(feature = "bud")]
            self.hid_mouse_active.store(false, Ordering::Release);
            #[cfg(feature = "bud")]
            self.hid_keyboard_active.store(false, Ordering::Release);
            self.set_video_ready(false);
        }
    }
    pub fn end_session(&self) {
        self.begin_video(); // Retire the old surface before a reconnect can own it.
        *self.host_cursor.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.relative_requested.store(false, Ordering::Release);
        self.set_input_state(Default::default());
        self.apply_capture();
    }

    /// The layer letterboxes with resize-aspect, which Viewport::rect computes.
    pub fn presented_viewport(&self) -> Option<crate::viewport::Viewport> {
        if !self.video_ready.load(Ordering::Acquire) {
            return None;
        }
        let report = self.video_report.lock().unwrap_or_else(|e| e.into_inner());
        let v = report
            .as_ref()
            .filter(|v| v.frames_submitted > 0 && v.failure_stage.is_none())?;
        let size = *self.dimensions.lock().unwrap_or_else(|e| e.into_inner());
        let viewport = crate::viewport::Viewport {
            source: (v.width?, v.height?),
            client: (u32::try_from(size.0).ok()?, u32::try_from(size.1).ok()?),
        };
        viewport.rect()?;
        Some(viewport)
    }

    pub fn set_host_cursor(&self, x: i32, y: i32, relative: bool, hidden: bool) {
        // Same rule as Windows: a hidden host cursor also requests relative input.
        let relative = relative || hidden;
        *self.host_cursor.lock().unwrap_or_else(|e| e.into_inner()) = Some((x, y, relative));
        if self.relative_requested.swap(relative, Ordering::AcqRel) != relative {
            self.apply_capture();
        }
    }
    pub fn set_input_state(&self, state: crate::input_state::State) {
        let mut current = self.input_state.lock().unwrap_or_else(|e| e.into_inner());
        if *current != state {
            *current = state;
            drop(current);
            self.apply_capture();
        }
    }

    /// Acquire or release relative capture from the current focus/video state.
    /// HID mouse keeps relative capture too, like Windows: the USB adapter
    /// switches report mode on RelativeMode instead of receiving absolute moves.
    fn apply_capture(&self) {
        let enabled =
            self.relative_requested.load(Ordering::Acquire) && self.native_input_allowed();
        if self.relative_mouse.swap(enabled, Ordering::AcqRel) == enabled {
            return;
        }
        post(move |_| unsafe {
            CGAssociateMouseAndMouseCursorPosition(u8::from(!enabled));
        });
        self.select_cursor();
        // Report the actual capture state to the input consumer.
        self.events.push(Event::RelativeMode(enabled));
    }

    pub fn clipboard_allowed(&self) -> bool {
        self.native_input_allowed()
    }
    pub fn take_clipboard_event(&self) {}

    #[cfg(feature = "bud")]
    pub fn hid_input_generation(&self) -> u64 {
        *self
            .video_generation
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
    #[cfg(feature = "bud")]
    pub fn set_hid_input_active(&self, expected: u64, mouse: bool, keyboard: bool) {
        let generation = self
            .video_generation
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *generation != expected {
            return;
        }
        let mouse_changed = self.hid_mouse_active.swap(mouse, Ordering::AcqRel) != mouse;
        self.hid_keyboard_active.store(keyboard, Ordering::Release);
        drop(generation);
        if mouse_changed {
            // Seed a new USB adapter with the current capture state.
            self.events.push(Event::RelativeMode(
                self.relative_mouse.load(Ordering::Acquire),
            ));
        }
        self.sync_hid_acquired();
    }
    #[cfg(feature = "bud")]
    fn sync_hid_acquired(&self) {
        let acquired = self.native_input_allowed()
            && (self.hid_mouse_active.load(Ordering::Acquire)
                || self.hid_keyboard_active.load(Ordering::Acquire));
        if self.hid_input_acquired.swap(acquired, Ordering::AcqRel) && !acquired {
            self.events.push(Event::HidInputRelease);
        }
    }

    pub fn close(&self) {
        self.request_stop();
        let window = self.ns_window;
        post(move |_| unsafe { (*(window as *const NSWindow)).orderOut(None) });
    }
    pub fn request_stop(&self) {
        self.closing.store(true, Ordering::Release);
        self.relative_requested.store(false, Ordering::Release);
        self.apply_capture();
        self.events.stop();
        self.stop.stop();
    }

    pub fn set_cursor(&self, request: crate::cursor::Request) {
        if self.closing.load(Ordering::Acquire) {
            return;
        }
        self.cursor_pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .submit(request);
        self.select_cursor();
    }
    fn select_cursor(&self) {
        let pending = std::mem::take(
            &mut *self
                .cursor_pending
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        let hidden = self.relative_mouse.load(Ordering::Acquire);
        let window = self.ns_window;
        post(move |mtm| {
            CURSOR.with_borrow_mut(|cursor| {
                cursor.apply(pending, mtm);
                // Only the stream window's pointer changes; menus keep theirs.
                let window = unsafe { &*(window as *const NSWindow) };
                if window.isKeyWindow() {
                    cursor.select(hidden);
                }
            })
        });
    }

    pub fn native_display_size(&self) -> (u32, u32) {
        let window = self.ns_window;
        on_main(move |mtm| {
            let window = unsafe { &*(window as *const NSWindow) };
            let screen = window
                .screen()
                .or_else(|| objc2_app_kit::NSScreen::mainScreen(mtm));
            screen.map_or((1920, 1080), |screen| {
                // Physical pixels, not the point size of the desktop.
                let size = screen.convertRectToBacking(screen.frame()).size;
                (size.width.max(1.0) as u32, size.height.max(1.0) as u32)
            })
        })
    }

    pub fn set_fullscreen(&self, enable: bool) {
        if self.closing.load(Ordering::Acquire) {
            return;
        }
        let window = self.ns_window;
        post(move |_| {
            let window = unsafe { &*(window as *const NSWindow) };
            if window.styleMask().contains(NSWindowStyleMask::FullScreen) != enable {
                window.toggleFullScreen(None);
            }
        });
    }
    /// Applied by the main thread's next window sync.
    pub fn set_wake_lock(&self, enable: bool) {
        if !self.closing.load(Ordering::Acquire) {
            self.wake_requested.store(enable, Ordering::Release);
        }
    }
    fn apply_wake_lock(&self, mtm: MainThreadMarker) {
        let visible =
            !self.ns_window(mtm).isMiniaturized() && !self.closing.load(Ordering::Acquire);
        self.wake_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .update(self.wake_requested.load(Ordering::Acquire), visible);
    }

    pub fn open_stream_settings(&self) {
        let window = active().filter(|w| std::ptr::eq(&**w, self));
        if let (Some(window), Some(manager)) = (window, self.manager()) {
            crate::native_core::open_stream_settings(window, manager);
        }
    }
    pub fn open_stats(&self) {
        crate::native_core::ui::open_stats(self.stats.clone());
    }

    fn release_buttons(&self) {
        let pressed = self.pressed_buttons.swap(0, Ordering::AcqRel);
        for button in 0..5 {
            if pressed & (1 << button) != 0 {
                self.events.push(Event::Button(false, button, 0, 0));
            }
        }
        self.local_shortcuts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// Focus, size, fullscreen, visibility and pasteboard changes, observed on
    /// the main thread whenever AppKit delivers an event.
    fn sync(self: &Arc<Self>, mtm: MainThreadMarker) {
        let window = self.ns_window(mtm);
        if !window.isVisible() && !window.isMiniaturized() {
            // The user closed the stream window: end the session.
            if !self.closing.load(Ordering::Acquire) {
                self.request_stop();
            }
            return;
        }
        let focus = window.isKeyWindow() && !window.isMiniaturized();
        if self.has_focus.swap(focus, Ordering::AcqRel) != focus {
            if !focus {
                self.release_buttons();
            }
            self.events.push(Event::Focus(focus));
            self.apply_capture();
            #[cfg(feature = "bud")]
            self.sync_hid_acquired();
            if focus {
                self.select_cursor();
                // A local copy made while another app was active.
                let count = NSPasteboard::generalPasteboard().changeCount();
                if self.pasteboard_seen.swap(count, Ordering::AcqRel) != count {
                    self.events.push(Event::Clipboard);
                }
            }
        }
        let size = window.contentRectForFrameRect(window.frame()).size;
        let size = (size.width as i32, size.height as i32);
        let mut dimensions = self.dimensions.lock().unwrap_or_else(|e| e.into_inner());
        if size.0 > 0 && size.1 > 0 && *dimensions != size {
            *dimensions = size;
            drop(dimensions);
            self.events.push(Event::Size(size.0, size.1));
            CURSOR.with_borrow_mut(|cursor| cursor.refresh_size(mtm));
        }
        self.apply_wake_lock(mtm);
        let fullscreen = window.styleMask().contains(NSWindowStyleMask::FullScreen);
        if self.fullscreen_active.swap(fullscreen, Ordering::AcqRel) != fullscreen {
            self.events.push(Event::Fullscreen(fullscreen));
        }
    }

    fn shortcut(&self, code: &'static str, down: bool, repeated: bool, mods: i32) -> bool {
        use crate::input::{Shortcut, ShortcutRoute};
        let route = self
            .local_shortcuts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .route(code, down, repeated, crate::input::shortcut(code, mods));
        match route {
            ShortcutRoute::Forward => return false,
            ShortcutRoute::Consume => {}
            ShortcutRoute::Activate(Shortcut::Menu) => self.open_stream_settings(),
            ShortcutRoute::Activate(Shortcut::Statistics) => self.open_stats(),
            ShortcutRoute::Activate(Shortcut::SecureAttention) => self.events.push(
                Event::NativeSession(crate::native_core::session_model::Command::SecureAttention),
            ),
            ShortcutRoute::Activate(action) => self.events.push(Event::Shortcut(action)),
        }
        true
    }

    fn button(&self, down: bool, button: i32, x: i32, y: i32) {
        let bit = 1usize << button;
        if down {
            self.pressed_buttons.fetch_or(bit, Ordering::AcqRel);
            #[cfg(feature = "bud")]
            if self.hid_mouse_active.load(Ordering::Acquire)
                && !self.relative_mouse.load(Ordering::Acquire)
            {
                self.events.push(Event::Motion(x, y));
            }
        } else {
            self.pressed_buttons.fetch_and(!bit, Ordering::AcqRel);
        }
        self.events.push(Event::Button(down, button, x, y));
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        let (window, layer) = (self.ns_window, self.video_layer);
        let wake_lock =
            std::mem::take(&mut *self.wake_lock.lock().unwrap_or_else(|e| e.into_inner()));
        post(move |_| unsafe {
            let mut wake_lock = wake_lock;
            wake_lock.update(false, false);
            let window = Retained::from_raw(window as *mut NSWindow);
            if let Some(window) = &window {
                window.orderOut(None);
            }
            drop(Retained::from_raw(layer as *mut AnyObject));
        });
    }
}

/// Observes the active stream window once per main-loop iteration.
pub fn sync_active(mtm: MainThreadMarker) {
    if let Some(window) = active().filter(|w| !w.closing.load(Ordering::Acquire)) {
        window.sync(mtm);
    }
}

/// Compact modifier mask shared with the Windows adapter.
fn modifiers(flags: NSEventModifierFlags) -> i32 {
    [
        (NSEventModifierFlags::Shift, 1),
        (NSEventModifierFlags::Control, 2),
        (NSEventModifierFlags::Option, 4),
        (NSEventModifierFlags::Command, 8),
        (NSEventModifierFlags::CapsLock, 16),
    ]
    .into_iter()
    .filter(|(flag, _)| flags.contains(*flag))
    .fold(0, |mods, (_, bit)| mods | bit)
}

/// Handles one AppKit event for the stream window on the main thread. Returns
/// true when the event was consumed and must not reach AppKit (which would
/// otherwise beep for unhandled keys).
pub fn handle_event(event: &NSEvent, mtm: MainThreadMarker) -> bool {
    let Some(window) = active() else {
        return false;
    };
    if window.closing.load(Ordering::Acquire) {
        return false;
    }
    let ours = event
        .window(mtm)
        .is_some_and(|target| std::ptr::eq(&*target, window.ns_window(mtm)));
    if !ours {
        return false;
    }
    let (_, height) = *window.dimensions.lock().unwrap_or_else(|e| e.into_inner());
    let location = event.locationInWindow();
    let (x, y) = (location.x as i32, height - location.y as i32);
    let mouse_button = |number: isize| match number {
        0 => 0,
        1 => 2,
        2 => 1,
        3 => 3,
        _ => 4,
    };
    match event.r#type() {
        NSEventType::LeftMouseDown | NSEventType::RightMouseDown | NSEventType::OtherMouseDown => {
            window.button(true, mouse_button(event.buttonNumber()), x, y);
        }
        NSEventType::LeftMouseUp | NSEventType::RightMouseUp | NSEventType::OtherMouseUp => {
            window.button(false, mouse_button(event.buttonNumber()), x, y);
        }
        NSEventType::MouseMoved
        | NSEventType::LeftMouseDragged
        | NSEventType::RightMouseDragged
        | NSEventType::OtherMouseDragged => {
            if window.relative_mouse.load(Ordering::Acquire) {
                let (dx, dy) = (event.deltaX() as i32, event.deltaY() as i32);
                if dx != 0 || dy != 0 {
                    window.events.push(Event::RelativeMotion(dx, dy));
                }
            } else {
                window.events.push(Event::Motion(x, y));
            }
        }
        NSEventType::ScrollWheel => {
            // Windows wheel units: 120 per notch, +y down and +x right.
            // ponytail: 1.2 per trackpad point is a guess; tune if it feels off.
            let scale = if event.hasPreciseScrollingDeltas() {
                1.2
            } else {
                120.0
            };
            let (dx, dy) = SCROLL.with(|remainder| {
                let (rx, ry) = remainder.get();
                let dx = rx - event.scrollingDeltaX() * scale;
                let dy = ry - event.scrollingDeltaY() * scale;
                // Keep fractions so slow trackpad motion still adds up.
                remainder.set((dx.fract(), dy.fract()));
                (dx as i32, dy as i32)
            });
            if dx != 0 || dy != 0 {
                window.events.push(Event::Scroll(dx, dy));
            }
        }
        NSEventType::KeyDown | NSEventType::KeyUp => {
            let down = event.r#type() == NSEventType::KeyDown;
            let flags = event.modifierFlags();
            let mods = modifiers(flags);
            let code = key_code(event.keyCode());
            // Session shortcuts win over menu equivalents, so Command+Q
            // disconnects like Parsec instead of quitting the app.
            if code.is_some_and(|code| window.shortcut(code, down, down && event.isARepeat(), mods))
            {
                return true;
            }
            // Command menu equivalents (Settings, Edit, ...) stay local; other
            // Command chords reach the host.
            if down
                && flags.contains(NSEventModifierFlags::Command)
                && objc2_app_kit::NSApplication::sharedApplication(mtm)
                    .mainMenu()
                    .is_some_and(|menu| menu.performKeyEquivalent(event))
            {
                return true;
            }
            if let Some(code) = code {
                window.events.push(Event::Key(down, code, mods));
            }
            return true;
        }
        NSEventType::FlagsChanged => {
            if let Some(code) = key_code(event.keyCode()) {
                let flags = event.modifierFlags();
                let flag = match code {
                    "ShiftLeft" | "ShiftRight" => NSEventModifierFlags::Shift,
                    "ControlLeft" | "ControlRight" => NSEventModifierFlags::Control,
                    "AltLeft" | "AltRight" => NSEventModifierFlags::Option,
                    "MetaLeft" | "MetaRight" => NSEventModifierFlags::Command,
                    "CapsLock" => NSEventModifierFlags::CapsLock,
                    _ => return false,
                };
                let down = flags.contains(flag);
                if !window.shortcut(code, down, false, modifiers(flags)) {
                    window.events.push(Event::Key(down, code, modifiers(flags)));
                }
            }
        }
        _ => {}
    }
    false
}

/// macOS virtual key codes (Carbon kVK_*) to the shared physical key names.
fn key_code(code: u16) -> Option<&'static str> {
    Some(match code {
        0 => "KeyA",
        1 => "KeyS",
        2 => "KeyD",
        3 => "KeyF",
        4 => "KeyH",
        5 => "KeyG",
        6 => "KeyZ",
        7 => "KeyX",
        8 => "KeyC",
        9 => "KeyV",
        10 => "IntlBackslash",
        11 => "KeyB",
        12 => "KeyQ",
        13 => "KeyW",
        14 => "KeyE",
        15 => "KeyR",
        16 => "KeyY",
        17 => "KeyT",
        18 => "Digit1",
        19 => "Digit2",
        20 => "Digit3",
        21 => "Digit4",
        22 => "Digit6",
        23 => "Digit5",
        24 => "Equal",
        25 => "Digit9",
        26 => "Digit7",
        27 => "Minus",
        28 => "Digit8",
        29 => "Digit0",
        30 => "BracketRight",
        31 => "KeyO",
        32 => "KeyU",
        33 => "BracketLeft",
        34 => "KeyI",
        35 => "KeyP",
        36 => "Enter",
        37 => "KeyL",
        38 => "KeyJ",
        39 => "Quote",
        40 => "KeyK",
        41 => "Semicolon",
        42 => "Backslash",
        43 => "Comma",
        44 => "Slash",
        45 => "KeyN",
        46 => "KeyM",
        47 => "Period",
        48 => "Tab",
        49 => "Space",
        50 => "Backquote",
        51 => "Backspace",
        53 => "Escape",
        54 => "MetaRight",
        55 => "MetaLeft",
        56 => "ShiftLeft",
        57 => "CapsLock",
        58 => "AltLeft",
        59 => "ControlLeft",
        60 => "ShiftRight",
        61 => "AltRight",
        62 => "ControlRight",
        64 => "F17",
        65 => "NumpadDecimal",
        67 => "NumpadMultiply",
        69 => "NumpadAdd",
        71 => "NumLock",
        75 => "NumpadDivide",
        76 => "NumpadEnter",
        78 => "NumpadSubtract",
        79 => "F18",
        80 => "F19",
        81 => "NumpadEqual",
        82 => "Numpad0",
        83 => "Numpad1",
        84 => "Numpad2",
        85 => "Numpad3",
        86 => "Numpad4",
        87 => "Numpad5",
        88 => "Numpad6",
        89 => "Numpad7",
        90 => "F20",
        91 => "Numpad8",
        92 => "Numpad9",
        96 => "F5",
        97 => "F6",
        98 => "F7",
        99 => "F3",
        100 => "F8",
        101 => "F9",
        103 => "F11",
        105 => "F13",
        106 => "F16",
        107 => "F14",
        109 => "F10",
        110 => "ContextMenu",
        111 => "F12",
        113 => "F15",
        114 => "Insert",
        115 => "Home",
        116 => "PageUp",
        117 => "Delete",
        118 => "F4",
        119 => "End",
        120 => "F2",
        121 => "PageDown",
        122 => "F1",
        123 => "ArrowLeft",
        124 => "ArrowRight",
        125 => "ArrowDown",
        126 => "ArrowUp",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mac_keys_and_modifiers_use_shared_physical_names_and_mask() {
        for (code, name) in [
            (0, "KeyA"),
            (18, "Digit1"),
            (29, "Digit0"),
            (36, "Enter"),
            (55, "MetaLeft"),
            (54, "MetaRight"),
            (58, "AltLeft"),
            (59, "ControlLeft"),
            (82, "Numpad0"),
            (92, "Numpad9"),
            (115, "Home"),
            (117, "Delete"),
            (119, "End"),
            (122, "F1"),
            (111, "F12"),
            (126, "ArrowUp"),
        ] {
            assert_eq!(key_code(code), Some(name));
        }
        assert_eq!(key_code(52), None);
        assert_eq!(modifiers(NSEventModifierFlags::empty()), 0);
        assert_eq!(
            modifiers(NSEventModifierFlags::Shift | NSEventModifierFlags::Control),
            3
        );
        assert_eq!(
            modifiers(NSEventModifierFlags::Option | NSEventModifierFlags::Command),
            12
        );
        // Command+Option chords use the Parsec macOS shortcut mask.
        assert_eq!(
            crate::input::shortcut(
                "Backquote",
                modifiers(NSEventModifierFlags::Command | NSEventModifierFlags::Option)
            ),
            Some(crate::input::Shortcut::Disconnect)
        );
    }
}
