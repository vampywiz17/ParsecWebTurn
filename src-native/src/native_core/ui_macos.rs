//! AppKit application loop, menu bar, account/computer picker and statistics.
//! Controls live on the main thread; the sign-in and picker controllers stay
//! on the account thread and receive button actions over a channel.
use super::account::{self, Login, Session};
use super::picker::{self, Picker, Refresh};
use super::sign_in::{Completion, SignIn};
use crate::window_macos::on_main;
use anyhow::Result;
use objc2::{
    define_class, msg_send, rc::Retained, runtime::NSObject, sel, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSAlert, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSButton,
    NSEventMask, NSFont, NSMenu, NSMenuItem, NSPopUpButton, NSScrollView, NSSecureTextField,
    NSTextField, NSTextView, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize, NSString};
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub struct Selection {
    pub session: Arc<Session>,
    pub peer: String,
}

static QUIT: AtomicBool = AtomicBool::new(false);
static MANAGER: Mutex<Option<Arc<crate::connection_settings::Manager>>> = Mutex::new(None);
static COMMANDS: Mutex<Option<mpsc::Sender<Command>>> = Mutex::new(None);

enum Command {
    Login(Login, bool),
    Refresh,
    Connect(String),
    Logout,
}

define_class!(
    // Target for menu items and launcher buttons; it only forwards actions.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "EvoParseqActions"]
    struct Actions;

    impl Actions {
        #[unsafe(method(about:))]
        fn about(&self, _sender: &NSObject) {
            message(&format!(
                "EvoParseq {}\nNative Rust client for existing Parsec hosts.",
                env!("CARGO_PKG_VERSION")
            ));
        }
        #[unsafe(method(settings:))]
        fn settings(&self, _sender: &NSObject) {
            let manager = MANAGER.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(manager) = manager {
                crate::connection_settings_ui_macos::open(manager);
            }
        }
        #[unsafe(method(quit:))]
        fn quit(&self, _sender: &NSObject) {
            QUIT.store(true, Ordering::Release);
            if let Some(window) = crate::window_macos::active() {
                window.request_stop();
            }
        }
        #[unsafe(method(streamSettings:))]
        fn stream_settings(&self, _sender: &NSObject) {
            if let Some(window) = crate::window_macos::active() {
                window.open_stream_settings();
            }
        }
        #[unsafe(method(stats:))]
        fn stats(&self, _sender: &NSObject) {
            if let Some(window) = crate::window_macos::active() {
                window.open_stats();
            }
        }
        #[unsafe(method(fullscreen:))]
        fn fullscreen(&self, _sender: &NSObject) {
            session(crate::window::Event::Shortcut(crate::input::Shortcut::Fullscreen));
        }
        #[unsafe(method(release:))]
        fn release(&self, _sender: &NSObject) {
            session(crate::window::Event::Shortcut(crate::input::Shortcut::ReleaseInput));
        }
        #[unsafe(method(attention:))]
        fn attention(&self, _sender: &NSObject) {
            session(crate::window::Event::NativeSession(
                super::session_model::Command::SecureAttention,
            ));
        }
        #[unsafe(method(disconnect:))]
        fn disconnect(&self, _sender: &NSObject) {
            session(crate::window::Event::NativeSession(
                super::session_model::Command::Disconnect,
            ));
        }
        #[unsafe(method(login:))]
        fn login(&self, _sender: &NSObject) {
            LAUNCHER.with_borrow(|l| {
                let Some(l) = l else { return };
                let login = Login {
                    email: l.email.stringValue().to_string(),
                    password: zeroize::Zeroizing::new(l.password.stringValue().to_string()),
                    tfa: zeroize::Zeroizing::new(l.tfa.stringValue().to_string()),
                };
                send(Command::Login(login, l.remember.state() != 0));
            });
        }
        #[unsafe(method(refresh:))]
        fn refresh(&self, _sender: &NSObject) {
            send(Command::Refresh);
        }
        #[unsafe(method(connect:))]
        fn connect(&self, _sender: &NSObject) {
            LAUNCHER.with_borrow(|l| {
                if let Some(l) = l {
                    send(Command::Connect(l.peer.stringValue().to_string()));
                }
            });
        }
        #[unsafe(method(logout:))]
        fn logout(&self, _sender: &NSObject) {
            send(Command::Logout);
        }
        #[unsafe(method(hostSelected:))]
        fn host_selected(&self, _sender: &NSObject) {
            LAUNCHER.with_borrow(|l| {
                let Some(l) = l else { return };
                let index = l.hosts.indexOfSelectedItem();
                if let Some(peer) = usize::try_from(index).ok().and_then(|i| l.peers.get(i)) {
                    l.peer.setStringValue(&NSString::from_str(peer));
                }
            });
        }
    }
);

