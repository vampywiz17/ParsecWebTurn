//! AppKit Settings window. Values, dependencies, validation and saving come
//! from the shared settings model; this adapter only maps controls.
use crate::connection_settings::Manager;
use crate::settings_model::{BinaryChoice, Form, InputMode, RouteMode, Tab, VideoFormat};
use objc2::{
    define_class, msg_send, rc::Retained, runtime::NSObject, sel, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSPopUpButton, NSScrollView, NSSecureTextField, NSTabView,
    NSTabViewItem, NSTextField, NSTextView, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::{cell::RefCell, sync::Arc};

const WIDTH: f64 = 640.0;
const PAGE_HEIGHT: f64 = 640.0;
const LABEL: f64 = 190.0;

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "EvoParseqSettingsActions"]
    struct Actions;

    impl Actions {
        #[unsafe(method(changed:))]
        fn changed(&self, _sender: &NSObject) {
            with_page(|page| page.refresh());
        }
        #[unsafe(method(save:))]
        fn save(&self, _sender: &NSObject) {
            with_page(|page| {
                page.read();
                let saved = page.form.save(&page.manager);
                if saved {
                    // Saved secrets never return to the controls.
                    page.password.setStringValue(&NSString::from_str(""));
                    page.token.setStringValue(&NSString::from_str(""));
                    page.forget_password.setState(0);
                    page.forget_token.setState(0);
                }
                page.feedback
                    .setStringValue(&NSString::from_str(&page.form.feedback));
                page.refresh();
            });
        }
        #[unsafe(method(close:))]
        fn close(&self, _sender: &NSObject) {
            with_page(|page| page.window.orderOut(None));
        }
    }
);

struct Page {
    window: Retained<NSWindow>,
    _actions: Retained<Actions>,
    manager: Arc<Manager>,
    form: Form,
    #[cfg(feature = "bud")]
    transport: Retained<NSPopUpButton>,
    route: Retained<NSPopUpButton>,
    provider: Retained<NSPopUpButton>,
    stun: Retained<NSTextView>,
    turn: Retained<NSTextView>,
    username: Retained<NSTextField>,
    password: Retained<NSSecureTextField>,
    forget_password: Retained<NSButton>,
    key_id: Retained<NSTextField>,
    token: Retained<NSSecureTextField>,
    forget_token: Retained<NSButton>,
    ttl: Retained<NSTextField>,
    cache: Retained<NSButton>,
    event_log: Retained<NSButton>,
    video: Retained<NSPopUpButton>,
    audio_buffer: Retained<NSPopUpButton>,
    audio_min: Retained<NSTextField>,
    audio_max: Retained<NSTextField>,
    #[cfg(feature = "bud")]
    audio_codec: Retained<NSPopUpButton>,
    #[cfg(feature = "bud")]
    speakers: Retained<NSPopUpButton>,
    #[cfg(feature = "bud")]
    input: Retained<NSPopUpButton>,
    feedback: Retained<NSTextField>,
}
thread_local! {
    static PAGE: RefCell<Option<Page>> = const { RefCell::new(None) };
}
fn with_page(f: impl FnOnce(&mut Page)) {
    PAGE.with_borrow_mut(|page| {
        if let Some(page) = page {
            f(page)
        }
    });
}

fn string(text: &str) -> Retained<NSString> {
    NSString::from_str(text)
}
fn selected(popup: &NSPopUpButton) -> usize {
    usize::try_from(popup.indexOfSelectedItem()).unwrap_or(0)
}
fn checked(button: &NSButton) -> bool {
    button.state() != 0
}
fn set(button: &NSButton, on: bool) {
    button.setState(isize::from(on));
}

