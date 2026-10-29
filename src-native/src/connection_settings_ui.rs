//! Native, modeless settings page inside the existing client window.
//! All HWND/GDI ownership and control access stay on the window's UI thread.
#[path = "settings_controls.rs"]
pub(crate) mod controls;
use crate::connection_settings::Manager;
use crate::settings_model::{
    AiUpscaler, BinaryChoice, Form, InputMode, RouteMode, Tab, VideoFormat,
};
use std::{
    ptr::{null, null_mut},
    sync::Arc,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::*,
    UI::{
        Controls::{
            SetScrollInfo, DRAWITEMSTRUCT, EM_SETLIMITTEXT, MEASUREITEMSTRUCT, ODS_DISABLED,
            ODS_FOCUS, ODS_SELECTED,
        },
        Input::KeyboardAndMouse::*,
        WindowsAndMessaging::*,
    },
};
pub const OPEN: usize = 4100;
pub const ABOUT: usize = crate::updater_ui::ABOUT;
// Give the large heading and subtitle enough space, including font overhang.
const HEADER_EXTRA: i32 = 20;
const SAVE: usize = 4101;
const BACK: usize = 4102;

const HIDE_SESSION_BUTTON: usize = 4135;
const PROVIDER: usize = 4103;
const ROUTE_MODE: usize = 4104;
const STUN: usize = 4105;
const TURN: usize = 4106;
const USERNAME: usize = 4107;
const PASSWORD: usize = 4108;
const KEY: usize = 4109;
const TOKEN: usize = 4110;
const TTL: usize = 4111;
const CACHE: usize = 4112;
const FORGET_PASSWORD: usize = 4113;
const FORGET_TOKEN: usize = 4114;
const STATUS: usize = 4115;
const GPU: usize = 4116;
const VIDEO_FORMAT: usize = 4117;
#[cfg(feature = "bud")]
const BUD: usize = 4119;
const BUD_EXTRA: i32 = if cfg!(feature = "bud") { 94 } else { 0 };
#[cfg(feature = "vendor-video")]
const ZERO_COPY: usize = 4120;
const BG: u32 = crate::branding::BACKGROUND;
const FIELD: u32 = 0x00212121;
const FG: u32 = 0x00eeeeee;
const MUTED: u32 = 0x00bcbcbc;
const ACCENT: u32 = 0x00ffbb00;
const CUSTOM: u8 = 1;
const CLOUDFLARE: u8 = 2;
const HELP: usize = 4200;
const NETWORK_TAB: usize = 4201;
const VIDEO_TAB: usize = 4121;

