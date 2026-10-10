//! Documented Win32 subclassing keeps native editing and combo keyboard behavior.
use super::{wide, FG, FIELD};
use std::ptr::{null, null_mut};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::*,
    UI::{Controls::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
};
const BAR: i32 = 16;
const SUBCLASS: usize = 1;

pub unsafe fn combo(hwnd: HWND) {
    SetWindowSubclass(hwnd, Some(combo_proc), SUBCLASS, 0);
}
unsafe fn combo_paint(hwnd: HWND, dc: HDC) {
    let mut info: COMBOBOXINFO = std::mem::zeroed();
    info.cbSize = std::mem::size_of::<COMBOBOXINFO>() as u32;
    if GetComboBoxInfo(hwnd, &mut info) == 0 {
        return;
    }
    let mut rect: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut rect);
    let brush = CreateSolidBrush(FIELD);
    FillRect(dc, &info.rcButton, brush);
    DeleteObject(brush);
    let edge = CreateSolidBrush(0x00444444);
    FrameRect(dc, &rect, edge);
    DeleteObject(edge);
    let x = (info.rcButton.left + info.rcButton.right) / 2;
    let y = (info.rcButton.top + info.rcButton.bottom) / 2;
    let points = [
        POINT { x: x - 5, y: y - 2 },
        POINT { x: x + 5, y: y - 2 },
        POINT { x, y: y + 4 },
    ];
    let brush = CreateSolidBrush(if IsWindowEnabled(hwnd) != 0 {
        FG
    } else {
        0x00777777
    });
    let old_brush = SelectObject(dc, brush);
    let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
    Polygon(dc, points.as_ptr(), 3);
    SelectObject(dc, old_pen);
    SelectObject(dc, old_brush);
    DeleteObject(brush);
}
unsafe extern "system" fn combo_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    _: usize,
) -> LRESULT {
    let result = DefSubclassProc(hwnd, msg, wp, lp);
    match msg {
        WM_NCDESTROY => {
            RemoveWindowSubclass(hwnd, Some(combo_proc), id);
        }
        WM_PRINT | WM_PRINTCLIENT => combo_paint(hwnd, wp as HDC),
        WM_PAINT | WM_ENABLE | WM_SETFOCUS | WM_KILLFOCUS | WM_LBUTTONDOWN | WM_LBUTTONUP
        | WM_MOUSEMOVE | WM_CAPTURECHANGED | CB_SETCURSEL => {
            let dc = GetDC(hwnd);
            combo_paint(hwnd, dc);
            ReleaseDC(hwnd, dc);
        }
        _ => {}
    }
    result
}

