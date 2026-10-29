//! Event-driven native session panel. GDI controls overlay the video child;
//! no video pixels or additional decoded-frame copy. Visible metrics refresh at 1 Hz.
use super::{
    controls::{self, BG, FG, FIELD},
    profile::Video,
    session_model::{
        current_choice, resolution_text, Command, Notification, PanelFields, State, Status, View,
        BANDWIDTH_CHOICES, FPS_CHOICES, HOST_RESOLUTION, RESOLUTION_CHOICES,
    },
    ui::{draw_button, wide},
};
use crate::stats_view::compact_metrics as metrics;
use anyhow::Result;
use std::{
    ptr::{null, null_mut},
    sync::{atomic::Ordering, Arc},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{LibraryLoader::*, SystemServices::SS_OWNERDRAW},
    UI::{Controls::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};
pub const OPEN: usize = 4600;
pub const OPEN_SHORTCUT: usize = 4610;
const APPLY: usize = 4602;
const CLOSE: usize = 4603;
const DISCONNECT: usize = 4604;
const SETTINGS: usize = 4605;
const STATUS: i32 = 4606;
const HIDE_BUTTON: usize = 4607;
const SECURE_ATTENTION: usize = 4608;
const METRICS: i32 = 4609;
const DISPLAY: usize = 4611;
const REFRESH_DISPLAYS: usize = 4612;
const DISPLAY_HINT: i32 = 4613;
const TIMING_HINT: i32 = 4614;
const DISPLAYS: u32 = WM_APP + 28;
const DISPLAY_TIMING_CHANGED: u32 = WM_APP + 29;
const PANEL_HEIGHT: i32 = 920;
pub const RESULT: u32 = WM_APP + 26;
pub const VISIBILITY: u32 = WM_APP + 27;
impl Status {
    // Preserve the existing Win32 message values in the platform adapter.
    fn message(self) -> usize {
        match self {
            Self::Applied => 0,
            Self::Failed => 1,
            Self::Confirmed => 2,
            Self::Waiting => 3,
            Self::Unconfirmed => 4,
            Self::AttentionSent => 5,
            Self::AttentionFailed => 6,
        }
    }
    fn from_message(value: usize) -> Self {
        match value {
            0 => Self::Applied,
            2 => Self::Confirmed,
            3 => Self::Waiting,
            4 => Self::Unconfirmed,
            5 => Self::AttentionSent,
            6 => Self::AttentionFailed,
            _ => Self::Failed,
        }
    }
}
struct Panel {
    window: *const crate::window::Window,
    window_owned: bool,
    scroll: i32,
    controls: [HWND; 6],
    initial_fields: [String; 3],
    display_ids: Vec<String>,
    fullscreen: HWND,
    font: HFONT,
    heading: HFONT,
    caption: HFONT,
    metrics_edge: HBRUSH,
    bg: HBRUSH,
    field: HBRUSH,
    rates: crate::stats::Rates,
    display: super::display_timing::Display,
    monitor: HMONITOR,
    measured_fps: Option<f64>,
}
impl Drop for Panel {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.font);
            DeleteObject(self.heading);
            DeleteObject(self.caption);
            DeleteObject(self.metrics_edge);
            DeleteObject(self.bg);
            DeleteObject(self.field);
        }
    }
}
unsafe fn text(hwnd: HWND) -> String {
    controls::text(hwnd, 256)
}
unsafe fn selected_field(p: &Panel, i: usize) -> String {
    let index = SendMessageW(p.controls[i], CB_GETCURSEL, 0, 0);
    if index == CB_ERR as isize
        || SendMessageW(p.controls[i], CB_GETITEMDATA, index as usize, 0) == 1
    {
        p.initial_fields[i].clone()
    } else {
        text(p.controls[i])
    }
}
unsafe fn collect(p: &Panel) -> Result<Video> {
    let select = |i: usize| SendMessageW(p.controls[i], CB_GETCURSEL, 0, 0);
    PanelFields {
        resolution: &selected_field(p, 0),
        bandwidth: &selected_field(p, 1),
        fps: &selected_field(p, 2),
        constant_fps: select(3) == 1,
        rotated: select(4) == 1,
        immersive: match select(5) {
            1 => 3,
            2 => 2,
            3 => 1,
            _ => 0,
        },
        fullscreen: SendMessageW(p.fullscreen, BM_GETCHECK, 0, 0) == BST_CHECKED as isize,
    }
    .parse()
}
unsafe fn close(hwnd: HWND) {
    let parent = GetParent(hwnd);
    DestroyWindow(hwnd);
    SetFocus(parent);
}
unsafe fn scroll(hwnd: HWND, p: &mut Panel, target: i32) {
    if target == p.scroll {
        return;
    }
    let delta = p.scroll - target;
    p.scroll = target;
    ScrollWindowEx(
        hwnd,
        0,
        delta,
        null(),
        null(),
        null_mut(),
        null_mut(),
        SW_SCROLLCHILDREN | SW_INVALIDATE | SW_ERASE,
    );
    SetScrollPos(hwnd, SB_VERT, target, 1);
}
unsafe fn update_displays(hwnd: HWND, panel: &mut Panel) {
    let window = &*panel.window;
    let displays = window
        .session
        .displays
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let combo = GetDlgItem(hwnd, DISPLAY as i32);
    SendMessageW(combo, WM_SETREDRAW, 0, 0);
    SendMessageW(combo, CB_RESETCONTENT, 0, 0);
    panel.display_ids.clear();
    let mut selected = None;
    for output in &displays.outputs {
        let label = format!(
            "{}{}",
            output.name,
            if output.adapter.is_empty() {
                String::new()
            } else {
                format!(" ({})", output.adapter)
            }
        );
        let index = SendMessageW(combo, CB_ADDSTRING, 0, wide(&label).as_ptr() as isize);
        if displays.current.as_deref() == Some(output.id.as_str()) {
            selected = Some(index);
        }
        panel.display_ids.push(output.id.clone());
    }
    if let Some(index) = selected {
        SendMessageW(combo, CB_SETCURSEL, index as usize, 0);
    } else {
        let label = if displays.loading {
            "Loading displays..."
        } else if displays.outputs.is_empty() {
            "Display list unavailable"
        } else {
            "Select host display"
        };
        let index = SendMessageW(combo, CB_ADDSTRING, 0, wide(label).as_ptr() as isize);
        SendMessageW(combo, CB_SETCURSEL, index as usize, 0);
    }
    let busy = window.session.video_pending.load(Ordering::Acquire);
    EnableWindow(
        combo,
        i32::from(
            !displays.hid_mouse && !displays.loading && !busy && !displays.outputs.is_empty(),
        ),
    );
    EnableWindow(
        GetDlgItem(hwnd, REFRESH_DISPLAYS as i32),
        i32::from(!displays.loading && !busy),
    );
    let hint = if displays.hid_mouse {
        if window.session.hid_primary_ready.load(Ordering::Acquire) {
            "HID mouse: primary host display only. Switching disabled."
        } else {
            "HID mouse paused until the primary host display is verified."
        }
    } else {
        "Switching uses the selected display's host resolution."
    };
    SetWindowTextW(GetDlgItem(hwnd, DISPLAY_HINT), wide(hint).as_ptr());
    SendMessageW(combo, WM_SETREDRAW, 1, 0);
    InvalidateRect(combo, null(), 0);
}
fn post_displays(window: &crate::window::Window, selected: bool) {
    unsafe {
        let hwnd = panel(window);
        if !hwnd.is_null() {
            PostMessageW(hwnd, DISPLAYS, usize::from(selected), 0);
        }
    }
}

