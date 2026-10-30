//! Private, pinned Parsec control framing. This is not a WebRTC standard.
//! DataView's default byte order is big endian; video metadata is separate.
use anyhow::{bail, Context, Result};
use bytes::Bytes;
use serde_json::{json, Value};

pub(crate) const MAX_CHANNEL_MESSAGE: usize = 1024 * 1024;
const LIMIT: usize = MAX_CHANNEL_MESSAGE;

#[derive(Clone)]
pub struct Config {
    pub video_protocol: crate::video_stream::VideoProtocol,
    pub allow_hevc: bool,
    pub ten_bit: bool,
    pub chroma_444: bool,
}

impl Config {
    pub fn video_flags(&self) -> u32 {
        0x08 | u32::from(self.allow_hevc)
            | if self.allow_hevc && self.ten_bit {
                0x10
            } else {
                0
            }
            | if self.allow_hevc && self.chroma_444 {
                2
            } else {
                0
            }
    }

    /// Keep the negotiated capabilities on later per-stream reconfigurations.
    /// Only native, capability-checked preferences can request full chroma.
    pub fn encoder_request(&self, bytes: Bytes) -> Bytes {
        if bytes.len() < 13 || bytes[12] != 13 || bytes[..4] != [0; 4] {
            return bytes;
        }
        let old = u32::from_be_bytes(bytes[4..8].try_into().unwrap());
        let flags = (old & !0x1b) | self.video_flags();
        if old == flags {
            return bytes;
        }
        let mut patched = bytes.to_vec();
        patched[4..8].copy_from_slice(&flags.to_be_bytes());
        Bytes::from(patched)
    }
    /// Pinned private opcode, corroborated by lowlat's WebRTC client. Preserve
    /// the negotiated flags; the third argument requests an encoder refresh.
    pub fn keyframe_request(&self) -> Bytes {
        header(13, 0, self.video_flags() as i32, 1)
    }
    pub fn startup(&self) -> Result<Bytes> {
        text(
            11,
            0,
            // Native SDK 6.0 advertises HEVC with bit 0 of this private field.
            // Keep it isolated from WebRTC/SDP; see docs/hevc-negotiation.md.
            &json!({"_version":1,"_max_w":60000,"_max_h":60000,"_flags":self.video_flags(),
            "resolutionX":1920,"resolutionY":1080,"refreshRate":60,"mediaContainer":0,
            "_VideoProtocolVersion":self.video_protocol.version})
            .to_string(),
        )
    }
}

