//! Cheap control-plane formatting; no frame work or GPU queries.
use serde_json::{json, Value};
pub const AUTO_HIDE_MS: u64 = 5000;
pub fn metrics(
    fps: Option<f64>,
    rtt: Option<f64>,
    mbps: f64,
    codec: &str,
    width: u32,
    height: u32,
    depth: u32,
) -> Value {
    let value = |n: Option<f64>, unit: &str| {
        n.filter(|v| v.is_finite() && *v >= 0.)
            .map(|v| format!("{v:.1} {unit}"))
            .unwrap_or_else(|| format!("N/A {unit}"))
    };
    json!({"fps":value(fps,"FPS"),"rtt":value(rtt,"ms"),"bitrate":value(Some(mbps),"Mbit/s"),"format":format!("{codec} | {width} x {height} | {depth}-bit"),"auto_hide_ms":AUTO_HIDE_MS})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_and_invalid_metrics_remain_unknown() {
        let m = metrics(None, Some(f64::NAN), 12.5, "HEVC", 1920, 1080, 10);
        assert_eq!(m["rtt"], "N/A ms");
        assert_eq!(m["bitrate"], "12.5 Mbit/s");
    }
}

#[cfg(target_os = "android")]
pub static REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
