//! AppKit stream-settings window. Choices, parsing and host display rules come
//! from the shared session model; this adapter only maps controls to commands.
use super::session_model::{
    current_choice, resolution_text, Command, PanelFields, Status, BANDWIDTH_CHOICES, FPS_CHOICES,
    HOST_RESOLUTION, RESOLUTION_CHOICES,
};
use crate::window::{Event, Window};
use crate::window_macos::{on_main, post};
use objc2::{
    define_class, msg_send, rc::Retained, runtime::NSObject, sel, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSPopUpButton, NSTextField, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::{
    cell::RefCell,
    sync::{atomic::Ordering, Arc, Weak},
};

const WIDTH: f64 = 460.0;
const HEIGHT: f64 = 470.0;

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "EvoParseqSessionActions"]
    struct Actions;

    impl Actions {
        #[unsafe(method(apply:))]
        fn apply(&self, _sender: &NSObject) {
            with_panel(|p| match p.collect() {
                Ok(video) => {
                    p.apply.setEnabled(false);
                    p.status.setStringValue(&NSString::from_str("Applying..."));
                    p.send(Command::Apply(video));
                }
                Err(error) => p.status.setStringValue(&NSString::from_str(&error.to_string())),
            });
        }
        #[unsafe(method(display:))]
        fn display(&self, _sender: &NSObject) {
            with_panel(|p| {
                let Some(window) = p.stream.upgrade() else { return };
                let displays = window.session.displays.lock().unwrap_or_else(|e| e.into_inner());
                if displays.hid_mouse
                    || displays.loading
                    || window.session.video_pending.load(Ordering::Acquire)
                {
                    return;
                }
                drop(displays);
                let index = usize::try_from(p.display.indexOfSelectedItem()).ok();
                if let Some(id) = index.and_then(|i| p.display_ids.get(i)).cloned() {
                    p.display.setEnabled(false);
                    p.refresh.setEnabled(false);
                    p.send(Command::SelectDisplay(id));
                }
            });
        }
        #[unsafe(method(refreshDisplays:))]
        fn refresh_displays(&self, _sender: &NSObject) {
            with_panel(|p| p.send(Command::RefreshDisplays));
        }
    }
);

struct Panel {
    window: Retained<NSWindow>,
    _actions: Retained<Actions>,
    stream: Weak<Window>,
    // Resolution, bandwidth and FPS; a custom saved value is a final
    // "keep current" entry that resubmits the initial text.
    fields: [Retained<NSPopUpButton>; 3],
    initial: [String; 3],
    pacing: Retained<NSPopUpButton>,
    orientation: Retained<NSPopUpButton>,
    immersive: Retained<NSPopUpButton>,
    fullscreen: Retained<NSButton>,
    display: Retained<NSPopUpButton>,
    display_ids: Vec<String>,
    display_hint: Retained<NSTextField>,
    refresh: Retained<NSButton>,
    apply: Retained<NSButton>,
    status: Retained<NSTextField>,
}
thread_local! {
    static PANEL: RefCell<Option<Panel>> = const { RefCell::new(None) };
}
fn with_panel(f: impl FnOnce(&mut Panel)) {
    PANEL.with_borrow_mut(|p| {
        if let Some(p) = p.as_mut().filter(|p| p.window.isVisible()) {
            f(p)
        }
    });
}

fn select(popup: &NSPopUpButton, items: &[&str], value: &str) -> bool {
    for item in items {
        popup.addItemWithTitle(&NSString::from_str(item));
    }
    match items.iter().position(|s| *s == value) {
        Some(index) => {
            popup.selectItemAtIndex(index as isize);
            false
        }
        None => {
            popup.addItemWithTitle(&NSString::from_str(&current_choice(value)));
            popup.selectItemAtIndex(items.len() as isize);
            true
        }
    }
}
fn selected(popup: &NSPopUpButton) -> usize {
    usize::try_from(popup.indexOfSelectedItem()).unwrap_or(0)
}