pub fn header(kind: u8, a: i32, b: i32, c: i32) -> Bytes {
    let mut bytes = Vec::with_capacity(13);
    for value in [a, b, c] {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes.push(kind);
    Bytes::from(bytes)
}

pub fn text(kind: u8, id: i32, value: &str) -> Result<Bytes> {
    if value.len() > LIMIT - 14 || value.contains('\0') {
        bail!("invalid control text length/content");
    }
    let mut bytes = header(kind, i32::try_from(value.len() + 1)?, id, 0).to_vec();
    bytes.extend_from_slice(value.as_bytes());
    bytes.push(0);
    Ok(Bytes::from(bytes))
}

pub enum Message {
    Status(i32),
    Buffer {
        event: Value,
        payload: Option<std::ops::Range<usize>>,
    },
    Ignored,
}

fn cursor_payload(bytes: &[u8]) -> Result<(usize, usize)> {
    if !(34..=LIMIT).contains(&bytes.len()) {
        bail!("invalid cursor header size");
    }
    let size = usize::try_from(i32::from_be_bytes(bytes[16..20].try_into().unwrap()))
        .context("negative cursor image length")?;
    let end = 34usize
        .checked_add(size)
        .context("cursor image length overflow")?;
    if end > bytes.len() {
        bail!("truncated cursor image");
    }
    Ok((size, end))
}

/// A scheduling hint from validated host metadata, never from button state.
#[cfg(all(windows, feature = "bud"))]
pub(crate) fn cursor_capture(bytes: &[u8]) -> Option<bool> {
    if bytes.get(12) != Some(&9) {
        return None;
    }
    cursor_payload(bytes).ok()?;
    Some(u16::from_be_bytes([bytes[32], bytes[33]]) & 0x300 != 0)
}

pub fn decode(bytes: &[u8]) -> Result<Message> {
    if !(13..=LIMIT).contains(&bytes.len()) {
        bail!("invalid control frame size");
    }
    let number = |offset| i32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let (a, b) = (number(0), number(4));
    Ok(match bytes[12] {
        10 => Message::Status(a),
        // Timing is collected by the transport. The native session does not
        // consume the former web-core gamepad, host-mode or guest callbacks.
        16 | 20 | 21 | 28 => Message::Ignored,
        25 => {
            let size = usize::try_from(a).context("negative guest list length")?;
            if size == 0 || size != bytes.len() - 13 || bytes.last() != Some(&0) {
                bail!("invalid guest list payload boundary");
            }
            let list: Vec<Value> = serde_json::from_slice(&bytes[13..bytes.len() - 1])?;
            if list.len() > 256 || list.iter().any(|v| !v.is_object()) {
                bail!("invalid guest list");
            }
            Message::Ignored
        }
        17 => {
            let size = usize::try_from(a).context("negative user-data length")?;
            let end = 13usize
                .checked_add(size)
                .context("user-data length overflow")?;
            if end > bytes.len() {
                bail!("truncated user-data payload");
            }
            Message::Buffer {
                event: json!({"type":3,"id":b}),
                payload: Some(13..end),
            }
        }
        9 => {
            let (size, end) = cursor_payload(bytes)?;
            let short = |offset| i16::from_be_bytes(bytes[offset..offset + 2].try_into().unwrap());
            let flags = short(32);
            Message::Buffer {
                event: json!({"type":1,"cursor":{"size":size,"positionX":short(24),"positionY":short(26),"width":short(20),"height":short(22),"hotX":short(28),"hotY":short(30),"imageUpdate":size>0,"relative":flags&256!=0,"hidden":flags&512!=0,"stream":0}}),
                payload: if size > 0 { Some(34..end) } else { None },
            }
        }
        _ => Message::Ignored, // Same default branch as the pinned client.
    })
}

#[derive(Debug)]
pub struct AbsoluteMouseUnavailable;
impl std::fmt::Display for AbsoluteMouseUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("absolute mouse requires presented video dimensions")
    }
}
impl std::error::Error for AbsoluteMouseUnavailable {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_chroma_main10_request_is_preserved_on_refresh() {
        let mut config = Config {
            video_protocol: crate::video_stream::VideoProtocol {
                version: 1,
                message_size: 11,
                version_offset: 0,
                flag_offset: 4,
            },
            allow_hevc: true,
            ten_bit: true,
            chroma_444: true,
        };
        assert_eq!(config.video_flags(), 0x1b);
        let refresh = config.encoder_request(header(13, 0, 0, 1));
        assert_eq!(i32::from_be_bytes(refresh[4..8].try_into().unwrap()), 0x1b);
        config.chroma_444 = false;
        let refresh = config.encoder_request(header(13, 0, 0x1b, 1));
        assert_eq!(i32::from_be_bytes(refresh[4..8].try_into().unwrap()), 0x19);
        assert!(config.ten_bit && config.allow_hevc);
    }
    #[test]
    fn desktop_fullfps_settings_keep_the_pinned_userdata_envelope() {
        for full_fps in [false, true] {
            let configuration = json!({"video":[{"encoderFPS":60,"fullFPS":full_fps}]}).to_string();
            let frame = text(17, 11, &configuration).unwrap();
            assert_eq!(frame[12], 17);
            assert_eq!(
                i32::from_be_bytes(frame[..4].try_into().unwrap()) as usize,
                configuration.len() + 1
            );
            assert_eq!(&frame[4..8], &11i32.to_be_bytes());
            assert_eq!(&frame[8..12], &[0; 4]);
            assert_eq!(&frame[13..frame.len() - 1], configuration.as_bytes());
            assert_eq!(frame.last(), Some(&0));
        }
    }
    #[cfg(windows)]
    #[test]
    fn refresh_request_preserves_startup_codec_flags_and_primary_stream() {
        for allow_hevc in [false, true] {
            let config = Config {
                video_protocol: crate::video_stream::VideoProtocol {
                    version: 0,
                    message_size: 11,
                    version_offset: 0,
                    flag_offset: 4,
                },
                allow_hevc,
                ten_bit: false,
                chroma_444: false,
            };
            assert_eq!(
                config.keyframe_request(),
                header(13, 0, 8 | i32::from(allow_hevc), 1)
            );
            let request = config.keyframe_request();
            assert_eq!(request.len(), 13);
            assert_eq!(&request[0..4], &[0; 4]);
            assert_eq!(&request[8..12], &1i32.to_be_bytes());
        }
    }
    #[test]
    fn hevc_changes_only_the_verified_init_capability_bit() {
        let mut config = Config {
            video_protocol: crate::video_stream::VideoProtocol {
                version: 0,
                message_size: 11,
                version_offset: 0,
                flag_offset: 4,
            },
            allow_hevc: false,
            ten_bit: false,
            chroma_444: false,
        };
        let avc = config.startup().unwrap();
        config.allow_hevc = true;
        let hevc = config.startup().unwrap();
        for frame in [&avc, &hevc] {
            assert_eq!(frame[12], 11);
            assert_eq!(
                i32::from_be_bytes(frame[..4].try_into().unwrap()) as usize,
                frame.len() - 13
            );
            assert_eq!(&frame[4..12], &[0; 8]);
            assert_eq!(frame.last(), Some(&0));
        }
        let avc: Value = serde_json::from_slice(&avc[13..avc.len() - 1]).unwrap();
        let mut hevc: Value = serde_json::from_slice(&hevc[13..hevc.len() - 1]).unwrap();
        assert_eq!(hevc["_flags"], 9);
        hevc["_flags"] = json!(8);
        assert_eq!(avc, hevc);
    }
    #[test]
    fn stream_reconfiguration_preserves_main10_without_enabling_444() {
        let config = Config {
            video_protocol: crate::video_stream::VideoProtocol {
                version: 0,
                message_size: 11,
                version_offset: 0,
                flag_offset: 4,
            },
            allow_hevc: true,
            ten_bit: true,
            chroma_444: false,
        };
        let init = config.startup().unwrap();
        let init: Value = serde_json::from_slice(&init[13..init.len() - 1]).unwrap();
        assert_eq!(init["_flags"], 0x19);
        assert_eq!(
            config.encoder_request(header(13, 0, 2, 1)),
            header(13, 0, 0x19, 1)
        );
        assert_eq!(
            config.encoder_request(header(13, 1, 2, 1)),
            header(13, 1, 2, 1)
        );
        assert_eq!(
            config.encoder_request(header(12, 0, 2, 1)),
            header(12, 0, 2, 1)
        );
    }