fn send(command: Command) {
    if let Some(tx) = COMMANDS.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.send(command);
    }
}
fn session(event: crate::window::Event) {
    if let Some(window) = crate::window_macos::active() {
        window.events.push(event);
    }
}

struct Launcher {
    window: Retained<NSWindow>,
    email: Retained<NSTextField>,
    password: Retained<NSSecureTextField>,
    tfa: Retained<NSTextField>,
    remember: Retained<NSButton>,
    hosts: Retained<NSPopUpButton>,
    peer: Retained<NSTextField>,
    status: Retained<NSTextField>,
    login_controls: Vec<Retained<NSView>>,
    picker_controls: Vec<Retained<NSView>>,
    buttons: Vec<Retained<NSButton>>,
    peers: Vec<String>,
}
thread_local! {
    static ACTIONS: RefCell<Option<Retained<Actions>>> = const { RefCell::new(None) };
    static LAUNCHER: RefCell<Option<Launcher>> = const { RefCell::new(None) };
}
fn actions(mtm: MainThreadMarker) -> Retained<Actions> {
    ACTIONS.with_borrow_mut(|a| {
        a.get_or_insert_with(|| unsafe { msg_send![Actions::alloc(mtm), init] })
            .clone()
    })
}

fn item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<objc2::runtime::Sel>,
    key: &str,
) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    };
    if action.is_some_and(|a| {
        a.name().to_bytes().ends_with(b":") && !STANDARD.contains(&a.name().to_bytes())
    }) {
        unsafe { item.setTarget(Some(&actions(mtm))) };
    }
    item
}
// Responder-chain actions handled by AppKit text fields and windows.
const STANDARD: [&[u8]; 6] = [
    b"cut:",
    b"copy:",
    b"paste:",
    b"selectAll:",
    b"undo:",
    b"performMiniaturize:",
];

fn menu(
    mtm: MainThreadMarker,
    title: &str,
    items: Vec<Retained<NSMenuItem>>,
) -> Retained<NSMenuItem> {
    let menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(title));
    for item in items {
        menu.addItem(&item);
    }
    let top = item(mtm, title, None, "");
    top.setSubmenu(Some(&menu));
    top
}

fn install_menu(mtm: MainThreadMarker, app: &NSApplication) {
    let separator = || NSMenuItem::separatorItem(mtm);
    let bar = NSMenu::new(mtm);
    bar.addItem(&menu(
        mtm,
        "EvoParseq",
        vec![
            item(mtm, "About EvoParseq", Some(sel!(about:)), ""),
            separator(),
            item(mtm, "Settings…", Some(sel!(settings:)), ","),
            separator(),
            item(mtm, "Quit EvoParseq", Some(sel!(quit:)), "q"),
        ],
    ));
    bar.addItem(&menu(
        mtm,
        "Edit",
        vec![
            item(mtm, "Undo", Some(sel!(undo:)), "z"),
            separator(),
            item(mtm, "Cut", Some(sel!(cut:)), "x"),
            item(mtm, "Copy", Some(sel!(copy:)), "c"),
            item(mtm, "Paste", Some(sel!(paste:)), "v"),
            item(mtm, "Select All", Some(sel!(selectAll:)), "a"),
        ],
    ));
    bar.addItem(&menu(
        mtm,
        "Session",
        vec![
            item(
                mtm,
                "Stream Settings… (⌘⇧M)",
                Some(sel!(streamSettings:)),
                "",
            ),
            item(mtm, "Connection Stats (⌘⇧S)", Some(sel!(stats:)), ""),
            item(mtm, "Toggle Fullscreen (⌘⇧W)", Some(sel!(fullscreen:)), ""),
            item(mtm, "Release Input (⌘⌥Z)", Some(sel!(release:)), ""),
            item(mtm, "Send Ctrl+Alt+Del (⌃⌥⌫)", Some(sel!(attention:)), ""),
            separator(),
            item(
                mtm,
                "Disconnect (⌘⌥`, ⌘W or ⌘Q)",
                Some(sel!(disconnect:)),
                "",
            ),
        ],
    ));
    bar.addItem(&menu(
        mtm,
        "Window",
        vec![item(mtm, "Minimize", Some(sel!(performMiniaturize:)), "m")],
    ));
    app.setMainMenu(Some(&bar));
}

