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

#[derive(PartialEq, Eq)]
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
    if !matches!(
        profile,
        66 | 77 | 88 | 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        return None;
    }
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
            // A parameter-set ID is replaced, not accumulated. Canonical ID
            // order also keeps identical header repeats from reopening a codec.
            let mut sequences = [None::<&[u8]>; 32];
            let mut ids = 0u32;
            for unit in units.iter().rev() {
                if let Some((id, shape)) = avc_sps(unit) {
                    if shape == geometry && sequences[id as usize].is_none() {
                        ids |= 1 << id;
                        sequences[id as usize] = Some(unit);
                    }
                }
            }
            for unit in sequences.into_iter().flatten() {
                append(&mut csd0, unit);
            }
            let mut pictures = [None::<&[u8]>; 256];
            for unit in units.iter().rev() {
                if unit[0] & 31 == 8 {
                    let mut bits = ParameterBits::new(&unit[1..]);
                    let id = usize::try_from(bits.ue()?).ok()?;
                    if id >= pictures.len() {
                        return None;
                    }
                    if pictures[id].is_none() {
                        pictures[id] = Some(unit);
                    }
                }
            }
            for unit in pictures.into_iter().flatten() {
                let mut bits = ParameterBits::new(&unit[1..]);
                bits.ue()?;
                let id = bits.ue()?;
                if id < 32 && ids & (1 << id) != 0 {
                    append(&mut csd1, unit);
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

#[cfg(test)]
mod tests {
    use super::*;
    // Build standard baseline SPS syntax through frame cropping. No driver,
    // network, account or decoded image is involved in metadata validation.
    fn sps(width_mbs: u32, height_mbs: u32, bottom_crop: u32) -> Vec<u8> {
        let mut bits = Vec::new();
        fn word(bits: &mut Vec<bool>, n: u32, len: u32) {
            for shift in (0..len).rev() {
                bits.push(n & (1 << shift) != 0);
            }
        }
        fn ue(bits: &mut Vec<bool>, n: u32) {
            let n = n + 1;
            let len = 32 - n.leading_zeros();
            bits.extend(std::iter::repeat_n(false, (len - 1) as usize));
            word(bits, n, len);
        }
        word(&mut bits, 66, 8);
        word(&mut bits, 0, 8);
        word(&mut bits, 40, 8);
        for n in [0, 0, 0, 0, 1] {
            ue(&mut bits, n);
        }
        bits.push(false);
        ue(&mut bits, width_mbs - 1);
        ue(&mut bits, height_mbs - 1);
        bits.extend([true, true, true]);
        for n in [0, 0, 0, bottom_crop] {
            ue(&mut bits, n);
        }
        bits.push(true);
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        let mut nal = vec![0x67];
        for chunk in bits.chunks(8) {
            nal.push(
                chunk
                    .iter()
                    .fold(0, |value, bit| (value << 1) | u8::from(*bit)),
            );
        }
        nal
    }
    #[test]
    fn cropped_avc_keeps_coded_and_visible_dimensions_and_requires_pps() {
        let unit = sps(120, 68, 4);
        let (_, geometry) = avc_sps(&unit).unwrap();
        assert_eq!((geometry.width, geometry.height), (1920, 1088));
        assert_eq!(
            (geometry.visible_width, geometry.visible_height),
            (1920, 1080)
        );
        assert!(initialize(std::slice::from_ref(&unit), Codec::H264).is_none());
        let config = initialize(&[unit.clone(), vec![0x68, 0xc0]], Codec::H264).unwrap();
        assert_eq!(config.csd0, [vec![0, 0, 0, 1], unit].concat());
        assert_eq!(config.csd1, vec![0, 0, 0, 1, 0x68, 0xc0]);
    }
    #[test]
    fn latest_parameter_id_replaces_old_bytes_and_repeats_have_stable_configuration() {
        let old = sps(120, 68, 4);
        let mut new = old.clone();
        new[3] = 41;
        let old_pps = vec![0x68, 0xc0];
        let new_pps = vec![0x68, 0xc8];
        let config =
            initialize(&[old, old_pps, new.clone(), new_pps.clone()], Codec::H264).unwrap();
        assert_eq!(config.csd0, [vec![0, 0, 0, 1], new.clone()].concat());
        assert_eq!(config.csd1, [vec![0, 0, 0, 1], new_pps.clone()].concat());
        let repeat = initialize(&[new_pps, new], Codec::H264).unwrap();
        assert!(config == repeat);
    }

    #[test]
    fn reject_truncated_oversized_and_unrelated_parameters_without_panicking() {
        let unit = sps(120, 68, 4);
        for end in 0..unit.len() - 1 {
            assert!(avc_sps(&unit[..end]).is_none());
        }
        assert!(avc_sps(&sps(1024, 68, 0)).is_none());
        assert!(initialize(&[vec![]], Codec::H264).is_none());
        assert!(initialize(&[unit, vec![0x68, 0xa0]], Codec::H264).is_none()); // PPS references SPS 1.
    }
}
