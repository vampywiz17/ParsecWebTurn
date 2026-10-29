//! Native account and computer picker; all control access stays on the UI thread.
#[cfg(test)]
use super::account::Host;
use super::account::{self, Login, Session};
use super::controls::{self, BG, FG, FIELD};
pub(super) use super::controls::{font, wide};
use super::picker::{self, Picker, Refresh};
use super::sign_in::{Completion, SignIn};
use anyhow::{ensure, Result};
use std::{
    ptr::{null, null_mut},
    sync::Arc,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{
        LibraryLoader::*,
        SystemServices::{SS_CENTERIMAGE, SS_OWNERDRAW},
    },
    UI::{Controls::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};
const LOGIN: usize = 1;
const REFRESH: usize = 2;
const CONNECT: usize = 3;
const LOGOUT: usize = 4;
const LIST: usize = 5;
const REMEMBER: usize = 6;
const SETTINGS: usize = 7;
const DONE: usize = 0x6e01;
const AUTO_REFRESH: usize = 0x6e02;
const WORDMARK_BLUE: usize = 0x6e03;
const WORDMARK_ICON: usize = 8;
#[cfg(test)]
thread_local! {static UI_FIXTURE:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};}

pub struct Selection {
    pub session: Arc<Session>,
    pub peer: String,
}
struct Form {
    settings_page: HWND,
    info_pages: Arc<crate::updater_ui::Pages>,
    manager: Arc<crate::connection_settings::Manager>,
    fields: [HWND; 3],
    remember: HWND,
    list: HWND,
    peer: HWND,
    status: HWND,
    common_controls: Vec<HWND>,
    login_controls: Vec<HWND>,
    picker_controls: Vec<HWND>,
    font: HFONT,
    heading: HFONT,
    brush: HBRUSH,
    field_brush: HBRUSH,
    sign_in: SignIn,
    picker: Picker,
    done: bool,
    result: Option<Selection>,
}
impl Drop for Form {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.font);
            DeleteObject(self.heading);
            DeleteObject(self.brush);
            DeleteObject(self.field_brush);
        }
    }
}
unsafe fn text(hwnd: HWND) -> String {
    controls::text(hwnd, 4096)
}
unsafe fn add(
    parent: HWND,
    f: &Form,
    class: &str,
    label: &str,
    id: usize,
    style: u32,
    r: [i32; 4],
) -> HWND {
    let owner_draw = class == "BUTTON" && style & 0xf != BS_AUTOCHECKBOX as u32;
    let style = style | if owner_draw { BS_OWNERDRAW as u32 } else { 0 };
    controls::create(parent, f.font, class, label, id, style, r)
}
unsafe fn status(f: &Form, s: &str) {
    SetWindowTextW(f.status, wide(s).as_ptr());
}
unsafe fn view(f: &Form) {
    let launcher = f.settings_page.is_null();
    let login = f.sign_in.session.is_none();
    for &h in &f.common_controls {
        ShowWindow(h, if launcher { SW_SHOW } else { SW_HIDE });
    }
    for &h in &f.login_controls {
        ShowWindow(h, if launcher && login { SW_SHOW } else { SW_HIDE });
    }
    for &h in &f.picker_controls {
        ShowWindow(h, if launcher && !login { SW_SHOW } else { SW_HIDE });
    }
    let busy = f.sign_in.busy() || f.picker.blocks_controls();
    if f.sign_in.session.is_some() {
        SetTimer(
            GetParent(f.status),
            AUTO_REFRESH,
            picker::AUTO_REFRESH_MS,
            None,
        );
    } else {
        KillTimer(GetParent(f.status), AUTO_REFRESH);
    }
    for h in f.fields.into_iter().chain([f.remember]) {
        EnableWindow(h, i32::from(!busy));
    }
    for id in [LOGIN, REFRESH, CONNECT, LOGOUT, SETTINGS] {
        let h = GetDlgItem(GetParent(f.status), id as i32);
        EnableWindow(h, i32::from(!busy));
    }
}
unsafe fn start_hosts(hwnd: HWND, f: &mut Form, mode: Refresh) {
    if f.sign_in.busy() || f.done {
        return;
    }
    let Some(session) = f.sign_in.session.clone() else {
        return;
    };
    if !f.picker.start(session, mode) {
        return;
    }
    if !mode.quiet() {
        status(f, "Refreshing computers...");
        view(f);
    }
    SetTimer(hwnd, DONE, mode.poll_ms(), None);
}
unsafe fn completed(hwnd: HWND, f: &mut Form, job: Completion) {
    match job {
        Completion::Login(Ok(session)) => {
            f.sign_in.accept(session, f.manager.as_ref(), |notice| {
                message_box(hwnd, notice.text());
            });
            start_hosts(hwnd, f, Refresh::Manual);
        }
        Completion::Login(Err(_)) => {
            crate::event_log::record(
                "ERROR",
                "native_account_login_failed",
                serde_json::json!({}),
            );
            status(f, f.sign_in.message());
            view(f);
            SetFocus(f.fields[1]);
        }
        Completion::Logout => {
            status(f, f.sign_in.message());
            view(f);
            SetFocus(f.fields[0]);
        }
        Completion::Disconnected => {
            status(f, f.sign_in.message());
            view(f);
        }
    }
}
unsafe fn hosts_completed(f: &mut Form, job: picker::Completion) {
    let quiet = matches!(&job, picker::Completion::Hosts(Refresh::Background, _));
    match job {
        picker::Completion::Hosts(mode, Ok(hosts)) => {
            let list = f.list;
            let manager = &f.manager;
            let change = f
                .picker
                .replace_hosts(hosts, mode, || picker::ListPosition {
                    selected: usize::try_from(SendMessageW(list, LB_GETCURSEL, 0, 0)).ok(),
                    last_peer: manager.view().0.native_core.last_peer,
                    top: SendMessageW(list, LB_GETTOPINDEX, 0, 0).max(0) as usize,
                });
            if let Some(change) = change {
                let redraw = GetWindowLongW(f.list, GWL_STYLE) as u32 & WS_VISIBLE != 0;
                if redraw {
                    SendMessageW(f.list, WM_SETREDRAW, 0, 0);
                }
                SendMessageW(f.list, LB_RESETCONTENT, 0, 0);
                for h in f.picker.hosts() {
                    let name = format!("{}   /   {}", h.name, h.peer);
                    SendMessageW(f.list, LB_ADDSTRING, 0, wide(&name).as_ptr() as isize);
                }
                if let Some(selected) = change.selected {
                    SendMessageW(f.list, LB_SETCURSEL, selected, 0);
                    SendMessageW(f.list, LB_SETTOPINDEX, change.top, 0);
                }
                if let Some(peer) = change.peer {
                    SetWindowTextW(f.peer, wide(&peer).as_ptr());
                }
                if redraw {
                    SendMessageW(f.list, WM_SETREDRAW, 1, 0);
                    RedrawWindow(f.list, null(), null_mut(), RDW_INVALIDATE | RDW_ERASE);
                }
            }
            if !quiet {
                status(
                    f,
                    if f.picker.hosts().is_empty() {
                        "No computers listed. Refresh, or enter the host peer ID."
                    } else {
                        "Choose a computer, or enter its peer ID. Owner connections are supported."
                    },
                );
                view(f);
                if f.settings_page.is_null() {
                    SetFocus(f.list);
                }
            }
        }
        picker::Completion::Hosts(_, Err(e)) => {
            crate::event_log::record(
                "WARN",
                "native_computer_list_failed",
                serde_json::json!({"session_rejected":e.downcast_ref::<account::ExpiredSession>().is_some()}),
            );
            if e.downcast_ref::<account::ExpiredSession>().is_some() {
                f.sign_in.expire_saved(f.manager.as_ref());
            }
            if !quiet || f.sign_in.session.is_none() {
                status(f, &e.to_string());
                view(f);
            }
        }
        picker::Completion::Disconnected => {
            status(f, "The account request ended unexpectedly. Please retry.");
            view(f);
        }
    }
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            (*(lp as *const CREATESTRUCTW)).lpCreateParams as isize,
        );
    }
    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Form;
    if !p.is_null() {
        let f = &mut *p;
        match msg {
            m if m == WM_APP + 9 => {
                let page = std::mem::replace(&mut f.settings_page, null_mut());
                if !page.is_null() {
                    DestroyWindow(page);
                }
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    0,
                    0,
                    760,
                    610,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
                view(f);
                InvalidateRect(hwnd, null(), 1);
                SetFocus(if f.sign_in.session.is_some() {
                    f.list
                } else {
                    f.fields[0]
                });
                return 0;
            }

            WM_DRAWITEM => {
                if draw_wordmark(lp, f) || draw_button(lp) {
                    return 1;
                }
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN => {
                SetTextColor(
                    wp as HDC,
                    if GetDlgCtrlID(lp as HWND) == WORDMARK_BLUE as i32 {
                        crate::branding::BLUE
                    } else {
                        FG
                    },
                );
                SetBkColor(wp as HDC, if msg == WM_CTLCOLORSTATIC { BG } else { FIELD });
                return if msg == WM_CTLCOLORSTATIC {
                    f.brush
                } else {
                    f.field_brush
                } as isize;
            }
            WM_ERASEBKGND => {
                let mut r = std::mem::zeroed();
                GetClientRect(hwnd, &mut r);
                FillRect(wp as HDC, &r, f.brush);
                return 1;
            }
            WM_TIMER if wp == AUTO_REFRESH => {
                if f.sign_in.session.is_some() && !f.sign_in.busy() && !f.done {
                    start_hosts(hwnd, f, Refresh::Background);
                }
                return 0;
            }
            WM_TIMER if wp == DONE => {
                if let Some(job) = f.picker.poll() {
                    KillTimer(hwnd, DONE);
                    hosts_completed(f, job);
                    return 0;
                }
                if let Some(job) = f.sign_in.poll() {
                    KillTimer(hwnd, DONE);
                    completed(hwnd, f, job);
                }
                return 0;
            }
            WM_COMMAND => {
                let id = wp & 0xffff;
                if id == LIST && (wp >> 16) as u32 == LBN_SELCHANGE {
                    let n = SendMessageW(f.list, LB_GETCURSEL, 0, 0);
                    if let Some(peer) = f.picker.peer_at(usize::try_from(n).ok()) {
                        SetWindowTextW(f.peer, wide(peer).as_ptr());
                    }
                }
                if id == crate::updater_ui::ABOUT || id == crate::updater_ui::OPEN {
                    f.info_pages.open(
                        hwnd,
                        if id == crate::updater_ui::ABOUT {
                            crate::updater::Mode::About
                        } else {
                            crate::updater::Mode::Updates
                        },
                    );
                    return 0;
                }
                if f.sign_in.busy() || f.picker.blocks_controls() {
                    return 0;
                }
                match id {
                    SETTINGS => {
                        if f.settings_page.is_null() {
                            f.settings_page =
                                crate::connection_settings_ui::open(hwnd, f.manager.clone());
                        }
                        if !f.settings_page.is_null() {
                            view(f);
                            SetWindowPos(
                                f.settings_page,
                                HWND_TOP,
                                0,
                                0,
                                0,
                                0,
                                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                            );
                            SetFocus(f.settings_page);
                        }
                    }

                    LOGIN => {
                        let email = text(f.fields[0]);
                        let password = zeroize::Zeroizing::new(text(f.fields[1]));
                        let tfa = zeroize::Zeroizing::new(text(f.fields[2]));
                        let remember =
                            SendMessageW(f.remember, BM_GETCHECK, 0, 0) == BST_CHECKED as isize;
                        let login = Login {
                            email,
                            password,
                            tfa,
                        };
                        match f.sign_in.login(login, remember) {
                            Ok(true) => {}
                            Ok(false) => return 0,
                            Err(error) => {
                                status(f, &error.to_string());
                                return 0;
                            }
                        }
                        SetWindowTextW(f.fields[1], wide("").as_ptr());
                        SetWindowTextW(f.fields[2], wide("").as_ptr());
                        f.picker.cancel();
                        status(f, f.sign_in.message());
                        view(f);
                        SetTimer(hwnd, DONE, 80, None);
                    }
                    REFRESH => start_hosts(hwnd, f, Refresh::Manual),
                    CONNECT => {
                        let peer = match picker::connection_peer(&text(f.peer)) {
                            Ok(peer) => peer,
                            Err(error) => {
                                status(f, &error.to_string());
                                return 0;
                            }
                        };
                        if let Some(session) = f.sign_in.session.take() {
                            if f.manager
                                .update_native(|p| p.last_peer = peer.clone())
                                .is_err()
                            {
                                message_box(hwnd,"Could not save the selected computer. Your connection can still continue.");
                            }
                            f.result = Some(Selection { session, peer });
                            DestroyWindow(hwnd);
                        }
                    }
                    LOGOUT => {
                        if f.sign_in.prepare_logout(f.manager.as_ref()).is_err() {
                            status(f, f.sign_in.message());
                            return 0;
                        }
                        f.picker.cancel();
                        if f.sign_in.logout() {
                            status(f, f.sign_in.message());
                            view(f);
                            SetTimer(hwnd, DONE, 80, None);
                        }
                    }
                    _ => {}
                }
                return 0;
            }
            WM_CLOSE => {
                DestroyWindow(hwnd);
                return 0;
            }
            WM_DESTROY => {
                f.done = true;
                f.info_pages.close();
                f.picker.cancel();
                f.sign_in.cancel();
                KillTimer(hwnd, DONE);
                KillTimer(hwnd, AUTO_REFRESH);
                return 0;
            }
            _ => {}
        }
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}
pub unsafe fn message_box(owner: HWND, message: &str) {
    MessageBoxW(
        owner,
        wide(message).as_ptr(),
        wide("EvoParseq").as_ptr(),
        MB_OK | MB_ICONINFORMATION,
    );
}

pub fn choose(
    manager: Arc<crate::connection_settings::Manager>,
    current: Option<Arc<Session>>,
) -> Result<Option<Selection>> {
    unsafe {
        let profile = manager.view().0.native_core;
        let (saved, notice) = if current.is_some() {
            (None, None)
        } else {
            match profile.session() {
            Ok(s) => (s.map(Arc::new), None),
            Err(_) => (
                None,
                Some("The saved sign-in cannot be decrypted for this Windows user. Sign in again."),
            ),
        }
        };
        let mut f = Box::new(Form {
            settings_page: null_mut(),
            info_pages: Default::default(),
            manager,
            fields: [null_mut(); 3],
            remember: null_mut(),
            list: null_mut(),
            peer: null_mut(),
            status: null_mut(),
            common_controls: vec![],
            login_controls: vec![],
            picker_controls: vec![],
            font: font(17, 400),
            heading: font(32, 600),
            brush: CreateSolidBrush(BG),
            field_brush: CreateSolidBrush(FIELD),
            sign_in: SignIn::new(
                current.or(saved),
                profile.email.clone(),
                !profile.encrypted_session.is_empty(),
            ),
            picker: Picker::default(),
            done: false,
            result: None,
        });
        let name = wide("EvoParseq.NativeCore.Launcher");
        let instance = GetModuleHandleW(null());
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance,
            hIcon: crate::branding::icon(),
            lpszClassName: name.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..std::mem::zeroed()
        });
        let hwnd = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            name.as_ptr(),
            wide("EvoParseq").as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            760,
            610,
            null_mut(),
            null_mut(),
            instance,
            (&mut *f as *mut Form).cast(),
        );
        ensure!(!hwnd.is_null(), "Native account window unavailable");
        let menu = CreateMenu();
        crate::updater_ui::append_help(menu);
        SetMenu(hwnd, menu);
        let mark = add(
            hwnd,
            &f,
            "STATIC",
            "",
            WORDMARK_ICON,
            SS_OWNERDRAW,
            [32, 24, 40, 46],
        );
        f.common_controls.push(mark);
        for (label, id, rect) in [
            ("EVO", 0, [84, 24, 68, 46]),
            ("PARSEQ", WORDMARK_BLUE, [152, 24, 180, 46]),
        ] {
            let heading = add(hwnd, &f, "STATIC", label, id, SS_CENTERIMAGE, rect);
            SendMessageW(heading, WM_SETFONT, f.heading as usize, 0);
            f.common_controls.push(heading);
        }
        let settings = add(
            hwnd,
            &f,
            "BUTTON",
            "Settings",
            SETTINGS,
            WS_TABSTOP,
            [580, 28, 116, 36],
        );
        f.common_controls.push(settings);
        let subtitle = add(
            hwnd,
            &f,
            "STATIC",
            "NATIVE CORE  /  Your computers, your connection",
            0,
            0,
            [32, 80, 650, 26],
        );
        f.common_controls.push(subtitle);
        for (i, label) in ["Email", "Password", "Two-factor code (optional)"]
            .iter()
            .enumerate()
        {
            let y = 136 + i as i32 * 60;
            let l = add(hwnd, &f, "STATIC", label, 0, 0, [32, y, 215, 26]);
            let edit = add(
                hwnd,
                &f,
                "EDIT",
                "",
                10 + i,
                WS_TABSTOP
                    | WS_BORDER
                    | ES_AUTOHSCROLL as u32
                    | if i == 1 { ES_PASSWORD as u32 } else { 0 },
                [250, y - 2, 446, 32],
            );
            SendMessageW(edit, EM_SETLIMITTEXT, 4096, 0);
            f.fields[i] = edit;
            f.login_controls.extend([l, edit]);
        }
        SetWindowTextW(f.fields[0], wide(&profile.email).as_ptr());
        f.remember = add(
            hwnd,
            &f,
            "BUTTON",
            "Remember sign-in for this Windows user",
            REMEMBER,
            BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
            [250, 322, 450, 30],
        );
        SendMessageW(
            f.remember,
            BM_SETCHECK,
            usize::from(f.sign_in.remember_login),
            0,
        );
        f.login_controls.push(f.remember);
        let b = add(
            hwnd,
            &f,
            "BUTTON",
            "Sign in",
            LOGIN,
            WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
            [490, 370, 206, 40],
        );
        f.login_controls.push(b);
        let list = add(
            hwnd,
            &f,
            "LISTBOX",
            "",
            LIST,
            WS_TABSTOP | WS_BORDER | WS_VSCROLL | LBS_NOTIFY as u32,
            [32, 130, 664, 220],
        );
        f.list = list;
        f.picker_controls.push(list);
        let l = add(hwnd, &f, "STATIC", "Host peer ID", 0, 0, [32, 370, 155, 26]);
        f.peer = add(
            hwnd,
            &f,
            "EDIT",
            &profile.last_peer,
            20,
            WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL as u32,
            [200, 366, 496, 32],
        );
        SendMessageW(f.peer, EM_SETLIMITTEXT, 256, 0);
        f.picker_controls.extend([l, f.peer]);
        for (id, label, x, w) in [
            (REFRESH, "Refresh", 32, 140),
            (LOGOUT, "Sign out", 184, 140),
            (CONNECT, "Connect", 490, 206),
        ] {
            let b = add(hwnd, &f, "BUTTON", label, id, WS_TABSTOP, [x, 420, w, 40]);
            f.picker_controls.push(b);
        }
        f.status = add(
            hwnd,
            &f,
            "STATIC",
            notice.unwrap_or(
                "Use your Parsec account. Passwords and two-factor codes are never saved.",
            ),
            0,
            0,
            [32, 484, 664, 66],
        );
        f.common_controls.push(f.status);
        view(&f);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        if f.sign_in.session.is_some() {
            start_hosts(hwnd, &mut f, Refresh::Manual);
        } else {
            SetFocus(f.fields[0]);
        }
        #[cfg(test)]
        if UI_FIXTURE.with(|f| f.get()) {
            if let Some(path) = std::env::var_os("EVOPARSEQ_UI_CAPTURE") {
                capture_fixture(
                    hwnd,
                    &std::path::PathBuf::from(&path).with_file_name("sign-in.png"),
                )
                .unwrap();
            }
            for show_picker in [false, true] {
                if show_picker {
                    // A local fixture token never enters auth, host discovery,
                    // persistence or explicit logout requests.
                    f.sign_in.session =
                        Some(Arc::new(Session::from_token("offline-ui-fixture").unwrap()));
                    hosts_completed(
                        &mut f,
                        picker::Completion::Hosts(
                            Refresh::Manual,
                            Ok(vec![Host {
                                peer: "fixture-host".into(),
                                name: "Fixture computer".into(),
                            }]),
                        ),
                    );
                }
                let selected = text(f.peer);
                SendMessageW(hwnd, WM_COMMAND, SETTINGS, 0);
                assert!(!f.settings_page.is_null());
                assert_ne!(IsWindowVisible(f.settings_page), 0);
                if show_picker {
                    SetWindowTextW(f.peer, wide("manually-entered-host").as_ptr());
                    hosts_completed(
                        &mut f,
                        picker::Completion::Hosts(
                            Refresh::Background,
                            Ok(vec![Host {
                                peer: "fixture-host".into(),
                                name: "Updated computer".into(),
                            }]),
                        ),
                    );
                    assert_eq!(text(f.peer), "manually-entered-host");
                    SendMessageW(f.list, LB_SETITEMDATA, 0, 0x6e11);
                    hosts_completed(
                        &mut f,
                        picker::Completion::Hosts(
                            Refresh::Background,
                            Ok(vec![Host {
                                peer: "fixture-host".into(),
                                name: "Updated computer".into(),
                            }]),
                        ),
                    );
                    assert_eq!(text(f.peer), "manually-entered-host");
                    assert_eq!(
                        SendMessageW(f.list, LB_GETITEMDATA, 0, 0),
                        0x6e11,
                        "An unchanged background reply must not rebuild list controls"
                    );
                    SetWindowTextW(f.peer, wide(&selected).as_ptr());
                }
                // Refreshing the launcher state must not resurrect its controls
                // while Settings owns the view.
                view(&f);
                for &control in f
                    .common_controls
                    .iter()
                    .chain(&f.login_controls)
                    .chain(&f.picker_controls)
                {
                    assert_eq!(
                        IsWindowVisible(control),
                        0,
                        "Launcher control covers Settings"
                    );
                }
                if let Some(path) = std::env::var_os("EVOPARSEQ_UI_CAPTURE") {
                    capture_fixture(
                        hwnd,
                        &std::path::PathBuf::from(&path).with_file_name(if show_picker {
                            "picker-settings.png"
                        } else {
                            "sign-in-settings.png"
                        }),
                    )
                    .unwrap();
                    if show_picker {
                        SendMessageW(f.settings_page, WM_COMMAND, 4121, 0);
                        assert!(!GetDlgItem(f.settings_page, 4135).is_null());
                        SendMessageW(f.settings_page, WM_VSCROLL, SB_BOTTOM as usize, 0);
                        capture_fixture(
                            hwnd,
                            &std::path::PathBuf::from(&path)
                                .with_file_name("native-video-settings.png"),
                        )
                        .unwrap();
                    }
                }
                SendMessageW(hwnd, WM_APP + 9, 0, 0);
                assert!(f.settings_page.is_null());
                for &control in &f.common_controls {
                    assert_ne!(IsWindowVisible(control), 0);
                }
                for &control in &f.login_controls {
                    assert_eq!(IsWindowVisible(control) != 0, !show_picker);
                }
                for &control in &f.picker_controls {
                    assert_eq!(IsWindowVisible(control) != 0, show_picker);
                }
                assert_eq!(text(f.peer), selected);
                if let Some(path) = std::env::var_os("EVOPARSEQ_UI_CAPTURE") {
                    let path = std::path::PathBuf::from(path);
                    let restored = path.with_file_name(if show_picker {
                        "picker-restored.png"
                    } else {
                        "sign-in-restored.png"
                    });
                    capture_fixture(hwnd, &restored).unwrap();
                    let initial = image::open(path.with_file_name("sign-in.png"))
                        .unwrap()
                        .to_rgb8();
                    let after = image::open(restored).unwrap().to_rgb8();
                    // WM_PRINT's child rendering includes the native frame
                    // offset. Use actual child/window geometry, not client
                    // layout constants, for this offline bitmap comparison.
                    let mut bounds = RECT::default();
                    let mut parent = RECT::default();
                    GetWindowRect(GetDlgItem(hwnd, WORDMARK_ICON as i32), &mut bounds);
                    GetWindowRect(hwnd, &mut parent);
                    let left = (bounds.left - parent.left) as u32;
                    let top = (bounds.top - parent.top) as u32;
                    for y in top..top + 46 {
                        for x in left..left + 40 {
                            assert_eq!(
                                initial.get_pixel(x, y),
                                after.get_pixel(x, y),
                                "Wordmark changed after Settings at {x},{y}"
                            );
                            if y < top + 3 || y >= top + 43 {
                                assert_eq!(
                                    after.get_pixel(x, y).0,
                                    [240, 243, 245],
                                    "Missing accent bar at {x},{y}"
                                );
                            }
                        }
                    }
                }
            }
            f.sign_in.session = None; // Do not run explicit account logout for the fixture.
            DestroyWindow(hwnd);
        }
        let mut msg: MSG = std::mem::zeroed();
        while !f.done && GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if f.info_pages.dialog(&msg) {
                continue;
            }
            if !f.settings_page.is_null() && IsDialogMessageW(f.settings_page, &msg) != 0 {
                continue;
            }
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if IsWindow(hwnd) != 0 {
            DestroyWindow(hwnd);
        }
        f.sign_in
            .close(|| !f.manager.view().0.native_core.encrypted_session.is_empty());
        Ok(f.result.take())
    }
}