/// Runs AppKit on the main thread and `run` on a separate account thread.
pub fn run_application(run: impl FnOnce() + Send + 'static) {
    let mtm = MainThreadMarker::new().expect("AppKit must start on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    install_menu(mtm, &app);
    app.finishLaunching();
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    let done = Arc::new(AtomicBool::new(false));
    let finished = done.clone();
    let spawned = std::thread::Builder::new()
        .name("evoparseq-account".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            run();
            finished.store(true, Ordering::Release);
        });
    if spawned.is_err() {
        eprintln!("EvoParseq could not start its account thread.");
        return;
    }
    while !done.load(Ordering::Acquire) {
        objc2::rc::autoreleasepool(|_| {
            // Bounded wait so window state is observed without input events.
            let event = unsafe {
                app.nextEventMatchingMask_untilDate_inMode_dequeue(
                    NSEventMask::Any,
                    Some(&NSDate::dateWithTimeIntervalSinceNow(0.25)),
                    NSDefaultRunLoopMode,
                    true,
                )
            };
            crate::window_macos::sync_active(mtm);
            if let Some(event) = event {
                if !crate::window_macos::handle_event(&event, mtm) {
                    app.sendEvent(&event);
                }
            }
        });
    }
}

fn message(text: &str) {
    let text = text.to_owned();
    on_main(move |mtm| {
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str("EvoParseq"));
        alert.setInformativeText(&NSString::from_str(&text));
        alert.runModal();
    });
}
/// Same signature as the Windows adapter; there is no owner window here.
pub unsafe fn message_box(_owner: *mut std::ffi::c_void, text: &str) {
    message(text);
}

/// AppKit frames grow upward; layout reads top-down inside a fixed launcher.
const WIDTH: f64 = 600.0;
const HEIGHT: f64 = 440.0;
fn frame(x: f64, top: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(x, HEIGHT - top - height),
        NSSize::new(width, height),
    )
}
fn label(mtm: MainThreadMarker, text: &str, rect: NSRect) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setFrame(rect);
    label
}
fn field(mtm: MainThreadMarker, text: &str, rect: NSRect) -> Retained<NSTextField> {
    let field = NSTextField::textFieldWithString(&NSString::from_str(text), mtm);
    field.setFrame(rect);
    field
}
fn button(
    mtm: MainThreadMarker,
    title: &str,
    action: objc2::runtime::Sel,
    rect: NSRect,
) -> Retained<NSButton> {
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(&actions(mtm)),
            Some(action),
            mtm,
        )
    };
    button.setFrame(rect);
    button
}

