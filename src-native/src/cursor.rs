//! Native cursor messages, backed by documented Win32 or AppKit cursor APIs.
//! Cursor image buffers are copied; window cursor ownership stays on its UI thread.

use anyhow::{bail, Result};
#[cfg(windows)]
use windows_sys::Win32::{Graphics::Gdi::*, UI::WindowsAndMessaging::*};

const MAX_SIDE: u32 = 256;
const MAX_LOCAL_SIDE: u32 = 512;
const MAX_ENCODED: usize = 1024 * 1024;

pub struct Frame {
    width: u32,
    height: u32,
    hot_x: u32,
    hot_y: u32,
    rgba: Vec<u8>,
}
impl Frame {
    pub fn native_png(bytes: &[u8], x: i32, y: i32) -> Result<Self> {
        Self::png(bytes.to_vec(), x, y)
    }
    fn rgba(bytes: Vec<u8>, width: u32, height: u32, x: i32, y: i32) -> Result<Self> {
        let size = Self::size(width, height)?;
        if bytes.len() != size {
            bail!("cursor pixel length mismatch");
        }
        Ok(Self {
            width,
            height,
            hot_x: x.clamp(0, width as i32 - 1) as u32,
            hot_y: y.clamp(0, height as i32 - 1) as u32,
            rgba: bytes,
        })
    }
    fn size(width: u32, height: u32) -> Result<usize> {
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            bail!("cursor dimensions exceed limit");
        }
        Ok(width as usize * height as usize * 4)
    }
    fn png(bytes: Vec<u8>, x: i32, y: i32) -> Result<Self> {
        if bytes.len() > MAX_ENCODED {
            bail!("encoded cursor exceeds limit");
        }
        let mut reader =
            image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_SIDE);
        limits.max_image_height = Some(MAX_SIDE);
        limits.max_alloc = Some(2 * 1024 * 1024);
        reader.limits(limits);
        let image = reader.decode()?.into_rgba8();
        Self::rgba(image.as_raw().clone(), image.width(), image.height(), x, y)
    }
    fn fit_local(&self, reference: (u32, u32)) -> Geometry {
        let (rw, rh) = (
            reference.0.clamp(1, MAX_LOCAL_SIDE),
            reference.1.clamp(1, MAX_LOCAL_SIDE),
        );
        let rounded = |value: u32, target: u32, source: u32| {
            ((u64::from(value) * u64::from(target) + u64::from(source) / 2) / u64::from(source))
                as u32
        };
        let (width, height) =
            if u64::from(self.width) * u64::from(rh) >= u64::from(self.height) * u64::from(rw) {
                (rw, rounded(self.height, rw, self.width).max(1))
            } else {
                (rounded(self.width, rh, self.height).max(1), rh)
            };
        Geometry {
            width,
            height,
            hot_x: rounded(self.hot_x, width, self.width).min(width - 1),
            hot_y: rounded(self.hot_y, height, self.height).min(height - 1),
        }
    }
    fn resized(&self, geometry: Geometry) -> Self {
        let source = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(
            self.width,
            self.height,
            self.rgba.as_slice(),
        )
        .expect("validated cursor pixels");
        let rgba = image::imageops::resize(
            &source,
            geometry.width,
            geometry.height,
            image::imageops::FilterType::Triangle,
        )
        .into_raw();
        Self {
            width: geometry.width,
            height: geometry.height,
            hot_x: geometry.hot_x,
            hot_y: geometry.hot_y,
            rgba,
        }
    }
    #[cfg(windows)]
    fn bgra(&self) -> Vec<u8> {
        self.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| {
                let premultiply = |c: u8| ((u32::from(c) * u32::from(p[3]) + 127) / 255) as u8;
                [
                    premultiply(p[2]),
                    premultiply(p[1]),
                    premultiply(p[0]),
                    p[3],
                ]
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Geometry {
    width: u32,
    height: u32,
    hot_x: u32,
    hot_y: u32,
}

pub enum Request {
    Image(Option<Frame>),
    Visible(bool),
}
#[derive(Default)]
pub struct Pending {
    image: Option<Option<Frame>>,
    visible: Option<bool>,
}
impl Pending {
    pub fn submit(&mut self, request: Request) {
        match request {
            Request::Image(image) => self.image = Some(image),
            Request::Visible(visible) => self.visible = Some(visible),
        }
    }
}

#[cfg(windows)]
struct OwnedCursor(usize);
#[cfg(windows)]
impl Drop for OwnedCursor {
    fn drop(&mut self) {
        unsafe {
            // Never destroy the currently selected cursor. Shared system cursors
            // are borrowed and are never stored in OwnedCursor.
            if GetCursor() == self.0 as HCURSOR {
                SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_ARROW));
            }
            DestroyCursor(self.0 as HCURSOR);
        }
    }
}
#[cfg(windows)]
struct Bitmap(HBITMAP);
#[cfg(windows)]
impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.0);
        }
    }
}

