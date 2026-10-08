//! Private, pinned Parsec control framing. This is not a WebRTC standard.
//! DataView's default byte order is big endian; video metadata is separate.
use anyhow::{bail, Context, Result};
use bytes::Bytes;
use serde_json::{json, Value};

const LIMIT: usize = 1024 * 1024;

#[derive(Clone)]
pub struct Config {
    pub video_protocol: u32,
}

impl Config {
    pub fn startup(&self) -> Result<Bytes> {
        text(
            11,
            0,
            &json!({"_version":1,"_max_w":60000,"_max_h":60000,"_flags":0,
            "resolutionX":1920,"resolutionY":1080,"refreshRate":60,"mediaContainer":0,
            "_VideoProtocolVersion":self.video_protocol})
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
    Guests { list: Vec<Value>, me: Value },
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
        9 | 17 => bail!("cursor/user-data buffer bridge is not implemented"),
        _ => Message::Ignored, // Same default branch as the pinned client.
    })
}

/// Relative mouse coordinates only until decoded dimensions/presentation exist.
/// Absolute positioning is rejected instead of guessing a viewport transform.
pub fn input(value: &Value) -> Result<Bytes> {
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
            if !boolean(value, "relative")? {
                bail!("absolute mouse mapping requires decoded video dimensions");
            }
            header(3, 1, int(value, "x")?, int(value, "y")?)
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
        9 => header(24, 0, 0, 0),
        _ => bail!("input event is not supported by the native control adapter"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