const CHROMA_444: usize = 4136;
const EVENT_LOG: usize = 4123;
const TCP_QOS: usize = 4138;
const AUDIO_TAB: usize = 4124;
const INPUT_TAB: usize = 4129;
const INPUT_MODE: usize = 4130;
const AUDIO_CUSTOM: usize = 4125;
const AUDIO_MIN: usize = 4126;
const AUDIO_MAX: usize = 4127;
const AUDIO_RAW: usize = 4128;
const AUDIO_SURROUND: usize = 4132;
const PRESENTATION: usize = 4133;
const AI_UPSCALE: usize = 4139;
const SECTION: usize = 4202;
const NOTICE: usize = 4203;
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
struct Item {
    hwnd: HWND,
    rect: RECT,
    group: u8,
    tab: Option<Tab>,
}
struct Page {
    window_owned: bool,
    manager: Arc<Manager>,
    model: Form,
    gpu_labels: Vec<String>,
    ai_upscaler: Option<AiUpscaler>,
    ai_hint: HWND,
    background: HBRUSH,
    field: HBRUSH,
    font: HFONT,
    heading: HFONT,
    small: HFONT,
    items: Vec<Item>,
    building_tab: Option<Tab>,
    scroll: i32,
    reflowing: bool,
}
impl Drop for Page {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.background);
            DeleteObject(self.field);
            DeleteObject(self.font);
            DeleteObject(self.heading);
            DeleteObject(self.small);
        }
    }
}
unsafe fn font(size: i32, weight: i32) -> HFONT {
    CreateFontW(
        -size,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        0,
        0,
        CLEARTYPE_QUALITY as u32,
        0,
        wide("Segoe UI").as_ptr(),
    )
}
/// A native menu command and keyboard shortcut; no private Parsec UI callbacks.
pub unsafe fn install_menu(parent: HWND) {
    if !GetMenu(parent).is_null() {
        return;
    }
    let menu = CreateMenu();
    AppendMenuW(menu, MF_STRING, OPEN, wide("Settings").as_ptr());
    AppendMenuW(
        menu,
        MF_STRING,
        crate::stats_ui::OPEN,
        wide("Connection stats").as_ptr(),
    );
    crate::updater_ui::append_help(menu);
    SetMenu(parent, menu);
    DrawMenuBar(parent);
}
pub unsafe fn open(parent: HWND, manager: Arc<Manager>) -> HWND {
    let name = wide("EvoParseqSettings");
    let instance = GetModuleHandleW(null());
    let class = WNDCLASSW {
        lpfnWndProc: Some(page_proc),
        hInstance: instance,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: name.as_ptr(),
        ..std::mem::zeroed()
    };
    RegisterClassW(&class);
    let (saved, error) = manager.view();
    let mut page = Box::new(Page {
        window_owned: false,
        manager,
        model: Form::new(saved, crate::video_windows::video_gpu_vendor(None)),
        gpu_labels: vec!["Automatic (Windows default)".into()],
        ai_upscaler: None,
        ai_hint: null_mut(),
        background: CreateSolidBrush(BG),
        field: CreateSolidBrush(FIELD),
        font: font(16, 400),
        heading: font(46, 300),
        small: font(13, 400),
        items: Vec::new(),
        building_tab: None,
        scroll: 0,
        reflowing: false,
    });
    // Failed CreateWindowEx may send WM_NCDESTROY. Transfer ownership only on success.
    let pointer = (&mut *page) as *mut Page;
    let mut rect: RECT = std::mem::zeroed();
    GetClientRect(parent, &mut rect);
    if rect.right < 860 || rect.bottom < 600 {
        SetWindowPos(
            parent,
            null_mut(),
            0,
            0,
            1000,
            780,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        GetClientRect(parent, &mut rect);
    }
    let hwnd = CreateWindowExW(
        // Buffer the GDI page and its native child controls together. This
        // class has no CS_OWNDC/CS_CLASSDC/CS_PARENTDC; the GL parent is unchanged.
        WS_EX_CONTROLPARENT | WS_EX_COMPOSITED,
        name.as_ptr(),
        wide("Settings").as_ptr(),
        // Build the entire page offscreen before making any control visible.
        WS_CHILD | WS_CLIPCHILDREN | WS_VSCROLL,
        0,
        0,
        rect.right,
        rect.bottom,
        parent,
        null_mut(),
        instance,
        pointer.cast(),
    );
    if hwnd.is_null() {
        return hwnd;
    }
    page.window_owned = true;
    let page = &mut *Box::into_raw(page);
    let s = page.model.values.clone();
    add(
        hwnd,
        page,
        "STATIC",
        "Settings",
        0,
        [0, 24, 820, 64],
        0,
        0,
        page.heading,
    );
    add(
        hwnd,
        page,
        "STATIC",
        "Customize your connection, video and audio playback.",
        0,
        [4, 88, 816, 24],
        0,
        0,
        page.font,
    );
    for (id, label, rect) in [
        (NETWORK_TAB, "Network", [0, 142, 120, 40]),
        (VIDEO_TAB, "Video", [132, 142, 120, 40]),
        (AUDIO_TAB, "Audio", [264, 142, 120, 40]),
        (INPUT_TAB, "Input", [396, 142, 120, 40]),
        (BACK, "Back", [550, 142, 110, 40]),
        (SAVE, "Save settings", [676, 142, 144, 40]),
    ] {
        add(
            hwnd,
            page,
            "BUTTON",
            label,
            BS_OWNERDRAW as u32 | WS_TABSTOP,
            rect,
            id,
            0,
            page.font,
        );
    }
    add(
        hwnd,
        page,
        "STATIC",
        "NETWORK",
        0,
        [0, 204, 820, 25],
        SECTION,
        0,
        page.font,
    );
    page.building_tab = Some(Tab::Network);
    row(hwnd, page, "STUN servers",
        "Discover a direct route. One stun: URL per line.\nLeave blank to use the default discovery server.",
        STUN, &s.stun_urls.join("\r\n"), 242, 76, 0, ES_MULTILINE as u32 | ES_WANTRETURN as u32 | ES_AUTOVSCROLL as u32 | WS_CLIPCHILDREN);
    row_label(hwnd, page, "Connection route", "Choose automatic routing, direct-only or TURN relay.\nDirect-only is intended for LAN or VPN connections.", 344, 0, 40);
    choice(
        hwnd,
        page,
        ROUTE_MODE,
        344,
        &RouteMode::LABELS,
        RouteMode::current(&s) as usize,
    );
    row_label(
        hwnd,
        page,
        "TURN provider",
        "An optional relay when a direct connection is unavailable.",
        418,
        0,
        40,
    );
    let provider = add(
        hwnd,
        page,
        "COMBOBOX",
        "",
        CBS_DROPDOWNLIST as u32
            | CBS_OWNERDRAWFIXED as u32
            | CBS_HASSTRINGS as u32
            | WS_VSCROLL
            | WS_TABSTOP,
        [420, 418, 400, 160],
        PROVIDER,
        0,
        page.font,
    );
    controls::combo(provider);
    for name in ["Custom servers", "Cloudflare TURN"] {
        SendMessageW(provider, CB_ADDSTRING, 0, wide(name).as_ptr() as isize);
    }
    SendMessageW(
        provider,
        CB_SETCURSEL,
        usize::from(s.provider == "cloudflare"),
        0,
    );
    row(hwnd, page, "TURN servers",
        "coturn, eturnal, ExpressTURN and other providers.\nUDP, TCP or TLS; e.g. turns:host:443. One URL per line.",
        TURN, &s.turn_urls.join("\r\n"), 502, 76, CUSTOM,
        ES_MULTILINE as u32 | ES_WANTRETURN as u32 | ES_AUTOVSCROLL as u32 | WS_CLIPCHILDREN);
    row(
        hwnd,
        page,
        "Username",
        "The username supplied by your TURN provider.",
        USERNAME,
        &s.custom_username,
        608,
        36,
        CUSTOM,
        ES_AUTOHSCROLL as u32,
    );
    row(
        hwnd,
        page,
        "Password",
        "Leave blank to keep your saved password.",
        PASSWORD,
        "",
        688,
        36,
        CUSTOM,
        ES_AUTOHSCROLL as u32 | ES_PASSWORD as u32,
    );
    add(
        hwnd,
        page,
        "BUTTON",
        "Forget saved password",
        BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
        [420, 734, 400, 28],
        FORGET_PASSWORD,
        CUSTOM,
        page.small,
    );
    row(
        hwnd,
        page,
        "TURN Key ID",
        "The key ID from your Cloudflare TURN configuration.",
        KEY,
        &s.turn_key_id,
        502,
        36,
        CLOUDFLARE,
        ES_AUTOHSCROLL as u32,
    );
    row(
        hwnd,
        page,
        "API token",
        "Leave blank to keep your saved token.",
        TOKEN,
        "",
        582,
        36,
        CLOUDFLARE,
        ES_AUTOHSCROLL as u32 | ES_PASSWORD as u32,
    );
    add(
        hwnd,
        page,
        "BUTTON",
        "Forget saved API token",
        BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
        [420, 628, 400, 28],
        FORGET_TOKEN,
        CLOUDFLARE,
        page.small,
    );
    row(
        hwnd,
        page,
        "Credential lifetime",
        "How long generated TURN credentials remain valid (seconds).",
        TTL,
        &s.ttl.to_string(),
        678,
        36,
        CLOUDFLARE,
        ES_NUMBER as u32,
    );
    row_label(
        hwnd,
        page,
        "Credential cache",
        "Reuse valid credentials until you close EvoParseq.",
        758,
        CLOUDFLARE,
        24,
    );
    add(
        hwnd,
        page,
        "BUTTON",
        "Reuse unexpired credentials",
        BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
        [420, 758, 400, 36],
        CACHE,
        CLOUDFLARE,
        page.font,
    );
    check(hwnd, CACHE, s.cache_credentials);
    page.building_tab = Some(Tab::Video);
    row_label(
        hwnd,
        page,
        "Video GPU",
        "GPU for stream decoding and rendering.\nAutomatic uses Windows' default. Next connection.",
        242,
        0,
        40,
    );
    let gpu = add(
        hwnd,
        page,
        "COMBOBOX",
        "",
        CBS_DROPDOWNLIST as u32
            | CBS_OWNERDRAWFIXED as u32
            | CBS_HASSTRINGS as u32
            | WS_VSCROLL
            | WS_TABSTOP,
        [420, 242, 400, 200],
        GPU,
        0,
        page.font,
    );
    controls::combo(gpu);
    SendMessageW(
        gpu,
        CB_ADDSTRING,
        0,
        wide("Automatic (Windows default)").as_ptr() as isize,
    );
    if let Ok(adapters) = crate::gpu_windows::enumerate() {
        for (index, adapter) in adapters.iter().enumerate() {
            let p = adapter.preference.clone();
            let label = if adapters
                .iter()
                .filter(|a| a.preference.name == p.name)
                .count()
                > 1
            {
                format!("{} (GPU {})", p.name, index + 1)
            } else {
                p.name.clone()
            };
            SendMessageW(gpu, CB_ADDSTRING, 0, wide(&label).as_ptr() as isize);
            page.model.gpu_choices.push(Some(p));
            page.gpu_labels.push(label);
        }
    }
    let selected = s
        .video_gpu
        .as_ref()
        .map(|preferred| {
            let available: Vec<_> = page
                .model
                .gpu_choices
                .iter()
                .skip(1)
                .flatten()
                .cloned()
                .collect();
            match crate::gpu_windows::match_index(preferred, &available) {
                Some(index) => index + 1,
                None => {
                    SendMessageW(
                        gpu,
                        CB_ADDSTRING,
                        0,
                        wide(&format!("Unavailable: {}", preferred.name)).as_ptr() as isize,
                    );
                    page.model.gpu_choices.push(Some(preferred.clone()));
                    page.gpu_labels
                        .push(format!("Unavailable: {}", preferred.name));
                    page.model.gpu_choices.len() - 1
                }
            }
        })
        .unwrap_or(0);
    SendMessageW(gpu, CB_SETCURSEL, selected, 0);
    row_label(hwnd, page, "Video format", "HEVC and 10-bit require compatible host encoding.\nThe host may select H.264; reconnect after changing.", 344, 0, 48);
    choice(
        hwnd,
        page,
        VIDEO_FORMAT,
        344,
        &VideoFormat::LABELS,
        VideoFormat::current(&s) as usize,
    );
    row_label(
        hwnd,
        page,
        "Display timing",
        crate::settings_model::DISPLAY_TIMING_HINT,
        626,
        0,
        60,
    );
    let timing = add(
        hwnd,
        page,
        "COMBOBOX",
        "",
        CBS_DROPDOWNLIST as u32 | WS_VSCROLL | WS_TABSTOP,
        [420, 626, 400, 180],
        PRESENTATION,
        0,
        page.font,
    );
    for mode in crate::connection_settings::PresentationMode::ALL {
        SendMessageW(
            timing,
            CB_ADDSTRING,
            0,
            wide(mode.label()).as_ptr() as isize,
        );
    }
    let selection = crate::connection_settings::PresentationMode::ALL
        .iter()
        .position(|mode| *mode == s.presentation_mode)
        .unwrap_or(0);
    SendMessageW(timing, CB_SETCURSEL, selection, 0);

    {
        row_label(hwnd, page, "Chroma", "NVIDIA tested; Intel not live-validated. AMD unavailable.\nHEVC 8/10-bit; requires compatible host and GPU.\nNormal GPU rendering; host may send 4:2:0 instead.", 438, 0, 60);
        binary_choice(hwnd, page, CHROMA_444, 438);
        check(hwnd, CHROMA_444, s.experimental_chroma_444);
        row_label(
            hwnd,
            page,
            "Session menu",
            "Ctrl+Shift+M opens the menu even when its button is hidden.\nApplies after saving.",
            834,
            0,
            48,
        );
        add(
            hwnd,
            page,
            "BUTTON",
            "Hide session button",
            BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
            [420, 834, 400, 36],
            HIDE_SESSION_BUTTON,
            0,
            page.font,
        );
        check(hwnd, HIDE_SESSION_BUTTON, s.native_core.hide_session_button);
    }
    page.ai_hint = row_label(hwnd, page, "AI upscaling (experimental)", "", 924, 0, 64);
    let ai = add(
        hwnd,
        page,
        "COMBOBOX",
        "",
        CBS_DROPDOWNLIST as u32 | WS_VSCROLL | WS_TABSTOP,
        [420, 924, 400, 180],
        AI_UPSCALE,
        0,
        page.font,
    );
    controls::combo(ai);
    page.building_tab = Some(Tab::Network);
    #[cfg(feature = "bud")]
    {
        row_label(hwnd, page, "Transport", "WebRTC or BUD: direct UDP or TURN over UDP/TLS/TCP.\nUses the TURN URLs above. Reconnect after changing.", 844, 0, 40);
        let transport = add(
            hwnd,
            page,
            "COMBOBOX",
            "",
            CBS_DROPDOWNLIST as u32 | WS_VSCROLL | WS_TABSTOP,
            [420, 844, 400, 160],
            BUD,
            0,
            page.font,
        );
        for label in ["WebRTC", "BUD"] {
            SendMessageW(transport, CB_ADDSTRING, 0, wide(label).as_ptr() as isize);
        }
        controls::combo(transport);
        check(hwnd, BUD, s.bud);
    }
    #[cfg(feature = "vendor-video")]
    {
        page.building_tab = Some(Tab::Video);
        let y = 532;
        let hint = "HEVC 4:2:0: NV12 (8-bit) or P010 (10-bit).\nRequires driver support; otherwise MF. Reconnect to apply.";
        row_label(hwnd, page, "Video presentation", hint, y, 0, 40);
        binary_choice(hwnd, page, ZERO_COPY, 532);
        check(hwnd, ZERO_COPY, s.experimental_zero_copy);
    }
    page.building_tab = Some(Tab::Network);
    row_label(
        hwnd,
        page,
        crate::settings_model::TCP_QOS_LABEL,
        crate::settings_model::TCP_QOS_HINT,
        844 + BUD_EXTRA,
        0,
        60,
    );
    binary_choice(hwnd, page, TCP_QOS, 844 + BUD_EXTRA);
    check(hwnd, TCP_QOS, s.experimental_tcp_qos);
    row_label(hwnd, page, "Event log", "Log connection changes and decoder errors.\nSaved beside settings.json; no credentials or media payloads.\nApplies immediately after saving.", 938 + BUD_EXTRA, 0, 60);
    add(
        hwnd,
        page,
        "BUTTON",
        "Enable event logging",
        BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
        [420, 938 + BUD_EXTRA, 400, 36],
        EVENT_LOG,
        0,
        page.font,
    );
    check(hwnd, EVENT_LOG, s.event_logging);
    page.building_tab = Some(Tab::Audio);
    row_label(hwnd, page, "Playback buffer", "Custom limits apply only when enabled.\nOff uses the core's limits (normally 75 / 150 ms).", 242, 0, 48);
    binary_choice(hwnd, page, AUDIO_CUSTOM, 242);
    check(hwnd, AUDIO_CUSTOM, s.custom_audio_buffer);
    row(hwnd, page, "Minimum buffer (ms)", "PCM reserve before starting or refilling playback.\nSmaller values can increase audible gaps.", AUDIO_MIN,
        &s.audio_min_buffer_ms.to_string(), 344, 36, 0, ES_NUMBER as u32);
    row(hwnd, page, "Maximum buffer (ms)", "Local PCM plus audio queued on the Windows device.\n0 <= minimum <= maximum <= 500; maximum above zero.", AUDIO_MAX,
        &s.audio_max_buffer_ms.to_string(), 438, 36, 0, ES_NUMBER as u32);
    row_label(hwnd, page, "Audio format", "RAW: BUD only; host must also enable RAW.\n48 kHz stereo PCM uses about 1.54 Mbps before overhead.\nThe actual codec is shown in Connection stats.", 532, 0, 60);
    binary_choice(hwnd, page, AUDIO_RAW, 532);
    check(hwnd, AUDIO_RAW, s.allow_raw_audio);
    row_label(hwnd, page, "Speaker layout", "Experimental: match the Windows output's 5.1 or 7.1 layout.\nSynthetic tests only; no live host/client validation.\nBUD only; stereo fallback. Reconnect to apply.", 630, 0, 68);
    binary_choice(hwnd, page, AUDIO_SURROUND, 630);
    check(hwnd, AUDIO_SURROUND, s.experimental_surround_audio);
    page.building_tab = Some(Tab::Input);
    row_label(
        hwnd,
        page,
        "Input mode",
        crate::settings_model::INPUT_MODE_HINT,
        242,
        0,
        48,
    );
    choice(
        hwnd,
        page,
        INPUT_MODE,
        242,
        &InputMode::LABELS,
        InputMode::current(&s) as usize,
    );
    row_label(
        hwnd,
        page,
        "HID requirements",
        crate::settings_model::HID_REQUIREMENTS_HINT,
        354,
        0,
        64,
    );
    row_label(
        hwnd,
        page,
        "Compatibility",
        crate::settings_model::HID_COMPATIBILITY_HINT,
        466,
        0,
        64,
    );

    page.building_tab = None;
    add(hwnd, page, "STATIC",
        "ICE chooses the route automatically unless Force TURN relay is enabled.\nChanges apply to your next connection.",
        0, [0, 1056 + BUD_EXTRA, 820, 64], NOTICE, 0, page.small);
    page.model.feedback = error.unwrap_or_else(|| {
        "Saved in settings.json. Secrets are protected for your Windows user.".into()
    });
    let status = page.model.feedback.clone();
    add(
        hwnd,
        page,
        "STATIC",
        &status,
        0,
        [0, 1118 + BUD_EXTRA, 820, 40],
        STATUS,
        0,
        page.small,
    );
    update_enabled(hwnd);
    ShowWindow(hwnd, SW_SHOW);
    RedrawWindow(
        hwnd,
        null(),
        null_mut(),
        RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN | RDW_UPDATENOW,
    );
    SetFocus(GetDlgItem(hwnd, STUN as i32));
    hwnd
}
#[allow(clippy::too_many_arguments)] // Native control descriptors, UI-thread only.
unsafe fn add(
    parent: HWND,
    page: &mut Page,
    class: &str,
    text: &str,
    style: u32,
    rect: [i32; 4],
    id: usize,
    group: u8,
    font: HFONT,
) -> HWND {
    let [x, y, w, h] = rect;
    let hwnd = control(parent, class, text, style, x, y, w, h, id, font);
    page.items.push(Item {
        hwnd,
        rect: RECT {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        },
        group,
        tab: page.building_tab,
    });
    hwnd
}
unsafe fn row_label(
    hwnd: HWND,
    page: &mut Page,
    label: &str,
    help: &str,
    y: i32,
    group: u8,
    help_height: i32,
) -> HWND {
    add(
        hwnd,
        page,
        "STATIC",
        label,
        0,
        [0, y + 2, 392, 24],
        0,
        group,
        page.font,
    );
    add(
        hwnd,
        page,
        "STATIC",
        help,
        0,
        [0, y + 32, 392, help_height],
        HELP,
        group,
        page.small,
    )
}
#[allow(clippy::too_many_arguments)] // Label, help and native editor describe one settings row.
unsafe fn row(
    hwnd: HWND,
    page: &mut Page,
    label: &str,
    help: &str,
    id: usize,
    value: &str,
    y: i32,
    height: i32,
    group: u8,
    style: u32,
) {
    row_label(hwnd, page, label, help, y, group, 48);
    add(
        hwnd,
        page,
        "EDIT",
        value,
        style | WS_BORDER | WS_TABSTOP,
        [420, y, 400, height],
        id,
        group,
        page.font,
    );
    if style & ES_MULTILINE as u32 != 0 {
        controls::multiline(GetDlgItem(hwnd, id as i32));
    }
}
/// Reflow on resize/provider changes. Hidden controls retain unsaved values.
unsafe fn layout(hwnd: HWND) {
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Page;
    if pointer.is_null() {
        return;
    }
    let page = &mut *pointer;
    if page.items.is_empty() || page.reflowing {
        return;
    }
    page.reflowing = true;
    let mut client = std::mem::zeroed();
    GetClientRect(hwnd, &mut client);
    let cloudflare = SendMessageW(GetDlgItem(hwnd, PROVIDER as i32), CB_GETCURSEL, 0, 0) == 1;
    let footer = match page.model.active_tab {
        Tab::Network => 1056 + BUD_EXTRA,
        Tab::Audio => 750,
        Tab::Input => 570,
        Tab::Video => 1034,
    };
    let height = footer + 138 + HEADER_EXTRA;
    page.scroll = page.scroll.clamp(0, (height - client.bottom).max(0));
    let info = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
        nMin: 0,
        nMax: height - 1,
        nPage: client.bottom.max(0) as u32,
        nPos: page.scroll,
        nTrackPos: 0,
    };
    SetScrollInfo(hwnd, SB_VERT, &info, 1);
    // Showing/hiding the scrollbar can synchronously send WM_SIZE. Reflow once,
    // using the final client width instead of recursively moving every control.
    GetClientRect(hwnd, &mut client);
    let x = ((client.right - 820) / 2).max(24);
    for item in &page.items {
        let visible = item.tab.is_none_or(|tab| tab == page.model.active_tab)
            && (item.group == 0 || item.group == if cloudflare { CLOUDFLARE } else { CUSTOM });
        let mut r = item.rect;
        let id = GetDlgCtrlID(item.hwnd) as usize;
        let new_top = match id {
            NOTICE => Some(footer),
            STATUS => Some(footer + 78),
            _ => None,
        };
        if let Some(top) = new_top {
            r.bottom += top - r.top;
            r.top = top;
        }
        // Moving overlapping child windows individually must not preserve old
        // pixels or paint halfway through reflow. A parent-only invalidation
        // excludes children (WS_CLIPCHILDREN), leaving clipped text/buttons.
        SetWindowPos(
            item.hwnd,
            null_mut(),
            x + r.left,
            r.top + if r.top >= 88 { HEADER_EXTRA } else { 0 } - page.scroll,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOZORDER
                | SWP_NOACTIVATE
                | SWP_NOCOPYBITS
                | SWP_NOREDRAW
                | if visible {
                    SWP_SHOWWINDOW
                } else {
                    SWP_HIDEWINDOW
                },
        );
    }
    page.reflowing = false;
    RedrawWindow(
        hwnd,
        null(),
        null_mut(),
        RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN | RDW_UPDATENOW,
    );
}