pub unsafe fn multiline(edit: HWND) {
    let name = wide("ParsecWebTurnTextScroll");
    let instance = GetModuleHandleW(null());
    let class = WNDCLASSW {
        lpfnWndProc: Some(scroll_proc),
        hInstance: instance,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        cbWndExtra: 2 * std::mem::size_of::<isize>() as i32,
        lpszClassName: name.as_ptr(),
        ..std::mem::zeroed()
    };
    RegisterClassW(&class);
    let bar = CreateWindowExW(
        0,
        name.as_ptr(),
        wide("").as_ptr(),
        WS_CHILD | WS_VISIBLE,
        0,
        0,
        BAR,
        1,
        edit,
        null_mut(),
        instance,
        null(),
    );
    SetWindowSubclass(edit, Some(edit_proc), SUBCLASS, bar as usize);
    size_editor(edit, bar);
}
unsafe fn size_editor(edit: HWND, bar: HWND) {
    let mut rect: RECT = std::mem::zeroed();
    GetClientRect(edit, &mut rect);
    SetWindowPos(
        bar,
        null_mut(),
        rect.right - BAR,
        0,
        BAR,
        rect.bottom,
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
    rect.left = 4;
    rect.top = 3;
    rect.right -= BAR + 4;
    rect.bottom -= 3;
    SendMessageW(edit, EM_SETRECTNP, 0, &rect as *const RECT as isize);
}
unsafe extern "system" fn edit_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    let result = DefSubclassProc(hwnd, msg, wp, lp);
    let bar = data as HWND;
    match msg {
        WM_NCDESTROY => {
            RemoveWindowSubclass(hwnd, Some(edit_proc), id);
        }
        WM_SIZE => size_editor(hwnd, bar),
        WM_ENABLE => {
            EnableWindow(bar, IsWindowEnabled(hwnd));
        }
        _ => {}
    }
    if matches!(
        msg,
        WM_SIZE
            | WM_SETTEXT
            | WM_KEYDOWN
            | WM_CHAR
            | WM_PASTE
            | WM_CUT
            | WM_UNDO
            | WM_MOUSEWHEEL
            | WM_VSCROLL
            | EM_LINESCROLL
            | EM_SCROLLCARET
            | WM_LBUTTONUP
            | WM_MOUSEMOVE
    ) {
        InvalidateRect(bar, null(), 0);
    }
    result
}
unsafe fn state(edit: HWND) -> (i32, i32, i32) {
    let mut rect: RECT = std::mem::zeroed();
    SendMessageW(edit, EM_GETRECT, 0, &mut rect as *mut RECT as isize);
    let dc = GetDC(edit);
    let font = SendMessageW(edit, WM_GETFONT, 0, 0) as HFONT;
    let old = SelectObject(dc, font);
    let mut tm = std::mem::zeroed();
    GetTextMetricsW(dc, &mut tm);
    SelectObject(dc, old);
    ReleaseDC(edit, dc);
    let visible = ((rect.bottom - rect.top) / tm.tmHeight.max(1)).max(1);
    let count = SendMessageW(edit, EM_GETLINECOUNT, 0, 0) as i32;
    (
        SendMessageW(edit, EM_GETFIRSTVISIBLELINE, 0, 0) as i32,
        (count - visible).max(0),
        visible,
    )
}
unsafe fn thumb(hwnd: HWND, first: i32, max: i32, visible: i32) -> RECT {
    let mut rect: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut rect);
    let height = (rect.bottom * visible / (max + visible))
        .max(18)
        .min(rect.bottom);
    let top = if max > 0 {
        (rect.bottom - height) * first.min(max) / max
    } else {
        0
    };
    RECT {
        left: 5,
        right: BAR - 5,
        top: top + 2,
        bottom: (top + height - 2).max(top + 2),
    }
}
unsafe extern "system" fn scroll_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let edit = GetParent(hwnd);
    match msg {
        WM_ERASEBKGND => return 1,
        WM_PAINT | WM_PRINTCLIENT => {
            let mut ps = std::mem::zeroed();
            let dc = if msg == WM_PAINT {
                BeginPaint(hwnd, &mut ps)
            } else {
                wp as HDC
            };
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rect);
            let brush = CreateSolidBrush(FIELD);
            FillRect(dc, &rect, brush);
            DeleteObject(brush);
            let (first, max, visible) = state(edit);
            if max > 0 {
                let brush = CreateSolidBrush(if GetCapture() == hwnd {
                    0x00999999
                } else {
                    0x00666666
                });
                FillRect(dc, &thumb(hwnd, first, max, visible), brush);
                DeleteObject(brush);
            }
            if msg == WM_PAINT {
                EndPaint(hwnd, &ps);
            }
            return 0;
        }
        WM_LBUTTONDOWN => {
            SetFocus(edit);
            let y = (lp >> 16) as i16 as i32;
            let (first, max, visible) = state(edit);
            let t = thumb(hwnd, first, max, visible);
            if y >= t.top && y < t.bottom {
                SetWindowLongPtrW(hwnd, 0, y as isize);
                SetWindowLongPtrW(hwnd, std::mem::size_of::<isize>() as i32, first as isize);
                SetCapture(hwnd);
            } else {
                let delta = if y < t.top { -visible } else { visible };
                SendMessageW(edit, EM_LINESCROLL, 0, delta as isize);
            }
            InvalidateRect(hwnd, null(), 0);
            return 0;
        }
        WM_MOUSEMOVE if GetCapture() == hwnd => {
            let (first, max, visible) = state(edit);
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rect);
            let travel = (rect.bottom - (rect.bottom * visible / (max + visible)).max(18)).max(1);
            let delta = (lp >> 16) as i16 as i32 - GetWindowLongPtrW(hwnd, 0) as i32;
            let target = (GetWindowLongPtrW(hwnd, std::mem::size_of::<isize>() as i32) as i32
                + delta * max / travel)
                .clamp(0, max);
            SendMessageW(edit, EM_LINESCROLL, 0, (target - first) as isize);
            return 0;
        }
        WM_LBUTTONUP => {
            if GetCapture() == hwnd {
                ReleaseCapture();
            }
            InvalidateRect(hwnd, null(), 0);
            return 0;
        }
        WM_MOUSEWHEEL => {
            SendMessageW(edit, msg, wp, lp);
            return 0;
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

#[cfg(test)]
pub unsafe fn verify_multiline(edit: HWND) {
    let value = (0..20)
        .map(|n| format!("stun:server{n}.example:3478"))
        .collect::<Vec<_>>()
        .join("\r\n");
    SetWindowTextW(edit, wide(&value).as_ptr());
    let bar = GetWindow(edit, GW_CHILD);
    assert!(!bar.is_null());
    assert_eq!(GetWindowLongW(edit, GWL_STYLE) as u32 & WS_VSCROLL, 0);
    SendMessageW(edit, EM_LINESCROLL, 0, -100);
    assert_eq!(state(edit).0, 0);
    SendMessageW(bar, WM_LBUTTONDOWN, 0, (65isize << 16) | 8);
    SendMessageW(bar, WM_LBUTTONUP, 0, 0);
    assert!(
        state(edit).0 > 0,
        "Dark scrollbar must scroll the native editor"
    );
    SendMessageW(edit, EM_LINESCROLL, 0, -100);
    assert_eq!(state(edit).0, 0);
    assert_eq!(SendMessageW(edit, EM_GETLINECOUNT, 0, 0), 20);
    SetWindowTextW(edit, wide("").as_ptr());
}
