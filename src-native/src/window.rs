//! Native Win32 window. Its UI thread blocks in GetMessage; WASM runs separately.
use anyhow::{bail, Result};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Gdi::*, OpenGL::*},
    System::LibraryLoader::*,
    UI::Input::{KeyboardAndMouse::*, *},
    UI::WindowsAndMessaging::*,
};

#[derive(Clone, Debug)]
pub enum Event {
    Size(i32, i32),
    Focus(bool),
    Motion(i32, i32),
    RelativeMotion(i32, i32),
    RelativeMode(bool),
    Button(bool, i32, i32, i32),
    Text(u32),
    Key(bool, &'static str, i32),
    Scroll(i32, i32),
    Fullscreen(bool),
}

pub struct Window {
    pub stats: Arc<crate::stats::Shared>,
    stats_page: AtomicUsize,
    connection_settings: Mutex<Option<Arc<crate::connection_settings::Manager>>>,
    settings_page: AtomicUsize,
    pub hwnd: AtomicUsize,
    pub closing: AtomicBool,
    pub active_contexts: AtomicUsize,
    pub events: Mutex<std::collections::VecDeque<Event>>,
    pub dimensions: Mutex<(i32, i32)>,
    pub overlay: Mutex<crate::overlay::Shared>,
    pub graphics: Mutex<Option<crate::graphics::GraphicsReport>>,
    pub audio_registry: Mutex<crate::audio_windows::Registry>,
    pub video_report: Mutex<Option<crate::video_output::Snapshot>>,
    video_hwnd: AtomicUsize,
    video_create_failed: AtomicBool,
    video_ready: AtomicBool,
    video_visible: AtomicBool,
    #[cfg(any(test, feature = "diagnostics"))]
    pub capture: Mutex<Option<std::path::PathBuf>>,
    #[cfg(any(test, feature = "diagnostics"))]
    pub synthetic_login: bool,
    #[cfg(any(test, feature = "diagnostics"))]
    pub script_steps: AtomicUsize,
    pub run_seconds: u64,
    pub live: bool,
    pub online: bool,
    #[cfg(any(test, feature = "diagnostics"))]
    pub network_origin_audit: bool,
    pub stop: Arc<crate::lifecycle::StopSignal>,
    relative_mouse: AtomicBool,
    relative_pending: AtomicUsize,
    pressed_buttons: AtomicUsize,
    pressed_keys: Mutex<std::collections::BTreeSet<&'static str>>,
    text_decoder: Mutex<crate::input::TextDecoder>,
    cursor_pending: Mutex<crate::cursor::Pending>,
    cursor_posted: AtomicBool,
    cursor: Mutex<crate::cursor::State>,
    fullscreen: Mutex<crate::fullscreen::State>,
    fullscreen_active: AtomicBool,
    fullscreen_pending: AtomicUsize,
    wake_pending: AtomicUsize,
    wake_lock: Mutex<crate::wake_lock::State>,
}

impl Window {
    unsafe fn open_stats(&self) {
        let existing = self.stats_page.load(Ordering::Acquire) as HWND;
        if !existing.is_null() {
            ShowWindow(existing, SW_RESTORE);
            SetForegroundWindow(existing);
            return;
        }
        let page = crate::stats_ui::open(self.handle(), self.stats.clone());
        self.stats_page.store(page as usize, Ordering::Release);
    }
    pub fn create(
        synthetic_login: bool,
        live: bool,
        online: bool,
        network_origin_audit: bool,
    ) -> Result<Arc<Self>> {
        Self::create_mode(synthetic_login, live, online, network_origin_audit, true)
    }

    // The standalone D3D11 test has no OpenGL UI and must not depend on WGL.
    #[cfg(feature = "diagnostics")]
    pub fn create_video_probe() -> Result<Arc<Self>> {
        Self::create_mode(false, true, false, false, false)
    }

