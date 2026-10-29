//! Documented AppKit NSCursor images; created, selected and released on the
//! main thread only. Host images keep their shape/hotspot proportions at the
//! local arrow cursor's size, as on Windows.
use super::{Frame, Geometry, Pending};
use objc2::{rc::Retained, AnyThread, MainThreadMarker};
use objc2_app_kit::{NSBitmapImageRep, NSCursor, NSImage};
use objc2_foundation::{NSPoint, NSSize, NSString};

pub struct State {
    custom: Option<Retained<NSCursor>>,
    blank: Option<Retained<NSCursor>>,
    source: Option<Frame>,
    geometry: Option<Geometry>,
    visible: bool,
}
impl Default for State {
    fn default() -> Self {
        Self {
            custom: None,
            blank: None,
            source: None,
            geometry: None,
            visible: true,
        }
    }
}

/// Backing pixels of the local arrow cursor and the screen scale they use.
fn local_size(mtm: MainThreadMarker) -> ((u32, u32), f64) {
    let scale = objc2_app_kit::NSScreen::mainScreen(mtm).map_or(1.0, |s| s.backingScaleFactor());
    let size = NSCursor::arrowCursor().image().size();
    (
        (
            (size.width * scale).round().max(1.0) as u32,
            (size.height * scale).round().max(1.0) as u32,
        ),
        scale,
    )
}

fn create(frame: &Frame, scale: f64) -> Option<Retained<NSCursor>> {
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            frame.width as isize,
            frame.height as isize,
            8,
            4,
            true,
            false,
            &NSString::from_str("NSDeviceRGBColorSpace"),
            objc2_app_kit::NSBitmapFormat::AlphaNonpremultiplied,
            frame.width as isize * 4,
            32,
        )
    }?;
    let pixels = rep.bitmapData();
    if pixels.is_null() {
        return None;
    }
    unsafe { std::ptr::copy_nonoverlapping(frame.rgba.as_ptr(), pixels, frame.rgba.len()) };
    let points = NSSize::new(
        f64::from(frame.width) / scale,
        f64::from(frame.height) / scale,
    );
    rep.setSize(points);
    let image = NSImage::initWithSize(NSImage::alloc(), points);
    image.addRepresentation(&rep);
    let hotspot = NSPoint::new(
        f64::from(frame.hot_x) / scale,
        f64::from(frame.hot_y) / scale,
    );
    Some(NSCursor::initWithImage_hotSpot(
        NSCursor::alloc(),
        &image,
        hotspot,
    ))
}

impl State {
    pub fn apply(&mut self, pending: Pending, mtm: MainThreadMarker) {
        if let Some(visible) = pending.visible {
            self.visible = visible;
        }
        match pending.image {
            Some(Some(frame)) => {
                self.source = Some(frame);
                self.geometry = None;
                self.refresh_size(mtm);
            }
            Some(None) => {
                self.source = None;
                self.geometry = None;
                self.custom = None;
            }
            None => {}
        }
    }
    /// Recreate only when the local reference size changes (display scale).
    pub fn refresh_size(&mut self, mtm: MainThreadMarker) {
        let Some(frame) = self.source.as_ref() else {
            return;
        };
        let (reference, scale) = local_size(mtm);
        let geometry = frame.fit_local(reference);
        if self.geometry == Some(geometry) {
            return;
        }
        let resized;
        let local = if (geometry.width, geometry.height) == (frame.width, frame.height) {
            frame
        } else {
            resized = frame.resized(geometry);
            &resized
        };
        match create(local, scale) {
            Some(cursor) => {
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
                self.geometry = Some(geometry);
                self.custom = Some(cursor);
            }
            None => crate::event_log::record(
                "WARN",
                "cursor_image_creation_failed",
                serde_json::json!({}),
            ),
        }
    }
    /// A transparent cursor hides the pointer without NSCursor's hide counter.
    pub fn select(&mut self, hidden: bool) {
        if hidden || !self.visible {
            let blank = self.blank.get_or_insert_with(|| {
                let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(1.0, 1.0));
                NSCursor::initWithImage_hotSpot(NSCursor::alloc(), &image, NSPoint::ZERO)
            });
            blank.set();
        } else if let Some(cursor) = &self.custom {
            cursor.set();
        } else {
            NSCursor::arrowCursor().set();
        }
    }
}
