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
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
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
const BG: u32 = 0x00212121;
const FIELD: u32 = 0x00333333;
const FG: u32 = 0x00eeeeee;
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

struct Page {
    window_owned: bool,
    manager: Arc<Manager>,
    saved: Settings,
    background: HBRUSH,
    field: HBRUSH,
    font: HFONT,
    heading: HFONT,
}
impl Drop for Page {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.background);
            DeleteObject(self.field);
            DeleteObject(self.font);
            DeleteObject(self.heading);
        }
    }
}

/// Use standard menus/accessibility and a stable keyboard shortcut, without
/// modifying the Parsec WASM's UI or intercepting its private menu callbacks.
pub unsafe fn install_menu(parent: HWND) {
    if !GetMenu(parent).is_null() {
        return;
    }
    let menu = CreateMenu();
    let app = CreatePopupMenu();
    AppendMenuW(
        app,
        MF_STRING,
        OPEN,
        wide("Connection settings\tCtrl+,").as_ptr(),
    );
    AppendMenuW(menu, MF_POPUP, app as usize, wide("ParsecWebTurn").as_ptr());
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
    // A class can remain registered between page openings.
    RegisterClassW(&class);
    let (saved, error) = manager.view();
    let mut page = Box::new(Page {
        window_owned: false,
        manager,
        saved,
        background: CreateSolidBrush(BG),
        field: CreateSolidBrush(FIELD),
        font: CreateFontW(
            -16,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            CLEARTYPE_QUALITY as u32,
            0,
            wide("Segoe UI").as_ptr(),
        ),
        heading: CreateFontW(
            -26,
            0,
            0,
            0,
            600,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            CLEARTYPE_QUALITY as u32,
            0,
            wide("Segoe UI").as_ptr(),
        ),
    });
    // Keep ownership until CreateWindowEx succeeds: WM_NCDESTROY may also run
    // during failed creation and must not free the creator's Box a second time.
    let pointer = (&mut *page) as *mut Page;
    let mut rect: RECT = std::mem::zeroed();
    GetClientRect(parent, &mut rect);
    if rect.right < 860 || rect.bottom < 650 {
        SetWindowPos(
            parent,
            null_mut(),
            0,
            0,
            920,
            750,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        GetClientRect(parent, &mut rect);
    }
    let hwnd = CreateWindowExW(
        WS_EX_CONTROLPARENT,
        name.as_ptr(),
        wide("Connection settings").as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
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
    let pointer = Box::into_raw(page);
    let page = &*pointer;
    let s = &page.saved;
    let x = (rect.right - 820).max(24) / 2;
    let right = x + 420;
    control(
        hwnd,
        "STATIC",
        "Connection settings",
        0,
        x,
        22,
        800,
        38,
        0,
        page.heading,
    );
    control(
        hwnd,
        "STATIC",
        "Choose how this client finds and reaches your computer.",
        0,
        x,
        66,
        800,
        24,
        0,
        page.font,
    );
    control(
        hwnd,
        "STATIC",
        "STUN discovery",
        0,
        x,
        108,
        380,
        24,
        0,
        page.font,
    );
    control(
        hwnd,
        "EDIT",
        &s.stun_urls.join("\r\n"),
        ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | WS_VSCROLL | WS_BORDER | WS_TABSTOP,
        x,
        138,
        380,
        84,
        STUN,
        page.font,
    );
    control(
        hwnd,
        "STATIC",
        "One stun: URL per line. Blank uses Parsec's default.",
        0,
        x,
        230,
        390,
        38,
        0,
        page.font,
    );
    control(
        hwnd,
        "BUTTON",
        "STUN only (LAN / VPN; no TURN relay)",
        BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
        x,
        280,
        395,
        30,
        STUN_ONLY,
        page.font,
    );
    check(hwnd, STUN_ONLY, s.stun_only);
    control(
        hwnd,
        "STATIC",
        "TURN provider",
        0,
        right,
        108,
        380,
        24,
        0,
        page.font,
    );
    let provider = control(
        hwnd,
        "COMBOBOX",
        "",
        CBS_DROPDOWNLIST as u32 | WS_VSCROLL | WS_TABSTOP,
        right,
        138,
        380,
        120,
        PROVIDER,
        page.font,
    );
    SendMessageW(
        provider,
        CB_ADDSTRING,
        0,
        wide("Custom servers (coturn, eturnal, ExpressTURN…)").as_ptr() as isize,
    );
    SendMessageW(
        provider,
        CB_ADDSTRING,
        0,
        wide("Cloudflare TURN").as_ptr() as isize,
    );
    SendMessageW(
        provider,
        CB_SETCURSEL,
        usize::from(s.provider == "cloudflare"),
        0,
    );
    control(
        hwnd,
        "STATIC",
        "TURN URLs — one per line",
        0,
        right,
        184,
        380,
        24,
        0,
        page.font,
    );
    control(
        hwnd,
        "EDIT",
        &s.turn_urls.join("\r\n"),
        ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | WS_VSCROLL | WS_BORDER | WS_TABSTOP,
        right,
        214,
        380,
        72,
        TURN,
        page.font,
    );
    control(
        hwnd,
        "STATIC",
        "UDP, TCP and TLS supported. Example: turns:host:443",
        0,
        right,
        294,
        390,
        38,
        0,
        page.font,
    );
    field(
        hwnd,
        page,
        "TURN username",
        &s.custom_username,
        USERNAME,
        x,
        340,
        false,
    );
    field(
        hwnd,
        page,
        if s.encrypted_custom_password.is_empty() {
            "TURN password"
        } else {
            "TURN password (blank keeps saved password)"
        },
        "",
        PASSWORD,
        x,
        410,
        true,
    );
    control(
        hwnd,
        "BUTTON",
        "Forget saved TURN password",
        BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
        x,
        480,
        390,
        28,
        FORGET_PASSWORD,
        page.font,
    );
    field(
        hwnd,
        page,
        "Cloudflare TURN Key ID",
        &s.turn_key_id,
        KEY,
        right,
        340,
        false,
    );
    field(
        hwnd,
        page,
        if s.encrypted_api_token.is_empty() {
            "Cloudflare API token"
        } else {
            "API token (blank keeps saved token)"
        },
        "",
        TOKEN,
        right,
        410,
        true,
    );
    control(
        hwnd,
        "BUTTON",
        "Forget saved API token",
        BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
        right,
        480,
        390,
        28,
        FORGET_TOKEN,
        page.font,
    );
    control(
        hwnd,
        "STATIC",
        "Credential lifetime (seconds)",
        0,
        right,
        518,
        245,
        24,
        0,
        page.font,
    );
    control(
        hwnd,
        "EDIT",
        &s.ttl.to_string(),
        ES_NUMBER as u32 | WS_BORDER | WS_TABSTOP,
        right + 265,
        514,
        115,
        30,
        TTL,
        page.font,
    );
    control(
        hwnd,
        "BUTTON",
        "Reuse unexpired credentials while the app is open",
        BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
        right,
        554,
        395,
        28,
        CACHE,
        page.font,
    );
    check(hwnd, CACHE, s.cache_credentials);
    control(hwnd, "STATIC", "TURN is optional. ICE chooses the route; enabling TURN may select a relay even when a direct route exists. Changes apply on the next connection.", 0, x, 522, 395, 70, 0, page.font);
    let status = error.unwrap_or_else(|| {
        "Settings are stored in settings.json; secrets are protected for your Windows user.".into()
    });
    control(
        hwnd, "STATIC", &status, 0, x, 602, 540, 46, STATUS, page.font,
    );
    control(
        hwnd,
        "BUTTON",
        "Back",
        BS_PUSHBUTTON as u32 | WS_TABSTOP,
        right + 160,
        608,
        90,
        34,
        BACK,
        page.font,
    );
    control(
        hwnd,
        "BUTTON",
        "Save settings",
        BS_DEFPUSHBUTTON as u32 | WS_TABSTOP,
        right + 260,
        608,
        120,
        34,
        SAVE,
        page.font,
    );
    update_enabled(hwnd);
    SetFocus(GetDlgItem(hwnd, STUN as i32));
    hwnd
}

#[allow(clippy::too_many_arguments)] // Flat Win32 control descriptor, UI-thread only.
unsafe fn field(
    hwnd: HWND,
    page: &Page,
    label: &str,
    value: &str,
    id: usize,
    x: i32,
    y: i32,
    secret: bool,
) {
    control(hwnd, "STATIC", label, 0, x, y, 395, 24, 0, page.font);
    control(
        hwnd,
        "EDIT",
        value,
        ES_AUTOHSCROLL as u32
            | WS_BORDER
            | WS_TABSTOP
            | if secret { ES_PASSWORD as u32 } else { 0 },
        x,
        y + 28,
        380,
        30,
        id,
        page.font,
    );
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
        WS_CHILD | WS_VISIBLE | style,
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
            WM_COMMAND => match wp & 0xffff {
                SAVE => {
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
                PROVIDER | STUN_ONLY => {
                    update_enabled(hwnd);
                    return 0;
                }
                _ => {}
            },
            WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                let dc = wp as HDC;
                let edit = matches!(message, WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX);
                SetTextColor(dc, FG);
                SetBkColor(dc, if edit { FIELD } else { BG });
                return if edit { page.field } else { page.background } as isize;
            }
            WM_ERASEBKGND => {
                let mut rect = std::mem::zeroed();
                GetClientRect(hwnd, &mut rect);
                FillRect(wp as HDC, &rect, page.background);
                return 1;
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
}