impl Page {
    /// Controls -> shared model, in the model's own vocabulary.
    fn read(&mut self) {
        let f = &mut self.form;
        #[cfg(feature = "bud")]
        {
            f.values.bud = selected(&self.transport) == 1;
        }
        RouteMode::ALL[selected(&self.route).min(2)].apply(&mut f.values);
        f.values.provider = if selected(&self.provider) == 1 {
            "cloudflare"
        } else {
            "custom"
        }
        .into();
        f.stun_urls = self.stun.string().to_string();
        f.turn_urls = self.turn.string().to_string();
        f.values.custom_username = self.username.stringValue().to_string();
        f.password = zeroize::Zeroizing::new(self.password.stringValue().to_string());
        f.forget_password = checked(&self.forget_password);
        f.values.turn_key_id = self.key_id.stringValue().to_string();
        f.token = zeroize::Zeroizing::new(self.token.stringValue().to_string());
        f.forget_token = checked(&self.forget_token);
        f.ttl = self.ttl.stringValue().to_string();
        f.values.cache_credentials = checked(&self.cache);
        f.values.event_logging = checked(&self.event_log);
        VIDEO_FORMATS[selected(&self.video).min(VIDEO_FORMATS.len() - 1)].apply(&mut f.values);
        f.values.custom_audio_buffer = selected(&self.audio_buffer) == 1;
        f.audio_min = self.audio_min.stringValue().to_string();
        f.audio_max = self.audio_max.stringValue().to_string();
        #[cfg(feature = "bud")]
        {
            f.values.allow_raw_audio = selected(&self.audio_codec) == 1;
            f.values.experimental_surround_audio = selected(&self.speakers) == 1;
            InputMode::ALL[selected(&self.input).min(3)].apply(&mut f.values);
        }
    }
    /// Shared model -> controls, including dependent enablement.
    fn write(&mut self) {
        let s = self.form.values.clone();
        #[cfg(feature = "bud")]
        self.transport.selectItemAtIndex(isize::from(s.bud));
        self.route.selectItemAtIndex(
            RouteMode::ALL
                .iter()
                .position(|m| *m == RouteMode::current(&s))
                .unwrap_or(0) as isize,
        );
        self.provider
            .selectItemAtIndex(isize::from(self.form.cloudflare()));
        self.stun
            .setString(&string(&self.form.stun_urls.replace("\r\n", "\n")));
        self.turn
            .setString(&string(&self.form.turn_urls.replace("\r\n", "\n")));
        self.username.setStringValue(&string(&s.custom_username));
        self.key_id.setStringValue(&string(&s.turn_key_id));
        self.ttl.setStringValue(&string(&self.form.ttl));
        set(&self.cache, s.cache_credentials);
        set(&self.event_log, s.event_logging);
        let format = VideoFormat::current(&s);
        self.video.selectItemAtIndex(
            VIDEO_FORMATS.iter().position(|f| *f == format).unwrap_or(1) as isize
        );
        self.audio_buffer
            .selectItemAtIndex(isize::from(s.custom_audio_buffer));
        self.audio_min.setStringValue(&string(&self.form.audio_min));
        self.audio_max.setStringValue(&string(&self.form.audio_max));
        #[cfg(feature = "bud")]
        {
            self.audio_codec
                .selectItemAtIndex(isize::from(s.allow_raw_audio));
            self.speakers
                .selectItemAtIndex(isize::from(s.experimental_surround_audio));
            self.input.selectItemAtIndex(
                InputMode::ALL
                    .iter()
                    .position(|m| *m == InputMode::current(&s))
                    .unwrap_or(0) as isize,
            );
        }
        self.refresh_enabled();
    }
    fn refresh(&mut self) {
        self.read();
        self.refresh_enabled();
    }
    fn refresh_enabled(&mut self) {
        let a = self.form.availability();
        for control in [&self.username, &self.ttl] {
            control.setEnabled(a.relay);
        }
        self.provider.setEnabled(a.relay);
        self.turn.setEditable(a.custom_turn);
        self.username.setEnabled(a.custom_turn);
        self.password.setEnabled(a.custom_turn);
        self.forget_password.setEnabled(a.custom_turn);
        self.key_id.setEnabled(a.cloudflare_turn);
        self.token.setEnabled(a.cloudflare_turn);
        self.forget_token.setEnabled(a.cloudflare_turn);
        self.audio_min.setEnabled(a.audio_limits);
        self.audio_max.setEnabled(a.audio_limits);
        #[cfg(feature = "bud")]
        {
            self.audio_codec.setEnabled(a.bud);
            self.speakers.setEnabled(a.bud);
            self.input.setEnabled(a.bud);
        }
    }
}

/// HEVC Main 10 is not offered by the macOS decoder path.
const VIDEO_FORMATS: [VideoFormat; 2] = [VideoFormat::H264, VideoFormat::Hevc8];

