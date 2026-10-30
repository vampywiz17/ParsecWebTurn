//! Android TV request presets. Rust owns validation; the TV view only renders
//! these choices. Decoder support and actual host replies are separate checks.
use crate::session_preferences::{PanelFields, BANDWIDTH_CHOICES, FPS_CHOICES, RESOLUTION_CHOICES};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub transport: usize,
    pub video: usize,
    pub resolution: usize,
    pub fps: usize,
    pub bandwidth: usize,
}

const TRANSPORT: &[&str] = &["WebRTC", "BUD"];
const VIDEO: &[&str] = &["H.264 · 8-bit SDR", "HEVC · 8-bit SDR", "HEVC · 10-bit SDR"];
type ChoiceField = (&'static str, &'static str, &'static [&'static str], usize);

impl Default for Preferences {
    fn default() -> Self {
        Self {
            transport: 0,
            video: 0,
            resolution: 0,
            fps: FPS_CHOICES.iter().position(|s| *s == "60").unwrap(),
            bandwidth: BANDWIDTH_CHOICES.iter().position(|s| *s == "20").unwrap(),
        }
    }
}

impl Preferences {
    fn fields(&self) -> [ChoiceField; 5] {
        [
            ("transport", "Transport", TRANSPORT, self.transport),
            ("video", "Video request", VIDEO, self.video),
            (
                "resolution",
                "Resolution request",
                RESOLUTION_CHOICES,
                self.resolution,
            ),
            ("fps", "Frame rate request (FPS)", FPS_CHOICES, self.fps),
            (
                "bandwidth",
                "Bandwidth limit request (Mbps)",
                BANDWIDTH_CHOICES,
                self.bandwidth,
            ),
        ]
    }

    pub fn parse(text: &str) -> Result<Self> {
        ensure!(text.len() <= 4096, "Settings exceed limit.");
        let value: Self = serde_json::from_str(text)?;
        for (_, _, choices, index) in value.fields() {
            ensure!(index < choices.len(), "Invalid TV setting.");
        }
        // Reuse the Windows panel's parsing/limits rather than duplicating them.
        value.host_request()?;
        Ok(value)
    }

    pub fn select(&mut self, field: &str, index: usize) -> Result<()> {
        let choices = self
            .fields()
            .into_iter()
            .find(|(key, _, _, _)| *key == field)
            .ok_or_else(|| anyhow::anyhow!("Unknown TV setting."))?
            .2;
        ensure!(index < choices.len(), "Invalid TV setting.");
        match field {
            "transport" => self.transport = index,
            "video" => self.video = index,
            "resolution" => self.resolution = index,
            "fps" => self.fps = index,
            "bandwidth" => self.bandwidth = index,
            _ => unreachable!(),
        }
        Ok(())
    }

    pub fn view(&self) -> Value {
        json!({"ok":true, "settings":self, "rows":self.fields().map(|(key,label,choices,selected)|
            json!({"key":key,"label":label,"choices":choices,"selected":selected}))})
    }

    pub fn host_request(&self) -> Result<crate::session_preferences::Video> {
        PanelFields {
            resolution: RESOLUTION_CHOICES
                .get(self.resolution)
                .ok_or_else(|| anyhow::anyhow!("Invalid resolution."))?,
            bandwidth: BANDWIDTH_CHOICES
                .get(self.bandwidth)
                .ok_or_else(|| anyhow::anyhow!("Invalid bandwidth."))?,
            fps: FPS_CHOICES
                .get(self.fps)
                .ok_or_else(|| anyhow::anyhow!("Invalid frame rate."))?,
            constant_fps: false,
            rotated: false,
            immersive: 0,
            fullscreen: true,
        }
        .parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_webrtc_h264_and_round_trips_without_secrets() {
        let value = Preferences::default();
        let text = serde_json::to_string(&value).unwrap();
        let restored = Preferences::parse(&text).unwrap();
        assert_eq!(
            restored.view()["rows"][0]["choices"][restored.transport],
            "WebRTC"
        );
        assert_eq!(restored.video, 0);
        assert_eq!(restored.host_request().unwrap().fps, Some(60));
        assert_eq!(restored.host_request().unwrap().bandwidth_mbps, Some(20));
        assert!(!text.contains("token"));
    }

    #[test]
    fn reject_invalid_settings_and_preserve_selection_on_failure() {
        let mut value = Preferences::default();
        assert!(value.select("video", 3).is_err());
        assert_eq!(value.video, 0);
        assert!(value.select("unknown", 0).is_err());
        value.select("video", 2).unwrap();
        assert_eq!(value.video, 2);
        assert!(Preferences::parse(
            "{\"transport\":99,\"video\":0,\"resolution\":0,\"fps\":0,\"bandwidth\":0}"
        )
        .is_err());
        assert!(Preferences::parse(&"x".repeat(4097)).is_err());
    }
}