fn create_launcher(mtm: MainThreadMarker, email: &str, peer: &str, remember: bool, status: &str) {
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, HEIGHT)),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("EvoParseq"));
    let content = window.contentView().expect("window content view");
    let heading = label(mtm, "EvoParseq", frame(32.0, 24.0, 300.0, 36.0));
    heading.setFont(Some(&NSFont::boldSystemFontOfSize(26.0)));
    let subtitle = label(
        mtm,
        "Native core  /  Your computers, your connection",
        frame(32.0, 64.0, 520.0, 20.0),
    );
    let settings = button(
        mtm,
        "Settings",
        sel!(settings:),
        frame(460.0, 28.0, 108.0, 30.0),
    );
    let mut login_controls: Vec<Retained<NSView>> = Vec::new();
    let mut fields = Vec::new();
    for (i, text) in ["Email", "Password", "Two-factor code (optional)"]
        .iter()
        .enumerate()
    {
        let top = 112.0 + i as f64 * 44.0;
        let caption = label(mtm, text, frame(32.0, top + 3.0, 200.0, 20.0));
        let rect = frame(240.0, top, 328.0, 24.0);
        let control: Retained<NSTextField> = if i == 1 {
            let secure = NSSecureTextField::initWithFrame(mtm.alloc(), rect);
            Retained::into_super(secure)
        } else {
            field(mtm, if i == 0 { email } else { "" }, rect)
        };
        login_controls.push(Retained::into_super(Retained::into_super(caption)));
        login_controls.push(Retained::into_super(Retained::into_super(control.clone())));
        fields.push(control);
    }
    let remember_box = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str("Remember sign-in for this macOS user"),
            None,
            None,
            mtm,
        )
    };
    remember_box.setFrame(frame(240.0, 244.0, 328.0, 22.0));
    remember_box.setState(isize::from(remember));
    let sign_in = button(
        mtm,
        "Sign in",
        sel!(login:),
        frame(428.0, 280.0, 140.0, 32.0),
    );
    sign_in.setKeyEquivalent(&NSString::from_str("\r"));
    login_controls.push(Retained::into_super(Retained::into_super(
        remember_box.clone(),
    )));
    login_controls.push(Retained::into_super(Retained::into_super(sign_in.clone())));

    let hosts =
        NSPopUpButton::initWithFrame_pullsDown(mtm.alloc(), frame(32.0, 112.0, 536.0, 28.0), false);
    unsafe {
        hosts.setTarget(Some(&actions(mtm)));
        hosts.setAction(Some(sel!(hostSelected:)));
    }
    let peer_caption = label(mtm, "Host peer ID", frame(32.0, 163.0, 150.0, 20.0));
    let peer_field = field(mtm, peer, frame(190.0, 160.0, 378.0, 24.0));
    let refresh = button(
        mtm,
        "Refresh",
        sel!(refresh:),
        frame(32.0, 210.0, 120.0, 32.0),
    );
    let logout = button(
        mtm,
        "Sign out",
        sel!(logout:),
        frame(160.0, 210.0, 120.0, 32.0),
    );
    let connect = button(
        mtm,
        "Connect",
        sel!(connect:),
        frame(428.0, 210.0, 140.0, 32.0),
    );
    let picker_controls: Vec<Retained<NSView>> = vec![
        Retained::into_super(Retained::into_super(Retained::into_super(hosts.clone()))),
        Retained::into_super(Retained::into_super(peer_caption)),
        Retained::into_super(Retained::into_super(peer_field.clone())),
        Retained::into_super(Retained::into_super(refresh.clone())),
        Retained::into_super(Retained::into_super(logout.clone())),
        Retained::into_super(Retained::into_super(connect.clone())),
    ];
    let status_label = NSTextField::wrappingLabelWithString(&NSString::from_str(status), mtm);
    status_label.setFrame(frame(32.0, 350.0, 536.0, 64.0));
    for view in [
        Retained::into_super(Retained::into_super(heading)),
        Retained::into_super(Retained::into_super(subtitle)),
        Retained::into_super(Retained::into_super(settings.clone())),
        Retained::into_super(Retained::into_super(status_label.clone())),
    ]
    .iter()
    .chain(&login_controls)
    .chain(&picker_controls)
    {
        content.addSubview(view);
    }
    window.center();
    window.makeKeyAndOrderFront(None);
    let mut fields = fields.into_iter();
    LAUNCHER.with_borrow_mut(|l| {
        *l = Some(Launcher {
            email: fields.next().unwrap(),
            password: unsafe { Retained::cast_unchecked(fields.next().unwrap()) },
            tfa: fields.next().unwrap(),
            remember: remember_box,
            hosts,
            peer: peer_field,
            status: status_label,
            login_controls,
            picker_controls,
            buttons: vec![settings, sign_in, refresh, logout, connect],
            peers: Vec::new(),
            window,
        })
    });
}

