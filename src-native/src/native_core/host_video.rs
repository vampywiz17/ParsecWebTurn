//! Owner settings contract: query sub-ID 9, object reply/update sub-ID 11.
//! Preserve all streams and untouched settings, as the original UI does.
use crate::session_preferences::{Resolution, Video};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::time::{Duration, Instant};

/// Sub-ID 11 has no request ID or apply acknowledgement. A query can return
/// the old configuration while the host's UI thread applies the update.
/// Send the write once; poll reads only until its fields agree or time out.
pub struct Confirmation {
    expected: Value,
    poll_at: Instant,
    deadline: Instant,
    output_only: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Verification {
    Waiting,
    Query,
    Confirmed,
    TimedOut,
}
impl Confirmation {
    pub fn new(frame: &[u8], now: Instant) -> Result<Self> {
        let value: Value = serde_json::from_slice(&frame[13..frame.len() - 1])?;
        Ok(Self {
            expected: value["video"][0].clone(),
            output_only: false,
            poll_at: now + Duration::from_millis(150),
            deadline: now + Duration::from_secs(5),
        })
    }
    pub fn output(id: &str, now: Instant) -> Self {
        Self {
            expected: serde_json::json!({"output":id}),
            poll_at: now + Duration::from_millis(150),
            deadline: now + Duration::from_secs(5),
            output_only: true,
        }
    }
    pub fn poll(&mut self, config: &Config, now: Instant) -> Verification {
        if config.value.as_ref().is_some_and(|v| {
            if self.output_only {
                return v["video"][0]["output"] == self.expected["output"];
            }
            [
                "encoderFPS",
                "encoderMaxBitrate",
                "resolutionX",
                "resolutionY",
                "fullFPS",
                "rotated",
            ]
            .iter()
            .all(|key| v["video"][0][*key] == self.expected[*key])
        }) {
            return Verification::Confirmed;
        }
        if now >= self.deadline {
            return Verification::TimedOut;
        }
        if now >= self.poll_at {
            self.poll_at = now + Duration::from_millis(250);
            return Verification::Query;
        }
        Verification::Waiting
    }
}
#[derive(Default)]
pub struct Config {
    value: Option<Value>,
}
impl Config {
    pub fn receive(&mut self, bytes: &[u8]) -> Result<()> {
        ensure!(
            bytes.len() <= 65536,
            "Host video configuration exceeds limit"
        );
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|_| anyhow::anyhow!("Invalid host video configuration"))?;
        let streams = value["video"]
            .as_array()
            .context("Host video configuration is missing its video array")?;
        ensure!(
            // The current stock Windows host serializes five slots, even
            // for one connected display. The older web UI reads only three.
            (1..=5).contains(&streams.len()) && streams.iter().all(Value::is_object),
            "Invalid host stream configuration"
        );
        self.value = Some(value);
        Ok(())
    }
    pub fn ready(&self) -> bool {
        self.value.is_some()
    }
    pub fn request(&self, video: &Video, size: (u32, u32)) -> Result<bytes::Bytes> {
        let mut value = self
            .value
            .clone()
            .context("Waiting for the host video configuration")?;
        let frame = video.request(size)?;
        let patch: Value = serde_json::from_slice(&frame[13..frame.len() - 1])?;
        let target = value["video"][0]
            .as_object_mut()
            .context("Host primary stream is unavailable")?;
        for (key, v) in patch["video"][0].as_object().unwrap() {
            if video.resolution == Resolution::Host
                && !video.has_host_settings()
                && matches!(key.as_str(), "resolutionX" | "resolutionY")
            {
                continue;
            }
            target.insert(key.clone(), v.clone());
        }
        crate::control::text(17, 11, &value.to_string())
    }
    pub fn windows_host(&self) -> bool {
        // Stock Windows host's sub-ID 9 handler calls its serializer with 1,
        // which is written to every video slot's hostOS field.
        self.value
            .as_ref()
            .is_some_and(|v| v["video"][0]["hostOS"].as_u64() == Some(1))
    }
    pub fn output_id(&self) -> Option<&str> {
        self.value.as_ref()?["video"][0]["output"].as_str()
    }
    pub fn select_output(&self, id: &str) -> Result<bytes::Bytes> {
        let mut value = self.value.clone().context("Waiting for host settings")?;
        value["video"][0]["output"] = Value::String(id.to_owned());
        // A size forced on the previous output may not exist on this one
        // (e.g. a fixed 800x600 VM display). Release that old mode request.
        value["video"][0]["resolutionX"] = 0.into();
        value["video"][0]["resolutionY"] = 0.into();
        crate::control::text(17, 11, &value.to_string())
    }
    pub fn observed(&self) -> Value {
        let v = self.value.as_ref().map(|v| &v["video"][0]);
        let number = |key: &str, max: u64| v.and_then(|v| v[key].as_u64()).filter(|n| *n <= max);
        let flag = |key: &str| v.and_then(|v| v[key].as_bool());
        serde_json::json!({"fps":number("encoderFPS",1000),"constant_fps":flag("fullFPS"),"bandwidth_mbps":number("encoderMaxBitrate",1000),"width":number("resolutionX",16384),"height":number("resolutionY",16384),"rotated":flag("rotated")})
    }
}
pub fn query() -> Result<bytes::Bytes> {
    crate::control::text(17, 9, "")
}
/// Shape only: never record output identities or arbitrary host text.
pub fn diagnostic(bytes: &[u8]) -> Value {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(value) => serde_json::json!({"bytes":bytes.len(),"json_valid":true,
            "root_object":value.is_object(),
            "video_slots":value["video"].as_array().map(Vec::len),
            "video_objects":value["video"].as_array().map(|v|v.iter().filter(|v|v.is_object()).count())}),
        Err(error) => serde_json::json!({"bytes":bytes.len(),"json_valid":false,
            "error_line":error.line(),"error_column":error.column()}),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_keep_host_releases_previous_fixed_size_without_touching_other_slots() {
        let base = serde_json::json!({"video":[{"output":"same","resolutionX":1920,"resolutionY":1080,"encoderFPS":60},{"output":"other","resolutionX":2560,"resolutionY":1440}]});
        let mut config = Config::default();
        config.receive(base.to_string().as_bytes()).unwrap();
        let frame = config
            .request(
                &Video {
                    resolution: Resolution::Host,
                    fps: Some(60),
                    ..Default::default()
                },
                (3840, 2160),
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&frame[13..frame.len() - 1]).unwrap();
        let mut expected = base;
        expected["video"][0]["resolutionX"] = 0.into();
        expected["video"][0]["resolutionY"] = 0.into();
        assert_eq!(value, expected);
    }
    #[test]
    fn host_os_is_verified_instead_of_assuming_every_peer_is_windows() {
        let mut config = Config::default();
        assert!(!config.windows_host());
        for (os, windows) in [(0, false), (1, true), (2, false)] {
            config
                .receive(
                    serde_json::json!({"video":[{"hostOS":os}]})
                        .to_string()
                        .as_bytes(),
                )
                .unwrap();
            assert_eq!(config.windows_host(), windows);
        }
    }
    #[test]
    fn output_switch_preserves_settings_and_confirms_identity_not_old_dimensions() {
        let initial = serde_json::json!({"virtualMicrophone":1,"extra":"keep","video":[{"output":"old","encoderFPS":60,"resolutionX":1920,"resolutionY":1080,"other":42},{"output":"untouched"}]});
        let mut config = Config::default();
        config.receive(initial.to_string().as_bytes()).unwrap();
        let frame = config.select_output("new").unwrap();
        let mut expected = initial.clone();
        expected["video"][0]["output"] = Value::String("new".into());
        expected["video"][0]["resolutionX"] = 0.into();
        expected["video"][0]["resolutionY"] = 0.into();
        let sent: Value = serde_json::from_slice(&frame[13..frame.len() - 1]).unwrap();
        assert_eq!(sent, expected);
        let now = Instant::now();
        let mut confirmation = Confirmation::output("new", now);
        assert_eq!(confirmation.poll(&config, now), Verification::Waiting);
        expected["video"][0]["resolutionX"] = 2560.into();
        config.receive(expected.to_string().as_bytes()).unwrap();
        assert_eq!(confirmation.poll(&config, now), Verification::Confirmed);
        let frame = config
            .request(
                &Video {
                    fps: Some(30),
                    ..Default::default()
                },
                (1920, 1080),
            )
            .unwrap();
        let sent: Value = serde_json::from_slice(&frame[13..frame.len() - 1]).unwrap();
        assert_eq!(sent["video"][0]["output"], "new");
    }
    #[test]
    fn delayed_host_apply_is_polled_without_accepting_the_previous_values() {
        let original = serde_json::json!({"video":[{"encoderFPS":60,"encoderMaxBitrate":50,"resolutionX":1920,"resolutionY":1080,"fullFPS":false,"rotated":false}]});
        let mut config = Config::default();
        config.receive(original.to_string().as_bytes()).unwrap();
        let desired = Video {
            fps: Some(30),
            bandwidth_mbps: Some(20),
            constant_fps: Some(true),
            resolution: Resolution::Fixed(1280, 720),
            ..Default::default()
        };
        let frame = config.request(&desired, (1920, 1080)).unwrap();
        let start = Instant::now();
        let mut verify = Confirmation::new(&frame, start).unwrap();
        assert_eq!(verify.poll(&config, start), Verification::Waiting);
        assert_eq!(
            verify.poll(&config, start + Duration::from_millis(150)),
            Verification::Query
        );
        // The first reply can still be the host state before its apply task.
        config.receive(original.to_string().as_bytes()).unwrap();
        assert_eq!(
            verify.poll(&config, start + Duration::from_millis(200)),
            Verification::Waiting
        );
        assert_eq!(
            verify.poll(&config, start + Duration::from_millis(400)),
            Verification::Query
        );
        config.receive(&frame[13..frame.len() - 1]).unwrap();
        assert_eq!(
            verify.poll(&config, start + Duration::from_millis(450)),
            Verification::Confirmed
        );
        // A later bandwidth-only request must preserve the confirmed resolution.
        let next = config
            .request(
                &Video {
                    bandwidth_mbps: Some(10),
                    resolution: Resolution::Fixed(1280, 720),
                    ..Default::default()
                },
                (1920, 1080),
            )
            .unwrap();
        let target: Value = serde_json::from_slice(&next[13..next.len() - 1]).unwrap();
        assert_eq!(target["video"][0]["resolutionX"], 1280);
        assert_eq!(target["video"][0]["encoderFPS"], 30);
        let mut next_verify = Confirmation::new(&next, start).unwrap();
        assert_eq!(
            next_verify.poll(&config, start + Duration::from_secs(5)),
            Verification::TimedOut
        );
    }
    #[test]
    fn current_stock_host_five_slots_are_accepted_and_preserved() {
        let stream = serde_json::json!({"output":"none","encoderFPS":60,"encoderMaxBitrate":50,"resolutionX":0,"resolutionY":0,"rotated":false,"fullFPS":false,"hostOS":0});
        for count in [1, 3, 5] {
            let base = serde_json::json!({"virtualTablet":0,"virtualMicrophone":0,"video":vec![stream.clone();count]});
            let mut config = Config::default();
            config.receive(base.to_string().as_bytes()).unwrap();
            let video = Video {
                resolution: Resolution::Fixed(1280, 720),
                fps: Some(30),
                constant_fps: Some(true),
                ..Default::default()
            };
            let frame = config.request(&video, (1920, 1080)).unwrap();
            let value: Value = serde_json::from_slice(&frame[13..frame.len() - 1]).unwrap();
            assert_eq!(value["video"][0]["encoderFPS"], 30);
            assert_eq!(value["video"][0]["fullFPS"], true);
            assert_eq!(value["video"][0]["resolutionX"], 1280);
            assert_eq!(
                &value["video"].as_array().unwrap()[1..],
                &base["video"].as_array().unwrap()[1..]
            );
            assert_eq!(config.observed()["fps"], 60);
        }
        let invalid = serde_json::json!({"video":vec![stream;6]});
        assert!(Config::default()
            .receive(invalid.to_string().as_bytes())
            .is_err());
        let diagnostic = diagnostic(br#"{"video":[{"output":"private-identity"}]}"#).to_string();
        assert!(!diagnostic.contains("private-identity"));
    }
    #[test]
    fn every_host_menu_patches_the_complete_object_and_preserves_other_streams() {
        let base = serde_json::json!({"virtualTablet":0,"virtualMicrophone":1,"video":[{"output":"DISPLAY1","resolutionX":1920,"resolutionY":1080,"encoderFPS":60,"fullFPS":false,"rotated":false,"encoderMaxBitrate":10,"hostOS":0},{"output":"DISPLAY2","encoderFPS":30},{"output":"none","encoderFPS":0}]});
        let mut config = Config::default();
        config.receive(base.to_string().as_bytes()).unwrap();
        for (resolution, size) in [
            (Resolution::Host, (0, 0)),
            (Resolution::Client, (2560, 1440)),
            (Resolution::Fixed(1280, 720), (1280, 720)),
        ] {
            let video = Video {
                resolution,
                bandwidth_mbps: Some(25),
                fps: Some(120),
                constant_fps: Some(true),
                rotated: Some(true),
                ..Default::default()
            };
            let bytes = config.request(&video, (2560, 1440)).unwrap();
            assert_eq!(bytes[12], 17);
            assert_eq!(&bytes[4..8], &11i32.to_be_bytes());
            let value: Value = serde_json::from_slice(&bytes[13..bytes.len() - 1]).unwrap();
            assert_eq!(value["video"].as_array().unwrap().len(), 3);
            assert_eq!(value["virtualMicrophone"], 1);
            assert_eq!(value["video"][1], base["video"][1]);
            assert_eq!(value["video"][2], base["video"][2]);
            let first = &value["video"][0];
            assert_eq!(first["output"], "DISPLAY1");
            assert_eq!(first["resolutionX"], size.0);
            assert_eq!(first["resolutionY"], size.1);
            assert_eq!(first["encoderFPS"], 120);
            assert_eq!(first["encoderMaxBitrate"], 25);
            assert_eq!(first["fullFPS"], true);
            assert_eq!(first["rotated"], true);
        }
        let bytes = config.request(&Video::default(), (1, 1)).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes[13..bytes.len() - 1]).unwrap(),
            base
        );
        assert!(Config::default()
            .request(&Video::default(), (1, 1))
            .is_err());
        assert!(config.receive(b"[]").is_err());
    }
}
