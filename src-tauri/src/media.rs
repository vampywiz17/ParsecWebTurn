use crate::stats::ConnectionStats;
use regex::Regex;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{Arc, LazyLock, Mutex},
};

#[derive(Default, Clone, PartialEq)]
struct Video {
    codec: Option<String>,
    decoder: Option<String>,
    profile: Option<String>,
    backend: Option<String>,
    hardware: Option<bool>,
    size: Option<(u32, u32)>,
    sequence: u64,
}

#[derive(Default)]
pub struct MediaState {
    players: BTreeMap<String, Video>,
    sequence: u64,
}

static CODEC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:codec: |using )(h264|hevc|h265|vp8|vp9|av1)\b").unwrap());
static PROFILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:profile: |using )(h264 (?:baseline|main|high)|hevc main(?: 10)?|vp9 profile[ 0-3]*)",
    )
    .unwrap()
});
static SIZE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:visible_rect: \d+,\d+ |natural size: \[)(\d+)[x,](\d+)").unwrap()
});

impl MediaState {
    pub fn receive(&mut self, event: &str, json: &str) {
        if json.len() > 65536 {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(json) else {
            return;
        };
        let Some(id) = value["playerId"].as_str().filter(|id| id.len() <= 128) else {
            return;
        };
        if event == "Media.playerEventsAdded" {
            if value["events"].as_array().is_some_and(|events| {
                events.iter().any(|item| {
                    item["value"].as_str().is_some_and(|s| {
                        s.contains("kWebMediaPlayerDestroyed")
                            || s.contains("kVideoDecoderDestroyed")
                    })
                })
            }) {
                self.players.remove(id);
            }
            return;
        }
        if !self.players.contains_key(id) && self.players.len() >= 16 {
            return;
        }
        let video = self.players.entry(id.into()).or_default();
        let old = video.clone();
        if event == "Media.playerPropertiesChanged" {
            if let Some(properties) = value["properties"].as_array() {
                for property in properties {
                    let Some(text) = property["value"].as_str() else {
                        continue;
                    };
                    match property["name"].as_str() {
                        Some("kVideoDecoderName") => set_decoder(video, text),
                        Some("kIsPlatformVideoDecoder") => {
                            video.hardware = match text {
                                "true" => Some(true),
                                "false" => Some(false),
                                _ => None,
                            }
                        }
                        Some("kVideoTracks") => {
                            if let Ok(Value::Array(tracks)) = serde_json::from_str::<Value>(text) {
                                if let Some(track) = tracks.first() {
                                    if let Some(codec) = track["codec"].as_str() {
                                        video.codec = known_codec(codec);
                                    }
                                    if let Some(profile) = track["profile"].as_str() {
                                        video.profile = safe_word(profile);
                                    }
                                    for field in ["natural size", "visible rect", "coded size"] {
                                        if let Some(size) = track[field].as_str() {
                                            if let Some((width, height)) = size.split_once('x') {
                                                if let (Ok(width), Ok(height)) =
                                                    (width.parse(), height.parse())
                                                {
                                                    if valid_size(width, height) {
                                                        video.size = Some((width, height));
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        } else if event == "Media.playerMessagesLogged" {
            if let Some(messages) = value["messages"].as_array() {
                for message in messages {
                    let Some(text) = message["message"].as_str() else {
                        continue;
                    };
                    for decoder in [
                        "D3DVideoDecoder",
                        "FFmpegVideoDecoder",
                        "Dav1dVideoDecoder",
                        "VpxVideoDecoder",
                    ] {
                        if text.contains(decoder)
                            && (text.starts_with("Use ") || text.contains(" is using "))
                        {
                            set_decoder(video, decoder);
                        }
                    }
                    if text.contains("using D3D11 backend") {
                        video.backend = Some("D3D11".into());
                    }
                    if let Some(codec) = CODEC.captures(text) {
                        video.codec = known_codec(&codec[1]);
                    }
                    if let Some(profile) = PROFILE.captures(text) {
                        video.profile = safe_word(&profile[1]);
                    }
                    if let Some(size) = SIZE.captures(text) {
                        if let (Ok(width), Ok(height)) = (size[1].parse(), size[2].parse()) {
                            if valid_size(width, height) {
                                video.size = Some((width, height));
                            }
                        }
                    }
                }
            }
        }
        if *video != old {
            self.sequence += 1;
            video.sequence = self.sequence;
        }
    }

    pub fn supplement(&self, sample: &mut ConnectionStats) {
        if sample.stale || sample.state != "connected" {
            return;
        }
        let Some(video) = self
            .players
            .values()
            // Media also reports encoder players. Never let an encoder's newer
            // codec/configuration replace the active decoder's metadata.
            .filter(|v| v.sequence > 0 && v.decoder.is_some())
            .max_by_key(|v| v.sequence)
        else {
            return;
        };
        // Media describes the active decoder, including mid-stream reconfiguration.
        if video.codec.is_some() {
            sample.codec.clone_from(&video.codec);
        }
        if video.decoder.is_some() {
            sample.decoder.clone_from(&video.decoder);
        }
        sample.video_profile.clone_from(&video.profile);
        sample.decoder_backend.clone_from(&video.backend);
        sample.hardware_decode = video.hardware;
        if let Some((width, height)) = video.size {
            sample.width = Some(width);
            sample.height = Some(height);
        }
        sample.video_source = Some("Chromium Media".into());
    }
}

fn valid_size(width: u32, height: u32) -> bool {
    (1..=16384).contains(&width) && (1..=16384).contains(&height)
}
fn set_decoder(video: &mut Video, text: &str) {
    let Some(name) = safe_word(text) else {
        return;
    };
    if video.decoder.as_ref() != Some(&name) {
        video.backend = None;
    }
    video.hardware = match name.as_str() {
        "D3DVideoDecoder" => Some(true),
        "FFmpegVideoDecoder" | "Dav1dVideoDecoder" | "VpxVideoDecoder" => Some(false),
        _ => None,
    };
    video.decoder = Some(name);
}
fn safe_word(text: &str) -> Option<String> {
    (!text.is_empty()
        && text.len() <= 128
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " /._-".contains(c)))
    .then(|| text.into())
}
fn known_codec(text: &str) -> Option<String> {
    match text.to_lowercase().as_str() {
        "h264" => Some("H.264".into()),
        "hevc" | "h265" => Some("HEVC".into()),
        "vp8" => Some("VP8".into()),
        "vp9" => Some("VP9".into()),
        "av1" => Some("AV1".into()),
        _ => None,
    }
}

pub fn attach(window: &tauri::WebviewWindow, state: Arc<Mutex<MediaState>>) {
    // Direct WebView2 API: no debugging port, external process or page privilege.
    let _ = window.with_webview(move |view| unsafe {
        use webview2_com::{
            CallDevToolsProtocolMethodCompletedHandler, DevToolsProtocolEventReceivedEventHandler,
        };
        use windows::core::{HSTRING, PCWSTR, PWSTR};
        let Ok(core) = view.controller().CoreWebView2() else {
            return;
        };
        for event in [
            "Media.playerPropertiesChanged",
            "Media.playerMessagesLogged",
            "Media.playerEventsAdded",
        ] {
            let event_name = HSTRING::from(event);
            let Ok(receiver) = core.GetDevToolsProtocolEventReceiver(PCWSTR(event_name.as_ptr()))
            else {
                continue;
            };
            let state = state.clone();
            let mut token = 0;
            let _ = receiver.add_DevToolsProtocolEventReceived(
                &DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        let mut text = PWSTR::null();
                        args.ParameterObjectAsJson(&mut text)?;
                        let json = webview2_com::take_pwstr(text);
                        if let Ok(mut state) = state.lock() {
                            state.receive(event, &json);
                        }
                    }
                    Ok(())
                })),
                &mut token,
            );
        }
        let method = HSTRING::from("Media.enable");
        let args = HSTRING::from("{}");
        let _ = core.CallDevToolsProtocolMethod(
            PCWSTR(method.as_ptr()),
            PCWSTR(args.as_ptr()),
            &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(()))),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_decoder_messages_update_visible_size_and_never_copy_adapter_ids() {
        let mut state = MediaState::default();
        let messages = ["Use D3DVideoDecoder", "Initialized VideoDecoder: codec: h264, profile: h264 baseline, natural size: [1280,720]", "D3DVideoDecoder config change: profile: h264 high, coded_size: 1920x1088, visible_rect: 0,0 1920x1080", "D3DVideoDecoder is using D3D11 backend", "Selected D3DVideoDecoder adapter LUID:{0, 185805}"];
        state.receive("Media.playerMessagesLogged", &serde_json::json!({"playerId":"p", "messages":messages.map(|message| serde_json::json!({"message":message}))}).to_string());
        let mut sample = ConnectionStats {
            state: "connected".into(),
            ..Default::default()
        };
        state.supplement(&mut sample);
        assert_eq!(sample.codec.as_deref(), Some("H.264"));
        assert_eq!(sample.decoder.as_deref(), Some("D3DVideoDecoder"));
        assert_eq!(sample.video_profile.as_deref(), Some("h264 high"));
        assert_eq!((sample.width, sample.height), (Some(1920), Some(1080)));
        assert_eq!(sample.hardware_decode, Some(true));
        assert!(!serde_json::to_string(&sample).unwrap().contains("185805"));
        state.receive("Media.playerMessagesLogged", r#"{"playerId":"encoder","messages":[{"message":"Initialized VideoEncoder: codec: vp8, natural size: [64,64]"}]}"#);
        state.supplement(&mut sample);
        assert_eq!(sample.codec.as_deref(), Some("H.264"));
        state.receive("Media.playerPropertiesChanged", r#"{"playerId":"p","properties":[{"name":"kVideoDecoderName","value":"VpxVideoDecoder"},{"name":"kIsPlatformVideoDecoder","value":"false"}]}"#);
        state.supplement(&mut sample);
        assert_eq!(sample.hardware_decode, Some(false));
        assert_eq!(sample.decoder_backend, None);
        state.receive(
            "Media.playerEventsAdded",
            r#"{"playerId":"p","events":[{"value":"{\"event\":\"kVideoDecoderDestroyed\"}"}]}"#,
        );
        assert!(!state.players.contains_key("p"));
    }
}