/// Paint every pixel, including icon padding. SS_ICON's centered padding
/// could retain a light/default brush after the Settings hide/show cycle.
unsafe fn draw_wordmark(lp: LPARAM, f: &Form) -> bool {
    if lp == 0 {
        return false;
    }
    let item = &*(lp as *const DRAWITEMSTRUCT);
    if item.CtlType != ODT_STATIC || item.CtlID != WORDMARK_ICON as u32 {
        return false;
    }
    FillRect(item.hDC, &item.rcItem, f.brush);
    let bounds = item.rcItem;
    DrawIconEx(
        item.hDC,
        bounds.left,
        bounds.top + 3,
        crate::branding::launcher_icon(),
        40,
        40,
        0,
        null_mut(),
        DI_NORMAL,
    );
    // Keep the two liked accent bars as intentional decoration, not leftovers.
    let brush = GetStockObject(DC_BRUSH) as HBRUSH;
    let previous = SetDCBrushColor(item.hDC, FG);
    let mut bar = bounds;
    bar.bottom = bar.top + 3;
    FillRect(item.hDC, &bar, brush);
    bar = bounds;
    bar.top = bar.bottom - 3;
    FillRect(item.hDC, &bar, brush);
    SetDCBrushColor(item.hDC, previous);
    true
}

