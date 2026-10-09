//! Pinned Matoya cursor ABI, backed by documented Win32 cursor APIs.
//! Guest image buffers are copied; all window cursor ownership stays on its UI thread.
use crate::memory::GuestMemory;
use anyhow::{bail, Context, Result};
use windows_sys::Win32::{Graphics::Gdi::*, UI::WindowsAndMessaging::*};

const MAX_SIDE: u32 = 256;
const MAX_ENCODED: usize = 1024 * 1024;

pub struct Frame {
    width: u32,
    height: u32,
    hot_x: u32,
    hot_y: u32,
    rgba: Vec<u8>,
}
impl Frame {
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

pub enum Request {
    Image(Option<Frame>),
    Visible(bool),
    Default(bool),
}
#[derive(Default)]
pub struct Pending {
    image: Option<Option<Frame>>,
    visible: Option<bool>,
    default: Option<bool>,
}
impl Pending {
    pub fn submit(&mut self, request: Request) {
        match request {
            Request::Image(image) => self.image = Some(image),
            Request::Visible(visible) => self.visible = Some(visible),
            Request::Default(default) => self.default = Some(default),
        }
    }
}

pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "web_set_png_cursor" | "web_set_rgba_cursor" | "web_show_cursor" | "web_use_default_cursor"
    )
}
pub fn decode(memory: &GuestMemory, name: &str, args: &[wasmtime::Val]) -> Result<Request> {
    let int = |i| {
        args.get(i)
            .and_then(wasmtime::Val::i32)
            .context("cursor ABI expected i32")
    };
    Ok(match name {
        "web_show_cursor" => Request::Visible(int(0)? != 0),
        "web_use_default_cursor" => Request::Default(int(0)? != 0),
        "web_set_png_cursor" | "web_set_rgba_cursor" if int(0)? == 0 => Request::Image(None),
        "web_set_png_cursor" => {
            let size = usize::try_from(int(1)?)?;
            if size > MAX_ENCODED {
                bail!("encoded cursor exceeds limit");
            }
            Request::Image(Some(Frame::png(
                memory.read(int(0)? as u32, size)?,
                int(2)?,
                int(3)?,
            )?))
        }
        "web_set_rgba_cursor" => {
            let (w, h) = (int(1)? as u32, int(2)? as u32);
            Request::Image(Some(Frame::rgba(
                memory.read(int(0)? as u32, Frame::size(w, h)?)?,
                w,
                h,
                int(3)?,
                int(4)?,
            )?))
        }
        _ => bail!("unknown cursor import"),
    })
}

struct OwnedCursor(usize);
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
struct Bitmap(HBITMAP);
impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.0);
        }
    }
}

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

pub struct State {
    custom: Option<OwnedCursor>,
    visible: bool,
    default: bool,
}
impl Default for State {
    fn default() -> Self {
        Self {
            custom: None,
            visible: true,
            default: false,
        }
    }
}
impl State {
    pub fn apply(&mut self, pending: Pending) {
        if let Some(image) = pending.image {
            self.custom = image.as_ref().and_then(|frame| create(frame).ok());
        }
        if let Some(visible) = pending.visible {
            self.visible = visible;
        }
        if let Some(default) = pending.default {
            self.default = default;
        }
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
        if !self.default {
            if let Some(cursor) = &self.custom {
                return cursor.0 as HCURSOR;
            }
        }
        unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
                assert_eq!((info.fIcon, info.xHotspot, info.yHotspot), (0, 4, 2));
            }
            let mut pending = Pending::default();
            pending.submit(Request::Default(true));
            state.apply(pending);
            assert_ne!(state.handle() as usize, custom);
            let mut pending = Pending::default();
            pending.submit(Request::Visible(false));
            state.apply(pending);
            assert!(state.handle().is_null());
            let mut pending = Pending::default();
            pending.submit(Request::Image(None));
            pending.submit(Request::Visible(true));
            pending.submit(Request::Default(false));
            state.apply(pending);
            assert!(state.custom.is_none());
            assert!(!state.handle().is_null());
        }
    }
    #[test]
    fn actual_guest_imports_allow_reset_and_reject_bad_images_without_trapping() {
        let mut config = wasmtime::Config::new();
        config
            .wasm_threads(true)
            .consume_fuel(true)
            .epoch_interruption(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        let module = wasmtime::Module::new(
            &engine,
            r#"(module
            (import "env" "memory" (memory 1 1 shared))
            (import "env" "web_set_png_cursor" (func $png (param i32 i32 i32 i32)))
            (import "env" "web_set_rgba_cursor" (func $rgba (param i32 i32 i32 i32 i32)))
            (import "env" "web_show_cursor" (func $show (param i32)))
            (import "env" "web_use_default_cursor" (func $default (param i32)))
            (func (export "test")
                i32.const 0 i32.const -1 i32.const 0 i32.const 0 call $png
                i32.const 65535 i32.const 16 i32.const 0 i32.const 0 call $png
                i32.const 65535 i32.const 32 i32.const 32 i32.const 0 i32.const 0 call $rgba
                i32.const 0 call $show i32.const 1 call $show
                i32.const 1 call $default i32.const 0 call $default))"#,
        )
        .unwrap();
        let (mut store, instance) = crate::instantiate(&engine, &module).unwrap();
        instance
            .get_typed_func::<(), ()>(&mut store, "test")
            .unwrap()
            .call(&mut store, ())
            .unwrap();
        assert!(store.data().boundary.is_none());
        assert_eq!(store.data().calls["env::web_set_png_cursor"], 2);
        assert_eq!(store.data().calls["env::web_set_rgba_cursor"], 1);
    }
}
