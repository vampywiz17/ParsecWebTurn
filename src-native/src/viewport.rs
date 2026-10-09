//! Shared letterbox geometry for GPU presentation and absolute mouse input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub source: (u32, u32),
    pub client: (u32, u32),
}
impl Viewport {
    pub fn rect(self) -> Option<[i32; 4]> {
        let (sw, sh) = self.source;
        let (cw, ch) = self.client;
        if [sw, sh, cw, ch].iter().any(|n| *n == 0 || *n > 16384) {
            return None;
        }
        let scale = (cw as f64 / sw as f64).min(ch as f64 / sh as f64);
        let (w, h) = (
            (sw as f64 * scale).round() as i32,
            (sh as f64 * scale).round() as i32,
        );
        if w == 0 || h == 0 {
            return None;
        }
        Some([
            (cw as i32 - w) / 2,
            (ch as i32 - h) / 2,
            (cw as i32 + w) / 2,
            (ch as i32 + h) / 2,
        ])
    }
    pub fn map(self, x: i32, y: i32) -> Option<(i32, i32)> {
        let [l, t, r, b] = self.rect()?;
        let axis = |v: i32, origin: i32, span: i32, source: u32| {
            let v = ((v as f64 - origin as f64) * source as f64 / span as f64).round() as i64;
            // Preserve the pinned browser adapter's inclusive right/bottom edge.
            if v == source as i64 - 1 {
                source as i32
            } else {
                v.clamp(0, source as i64) as i32
            }
        };
        Some((
            axis(x, l, r - l, self.source.0),
            axis(y, t, b - t, self.source.1),
        ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_uses_the_presented_letterbox_and_clamps_edges() {
        let v = Viewport {
            source: (1920, 1080),
            client: (1024, 768),
        };
        assert_eq!(v.rect(), Some([0, 96, 1024, 672]));
        assert_eq!(v.map(512, 384), Some((960, 540)));
        assert_eq!(v.map(i32::MIN, i32::MAX), Some((0, 1080)));
        assert_eq!(
            Viewport {
                client: (0, 0),
                ..v
            }
            .map(0, 0),
            None
        );
        let v = Viewport {
            source: (800, 600),
            client: (1920, 1080),
        };
        assert_eq!(v.rect(), Some([240, 0, 1680, 1080]));
        assert_eq!(v.map(240, 0), Some((0, 0)));
        assert_eq!(v.map(1680, 1080), Some((800, 600)));
    }
}