impl Panel {
    fn send(&self, command: Command) {
        if let Some(window) = self.stream.upgrade() {
            window.events.push(Event::NativeSession(command));
        }
    }
    fn collect(&self) -> anyhow::Result<super::profile::Video> {
        let field = |i: usize| {
            let lists = [RESOLUTION_CHOICES, BANDWIDTH_CHOICES, FPS_CHOICES];
            lists[i]
                .get(selected(&self.fields[i]))
                .map_or_else(|| self.initial[i].clone(), |s| (*s).to_owned())
        };
        PanelFields {
            resolution: &field(0),
            bandwidth: &field(1),
            fps: &field(2),
            constant_fps: selected(&self.pacing) == 1,
            rotated: selected(&self.orientation) == 1,
            immersive: match selected(&self.immersive) {
                1 => 3,
                2 => 2,
                3 => 1,
                _ => 0,
            },
            fullscreen: self.fullscreen.state() != 0,
        }
        .parse()
    }
    fn update_displays(&mut self) {
        let Some(window) = self.stream.upgrade() else {
            return;
        };
        let displays = window
            .session
            .displays
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.display.removeAllItems();
        self.display_ids.clear();
        let mut current = None;
        for (index, output) in displays.outputs.iter().enumerate() {
            let label = if output.adapter.is_empty() {
                output.name.clone()
            } else {
                format!("{} ({})", output.name, output.adapter)
            };
            // Titles must be unique in a pop-up; the index keeps duplicates apart.
            self.display
                .addItemWithTitle(&NSString::from_str(&format!("{}. {label}", index + 1)));
            if displays.current.as_deref() == Some(output.id.as_str()) {
                current = Some(index);
            }
            self.display_ids.push(output.id.clone());
        }
        match current {
            Some(index) => self.display.selectItemAtIndex(index as isize),
            None => {
                self.display
                    .addItemWithTitle(&NSString::from_str(if displays.loading {
                        "Loading displays..."
                    } else if displays.outputs.is_empty() {
                        "Display list unavailable"
                    } else {
                        "Select host display"
                    }));
                self.display
                    .selectItemAtIndex(self.display_ids.len() as isize);
            }
        }
        let busy = window.session.video_pending.load(Ordering::Acquire);
        self.display.setEnabled(
            !displays.hid_mouse && !displays.loading && !busy && !displays.outputs.is_empty(),
        );
        self.refresh.setEnabled(!displays.loading && !busy);
        self.display_hint
            .setStringValue(&NSString::from_str(if displays.hid_mouse {
                if window.session.hid_primary_ready.load(Ordering::Acquire) {
                    "HID mouse: primary host display only. Switching disabled."
                } else {
                    "HID mouse paused until the primary host display is verified."
                }
            } else {
                "Switching uses the selected display's host resolution."
            }));
    }
}

pub fn open(stream: Arc<Window>, manager: Arc<crate::connection_settings::Manager>) {
    let video = stream
        .session
        .panel_video(manager.view().0.native_core.video);
    let stream = Arc::downgrade(&stream);
    on_main(move |mtm| {
        let reuse = PANEL.with_borrow(|p| {
            p.as_ref()
                .filter(|p| p.window.isVisible() && p.stream.ptr_eq(&stream))
                .map(|p| p.window.makeKeyAndOrderFront(None))
                .is_some()
        });
        if !reuse {
            build(mtm, stream, &video);
            stream_refresh();
        }
    });
}

fn stream_refresh() {
    with_panel(|p| p.send(Command::RefreshDisplays));
}