unsafe fn scroll_to(hwnd: HWND, requested: i32) {
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Page;
    if pointer.is_null() {
        return;
    }
    let mut info = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE,
        ..std::mem::zeroed()
    };
    if GetScrollInfo(hwnd, SB_VERT, &mut info) == 0 {
        return;
    }
    let limit = (info.nMax - info.nPage as i32 + 1).max(info.nMin);
    let position = requested.clamp(info.nMin, limit);
    if (*pointer).scroll != position {
        (*pointer).scroll = position;
        layout(hwnd);
    }
}

unsafe fn paint_page(hwnd: HWND, dc: HDC, page: &Page) {
    let mut rect: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut rect);
    // Paint is self-contained even when Windows supplies a paint region without
    // requesting a separate background erase (e.g. after an exposed child).
    FillRect(dc, &rect, page.background);
    let x = ((rect.right - 820) / 2).max(24);
    let line = RECT {
        left: x,
        top: 127 + HEADER_EXTRA - page.scroll,
        right: x + 820,
        bottom: 129 + HEADER_EXTRA - page.scroll,
    };
    SetDCBrushColor(dc, 0x003b3b3b);
    FillRect(dc, &line, GetStockObject(DC_BRUSH) as HBRUSH);
}
#[allow(clippy::too_many_arguments)] // Flat Win32 control descriptor, UI-thread only.
unsafe fn control(
    parent: HWND,
    class: &str,
    text: &str,
    style: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    id: usize,
    font: HFONT,
) -> HWND {
    let hwnd = CreateWindowExW(
        0,
        wide(class).as_ptr(),
        wide(text).as_ptr(),
        WS_CHILD
            | WS_VISIBLE
            | style
            | if class == "BUTTON" {
                BS_NOTIFY as u32
            } else {
                0
            },
        x,
        y,
        width,
        height,
        parent,
        id as HMENU,
        GetModuleHandleW(null()),
        null(),
    );
    SendMessageW(hwnd, WM_SETFONT, font as usize, 1);
    if class == "EDIT" {
        SendMessageW(hwnd, EM_SETLIMITTEXT, 16384, 0);
    }
    hwnd
}
unsafe fn text(hwnd: HWND, id: usize) -> String {
    let field = GetDlgItem(hwnd, id as i32);
    let mut buffer = vec![0; GetWindowTextLengthW(field).min(16384) as usize + 1];
    let length = GetWindowTextW(field, buffer.as_mut_ptr(), buffer.len() as i32);
    String::from_utf16_lossy(&buffer[..length as usize])
}
fn binary_choice_kind(id: usize) -> Option<BinaryChoice> {
    match id {
        TCP_QOS => Some(BinaryChoice::TcpQos),
        CHROMA_444 => Some(BinaryChoice::Chroma),
        AUDIO_CUSTOM => Some(BinaryChoice::AudioBuffer),
        AUDIO_RAW => Some(BinaryChoice::AudioCodec),
        AUDIO_SURROUND => Some(BinaryChoice::Speakers),
        #[cfg(feature = "vendor-video")]
        ZERO_COPY => Some(BinaryChoice::Rendering),
        _ => None,
    }
}
unsafe fn choice(hwnd: HWND, page: &mut Page, id: usize, y: i32, labels: &[&str], selected: usize) {
    let control = add(
        hwnd,
        page,
        "COMBOBOX",
        "",
        CBS_DROPDOWNLIST as u32
            | CBS_OWNERDRAWFIXED as u32
            | CBS_HASSTRINGS as u32
            | WS_VSCROLL
            | WS_TABSTOP,
        [420, y, 400, 200],
        id,
        0,
        page.font,
    );
    for label in labels {
        SendMessageW(control, CB_ADDSTRING, 0, wide(label).as_ptr() as isize);
    }
    controls::combo(control);
    select(hwnd, id, selected);
}
unsafe fn binary_choice(hwnd: HWND, page: &mut Page, id: usize, y: i32) {
    let labels = binary_choice_kind(id)
        .expect("binary mode binding")
        .labels();
    choice(hwnd, page, id, y, &labels, 0);
}
fn mode_combo(id: usize) -> bool {
    matches!(id, VIDEO_FORMAT | INPUT_MODE | ROUTE_MODE) || binary_choice_kind(id).is_some()
}
fn mode_label(id: usize, index: u32) -> &'static str {
    let index = index as usize;
    if id == VIDEO_FORMAT {
        VideoFormat::LABELS.get(index).copied()
    } else if id == INPUT_MODE {
        InputMode::LABELS.get(index).copied()
    } else if id == ROUTE_MODE {
        RouteMode::LABELS.get(index).copied()
    } else {
        binary_choice_kind(id).and_then(|kind| kind.labels().get(index).copied())
    }
    .unwrap_or_default()
}
fn binary_combo(id: usize) -> bool {
    #[cfg(feature = "bud")]
    if id == BUD {
        return true;
    }
    binary_choice_kind(id).is_some()
}
unsafe fn selection(hwnd: HWND, id: usize) -> usize {
    SendMessageW(GetDlgItem(hwnd, id as i32), CB_GETCURSEL, 0, 0) as usize
}
unsafe fn select(hwnd: HWND, id: usize, index: usize) {
    SendMessageW(GetDlgItem(hwnd, id as i32), CB_SETCURSEL, index, 0);
}
unsafe fn check(hwnd: HWND, id: usize, value: bool) {
    if binary_combo(id) {
        SendMessageW(
            GetDlgItem(hwnd, id as i32),
            CB_SETCURSEL,
            usize::from(value),
            0,
        );
        return;
    }
    SendMessageW(
        GetDlgItem(hwnd, id as i32),
        BM_SETCHECK,
        usize::from(value),
        0,
    );
}
unsafe fn checked(hwnd: HWND, id: usize) -> bool {
    if binary_combo(id) {
        return SendMessageW(GetDlgItem(hwnd, id as i32), CB_GETCURSEL, 0, 0) == 1;
    }
    SendMessageW(GetDlgItem(hwnd, id as i32), BM_GETCHECK, 0, 0) == 1
}
// Read native editor values; parsing, option rules and save feedback belong
// to the shared model. This runs on user edits/saving, never on a media frame.
unsafe fn read_options(hwnd: HWND, page: &mut Page) {
    let model = &mut page.model;
    model.gpu_selection = usize::try_from(SendMessageW(
        GetDlgItem(hwnd, GPU as i32),
        CB_GETCURSEL,
        0,
        0,
    ))
    .ok();
    let s = &mut model.values;
    if let Some(mode) = VideoFormat::ALL.get(selection(hwnd, VIDEO_FORMAT)) {
        mode.apply(s);
    }
    s.experimental_chroma_444 = checked(hwnd, CHROMA_444);
    if let Some(backend) = page.ai_upscaler {
        backend.apply(s, selection(hwnd, AI_UPSCALE));
    }
    s.custom_audio_buffer = checked(hwnd, AUDIO_CUSTOM);
    if let Some(mode) = RouteMode::ALL.get(selection(hwnd, ROUTE_MODE)) {
        mode.apply(s);
    }
    s.provider = if SendMessageW(GetDlgItem(hwnd, PROVIDER as i32), CB_GETCURSEL, 0, 0) == 1 {
        "cloudflare"
    } else {
        "custom"
    }
    .into();
    #[cfg(feature = "bud")]
    {
        s.bud = checked(hwnd, BUD);
    }
}
unsafe fn update_enabled(hwnd: HWND) {
    let page = &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Page);
    read_options(hwnd, page);
    let enabled = page.model.availability();
    let backend = page.model.ai_upscaler();
    if page.ai_upscaler != Some(backend) {
        let control = GetDlgItem(hwnd, AI_UPSCALE as i32);
        SendMessageW(control, WM_SETREDRAW, 0, 0);
        SendMessageW(control, CB_RESETCONTENT, 0, 0);
        for label in backend.labels() {
            SendMessageW(control, CB_ADDSTRING, 0, wide(label).as_ptr() as isize);
        }
        SendMessageW(
            control,
            CB_SETCURSEL,
            backend.selection(&page.model.values),
            0,
        );
        SetWindowTextW(page.ai_hint, wide(backend.hint()).as_ptr());
        page.ai_upscaler = Some(backend);
        SendMessageW(control, WM_SETREDRAW, 1, 0);
        InvalidateRect(control, null(), 0);
    }
    check(hwnd, CHROMA_444, page.model.values.experimental_chroma_444);
    for (id, on) in [
        (INPUT_MODE, enabled.bud),
        (AUDIO_MIN, enabled.audio_limits),
        (AUDIO_MAX, enabled.audio_limits),
        (AUDIO_RAW, enabled.bud),
        (AUDIO_SURROUND, enabled.bud),
        (CHROMA_444, enabled.chroma_444),
        (AI_UPSCALE, enabled.ai_upscale),
        (TCP_QOS, enabled.relay),
        (PROVIDER, enabled.relay),
        (TURN, enabled.custom_turn),
        (USERNAME, enabled.custom_turn),
        (PASSWORD, enabled.custom_turn),
        (FORGET_PASSWORD, enabled.custom_turn),
        (KEY, enabled.cloudflare_turn),
        (TOKEN, enabled.cloudflare_turn),
        (FORGET_TOKEN, enabled.cloudflare_turn),
        (TTL, enabled.cloudflare_turn),
        (CACHE, enabled.cloudflare_turn),
    ] {
        EnableWindow(GetDlgItem(hwnd, id as i32), i32::from(on));
    }
    #[cfg(feature = "vendor-video")]
    {
        EnableWindow(
            GetDlgItem(hwnd, ZERO_COPY as i32),
            i32::from(enabled.zero_copy),
        );
    }
    layout(hwnd);
}
unsafe fn save(hwnd: HWND, page: &mut Page) {
    read_options(hwnd, page);
    let model = &mut page.model;
    let vsr_available =
        model.ai_upscaler() == AiUpscaler::Nvidia && model.availability().ai_upscale;
    let s = &mut model.values;
    s.native_core.hide_session_button = checked(hwnd, HIDE_SESSION_BUTTON);
    s.presentation_mode = crate::connection_settings::PresentationMode::from_index(SendMessageW(
        GetDlgItem(hwnd, PRESENTATION as i32),
        CB_GETCURSEL,
        0,
        0,
    ) as usize);
    s.allow_raw_audio = checked(hwnd, AUDIO_RAW);
    s.experimental_surround_audio = checked(hwnd, AUDIO_SURROUND);
    s.event_logging = checked(hwnd, EVENT_LOG);
    s.experimental_tcp_qos = checked(hwnd, TCP_QOS);
    #[cfg(feature = "vendor-video")]
    {
        s.experimental_zero_copy = checked(hwnd, ZERO_COPY);
    }
    #[cfg(feature = "bud")]
    {
        if let Some(mode) = InputMode::ALL.get(selection(hwnd, INPUT_MODE)) {
            mode.apply(s);
        }
    }
    s.custom_username = text(hwnd, USERNAME);
    s.turn_key_id = text(hwnd, KEY);
    s.cache_credentials = checked(hwnd, CACHE);
    model.audio_min = text(hwnd, AUDIO_MIN);
    model.audio_max = text(hwnd, AUDIO_MAX);
    model.ttl = text(hwnd, TTL);
    model.stun_urls = text(hwnd, STUN);
    model.turn_urls = text(hwnd, TURN);
    model.password = zeroize::Zeroizing::new(text(hwnd, PASSWORD));
    model.token = zeroize::Zeroizing::new(text(hwnd, TOKEN));
    model.forget_password = checked(hwnd, FORGET_PASSWORD);
    model.forget_token = checked(hwnd, FORGET_TOKEN);
    let needs_vsr = s.nvidia_vsr != 0 && vsr_available && !crate::vsr_runtime::present();
    if needs_vsr && s.vsr_runtime_consent != crate::connection_settings::VsrRuntimeConsent::Allowed
    {
        let accepted = MessageBoxW(hwnd, wide("The NVIDIA VSR runtime is missing. Allow its verified download from NVIDIA on the next app start? Normal video remains available without it.").as_ptr(), wide("EvoParseq - NVIDIA VSR").as_ptr(), MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION) == IDYES;
        if accepted {
            s.vsr_runtime_consent = crate::connection_settings::VsrRuntimeConsent::Allowed;
        } else {
            s.nvidia_vsr = 0;
            PostMessageW(
                hwnd,
                WM_COMMAND,
                AI_UPSCALE | ((CBN_SELCHANGE as usize) << 16),
                0,
            );
            SendMessageW(GetDlgItem(hwnd, AI_UPSCALE as i32), CB_SETCURSEL, 0, 0);
        }
    }
    if model.save(&page.manager) {
        if needs_vsr
            && model.values.vsr_runtime_consent
                == crate::connection_settings::VsrRuntimeConsent::Allowed
        {
            model.feedback.push_str(crate::vsr_runtime::RESTART_HINT);
        }
        PostMessageW(
            GetParent(hwnd),
            crate::native_core::session_ui::VISIBILITY,
            0,
            0,
        );
        for id in [PASSWORD, TOKEN] {
            SetWindowTextW(GetDlgItem(hwnd, id as i32), wide("").as_ptr());
        }
        check(hwnd, FORGET_PASSWORD, false);
        check(hwnd, FORGET_TOKEN, false);
    }
}