    fn create_mode(
        synthetic_login: bool,
        live: bool,
        online: bool,
        _network_origin_audit: bool,
        opengl_required: bool,
    ) -> Result<Arc<Self>> {
        let state = Arc::new(Self {
            stats: Default::default(),
            stats_page: AtomicUsize::new(0),
            connection_settings: Default::default(),
            settings_page: AtomicUsize::new(0),
            hwnd: AtomicUsize::new(0),
            closing: AtomicBool::new(false),
            active_contexts: AtomicUsize::new(0),
            events: Default::default(),
            dimensions: Mutex::new((1024, 720)),
            graphics: Default::default(),
            overlay: Default::default(),
            video_report: Default::default(),
            audio_registry: Default::default(),
            video_hwnd: AtomicUsize::new(0),
            video_create_failed: AtomicBool::new(false),
            video_ready: AtomicBool::new(false),
            video_visible: AtomicBool::new(true),
            #[cfg(any(test, feature = "diagnostics"))]
            capture: Default::default(),
            #[cfg(any(test, feature = "diagnostics"))]
            synthetic_login,
            #[cfg(any(test, feature = "diagnostics"))]
            script_steps: AtomicUsize::new(0),
            run_seconds: if synthetic_login { 20 } else { 8 },
            live,
            online,
            #[cfg(any(test, feature = "diagnostics"))]
            network_origin_audit: _network_origin_audit,
            stop: Default::default(),
            relative_mouse: AtomicBool::new(false),
            relative_pending: AtomicUsize::new(0),
            pressed_buttons: AtomicUsize::new(0),
            pressed_keys: Default::default(),
            text_decoder: Default::default(),
            cursor_pending: Default::default(),
            cursor_posted: AtomicBool::new(false),
            cursor: Default::default(),
            fullscreen: Default::default(),
            fullscreen_active: AtomicBool::new(false),
            fullscreen_pending: AtomicUsize::new(0),
            wake_pending: AtomicUsize::new(0),
            wake_lock: Default::default(),
        });
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let ui = state.clone();
        std::thread::spawn(move || {
            // SAFETY: all window creation/destruction and message dispatch run
            // on this thread. Arc keeps the userdata alive until dispatch ends.
            let outcome = unsafe { create_window(&ui, opengl_required) };
            match outcome {
                Ok(hwnd) => {
                    ui.hwnd.store(hwnd as usize, Ordering::Release);
                    let _ = tx.send(Ok(()));
                    unsafe {
                        let mut msg: MSG = std::mem::zeroed();
                        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                            if msg.message == WM_KEYDOWN
                                && msg.wParam == b'S' as usize
                                && GetKeyState(VK_CONTROL as i32) < 0
                                && GetKeyState(VK_SHIFT as i32) < 0
                            {
                                if msg.lParam & (1 << 30) == 0 {
                                    ui.open_stats();
                                }
                                continue;
                            }
                            let page = ui.settings_page.load(Ordering::Acquire) as HWND;
                            if !page.is_null() && (msg.hwnd == page || IsChild(page, msg.hwnd) != 0)
                            {
                                if msg.message == WM_KEYDOWN && msg.wParam == VK_ESCAPE as usize {
                                    PostMessageW(hwnd, WM_APP + 9, 0, 0);
                                    continue;
                                }
                                if IsDialogMessageW(page, &msg) != 0 {
                                    continue;
                                }
                            }
                            TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                    ui.apply_wake_lock(false, false);
                    ui.hwnd.store(0, Ordering::Release);
                }
                Err(error) => {
                    let _ = tx.send(Err(error));
                }
            }
        });
        rx.recv()??;
        Ok(state)
    }

    pub fn handle(&self) -> HWND {
        self.hwnd.load(Ordering::Acquire) as HWND
    }
    pub fn settings_open(&self) -> bool {
        self.settings_page.load(Ordering::Acquire) != 0
    }
    pub fn video_gpu(&self) -> Option<crate::connection_settings::GpuPreference> {
        self.connection_settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|manager| manager.view().0.video_gpu)
    }
    #[cfg(not(feature = "diagnostics"))]
    pub fn install_connection_settings(&self, manager: Arc<crate::connection_settings::Manager>) {
        *self
            .connection_settings
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(manager);
        unsafe {
            PostMessageW(self.handle(), WM_APP + 8, 0, 0);
        }
    }
    unsafe fn open_connection_settings(&self) {
        let page = self.settings_page.load(Ordering::Acquire) as HWND;
        if !page.is_null() {
            SetFocus(page);
            return;
        }
        let manager = self
            .connection_settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(manager) = manager {
            self.apply_relative_mouse(false);
            let page = crate::connection_settings_ui::open(self.handle(), manager);
            self.settings_page.store(page as usize, Ordering::Release);
            self.show_video_surface();
        }
    }
    // HWND creation and visibility remain on the existing UI thread.
    pub fn ensure_video_surface(&self) -> Result<usize> {
        unsafe {
            PostMessageW(self.handle(), WM_APP + 5, 0, 0);
        }
        for _ in 0..150 {
            let hwnd = self.video_hwnd.load(Ordering::Acquire);
            if hwnd != 0 {
                return Ok(hwnd);
            }
            if self.closing.load(Ordering::Acquire)
                || self.video_create_failed.load(Ordering::Acquire)
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        bail!("native video surface unavailable")
    }
    pub fn set_video_ready(&self, ready: bool) {
        if self.video_ready.swap(ready, Ordering::AcqRel) != ready {
            unsafe {
                PostMessageW(self.handle(), WM_APP + 6, 0, 0);
            }
        }
    }
    unsafe fn show_video_surface(&self) {
        let hwnd = self.video_hwnd.load(Ordering::Acquire) as HWND;
        if !hwnd.is_null() {
            let show = self.video_ready.load(Ordering::Acquire)
                && self.settings_page.load(Ordering::Acquire) == 0
                && self.video_visible.load(Ordering::Acquire)
                && !self.closing.load(Ordering::Acquire);
            ShowWindow(hwnd, if show { SW_SHOWNA } else { SW_HIDE });
        }
    }

    pub fn presented_viewport(&self) -> Option<crate::viewport::Viewport> {
        if !self.video_ready.load(Ordering::Acquire) || !self.video_visible.load(Ordering::Acquire)
        {
            return None;
        }
        let report = self.video_report.lock().unwrap_or_else(|e| e.into_inner());
        let v = report
            .as_ref()
            .filter(|v| v.frames_presented > 0 && v.failure_stage.is_none())?;
        let size = *self.dimensions.lock().unwrap_or_else(|e| e.into_inner());
        let viewport = crate::viewport::Viewport {
            source: (v.width?, v.height?),
            client: (u32::try_from(size.0).ok()?, u32::try_from(size.1).ok()?),
        };
        viewport.rect()?;
        Some(viewport)
    }

    unsafe fn mouse_button(&self, down: bool, button: i32, x: i32, y: i32) {
        let bit = 1usize << button;
        if down {
            self.pressed_buttons.fetch_or(bit, Ordering::AcqRel);
            SetCapture(self.handle());
        } else if self.pressed_buttons.fetch_and(!bit, Ordering::AcqRel) & !bit == 0
            && GetCapture() == self.handle()
        {
            ReleaseCapture();
        }
        self.push(Event::Button(down, button, x, y));
    }
    fn release_buttons(&self) {
        let pressed = self.pressed_buttons.swap(0, Ordering::AcqRel);
        for button in 0..5 {
            if pressed & (1 << button) != 0 {
                self.push(Event::Button(false, button, 0, 0));
            }
        }
    }

    pub fn set_relative_mouse(&self, enabled: bool) {
        self.relative_pending
            .store(if enabled { 2 } else { 1 }, Ordering::Release);
        unsafe {
            PostMessageW(self.handle(), WM_APP + 7, 0, 0);
        }
    }
    unsafe fn apply_relative_mouse(&self, enabled: bool) {
        let enabled = enabled
            && GetForegroundWindow() == self.handle()
            && !self.closing.load(Ordering::Acquire);
        let device = RAWINPUTDEVICE {
            usUsagePage: 1,
            usUsage: 2,
            dwFlags: if enabled { 0 } else { RIDEV_REMOVE },
            hwndTarget: if enabled {
                self.handle()
            } else {
                std::ptr::null_mut()
            },
        };
        let acquired =
            RegisterRawInputDevices(&device, 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32) != 0
                && enabled;
        if acquired {
            let mut rect = RECT::default();
            GetClientRect(self.handle(), &mut rect);
            let mut points = [
                POINT {
                    x: rect.left,
                    y: rect.top,
                },
                POINT {
                    x: rect.right,
                    y: rect.bottom,
                },
            ];
            MapWindowPoints(self.handle(), std::ptr::null_mut(), points.as_mut_ptr(), 2);
            rect = RECT {
                left: points[0].x,
                top: points[0].y,
                right: points[1].x,
                bottom: points[1].y,
            };
            if ClipCursor(&rect) == 0 {
                self.apply_relative_mouse(false);
                return;
            }
            SetCursor(std::ptr::null_mut());
        } else if self.relative_mouse.load(Ordering::Acquire) {
            ClipCursor(std::ptr::null());
        }
        // A rejected request must also report the actual state to the guest.
        self.relative_mouse.store(acquired, Ordering::Release);
        self.push(Event::RelativeMode(acquired));
    }

    pub fn initial_geometry(&self) -> (i32, i32, i32, i32, bool) {
        unsafe {
            let mut origin = POINT { x: 0, y: 0 };
            ClientToScreen(self.handle(), &mut origin);
            (
                origin.x,
                origin.y,
                GetSystemMetrics(SM_CXSCREEN),
                GetSystemMetrics(SM_CYSCREEN),
                GetForegroundWindow() == self.handle(),
            )
        }
    }
    pub fn close(&self) {
        self.request_stop();
        unsafe {
            PostMessageW(self.handle(), WM_APP + 1, 0, 0);
        }
    }
    pub fn request_stop(&self) {
        self.set_relative_mouse(false);
        self.closing.store(true, Ordering::Release);
        self.stop.stop();
    }
    pub fn set_cursor(&self, request: crate::cursor::Request) {
        if self.closing.load(Ordering::Acquire) {
            return;
        }
        self.cursor_pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .submit(request);
        if !self.cursor_posted.swap(true, Ordering::AcqRel) {
            unsafe {
                if PostMessageW(self.handle(), WM_APP + 2, 0, 0) == 0 {
                    self.cursor_posted.store(false, Ordering::Release);
                }
            }
        }
    }
    pub fn set_fullscreen(&self, enable: bool) {
        if self.closing.load(Ordering::Acquire) {
            return;
        }
        // One outstanding UI message, with the latest requested state.
        if self
            .fullscreen_pending
            .swap(if enable { 2 } else { 1 }, Ordering::AcqRel)
            == 0
        {
            unsafe {
                if PostMessageW(self.handle(), WM_APP + 3, 0, 0) == 0 {
                    self.fullscreen_pending.store(0, Ordering::Release);
                }
            }
        }
    }
    pub fn wake_lock_snapshot(&self) -> crate::wake_lock::State {
        self.wake_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    // Only window_proc and the UI thread exit path call this method.
    fn apply_wake_lock(&self, requested: bool, visible: bool) {
        self.wake_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .update(requested, visible);
    }
    pub fn set_wake_lock(&self, enable: bool) {
        if self.closing.load(Ordering::Acquire) || !self.online {
            return;
        }
        if self
            .wake_pending
            .swap(if enable { 2 } else { 1 }, Ordering::AcqRel)
            == 0
        {
            unsafe {
                if PostMessageW(self.handle(), WM_APP + 4, 0, 0) == 0 {
                    self.wake_pending.store(0, Ordering::Release);
                }
            }
        }
    }
    fn push(&self, event: Event) {
        let mut queue = self.events.lock().unwrap_or_else(|e| e.into_inner());
        if queue.len() < 256 {
            queue.push_back(event);
        }
    }
}

unsafe fn create_window(state: &Arc<Window>, opengl_required: bool) -> Result<HWND> {
    let instance = GetModuleHandleW(std::ptr::null());
    let name: Vec<u16> = "ParsecWebTurnNative\0".encode_utf16().collect();
    let class = WNDCLASSW {
        style: CS_OWNDC | CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
        hIcon: LoadIconW(instance, std::ptr::without_provenance(1)),
        lpszClassName: name.as_ptr(),
        ..std::mem::zeroed()
    };
    if RegisterClassW(&class) == 0 {
        bail!("RegisterClassW failed: {}", GetLastError());
    }
    let title: Vec<u16> = crate::APP_TITLE.encode_utf16().chain([0]).collect();
    let hwnd = CreateWindowExW(
        0,
        name.as_ptr(),
        title.as_ptr(),
        WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_CLIPCHILDREN,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        1040,
        760,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        instance,
        Arc::as_ptr(state).cast(),
    );
    if hwnd.is_null() {
        bail!("CreateWindowExW failed: {}", GetLastError());
    }
    if opengl_required {
        let dc = GetDC(hwnd);
        let descriptor = PIXELFORMATDESCRIPTOR {
            nSize: std::mem::size_of::<PIXELFORMATDESCRIPTOR>() as u16,
            nVersion: 1,
            dwFlags: PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER,
            iPixelType: PFD_TYPE_RGBA,
            cColorBits: 32,
            cAlphaBits: 8,
            ..std::mem::zeroed()
        };
        let format = ChoosePixelFormat(dc, &descriptor);
        let mut actual: PIXELFORMATDESCRIPTOR = std::mem::zeroed();
        let described = DescribePixelFormat(
            dc,
            format,
            std::mem::size_of_val(&actual) as u32,
            &mut actual,
        );
        let configured =
            format != 0 && described != 0 && SetPixelFormat(dc, format, &descriptor) != 0;
        ReleaseDC(hwnd, dc);
        if !configured || actual.dwFlags & PFD_GENERIC_FORMAT != 0 {
            DestroyWindow(hwnd);
            bail!("A native accelerated OpenGL pixel format is required; no software fallback");
        }
    }
    let mut rect: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut rect);
    *state.dimensions.lock().unwrap_or_else(|e| e.into_inner()) = (rect.right, rect.bottom);
    Ok(hwnd)
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(lp as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Window;
    if !ptr.is_null() {
        let s = &*ptr;
        let x = lp as i16 as i32;
        let y = (lp >> 16) as i16 as i32;
        match message {
            m if m == WM_APP + 10 => {
                let _ = s
                    .stats_page
                    .compare_exchange(wp, 0, Ordering::AcqRel, Ordering::Acquire);
                return 0;
            }
            WM_COMMAND if wp & 0xffff == crate::stats_ui::OPEN => {
                s.open_stats();
                return 0;
            }
            m if m == WM_APP + 8 => {
                crate::connection_settings_ui::install_menu(hwnd);
                return 0;
            }
            m if m == WM_APP + 9 => {
                let page = s.settings_page.swap(0, Ordering::AcqRel) as HWND;
                if !page.is_null() {
                    DestroyWindow(page);
                    SetFocus(hwnd);
                }
                s.show_video_surface();
                return 0;
            }
            WM_COMMAND if wp & 0xffff == crate::connection_settings_ui::OPEN => {
                s.open_connection_settings();
                return 0;
            }
            WM_COMMAND if wp & 0xffff == crate::connection_settings_ui::ABOUT => {
                crate::connection_settings_ui::show_about(hwnd);
                return 0;
            }
            WM_KEYDOWN
                if wp as u32 == VK_OEM_COMMA as u32 && GetKeyState(VK_CONTROL as i32) < 0 =>
            {
                if lp & (1 << 30) == 0 {
                    s.open_connection_settings();
                }
                return 0;
            }
            WM_GETMINMAXINFO if s.settings_page.load(Ordering::Acquire) != 0 => {
                let limits = &mut *(lp as *mut MINMAXINFO);
                limits.ptMinTrackSize.x = 900;
                limits.ptMinTrackSize.y = 600;
                return 0;
            }
            WM_CLOSE => {
                s.apply_relative_mouse(false);
                s.apply_wake_lock(false, false);
                s.request_stop();
                return 0;
            }
            m if m == WM_APP + 1 => {
                DestroyWindow(hwnd);
                return 0;
            }
            m if m == WM_APP + 2 => {
                s.cursor_posted.store(false, Ordering::Release);
                let pending = std::mem::take(
                    &mut *s.cursor_pending.lock().unwrap_or_else(|e| e.into_inner()),
                );
                let mut cursor = s.cursor.lock().unwrap_or_else(|e| e.into_inner());
                cursor.apply(pending);
                // Refresh stationary pointers too, without changing another
                // window's cursor or the system resize/titlebar cursors.
                let mut point: POINT = std::mem::zeroed();
                if GetCursorPos(&mut point) != 0 && WindowFromPoint(point) == hwnd {
                    ScreenToClient(hwnd, &mut point);
                    let mut rect: RECT = std::mem::zeroed();
                    GetClientRect(hwnd, &mut rect);
                    if point.x >= rect.left
                        && point.x < rect.right
                        && point.y >= rect.top
                        && point.y < rect.bottom
                    {
                        if s.relative_mouse.load(Ordering::Acquire) {
                            SetCursor(std::ptr::null_mut());
                        } else {
                            cursor.select();
                        }
                    }
                }
                return 0;
            }
            WM_SETCURSOR if lp as u16 as u32 == HTCLIENT => {
                if s.relative_mouse.load(Ordering::Acquire) {
                    SetCursor(std::ptr::null_mut());
                    return 1;
                }
                s.cursor.lock().unwrap_or_else(|e| e.into_inner()).select();
                return 1;
            }
            m if m == WM_APP + 3 => {
                let pending = s.fullscreen_pending.swap(0, Ordering::AcqRel);
                if pending != 0 {
                    let mut fullscreen = s.fullscreen.lock().unwrap_or_else(|e| e.into_inner());
                    // Like a rejected browser fullscreen request, an OS failure
                    // leaves the guest running and reports the retained state.
                    let _ = fullscreen.set(hwnd, pending == 2);
                    let active = fullscreen.active();
                    s.fullscreen_active.store(active, Ordering::Release);
                    s.push(Event::Fullscreen(active));
                }
                return 0;
            }
            m if m == WM_APP + 4 => {
                let pending = s.wake_pending.swap(0, Ordering::AcqRel);
                if pending != 0 {
                    s.apply_wake_lock(
                        pending == 2 && !s.closing.load(Ordering::Acquire),
                        IsIconic(hwnd) == 0,
                    );
                }
                return 0;
            }
            m if m == WM_APP + 5 => {
                if s.video_hwnd.load(Ordering::Acquire) == 0 && !s.closing.load(Ordering::Acquire) {
                    let class: Vec<u16> = "ParsecNativeVideoSurface\0".encode_utf16().collect();
                    let (width, height) = *s.dimensions.lock().unwrap_or_else(|e| e.into_inner());
                    // Disabled child receives no input: parent retains its
                    // documented keyboard/mouse handling and window shortcuts.
                    let video_class = WNDCLASSW {
                        lpfnWndProc: Some(video_surface_proc),
                        hInstance: GetModuleHandleW(std::ptr::null()),
                        lpszClassName: class.as_ptr(),
                        ..std::mem::zeroed()
                    };
                    RegisterClassW(&video_class);
                    let child = CreateWindowExW(
                        0,
                        class.as_ptr(),
                        std::ptr::null(),
                        WS_CHILD | WS_DISABLED,
                        0,
                        0,
                        width.max(1),
                        height.max(1),
                        hwnd,
                        std::ptr::null_mut(),
                        GetModuleHandleW(std::ptr::null()),
                        std::ptr::null(),
                    );
                    s.video_create_failed
                        .store(child.is_null(), Ordering::Release);
                    s.video_hwnd.store(child as usize, Ordering::Release);
                }
                return 0;
            }
            m if m == WM_APP + 6 => {
                s.show_video_surface();
                return 0;
            }
            m if m == WM_APP + 7 => {
                let pending = s.relative_pending.swap(0, Ordering::AcqRel);
                if pending != 0 {
                    s.apply_relative_mouse(pending == 2);
                }
                return 0;
            }
            WM_INPUT if s.relative_mouse.load(Ordering::Acquire) => {
                let mut raw: RAWINPUT = std::mem::zeroed();
                let mut size = std::mem::size_of::<RAWINPUT>() as u32;
                let read = GetRawInputData(
                    lp as HRAWINPUT,
                    RID_INPUT,
                    (&mut raw as *mut RAWINPUT).cast(),
                    &mut size,
                    std::mem::size_of::<RAWINPUTHEADER>() as u32,
                );
                if read != u32::MAX
                    && raw.header.dwType == RIM_TYPEMOUSE
                    && read
                        >= std::mem::size_of::<RAWINPUTHEADER>() as u32
                            + std::mem::size_of::<RAWMOUSE>() as u32
                {
                    let mouse = raw.data.mouse;
                    if mouse.usFlags & MOUSE_MOVE_ABSOLUTE == 0 {
                        s.push(Event::RelativeMotion(mouse.lLastX, mouse.lLastY));
                    }
                }
                // DefWindowProc releases the foreground WM_INPUT bookkeeping.
            }
            WM_KEYDOWN
                if wp as u32 == VK_F8 as u32 && s.video_hwnd.load(Ordering::Acquire) != 0 =>
            {
                if lp & (1 << 30) == 0 {
                    s.apply_relative_mouse(false);
                    s.video_visible.fetch_xor(true, Ordering::AcqRel);
                    s.show_video_surface();
                }
                return 0;
            }
            WM_KEYUP if wp as u32 == VK_F8 as u32 && s.video_hwnd.load(Ordering::Acquire) != 0 => {
                return 0
            }
            WM_KEYDOWN if wp as u32 == VK_F11 as u32 => {
                if lp & (1 << 30) == 0 {
                    s.set_fullscreen(!s.fullscreen_active.load(Ordering::Acquire));
                }
                return 0;
            }
            WM_KEYUP if wp as u32 == VK_F11 as u32 => return 0,
            WM_SIZE => {
                let requested =
                    s.wake_lock_snapshot().requested && !s.closing.load(Ordering::Acquire);
                s.apply_wake_lock(requested, wp as u32 != SIZE_MINIMIZED);
                let w = (lp as u32 & 0xffff) as i32;
                let h = ((lp as u32 >> 16) & 0xffff) as i32;
                *s.dimensions.lock().unwrap_or_else(|e| e.into_inner()) = (w, h);
                let page = s.settings_page.load(Ordering::Acquire) as HWND;
                if !page.is_null() {
                    SetWindowPos(
                        page,
                        std::ptr::null_mut(),
                        0,
                        0,
                        w.max(1),
                        h.max(1),
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    );
                }
                let child = s.video_hwnd.load(Ordering::Acquire) as HWND;
                if !child.is_null() {
                    SetWindowPos(
                        child,
                        std::ptr::null_mut(),
                        0,
                        0,
                        w.max(1),
                        h.max(1),
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    );
                }
                s.push(Event::Size(w, h));
                if s.relative_mouse.load(Ordering::Acquire) {
                    s.apply_relative_mouse(w > 0 && h > 0);
                }
            }
            WM_MOVE if s.relative_mouse.load(Ordering::Acquire) => s.apply_relative_mouse(true),
            WM_SETFOCUS => s.push(Event::Focus(true)),
            WM_KILLFOCUS => {
                s.apply_relative_mouse(false);
                s.release_buttons();
                let mut pressed = s.pressed_keys.lock().unwrap_or_else(|e| e.into_inner());
                for code in std::mem::take(&mut *pressed) {
                    s.push(Event::Key(false, code, 0));
                }
                s.push(Event::Focus(false));
                *s.text_decoder.lock().unwrap_or_else(|e| e.into_inner()) = Default::default();
            }
            WM_CAPTURECHANGED => s.release_buttons(),
            WM_MOUSEMOVE if !s.relative_mouse.load(Ordering::Acquire) => {
                s.push(Event::Motion(x, y))
            }
            WM_LBUTTONDOWN | WM_LBUTTONUP => s.mouse_button(message == WM_LBUTTONDOWN, 0, x, y),
            WM_MBUTTONDOWN | WM_MBUTTONUP => {
                s.mouse_button(message == WM_MBUTTONDOWN, 1, x, y);
            }
            WM_XBUTTONDOWN | WM_XBUTTONUP => {
                s.mouse_button(
                    message == WM_XBUTTONDOWN,
                    if ((wp >> 16) & 0xffff) == 1 { 3 } else { 4 },
                    x,
                    y,
                );
                return 1;
            }
            WM_RBUTTONDOWN | WM_RBUTTONUP => s.mouse_button(message == WM_RBUTTONDOWN, 2, x, y),
            WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
                if let Some(code) =
                    crate::input::code(wp as u32, ((lp >> 16) & 0xFF) as u8, lp & (1 << 24) != 0)
                {
                    let down = matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN);
                    let mut pressed = s.pressed_keys.lock().unwrap_or_else(|e| e.into_inner());
                    if down {
                        pressed.insert(code);
                    } else {
                        pressed.remove(code);
                    }
                    s.push(Event::Key(down, code, modifiers()));
                }
                // Keep system handling (notably Alt+F4) in DefWindowProc.
                if matches!(message, WM_KEYDOWN | WM_KEYUP) {
                    return 0;
                }
            }
            WM_CHAR => {
                if let Some(ch) = s
                    .text_decoder
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(wp as u16)
                {
                    s.push(Event::Text(ch as u32));
                }
                return 0;
            }
            WM_MOUSEWHEEL => {
                s.push(Event::Scroll(0, -((wp >> 16) as i16 as i32)));
                return 0;
            }
            WM_MOUSEHWHEEL => {
                s.push(Event::Scroll((wp >> 16) as i16 as i32, 0));
                return 0;
            }
            WM_DESTROY => {
                s.apply_relative_mouse(false);
                s.apply_wake_lock(false, false);
                // Cursor resources are created, replaced and freed on this UI
                // thread. The remaining Arc state owns no native cursor handle.
                *s.cursor.lock().unwrap_or_else(|e| e.into_inner()) = Default::default();
                *s.cursor_pending.lock().unwrap_or_else(|e| e.into_inner()) = Default::default();
                PostQuitMessage(0);
                return 0;
            }
            _ => {}
        }
    }
    DefWindowProcW(hwnd, message, wp, lp)
}

unsafe fn modifiers() -> i32 {
    let mut mods = 0;
    for (vk, bit) in [(VK_SHIFT, 1), (VK_CONTROL, 2), (VK_MENU, 4)] {
        if GetKeyState(vk as i32) < 0 {
            mods |= bit;
        }
    }
    if GetKeyState(VK_LWIN as i32) < 0 || GetKeyState(VK_RWIN as i32) < 0 {
        mods |= 8;
    }
    if GetKeyState(VK_CAPITAL as i32) & 1 != 0 {
        mods |= 16;
    }
    if GetKeyState(VK_NUMLOCK as i32) & 1 != 0 {
        mods |= 32;
    }
    mods
}

// DXGI owns video pixels; do not repaint the child with GDI/static-control text.
unsafe extern "system" fn video_surface_proc(
    hwnd: HWND,
    message: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    match message {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut paint: PAINTSTRUCT = std::mem::zeroed();
            BeginPaint(hwnd, &mut paint);
            EndPaint(hwnd, &paint);
            0
        }
        _ => DefWindowProcW(hwnd, message, wp, lp),
    }
}
