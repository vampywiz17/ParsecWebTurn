//! Read-only presentation advice. No scheduling, frame processing or OS calls.
#[derive(Default)]
pub struct Display {
    /// Configured target refresh, not proof of live VRR/scanout frequency.
    pub hz: Option<f64>,
    /// Windows-enumerated progressive rates at the current resolution/depth.
    pub supported_hz: Vec<u32>,
}

#[derive(Debug, PartialEq, Eq)]
enum Cadence {
    Aligned,
    Near,
    Uneven,
    Unknown,
}
fn cadence(hz: f64, fps: u32) -> Cadence {
    if !hz.is_finite() || hz < 1.0 || fps == 0 {
        return Cadence::Unknown;
    }
    let ratio = hz / f64::from(fps);
    let multiple = ratio.round();
    if multiple < 1.0 {
        return Cadence::Uneven;
    }
    let error = (ratio / multiple - 1.0).abs();
    if error < 0.000_01 {
        Cadence::Aligned
    } else if error < 0.002 {
        Cadence::Near
    } else {
        Cadence::Uneven
    }
}
fn rate(value: Option<f64>, suffix: &str) -> String {
    match value.filter(|v| v.is_finite() && *v > 0.0) {
        Some(value) => format!(
            "{} {suffix}",
            format!("{value:.2}")
                .trim_end_matches('0')
                .trim_end_matches('.')
        ),
        None => "unknown".into(),
    }
}
impl Display {
    pub fn advice(
        &self,
        requested: u32,
        host_fps: Option<u32>,
        measured_fps: Option<f64>,
        vrr_requested: bool,
    ) -> String {
        let mut lines = vec![
            format!(
                "Display: {} | Request: {requested} FPS",
                rate(self.hz, "Hz")
            ),
            format!(
                "Host setting: {} | Decoded: {}",
                host_fps.map_or("unknown".into(), |n| n.to_string()),
                rate(measured_fps, "FPS")
            ),
        ];
        // Use the host's reported setting for cadence, never an idle-dependent
        // measured FPS or an unapplied combo selection when the host is known.
        let fps = host_fps.unwrap_or(requested);
        if vrr_requested {
            lines.push("VRR requested; monitor activation unverified.".into());
        } else {
            match self
                .hz
                .map(|hz| cadence(hz, fps))
                .unwrap_or(Cadence::Unknown)
            {
                Cadence::Aligned => {
                    lines.push("Fixed-refresh ratio aligns; clocks may drift.".into())
                }
                Cadence::Near => lines.push("Nearly aligned; clock drift is possible.".into()),
                Cadence::Uneven => {
                    lines.push("Fixed-refresh frame cadence may be uneven.".into());
                    let alternative = self
                        .supported_hz
                        .iter()
                        .copied()
                        .filter(|hz| cadence(f64::from(*hz), fps) == Cadence::Aligned)
                        .min_by(|a, b| {
                            (f64::from(*a) - self.hz.unwrap())
                                .abs()
                                .total_cmp(&(f64::from(*b) - self.hz.unwrap()).abs())
                        });
                    lines.push(alternative.map_or_else(
                        || "Compatible VRR may help; no changes are automatic.".into(),
                        |hz| format!("Consider supported {hz} Hz, or compatible VRR."),
                    ));
                }
                Cadence::Unknown => {
                    lines.push("Display refresh unavailable; cadence unknown.".into())
                }
            }
        }
        if requested > 60 || host_fps.is_some_and(|fps| fps > 60) {
            lines.push("Above 60 FPS needs a capable host display/mode.".into());
        }
        if measured_fps.is_some_and(|measured| {
            measured.is_finite() && measured > 0.0 && measured < f64::from(fps) * 0.85
        }) {
            lines.push("Lower FPS can reflect static content or host limits.".into());
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_refresh_ratios_preserve_fractional_rates_and_unknowns() {
        for hz in [60.0, 120.0, 240.0] {
            assert_eq!(cadence(hz, 60), Cadence::Aligned);
        }
        for hz in [144.0, 165.0] {
            assert_eq!(cadence(hz, 60), Cadence::Uneven);
        }
        assert_eq!(cadence(60.0, 120), Cadence::Uneven);
        assert_eq!(cadence(60000.0 / 1001.0, 60), Cadence::Near);
        for hz in [f64::NAN, f64::INFINITY, 0.0] {
            assert_eq!(cadence(hz, 60), Cadence::Unknown);
        }
        assert_eq!(cadence(165.0, 0), Cadence::Unknown);
    }
    #[test]
    fn advice_uses_reported_setting_and_only_supported_alternatives() {
        let d = Display {
            hz: Some(165.0),
            supported_hz: vec![60, 120, 144, 165],
        };
        let text = d.advice(120, Some(60), Some(30.0), false);
        assert!(text.contains("Request: 120 FPS"));
        assert!(text.contains("Host setting: 60"));
        assert!(text.contains("supported 120 Hz"));
        assert!(text.contains("static content or host limits"));
        let d = Display {
            hz: Some(144.0),
            supported_hz: vec![144],
        };
        let text = d.advice(60, Some(60), None, false);
        assert!(!text.contains("120 Hz"));
        assert!(text.contains("Decoded: unknown"));
        let text = d.advice(60, Some(60), Some(60.0), true);
        assert!(text.contains("activation unverified"));
        assert!(!text.contains("cadence may be uneven"));
        assert!(Display::default()
            .advice(60, None, None, false)
            .contains("cadence unknown"));
    }
}
