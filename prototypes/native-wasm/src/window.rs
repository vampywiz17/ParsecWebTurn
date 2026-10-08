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
    UI::WindowsAndMessaging::*,
};

#[derive(Clone, Debug)]
pub enum Event {
    Size(i32, i32),
    Focus(bool),
    Motion(i32, i32),
    Button(bool, i32, i32, i32),
    Text(u32),
}

pub struct Window {
    pub hwnd: AtomicUsize,
    pub closing: AtomicBool,
    pub active_contexts: AtomicUsize,
    pub events: Mutex<std::collections::VecDeque<Event>>,
    pub dimensions: Mutex<(i32, i32)>,
    pub graphics: Mutex<Option<crate::graphics::GraphicsReport>>,
    pub capture: Mutex<Option<std::path::PathBuf>>,
}

impl Window {
    pub fn create() -> Result<Arc<Self>> {
        let state = Arc::new(Self {
            hwnd: AtomicUsize::new(0),
            closing: AtomicBool::new(false),
            active_contexts: AtomicUsize::new(0),
            events: Default::default(),
            dimensions: Mutex::new((1024, 720)),
            graphics: Default::default(),
            capture: Default::default(),
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
        self.closing.store(true, Ordering::Release);
        unsafe {
            PostMessageW(self.handle(), WM_APP + 1, 0, 0);
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
    let title: Vec<u16> = "Parsec native WASM — GPU prototype\0"
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
                s.closing.store(true, Ordering::Release);
                return 0;
            }
            m if m == WM_APP + 1 => {
                DestroyWindow(hwnd);
                return 0;
            }
            WM_SIZE => {
                let w = (lp as u32 & 0xffff) as i32;
                let h = ((lp as u32 >> 16) & 0xffff) as i32;
                *s.dimensions.lock().unwrap_or_else(|e| e.into_inner()) = (w, h);
                s.push(Event::Size(w, h));
            }
            WM_SETFOCUS => s.push(Event::Focus(true)),
            WM_KILLFOCUS => s.push(Event::Focus(false)),
            WM_MOUSEMOVE => s.push(Event::Motion(x, y)),
            WM_LBUTTONDOWN | WM_LBUTTONUP => {
                s.push(Event::Button(message == WM_LBUTTONDOWN, 1, x, y))
            }
            WM_RBUTTONDOWN | WM_RBUTTONUP => {
                s.push(Event::Button(message == WM_RBUTTONDOWN, 2, x, y))
            }
            WM_CHAR => s.push(Event::Text(wp as u32)),
            WM_DESTROY => {
                PostQuitMessage(0);
                return 0;
            }
            _ => {}
        }
    }
    DefWindowProcW(hwnd, message, wp, lp)
}
