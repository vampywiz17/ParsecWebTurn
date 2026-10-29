//! Absolute HID desktop input. Does not touch decoding or render surfaces.
use std::sync::Arc;
#[cfg(windows)]
pub const REFRESH: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 40;
pub fn absolute_motion(
    frame: &[u8],
    viewport: Option<crate::viewport::Viewport>,
) -> anyhow::Result<Option<bytes::Bytes>> {
    if frame.len() != 13 || frame[12] != 3 || frame[..4] != [0; 4] {
        return Ok(None);
    }
    let viewport = viewport.ok_or(crate::control::AbsoluteMouseUnavailable)?;
    anyhow::ensure!(
        viewport.rect().is_some(),
        "absolute HID requires a valid video viewport"
    );
    let axis = |offset: usize, size: u32| {
        let point = i32::from_be_bytes(frame[offset..offset + 4].try_into().unwrap());
        let span = i64::from(size.saturating_sub(1));
        if span == 0 {
            0
        } else {
            ((i64::from(point).clamp(0, span) * 32767 + span / 2) / span) as i32
        }
    };
    Ok(Some(crate::control::header(
        3,
        2,
        axis(4, viewport.source.0),
        axis(8, viewport.source.1),
    )))
}

pub struct Lifetime {
    window: Arc<crate::window::Window>,
    generation: u64,
}
impl Lifetime {
    pub fn new(window: Arc<crate::window::Window>, generation: u64) -> Self {
        Self { window, generation }
    }
}
impl Drop for Lifetime {
    fn drop(&mut self) {
        self.window
            .set_hid_input_active(self.generation, false, false);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absolute_hid_desktop_cannot_escape_normalized_primary_bounds() {
        let viewport = crate::viewport::Viewport {
            source: (1920, 1080),
            client: (1920, 1080),
        };
        for (point, expected) in [(i32::MIN, 0), (-1, 0), (i32::MAX, 32767)] {
            let packet = crate::control::header(3, 0, point, point);
            assert_eq!(
                absolute_motion(&packet, Some(viewport)).unwrap().unwrap(),
                crate::control::header(3, 2, expected, expected)
            );
        }
        // Camera deltas must bypass absolute coordinate clipping.
        let camera = crate::control::header(3, 1, -5000, 9000);
        assert!(absolute_motion(&camera, Some(viewport)).unwrap().is_none());
    }
    #[test]
    fn hid_input_absolute_normalization_uses_video_geometry_and_preserves_other_commands() {
        let viewport = crate::viewport::Viewport {
            source: (1920, 1080),
            client: (1024, 768),
        };
        for (x, y, expected) in [(0, 96, (0, 0)), (1024, 672, (32767, 32767))] {
            let (source_x, source_y) = viewport.map(x, y).unwrap();
            let packet = crate::control::header(3, 0, source_x, source_y);
            assert_eq!(
                absolute_motion(&packet, Some(viewport)).unwrap().unwrap(),
                crate::control::header(3, 2, expected.0, expected.1)
            );
        }
        assert!(absolute_motion(&crate::control::header(1, 1, 1, 0), None)
            .unwrap()
            .is_none());
        assert!(
            absolute_motion(&crate::control::header(3, 1, 100, 200), None)
                .unwrap()
                .is_none()
        );
        assert!(absolute_motion(&crate::control::header(3, 0, 100, 200), None).is_err());
    }
}