struct Builder<'a> {
    mtm: MainThreadMarker,
    actions: &'a Actions,
    view: Retained<NSView>,
    top: f64,
}
impl Builder<'_> {
    fn new<'a>(mtm: MainThreadMarker, actions: &'a Actions, notice: &str) -> Builder<'a> {
        let view = NSView::initWithFrame(
            mtm.alloc(),
            NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH - 40.0, PAGE_HEIGHT)),
        );
        let mut builder = Builder {
            mtm,
            actions,
            view,
            top: 12.0,
        };
        let note = NSTextField::wrappingLabelWithString(&string(notice), mtm);
        builder.place(&note, 16.0, WIDTH - 72.0, 48.0);
        builder.top += 8.0;
        builder
    }
    fn place(&mut self, control: &NSView, x: f64, width: f64, height: f64) {
        control.setFrame(NSRect::new(
            NSPoint::new(x, PAGE_HEIGHT - self.top - height),
            NSSize::new(width, height),
        ));
        self.view.addSubview(control);
        self.top += height + 10.0;
    }
    fn caption(&mut self, text: &str, height: f64) {
        let label = NSTextField::labelWithString(&string(text), self.mtm);
        label.setFrame(NSRect::new(
            NSPoint::new(
                16.0,
                PAGE_HEIGHT - self.top - height + (height - 18.0).min(4.0),
            ),
            NSSize::new(LABEL - 24.0, 18.0),
        ));
        self.view.addSubview(&label);
    }
    /// A captioned explanation row without a control.
    fn hint(&mut self, caption: &str, text: &str) {
        self.caption(caption, 52.0);
        let note = NSTextField::wrappingLabelWithString(&string(text), self.mtm);
        self.place(&note, LABEL, WIDTH - LABEL - 56.0, 52.0);
    }
    fn popup(&mut self, text: &str, labels: &[&str]) -> Retained<NSPopUpButton> {
        self.caption(text, 26.0);
        let popup = NSPopUpButton::initWithFrame_pullsDown(self.mtm.alloc(), NSRect::ZERO, false);
        for label in labels {
            popup.addItemWithTitle(&string(label));
        }
        unsafe {
            popup.setTarget(Some(self.actions));
            popup.setAction(Some(sel!(changed:)));
        }
        self.place(&popup, LABEL, WIDTH - LABEL - 56.0, 26.0);
        popup
    }
    fn field(&mut self, text: &str) -> Retained<NSTextField> {
        self.caption(text, 24.0);
        let field = NSTextField::textFieldWithString(&string(""), self.mtm);
        self.place(&field, LABEL, WIDTH - LABEL - 56.0, 24.0);
        field
    }
    fn secret(&mut self, text: &str) -> Retained<NSSecureTextField> {
        self.caption(text, 24.0);
        let field = NSSecureTextField::initWithFrame(self.mtm.alloc(), NSRect::ZERO);
        field.setPlaceholderString(Some(&string("Leave blank to keep the saved value")));
        self.place(&field, LABEL, WIDTH - LABEL - 56.0, 24.0);
        field
    }
    fn check(&mut self, text: &str) -> Retained<NSButton> {
        let button = unsafe {
            NSButton::checkboxWithTitle_target_action(
                &string(text),
                Some(self.actions),
                Some(sel!(changed:)),
                self.mtm,
            )
        };
        self.place(&button, LABEL, WIDTH - LABEL - 56.0, 20.0);
        button
    }
    fn area(&mut self, text: &str) -> Retained<NSTextView> {
        self.caption(text, 54.0);
        let rect = NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH - LABEL - 56.0, 54.0));
        let scroll = NSScrollView::initWithFrame(self.mtm.alloc(), rect);
        scroll.setHasVerticalScroller(true);
        let area = NSTextView::initWithFrame(self.mtm.alloc(), rect);
        area.setRichText(false);
        area.setAutomaticQuoteSubstitutionEnabled(false);
        scroll.setDocumentView(Some(&area));
        self.place(&scroll, LABEL, WIDTH - LABEL - 56.0, 54.0);
        area
    }
    fn item(self, title: &str) -> Retained<NSTabViewItem> {
        let item = NSTabViewItem::new();
        item.setLabel(&string(title));
        item.setView(Some(&self.view));
        item
    }
}

pub fn open(manager: Arc<Manager>) {
    crate::window_macos::on_main(move |mtm| {
        let existing = PAGE.with_borrow(|page| {
            page.as_ref()
                .map(|page| page.window.makeKeyAndOrderFront(None))
                .is_some()
        });
        if existing {
            return;
        }
        build(mtm, manager);
    });
}