    #[test]
    fn headers_use_big_endian_and_text_uses_utf8_byte_lengths() {
        assert_eq!(
            header(10, -3, 0x01020304, 0).as_ref(),
            [255, 255, 255, 253, 1, 2, 3, 4, 0, 0, 0, 0, 10]
        );
        let frame = text(25, 42, "[]").unwrap();
        assert!(matches!(decode(&frame).unwrap(), Message::Ignored));
        let frame = text(17, 42, "árvíz").unwrap();
        assert_eq!(
            i32::from_be_bytes(frame[..4].try_into().unwrap()),
            "árvíz".len() as i32 + 1
        );
    }
    #[test]
    fn malformed_control_frames_fail() {
        for length in 0..13 {
            assert!(decode(&vec![0; length]).is_err());
        }
        assert!(decode(&header(25, -1, 0, 0)).is_err());
        let mut frame = text(25, 0, "[]").unwrap().to_vec();
        frame.pop();
        assert!(decode(&frame).is_err());
        assert!(decode(&text(25, 0, "{}").unwrap()).is_err());
    }
    #[test]
    fn cursor_and_user_data_ranges_validate_before_access() {
        assert!(decode(&header(17, -1, 7, 0)).is_err());
        assert!(decode(&header(17, 1, 7, 0)).is_err());
        assert!(decode(&header(9, 0, 0, 0)).is_err());
        let mut cursor = vec![0; 34];
        cursor[12] = 9;
        cursor[16..20].copy_from_slice(&(-1i32).to_be_bytes());
        assert!(decode(&cursor).is_err());
        cursor[16..20].copy_from_slice(&1i32.to_be_bytes());
        assert!(decode(&cursor).is_err());
        cursor.push(255);
        cursor[24..26].copy_from_slice(&(-12i16).to_be_bytes());
        cursor[32..34].copy_from_slice(&768i16.to_be_bytes());
        match decode(&cursor).unwrap() {
            Message::Buffer { event, payload } => {
                assert_eq!(payload, Some(34..35));
                assert_eq!(event["cursor"]["positionX"], -12);
                assert_eq!(event["cursor"]["relative"], true);
                assert_eq!(event["cursor"]["hidden"], true);
            }
            _ => panic!("not a cursor buffer"),
        }
        cursor.truncate(34);
        cursor[16..20].copy_from_slice(&0i32.to_be_bytes());
        assert!(matches!(
            decode(&cursor).unwrap(),
            Message::Buffer { payload: None, .. }
        ));
        assert!(
            matches!(decode(&header(17,0,7,0)).unwrap(),Message::Buffer {payload:Some(range),..} if range.is_empty())
        );
    }

    #[cfg(all(windows, feature = "bud"))]
    #[test]
    fn cursor_wakeup_uses_validated_host_capture_flags_only() {
        let mut frame = vec![0; 34];
        frame[12] = 9;
        for (flags, capture) in [(0u16, false), (0x100, true), (0x200, true), (0x300, true)] {
            frame[32..34].copy_from_slice(&flags.to_be_bytes());
            assert_eq!(cursor_capture(&frame), Some(capture));
        }
        for length in 0..34 {
            assert_eq!(cursor_capture(&frame[..length]), None);
        }
        frame[16..20].copy_from_slice(&1i32.to_be_bytes());
        assert_eq!(cursor_capture(&frame), None);
        frame[16..20].copy_from_slice(&(-1i32).to_be_bytes());
        assert_eq!(cursor_capture(&frame), None);
        assert_eq!(cursor_capture(&header(1, 0, 0, 0)), None);
    }
}
