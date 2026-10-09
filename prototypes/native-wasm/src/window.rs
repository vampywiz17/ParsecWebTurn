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
    UI::Input::KeyboardAndMouse::*,
    UI::WindowsAndMessaging::*,
};

#[derive(Clone, Debug)]
pub enum Event {
    Size(i32, i32),
    Focus(bool),
    Motion(i32, i32),
    Button(bool, i32, i32, i32),
    Text(u32),
    Key(bool, &'static str, i32),
    Scroll(i32, i32),
    Fullscreen(bool),
}

pub struct Window {
    pub hwnd: AtomicUsize,
    pub closing: AtomicBool,
    pub active_contexts: AtomicUsize,
    pub events: Mutex<std::collections::VecDeque<Event>>,
    pub dimensions: Mutex<(i32, i32)>,
    pub graphics: Mutex<Option<crate::graphics::GraphicsReport>>,
    pub capture: Mutex<Option<std::path::PathBuf>>,
    pub synthetic_login: bool,
    pub script_steps: AtomicUsize,
    pub run_seconds: u64,
    pub live: bool,
    pub online: bool,
    pub network_origin_audit: bool,
    pub stop: Arc<crate::lifecycle::StopSignal>,
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
    pub fn create(
        synthetic_login: bool,
        live: bool,
        online: bool,
        network_origin_audit: bool,
    ) -> Result<Arc<Self>> {
        let state = Arc::new(Self {
            hwnd: AtomicUsize::new(0),
            closing: AtomicBool::new(false),
            active_contexts: AtomicUsize::new(0),
            events: Default::default(),
            dimensions: Mutex::new((1024, 720)),
            graphics: Default::default(),
            capture: Default::default(),
            synthetic_login,
            script_steps: AtomicUsize::new(0),
            run_seconds: if synthetic_login { 20 } else { 8 },
            live,
            online,
            network_origin_audit,
            stop: Default::default(),
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
            let outcome = unsafe { create_window(&ui) };
            match outcome {
                Ok(hwnd) => {
                    ui.hwnd.store(hwnd as usize, Ordering::Release);
                    let _ = tx.send(Ok(()));
                    unsafe {
                        let mut msg: MSG = std::mem::zeroed();
                        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
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

unsafe fn create_window(state: &Arc<Window>) -> Result<HWND> {
    let instance = GetModuleHandleW(std::ptr::null());
    let name: Vec<u16> = "ParsecNativePrototype\0".encode_utf16().collect();
    let class = WNDCLASSW {
        style: CS_OWNDC | CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
        lpszClassName: name.as_ptr(),
        ..std::mem::zeroed()
    };
    if RegisterClassW(&class) == 0 {
        bail!("RegisterClassW failed: {}", GetLastError());
    }
    let title: Vec<u16> = "Parsec native WASM â€” GPU prototype\0"
        .encode_utf16()
        .collect();
    let hwnd = CreateWindowExW(
        0,
        name.as_ptr(),
        title.as_ptr(),
        WS_OVERLAPPEDWINDOW | WS_VISIBLE,
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
    let configured = format != 0 && described != 0 && SetPixelFormat(dc, format, &descriptor) != 0;
    ReleaseDC(hwnd, dc);
    if !configured || actual.dwFlags & PFD_GENERIC_FORMAT != 0 {
        DestroyWindow(hwnd);
        bail!("A native accelerated OpenGL pixel format is required; no software fallback");
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
            WM_CLOSE => {
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
                        cursor.select();
                    }
                }
                return 0;
            }
            WM_SETCURSOR if lp as u16 as u32 == HTCLIENT => {
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
                s.push(Event::Size(w, h));
            }
            WM_SETFOCUS => s.push(Event::Focus(true)),
            WM_KILLFOCUS => {
                let mut pressed = s.pressed_keys.lock().unwrap_or_else(|e| e.into_inner());
                for code in std::mem::take(&mut *pressed) {
                    s.push(Event::Key(false, code, 0));
                }
                s.push(Event::Focus(false));
                *s.text_decoder.lock().unwrap_or_else(|e| e.into_inner()) = Default::default();
            }
            WM_MOUSEMOVE => s.push(Event::Motion(x, y)),
            WM_LBUTTONDOWN | WM_LBUTTONUP => {
                s.push(Event::Button(message == WM_LBUTTONDOWN, 0, x, y))
            }
            WM_RBUTTONDOWN | WM_RBUTTONUP => {
                s.push(Event::Button(message == WM_RBUTTONDOWN, 2, x, y))
            }
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