fn build(mtm: MainThreadMarker, stream: Weak<Window>, video: &super::profile::Video) {
    let actions: Retained<Actions> = unsafe { msg_send![Actions::alloc(mtm), init] };
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, HEIGHT)),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("Stream Settings"));
    let content = window.contentView().expect("window content view");
    let top = std::cell::Cell::new(20.0);
    let row = |caption: Option<&str>, height: f64| {
        if let Some(caption) = caption {
            let label = NSTextField::labelWithString(&NSString::from_str(caption), mtm);
            label.setFrame(NSRect::new(
                NSPoint::new(20.0, HEIGHT - top.get() - 22.0),
                NSSize::new(150.0, 18.0),
            ));
            content.addSubview(&label);
        }
        let rect = NSRect::new(
            NSPoint::new(170.0, HEIGHT - top.get() - height),
            NSSize::new(WIDTH - 190.0, height),
        );
        top.set(top.get() + height + 10.0);
        rect
    };
    let popup = |caption: &str| {
        let popup =
            NSPopUpButton::initWithFrame_pullsDown(mtm.alloc(), row(Some(caption), 26.0), false);
        content.addSubview(&popup);
        popup
    };
    let initial = [
        resolution_text(video.resolution),
        video.bandwidth_mbps.unwrap_or(10).to_string(),
        video.fps.unwrap_or(60).to_string(),
    ];
    let fields = [
        popup("Resolution"),
        popup("Bandwidth (Mbps)"),
        popup("Frame rate (FPS)"),
    ];
    for (i, items) in [RESOLUTION_CHOICES, BANDWIDTH_CHOICES, FPS_CHOICES]
        .iter()
        .enumerate()
    {
        select(&fields[i], items, &initial[i]);
    }
    let pacing = popup("Frame pacing");
    select(
        &pacing,
        &["Adaptive", "Constant FPS"],
        if video.constant_fps == Some(true) {
            "Constant FPS"
        } else {
            "Adaptive"
        },
    );
    let orientation = popup("Orientation");
    select(
        &orientation,
        &["Default", "Rotated"],
        if video.rotated == Some(true) {
            "Rotated"
        } else {
            "Default"
        },
    );
    let immersive = popup("Immersive mode");
    select(
        &immersive,
        &["Disabled", "Mouse", "Keyboard", "Mouse + keyboard"],
        match video.immersive {
            1 => "Mouse + keyboard",
            2 => "Keyboard",
            3 => "Mouse",
            _ => "Disabled",
        },
    );
    let fullscreen = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str("Fullscreen"),
            None,
            None,
            mtm,
        )
    };
    fullscreen.setFrame(row(None, 20.0));
    fullscreen.setState(isize::from(video.fullscreen));
    content.addSubview(&fullscreen);
    let display = popup("Host display");
    unsafe {
        display.setTarget(Some(&actions));
        display.setAction(Some(sel!(display:)));
    }
    let display_hint = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
    display_hint.setFrame(row(None, 34.0));
    content.addSubview(&display_hint);
    let button = |title: &str, action, x: f64| {
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(&actions),
                Some(action),
                mtm,
            )
        };
        button.setFrame(NSRect::new(NSPoint::new(x, 60.0), NSSize::new(130.0, 32.0)));
        content.addSubview(&button);
        button
    };
    let refresh = button("Refresh displays", sel!(refreshDisplays:), 20.0);
    let apply = button("Apply", sel!(apply:), WIDTH - 150.0);
    apply.setKeyEquivalent(&NSString::from_str("\r"));
    let status = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
    status.setFrame(NSRect::new(
        NSPoint::new(20.0, 12.0),
        NSSize::new(WIDTH - 40.0, 40.0),
    ));
    content.addSubview(&status);
    let mut panel = Panel {
        window,
        _actions: actions,
        stream,
        fields,
        initial,
        pacing,
        orientation,
        immersive,
        fullscreen,
        display,
        display_ids: Vec::new(),
        display_hint,
        refresh,
        apply,
        status,
    };
    panel.update_displays();
    panel.window.center();
    panel.window.makeKeyAndOrderFront(None);
    PANEL.with_borrow_mut(|p| *p = Some(panel));
}

/// Host-settings feedback from the session thread.
pub fn status(status: Status) {
    post(move |_| {
        with_panel(|p| {
            let pending = p
                .stream
                .upgrade()
                .is_some_and(|w| w.session.video_pending.load(Ordering::Acquire));
            p.apply.setEnabled(status != Status::Waiting && !pending);
            p.status.setStringValue(&NSString::from_str(status.text()));
            p.update_displays();
        })
    });
}

/// Display list changes; a completed switch returns resolution to the host's.
pub fn displays(selected: bool) {
    post(move |_| {
        with_panel(|p| {
            if selected {
                p.fields[0].selectItemAtIndex(0);
                p.initial[0] = HOST_RESOLUTION.into();
            }
            p.update_displays();
        })
    });
}

/// The session ended: a panel for it must not send commands to a later one.
pub fn close() {
    post(|_| {
        PANEL.with_borrow_mut(|p| {
            if let Some(p) = p.take() {
                p.window.orderOut(None);
            }
        })
    });
}
