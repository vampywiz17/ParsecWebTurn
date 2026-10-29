//! Modeless native statistics. Painting never touches video textures.
use crate::stats::{Rates, Sample, Shared, Values};
use std::{
    ptr::{null, null_mut},
    sync::{atomic::Ordering, Arc},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::*,
    UI::{Controls::*, WindowsAndMessaging::*},
};
pub const OPEN: usize = 4301;
const TAB_BASE: usize = 4400;
const CONTENT_TOP: i32 = 174;
const BG: u32 = crate::branding::BACKGROUND;
const FG: u32 = 0xeeeeee;
const MUTED: u32 = 0xbcbcbc;
const CYAN: u32 = 0xffbb00;
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
struct Page {
    tab: crate::stats_view::Tab,
    tabs: [HWND; 5],
    notify: HWND,
    window_owned: bool,
    bus: Arc<Shared>,
    sample: Sample,
    rates: Rates,
    values: Values,
    resources: crate::performance::Sampler,
    usage: (Option<f64>, Option<f64>, Option<f64>),
    memory_usage: Option<crate::performance::MemoryUsage>,
    scroll: i32,
    height: i32,
    font: HFONT,
    heading: HFONT,
}
impl Drop for Page {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.font);
            DeleteObject(self.heading);
        }
        self.bus.visible.store(false, Ordering::Release);
    }
}
unsafe fn layout_tabs(hwnd: HWND, page: &Page) {
    let mut rect = RECT::default();
    GetClientRect(hwnd, &mut rect);
    let width = ((rect.right - 56 - 4 * 8) / 5).max(1);
    for (i, &button) in page.tabs.iter().enumerate() {
        if !button.is_null() {
            MoveWindow(button, 28 + i as i32 * (width + 8), 122, width, 34, 1);
        }
    }
}
unsafe fn draw_tab(item: &DRAWITEMSTRUCT, page: &Page) {
    let index = item.CtlID as usize - TAB_BASE;
    let selected = crate::stats_view::Tab::ALL[index] == page.tab;
    let brush = CreateSolidBrush(if selected { CYAN } else { 0x302921 });
    FillRect(item.hDC, &item.rcItem, brush);
    DeleteObject(brush);
    SelectObject(item.hDC, page.font);
    SetBkMode(item.hDC, TRANSPARENT as i32);
    SetTextColor(item.hDC, if selected { 0x101010 } else { FG });
    let mut rect = item.rcItem;
    DrawTextW(
        item.hDC,
        wide(crate::stats_view::Tab::ALL[index].caption()).as_ptr(),
        -1,
        &mut rect,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
    if item.itemState & ODS_FOCUS != 0 {
        InflateRect(&mut rect, -3, -3);
        DrawFocusRect(item.hDC, &rect);
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
pub unsafe fn open(parent: HWND, bus: Arc<Shared>) -> HWND {
    let class = wide("EvoParseqStats");
    let instance = GetModuleHandleW(null());
    RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(proc),
        hInstance: instance,
        hIcon: crate::branding::icon(),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: class.as_ptr(),
        ..std::mem::zeroed()
    });
    let page = Box::new(Page {
        tab: Default::default(),
        tabs: [null_mut(); 5],
        notify: parent,
        window_owned: false,
        sample: bus.read().unwrap_or_default(),
        bus,
        rates: Rates::default(),
        values: Values::default(),
        resources: Default::default(),
        usage: (None, None, None),
        memory_usage: crate::performance::own_memory(),
        scroll: 0,
        height: 0,
        font: font(16),
        heading: font(32),
    });
    let ptr = Box::into_raw(page);
    let hwnd = CreateWindowExW(
        0,
        class.as_ptr(),
        wide("Connection stats").as_ptr(),
        WS_OVERLAPPEDWINDOW | WS_VSCROLL | WS_CLIPCHILDREN,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        760,
        820,
        null_mut(),
        null_mut(),
        instance,
        ptr.cast(),
    );
    if hwnd.is_null() {
        drop(Box::from_raw(ptr));
        return hwnd;
    }
    let page = &mut *ptr;
    page.bus.visible.store(true, Ordering::Release);
    page.window_owned = true;
    SetTimer(hwnd, 1, 1000, None);
    ShowWindow(hwnd, SW_SHOW);
    hwnd
}
unsafe fn text(dc: HDC, font: HFONT, color: u32, s: &str, mut rect: RECT) {
    SelectObject(dc, font);
    SetTextColor(dc, color);
    SetBkMode(dc, TRANSPARENT as i32);
    DrawTextW(
        dc,
        wide(s).as_ptr(),
        -1,
        &mut rect,
        DT_LEFT | DT_WORDBREAK | DT_NOPREFIX,
    );
}
unsafe fn wrapped_height(dc: HDC, font: HFONT, s: &str, width: i32) -> i32 {
    SelectObject(dc, font);
    let mut rect = RECT {
        right: width.max(1),
        ..std::mem::zeroed()
    };
    DrawTextW(
        dc,
        wide(s).as_ptr(),
        -1,
        &mut rect,
        DT_LEFT | DT_WORDBREAK | DT_NOPREFIX | DT_CALCRECT,
    );
    rect.bottom - rect.top
}
unsafe fn paint(hwnd: HWND, page: &mut Page, dc: HDC) {
    let mut client: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut client);
    let memory = CreateCompatibleDC(dc);
    let bitmap = CreateCompatibleBitmap(dc, client.right.max(1), client.bottom.max(1));
    let old = SelectObject(memory, bitmap);
    let background = CreateSolidBrush(BG);
    FillRect(memory, &client, background);
    DeleteObject(background);
    let left = 28;
    let right = client.right - 28;
    let mut y = 26;
    text(
        memory,
        page.heading,
        FG,
        "Connection stats",
        RECT {
            left,
            top: y,
            right,
            bottom: y + 46,
        },
    );
    y += 54;
    let timing = page.bus.host_timing.snapshot(page.sample.generation);
    let presentation = crate::stats_view::Presentation::new(
        &page.sample,
        &page.values,
        page.usage,
        page.memory_usage,
        &timing,
    )
    .select(page.tab);
    text(
        memory,
        page.font,
        CYAN,
        presentation.status,
        RECT {
            left,
            top: y,
            right,
            bottom: y + 28,
        },
    );
    y = CONTENT_TOP - page.scroll;
    let saved_clip = SaveDC(memory);
    IntersectClipRect(memory, 0, CONTENT_TOP, client.right, client.bottom);
    let cards = &presentation.cards;
    let width = (right - left - 16) / 2;
    for (i, (label, value)) in cards.iter().enumerate() {
        let x = left + (i as i32 % 2) * (width + 16);
        let top = y + (i as i32 / 2) * 100;
        let card = RECT {
            left: x,
            top,
            right: x + width,
            bottom: top + 86,
        };
        let brush = CreateSolidBrush(0x212121);
        FillRect(memory, &card, brush);
        DeleteObject(brush);
        text(
            memory,
            page.font,
            MUTED,
            label,
            RECT {
                left: x + 16,
                top: top + 10,
                right: x + width - 10,
                bottom: top + 34,
            },
        );
        text(
            memory,
            page.heading,
            FG,
            value,
            RECT {
                left: x + 16,
                top: top + 35,
                right: x + width - 10,
                bottom: top + 80,
            },
        );
    }
    y += (cards.len().div_ceil(2) * 100) as i32 + 18;
    for (label, value) in presentation.rows {
        let section = value.is_empty();
        let label_width = if section { right - left } else { 210 };
        let label_height = wrapped_height(memory, page.font, &label, label_width);
        let value_height = if section {
            0
        } else {
            wrapped_height(memory, page.font, &value, right - left - 220)
        };
        let h = (label_height.max(value_height) + 16).max(if section { 48 } else { 44 });
        text(
            memory,
            page.font,
            if section { CYAN } else { MUTED },
            &label,
            RECT {
                left,
                top: y + 8,
                right: if section { right } else { left + 210 },
                bottom: y + h,
            },
        );
        if !section {
            text(
                memory,
                page.font,
                FG,
                &value,
                RECT {
                    left: left + 220,
                    top: y + 8,
                    right,
                    bottom: y + h,
                },
            );
        }
        y += h;
    }
    y += 14;
    let footer = presentation.footer;
    // Measure wrapped text using the same font and width as the actual paint.
    // The BUD explanation can exceed a fixed-height box, especially on resize.
    let mut footer_rect = RECT {
        left,
        top: y,
        right,
        bottom: y,
    };
    SelectObject(memory, page.font);
    DrawTextW(
        memory,
        wide(&footer).as_ptr(),
        -1,
        &mut footer_rect,
        DT_LEFT | DT_WORDBREAK | DT_NOPREFIX | DT_CALCRECT,
    );
    text(memory, page.font, MUTED, &footer, footer_rect);
    y = footer_rect.bottom + 24;
    page.height = (y + page.scroll - CONTENT_TOP).max(0);
    let info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
        nMin: 0,
        nMax: page.height,
        nPage: (client.bottom - CONTENT_TOP).max(0) as u32,
        nPos: page.scroll,
        nTrackPos: 0,
    };
    SetScrollInfo(hwnd, SB_VERT, &info, 1);
    RestoreDC(memory, saved_clip);
    BitBlt(dc, 0, 0, client.right, client.bottom, memory, 0, 0, SRCCOPY);
    SelectObject(memory, old);
    DeleteObject(bitmap);
    DeleteDC(memory);
}
unsafe extern "system" fn proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            (*(lp as *const CREATESTRUCTW)).lpCreateParams as isize,
        );
        // Let Windows initialize the native caption from CREATESTRUCTW.
        return DefWindowProcW(hwnd, message, wp, lp);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Page;
    if !ptr.is_null() {
        let page = &mut *ptr;
        match message {
            WM_CREATE => {
                for (i, tab) in crate::stats_view::Tab::ALL.into_iter().enumerate() {
                    let button = CreateWindowExW(
                        0,
                        wide("BUTTON").as_ptr(),
                        wide(tab.caption()).as_ptr(),
                        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32,
                        0,
                        0,
                        1,
                        1,
                        hwnd,
                        (TAB_BASE + i) as HMENU,
                        GetModuleHandleW(null()),
                        null(),
                    );
                    if button.is_null() {
                        return -1;
                    }
                    page.tabs[i] = button;
                    SendMessageW(button, WM_SETFONT, page.font as usize, 0);
                }
                layout_tabs(hwnd, page);
                return 0;
            }
            WM_DRAWITEM if (TAB_BASE..TAB_BASE + 5).contains(&wp) => {
                draw_tab(&*(lp as *const DRAWITEMSTRUCT), page);
                return 1;
            }
            WM_COMMAND if wp & 0xffff == 2 => {
                DestroyWindow(hwnd);
                return 0;
            }
            WM_COMMAND if wp & 0xffff >= TAB_BASE && wp & 0xffff < TAB_BASE + 5 => {
                page.tab = crate::stats_view::Tab::ALL[(wp & 0xffff) - TAB_BASE];
                page.scroll = 0;
                for &button in &page.tabs {
                    InvalidateRect(button, null(), 0);
                }
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            WM_ACTIVATE => {
                PostMessageW(
                    page.notify,
                    crate::native_core::session_ui::VISIBILITY,
                    0,
                    0,
                );
            }
            WM_TIMER => {
                if IsIconic(hwnd) != 0 {
                    return 0;
                }
                if let Some(s) = page.bus.read() {
                    if s.at != page.sample.at {
                        page.values = page.rates.update(&s);
                        page.sample = s;
                    }
                }
                let id = std::process::id();
                page.memory_usage = crate::performance::own_memory();
                page.usage = page.resources.sample(&crate::performance::Processes {
                    all: vec![id],
                    gpu: vec![id],
                });
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            WM_SIZE => {
                layout_tabs(hwnd, page);
                let mut rect: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut rect);
                let viewport = (rect.bottom - CONTENT_TOP).max(0);
                page.scroll = page.scroll.min((page.height - viewport).max(0));
                if wp == SIZE_MINIMIZED as usize {
                    page.resources = Default::default();
                    page.usage = (None, None, None);
                    page.rates = Rates::default();
                    page.values = Values::default();
                }
                page.bus
                    .visible
                    .store(wp != SIZE_MINIMIZED as usize, Ordering::Release);
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            WM_GETMINMAXINFO => {
                let info = &mut *(lp as *mut MINMAXINFO);
                info.ptMinTrackSize.x = 660;
                info.ptMinTrackSize.y = 420;
                return 0;
            }
            WM_MOUSEWHEEL | WM_VSCROLL => {
                let mut rect = std::mem::zeroed();
                GetClientRect(hwnd, &mut rect);
                let viewport = (rect.bottom - CONTENT_TOP).max(0);
                let target = if message == WM_MOUSEWHEEL {
                    page.scroll - ((wp >> 16) as i16 as i32) * 90 / 120
                } else {
                    match wp as i32 & 0xffff {
                        SB_LINEUP => page.scroll - 32,
                        SB_LINEDOWN => page.scroll + 32,
                        SB_PAGEUP => page.scroll - viewport,
                        SB_PAGEDOWN => page.scroll + viewport,
                        SB_TOP => 0,
                        SB_BOTTOM => page.height,
                        SB_THUMBTRACK | SB_THUMBPOSITION => {
                            let mut i: SCROLLINFO = std::mem::zeroed();
                            i.cbSize = std::mem::size_of::<SCROLLINFO>() as u32;
                            i.fMask = SIF_TRACKPOS;
                            GetScrollInfo(hwnd, SB_VERT, &mut i);
                            i.nTrackPos
                        }
                        _ => page.scroll,
                    }
                };
                page.scroll = target.clamp(0, (page.height - viewport).max(0));
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            WM_ERASEBKGND => return 1,
            WM_PAINT => {
                let mut ps = std::mem::zeroed();
                let dc = BeginPaint(hwnd, &mut ps);
                paint(hwnd, page, dc);
                EndPaint(hwnd, &ps);
                return 0;
            }
            WM_PRINTCLIENT => {
                paint(hwnd, page, wp as HDC);
                return 0;
            }
            WM_KEYDOWN if wp == 27 => {
                DestroyWindow(hwnd);
                return 0;
            }
            WM_NCDESTROY => {
                KillTimer(hwnd, 1);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                SendMessageW(page.notify, WM_APP + 10, hwnd as usize, 0);
                if page.window_owned {
                    drop(Box::from_raw(ptr));
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
    fn modeless_window_scroll_resize_and_lifetime() {
        unsafe {
            let parent = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                wide("Synthetic stats fixture").as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                800,
                800,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            assert!(!parent.is_null());
            let bus = Arc::new(Shared::default());
            let generation = bus.begin();
            let mut local_timing = crate::video_output::LocalTiming::default();
            local_timing
                .decoder_service
                .record(std::time::Duration::from_micros(330));
            bus.publish(
                generation,
                Sample {
                    state: "connected".into(),
                    dtls: "connected".into(),
                    route: "Relay — TURN in use",
                    local: "relay".into(),
                    remote: "host".into(),
                    protocol: "udp".into(),
                    turn_server: Some("turns:turn.example.org:443?transport=tcp".into()),
                    turn_protocol: Some("tls".into()),
                    rtt_ms: Some(23.5),
                    audio_ready: true,
                    profile: Some(100),
                    codec: Some(crate::video_stream::Codec::H264),
                    video: Some(crate::video_output::Snapshot {
                        local_timing,
                        presentation_mode: Some("Low latency; maximum queued presentations: 1"),
                        decoder_initialized: true,
                        decoder: Some("Media Foundation H.264"),
                        renderer: Some("D3D11"),
                        width: Some(1920),
                        height: Some(1080),
                        d3d11_decoder_surface: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            );
            let hwnd = open(parent, bus.clone());
            assert!(!hwnd.is_null());

            assert!(
                GetWindow(hwnd, GW_OWNER).is_null(),
                "Native stats must be an independent window"
            );

            let mut title = [0u16; 64];
            let length = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
            assert_eq!(
                String::from_utf16_lossy(&title[..length as usize]),
                "Connection stats"
            );
            assert!(bus.requested());
            capture(hwnd, "stats-top");
            SendMessageW(hwnd, WM_VSCROLL, SB_BOTTOM as usize, 0);
            let page = &*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Page);
            assert!(page.scroll > 0);
            capture(hwnd, "stats-bottom");
            SendMessageW(hwnd, WM_COMMAND, TAB_BASE + 4, 0);
            assert_eq!(
                (*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Page)).tab,
                crate::stats_view::Tab::Timing
            );
            bus.host_timing.set_requested(generation, true);
            bus.host_timing.receive(
                generation,
                &crate::frame_timing::tests::packet(0, 0, &[12., 2., 4., 16.7]),
            );
            bus.host_timing.receive(
                generation,
                &crate::frame_timing::tests::packet(1, 0, &[8., 0., 20.]),
            );
            RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_UPDATENOW);
            SendMessageW(hwnd, WM_VSCROLL, SB_BOTTOM as usize, 0);
            capture(hwnd, "stats-host-timing-bottom");
            #[cfg(feature = "bud")]
            {
                let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Page;
                SendMessageW(hwnd, WM_COMMAND, TAB_BASE + 1, 0);
                (*state).sample.bud = Some(evoparseq_bud::bud::Snapshot::default());
                RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_UPDATENOW);
                SendMessageW(hwnd, WM_VSCROLL, SB_BOTTOM as usize, 0);
                capture(hwnd, "stats-bud-bottom");
                (*state).sample.bud = None;
            }
            SetWindowPos(
                hwnd,
                null_mut(),
                0,
                0,
                660,
                460,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            SendMessageW(hwnd, WM_VSCROLL, SB_TOP as usize, 0);
            capture(hwnd, "stats-narrow");
            let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Page;
            SendMessageW(hwnd, WM_COMMAND, TAB_BASE + 4, 0);
            (*state).scroll = 700;
            capture(hwnd, "stats-local-timing-narrow");
            for (i, tab) in crate::stats_view::Tab::ALL.into_iter().enumerate() {
                SendMessageW(hwnd, WM_COMMAND, TAB_BASE + i, 0);
                assert_eq!((*state).tab, tab);
                assert_eq!((*state).scroll, 0);
                assert!(bus.requested());
                capture(
                    hwnd,
                    &format!("stats-tab-{}", tab.caption().to_ascii_lowercase()),
                );
            }
            SendMessageW(hwnd, WM_SIZE, SIZE_MINIMIZED as usize, 0);
            assert!(!bus.requested());
            SendMessageW(hwnd, WM_SIZE, SIZE_RESTORED as usize, 0);
            assert!(bus.requested());
            DestroyWindow(hwnd);
            assert!(!bus.requested());
            DestroyWindow(parent);
        }
    }
    // Synthetic GDI fixture only; no account, clipboard or live video is captured.
    unsafe fn capture(hwnd: HWND, name: &str) {
        let mut rect: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rect);
        let dc = CreateCompatibleDC(null_mut());
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = rect.right;
        info.bmiHeader.biHeight = -rect.bottom;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut pixels = null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut pixels, null_mut(), 0);
        assert!(!bitmap.is_null() && !pixels.is_null());
        let old = SelectObject(dc, bitmap);
        SendMessageW(hwnd, WM_PRINTCLIENT, dc as usize, 0);
        // WM_PRINT's child traversal uses non-client origin offsets on this
        // offscreen DC. Reuse the actual owner-draw painter with the controls'
        // measured client-space bounds instead; no pixel relocation or video.
        let page = &*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Page);
        for (i, &button) in page.tabs.iter().enumerate() {
            let mut bounds = RECT::default();
            assert_ne!(GetWindowRect(button, &mut bounds), 0);
            MapWindowPoints(null_mut(), hwnd, (&mut bounds as *mut RECT).cast(), 2);
            assert_eq!(bounds.top, 122);
            assert!(bounds.bottom <= CONTENT_TOP);
            draw_tab(
                &DRAWITEMSTRUCT {
                    CtlType: ODT_BUTTON,
                    CtlID: (TAB_BASE + i) as u32,
                    itemAction: ODA_DRAWENTIRE,
                    hwndItem: button,
                    hDC: dc,
                    rcItem: bounds,
                    ..std::mem::zeroed()
                },
                page,
            );
        }
        GdiFlush();
        let mut rgba = std::slice::from_raw_parts(
            pixels.cast::<u8>(),
            (rect.right * rect.bottom * 4) as usize,
        )
        .to_vec();
        assert!(rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[0] != 0x28 || p[1] != 0x28 || p[2] != 0x28));
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }
        if let Some(path) = std::env::var_os("EVOPARSEQ_STATS_CAPTURE") {
            let path = std::path::PathBuf::from(path);
            std::fs::create_dir_all(&path).unwrap();
            image::save_buffer(
                path.join(format!("{name}.png")),
                &rgba,
                rect.right as u32,
                rect.bottom as u32,
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
        SelectObject(dc, old);
        DeleteObject(bitmap);
        DeleteDC(dc);
    }
}
