//! Read-only, modeless native statistics. Painting never touches video textures.
use crate::stats::{Rates, Sample, Shared, Values};
use std::{
    ptr::{null, null_mut},
    sync::{atomic::Ordering, Arc},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::*,
    UI::{Controls::SetScrollInfo, WindowsAndMessaging::*},
};
pub const OPEN: usize = 4301;
const BG: u32 = 0x282828;
const FG: u32 = 0xeeeeee;
const MUTED: u32 = 0xbcbcbc;
const CYAN: u32 = 0xffbb00;
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
struct Page {
    window_owned: bool,
    bus: Arc<Shared>,
    sample: Sample,
    rates: Rates,
    values: Values,
    resources: crate::performance::Sampler,
    usage: (Option<f64>, Option<f64>, Option<f64>),
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
    let class = wide("ParsecWebTurnStats");
    let instance = GetModuleHandleW(null());
    RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(proc),
        hInstance: instance,
        hIcon: LoadIconW(instance, std::ptr::without_provenance(1)),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: class.as_ptr(),
        ..std::mem::zeroed()
    });
    let page = Box::new(Page {
        window_owned: false,
        sample: bus.read().unwrap_or_default(),
        bus,
        rates: Rates::default(),
        values: Values::default(),
        resources: Default::default(),
        usage: (None, None, None),
        scroll: 0,
        height: 0,
        font: font(16),
        heading: font(32),
    });
    let ptr = Box::into_raw(page);
    let hwnd = CreateWindowExW(
        0,
        class.as_ptr(),
        wide("ParsecWebTurn — Connection stats").as_ptr(),
        WS_OVERLAPPEDWINDOW | WS_VSCROLL,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        760,
        820,
        parent,
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
fn number(value: Option<f64>, unit: &str) -> String {
    value
        .filter(|v| v.is_finite())
        .map(|v| format!("{v:.1} {unit}"))
        .unwrap_or_else(|| "—".into())
}
fn reported(s: &str) -> String {
    if s.is_empty() {
        "Not reported".into()
    } else {
        s.into()
    }
}
fn rows(
    s: &Sample,
    v: &Values,
    cpu: (Option<f64>, Option<f64>, Option<f64>),
) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let mut row = |a: &str, b: String| rows.push((a.into(), b));
    row("NETWORK", String::new());
    row(
        "Route",
        if s.state == "connected" && s.at.elapsed().as_secs() > 4 {
            "Unverified (telemetry unavailable)".into()
        } else {
            s.route.into()
        },
    );
    row("Local candidate", reported(&s.local));
    row("Remote candidate", reported(&s.remote));
    row("Transport", reported(&s.protocol));
    row(
        "TURN server in use",
        s.turn_server.clone().unwrap_or_else(|| {
            if s.route == "Direct — no TURN" {
                "None".into()
            } else {
                "Not reported".into()
            }
        }),
    );
    row(
        "TURN transport",
        s.turn_protocol
            .clone()
            .unwrap_or_else(|| "Not reported".into()),
    );
    row("VIDEO", String::new());
    let video = s.video.as_ref();
    row(
        "Codec",
        if s.profile.is_some() || video.is_some_and(|x| x.decoder_initialized) {
            "H.264".into()
        } else {
            "Not reported".into()
        },
    );
    row(
        "Profile",
        s.profile
            .map(|p| match p {
                66 => "Baseline".into(),
                77 => "Main".into(),
                100 => "High".into(),
                _ => format!("H.264 profile {p}"),
            })
            .unwrap_or_else(|| "Not reported".into()),
    );
    row(
        "Resolution",
        video
            .and_then(|x| x.width.zip(x.height))
            .map(|(w, h)| format!("{w} × {h}"))
            .unwrap_or_else(|| "Not reported".into()),
    );
    row(
        "Decoder",
        video
            .and_then(|x| x.decoder)
            .unwrap_or("Not reported")
            .into(),
    );
    row(
        "Renderer",
        video
            .and_then(|x| x.renderer)
            .unwrap_or("Not reported")
            .into(),
    );
    row(
        "GPU adapter",
        video
            .and_then(|x| x.adapter.clone())
            .unwrap_or_else(|| "Not reported".into()),
    );
    if let Some(stage) = video.and_then(|x| x.failure_stage) {
        row(
            "Video status",
            match stage {
                "video-selected-gpu-unavailable-select-automatic-in-settings" => {
                    "Selected GPU unavailable; reselect in Settings.".into()
                }
                "video-d3d11-device" | "video-decoder-not-d3d11-aware" => {
                    "Video GPU unavailable or unsupported; try another GPU.".into()
                }
                _ => format!("Video stopped: {stage}"),
            },
        );
    }
    row(
        "Hardware decode",
        video
            .map(|x| x.hardware_decode_status())
            .unwrap_or("Not reported")
            .into(),
    );
    row(
        "Decoder output",
        video
            .and_then(|x| x.d3d11_decoder_surface)
            .map(|x| {
                if x {
                    "D3D11 decoder surface"
                } else {
                    "DXGI surface"
                }
            })
            .unwrap_or("Not reported")
            .into(),
    );
    row(
        "Local dropped frames",
        video
            .map(|x| x.frames_dropped.to_string())
            .unwrap_or_else(|| "Not reported".into()),
    );
    row("AUDIO", String::new());
    for (name, value) in [
        ("Codec", "Opus"),
        ("Sample rate", "48,000 Hz"),
        ("Channels", "2 (stereo)"),
    ] {
        row(
            name,
            if s.audio_ready {
                value.into()
            } else {
                "Not reported".into()
            },
        );
    }
    row("Measured audio traffic", number(v.audio, "kbps"));
    row("SECURITY", String::new());
    row("DTLS state", s.dtls.clone());
    row(
        "Encryption",
        if s.dtls == "connected" {
            "DTLS protects SCTP data channels".into()
        } else {
            "Not reported".into()
        },
    );
    row("APP PERFORMANCE", String::new());
    row("CPU (all cores)", number(cpu.0, "%"));
    row("GPU (busiest engine)", number(cpu.1, "%"));
    rows
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
    let mut y = 26 - page.scroll;
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
    let stale = page.sample.at.elapsed().as_secs() > 4 && page.sample.state == "connected";
    let status = if stale {
        "Waiting for fresh telemetry"
    } else {
        &page.sample.state
    };
    text(
        memory,
        page.font,
        CYAN,
        status,
        RECT {
            left,
            top: y,
            right,
            bottom: y + 28,
        },
    );
    y += 40;
    let cards = [
        ("Connection RTT", number(page.sample.rtt_ms, "ms")),
        ("Decoded video", number(page.values.fps, "fps")),
        ("Incoming traffic", number(page.values.incoming, "Mbps")),
        ("Outgoing traffic", number(page.values.outgoing, "Mbps")),
    ];
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
            if stale { "—" } else { value },
            RECT {
                left: x + 16,
                top: top + 35,
                right: x + width - 10,
                bottom: top + 80,
            },
        );
    }
    y += 218;
    for (label, value) in rows(&page.sample, &page.values, page.usage) {
        let section = value.is_empty();
        let h = if section { 48 } else { 44 };
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
    text(memory,page.font,MUTED,"Traffic is measured usage, not available bandwidth. RTT is the active ICE path's round-trip time, not ICMP ping. Local dropped frames are not network packet loss. Missing values remain unknown.",RECT{left,top:y,right,bottom:y+100});
    y += 110;
    page.height = y + page.scroll;
    let info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
        nMin: 0,
        nMax: page.height,
        nPage: client.bottom.max(0) as u32,
        nPos: page.scroll,
        nTrackPos: 0,
    };
    SetScrollInfo(hwnd, SB_VERT, &info, 1);
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
        return 1;
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Page;
    if !ptr.is_null() {
        let page = &mut *ptr;
        match message {
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
                page.usage = page.resources.sample(&crate::performance::Processes {
                    all: vec![id],
                    gpu: vec![id],
                });
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            WM_SIZE => {
                let mut rect: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut rect);
                page.scroll = page.scroll.min((page.height - rect.bottom).max(0));
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
                let target = if message == WM_MOUSEWHEEL {
                    page.scroll - ((wp >> 16) as i16 as i32) * 90 / 120
                } else {
                    match wp as i32 & 0xffff {
                        SB_LINEUP => page.scroll - 32,
                        SB_LINEDOWN => page.scroll + 32,
                        SB_PAGEUP => page.scroll - rect.bottom,
                        SB_PAGEDOWN => page.scroll + rect.bottom,
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
                page.scroll = target.clamp(0, (page.height - rect.bottom).max(0));
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
                SendMessageW(GetWindow(hwnd, GW_OWNER), WM_APP + 10, hwnd as usize, 0);
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
                    video: Some(crate::video_output::Snapshot {
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
            assert!(bus.requested());
            capture(hwnd, "stats-top");
            SendMessageW(hwnd, WM_VSCROLL, SB_BOTTOM as usize, 0);
            let page = &*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Page);
            assert!(page.scroll > 0);
            capture(hwnd, "stats-bottom");
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
        if let Some(path) = std::env::var_os("PARSEC_SETTINGS_SCREENSHOTS") {
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
    #[test]
    fn silence_retains_audio_format_and_unknowns_stay_unknown() {
        let sample = Sample {
            audio_ready: true,
            route: "Direct — no TURN",
            ..Default::default()
        };
        let rows = rows(&sample, &Values::default(), (None, None, None));
        assert!(rows.contains(&("Codec".into(), "Opus".into())));
        assert!(rows.contains(&("TURN server in use".into(), "None".into())));
        assert!(rows.iter().all(|(label, _)| ![
            "Configured bitrate",
            "Packets lost",
            "DTLS version / cipher",
            "GPU video decode"
        ]
        .contains(&label.as_str())));
    }
}
