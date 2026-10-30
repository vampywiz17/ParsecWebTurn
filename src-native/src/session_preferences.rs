//! Shared video requests, presets and validation. No window, codec, storage or OS APIs.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

pub const MAX_BANDWIDTH_MBPS: u32 = 100;
pub const MAX_FPS: u32 = 240;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    #[default]
    Host,
    Client,
    Fixed(u32, u32),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Video {
    pub resolution: Resolution,
    /// None preserves the current host setting; Some is a request, not telemetry.
    pub bandwidth_mbps: Option<u32>,
    pub fps: Option<u32>,
    pub constant_fps: Option<bool>,
    pub rotated: Option<bool>,
    pub immersive: u8,
    pub fullscreen: bool,
}
impl Default for Video {
    fn default() -> Self {
        Self {
            resolution: Resolution::Host,
            bandwidth_mbps: None,
            fps: None,
            constant_fps: None,
            rotated: None,
            immersive: 0,
            fullscreen: false,
        }
    }
}
impl Video {
    pub fn has_host_settings(&self) -> bool {
        self.resolution != Resolution::Host
            || self.bandwidth_mbps.is_some()
            || self.fps.is_some()
            || self.constant_fps.is_some()
            || self.rotated.is_some()
    }

    pub fn validate(&self) -> Result<()> {
        if let Resolution::Fixed(w, h) = self.resolution {
            ensure!(
                (320..=8192).contains(&w) && (200..=8192).contains(&h),
                "Resolution is outside supported limits"
            );
        }
        ensure!(
            self.bandwidth_mbps
                .is_none_or(|n| (1..=MAX_BANDWIDTH_MBPS).contains(&n)),
            "Bandwidth must be 1–{MAX_BANDWIDTH_MBPS} Mbps"
        );
        ensure!(
            self.fps.is_none_or(|n| (1..=MAX_FPS).contains(&n)),
            "FPS must be 1–{MAX_FPS}"
        );
        ensure!(self.immersive <= 3, "Invalid immersive mode");
        Ok(())
    }
}

impl Video {
    pub fn request(&self, client_size: (u32, u32)) -> Result<bytes::Bytes> {
        self.validate()?;
        let mut values = serde_json::Map::new();
        let (w, h) = match self.resolution {
            Resolution::Host => (0, 0),
            Resolution::Client => client_size,
            Resolution::Fixed(w, h) => (w, h),
        };
        ensure!(
            w <= 16384 && h <= 16384,
            "Client display dimensions exceed limit"
        );
        values.insert("resolutionX".into(), w.into());
        values.insert("resolutionY".into(), h.into());
        if let Some(n) = self.bandwidth_mbps {
            values.insert("encoderMaxBitrate".into(), n.into());
        }
        if let Some(n) = self.fps {
            values.insert("encoderFPS".into(), n.into());
        }
        if let Some(n) = self.constant_fps {
            values.insert("fullFPS".into(), n.into());
        }
        if let Some(n) = self.rotated {
            values.insert("rotated".into(), n.into());
        }
        // Same owner-video contract used by the existing core and lowlat daemon.
        // No codec flags here: the BUD adapter preserves its negotiated capability.
        crate::control::text(17, 11, &serde_json::json!({"video":[values]}).to_string())
    }
}

pub const HOST_RESOLUTION: &str = "Keep host";
pub const CLIENT_RESOLUTION: &str = "Match client display";
// Stock Windows UI presets, not a host hardware capability declaration.
// Current stock output/settings replies do not contain supported-mode lists.
pub const RESOLUTION_CHOICES: &[&str] = &[
    HOST_RESOLUTION,
    CLIENT_RESOLUTION,
    "3840 x 2160",
    "3840 x 1600",
    "3440 x 1440",
    "2560 x 1600",
    "2560 x 1440",
    "2560 x 1080",
    "1920 x 1200",
    "1920 x 1080",
    "1680 x 1050",
    "1600 x 1200",
    "1366 x 768",
    "1280 x 1024",
    "1280 x 800",
    "1280 x 720",
    "1024 x 768",
];
pub const BANDWIDTH_CHOICES: &[&str] = &[
    "3", "5", "7", "10", "15", "20", "25", "30", "35", "40", "45", "50",
];
pub const FPS_CHOICES: &[&str] = &["30", "60", "100", "120", "144", "240"];
pub fn current_choice(value: &str) -> String {
    format!("Keep current: {value}")
}
pub fn resolution_text(resolution: Resolution) -> String {
    match resolution {
        Resolution::Host => HOST_RESOLUTION.into(),
        Resolution::Client => CLIENT_RESOLUTION.into(),
        Resolution::Fixed(w, h) => format!("{w} x {h}"),
    }
}
/// Native views read their widgets; every view uses the same parsing and
/// validation before producing an owned video-settings command.
pub struct PanelFields<'a> {
    pub resolution: &'a str,
    pub bandwidth: &'a str,
    pub fps: &'a str,
    pub constant_fps: bool,
    pub rotated: bool,
    pub immersive: u8,
    pub fullscreen: bool,
}
impl PanelFields<'_> {
    pub fn parse(&self) -> Result<Video> {
        let video = Video {
            resolution: resolution(self.resolution)?,
            bandwidth_mbps: number(self.bandwidth, MAX_BANDWIDTH_MBPS)?,
            fps: number(self.fps, MAX_FPS)?,
            constant_fps: Some(self.constant_fps),
            rotated: Some(self.rotated),
            immersive: self.immersive,
            fullscreen: self.fullscreen,
        };
        video.validate()?;
        Ok(video)
    }
}
pub(crate) fn number(text: &str, max: u32) -> Result<Option<u32>> {
    let n: u32 = text
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("Enter a number."))?;
    ensure!((1..=max).contains(&n), "Value must be 1–{max}.");
    Ok(Some(n))
}
pub(crate) fn resolution(text: &str) -> Result<Resolution> {
    match text.trim() {
        HOST_RESOLUTION => Ok(Resolution::Host),
        CLIENT_RESOLUTION => Ok(Resolution::Client),
        s => {
            let (w, h) = s
                .split_once(['x', '×'])
                .ok_or_else(|| anyhow::anyhow!("Enter resolution as width x height."))?;
            Ok(Resolution::Fixed(w.trim().parse()?, h.trim().parse()?))
        }
    }
}