unsafe fn update_metrics(hwnd: HWND, panel: &mut Panel) {
    let sample = (&*panel.window)
        .stats
        .read()
        .filter(|s| s.at.elapsed() < std::time::Duration::from_secs(3));
    let values = sample
        .as_ref()
        .map(|s| panel.rates.update(s))
        .unwrap_or_default();
    panel.measured_fps = values.fps;
    SetWindowTextW(
        GetDlgItem(hwnd, METRICS),
        wide(&metrics(sample.as_ref(), &values)).as_ptr(),
    );
    update_timing_hint(hwnd, panel);
}
unsafe fn update_timing_hint(hwnd: HWND, panel: &Panel) {
    let window = &*panel.window;
    let host = *window
        .session
        .host_video
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let requested = selected_field(panel, 2).parse().unwrap_or(60);
    let message = panel.display.advice(
        requested,
        host.fps,
        panel.measured_fps,
        window.presentation_mode() == crate::connection_settings::PresentationMode::Vrr,
    );
    SetWindowTextW(GetDlgItem(hwnd, TIMING_HINT), wide(&message).as_ptr());
}
unsafe fn update_display_cache(hwnd: HWND, panel: &mut Panel, force: bool) {
    if GetDlgItem(hwnd, TIMING_HINT).is_null() {
        return;
    }
    let window = &*panel.window;
    let monitor = MonitorFromWindow(window.handle(), MONITOR_DEFAULTTONEAREST);
    if !force && monitor == panel.monitor {
        return;
    }
    panel.monitor = monitor;
    panel.display = super::display_windows::query(window.handle());
    crate::event_log::record(
        "INFO",
        "client_display_timing",
        serde_json::json!({
            "configured_refresh_hz":panel.display.hz,
            "supported_rates_at_current_resolution":panel.display.supported_hz,
            "vrr_activation_verified":false,"automatic_changes":false
        }),
    );
    update_timing_hint(hwnd, panel);
}
pub fn display_changed(window: &crate::window::Window) {
    unsafe {
        let hwnd = panel(window);
        if !hwnd.is_null() {
            PostMessageW(hwnd, DISPLAY_TIMING_CHANGED, 0, 0);
        }
    }
}
unsafe fn draw_metrics(lp: LPARAM, p: &Panel) -> bool {
    if lp == 0 {
        return false;
    }
    let item = &*(lp as *const DRAWITEMSTRUCT);
    if item.CtlType != ODT_STATIC || item.CtlID != METRICS as u32 {
        return false;
    }
    FillRect(item.hDC, &item.rcItem, p.field);
    FrameRect(item.hDC, &item.rcItem, p.metrics_edge);
    SetBkMode(item.hDC, TRANSPARENT as i32);
    let previous = SelectObject(item.hDC, p.caption);
    SetTextColor(item.hDC, 0x00e7bd63);
    let mut title = wide("LIVE PERFORMANCE");
    let mut bounds = item.rcItem;
    bounds.left += 12;
    bounds.right -= 12;
    bounds.top += 9;
    DrawTextW(
        item.hDC,
        title.as_mut_ptr(),
        -1,
        &mut bounds,
        DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX,
    );
    SelectObject(item.hDC, p.font);
    SetTextColor(item.hDC, FG);
    bounds.top += 22;
    bounds.bottom -= 9;
    let mut values = wide(&text(item.hwndItem));
    DrawTextW(
        item.hDC,
        values.as_mut_ptr(),
        -1,
        &mut bounds,
        DT_LEFT | DT_TOP | DT_NOPREFIX,
    );
    SelectObject(item.hDC, previous);
    true
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            (*(lp as *const CREATESTRUCTW)).lpCreateParams as isize,
        );
    }
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Panel;
    if !pointer.is_null() {
        let p = &mut *pointer;
        match msg {
            WM_WINDOWPOSCHANGED => {
                update_display_cache(hwnd, p, false);
            }
            DISPLAY_TIMING_CHANGED => {
                update_display_cache(hwnd, p, true);
                return 0;
            }
            WM_TIMER if wp == 1 => {
                if IsWindowVisible(hwnd) != 0 {
                    update_metrics(hwnd, p);
                }
                return 0;
            }
            WM_ACTIVATE => {
                PostMessageW(GetParent(hwnd), VISIBILITY, 0, 0);
            }
            WM_SIZE => {
                let mut r = std::mem::zeroed();
                GetClientRect(hwnd, &mut r);
                let max = (PANEL_HEIGHT - r.bottom).max(0);
                let target = p.scroll.min(max);
                scroll(hwnd, p, target);
                let info = SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
                    nMin: 0,
                    nMax: PANEL_HEIGHT - 1,
                    nPage: r.bottom.max(1) as u32,
                    nPos: p.scroll,
                    ..std::mem::zeroed()
                };
                SetScrollInfo(hwnd, SB_VERT, &info, 1);
                return 0;
            }
            WM_VSCROLL => {
                let mut info = SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_ALL,
                    ..std::mem::zeroed()
                };
                GetScrollInfo(hwnd, SB_VERT, &mut info);
                let target = match wp & 0xffff {
                    x if x == SB_LINEUP as usize => p.scroll - 28,
                    x if x == SB_LINEDOWN as usize => p.scroll + 28,
                    x if x == SB_PAGEUP as usize => p.scroll - info.nPage as i32,
                    x if x == SB_PAGEDOWN as usize => p.scroll + info.nPage as i32,
                    x if x == SB_THUMBTRACK as usize => info.nTrackPos,
                    _ => p.scroll,
                };
                scroll(
                    hwnd,
                    p,
                    target.clamp(0, (PANEL_HEIGHT - info.nPage as i32).max(0)),
                );
                return 0;
            }

            WM_DRAWITEM => {
                if draw_metrics(lp, p) || draw_button(lp) {
                    return 1;
                }
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN => {
                SetTextColor(wp as HDC, FG);
                SetBkColor(wp as HDC, if msg == WM_CTLCOLORSTATIC { BG } else { FIELD });
                return if msg == WM_CTLCOLORSTATIC {
                    p.bg
                } else {
                    p.field
                } as isize;
            }
            WM_ERASEBKGND => {
                let mut r = std::mem::zeroed();
                GetClientRect(hwnd, &mut r);
                FillRect(wp as HDC, &r, p.bg);
                return 1;
            }
            DISPLAYS => {
                if wp == 1 {
                    SendMessageW(p.controls[0], CB_SETCURSEL, 0, 0);
                    p.initial_fields[0] = HOST_RESOLUTION.into();
                }
                update_displays(hwnd, p);
                return 0;
            }
            RESULT => {
                let status = Status::from_message(wp);
                EnableWindow(
                    GetDlgItem(hwnd, APPLY as i32),
                    i32::from(
                        status != Status::Waiting
                            && !(&*p.window).session.video_pending.load(Ordering::Acquire),
                    ),
                );
                update_displays(hwnd, p);
                SetWindowTextW(GetDlgItem(hwnd, STATUS), wide(status.text()).as_ptr());
                update_timing_hint(hwnd, p);
                return 0;
            }
            WM_COMMAND => {
                match wp & 0xffff {
                    22 if (wp >> 16) == CBN_SELCHANGE as usize => {
                        update_timing_hint(hwnd, p);
                    }
                    DISPLAY if (wp >> 16) == CBN_SELCHANGE as usize => {
                        let window = &*p.window;
                        let displays = window
                            .session
                            .displays
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        if displays.hid_mouse
                            || displays.loading
                            || window.session.video_pending.load(Ordering::Acquire)
                        {
                            return 0;
                        }
                        let selected =
                            SendMessageW(GetDlgItem(hwnd, DISPLAY as i32), CB_GETCURSEL, 0, 0);
                        if let Some(id) = usize::try_from(selected)
                            .ok()
                            .and_then(|index| p.display_ids.get(index))
                            .cloned()
                        {
                            EnableWindow(GetDlgItem(hwnd, DISPLAY as i32), 0);
                            EnableWindow(GetDlgItem(hwnd, REFRESH_DISPLAYS as i32), 0);
                            window.events.push(crate::window::Event::NativeSession(
                                Command::SelectDisplay(id),
                            ));
                        }
                    }
                    REFRESH_DISPLAYS => {
                        (&*p.window)
                            .events
                            .push(crate::window::Event::NativeSession(
                                Command::RefreshDisplays,
                            ));
                    }
                    APPLY => match collect(p) {
                        Ok(video) => {
                            EnableWindow(GetDlgItem(hwnd, APPLY as i32), 0);
                            (&*p.window)
                                .events
                                .push(crate::window::Event::NativeSession(Command::Apply(video)));
                            SetWindowTextW(GetDlgItem(hwnd, STATUS), wide("Applying...").as_ptr());
                        }
                        Err(e) => super::ui::message_box(hwnd, &e.to_string()),
                    },
                    HIDE_BUTTON => {
                        (&*p.window)
                            .events
                            .push(crate::window::Event::NativeSession(Command::HideButton));
                        close(hwnd);
                    }
                    SECURE_ATTENTION => {
                        (&*p.window)
                            .events
                            .push(crate::window::Event::NativeSession(
                                Command::SecureAttention,
                            ));
                    }
                    CLOSE | 2 => {
                        close(hwnd);
                    }
                    DISCONNECT => {
                        (&*p.window)
                            .events
                            .push(crate::window::Event::NativeSession(Command::Disconnect));
                    }
                    SETTINGS => {
                        let parent = GetParent(hwnd);
                        close(hwnd);
                        PostMessageW(parent, WM_COMMAND, crate::connection_settings_ui::OPEN, 0);
                    }
                    _ => {}
                }
                return 0;
            }
            WM_NCDESTROY => {
                KillTimer(hwnd, 1);
                (&*p.window)
                    .stats
                    .overlay_visible
                    .store(false, Ordering::Release);
                (&*p.window).native_panel.store(0, Ordering::Release);
                #[cfg(test)]
                RemovePropW(hwnd, wide("EvoParseq.SessionPanel.Ready").as_ptr());
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                if p.window_owned {
                    drop(Box::from_raw(pointer));
                }
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            _ => {}
        }
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}
unsafe fn control(
    hwnd: HWND,
    p: &Panel,
    class: &str,
    label: &str,
    id: usize,
    style: u32,
    r: [i32; 4],
) -> HWND {
    controls::create(hwnd, p.font, class, label, id, style, r)
}
unsafe fn combo(hwnd: HWND, p: &Panel, id: usize, y: i32, items: &[&str], value: &str) -> HWND {
    let h = control(
        hwnd,
        p,
        "COMBOBOX",
        "",
        id,
        WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
        [174, y, 218, 260],
    );
    crate::connection_settings_ui::controls::combo(h);
    for item in items {
        SendMessageW(h, CB_ADDSTRING, 0, wide(item).as_ptr() as isize);
    }
    if let Some(index) = items.iter().position(|s| *s == value) {
        SendMessageW(h, CB_SETCURSEL, index, 0);
    } else {
        // Preserve an existing custom value as an explicit keep-current action;
        // the user cannot type a new value or create a new custom preset.
        let index = SendMessageW(
            h,
            CB_ADDSTRING,
            0,
            wide(&current_choice(value)).as_ptr() as isize,
        );
        SendMessageW(h, CB_SETITEMDATA, index as usize, 1);
        SendMessageW(h, CB_SETCURSEL, index as usize, 0);
    }
    h
}
unsafe fn panel(window: &crate::window::Window) -> HWND {
    window.native_panel.load(Ordering::Acquire) as HWND
}
unsafe fn panel_for_parent(parent: HWND) -> HWND {
    let w = GetWindowLongPtrW(parent, GWLP_USERDATA) as *const crate::window::Window;
    if w.is_null() {
        null_mut()
    } else {
        panel(&*w)
    }
}
pub unsafe fn install(window: &crate::window::Window) {
    super::session_badge::install(window);
    raise(window);
}
// Keep the overlays over the stream, but below another active owned window
// such as Connection stats. Switching apps must not raise or hide either overlay.
fn stacking_target(
    parent: HWND,
    button: HWND,
    panel: HWND,
    foreground: HWND,
    root_owner: HWND,
) -> Option<HWND> {
    if foreground.is_null() {
        None
    } else if foreground == parent || foreground == button || foreground == panel {
        Some(HWND_TOP)
    } else if root_owner == parent {
        Some(foreground)
    } else {
        None
    }
}
pub unsafe fn raise(window: &crate::window::Window) {
    let parent = window.handle();
    let foreground = GetForegroundWindow();
    let button = window.native_session_button.load(Ordering::Acquire) as HWND;
    let panel = panel(window);
    let insert_after = stacking_target(
        parent,
        button,
        panel,
        foreground,
        GetAncestor(foreground, GA_ROOTOWNER),
    );
    let visible = !window.settings_open()
        && !window.closing.load(Ordering::Acquire)
        && IsIconic(parent) == 0
        && IsWindowVisible(parent) != 0;
    let mut origin = POINT { x: 16, y: 16 };
    ClientToScreen(parent, &mut origin);
    if !button.is_null() {
        super::session_badge::position(button, insert_after);
        let show = visible && !window.native_session_button_hidden();
        if (IsWindowVisible(button) != 0) != show {
            ShowWindow(button, if show { SW_SHOWNA } else { SW_HIDE });
        }
    }
    window
        .stats
        .overlay_visible
        .store(!panel.is_null() && visible, Ordering::Release);
    if !panel.is_null() {
        let mut r = std::mem::zeroed();
        GetClientRect(parent, &mut r);
        let panel_offset = super::session_badge::SIZE + 10;
        SetWindowPos(
            panel,
            insert_after.unwrap_or(HWND_TOP),
            origin.x,
            origin.y + panel_offset,
            420,
            (r.bottom - panel_offset - 32).clamp(120, PANEL_HEIGHT),
            SWP_NOACTIVATE
                | SWP_NOOWNERZORDER
                | if insert_after.is_some() {
                    0
                } else {
                    SWP_NOZORDER
                },
        );
        if (IsWindowVisible(panel) != 0) != visible {
            ShowWindow(panel, if visible { SW_SHOWNA } else { SW_HIDE });
            if visible {
                SetTimer(panel, 1, 1000, None);
            } else {
                KillTimer(panel, 1);
            }
        }
    }
}
pub unsafe fn toggle(
    window: &crate::window::Window,
    manager: Arc<crate::connection_settings::Manager>,
) {
    let parent = window.handle();
    let existing = panel_for_parent(parent);
    if !existing.is_null() {
        DestroyWindow(existing);
        SetFocus(parent);
        return;
    }
    let video = window
        .session
        .panel_video(manager.view().0.native_core.video);
    let mut panel = Box::new(Panel {
        window,
        window_owned: false,
        scroll: 0,
        controls: [null_mut(); 6],
        initial_fields: [
            resolution_text(video.resolution),
            video.bandwidth_mbps.unwrap().to_string(),
            video.fps.unwrap().to_string(),
        ],
        display_ids: Vec::new(),
        fullscreen: null_mut(),
        font: super::ui::font(16, 400),
        heading: super::ui::font(25, 600),
        caption: super::ui::font(12, 600),
        metrics_edge: CreateSolidBrush(0x0065543f),
        bg: CreateSolidBrush(BG),
        field: CreateSolidBrush(FIELD),
        rates: Default::default(),
        display: Default::default(),
        monitor: null_mut(),
        measured_fps: None,
    });
    let name = wide("EvoParseq.NativeCore.SessionPanel");
    let instance = GetModuleHandleW(null());
    RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(proc),
        hInstance: instance,
        lpszClassName: name.as_ptr(),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        ..std::mem::zeroed()
    });
    let pointer = &mut *panel as *mut Panel;
    let hwnd = CreateWindowExW(
        WS_EX_CONTROLPARENT | WS_EX_LAYERED | WS_EX_TOOLWINDOW,
        name.as_ptr(),
        wide("Session controls").as_ptr(),
        WS_POPUP | WS_CLIPCHILDREN | WS_VSCROLL,
        16,
        60,
        420,
        PANEL_HEIGHT,
        parent,
        null_mut(),
        instance,
        pointer.cast(),
    );
    if hwnd.is_null() {
        return;
    }
    panel.window_owned = true;
    window.native_panel.store(hwnd as usize, Ordering::Release);
    SetLayeredWindowAttributes(hwnd, 0, 238, LWA_ALPHA);
    let p = &mut *Box::into_raw(panel);
    let title = control(hwnd, p, "STATIC", "Session", 0, 0, [24, 18, 350, 36]);
    SendMessageW(title, WM_SETFONT, p.heading as usize, 0);
    control(
        hwnd,
        p,
        "STATIC",
        "Decode CPU N/A | Encode N/A\nRTT N/A | RX N/A | FPS N/A",
        METRICS as usize,
        SS_OWNERDRAW,
        [24, 66, 370, 82],
    );
    control(
        hwnd,
        p,
        "COMBOBOX",
        "",
        DISPLAY,
        WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
        [24, 164, 280, 260],
    );
    crate::connection_settings_ui::controls::combo(GetDlgItem(hwnd, DISPLAY as i32));
    control(
        hwnd,
        p,
        "BUTTON",
        "Refresh",
        REFRESH_DISPLAYS,
        BS_OWNERDRAW as u32 | WS_TABSTOP,
        [314, 164, 80, 28],
    );
    control(
        hwnd,
        p,
        "STATIC",
        "",
        DISPLAY_HINT as usize,
        0,
        [24, 199, 370, 44],
    );
    update_displays(hwnd, p);
    for (i, label) in [
        "Resolution",
        "Bandwidth (Mbps)",
        "Frame rate (FPS)",
        "Frame pacing",
        "Orientation",
        "Immersive mode",
    ]
    .iter()
    .enumerate()
    {
        control(
            hwnd,
            p,
            "STATIC",
            label,
            0,
            0,
            [24, 254 + i as i32 * 44, 150, 24],
        );
    }
    p.controls[0] = combo(hwnd, p, 20, 250, RESOLUTION_CHOICES, &p.initial_fields[0]);
    p.controls[1] = combo(hwnd, p, 21, 294, BANDWIDTH_CHOICES, &p.initial_fields[1]);
    p.controls[2] = combo(hwnd, p, 22, 338, FPS_CHOICES, &p.initial_fields[2]);
    p.controls[3] = combo(
        hwnd,
        p,
        23,
        382,
        &["Adaptive", "Constant FPS"],
        if video.constant_fps == Some(true) {
            "Constant FPS"
        } else {
            "Adaptive"
        },
    );
    p.controls[4] = combo(
        hwnd,
        p,
        24,
        426,
        &["Default", "Rotated"],
        if video.rotated == Some(true) {
            "Rotated"
        } else {
            "Default"
        },
    );
    p.controls[5] = combo(
        hwnd,
        p,
        25,
        470,
        &["Disabled", "Mouse", "Keyboard", "Mouse + keyboard"],
        match video.immersive {
            1 => "Mouse + keyboard",
            2 => "Keyboard",
            3 => "Mouse",
            _ => "Disabled",
        },
    );
    p.fullscreen = control(
        hwnd,
        p,
        "BUTTON",
        "Fullscreen",
        30,
        BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
        [24, 516, 350, 30],
    );
    SendMessageW(
        p.fullscreen,
        BM_SETCHECK,
        usize::from(window.native_fullscreen()),
        0,
    );
    control(
        hwnd,
        p,
        "STATIC",
        "",
        TIMING_HINT as usize,
        0,
        [24, 554, 370, 146],
    );
    update_display_cache(hwnd, p, true);
    control(
        hwnd,
        p,
        "STATIC",
        "",
        STATUS as usize,
        0,
        [24, 710, 370, 40],
    );
    for (id, label, x, y, w) in [
        (SETTINGS, "Settings", 24, 758, 178),
        (APPLY, "Apply", 214, 758, 178),
        (DISCONNECT, "Disconnect", 24, 808, 178),
        (CLOSE, "Close", 214, 808, 178),
        (HIDE_BUTTON, "Hide button", 24, 858, 178),
        (SECURE_ATTENTION, "Send Ctrl+Alt+Del", 214, 858, 178),
    ] {
        control(
            hwnd,
            p,
            "BUTTON",
            label,
            id,
            BS_OWNERDRAW as u32 | WS_TABSTOP,
            [x, y, w, 36],
        );
    }
    ShowWindow(hwnd, SW_SHOW);
    SetTimer(hwnd, 1, 1000, None);
    update_metrics(hwnd, p);
    raise(window);
    SetFocus(p.controls[0]);
    if window.session.video_pending.load(Ordering::Acquire) {
        SendMessageW(hwnd, RESULT, Status::Waiting.message(), 0);
    }
    #[cfg(test)]
    SetPropW(
        hwnd,
        wide("EvoParseq.SessionPanel.Ready").as_ptr(),
        1usize as HANDLE,
    );
}
fn post_status(window: &crate::window::Window, status: Status) {
    unsafe {
        let hwnd = panel(window);
        if !hwnd.is_null() {
            PostMessageW(hwnd, RESULT, status.message(), 0);
        }
    }
}
impl View for crate::window::Window {
    fn session(&self) -> &State {
        &self.session
    }
    fn display_size(&self) -> (u32, u32) {
        self.native_display_size()
    }
    fn notify(&self, notification: Notification) {
        match notification {
            Notification::Status(status) => post_status(self, status),
            Notification::DisplaysChanged => post_displays(self, false),
            Notification::DisplaySelected => post_displays(self, true),
        }
    }
}
pub unsafe fn dialog(parent: HWND, msg: &MSG) -> bool {
    let h = panel_for_parent(parent);
    !h.is_null() && IsDialogMessageW(h, msg) != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    unsafe fn select_text(panel: HWND, id: i32, value: &str) {
        let combo = GetDlgItem(panel, id);
        let index = SendMessageW(
            combo,
            CB_FINDSTRINGEXACT,
            usize::MAX,
            wide(value).as_ptr() as isize,
        );
        assert_ne!(index, CB_ERR as isize);
        SendMessageW(combo, CB_SETCURSEL, index as usize, 0);
    }
    use crate::native_core::profile::Resolution;
    #[test]
    fn typed_statuses_keep_the_existing_window_message_contract() {
        for (code, status) in [
            (0, Status::Applied),
            (1, Status::Failed),
            (2, Status::Confirmed),
            (3, Status::Waiting),
            (4, Status::Unconfirmed),
            (5, Status::AttentionSent),
            (6, Status::AttentionFailed),
        ] {
            assert_eq!(status.message(), code);
            assert_eq!(Status::from_message(code), status);
        }
        assert_eq!(Status::from_message(usize::MAX), Status::Failed);
    }

    #[test]
    fn overlay_stacking_respects_active_stats_and_other_apps() {
        let owner = 1usize as HWND;
        let button = 2usize as HWND;
        let panel = 3usize as HWND;
        let stats = 4usize as HWND;
        let other = 5usize as HWND;
        // Independent stats must follow normal activation, not pin itself
        // above its former owner or re-raise that owner's Session popup.
        assert_eq!(stacking_target(owner, button, panel, stats, stats), None);
        for foreground in [owner, button, panel] {
            assert_eq!(
                stacking_target(owner, button, panel, foreground, owner),
                Some(HWND_TOP)
            );
        }
        assert_eq!(
            stacking_target(owner, button, panel, stats, owner),
            Some(stats)
        );
        assert_eq!(stacking_target(owner, button, panel, other, other), None);
        assert_eq!(
            stacking_target(owner, button, null_mut(), null_mut(), null_mut()),
            None
        );
    }
    #[test]
    fn panel_controls_emit_native_actions_without_network() {
        let window = crate::window::Window::create_native().unwrap();
        struct Close(Arc<crate::window::Window>);
        impl Drop for Close {
            fn drop(&mut self) {
                self.0.request_stop();
                self.0.close();
            }
        }
        let _close = Close(window.clone());
        let path = std::env::temp_dir().join(format!(
            "evoparseq-overlay-fixture-{}.json",
            crate::bud_attempt::native_attempt_id().unwrap()
        ));
        struct Remove(std::path::PathBuf);
        impl Drop for Remove {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _remove = Remove(path.clone());
        let manager = crate::connection_settings::Manager::open(path.clone());
        manager
            .update_native(|profile| profile.video.fps = Some(90))
            .unwrap();
        window.install_connection_settings(manager.clone());
        let button_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let button = loop {
            let button = window.native_session_button.load(Ordering::Acquire) as HWND;
            if !button.is_null() {
                break button;
            }
            assert!(
                std::time::Instant::now() < button_deadline,
                "Session button did not open"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        unsafe {
            let mut prior_events = std::collections::VecDeque::new();
            window.events.take_batch(&mut prior_events);
            let child = GetDlgItem(button, OPEN as i32);
            let mut before = RECT::default();
            GetWindowRect(button, &mut before);
            assert_eq!(
                (before.right - before.left, before.bottom - before.top),
                (48, 48)
            );
            let point = |x: u16, y: u16| (u32::from(x) | (u32::from(y) << 16)) as LPARAM;
            SendMessageW(child, WM_LBUTTONDOWN, 1, point(8, 8));
            SendMessageW(child, WM_MOUSEMOVE, 1, point(108, 68));
            SendMessageW(child, WM_LBUTTONUP, 0, point(8, 8));
            let mut after = RECT::default();
            GetWindowRect(button, &mut after);
            assert_eq!(
                (after.left - before.left, after.top - before.top),
                (100, 60)
            );
            assert!(panel(&window).is_null(), "Dragging must not open the menu");
            let mut drag_events = std::collections::VecDeque::new();
            window.events.take_batch(&mut drag_events);
            assert!(
                !drag_events.iter().any(|event| matches!(
                    event,
                    crate::window::Event::Button(..)
                        | crate::window::Event::Motion(..)
                        | crate::window::Event::RelativeMotion(..)
                )),
                "Badge dragging must stay local"
            );
            if let Some(path) = std::env::var_os("EVOPARSEQ_UI_CAPTURE") {
                let path = std::path::PathBuf::from(path).with_file_name("stream-waiting.png");
                super::super::ui::capture_fixture(window.handle(), &path).unwrap();
                let waiting = image::open(path).unwrap().to_rgb8();
                assert_eq!(
                    waiting
                        .get_pixel(waiting.width() / 2, waiting.height() / 2)
                        .0,
                    [0, 0, 0]
                );
            }
            SendMessageW(button, BM_CLICK, 0, 0);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let panel = loop {
            let panel = unsafe { panel(&window) };
            if !panel.is_null()
                && !unsafe { GetPropW(panel, wide("EvoParseq.SessionPanel.Ready").as_ptr()) }
                    .is_null()
            {
                break panel;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Session panel did not open"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        unsafe {
            assert_eq!(GetWindow(panel, GW_OWNER), window.handle());
            assert_ne!(
                IsWindowVisible(button),
                0,
                "Session button must remain visible independently of foreground focus"
            );
            let mut card: RECT = std::mem::zeroed();
            let mut resolution: RECT = std::mem::zeroed();
            GetWindowRect(GetDlgItem(panel, METRICS), &mut card);
            GetWindowRect(GetDlgItem(panel, 20), &mut resolution);
            assert!(
                resolution.top - card.bottom >= 12,
                "Performance card needs space before the controls"
            );
            for (id, choices) in [
                (20, RESOLUTION_CHOICES),
                (21, BANDWIDTH_CHOICES),
                (22, FPS_CHOICES),
            ] {
                let control = GetDlgItem(panel, id);
                assert_eq!(
                    GetWindowLongW(control, GWL_STYLE) as u32 & 3,
                    CBS_DROPDOWNLIST as u32
                );
                assert_eq!(
                    SendMessageW(control, CB_GETCOUNT, 0, 0),
                    (choices.len() + usize::from(id == 22)) as isize
                );
                for choice in choices {
                    assert_ne!(
                        SendMessageW(
                            control,
                            CB_FINDSTRINGEXACT,
                            usize::MAX,
                            wide(choice).as_ptr() as isize
                        ),
                        CB_ERR as isize
                    );
                }
            }
            // A legacy custom FPS is an explicit preservation action, not a new preset.
            assert_eq!(text(GetDlgItem(panel, 22)), current_choice("90"));
            assert_eq!(
                SendMessageW(
                    GetDlgItem(panel, 22),
                    CB_FINDSTRINGEXACT,
                    usize::MAX,
                    wide("90").as_ptr() as isize
                ),
                CB_ERR as isize
            );
            for id in [21, 22, 23, 24] {
                assert_eq!(
                    SendMessageW(
                        GetDlgItem(panel, id),
                        CB_FINDSTRINGEXACT,
                        usize::MAX,
                        wide("Keep host").as_ptr() as isize
                    ),
                    CB_ERR as isize
                );
            }
            SendMessageW(panel, RESULT, 3, 0);
            assert_eq!(IsWindowEnabled(GetDlgItem(panel, APPLY as i32)), 0);
            SendMessageW(panel, RESULT, 2, 0);
            assert!(
                text(GetDlgItem(panel, STATUS)).is_empty(),
                "Host configuration debug text must not appear in the panel"
            );
            assert_ne!(IsWindowEnabled(GetDlgItem(panel, APPLY as i32)), 0);
            assert_ne!(GetWindowLongPtrW(panel, GWL_STYLE) as u32 & WS_POPUP, 0);
            assert_ne!(
                GetWindowLongPtrW(panel, GWL_EXSTYLE) as u32 & WS_EX_LAYERED,
                0
            );
            assert_eq!(
                GetWindowLongPtrW(panel, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST,
                0
            );
            let mut alpha = 0u8;
            let mut key = 0u32;
            let mut flags = 0u32;
            assert_ne!(
                GetLayeredWindowAttributes(panel, &mut key, &mut alpha, &mut flags),
                0
            );
            assert_eq!(alpha, 238);
            if let Some(path) = std::env::var_os("EVOPARSEQ_UI_CAPTURE") {
                let path = std::path::PathBuf::from(path);
                super::super::ui::capture_fixture(panel, &path).unwrap();
                assert!(!GetDlgItem(button, OPEN as i32).is_null());
                let badge_path = path.with_file_name("session-button.png");
                super::super::ui::capture_fixture(button, &badge_path).unwrap();
                let badge = image::open(badge_path).unwrap().to_rgb8();
                // Check the interior, excluding the accent border: a blank
                // light rectangle cannot satisfy both icon and dark fill.
                let interior: Vec<_> = badge
                    .enumerate_pixels()
                    .filter(|(x, y, _)| *x > 2 && *x < 46 && *y > 2 && *y < 46)
                    .map(|(_, _, p)| p.0)
                    .collect();
                let icon_pixels = interior
                    .iter()
                    .filter(|p| p[2] > 160 && p[1] > 70 && p[2] > p[0].saturating_add(40))
                    .count();
                assert!(
                    icon_pixels > 60,
                    "Session icon was not painted: {icon_pixels} blue pixels"
                );
                assert!(
                    interior
                        .iter()
                        .filter(|p| p.iter().all(|v| *v < 100))
                        .count()
                        > 600,
                    "Session background was not painted"
                );
            }
            {
                let mut displays = window
                    .session
                    .displays
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                displays.outputs = super::super::host_displays::parse(br#"[{"id":"primary-fixture","name":"Display 1"},{"id":"virtual-fixture","name":"Virtual display"}]"#).unwrap();
                displays.current = Some("primary-fixture".into());
                displays.hid_mouse = true;
            }
            SendMessageW(panel, DISPLAYS, 0, 0);
            assert_eq!(IsWindowEnabled(GetDlgItem(panel, DISPLAY as i32)), 0);
            let mut prior = std::collections::VecDeque::new();
            window.events.take_batch(&mut prior);
            // A forged command must not bypass a disabled HID selector.
            SendMessageW(GetDlgItem(panel, DISPLAY as i32), CB_SETCURSEL, 1, 0);
            SendMessageW(
                panel,
                WM_COMMAND,
                DISPLAY | ((CBN_SELCHANGE as usize) << 16),
                0,
            );
            window.events.take_batch(&mut prior);
            assert!(!prior.iter().any(|e| matches!(
                e,
                crate::window::Event::NativeSession(Command::SelectDisplay(_))
            )));
            window
                .session
                .displays
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .hid_mouse = false;
            SendMessageW(panel, DISPLAYS, 0, 0);
            assert_ne!(IsWindowEnabled(GetDlgItem(panel, DISPLAY as i32)), 0);
            SendMessageW(GetDlgItem(panel, DISPLAY as i32), CB_SETCURSEL, 1, 0);
            SendMessageW(
                panel,
                WM_COMMAND,
                DISPLAY | ((CBN_SELCHANGE as usize) << 16),
                0,
            );
            prior.clear();
            window.events.take_batch(&mut prior);
            assert!(prior.iter().any(|e| matches!(e,crate::window::Event::NativeSession(Command::SelectDisplay(id)) if id == "virtual-fixture")));
            select_text(panel, 20, "1280 x 720");
            select_text(panel, 21, "35");
            SendMessageW(panel, DISPLAYS, 1, 0);
            assert_eq!(text(GetDlgItem(panel, 20)), "Keep host");
            assert_eq!(text(GetDlgItem(panel, 21)), "35");
            SendMessageW(panel, WM_COMMAND, APPLY, 0);
        }
        let mut events = std::collections::VecDeque::new();
        events.clear();
        window.events.take_batch(&mut events);
        assert!(events.iter().any(|e|matches!(e,crate::window::Event::NativeSession(Command::Apply(v)) if v.resolution==Resolution::Host && v.immersive==0 && v.fps==Some(90) && v.bandwidth_mbps==Some(35))));
        unsafe {
            select_text(panel, 22, "120");
            SendMessageW(panel, WM_COMMAND, 22 | ((CBN_SELCHANGE as usize) << 16), 0);
            assert!(text(GetDlgItem(panel, TIMING_HINT)).contains("Request: 120 FPS"));
            assert_eq!(
                collect(&*(GetWindowLongPtrW(panel, GWLP_USERDATA) as *const Panel))
                    .unwrap()
                    .fps,
                Some(120)
            );
            let mut hint = RECT::default();
            let mut status = RECT::default();
            let mut apply = RECT::default();
            GetWindowRect(GetDlgItem(panel, TIMING_HINT), &mut hint);
            GetWindowRect(GetDlgItem(panel, STATUS), &mut status);
            GetWindowRect(GetDlgItem(panel, APPLY as i32), &mut apply);
            assert!(
                hint.bottom <= status.top && status.bottom <= apply.top,
                "Timing advice must not overlap status or session actions"
            );
        }
        for (selected, mode) in [(0, 0), (1, 3), (2, 2), (3, 1)] {
            unsafe {
                select_text(panel, 20, "1280 x 720");
                select_text(panel, 21, "15");
                select_text(panel, 22, "60");
                for (id, index) in [(23, 1), (24, 1), (25, selected)] {
                    SendMessageW(GetDlgItem(panel, id), CB_SETCURSEL, index, 0);
                }
                SendMessageW(GetDlgItem(panel, 30), BM_SETCHECK, BST_CHECKED as usize, 0);
                SendMessageW(panel, RESULT, 0, 0);
                SendMessageW(panel, WM_COMMAND, APPLY, 0);
            }
            events.clear();
            window.events.take_batch(&mut events);
            assert!(events.iter().any(|event|matches!(event,crate::window::Event::NativeSession(Command::Apply(v)) if v.resolution==Resolution::Fixed(1280,720) && v.bandwidth_mbps==Some(15) && v.fps==Some(60) && v.constant_fps==Some(true) && v.rotated==Some(true) && v.immersive==mode && v.fullscreen)));
        }
        unsafe {
            SendMessageW(panel, WM_COMMAND, SECURE_ATTENTION, 0);
        }
        events.clear();
        window.events.take_batch(&mut events);
        assert!(events.iter().any(|e| matches!(
            e,
            crate::window::Event::NativeSession(Command::SecureAttention)
        )));
        unsafe {
            SendMessageW(panel, WM_COMMAND, DISCONNECT, 0);
        }
        events.clear();
        window.events.take_batch(&mut events);
        assert!(events.iter().any(|event| matches!(
            event,
            crate::window::Event::NativeSession(Command::Disconnect)
        )));
        unsafe {
            SendMessageW(panel, WM_COMMAND, HIDE_BUTTON, 0);
        }
        assert_eq!(window.native_panel.load(Ordering::Acquire), 0);
        assert!(!window.stats.overlay_visible.load(Ordering::Acquire));
        events.clear();
        window.events.take_batch(&mut events);
        assert!(events
            .iter()
            .any(|e| matches!(e, crate::window::Event::NativeSession(Command::HideButton))));
        manager
            .update_native(|p| p.hide_session_button = true)
            .unwrap();
        window.native_toggle_overlay();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while manager.native_session_button_hidden()
            || window.native_panel.load(Ordering::Acquire) == 0
        {
            assert!(
                std::time::Instant::now() < deadline,
                "Menu shortcut must restore the persisted button preference"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!crate::connection_settings::Manager::open(path).native_session_button_hidden());
    }
}
