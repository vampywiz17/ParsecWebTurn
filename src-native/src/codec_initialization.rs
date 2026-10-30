//! Compressed parameter metadata for system decoders, never decoded pixels.
//! H.264 SPS syntax follows ITU-T H.264 7.3.2.1 / 7.4.2.1 and the same bounded
//! geometry rules as lowlat's pinned H.264 parser. No decoder SDK is imported.
use crate::video_stream::Codec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    pub visible_width: u32,
    pub visible_height: u32,
    pub depth: u32,
}

pub struct Initialization {
    pub geometry: Geometry,
    pub csd0: Vec<u8>,
    pub csd1: Vec<u8>,
}

pub fn avc_sps(nal: &[u8]) -> Option<(u32, Geometry)> {
    use crate::video_stream::ParameterBits;
    if nal.first()? & 31 != 7 {
        return None;
    }
    let mut bits = ParameterBits::new(nal.get(1..)?);
    let profile = bits.read(8)?;
    bits.skip(16)?;
    let id = bits.ue()?;
    if id > 31 {
        return None;
    }
    if matches!(
        profile,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        // This application's AVC path is 8-bit 4:2:0. HEVC owns Main10.
        if bits.ue()? != 1 || bits.ue()? != 0 || bits.ue()? != 0 {
            return None;
        }
        bits.skip(1)?;
        if bits.read(1)? != 0 {
            for index in 0..8 {
                if bits.read(1)? != 0 {
                    let mut last = 8i32;
                    let mut next = 8i32;
                    for _ in 0..if index < 6 { 16 } else { 64 } {
                        if next != 0 {
                            next = (last + signed(bits.ue()?)?).rem_euclid(256);
                        }
                        if next != 0 {
                            last = next;
                        }
                    }
                }
            }
        }
    }
    if bits.ue()? > 12 {
        return None;
    }
    match bits.ue()? {
        0 => {
            if bits.ue()? > 12 {
                return None;
            }
        }
        1 => {
            bits.skip(1)?;
            signed(bits.ue()?)?;
            signed(bits.ue()?)?;
            let count = bits.ue()?;
            if count > 255 {
                return None;
            }
            for _ in 0..count {
                signed(bits.ue()?)?;
            }
        }
        2 => {}
        _ => return None,
    }
    if bits.ue()? > 32 {
        return None;
    }
    bits.skip(1)?;
    let width = bits.ue()?.checked_add(1)?.checked_mul(16)?;
    let map_height = bits.ue()?.checked_add(1)?;
    let frame_only = bits.read(1)?;
    if frame_only == 0 {
        bits.skip(1)?;
    }
    bits.skip(1)?;
    let height = map_height.checked_mul(16 * (2 - frame_only))?;
    if width > 8192 || height > 8192 {
        return None;
    }
    let (mut visible_width, mut visible_height) = (width, height);
    if bits.read(1)? != 0 {
        let horizontal = bits.ue()?.checked_add(bits.ue()?)?.checked_mul(2)?;
        let vertical = bits
            .ue()?
            .checked_add(bits.ue()?)?
            .checked_mul(2 * (2 - frame_only))?;
        visible_width = width.checked_sub(horizontal)?;
        visible_height = height.checked_sub(vertical)?;
    }
    if visible_width == 0 || visible_height == 0 {
        return None;
    }
    Some((
        id,
        Geometry {
            width,
            height,
            visible_width,
            visible_height,
            depth: 8,
        },
    ))
}

fn signed(code: u32) -> Option<i32> {
    let magnitude = i32::try_from(code.div_ceil(2)).ok()?;
    Some(if code & 1 != 0 { magnitude } else { -magnitude })
}

pub fn initialize(units: &[Vec<u8>], codec: Codec) -> Option<Initialization> {
    if units.is_empty() || units.iter().any(Vec::is_empty) {
        return None;
    }
    use crate::video_stream::ParameterBits;
    let mut csd0 = Vec::new();
    let mut csd1 = Vec::new();
    let append = |out: &mut Vec<u8>, unit: &[u8]| {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(unit);
    };
    let geometry = match codec {
        Codec::H264 => {
            let (_, geometry) = units.iter().rev().find_map(|u| avc_sps(u))?;
            let mut ids = 0u32;
            for unit in units {
                if let Some((id, shape)) = avc_sps(unit) {
                    if shape == geometry {
                        ids |= 1 << id;
                        append(&mut csd0, unit);
                    }
                }
            }
            for unit in units {
                if unit[0] & 31 == 8 {
                    let mut bits = ParameterBits::new(&unit[1..]);
                    bits.ue()?;
                    let id = bits.ue()?;
                    if id < 32 && ids & (1 << id) != 0 {
                        append(&mut csd1, unit);
                    }
                }
            }
            if csd1.is_empty() {
                return None;
            }
            geometry
        }
        Codec::Hevc => {
            let bytes = units.iter().rev().find(|u| (u[0] >> 1) & 63 == 33)?;
            let mut framed = vec![0, 0, 0, 1];
            framed.extend_from_slice(bytes);
            let shape = crate::video_stream::hevc_configuration(&framed)?;
            if shape.chroma != 1 || !matches!(shape.bit_depth, 8 | 10) {
                return None;
            }
            let mut seen = 0u8;
            for unit in units {
                let kind = (unit[0] >> 1) & 63;
                if (32..=34).contains(&kind) {
                    seen |= 1 << (kind - 32);
                    append(&mut csd0, unit);
                }
            }
            if seen != 7 {
                return None;
            }
            Geometry {
                width: shape.width,
                height: shape.height,
                visible_width: shape.visible_width,
                visible_height: shape.visible_height,
                depth: shape.bit_depth,
            }
        }
    };
    Some(Initialization {
        geometry,
        csd0,
        csd1,
    })
}