#[cfg(windows)]
fn create(frame: &Frame) -> Result<OwnedCursor> {
    // SAFETY: bounded, top-down 32bpp DIB; only owned initialized buffers are
    // copied. CreateIconIndirect copies the bitmaps, so both are released here.
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: frame.width as i32,
                biHeight: -(frame.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            },
            ..std::mem::zeroed()
        };
        let mut bits = std::ptr::null_mut();
        let color = Bitmap(CreateDIBSection(
            std::ptr::null_mut(),
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        ));
        if color.0.is_null() || bits.is_null() {
            bail!("cursor color bitmap unavailable");
        }
        let bgra = frame.bgra();
        std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits.cast::<u8>(), bgra.len());
        // CreateBitmap takes WORD-aligned scan lines, with top-down source rows.
        let stride = (frame.width as usize).div_ceil(16) * 2;
        let mut mask = vec![0u8; stride * frame.height as usize];
        for (i, pixel) in frame.rgba.as_chunks::<4>().0.iter().enumerate() {
            if pixel[3] == 0 {
                mask[(i / frame.width as usize) * stride + (i % frame.width as usize) / 8] |=
                    0x80 >> (i % frame.width as usize % 8);
            }
        }
        let mask = Bitmap(CreateBitmap(
            frame.width as i32,
            frame.height as i32,
            1,
            1,
            mask.as_ptr().cast(),
        ));
        if mask.0.is_null() {
            bail!("cursor mask bitmap unavailable");
        }
        let info = ICONINFO {
            fIcon: 0,
            xHotspot: frame.hot_x,
            yHotspot: frame.hot_y,
            hbmMask: mask.0,
            hbmColor: color.0,
        };
        let cursor = CreateIconIndirect(&info);
        if cursor.is_null() {
            bail!("native cursor unavailable");
        }
        Ok(OwnedCursor(cursor as usize))
    }
}