/// Applies view state on the main thread.
fn update(f: impl FnOnce(&mut Launcher) + Send) {
    on_main(|_| {
        LAUNCHER.with_borrow_mut(|l| {
            if let Some(l) = l {
                f(l)
            }
        })
    });
}
fn status(text: &str) {
    let text = text.to_owned();
    update(move |l| l.status.setStringValue(&NSString::from_str(&text)));
}
fn view(sign_in: &SignIn, picker: &Picker) {
    let login = sign_in.session.is_none();
    let busy = sign_in.busy() || picker.blocks_controls();
    update(move |l| {
        for control in &l.login_controls {
            control.setHidden(!login);
        }
        for control in &l.picker_controls {
            control.setHidden(login);
        }
        for button in &l.buttons {
            button.setEnabled(!busy);
        }
        for control in [&l.email, &l.tfa] {
            control.setEnabled(!busy);
        }
        l.password.setEnabled(!busy);
    });
}

pub fn choose(
    manager: Arc<crate::connection_settings::Manager>,
    current: Option<Arc<Session>>,
) -> Result<Option<Selection>> {
    *MANAGER.lock().unwrap_or_else(|e| e.into_inner()) = Some(manager.clone());
    let profile = manager.view().0.native_core;
    let (saved, notice) = if current.is_some() {
        (None, None)
    } else {
        match profile.session() {
            Ok(s) => (s.map(Arc::new), None),
            Err(_) => (
                None,
                Some("The saved sign-in cannot be decrypted for this macOS user. Sign in again."),
            ),
        }
    };
    let mut sign_in = SignIn::new(
        current.or(saved),
        profile.email.clone(),
        !profile.encrypted_session.is_empty(),
    );
    let mut picker = Picker::default();
    let (tx, rx) = mpsc::channel();
    *COMMANDS.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    let first_status = notice
        .unwrap_or("Use your Parsec account. Passwords and two-factor codes are never saved.")
        .to_owned();
    let (email, peer, remember) = (
        profile.email.clone(),
        profile.last_peer.clone(),
        sign_in.remember_login,
    );
    on_main(move |mtm| create_launcher(mtm, &email, &peer, remember, &first_status));
    view(&sign_in, &picker);
    let mut next_refresh = Instant::now() + Duration::from_millis(picker::AUTO_REFRESH_MS.into());
    if let Some(session) = sign_in.session.clone() {
        start_hosts(&mut picker, session, Refresh::Manual, &sign_in);
    }
    let mut result = None;
    while !QUIT.load(Ordering::Acquire) && result.is_none() {
        let open = on_main(|_| {
            LAUNCHER.with_borrow(|l| {
                l.as_ref()
                    .is_some_and(|l| l.window.isVisible() || l.window.isMiniaturized())
            })
        });
        if !open {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(80)) {
            Ok(command) => {
                if sign_in.busy() || picker.blocks_controls() {
                    continue;
                }
                match command {
                    Command::Login(login, remember) => match sign_in.login(login, remember) {
                        Ok(true) => {
                            update(|l| {
                                l.password.setStringValue(&NSString::from_str(""));
                                l.tfa.setStringValue(&NSString::from_str(""));
                            });
                            picker.cancel();
                            status(sign_in.message());
                            view(&sign_in, &picker);
                        }
                        Ok(false) => {}
                        Err(error) => status(&error.to_string()),
                    },
                    Command::Refresh => {
                        if let Some(session) = sign_in.session.clone() {
                            start_hosts(&mut picker, session, Refresh::Manual, &sign_in);
                        }
                    }
                    Command::Connect(text) => match picker::connection_peer(&text) {
                        Ok(peer) => {
                            if let Some(session) = sign_in.session.take() {
                                if manager
                                    .update_native(|p| p.last_peer = peer.clone())
                                    .is_err()
                                {
                                    message("Could not save the selected computer. Your connection can still continue.");
                                }
                                result = Some(Selection { session, peer });
                            }
                        }
                        Err(error) => status(&error.to_string()),
                    },
                    Command::Logout => {
                        if sign_in.prepare_logout(manager.as_ref()).is_err() {
                            status(sign_in.message());
                            continue;
                        }
                        picker.cancel();
                        if sign_in.logout() {
                            status(sign_in.message());
                            view(&sign_in, &picker);
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if let Some(job) = picker.poll() {
            hosts_completed(&mut picker, &mut sign_in, &manager, job);
        }
        if let Some(job) = sign_in.poll() {
            match job {
                Completion::Login(Ok(session)) => {
                    sign_in.accept(session, manager.as_ref(), |notice| message(notice.text()));
                    if let Some(session) = sign_in.session.clone() {
                        start_hosts(&mut picker, session, Refresh::Manual, &sign_in);
                    }
                }
                Completion::Login(Err(_)) => {
                    crate::event_log::record(
                        "ERROR",
                        "native_account_login_failed",
                        serde_json::json!({}),
                    );
                    status(sign_in.message());
                }
                Completion::Logout | Completion::Disconnected => status(sign_in.message()),
            }
            view(&sign_in, &picker);
        }
        if Instant::now() >= next_refresh {
            next_refresh = Instant::now() + Duration::from_millis(picker::AUTO_REFRESH_MS.into());
            if let Some(session) = sign_in.session.clone().filter(|_| !sign_in.busy()) {
                start_hosts(&mut picker, session, Refresh::Background, &sign_in);
            }
        }
    }
    *COMMANDS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    on_main(|_| {
        LAUNCHER.with_borrow_mut(|l| {
            if let Some(l) = l.take() {
                l.window.orderOut(None);
            }
        })
    });
    picker.cancel();
    sign_in.cancel();
    sign_in.close(|| !manager.view().0.native_core.encrypted_session.is_empty());
    Ok(result)
}

fn start_hosts(picker: &mut Picker, session: Arc<Session>, mode: Refresh, sign_in: &SignIn) {
    if picker.start(session, mode) && !mode.quiet() {
        status("Refreshing computers...");
        view(sign_in, picker);
    }
}

fn hosts_completed(
    picker: &mut Picker,
    sign_in: &mut SignIn,
    manager: &crate::connection_settings::Manager,
    job: picker::Completion,
) {
    let quiet = matches!(&job, picker::Completion::Hosts(Refresh::Background, _));
    match job {
        picker::Completion::Hosts(mode, Ok(hosts)) => {
            let selected = on_main(|_| {
                LAUNCHER.with_borrow(|l| {
                    l.as_ref()
                        .and_then(|l| usize::try_from(l.hosts.indexOfSelectedItem()).ok())
                })
            });
            let change = picker.replace_hosts(hosts, mode, || picker::ListPosition {
                selected,
                last_peer: manager.view().0.native_core.last_peer,
                top: 0,
            });
            if let Some(change) = change {
                let entries: Vec<(String, String)> = picker
                    .hosts()
                    .iter()
                    .map(|h| (format!("{}   /   {}", h.name, h.peer), h.peer.clone()))
                    .collect();
                update(move |l| {
                    l.hosts.removeAllItems();
                    for (title, _) in &entries {
                        l.hosts.addItemWithTitle(&NSString::from_str(title));
                    }
                    l.peers = entries.into_iter().map(|(_, peer)| peer).collect();
                    if let Some(selected) = change.selected {
                        l.hosts.selectItemAtIndex(selected as isize);
                    }
                    if let Some(peer) = change.peer {
                        l.peer.setStringValue(&NSString::from_str(&peer));
                    }
                });
            }
            if !quiet {
                status(if picker.hosts().is_empty() {
                    "No computers listed. Refresh, or enter the host peer ID."
                } else {
                    "Choose a computer, or enter its peer ID. Owner connections are supported."
                });
            }
        }
        picker::Completion::Hosts(_, Err(e)) => {
            let expired = e.downcast_ref::<account::ExpiredSession>().is_some();
            crate::event_log::record(
                "WARN",
                "native_computer_list_failed",
                serde_json::json!({"session_rejected":expired}),
            );
            if expired {
                sign_in.expire_saved(manager);
            }
            if !quiet || sign_in.session.is_none() {
                status(&e.to_string());
            }
        }
        picker::Completion::Disconnected => {
            status("The account request ended unexpectedly. Please retry.");
        }
    }
    view(sign_in, picker);
}

/// Connection stats from the shared presentation model, refreshed once per
/// second while the window is open. It never samples beyond existing telemetry.
pub fn open_stats(bus: Arc<crate::stats::Shared>) {
    thread_local! {
        static STATS: RefCell<Option<(Retained<NSWindow>, Retained<NSTextView>)>> = const { RefCell::new(None) };
    }
    let opened = on_main(|mtm| {
        STATS.with_borrow_mut(|stats| {
            if let Some((window, _)) = stats.as_ref().filter(|(w, _)| w.isVisible()) {
                window.makeKeyAndOrderFront(None);
                return false;
            }
            let rect = NSRect::new(NSPoint::ZERO, NSSize::new(560.0, 640.0));
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    mtm.alloc(),
                    rect,
                    NSWindowStyleMask::Titled
                        | NSWindowStyleMask::Closable
                        | NSWindowStyleMask::Resizable,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            unsafe { window.setReleasedWhenClosed(false) };
            window.setTitle(&NSString::from_str("Connection stats"));
            let scroll = NSScrollView::initWithFrame(mtm.alloc(), rect);
            scroll.setHasVerticalScroller(true);
            let text = NSTextView::initWithFrame(mtm.alloc(), rect);
            text.setEditable(false);
            text.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(12.0, 0.0)));
            scroll.setDocumentView(Some(&text));
            window.setContentView(Some(&scroll));
            window.center();
            window.makeKeyAndOrderFront(None);
            *stats = Some((window, text));
            true
        })
    });
    if !opened {
        return;
    }
    bus.visible.store(true, Ordering::Release);
    std::thread::spawn(move || {
        let mut rates = crate::stats::Rates::default();
        let mut sample: Option<crate::stats::Sample> = None;
        let mut values = crate::stats::Values::default();
        loop {
            if let Some(s) = bus.read() {
                if sample.as_ref().is_none_or(|old| old.at != s.at) {
                    values = rates.update(&s);
                    sample = Some(s);
                }
            }
            let text = sample.as_ref().map_or_else(
                || "Waiting for telemetry".to_owned(),
                |sample| {
                    let timing = bus.host_timing.snapshot(sample.generation);
                    let mut text = String::new();
                    for tab in crate::stats_view::Tab::ALL {
                        let p = crate::stats_view::Presentation::new(
                            sample,
                            &values,
                            (None, None, None),
                            None,
                            &timing,
                        )
                        .select(tab);
                        if tab == crate::stats_view::Tab::Overview {
                            text.push_str(&format!("{}\n\n", p.status));
                            for (label, value) in &p.cards {
                                text.push_str(&format!("{label:<28}{value}\n"));
                            }
                        }
                        text.push_str(&format!("\n{}\n", tab.caption().to_uppercase()));
                        for (label, value) in &p.rows {
                            if value.is_empty() {
                                text.push_str(&format!("\n  {label}\n"));
                            } else {
                                text.push_str(&format!("  {label:<26}{value}\n"));
                            }
                        }
                    }
                    text
                },
            );
            let open = on_main(move |_| {
                STATS.with_borrow(|stats| {
                    let Some((window, view)) = stats.as_ref().filter(|(w, _)| w.isVisible()) else {
                        return false;
                    };
                    if !window.isMiniaturized() {
                        view.setString(&NSString::from_str(&text));
                    }
                    true
                })
            });
            if !open {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        bus.visible.store(false, Ordering::Release);
    });
}
