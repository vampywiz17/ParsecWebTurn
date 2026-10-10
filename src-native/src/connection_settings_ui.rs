//! Native, modeless settings page inside the existing client window.
//! All HWND/GDI ownership and control access stay on the window's UI thread.
use crate::connection_settings::{Manager, Settings};
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
const SAVE: usize = 4101;
const BACK: usize = 4102;
const PROVIDER: usize = 4103;
const STUN_ONLY: usize = 4104;
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
const BG: u32 = 0x00282828;
const FIELD: u32 = 0x00212121;
const FG: u32 = 0x00eeeeee;
const MUTED: u32 = 0x00bcbcbc;
const ACCENT: u32 = 0x00ffbb00;
const CUSTOM: u8 = 1;
const CLOUDFLARE: u8 = 2;
const HELP: usize = 4200;
const TAB: usize = 4201;
const SECTION: usize = 4202;
const NOTICE: usize = 4203;
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
struct Item {
    hwnd: HWND,
    rect: RECT,
    group: u8,
}
struct Page {
    window_owned: bool,
    manager: Arc<Manager>,
    saved: Settings,
    background: HBRUSH,
    field: HBRUSH,
    font: HFONT,
    heading: HFONT,
    small: HFONT,
    items: Vec<Item>,
    scroll: i32,
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
    SetMenu(parent, menu);
    DrawMenuBar(parent);
}
pub unsafe fn open(parent: HWND, manager: Arc<Manager>) -> HWND {
    let name = wide("ParsecWebTurnSettings");
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
        saved,
        background: CreateSolidBrush(BG),
        field: CreateSolidBrush(FIELD),
        font: font(16, 400),
        heading: font(46, 300),
        small: font(13, 400),
        items: Vec::new(),
        scroll: 0,
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
        WS_EX_CONTROLPARENT,
        name.as_ptr(),
        wide("Settings").as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_VSCROLL,
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
    let s = page.saved.clone();
    add(
        hwnd,
        page,
        "STATIC",
        "Settings",
        0,
        [0, 24, 820, 58],
        0,
        0,
        page.heading,
    );
    add(
        hwnd,
        page,
        "STATIC",
        "Customize how ParsecWebTurn connects to your computer.",
        0,
        [4, 88, 816, 24],
        0,
        0,
        page.font,
    );
    add(
        hwnd,
        page,
        "STATIC",
        "Network",
        0,
        [4, 142, 300, 24],
        TAB,
        0,
        page.font,
    );
    add(
        hwnd,
        page,
        "STATIC",
        "CONNECTION SETTINGS",
        0,
        [0, 192, 820, 25],
        SECTION,
        0,
        page.font,
    );
    row(hwnd, page, "STUN servers",
        "Discover a direct route. One stun: URL per line.\nLeave blank to use the default discovery server.",
        STUN, &s.stun_urls.join("\r\n"), 242, 76, 0, ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | WS_VSCROLL);
    row_label(
        hwnd,
        page,
        "STUN only",
        "For LAN or VPN connections that do not need a relay.",
        344,
        0,
        40,
    );
    add(
        hwnd,
        page,
        "BUTTON",
        "Use STUN only (no TURN relay)",
        BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
        [420, 344, 400, 36],
        STUN_ONLY,
        0,
        page.font,
    );
    check(hwnd, STUN_ONLY, s.stun_only);
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
        ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | WS_VSCROLL);
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
        "Reuse valid credentials until you close ParsecWebTurn.",
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
    add(hwnd, page, "STATIC",
        "ICE chooses the route automatically. Enabling TURN may select a relay even when a direct route exists.\nChanges apply to your next connection.",
        0, [0, 816, 820, 48], NOTICE, 0, page.small);
    let status = error.unwrap_or_else(|| {
        "Saved in settings.json. Secrets are protected for your Windows user.".into()
    });
    add(
        hwnd,
        page,
        "STATIC",
        &status,
        0,
        [0, 878, 820, 40],
        STATUS,
        0,
        page.small,
    );
    add(
        hwnd,
        page,
        "BUTTON",
        "Back",
        BS_OWNERDRAW as u32 | WS_TABSTOP,
        [550, 934, 110, 40],
        BACK,
        0,
        page.font,
    );
    add(
        hwnd,
        page,
        "BUTTON",
        "Save settings",
        BS_OWNERDRAW as u32 | WS_TABSTOP,
        [676, 934, 144, 40],
        SAVE,
        0,
        page.font,
    );
    update_enabled(hwnd);
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
) {
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
    );
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
}
/// Reflow on resize/provider changes. Hidden controls retain unsaved values.
unsafe fn layout(hwnd: HWND) {
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Page;
    if pointer.is_null() {
        return;
    }
    let page = &mut *pointer;
    let mut client = std::mem::zeroed();
    GetClientRect(hwnd, &mut client);
    let cloudflare = SendMessageW(GetDlgItem(hwnd, PROVIDER as i32), CB_GETCURSEL, 0, 0) == 1;
    let shift = if cloudflare { 0 } else { 44 };
    let height = 1000 - shift;
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
    let x = ((client.right - 820) / 2).max(24);
    for item in &page.items {
        let visible = item.group == 0 || item.group == if cloudflare { CLOUDFLARE } else { CUSTOM };
        ShowWindow(item.hwnd, if visible { SW_SHOW } else { SW_HIDE });
        if visible {
            let r = item.rect;
            let dy = if r.top >= 816 { shift } else { 0 };
            SetWindowPos(
                item.hwnd,
                null_mut(),
                x + r.left,
                r.top - page.scroll - dy,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    InvalidateRect(hwnd, null(), 1);
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
unsafe fn check(hwnd: HWND, id: usize, value: bool) {
    SendMessageW(
        GetDlgItem(hwnd, id as i32),
        BM_SETCHECK,
        usize::from(value),
        0,
    );
}
unsafe fn checked(hwnd: HWND, id: usize) -> bool {
    SendMessageW(GetDlgItem(hwnd, id as i32), BM_GETCHECK, 0, 0) == 1
}
unsafe fn update_enabled(hwnd: HWND) {
    let turn = !checked(hwnd, STUN_ONLY);
    let cloudflare = SendMessageW(GetDlgItem(hwnd, PROVIDER as i32), CB_GETCURSEL, 0, 0) == 1;
    for id in [
        PROVIDER,
        TURN,
        USERNAME,
        PASSWORD,
        FORGET_PASSWORD,
        KEY,
        TOKEN,
        FORGET_TOKEN,
        TTL,
        CACHE,
    ] {
        let enabled = turn
            && match id {
                TURN | USERNAME | PASSWORD | FORGET_PASSWORD => !cloudflare,
                KEY | TOKEN | FORGET_TOKEN | TTL | CACHE => cloudflare,
                _ => true,
            };
        EnableWindow(GetDlgItem(hwnd, id as i32), enabled as i32);
    }
    layout(hwnd);
}
unsafe fn save(hwnd: HWND, page: &mut Page) -> anyhow::Result<()> {
    let mut s = page.saved.clone();
    let urls = |id| {
        text(hwnd, id)
            .lines()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(|u| {
                if let Some((scheme, rest)) = u.split_once(':') {
                    format!("{}:{rest}", scheme.to_ascii_lowercase())
                } else {
                    u.into()
                }
            })
            .collect()
    };
    s.provider = if SendMessageW(GetDlgItem(hwnd, PROVIDER as i32), CB_GETCURSEL, 0, 0) == 1 {
        "cloudflare"
    } else {
        "custom"
    }
    .into();
    s.stun_urls = urls(STUN);
    s.turn_urls = urls(TURN);
    s.stun_only = checked(hwnd, STUN_ONLY);
    s.custom_username = text(hwnd, USERNAME).trim().into();
    s.turn_key_id = text(hwnd, KEY).trim().into();
    s.ttl = text(hwnd, TTL)
        .parse()
        .map_err(|_| anyhow::anyhow!("Enter a valid credential lifetime"))?;
    s.cache_credentials = checked(hwnd, CACHE);
    page.manager.save(
        s,
        &text(hwnd, PASSWORD),
        &text(hwnd, TOKEN),
        checked(hwnd, FORGET_PASSWORD),
        checked(hwnd, FORGET_TOKEN),
    )?;
    page.saved = page.manager.view().0;
    for id in [PASSWORD, TOKEN] {
        SetWindowTextW(GetDlgItem(hwnd, id as i32), wide("").as_ptr());
    }
    check(hwnd, FORGET_PASSWORD, false);
    check(hwnd, FORGET_TOKEN, false);
    Ok(())
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
                page.scroll -= ((wp >> 16) as u16 as i16 as i32) * 72 / 120;
                layout(hwnd);
                return 0;
            }
            WM_VSCROLL => {
                let mut info = SCROLLINFO {
                    cbSize: size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_ALL,
                    ..std::mem::zeroed()
                };
                GetScrollInfo(hwnd, SB_VERT, &mut info);
                page.scroll = match (wp & 0xffff) as i32 {
                    SB_LINEUP => page.scroll - 36,
                    SB_LINEDOWN => page.scroll + 36,
                    SB_PAGEUP => page.scroll - info.nPage as i32,
                    SB_PAGEDOWN => page.scroll + info.nPage as i32,
                    SB_THUMBTRACK | SB_THUMBPOSITION => info.nTrackPos,
                    SB_TOP => 0,
                    SB_BOTTOM => info.nMax,
                    _ => page.scroll,
                };
                layout(hwnd);
                return 0;
            }
            WM_MEASUREITEM if wp == PROVIDER => {
                (*(lp as *mut MEASUREITEMSTRUCT)).itemHeight = 30;
                return 1;
            }
            WM_DRAWITEM => {
                let draw = &*(lp as *const DRAWITEMSTRUCT);
                if [SAVE as u32, BACK as u32, PROVIDER as u32].contains(&draw.CtlID) {
                    let dc = draw.hDC;
                    let state = SaveDC(dc);
                    let primary = draw.CtlID == SAVE as u32;
                    let color = if draw.itemState & ODS_SELECTED != 0 {
                        0x00404040
                    } else if primary {
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
                        } else if primary && draw.itemState & ODS_SELECTED == 0 {
                            FIELD
                        } else {
                            FG
                        },
                    );
                    let label = if draw.CtlID == PROVIDER as u32 {
                        match draw.itemID {
                            0 => "Custom servers",
                            1 => "Cloudflare TURN",
                            _ => "",
                        }
                    } else if primary {
                        "Save settings"
                    } else {
                        "Back"
                    };
                    let mut rect = draw.rcItem;
                    rect.left += 10;
                    rect.right -= 10;
                    DrawTextW(
                        dc,
                        wide(label).as_ptr(),
                        -1,
                        &mut rect,
                        DT_SINGLELINE
                            | DT_VCENTER
                            | if draw.CtlID == PROVIDER as u32 {
                                DT_LEFT
                            } else {
                                DT_CENTER
                            },
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
                    page.scroll += rect.top - 8;
                } else if rect.bottom > client.bottom - 8 {
                    page.scroll += rect.bottom - client.bottom + 8;
                }
                layout(hwnd);
                return 0;
            }
            WM_COMMAND => match wp & 0xffff {
                SAVE | 1 => {
                    let status = match save(hwnd, page) {
                        Ok(()) => "Settings saved. They apply on your next connection.".into(),
                        Err(e) => e.to_string(),
                    };
                    SetWindowTextW(GetDlgItem(hwnd, STATUS as i32), wide(&status).as_ptr());
                    return 0;
                }
                BACK => {
                    PostMessageW(GetParent(hwnd), WM_APP + 9, 0, 0);
                    return 0;
                }
                PROVIDER if (wp >> 16) as u32 == CBN_SELCHANGE => {
                    update_enabled(hwnd);
                    return 0;
                }
                STUN_ONLY if (wp >> 16) as u32 == BN_CLICKED => {
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
                    if id == TAB {
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
                let mut rect: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut rect);
                let x = ((rect.right - 820) / 2).max(24);
                let line = RECT {
                    left: x,
                    top: 127 - page.scroll,
                    right: x + 820,
                    bottom: 129 - page.scroll,
                };
                SetDCBrushColor(dc, 0x003b3b3b);
                FillRect(dc, &line, GetStockObject(DC_BRUSH) as HBRUSH);
                EndPaint(hwnd, &paint);
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
    fn native_controls_save_and_reopen_without_guest_or_real_profile() {
        let mut nonce = [0; 16];
        getrandom::fill(&mut nonce).unwrap();
        let suffix: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
        let directory = std::env::temp_dir().join(format!("parsec-settings-ui-{suffix}"));
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
            assert_eq!(GetMenuItemCount(GetMenu(parent)), 1);
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
            capture(page, "cloudflare-bottom");
            SendMessageW(GetDlgItem(page, PROVIDER as i32), CB_SETCURSEL, 0, 0);
            update_enabled(page);
            assert_eq!(text(page, USERNAME), "unsaved-user");
            SetWindowTextW(
                GetDlgItem(page, STUN as i32),
                wide("stun:stun.example.org:3478").as_ptr(),
            );
            check(page, STUN_ONLY, true);
            update_enabled(page);
            assert_eq!(IsWindowEnabled(GetDlgItem(page, TURN as i32)), 0);
            SendMessageW(page, WM_COMMAND, SAVE, 0);
            assert!(text(page, STATUS).starts_with("Settings saved"));
            DestroyWindow(page);
            let page = open(parent, manager.clone());
            assert_eq!(text(page, STUN), "stun:stun.example.org:3478");
            assert!(checked(page, STUN_ONLY));
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

    // Optional CI-only visual fixtures. This cannot capture an account, stream
    // or production profile: the fixture above owns only synthetic controls.
    unsafe fn capture(hwnd: HWND, name: &str) {
        let Some(directory) = std::env::var_os("PARSEC_SETTINGS_SCREENSHOTS") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        // Some standard controls skip painting while an ancestor is hidden.
        // Show only this isolated fixture, without activating another account UI.
        ShowWindow(GetParent(hwnd), SW_SHOWNOACTIVATE);
        RedrawWindow(
            GetParent(hwnd),
            null(),
            null_mut(),
            RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
        );
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
        SendMessageW(
            hwnd,
            WM_PRINT,
            dc as usize,
            (PRF_CLIENT | PRF_NONCLIENT | PRF_CHILDREN | PRF_ERASEBKGND) as isize,
        );
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
        image::save_buffer(
            directory.join(format!("{name}.png")),
            &rgba,
            rect.right as u32,
            rect.bottom as u32,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
}