#[cfg(windows)]
// GetIconInfo reports native bitmap pixels, without DPI virtualization. All
// returned bitmaps are owned copies; the system cursor handle stays borrowed.
fn cursor_size(cursor: HCURSOR) -> Result<(u32, u32)> {
    unsafe {
        let mut info: ICONINFO = std::mem::zeroed();
        if GetIconInfo(cursor as HICON, &mut info) == 0 {
            bail!("cursor size unavailable");
        }
        let color = Bitmap(info.hbmColor);
        let mask = Bitmap(info.hbmMask);
        let mut bitmap: BITMAP = std::mem::zeroed();
        let monochrome = color.0.is_null();
        let handle = if monochrome { mask.0 } else { color.0 };
        if GetObjectW(
            handle,
            std::mem::size_of::<BITMAP>() as i32,
            (&mut bitmap as *mut BITMAP).cast(),
        ) == 0
        {
            bail!("cursor bitmap size unavailable");
        }
        let height = bitmap.bmHeight / if monochrome { 2 } else { 1 };
        if bitmap.bmWidth <= 0 || height <= 0 {
            bail!("invalid cursor size");
        }
        Ok((bitmap.bmWidth as u32, height as u32))
    }
}
#[cfg(windows)]
fn local_size() -> Result<(u32, u32)> {
    cursor_size(unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) })
}
#[cfg(windows)]
fn create_local(frame: &Frame, geometry: Geometry) -> Result<OwnedCursor> {
    if (geometry.width, geometry.height) == (frame.width, frame.height) {
        create(frame)
    } else {
        create(&frame.resized(geometry))
    }
}
#[cfg(windows)]
pub struct State {
    custom: Option<OwnedCursor>,
    source: Option<Frame>,
    geometry: Option<Geometry>,
    visible: bool,
}
#[cfg(windows)]
impl Default for State {
    fn default() -> Self {
        Self {
            custom: None,
            source: None,
            geometry: None,
            visible: true,
        }
    }
}
#[cfg(windows)]
impl State {
    pub fn apply(&mut self, pending: Pending) {
        if let Some(visible) = pending.visible {
            self.visible = visible;
        }
        match pending.image {
            Some(Some(frame)) => {
                match local_size().and_then(|size| {
                    let geometry = frame.fit_local(size);
                    create_local(&frame, geometry).map(|cursor| (cursor, geometry, size))
                }) {
                    Ok((cursor, geometry, reference)) => {
                        if crate::event_log::enabled() {
                            crate::event_log::record(
                                "INFO",
                                "cursor_image_applied",
                                serde_json::json!({
                                    "source_size":[frame.width,frame.height], "local_reference":reference,
                                    "output_size":[geometry.width,geometry.height], "hotspot":[geometry.hot_x,geometry.hot_y]
                                }),
                            );
                        }
                        self.source = Some(frame);
                        self.geometry = Some(geometry);
                        self.replace(Some(cursor));
                    }
                    Err(_) => crate::event_log::record(
                        "WARN",
                        "cursor_image_creation_failed",
                        serde_json::json!({}),
                    ),
                }
            }
            Some(None) => {
                self.source = None;
                self.geometry = None;
                self.replace(None);
            }
            None => {}
        }
    }
    // Only on Windows cursor/display settings notifications, never on a
    // mouse move, video frame, or visibility/camera transition.
    pub fn refresh_size(&mut self) {
        let Some(frame) = self.source.as_ref() else {
            return;
        };
        let Ok(reference) = local_size() else {
            return;
        };
        let geometry = frame.fit_local(reference);
        if self.geometry == Some(geometry) {
            return;
        }
        match create_local(frame, geometry) {
            Ok(cursor) => {
                self.geometry = Some(geometry);
                self.replace(Some(cursor));
            }
            Err(_) => crate::event_log::record(
                "WARN",
                "cursor_image_creation_failed",
                serde_json::json!({}),
            ),
        }
    }
    fn replace(&mut self, cursor: Option<OwnedCursor>) {
        let retired = std::mem::replace(&mut self.custom, cursor);
        if retired
            .as_ref()
            .is_some_and(|old| unsafe { GetCursor() == old.0 as HCURSOR })
        {
            self.select();
        }
        drop(retired);
    }
    pub fn select(&self) {
        unsafe {
            SetCursor(self.handle());
        }
    }
    fn handle(&self) -> HCURSOR {
        if !self.visible {
            return std::ptr::null_mut();
        }
        if let Some(cursor) = &self.custom {
            return cursor.0 as HCURSOR;
        }
        unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn selected_cursor_replacement_never_uses_the_retirement_arrow() {
        struct Restore(HCURSOR);
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe { SetCursor(self.0) };
            }
        }
        let _restore = Restore(unsafe { GetCursor() });
        let mut state = State::default();
        let replacement = || {
            let mut pending = Pending::default();
            pending.submit(Request::Image(Some(
                Frame::rgba(vec![255; 16], 2, 2, 0, 0).unwrap(),
            )));
            pending
        };
        state.apply(replacement());
        state.select();
        let old = unsafe { GetCursor() };
        assert!(!old.is_null());
        state.apply(replacement());
        assert_ne!(state.handle(), old);
        assert_eq!(unsafe { GetCursor() }, state.handle());