fn build(mtm: MainThreadMarker, manager: Arc<Manager>) {
    let actions: Retained<Actions> = unsafe { msg_send![Actions::alloc(mtm), init] };
    let saved = manager.view().0;
    let height = PAGE_HEIGHT + 130.0;
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, height)),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&string("Settings"));
    let content = window.contentView().expect("window content view");

    let mut b = Builder::new(mtm, &actions, Tab::Network.notice());
    #[cfg(feature = "bud")]
    let transport = b.popup("Transport", &["WebRTC", "BUD (experimental)"]);
    let route = b.popup("Connection route", &RouteMode::LABELS);
    let provider = b.popup("TURN provider", &["Custom servers", "Cloudflare TURN"]);
    let stun = b.area("STUN servers");
    let turn = b.area("TURN servers");
    let username = b.field("Username");
    let password = b.secret("Password");
    let forget_password = b.check("Forget saved password");
    let key_id = b.field("TURN Key ID");
    let token = b.secret("API token");
    let forget_token = b.check("Forget saved API token");
    let ttl = b.field("Credential lifetime (s)");
    let cache = b.check("Reuse unexpired credentials");
    let event_log = b.check("Enable event logging");
    let network = b.item("Network");

    let mut b = Builder::new(mtm, &actions, Tab::Video.notice());
    let video = b.popup(
        "Video format",
        &VIDEO_FORMATS
            .map(|f| VideoFormat::LABELS[VideoFormat::ALL.iter().position(|a| *a == f).unwrap()]),
    );
    let note = NSTextField::wrappingLabelWithString(
        &string("VideoToolbox decodes and Core Animation presents video. HEVC needs a compatible host encoder. Rendering, GPU, chroma and AI upscaling options are Windows-only."),
        mtm,
    );
    b.place(&note, LABEL, WIDTH - LABEL - 56.0, 64.0);
    let video_tab = b.item("Video");

    let mut b = Builder::new(mtm, &actions, Tab::Audio.notice());
    let audio_buffer = b.popup("Playback buffer", &BinaryChoice::AudioBuffer.labels());
    let audio_min = b.field("Minimum buffer (ms)");
    let audio_max = b.field("Maximum buffer (ms)");
    #[cfg(feature = "bud")]
    let audio_codec = b.popup("Audio format", &BinaryChoice::AudioCodec.labels());
    #[cfg(feature = "bud")]
    let speakers = b.popup("Speaker layout", &BinaryChoice::Speakers.labels());
    let audio = b.item("Audio");

    let mut b = Builder::new(mtm, &actions, Tab::Input.notice());
    #[cfg(feature = "bud")]
    let input = b.popup("Input mode", &InputMode::LABELS);
    #[cfg(feature = "bud")]
    for (caption, text) in [
        ("", crate::settings_model::INPUT_MODE_HINT),
        (
            "HID requirements",
            crate::settings_model::HID_REQUIREMENTS_HINT,
        ),
        (
            "Compatibility",
            crate::settings_model::HID_COMPATIBILITY_HINT,
        ),
    ] {
        b.hint(caption, text);
    }
    let input_tab = b.item("Input");

    let tabs = NSTabView::initWithFrame(
        mtm.alloc(),
        NSRect::new(
            NSPoint::new(12.0, 76.0),
            NSSize::new(WIDTH - 24.0, PAGE_HEIGHT + 44.0),
        ),
    );
    for item in [network, video_tab, audio, input_tab] {
        tabs.addTabViewItem(&item);
    }
    content.addSubview(&tabs);
    let feedback = NSTextField::wrappingLabelWithString(
        &string("Saved in settings.json. Secrets are protected by your macOS Keychain."),
        mtm,
    );
    feedback.setFrame(NSRect::new(
        NSPoint::new(24.0, 14.0),
        NSSize::new(WIDTH - 300.0, 52.0),
    ));
    content.addSubview(&feedback);
    for (title, action, x) in [
        ("Close", sel!(close:), WIDTH - 260.0),
        ("Save settings", sel!(save:), WIDTH - 140.0),
    ] {
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &string(title),
                Some(&actions),
                Some(action),
                mtm,
            )
        };
        button.setFrame(NSRect::new(NSPoint::new(x, 24.0), NSSize::new(116.0, 32.0)));
        content.addSubview(&button);
    }
    let mut page = Page {
        window,
        _actions: actions,
        manager,
        form: Form::new(saved, None),
        #[cfg(feature = "bud")]
        transport,
        route,
        provider,
        stun,
        turn,
        username,
        password,
        forget_password,
        key_id,
        token,
        forget_token,
        ttl,
        cache,
        event_log,
        video,
        audio_buffer,
        audio_min,
        audio_max,
        #[cfg(feature = "bud")]
        audio_codec,
        #[cfg(feature = "bud")]
        speakers,
        #[cfg(feature = "bud")]
        input,
        feedback,
    };
    page.write();
    page.window.center();
    page.window.makeKeyAndOrderFront(None);
    PAGE.with_borrow_mut(|slot| *slot = Some(page));
}
