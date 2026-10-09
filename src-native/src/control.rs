//! Private, pinned Parsec control framing. This is not a WebRTC standard.
//! DataView's default byte order is big endian; video metadata is separate.
use anyhow::{bail, Context, Result};
use bytes::Bytes;
use serde_json::{json, Value};

const LIMIT: usize = 1024 * 1024;

#[derive(Clone)]
pub struct Config {
    pub video_protocol: crate::backend::VideoProtocol,
}

impl Config {
    pub fn startup(&self) -> Result<Bytes> {
        text(
            11,
            0,
            &json!({"_version":1,"_max_w":60000,"_max_h":60000,"_flags":0,
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
    EncodeLatency(f32),
    Event(Value),
    HostMode(i32),
    Guests {
        list: Vec<Value>,
        me: Value,
    },
    Buffer {
        event: Value,
        payload: Option<std::ops::Range<usize>>,
    },
    Ignored,
}

pub fn decode(bytes: &[u8]) -> Result<Message> {
    if !(13..=LIMIT).contains(&bytes.len()) {
        bail!("invalid control frame size");
    }
    let number = |offset| i32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let (a, b, c) = (number(0), number(4), number(8));
    Ok(match bytes[12] {
        10 => Message::Status(a),
        21 => Message::EncodeLatency(b as f32 / 1000.0),
        20 => Message::Event(json!({"type":2,"gamepadID":a,"motorBig":b,"motorSmall":c})),
        16 => Message::Event(json!({"type":if a!=0 {4} else {5}})),
        28 => Message::HostMode(a),
        25 => {
            let size = usize::try_from(a).context("negative guest list length")?;
            if size == 0 || size != bytes.len() - 13 || bytes.last() != Some(&0) {
                bail!("invalid guest list payload boundary");
            }
            let list: Vec<Value> = serde_json::from_slice(&bytes[13..bytes.len() - 1])?;
            if list.len() > 256 || list.iter().any(|v| !v.is_object()) {
                bail!("invalid guest list");
            }
            let me = list
                .iter()
                .rev()
                .find(|v| v["id"].as_i64() == Some(i64::from(b)))
                .cloned()
                .unwrap_or_else(|| json!({}));
            Message::Guests { list, me }
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
            if bytes.len() < 34 {
                bail!("truncated cursor header");
            }
            let size = usize::try_from(number(16)).context("negative cursor image length")?;
            let end = 34usize
                .checked_add(size)
                .context("cursor image length overflow")?;
            if end > bytes.len() {
                bail!("truncated cursor image");
            }
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
pub fn input(value: &Value) -> Result<Bytes> {
    input_with_viewport(value, None)
}

pub fn input_with_viewport(
    value: &Value,
    viewport: Option<crate::viewport::Viewport>,
) -> Result<Bytes> {
    fn int(value: &Value, name: &str) -> Result<i32> {
        i32::try_from(
            value[name]
                .as_i64()
                .context("input integer field missing")?,
        )
        .context("input field out of range")
    }
    fn boolean(value: &Value, name: &str) -> Result<bool> {
        value[name].as_bool().context("input boolean field missing")
    }
    Ok(match int(value, "type")? {
        1 => header(
            0,
            int(value, "code")?,
            int(value, "mod")?,
            i32::from(boolean(value, "pressed")?),
        ),
        2 => header(
            1,
            int(value, "button")?,
            i32::from(boolean(value, "pressed")?),
            0,
        ),
        3 => header(2, int(value, "x")?, int(value, "y")?, 0),
        4 => {
            let (x, y) = (int(value, "x")?, int(value, "y")?);
            if !boolean(value, "relative")? {
                let (x, y) = viewport
                    .and_then(|v| v.map(x, y))
                    .ok_or(AbsoluteMouseUnavailable)?;
                return Ok(header(3, 0, x, y));
            }
            header(3, 1, x, y)
        }
        5 => header(
            4,
            int(value, "button")?,
            i32::from(boolean(value, "pressed")?),
            int(value, "id")?,
        ),
        6 => header(
            5,
            int(value, "axis")?,
            int(value, "value")?,
            int(value, "id")?,
        ),
        7 => header(6, 0, 0, int(value, "id")?),
        8 => {
            let mut bytes = header(23, int(value, "id")?, 0, 0).to_vec();
            bytes.resize(28, 0);
            bytes[16..18].copy_from_slice(&u16::try_from(int(value, "buttons")?)?.to_be_bytes());
            for (offset, name) in [
                (18, "thumbLX"),
                (20, "thumbLY"),
                (22, "thumbRX"),
                (24, "thumbRY"),
            ] {
                bytes[offset..offset + 2]
                    .copy_from_slice(&i16::try_from(int(value, name)?)?.to_be_bytes());
            }
            bytes[26] = u8::try_from(int(value, "leftTrigger")?)?;
            bytes[27] = u8::try_from(int(value, "rightTrigger")?)?;
            Bytes::from(bytes)
        }
        9 => header(24, 0, 0, 0),
        _ => bail!("input event is not supported by the native control adapter"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absolute_motion_requires_a_real_viewport_and_preserves_control_framing() {
        let event = json!({"type":4,"relative":false,"x":512,"y":384});
        assert_eq!(
            input_with_viewport(
                &event,
                Some(crate::viewport::Viewport {
                    source: (1920, 1080),
                    client: (1024, 768)
                })
            )
            .unwrap(),
            header(3, 0, 960, 540)
        );
        let state = json!({"type":8,"id":7,"buttons":65535,"thumbLX":-32768,"thumbLY":32767,"thumbRX":0,"thumbRY":-1,"leftTrigger":0,"rightTrigger":255});
        let bytes = input(&state).unwrap();
        assert_eq!(
            &bytes[16..],
            &[255, 255, 128, 0, 127, 255, 0, 0, 255, 255, 0, 255]
        );
        assert_eq!(bytes[12], 23);
    }
    #[test]
    fn headers_use_big_endian_and_text_uses_utf8_byte_lengths() {
        assert_eq!(
            header(10, -3, 0x01020304, 0).as_ref(),
            [255, 255, 255, 253, 1, 2, 3, 4, 0, 0, 0, 0, 10]
        );
        let frame = text(25, 42, "[]").unwrap();
        assert!(matches!(decode(&frame).unwrap(),Message::Guests {list,..} if list.is_empty()));
        let frame = text(17, 42, "árvíz").unwrap();
        assert_eq!(
            i32::from_be_bytes(frame[..4].try_into().unwrap()),
            "árvíz".len() as i32 + 1
        );
    }
    #[test]
    fn malformed_frames_and_unsupported_viewport_mapping_fail() {
        for length in 0..13 {
            assert!(decode(&vec![0; length]).is_err());
        }
        assert!(decode(&header(25, -1, 0, 0)).is_err());
        let mut frame = text(25, 0, "[]").unwrap().to_vec();
        frame.pop();
        assert!(decode(&frame).is_err());
        assert!(decode(&text(25, 0, "{}").unwrap()).is_err());
        assert!(input(&json!({"type":4,"relative":false,"x":1,"y":2})).is_err());
        assert!(input(&json!({"type":1,"code":1,"mod":0,"pressed":1})).is_err());
        assert_eq!(
            input(&json!({"type":1,"code":65,"mod":2,"pressed":true})).unwrap(),
            header(0, 65, 2, 1)
        );
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
}