        // A replacement must not reveal a cursor hidden by relative capture.
        unsafe { SetCursor(std::ptr::null_mut()) };
        state.apply(replacement());
        assert!(unsafe { GetCursor() }.is_null());
        state.select();
        let mut pending = replacement();
        pending.submit(Request::Visible(false));
        state.apply(pending);
        assert!(unsafe { GetCursor() }.is_null());
    }
    #[test]
    fn bounded_pixels_hotspots_and_alpha() {
        let f = Frame::rgba(vec![200, 100, 50, 128], 1, 1, -1, 999).unwrap();
        assert_eq!((f.hot_x, f.hot_y), (0, 0));
        assert_eq!(f.bgra(), [25, 50, 100, 128]);
        for (w, h) in [(0, 1), (1, 0), (257, 1), (u32::MAX, 1)] {
            assert!(Frame::size(w, h).is_err());
        }
        assert!(Frame::rgba(vec![], 1, 1, 0, 0).is_err());
        assert!(Frame::png(vec![0; 16], 0, 0).is_err());
    }
    #[test]
    fn png_and_rgba_native_cursor_lifecycle() {
        let image = image::RgbaImage::from_pixel(17, 3, image::Rgba([200, 100, 50, 128]));
        let mut encoded = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let frame = Frame::png(encoded.into_inner(), 4, 2).unwrap();
        let mut state = State::default();
        for _ in 0..80 {
            let mut pending = Pending::default();
            pending.submit(Request::Image(Some(
                Frame::rgba(frame.rgba.clone(), 17, 3, 4, 2).unwrap(),
            )));
            state.apply(pending);
            let custom = state.custom.as_ref().expect("native cursor creation").0;
            unsafe {
                let mut info: ICONINFO = std::mem::zeroed();
                assert_ne!(GetIconInfo(custom as HICON, &mut info), 0);
                let _color = Bitmap(info.hbmColor);
                let _mask = Bitmap(info.hbmMask);
                let geometry = frame.fit_local(local_size().unwrap());
                assert_eq!(
                    (info.fIcon, info.xHotspot, info.yHotspot),
                    (0, geometry.hot_x, geometry.hot_y)
                );
                assert_eq!(
                    cursor_size(custom as HCURSOR).unwrap(),
                    (geometry.width, geometry.height)
                );
            }
            let mut pending = Pending::default();
            pending.submit(Request::Image(None));
            state.apply(pending);
            assert_ne!(state.handle() as usize, custom);
            let mut pending = Pending::default();
            pending.submit(Request::Visible(false));
            state.apply(pending);
            assert!(state.handle().is_null());
            let mut pending = Pending::default();
            pending.submit(Request::Image(None));
            pending.submit(Request::Visible(true));
            state.apply(pending);
            assert!(state.custom.is_none());
            assert!(!state.handle().is_null());
        }
    }
    #[test]
    fn local_cursor_geometry_is_independent_of_host_size_and_preserves_hotspot() {
        let frame = Frame::rgba(vec![255; 64 * 64 * 4], 64, 64, 16, 32).unwrap();
        let geometry = frame.fit_local((32, 32));
        assert_eq!(
            geometry,
            Geometry {
                width: 32,
                height: 32,
                hot_x: 8,
                hot_y: 16
            }
        );
        let cursor = create_local(&frame, geometry).unwrap();
        assert_eq!(cursor_size(cursor.0 as HCURSOR).unwrap(), (32, 32));
        let wide = Frame::rgba(vec![255; 64 * 32 * 4], 64, 32, 63, 31).unwrap();
        assert_eq!(
            wide.fit_local((32, 32)),
            Geometry {
                width: 32,
                height: 16,
                hot_x: 31,
                hot_y: 15
            }
        );
        assert_eq!(
            frame.fit_local((48, 48)),
            Geometry {
                width: 48,
                height: 48,
                hot_x: 12,
                hot_y: 24
            }
        );
        let mut state = State::default();
        let mut pending = Pending::default();
        pending.submit(Request::Image(Some(frame)));
        state.apply(pending);
        let original = state.custom.as_ref().unwrap().0;
        state.refresh_size();
        assert_eq!(
            state.custom.as_ref().unwrap().0,
            original,
            "unchanged settings must not rebuild the cursor"
        );
    }
}

#[cfg(target_os = "macos")]
#[path = "cursor_macos.rs"]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::State;