unsafe extern "system" fn page_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            (*(lp as *const CREATESTRUCTW)).lpCreateParams as isize,
        );
    }
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Page;
    if !pointer.is_null() {
        let page = &mut *pointer;
        match message {
            WM_SIZE => {
                layout(hwnd);
                return 0;
            }
            WM_MOUSEWHEEL => {
                let requested = page.scroll - ((wp >> 16) as u16 as i16 as i32) * 72 / 120;
                scroll_to(hwnd, requested);
                return 0;
            }
            WM_VSCROLL => {
                let mut info = SCROLLINFO {
                    cbSize: size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_ALL,
                    ..std::mem::zeroed()
                };
                GetScrollInfo(hwnd, SB_VERT, &mut info);
                let requested = match (wp & 0xffff) as i32 {
                    SB_LINEUP => page.scroll - 36,
                    SB_LINEDOWN => page.scroll + 36,
                    SB_PAGEUP => page.scroll - info.nPage as i32,
                    SB_PAGEDOWN => page.scroll + info.nPage as i32,
                    SB_THUMBTRACK | SB_THUMBPOSITION => info.nTrackPos,
                    SB_TOP => 0,
                    SB_BOTTOM => info.nMax,
                    _ => page.scroll,
                };
                scroll_to(hwnd, requested);
                return 0;
            }
            WM_MEASUREITEM if wp == PROVIDER || wp == GPU || mode_combo(wp) => {
                (*(lp as *mut MEASUREITEMSTRUCT)).itemHeight = 30;
                return 1;
            }
            WM_DRAWITEM => {
                let draw = &*(lp as *const DRAWITEMSTRUCT);
                if [
                    SAVE as u32,
                    BACK as u32,
                    NETWORK_TAB as u32,
                    VIDEO_TAB as u32,
                    AUDIO_TAB as u32,
                    INPUT_TAB as u32,
                    PROVIDER as u32,
                    GPU as u32,
                ]
                .contains(&draw.CtlID)
                    || mode_combo(draw.CtlID as usize)
                {
                    let dc = draw.hDC;
                    let state = SaveDC(dc);
                    let primary = draw.CtlID == SAVE as u32;
                    let selected_tab = (draw.CtlID == NETWORK_TAB as u32
                        && page.model.active_tab == Tab::Network)
                        || (draw.CtlID == VIDEO_TAB as u32 && page.model.active_tab == Tab::Video)
                        || (draw.CtlID == AUDIO_TAB as u32 && page.model.active_tab == Tab::Audio)
                        || (draw.CtlID == INPUT_TAB as u32 && page.model.active_tab == Tab::Input);
                    let color = if draw.itemState & ODS_SELECTED != 0 {
                        0x00404040
                    } else if primary || selected_tab {
                        ACCENT
                    } else {
                        FIELD
                    };
                    let brush = CreateSolidBrush(color);
                    FillRect(dc, &draw.rcItem, brush);
                    DeleteObject(brush);
                    SelectObject(dc, page.font);
                    SetBkMode(dc, TRANSPARENT as i32);
                    SetTextColor(
                        dc,
                        if draw.itemState & ODS_DISABLED != 0 {
                            MUTED
                        } else if (primary || selected_tab) && draw.itemState & ODS_SELECTED == 0 {
                            FIELD
                        } else {
                            FG
                        },
                    );
                    let combo = draw.CtlID == PROVIDER as u32
                        || draw.CtlID == GPU as u32
                        || mode_combo(draw.CtlID as usize);
                    let label = if draw.CtlID == PROVIDER as u32 {
                        match draw.itemID {
                            0 => "Custom servers".into(),
                            1 => "Cloudflare TURN".into(),
                            _ => String::new(),
                        }
                    } else if draw.CtlID == GPU as u32 {
                        page.gpu_labels
                            .get(draw.itemID as usize)
                            .cloned()
                            .unwrap_or_default()
                    } else if mode_combo(draw.CtlID as usize) {
                        mode_label(draw.CtlID as usize, draw.itemID).into()
                    } else if draw.CtlID == NETWORK_TAB as u32 {
                        "Network".into()
                    } else if draw.CtlID == VIDEO_TAB as u32 {
                        "Video".into()
                    } else if draw.CtlID == AUDIO_TAB as u32 {
                        "Audio".into()
                    } else if draw.CtlID == INPUT_TAB as u32 {
                        "Input".into()
                    } else if primary {
                        "Save settings".into()
                    } else {
                        "Back".into()
                    };
                    let mut rect = draw.rcItem;
                    rect.left += 10;
                    rect.right -= 10;
                    DrawTextW(
                        dc,
                        wide(&label).as_ptr(),
                        -1,
                        &mut rect,
                        DT_SINGLELINE | DT_VCENTER | if combo { DT_LEFT } else { DT_CENTER },
                    );
                    if draw.itemState & ODS_FOCUS != 0 {
                        InflateRect(&mut rect, -2, -4);
                        DrawFocusRect(dc, &rect);
                    }
                    RestoreDC(dc, state);
                    return 1;
                }
            }
            WM_COMMAND if matches!((wp >> 16) as u32, EN_SETFOCUS | BN_SETFOCUS | CBN_SETFOCUS) => {
                let mut rect: RECT = std::mem::zeroed();
                GetWindowRect(lp as HWND, &mut rect);
                MapWindowPoints(null_mut(), hwnd, (&mut rect as *mut RECT).cast(), 2);
                let mut client = std::mem::zeroed();
                GetClientRect(hwnd, &mut client);
                if rect.top < 8 {
                    scroll_to(hwnd, page.scroll + rect.top - 8);
                } else if rect.bottom > client.bottom - 8 {
                    scroll_to(hwnd, page.scroll + rect.bottom - client.bottom + 8);
                } else {
                    return 0;
                }
                return 0;
            }
            WM_COMMAND => match wp & 0xffff {
                NETWORK_TAB | VIDEO_TAB | AUDIO_TAB | INPUT_TAB
                    if (wp >> 16) as u32 == BN_CLICKED =>
                {
                    page.model.active_tab = if wp & 0xffff == VIDEO_TAB {
                        Tab::Video
                    } else if wp & 0xffff == INPUT_TAB {
                        Tab::Input
                    } else if wp & 0xffff == AUDIO_TAB {
                        Tab::Audio
                    } else {
                        Tab::Network
                    };
                    page.scroll = 0;
                    SetWindowTextW(
                        GetDlgItem(hwnd, SECTION as i32),
                        wide(page.model.active_tab.heading()).as_ptr(),
                    );
                    SetWindowTextW(
                        GetDlgItem(hwnd, NOTICE as i32),
                        wide(page.model.active_tab.notice()).as_ptr(),
                    );
                    layout(hwnd);
                    return 0;
                }
                SAVE | 1 => {
                    save(hwnd, page);
                    SetWindowTextW(
                        GetDlgItem(hwnd, STATUS as i32),
                        wide(&page.model.feedback).as_ptr(),
                    );
                    return 0;
                }
                BACK => {
                    PostMessageW(GetParent(hwnd), WM_APP + 9, 0, 0);
                    return 0;
                }
                GPU | PROVIDER if (wp >> 16) as u32 == CBN_SELCHANGE => {
                    update_enabled(hwnd);
                    return 0;
                }
                VIDEO_FORMAT if (wp >> 16) as u32 == CBN_SELCHANGE => {
                    update_enabled(hwnd);
                    return 0;
                }

                CHROMA_444 | AI_UPSCALE if (wp >> 16) as u32 == CBN_SELCHANGE => {
                    update_enabled(hwnd);
                    return 0;
                }
                AUDIO_CUSTOM if (wp >> 16) as u32 == CBN_SELCHANGE => {
                    update_enabled(hwnd);
                    return 0;
                }
                #[cfg(feature = "bud")]
                BUD if (wp >> 16) as u32 == CBN_SELCHANGE => {
                    update_enabled(hwnd);
                    return 0;
                }
                ROUTE_MODE if (wp >> 16) as u32 == CBN_SELCHANGE => {
                    update_enabled(hwnd);
                    return 0;
                }
                _ => {}
            },
            WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                let dc = wp as HDC;
                let edit = matches!(message, WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX);
                let id = GetDlgCtrlID(lp as HWND) as usize;
                SetTextColor(
                    dc,
                    if id == SECTION {
                        ACCENT
                    } else if matches!(id, HELP | NOTICE | STATUS)
                        || IsWindowEnabled(lp as HWND) == 0
                    {
                        MUTED
                    } else {
                        FG
                    },
                );
                SetBkColor(dc, if edit { FIELD } else { BG });
                return if edit { page.field } else { page.background } as isize;
            }
            WM_ERASEBKGND => {
                let mut rect = std::mem::zeroed();
                GetClientRect(hwnd, &mut rect);
                FillRect(wp as HDC, &rect, page.background);
                return 1;
            }
            WM_PAINT => {
                let mut paint = std::mem::zeroed();
                let dc = BeginPaint(hwnd, &mut paint);
                paint_page(hwnd, dc, page);
                EndPaint(hwnd, &paint);
                return 0;
            }
            WM_PRINTCLIENT => {
                paint_page(hwnd, wp as HDC, page);
                return 0;
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                if page.window_owned {
                    drop(Box::from_raw(pointer));
                }
            }
            _ => {}
        }
    }
    DefWindowProcW(hwnd, message, wp, lp)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_controls_save_and_reopen_without_real_profile() {
        let mut nonce = [0; 16];
        getrandom::fill(&mut nonce).unwrap();
        let suffix: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
        let directory = std::env::temp_dir().join(format!("evoparseq-settings-ui-{suffix}"));
        std::fs::create_dir(&directory).unwrap();
        let manager = Manager::open(directory.join("settings.json"));
        unsafe {
            let parent = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                wide("Settings fixture").as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                920,
                750,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            assert!(!parent.is_null());
            install_menu(parent);
            assert_eq!(GetMenuItemCount(GetMenu(parent)), 3);
            let help = GetSubMenu(GetMenu(parent), 2);
            assert!(!help.is_null());
            assert_eq!(GetMenuItemID(help, 0), ABOUT as u32);
            assert_eq!(GetMenuItemID(help, 1), crate::updater_ui::OPEN as u32);
            let page = open(parent, manager.clone());
            assert!(!page.is_null());
            assert_ne!(
                GetWindowLongW(GetDlgItem(page, TURN as i32), GWL_STYLE) as u32 & WS_VISIBLE,
                0
            );
            assert_eq!(
                GetWindowLongW(GetDlgItem(page, KEY as i32), GWL_STYLE) as u32 & WS_VISIBLE,
                0
            );
            assert_no_overlaps(page);
            capture(page, "custom");
            controls::verify_multiline(GetDlgItem(page, STUN as i32));
            SetWindowTextW(
                GetDlgItem(page, USERNAME as i32),
                wide("unsaved-user").as_ptr(),
            );
            SendMessageW(GetDlgItem(page, PROVIDER as i32), CB_SETCURSEL, 1, 0);
            update_enabled(page);
            assert_eq!(
                GetWindowLongW(GetDlgItem(page, TURN as i32), GWL_STYLE) as u32 & WS_VISIBLE,
                0
            );
            assert_ne!(
                GetWindowLongW(GetDlgItem(page, KEY as i32), GWL_STYLE) as u32 & WS_VISIBLE,
                0
            );
            assert_no_overlaps(page);
            capture(page, "cloudflare");
            assert_combo_label_visible(page, PROVIDER);
            paint_regression(page);
            scroll_boundary_does_not_repaint(page);
            // The last control remains reachable at a small window size and
            // keyboard focus scrolls it into view without touching real data.
            SendMessageW(
                page,
                WM_COMMAND,
                SAVE | ((BN_SETFOCUS as usize) << 16),
                GetDlgItem(page, SAVE as i32) as isize,
            );
            let mut button = std::mem::zeroed();
            let mut client = std::mem::zeroed();
            GetWindowRect(GetDlgItem(page, SAVE as i32), &mut button);
            MapWindowPoints(null_mut(), page, (&mut button as *mut RECT).cast(), 2);
            GetClientRect(page, &mut client);
            assert!(button.top >= 0 && button.bottom <= client.bottom);
            SendMessageW(page, WM_VSCROLL, SB_BOTTOM as usize, 0);
            capture(page, "cloudflare-bottom");
            SetWindowTextW(
                GetDlgItem(page, TOKEN as i32),
                wide("fixture-token").as_ptr(),
            );
            #[cfg(feature = "bud")]
            check(page, BUD, true);
            SendMessageW(page, WM_COMMAND, VIDEO_TAB, 0);
            assert_eq!(IsWindowVisible(GetDlgItem(page, PROVIDER as i32)), 0);
            assert_ne!(IsWindowVisible(GetDlgItem(page, GPU as i32)), 0);
            select(page, VIDEO_FORMAT, VideoFormat::Hevc8 as usize);
            SendMessageW(
                page,
                WM_COMMAND,
                VIDEO_FORMAT | ((CBN_SELCHANGE as usize) << 16),
                0,
            );
            assert_eq!(
                GetWindowLongW(GetDlgItem(page, VIDEO_FORMAT as i32), GWL_STYLE) as u32 & 3,
                CBS_DROPDOWNLIST as u32
            );
            assert_combo_label_visible(page, VIDEO_FORMAT);
            assert_no_overlaps(page);
            capture(page, "video");
            assert_combo_label_visible(page, GPU);

            {
                // Synthetic adapter identities: no production GPU settings are changed.
                let state = &mut *(GetWindowLongPtrW(page, GWLP_USERDATA) as *mut Page);
                let default_vendor = state.model.default_gpu_vendor;
                SendMessageW(GetDlgItem(page, GPU as i32), CB_SETCURSEL, 0, 0);
                for vendor in [0x10de, 0x1002, 0x8086] {
                    state.model.default_gpu_vendor = Some(vendor);
                    check(page, CHROMA_444, true);
                    update_enabled(page);
                    assert_eq!(
                        IsWindowEnabled(GetDlgItem(page, CHROMA_444 as i32)) != 0,
                        matches!(vendor, 0x10de | 0x8086)
                    );
                    assert_eq!(checked(page, CHROMA_444), matches!(vendor, 0x10de | 0x8086));
                    assert_ne!(IsWindowEnabled(GetDlgItem(page, VIDEO_FORMAT as i32)), 0);
                    #[cfg(feature = "vendor-video")]
                    assert_eq!(
                        IsWindowEnabled(GetDlgItem(page, ZERO_COPY as i32)) != 0,
                        !matches!(vendor, 0x10de | 0x8086)
                    );
                }
                state.model.default_gpu_vendor = default_vendor;
                check(page, CHROMA_444, false);
                update_enabled(page);
            }

            SendMessageW(page, WM_COMMAND, AUDIO_TAB, 0);
            assert_ne!(IsWindowVisible(GetDlgItem(page, AUDIO_CUSTOM as i32)), 0);
            assert_eq!(IsWindowEnabled(GetDlgItem(page, AUDIO_MIN as i32)), 0);
            check(page, AUDIO_CUSTOM, true);
            update_enabled(page);
            assert_ne!(IsWindowEnabled(GetDlgItem(page, AUDIO_MIN as i32)), 0);
            SetWindowTextW(GetDlgItem(page, AUDIO_MIN as i32), wide("20").as_ptr());
            SetWindowTextW(GetDlgItem(page, AUDIO_MAX as i32), wide("60").as_ptr());
            check(page, AUDIO_RAW, true);
            check(page, AUDIO_SURROUND, true);
            assert_no_overlaps(page);
            capture(page, "audio");
            SendMessageW(page, WM_COMMAND, INPUT_TAB, 0);
            assert_ne!(IsWindowVisible(GetDlgItem(page, INPUT_MODE as i32)), 0);
            assert_eq!(IsWindowVisible(GetDlgItem(page, GPU as i32)), 0);
            #[cfg(feature = "bud")]
            {
                update_enabled(page);
                assert_ne!(IsWindowEnabled(GetDlgItem(page, INPUT_MODE as i32)), 0);
                select(page, INPUT_MODE, InputMode::HidMouse as usize);
                check(page, BUD, false);
                update_enabled(page);
                assert_eq!(IsWindowEnabled(GetDlgItem(page, INPUT_MODE as i32)), 0);
                assert_eq!(selection(page, INPUT_MODE), InputMode::HidMouse as usize);
                check(page, BUD, true);
                update_enabled(page);
            }
            assert_combo_label_visible(page, INPUT_MODE);
            assert_no_overlaps(page);
            capture(page, "input");

            SendMessageW(page, WM_COMMAND, NETWORK_TAB, 0);
            assert_eq!(IsWindowVisible(GetDlgItem(page, GPU as i32)), 0);
            assert_ne!(selection(page, VIDEO_FORMAT), VideoFormat::H264 as usize);
            assert_eq!(text(page, TOKEN), "fixture-token");
            assert_eq!(text(page, USERNAME), "unsaved-user");
            SendMessageW(GetDlgItem(page, PROVIDER as i32), CB_SETCURSEL, 0, 0);
            update_enabled(page);
            assert_eq!(text(page, USERNAME), "unsaved-user");
            controls::verify_enter(
                page,
                GetDlgItem(page, STUN as i32),
                "stun:stun.example.org:3478",
                "stun:second.example.invalid:3478",
            );
            controls::verify_enter(
                page,
                GetDlgItem(page, TURN as i32),
                "turn:relay.example.invalid:3478?transport=udp",
                "turns:relay.example.invalid:443?transport=tcp",
            );
            SetWindowTextW(
                GetDlgItem(page, PASSWORD as i32),
                wide("fixture-password").as_ptr(),
            );
            select(page, ROUTE_MODE, RouteMode::ForceRelay as usize);
            select(page, VIDEO_FORMAT, VideoFormat::Hevc10 as usize);
            SendMessageW(GetDlgItem(page, PRESENTATION as i32), CB_SETCURSEL, 2, 0);
            check(page, EVENT_LOG, true);
            check(page, TCP_QOS, true);
            SetWindowTextW(GetDlgItem(page, AUDIO_MIN as i32), wide("invalid").as_ptr());
            SendMessageW(page, WM_COMMAND, SAVE, 0);
            assert_eq!(text(page, STATUS), "Enter an audio minimum in milliseconds");
            assert_eq!(text(page, PASSWORD), "fixture-password");
            assert_eq!(text(page, TOKEN), "fixture-token");
            SetWindowTextW(GetDlgItem(page, AUDIO_MIN as i32), wide("20").as_ptr());
            SendMessageW(page, WM_COMMAND, SAVE, 0);
            assert!(manager.view().0.force_relay);
            assert!(manager.view().0.experimental_ten_bit);
            assert_eq!(
                manager.view().0.presentation_mode,
                crate::connection_settings::PresentationMode::Vsync
            );
            SendMessageW(GetDlgItem(page, PRESENTATION as i32), CB_SETCURSEL, 3, 0);
            SendMessageW(page, WM_COMMAND, SAVE, 0);
            assert_eq!(
                manager.view().0.presentation_mode,
                crate::connection_settings::PresentationMode::Vrr
            );
            assert!(manager.view().0.event_logging);
            assert!(manager.view().0.experimental_tcp_qos);
            assert!(manager.view().0.custom_audio_buffer);
            assert_eq!(manager.view().0.audio_min_buffer_ms, 20);
            assert_eq!(manager.view().0.audio_max_buffer_ms, 60);
            assert!(manager.view().0.allow_raw_audio);
            assert!(manager.view().0.experimental_surround_audio);
            #[cfg(feature = "bud")]
            assert!(manager.view().0.bud);
            #[cfg(feature = "bud")]
            assert!(manager.view().0.hid_mouse_enabled());
            assert_eq!(manager.view().0.stun_urls.len(), 2);
            assert_eq!(manager.view().0.turn_urls.len(), 2);
            select(page, ROUTE_MODE, RouteMode::DirectOnly as usize);
            SendMessageW(
                page,
                WM_COMMAND,
                ROUTE_MODE | ((CBN_SELCHANGE as usize) << 16),
                0,
            );
            assert_eq!(selection(page, ROUTE_MODE), RouteMode::DirectOnly as usize);
            select(page, VIDEO_FORMAT, VideoFormat::Hevc8 as usize);
            update_enabled(page);
            assert_eq!(IsWindowEnabled(GetDlgItem(page, TURN as i32)), 0);
            let state = &*(GetWindowLongPtrW(page, GWLP_USERDATA) as *const Page);
            let selected = usize::from(state.model.gpu_choices.len() > 1);
            let expected_gpu = state.model.gpu_choices[selected].clone();
            SendMessageW(GetDlgItem(page, GPU as i32), CB_SETCURSEL, selected, 0);
            SendMessageW(page, WM_COMMAND, SAVE, 0);
            assert!(text(page, STATUS).starts_with("Settings saved"));
            assert_eq!(manager.view().0.video_gpu, expected_gpu);
            #[cfg(feature = "vendor-video")]
            {
                assert!(!manager.view().0.experimental_zero_copy);
                check(page, ZERO_COPY, true);
                SendMessageW(page, WM_COMMAND, SAVE, 0);
                assert!(manager.view().0.experimental_zero_copy);
            }
            assert!(manager.view().0.prefer_hevc);
            DestroyWindow(page);
            let page = open(parent, manager.clone());
            assert!(checked(page, TCP_QOS));
            assert_eq!(
                text(page, STUN),
                "stun:stun.example.org:3478\r\nstun:second.example.invalid:3478"
            );
            assert_eq!(text(page, TURN), "turn:relay.example.invalid:3478?transport=udp\r\nturns:relay.example.invalid:443?transport=tcp");
            assert_eq!(selection(page, ROUTE_MODE), RouteMode::DirectOnly as usize);
            #[cfg(feature = "vendor-video")]
            assert!(checked(page, ZERO_COPY));
            assert_ne!(selection(page, VIDEO_FORMAT), VideoFormat::H264 as usize);
            #[cfg(feature = "bud")]
            assert_eq!(selection(page, INPUT_MODE), InputMode::HidMouse as usize);
            assert_eq!(
                SendMessageW(GetDlgItem(page, GPU as i32), CB_GETCURSEL, 0, 0),
                selected as isize
            );
            SendMessageW(GetDlgItem(page, GPU as i32), CB_SETCURSEL, 0, 0);
            SendMessageW(page, WM_COMMAND, SAVE, 0);
            assert!(manager.view().0.video_gpu.is_none());
            DestroyWindow(page);
            DestroyWindow(parent);
        }
        assert!(
            Manager::open(directory.join("settings.json"))
                .view()
                .0
                .stun_only
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    unsafe fn assert_combo_label_visible(hwnd: HWND, id: usize) {
        let mut rect: RECT = std::mem::zeroed();
        GetWindowRect(GetDlgItem(hwnd, id as i32), &mut rect);
        MapWindowPoints(null_mut(), hwnd, (&mut rect as *mut RECT).cast(), 2);
        let (width, height, pixels) = snapshot(hwnd, false);
        assert!(rect.top >= 0 && rect.bottom <= height as i32);
        let mut foreground = 0;
        // Exclude the border and arrow; a blank owner-drawn selection must fail.
        for y in rect.top + 5..rect.bottom - 5 {
            for x in rect.left + 10..rect.right - 28 {
                let offset = (y as usize * width as usize + x as usize) * 4;
                if pixels[offset..offset + 3].iter().all(|v| *v > 140) {
                    foreground += 1;
                }
            }
        }
        assert!(foreground > 20, "Missing selected text in combo {id}");
    }

    unsafe fn assert_no_overlaps(hwnd: HWND) {
        let page = &*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Page);
        let rectangles: Vec<_> = page
            .items
            .iter()
            .filter(|item| GetWindowLongW(item.hwnd, GWL_STYLE) as u32 & WS_VISIBLE != 0)
            .map(|item| {
                let mut rect: RECT = std::mem::zeroed();
                GetWindowRect(item.hwnd, &mut rect);
                (item.hwnd, rect)
            })
            .collect();
        for (i, (first, a)) in rectangles.iter().enumerate() {
            for (second, b) in &rectangles[i + 1..] {
                assert!(
                    a.left >= b.right
                        || b.left >= a.right
                        || a.top >= b.bottom
                        || b.top >= a.bottom,
                    "Overlapping settings controls: {:?} and {:?}",
                    first,
                    second
                );
            }
        }
    }

    unsafe fn scroll_boundary_does_not_repaint(hwnd: HWND) {
        use std::cell::Cell;
        use windows_sys::Win32::UI::Shell::{
            DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass,
        };
        unsafe extern "system" fn observe(
            hwnd: HWND,
            msg: u32,
            wp: WPARAM,
            lp: LPARAM,
            _id: usize,
            data: usize,
        ) -> LRESULT {
            if msg == WM_PAINT {
                let count = &*(data as *const Cell<usize>);
                count.set(count.get() + 1);
            }
            DefSubclassProc(hwnd, msg, wp, lp)
        }
        assert_ne!(
            GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_COMPOSITED,
            0
        );
        assert_eq!(
            GetClassLongW(hwnd, GCL_STYLE) & (CS_OWNDC | CS_CLASSDC | CS_PARENTDC),
            0
        );
        let paints = Cell::new(0usize);
        let id = 0x5057;
        assert_ne!(
            SetWindowSubclass(
                hwnd,
                Some(observe),
                id,
                (&paints as *const Cell<usize>) as usize
            ),
            0
        );
        struct Remove(HWND, usize);
        impl Drop for Remove {
            fn drop(&mut self) {
                unsafe {
                    RemoveWindowSubclass(self.0, Some(observe), self.1);
                }
            }
        }
        let _remove = Remove(hwnd, id);
        for (edge, delta, line, page) in [
            (SB_BOTTOM, -120i16, SB_LINEDOWN, SB_PAGEDOWN),
            (SB_TOP, 120i16, SB_LINEUP, SB_PAGEUP),
        ] {
            SendMessageW(hwnd, WM_VSCROLL, edge as usize, 0);
            paints.set(0);
            for _ in 0..5 {
                SendMessageW(hwnd, WM_MOUSEWHEEL, (delta as u16 as usize) << 16, 0);
                SendMessageW(hwnd, WM_VSCROLL, line as usize, 0);
                SendMessageW(hwnd, WM_VSCROLL, page as usize, 0);
                SendMessageW(hwnd, WM_VSCROLL, SB_ENDSCROLL as usize, 0);
            }
            assert_eq!(
                paints.get(),
                0,
                "Scrolling beyond the boundary repainted Settings"
            );
        }
        SendMessageW(hwnd, WM_MOUSEWHEEL, (-120i16 as u16 as usize) << 16, 0);
        assert!(paints.get() > 0, "Real scrolling must repaint");
    }

    // Compare the actual incremental window painting against a clean repaint.
    // WM_PRINT alone cannot catch stale on-screen pixels: it always paints fresh.
    unsafe fn paint_regression(hwnd: HWND) {
        let parent = GetParent(hwnd);
        SetFocus(parent); // Suppress edit carets so both snapshots are deterministic.
        SetWindowPos(
            parent,
            HWND_TOPMOST,
            0,
            0,
            920,
            750,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        for (width, height) in [(920, 600), (1000, 740), (920, 750)] {
            SetWindowPos(
                parent,
                null_mut(),
                0,
                0,
                width,
                height,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let mut client: RECT = std::mem::zeroed();
            GetClientRect(parent, &mut client);
            SetWindowPos(
                hwnd,
                null_mut(),
                0,
                0,
                client.right,
                client.bottom,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            for direction in [SB_LINEDOWN, SB_LINEUP] {
                for _ in 0..6 {
                    SendMessageW(hwnd, WM_VSCROLL, direction as usize, 0);
                    assert_no_overlaps(hwnd);
                    let actual = snapshot(hwnd, true);
                    RedrawWindow(
                        hwnd,
                        null(),
                        null_mut(),
                        RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN | RDW_UPDATENOW,
                    );
                    let clean = snapshot(hwnd, true);
                    let changed = actual
                        .2
                        .iter()
                        .zip(&clean.2)
                        .filter(|(a, b)| a != b)
                        .count();
                    assert_eq!(
                        changed, 0,
                        "Stale pixels after scroll/resize at {width}x{height}"
                    );
                }
            }
        }
        capture(hwnd, "cloudflare-after-scroll-resize");
    }

    // Optional CI-only visual fixtures. This cannot capture an account, stream
    // or production profile: the fixture above owns only synthetic controls.
    unsafe fn capture(hwnd: HWND, name: &str) {
        // Visual assertions also need a painted window when PNG export is off.
        ShowWindow(GetParent(hwnd), SW_SHOWNOACTIVATE);
        RedrawWindow(
            GetParent(hwnd),
            null(),
            null_mut(),
            RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
        );
        let Some(directory) = std::env::var_os("PARSEC_SETTINGS_SCREENSHOTS") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        let (width, height, rgba) = snapshot(hwnd, false);
        image::save_buffer(
            directory.join(format!("{name}.png")),
            &rgba,
            width,
            height,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }

    unsafe fn snapshot(hwnd: HWND, live: bool) -> (u32, u32, Vec<u8>) {
        let mut rect: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rect);
        let dc = CreateCompatibleDC(null_mut());
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = rect.right;
        info.bmiHeader.biHeight = -rect.bottom;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut pixels = null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut pixels, null_mut(), 0);
        assert!(!bitmap.is_null() && !dc.is_null() && !pixels.is_null());
        let previous = SelectObject(dc, bitmap);
        if live {
            let source = GetDC(hwnd);
            assert!(!source.is_null());
            let copied = BitBlt(dc, 0, 0, rect.right, rect.bottom, source, 0, 0, SRCCOPY);
            ReleaseDC(hwnd, source);
            assert_ne!(copied, 0);
        } else {
            SendMessageW(
                hwnd,
                WM_PRINT,
                dc as usize,
                (PRF_CLIENT | PRF_NONCLIENT | PRF_CHILDREN | PRF_ERASEBKGND) as isize,
            );
        }
        GdiFlush();
        let mut rgba = std::slice::from_raw_parts(
            pixels.cast::<u8>(),
            (rect.right * rect.bottom * 4) as usize,
        )
        .to_vec();
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }
        SelectObject(dc, previous);
        DeleteObject(bitmap);
        DeleteDC(dc);
        (rect.right as u32, rect.bottom as u32, rgba)
    }
}
