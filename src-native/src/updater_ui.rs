//! Native modeless update window. Network workers never access HWNDs.
use crate::updater::{self, Mode, Phase, State, View};
use std::{
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{LibraryLoader::*, SystemServices::SS_ICON},
    UI::{
        Controls::{DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS},
        Input::KeyboardAndMouse::*,
        Shell::ShellExecuteW,
        WindowsAndMessaging::*,
    },
};
pub const ABOUT: usize = 4300;
pub const OPEN: usize = 4302;
#[derive(Default)]
pub struct Pages {
    about: AtomicUsize,
    updates: AtomicUsize,
    state: Arc<State>,
}
impl Pages {
    fn handle(&self, mode: Mode) -> &AtomicUsize {
        if mode == Mode::About {
            &self.about
        } else {
            &self.updates
        }
    }
    pub unsafe fn open(self: &Arc<Self>, parent: HWND, mode: Mode) -> HWND {
        let existing = self.handle(mode).load(Ordering::Acquire) as HWND;
        if !existing.is_null() && IsWindow(existing) != 0 {
            ShowWindow(existing, SW_RESTORE);
            SetForegroundWindow(existing);
            return existing;
        }
        let hwnd = open_page(parent, self.clone(), mode);
        if !hwnd.is_null() && mode == Mode::Updates {
            self.state.check();
        }
        hwnd
    }
    pub unsafe fn close(&self) {
        for slot in [&self.about, &self.updates] {
            let hwnd = slot.swap(0, Ordering::AcqRel) as HWND;
            if !hwnd.is_null() && IsWindow(hwnd) != 0 {
                DestroyWindow(hwnd);
            }
        }
    }
    pub unsafe fn dialog(&self, msg: &MSG) -> bool {
        [&self.about, &self.updates].into_iter().any(|slot| {
            let hwnd = slot.load(Ordering::Acquire) as HWND;
            !hwnd.is_null()
                && (msg.hwnd == hwnd || IsChild(hwnd, msg.hwnd) != 0)
                && IsDialogMessageW(hwnd, msg) != 0
        })
    }
}
pub unsafe fn append_help(menu: HMENU) {
    let help = CreatePopupMenu();
    AppendMenuW(help, MF_STRING, ABOUT, wide("About EvoParseq").as_ptr());
    AppendMenuW(help, MF_STRING, OPEN, wide("Check for updates").as_ptr());
    AppendMenuW(menu, MF_POPUP, help as usize, wide("Help").as_ptr());
}
const CHECK: usize = 4400;
const DOWNLOAD: usize = 4401;
const CLOSE: usize = 4402;
const STATUS: usize = 4403;
const NOTES: usize = 4404;
const VERSION: usize = 4405;
const BG: u32 = crate::branding::BACKGROUND;
const FIELD: u32 = 0x212121;
const FG: u32 = 0xeeeeee;
const CYAN: u32 = crate::branding::BLUE;
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
struct Page {
    owned: bool,
    pages: Arc<Pages>,
    mode: Mode,
    last: Option<View>,
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
unsafe fn font(size: i32) -> HFONT {
    CreateFontW(
        -size,
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
    )
}
unsafe fn add(parent: HWND, class: &str, title: &str, id: usize, style: u32, font: HFONT) -> HWND {
    let hwnd = CreateWindowExW(
        0,
        wide(class).as_ptr(),
        wide(title).as_ptr(),
        WS_CHILD | WS_VISIBLE | style,
        0,
        0,
        1,
        1,
        parent,
        id as HMENU,
        GetModuleHandleW(null()),
        null(),
    );
    SendMessageW(hwnd, WM_SETFONT, font as usize, 0);
    hwnd
}
unsafe fn open_page(parent: HWND, pages: Arc<Pages>, mode: Mode) -> HWND {
    let about = mode == Mode::About;
    let class = wide("EvoParseqInformation");
    let instance = GetModuleHandleW(null());
    RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(proc),
        hInstance: instance,
        hIcon: LoadIconW(instance, std::ptr::without_provenance(1)),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: class.as_ptr(),
        ..std::mem::zeroed()
    });
    let mut page = Box::new(Page {
        owned: false,
        pages,
        mode,
        last: None,
        background: CreateSolidBrush(BG),
        field: CreateSolidBrush(FIELD),
        font: font(16),
        heading: font(32),
    });
    let ptr = (&mut *page) as *mut Page;
    let hwnd = CreateWindowExW(
        WS_EX_CONTROLPARENT,
        class.as_ptr(),
        wide(mode.title()).as_ptr(),
        WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        740,
        if about { 640 } else { 720 },
        parent,
        null_mut(),
        instance,
        ptr.cast(),
    );
    if hwnd.is_null() {
        return hwnd;
    }
    page.owned = true;
    let page = &mut *Box::into_raw(page);
    page.pages
        .handle(mode)
        .store(hwnd as usize, Ordering::Release);
    add(hwnd, "STATIC", mode.heading(), 1, 0, page.heading);
    let mark = add(hwnd, "STATIC", "", 6, SS_ICON, page.font);
    SendMessageW(
        mark,
        STM_SETICON,
        crate::branding::launcher_icon() as usize,
        0,
    );
    add(hwnd, "STATIC", "", VERSION, 0, page.font);
    add(hwnd, "STATIC", "", STATUS, 0, page.font);
    let notes = add(
        hwnd,
        "EDIT",
        "",
        NOTES,
        ES_MULTILINE as u32
            | ES_READONLY as u32
            | ES_AUTOVSCROLL as u32
            | WS_VSCROLL
            | WS_TABSTOP
            | WS_CLIPCHILDREN,
        page.font,
    );
    crate::connection_settings_ui::controls::multiline(notes);
    for (id, label) in [
        (CHECK, if about { "Project page" } else { "Check again" }),
        (DOWNLOAD, "Download ZIP"),
        (CLOSE, "Close"),
    ] {
        add(
            hwnd,
            "BUTTON",
            label,
            id,
            BS_OWNERDRAW as u32 | WS_TABSTOP,
            page.font,
        );
    }
    if about {
        ShowWindow(GetDlgItem(hwnd, DOWNLOAD as i32), SW_HIDE);
    }
    refresh(hwnd, page);
    layout(hwnd);
    if !about {
        SetTimer(hwnd, 1, 500, None);
    }
    ShowWindow(hwnd, SW_SHOW);
    SetFocus(GetDlgItem(hwnd, CHECK as i32));
    hwnd
}
unsafe fn layout(hwnd: HWND) {
    let mut rect: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut rect);
    let width = (rect.right - 48).max(1);
    let bottom = rect.bottom - 62;
    for (id, x, y, w, h) in [
        (6, 24, 25, 40, 40),
        (1, 80, 20, width - 56, 48),
        (VERSION, 24, 76, width, 42),
        (STATUS, 24, 126, width, 78),
        (NOTES, 24, 214, width, (bottom - 234).max(1)),
        (CHECK, 24, bottom, 142, 36),
        (DOWNLOAD, 178, bottom, 150, 36),
        (CLOSE, rect.right - 132, bottom, 108, 36),
    ] {
        SetWindowPos(
            GetDlgItem(hwnd, id as i32),
            null_mut(),
            x,
            y,
            w,
            h,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
}
unsafe fn refresh(hwnd: HWND, page: &mut Page) {
    let view = page.pages.state.view();
    if page.last.as_ref() == Some(&view) {
        return;
    }
    let content = updater::content(page.mode, &view);
    for (id, text) in [
        (VERSION, content.version),
        (STATUS, content.status),
        (NOTES, content.notes),
    ] {
        SetWindowTextW(GetDlgItem(hwnd, id as i32), wide(&text).as_ptr());
    }
    EnableWindow(
        GetDlgItem(hwnd, CHECK as i32),
        i32::from(page.mode == Mode::About || view.phase != Phase::Checking),
    );
    ShowWindow(
        GetDlgItem(hwnd, DOWNLOAD as i32),
        if page.mode == Mode::Updates && view.phase == Phase::Available {
            SW_SHOW
        } else {
            SW_HIDE
        },
    );
    page.last = Some(view);
    InvalidateRect(hwnd, null(), 1);
}
unsafe fn browser(owner: HWND, url: &str) -> bool {
    if !updater::allowed_url(url) {
        return false;
    }
    (ShellExecuteW(
        owner,
        wide("open").as_ptr(),
        wide(url).as_ptr(),
        null(),
        null(),
        SW_SHOWNORMAL,
    ) as isize)
        > 32
}
unsafe fn draw_button(draw: &DRAWITEMSTRUCT, page: &Page) {
    let saved = SaveDC(draw.hDC);
    let active = draw.CtlID == DOWNLOAD as u32 && draw.itemState & ODS_DISABLED == 0;
    let brush = CreateSolidBrush(if active { CYAN } else { FIELD });
    FillRect(draw.hDC, &draw.rcItem, brush);
    DeleteObject(brush);
    SelectObject(draw.hDC, page.font);
    SetBkMode(draw.hDC, TRANSPARENT as i32);
    SetTextColor(
        draw.hDC,
        if draw.itemState & ODS_DISABLED != 0 {
            0x777777
        } else if active {
            BG
        } else {
            FG
        },
    );
    let mut text = [0u16; 64];
    GetWindowTextW(draw.hwndItem, text.as_mut_ptr(), 64);
    let mut rect = draw.rcItem;
    DrawTextW(
        draw.hDC,
        text.as_ptr(),
        -1,
        &mut rect,
        DT_SINGLELINE | DT_VCENTER | DT_CENTER,
    );
    if draw.itemState & ODS_FOCUS != 0 {
        InflateRect(&mut rect, -3, -3);
        DrawFocusRect(draw.hDC, &rect);
    }
    RestoreDC(draw.hDC, saved);
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            (*(lp as *const CREATESTRUCTW)).lpCreateParams as isize,
        );
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Page;
    if !ptr.is_null() {
        let page = &mut *ptr;
        match msg {
            WM_TIMER => {
                if IsIconic(hwnd) == 0 {
                    refresh(hwnd, page);
                }
                return 0;
            }
            WM_SIZE => {
                layout(hwnd);
                return 0;
            }
            WM_GETMINMAXINFO => {
                let info = &mut *(lp as *mut MINMAXINFO);
                info.ptMinTrackSize = POINT { x: 560, y: 500 };
                return 0;
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
                let dc = wp as HDC;
                SetTextColor(dc, FG);
                let notes = lp as HWND == GetDlgItem(hwnd, NOTES as i32);
                SetBkColor(dc, if notes { FIELD } else { BG });
                return if notes { page.field } else { page.background } as isize;
            }
            WM_ERASEBKGND => {
                let mut rect = std::mem::zeroed();
                GetClientRect(hwnd, &mut rect);
                FillRect(wp as HDC, &rect, page.background);
                return 1;
            }
            WM_DRAWITEM => {
                draw_button(&*(lp as *const DRAWITEMSTRUCT), page);
                return 1;
            }
            WM_COMMAND => match wp & 0xffff {
                CHECK => {
                    if page.mode == Mode::About {
                        if !browser(hwnd, updater::REPOSITORY) {
                            open_failed(hwnd);
                        }
                    } else {
                        page.pages.state.check();
                        refresh(hwnd, page);
                    }
                    return 0;
                }
                DOWNLOAD => {
                    if page.mode == Mode::Updates {
                        if let Some(release) = page.pages.state.view().release {
                            if !browser(hwnd, &release.download_url) {
                                open_failed(hwnd);
                            }
                        }
                    }
                    return 0;
                }
                CLOSE | 2 => {
                    DestroyWindow(hwnd);
                    return 0;
                }
                _ => {}
            },
            WM_NCDESTROY => {
                KillTimer(hwnd, 1);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let _ = page.pages.handle(page.mode).compare_exchange(
                    hwnd as usize,
                    0,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                if page.owned {
                    drop(Box::from_raw(ptr));
                }
            }
            _ => {}
        }
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

unsafe fn open_failed(hwnd: HWND) {
    MessageBoxW(hwnd, wide("Cannot open the project or download in your browser. Your organization may restrict it.").as_ptr(),
        wide("EvoParseq").as_ptr(), MB_OK | MB_ICONINFORMATION);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_information_pages_render_resize_and_release_ownership_without_network() {
        unsafe {
            let pages = Arc::new(Pages::default());
            let parent = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                wide("Offline information fixture").as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                800,
                700,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            assert!(!parent.is_null());
            for mode in [Mode::About, Mode::Updates] {
                // Same production constructor, without Pages::open's explicit network check.
                let hwnd = open_page(parent, pages.clone(), mode);
                assert!(!hwnd.is_null());
                assert_eq!(GetWindow(hwnd, GW_OWNER), parent);
                assert_eq!(pages.state.view().phase, Phase::Idle);
                assert_eq!(pages.handle(mode).load(Ordering::Acquire), hwnd as usize);
                if mode == Mode::About {
                    assert_eq!(pages.open(parent, mode), hwnd);
                }
                for (w, h, suffix) in [(740, 720, ""), (560, 500, "-small")] {
                    SetWindowPos(hwnd, null_mut(), 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER);
                    let mut notes: RECT = std::mem::zeroed();
                    let mut close: RECT = std::mem::zeroed();
                    GetWindowRect(GetDlgItem(hwnd, NOTES as i32), &mut notes);
                    GetWindowRect(GetDlgItem(hwnd, CLOSE as i32), &mut close);
                    assert!(notes.bottom < close.top);
                    assert!(notes.right > notes.left && notes.bottom > notes.top);
                    if let Some(path) = std::env::var_os("EVOPARSEQ_UI_CAPTURE") {
                        let path = std::path::PathBuf::from(path).with_file_name(format!(
                            "{}{}.png",
                            if mode == Mode::About {
                                "about"
                            } else {
                                "updates"
                            },
                            suffix
                        ));
                        crate::native_core::ui::capture_fixture(hwnd, &path).unwrap();
                    }
                }
                SendMessageW(hwnd, WM_COMMAND, CLOSE, 0);
                assert_eq!(IsWindow(hwnd), 0);
                assert_eq!(pages.handle(mode).load(Ordering::Acquire), 0);
            }
            let hwnd = open_page(parent, pages.clone(), Mode::About);
            pages.close();
            assert_eq!(IsWindow(hwnd), 0);
            DestroyWindow(parent);
        }
    }
}