pub(crate) unsafe fn draw_button(lp: LPARAM) -> bool {
    if lp == 0 {
        return false;
    }
    let item = &*(lp as *const DRAWITEMSTRUCT);
    if item.CtlType != ODT_BUTTON {
        return false;
    }
    let selected = item.itemState & ODS_SELECTED != 0;
    let disabled = item.itemState & ODS_DISABLED != 0;
    let bg = CreateSolidBrush(if selected { 0x00674c32 } else { FIELD });
    FillRect(item.hDC, &item.rcItem, bg);
    DeleteObject(bg);
    let edge = CreateSolidBrush(0x00e7bd63);
    FrameRect(item.hDC, &item.rcItem, edge);
    DeleteObject(edge);
    SetBkMode(item.hDC, TRANSPARENT as i32);
    SetTextColor(item.hDC, if disabled { 0x00888888 } else { FG });
    let mut r = item.rcItem;
    if item.CtlID == super::session_ui::OPEN as u32 {
        DrawIconEx(
            item.hDC,
            r.left + (r.right - r.left - 32) / 2,
            r.top + (r.bottom - r.top - 32) / 2,
            crate::branding::icon(),
            32,
            32,
            0,
            null_mut(),
            DI_NORMAL,
        );
    } else {
        let mut label = wide(&text(item.hwndItem));
        DrawTextW(
            item.hDC,
            label.as_mut_ptr(),
            -1,
            &mut r,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
    }
    if item.itemState & ODS_FOCUS != 0 {
        r.left += 3;
        r.top += 3;
        r.right -= 3;
        r.bottom -= 3;
        DrawFocusRect(item.hDC, &r);
    }
    true
}

/// Offline GDI fixture capture only. Never called by a live video path.
#[cfg(test)]
pub(crate) unsafe fn capture_fixture(hwnd: HWND, path: &std::path::Path) -> Result<()> {
    let mut r = std::mem::zeroed();
    GetClientRect(hwnd, &mut r);
    let (w, h) = (r.right as u32, r.bottom as u32);
    ensure!(
        w > 0 && h > 0 && w <= 2048 && h <= 2048,
        "Invalid UI fixture size"
    );
    let dc = CreateCompatibleDC(null_mut());
    ensure!(!dc.is_null(), "UI fixture DC unavailable");
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        },
        ..std::mem::zeroed()
    };
    let mut bits = null_mut();
    let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    if bitmap.is_null() {
        DeleteDC(dc);
        anyhow::bail!("UI fixture bitmap unavailable");
    }
    let old = SelectObject(dc, bitmap);
    SendMessageW(
        hwnd,
        WM_PRINT,
        dc as usize,
        (PRF_CLIENT | PRF_CHILDREN | PRF_ERASEBKGND) as isize,
    );
    GdiFlush();
    let mut bytes = std::slice::from_raw_parts(bits.cast::<u8>(), (w * h * 4) as usize).to_vec();
    SelectObject(dc, old);
    DeleteObject(bitmap);
    DeleteDC(dc);
    for p in bytes.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
        p[3] = 255;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    image::RgbaImage::from_raw(w, h, bytes)
        .unwrap()
        .save(path)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launcher_can_render_without_credentials_or_external_requests() {
        let path = std::env::temp_dir().join(format!(
            "evoparseq-launcher-fixture-{}.json",
            crate::bud_attempt::native_attempt_id().unwrap()
        ));
        let manager = crate::connection_settings::Manager::open(path);
        UI_FIXTURE.with(|f| f.set(true));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                UI_FIXTURE.with(|f| f.set(false));
            }
        }
        let _reset = Reset;
        assert!(choose(manager, None).unwrap().is_none());
    }
}
