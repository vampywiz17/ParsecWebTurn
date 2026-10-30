//! Bounded inspection of pinned Parsec video framing, not a decoder.
//! Proprietary metadata is separate from H.264/HEVC Annex B byte-stream syntax.
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VideoProtocol {
    pub version: u32,
    pub message_size: u32,
    pub version_offset: u32,
    pub flag_offset: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum Codec {
    #[default]
    H264,
    Hevc,
}
impl Codec {
    pub fn name(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::Hevc => "H.265 / HEVC",
        }
    }
}

#[derive(Clone, Default, Serialize)]
pub struct Snapshot {
    pub codec: Option<Codec>,
    pub hevc_advertised: bool,
    pub vps_observed: bool,
    pub metadata_messages: u64,
    pub encoded_messages: u64,
    pub key_chunks_announced: u64,
    pub delta_chunks_announced: u64,
    pub annex_b_messages: u64,
    pub unrecognized_messages: u64,
    pub idr_messages: u64,
    pub sps_observed: bool,
    pub pps_observed: bool,
    pub sps_profile_idc: Option<u8>,
    pub sps_constraint_flags: Option<u8>,
    pub sps_level_idc: Option<u8>,
    pub parameter_sets_and_idr_observed: bool,
    pub protocol_changes: u64,
}

#[derive(Default)]
pub struct Inspector {
    protocol: Option<VideoProtocol>,
    headers_seen: bool,
    next_key: bool,
    snapshot: Snapshot,
}

impl Inspector {
    pub fn enable_hevc(&mut self, enabled: bool) {
        self.snapshot.hevc_advertised = enabled;
    }
    pub fn configure(&mut self, protocol: VideoProtocol) {
        if self.protocol.as_ref() == Some(&protocol) {
            return;
        }
        let changes = self
            .snapshot
            .protocol_changes
            .saturating_add(u64::from(self.protocol.is_some()));
        *self = Self {
            protocol: Some(protocol),
            snapshot: Snapshot {
                protocol_changes: changes,
                hevc_advertised: self.snapshot.hevc_advertised,
                ..Default::default()
            },
            ..Default::default()
        };
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.clone()
    }

    pub fn receive(&mut self, bytes: &[u8]) -> Option<PacketInfo> {
        if let Some(protocol) = &self.protocol {
            let word = |offset: u32| -> Option<u32> {
                let start = usize::try_from(offset).ok()?;
                Some(u32::from_le_bytes(
                    bytes.get(start..start.checked_add(4)?)?.try_into().ok()?,
                ))
            };
            if bytes.len() == protocol.message_size as usize
                && word(protocol.version_offset) == Some(protocol.version)
            {
                if let Some(flags) = word(protocol.flag_offset) {
                    self.headers_seen = true;
                    self.next_key = flags & 2 != 0;
                    self.snapshot.metadata_messages =
                        self.snapshot.metadata_messages.saturating_add(1);
                    return None;
                }
            }
        }
        self.snapshot.encoded_messages = self.snapshot.encoded_messages.saturating_add(1);
        let announced_key = !self.headers_seen || self.next_key;
        self.next_key = false;
        if announced_key {
            self.snapshot.key_chunks_announced =
                self.snapshot.key_chunks_announced.saturating_add(1);
        } else {
            self.snapshot.delta_chunks_announced =
                self.snapshot.delta_chunks_announced.saturating_add(1);
        }
        // HEVC VPS (type 32) cannot be a valid H.264 NAL (type 0).
        // Only recognize HEVC after advertising it, and then lock the codec.
        let hevc = self.snapshot.hevc_advertised
            && (self.snapshot.codec == Some(Codec::Hevc)
                || (self.snapshot.codec.is_none()
                    && hevc_annex_b(bytes, false).is_some_and(|s| s.vps)));
        let Some(summary) = (if hevc {
            hevc_annex_b(bytes, false)
        } else {
            annex_b(bytes)
        }) else {
            self.snapshot.unrecognized_messages =
                self.snapshot.unrecognized_messages.saturating_add(1);
            return None;
        };
        self.snapshot.annex_b_messages = self.snapshot.annex_b_messages.saturating_add(1);
        let codec = if hevc { Codec::Hevc } else { Codec::H264 };
        if summary.sps.is_some() || summary.vps {
            self.snapshot.codec = Some(codec);
        }
        self.snapshot.vps_observed |= summary.vps;
        if let Some([profile, constraints, level]) = summary.sps {
            self.snapshot.sps_observed = true;
            self.snapshot.sps_profile_idc = Some(profile);
            if codec == Codec::H264 {
                self.snapshot.sps_constraint_flags = Some(constraints);
                self.snapshot.sps_level_idc = Some(level);
            }
        }
        self.snapshot.pps_observed |= summary.pps;
        if summary.idr {
            self.snapshot.idr_messages = self.snapshot.idr_messages.saturating_add(1);
        }
        self.snapshot.parameter_sets_and_idr_observed = self.snapshot.sps_observed
            && self.snapshot.pps_observed
            && self.snapshot.idr_messages != 0
            && (codec != Codec::Hevc || self.snapshot.vps_observed);
        Some(PacketInfo {
            codec,
            idr: summary.idr,
            picture: summary.picture,
            parameters: summary.vps || summary.sps.is_some() || summary.pps,
        })
    }
}

#[derive(Clone, Copy)]
pub struct PacketInfo {
    /// A supported VCL NAL was observed, not merely SEI/parameter metadata.
    pub picture: bool,
    pub codec: Codec,
    pub idr: bool,
    pub parameters: bool,
}

/// Only codec parameter NALs, never pictures. Bounded history preserves the
/// order of parameter-set updates for a decoder restarted during this session.
#[derive(Default)]
pub struct ParameterCache {
    units: Vec<Vec<u8>>,
    bytes: usize,
    overflowed: bool,
}
impl ParameterCache {
    pub fn observe(&mut self, bytes: &[u8], codec: Codec) {
        if self.overflowed {
            return;
        }
        let valid = match codec {
            Codec::H264 => annex_b(bytes),
            Codec::Hevc => hevc_annex_b(bytes, false),
        };
        if valid.is_none() {
            return;
        }
        let mut offset = 0;
        while offset < bytes.len() {
            while bytes.get(offset) == Some(&0) {
                offset += 1;
            }
            if bytes.get(offset) != Some(&1) {
                return;
            }
            let begin = offset + 1;
            let mut end = begin;
            while end < bytes.len()
                && !bytes[end..].starts_with(&[0, 0, 1])
                && !bytes[end..].starts_with(&[0, 0, 0, 1])
            {
                end += 1;
            }
            let nal = &bytes[begin..end];
            let parameter = match codec {
                Codec::H264 => matches!(nal[0] & 31, 7 | 8),
                Codec::Hevc => matches!((nal[0] >> 1) & 63, 32..=34),
            };
            if parameter {
                if let Some(index) = self.units.iter().position(|unit| unit.as_slice() == nal) {
                    let unit = self.units.remove(index);
                    self.units.push(unit);
                } else if self.units.len() < 64
                    && self.bytes.saturating_add(nal.len() + 4) <= 256 * 1024
                {
                    self.bytes += nal.len() + 4;
                    self.units.push(nal.to_vec());
                } else {
                    // A partial cache is not a safe decoder initialization.
                    self.units.clear();
                    self.bytes = 0;
                    self.overflowed = true;
                    return;
                }
            }
            offset = end;
        }
    }
    /// A restarted decoder must not see an older SPS geometry first. Keep
    /// VPS/PPS dependencies by ID, but retire incompatible SPS configurations
    /// and their orphaned PPS entries. Only used at a format boundary.
    pub fn retain_hevc_configuration(&mut self, config: HevcConfiguration) {
        self.units.retain(|unit| {
            if (unit[0] >> 1) & 63 != 33 {
                return true;
            }
            hevc_sps(unit).is_none_or(|previous| previous == config)
        });
        let sps_ids = self
            .units
            .iter()
            .filter(|unit| (unit[0] >> 1) & 63 == 33)
            .filter_map(|unit| hevc_sps_with_id(unit).map(|(id, _)| 1u16 << id))
            .fold(0, |mask, bit| mask | bit);
        self.units.retain(|unit| {
            if (unit[0] >> 1) & 63 != 34 {
                return true;
            }
            let mut bits = ParameterBits::new(&unit[2..]);
            let referenced = bits.ue().and_then(|_| bits.ue());
            referenced.is_none_or(|id| id < 16 && sps_ids & (1u16 << id) != 0)
        });
        // PPS parsing can reference SPS metadata even in a header-only packet.
        self.units.sort_by_key(|unit| (unit[0] >> 1) & 63);
        self.bytes = self.units.iter().map(|unit| unit.len() + 4).sum();
    }
    #[cfg(target_os = "android")]
    pub fn decoder_initialization(
        &mut self,
        codec: Codec,
    ) -> Option<crate::codec_initialization::Initialization> {
        if self.overflowed {
            return None;
        }
        if codec == Codec::Hevc {
            let latest = self
                .units
                .iter()
                .rev()
                .find(|u| (u[0] >> 1) & 63 == 33)
                .and_then(|u| hevc_sps(u));
            if let Some(config) = latest {
                self.retain_hevc_configuration(config);
            }
        }
        crate::codec_initialization::initialize(&self.units, codec)
    }
    pub fn bytes(&self) -> Option<Vec<u8>> {
        if self.overflowed || self.units.is_empty() {
            return None;
        }
        let mut bytes = Vec::with_capacity(self.bytes);
        for unit in &self.units {
            bytes.extend_from_slice(&[0, 0, 0, 1]);
            bytes.extend_from_slice(unit);
        }
        Some(bytes)
    }
}

#[derive(Default)]
struct NalSummary {
    picture: bool,
    vps: bool,
    sps: Option<[u8; 3]>,
    hevc_configuration: Option<HevcConfiguration>,
    pps: bool,
    idr: bool,
}

// ITU-T H.265 Annex B / NAL-unit header. Single-layer Main-profile path.
// Parameter sets and IRAP are read from syntax, never from the requested codec.
fn hevc_annex_b(bytes: &[u8], read_configuration: bool) -> Option<NalSummary> {
    let mut summary = NalSummary::default();
    let mut offset = 0;
    for _ in 0..256 {
        while bytes.get(offset) == Some(&0) {
            offset += 1;
        }
        if offset < 2 || bytes.get(offset) != Some(&1) {
            return None;
        }
        let begin = offset + 1;
        let mut end = begin;
        while end < bytes.len()
            && !bytes[end..].starts_with(&[0, 0, 1])
            && !bytes[end..].starts_with(&[0, 0, 0, 1])
        {
            end += 1;
        }
        let nal = bytes.get(begin..end)?;
        let header = *nal.first()?;
        let second = *nal.get(1)?;
        if header & 0x81 != 0 || second & 0xf8 != 0 || second & 7 == 0 || nal.len() < 3 {
            return None;
        }
        // Supported H.265 VCL types. Reserved types stay conservative. Reuse
        // the existing scan; metadata cannot justify replacing a picture.
        summary.picture |= matches!((header >> 1) & 63, 0..=9 | 16..=21);
        match (header >> 1) & 63 {
            32 => summary.vps = true,
            33 => {
                // profile_tier_level begins after the first SPS RBSP byte.
                // Main/Main10 profile IDs are distinct from AVC profile IDs.
                let profile = *nal.get(3)? & 31;
                summary.sps = Some([profile, 0, 0]);
                if read_configuration {
                    summary.hevc_configuration = hevc_sps(nal);
                }
            }
            34 => summary.pps = true,
            16..=21 => summary.idr = true, // BLA, IDR or CRA random access.
            0..=31 | 35..=40 => {}
            _ => return None,
        }
        if end == bytes.len() {
            return Some(summary);
        }
        offset = end;
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HevcConfiguration {
    pub width: u32,
    pub height: u32,
    // Display dimensions after the SPS conformance window. Keep coded
    // dimensions separate for decoder limits and format-change detection.
    pub visible_width: u32,
    pub visible_height: u32,
    pub profile: u32,
    pub chroma: u32,
    pub bit_depth: u32,
}

pub fn hevc_configuration(bytes: &[u8]) -> Option<HevcConfiguration> {
    hevc_annex_b(bytes, true)?.hevc_configuration
}

// H.265 SPS syntax through bit_depth_chroma_minus8. Read only bounded metadata;
// emulation-prevention bytes are removed as bits are read, without copying NALs.
pub(crate) struct ParameterBits<'a> {
    bytes: &'a [u8],
    offset: usize,
    zeros: u8,
    byte: u8,
    left: u8,
}
impl<'a> ParameterBits<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            offset: 0,
            zeros: 0,
            byte: 0,
            left: 0,
        }
    }

    pub(crate) fn read(&mut self, count: u32) -> Option<u32> {
        if count > 32 {
            return None;
        }
        let mut value = 0;
        for _ in 0..count {
            if self.left == 0 {
                self.byte = *self.bytes.get(self.offset)?;
                self.offset += 1;
                if self.zeros == 2 && self.byte == 3 {
                    self.byte = *self.bytes.get(self.offset)?;
                    self.offset += 1;
                    if self.byte > 3 {
                        return None;
                    }
                    self.zeros = 0;
                }
                self.zeros = if self.byte == 0 {
                    (self.zeros + 1).min(2)
                } else {
                    0
                };
                self.left = 8;
            }
            self.left -= 1;
            value = (value << 1) | u32::from((self.byte >> self.left) & 1);
        }
        Some(value)
    }
    pub(crate) fn skip(&mut self, mut count: u32) -> Option<()> {
        while count != 0 {
            let step = count.min(32);
            self.read(step)?;
            count -= step;
        }
        Some(())
    }
    pub(crate) fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.read(1)? == 0 {
            zeros += 1;
            if zeros > 30 {
                return None;
            }
        }
        Some(((1u32 << zeros) - 1) + self.read(zeros)?)
    }
}
fn hevc_sps(nal: &[u8]) -> Option<HevcConfiguration> {
    hevc_sps_with_id(nal).map(|(_, config)| config)
}

fn hevc_sps_with_id(nal: &[u8]) -> Option<(u32, HevcConfiguration)> {
    let mut bits = ParameterBits::new(nal.get(2..)?);
    bits.skip(4)?;
    let layers = bits.read(3)?;
    if layers > 6 {
        return None;
    }
    bits.skip(1 + 2 + 1)?;
    let profile = bits.read(5)?;
    bits.skip(32 + 48 + 8)?;
    let mut sub = [(false, false); 6];
    for item in sub.iter_mut().take(layers as usize) {
        *item = (bits.read(1)? != 0, bits.read(1)? != 0);
    }
    if layers != 0 {
        bits.skip((8 - layers) * 2)?;
    }
    for (profile, level) in sub.into_iter().take(layers as usize) {
        if profile {
            bits.skip(88)?;
        }
        if level {
            bits.skip(8)?;
        }
    }
    let id = bits.ue()?;
    if id > 15 {
        return None;
    }
    let chroma = bits.ue()?;
    if chroma > 3 {
        return None;
    }
    let separate = chroma == 3 && bits.read(1)? != 0;
    let width = bits.ue()?;
    let height = bits.ue()?;
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return None;
    }
    let (mut visible_width, mut visible_height) = (width, height);
    if bits.read(1)? != 0 {
        let horizontal = bits.ue()?.checked_add(bits.ue()?)?;
        let vertical = bits.ue()?.checked_add(bits.ue()?)?;
        let sub_width = if !separate && (chroma == 1 || chroma == 2) {
            2
        } else {
            1
        };
        let sub_height = if !separate && chroma == 1 { 2 } else { 1 };
        visible_width = width.checked_sub(horizontal.checked_mul(sub_width)?)?;
        visible_height = height.checked_sub(vertical.checked_mul(sub_height)?)?;
        if visible_width == 0 || visible_height == 0 {
            return None;
        }
    }
    let bit_depth = bits.ue()?.checked_add(8)?;
    let chroma_depth = bits.ue()?.checked_add(8)?;
    if bit_depth > 16 || bit_depth != chroma_depth {
        return None;
    }
    Some((
        id,
        HevcConfiguration {
            width,
            height,
            visible_width,
            visible_height,
            profile,
            chroma,
            bit_depth,
        },
    ))
}

// No allocation or payload retention. A bounded linear scan accepts 3/4-byte
// start codes and leading/trailing zero bytes; unknown syntax stays unknown.
fn annex_b(bytes: &[u8]) -> Option<NalSummary> {
    fn start(bytes: &[u8], from: usize) -> Option<(usize, usize)> {
        let mut i = from;
        while i.checked_add(3)? <= bytes.len() {
            if bytes[i..].starts_with(&[0, 0, 0, 1]) {
                return Some((i, 4));
            }
            if bytes[i..].starts_with(&[0, 0, 1]) {
                return Some((i, 3));
            }
            i += 1;
        }
        None
    }
    let (mut offset, mut prefix) = start(bytes, 0)?;
    if bytes[..offset].iter().any(|b| *b != 0) {
        return None;
    }
    let mut summary = NalSummary::default();
    for _ in 0..256 {
        let begin = offset + prefix;
        let next = start(bytes, begin);
        let end = next.map_or(bytes.len(), |(i, _)| i);
        let nal = bytes.get(begin..end)?;
        let header = *nal.first()?;
        let kind = header & 31;
        if header & 128 != 0 || !(1..=23).contains(&kind) {
            return None;
        }
        // H.264 coded slice and data-partition NALs. The additional flag
        // does not change validation, decoder input or stream acceptance.
        summary.picture |= (1..=5).contains(&kind);
        match kind {
            7 => summary.sps = Some(nal.get(1..4)?.try_into().ok()?),
            8 => {
                if nal.len() < 2 {
                    return None;
                }
                summary.pps = true;
            }
            5 => {
                if nal.len() < 2 {
                    return None;
                }
                summary.idr = true;
            }
            _ => {}
        }
        match next {
            Some((i, p)) => {
                offset = i;
                prefix = p;
            }
            None => return Some(summary),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn metadata_alone_never_announces_a_replacement_picture() {
        let mut avc = super::Inspector::default();
        for bytes in [
            &[0, 0, 1, 0x06, 0x80][..],
            &[0, 0, 1, 0x09, 0x80],
            &[0, 0, 1, 0x68, 0x80],
        ] {
            assert!(!avc.receive(bytes).unwrap().picture);
        }
        assert!(avc.receive(&[0, 0, 1, 0x41, 0x80]).unwrap().picture);
        assert!(avc.receive(&[0, 0, 1, 0x65, 0x80]).unwrap().picture);

        let mut hevc = super::Inspector::default();
        hevc.enable_hevc(true);
        assert!(!hevc.receive(&[0, 0, 1, 0x40, 1, 0x80]).unwrap().picture);
        for kind in [34, 35, 39, 40, 22] {
            assert!(
                !hevc
                    .receive(&[0, 0, 1, kind << 1, 1, 0x80])
                    .unwrap()
                    .picture
            );
        }
        for kind in [0, 1, 16, 19, 21] {
            assert!(
                hevc.receive(&[0, 0, 1, kind << 1, 1, 0x80])
                    .unwrap()
                    .picture
            );
        }
    }

    use super::*;
    #[test]
    fn recovery_cache_keeps_parameters_but_never_picture_payloads() {
        let mut cache = ParameterCache::default();
        let sps = [0, 0, 0, 1, 0x67, 0x64, 0, 0x28];
        let pps = [0, 0, 1, 0x68, 0xee];
        let picture = [0, 0, 1, 0x65, 0x42, 0x43];
        let packet = [sps.as_slice(), pps.as_slice(), picture.as_slice()].concat();
        cache.observe(&packet, Codec::H264);
        let expected = [sps.as_slice(), &[0, 0, 0, 1, 0x68, 0xee]].concat();
        assert_eq!(cache.bytes().unwrap(), expected);
        cache.observe(&packet, Codec::H264);
        assert_eq!(cache.bytes().unwrap(), expected);
        cache.observe(&[0, 0, 1, 0xff], Codec::H264);
        assert_eq!(cache.bytes().unwrap(), expected);

        let mut cache = ParameterCache::default();
        let huge = [sps.as_slice(), vec![2; 256 * 1024].as_slice()].concat();
        cache.observe(&huge, Codec::H264);
        assert!(cache.bytes().is_none());
    }
    #[test]
    fn format_restart_keeps_new_sps_and_parameter_dependencies() {
        let old = sps_fixture(1920, 1200, 0, 8);
        let next = sps_fixture(1920, 1080, 0, 8);
        let vps = [0, 0, 1, 0x40, 1, 7];
        let pps = [0, 0, 1, 0x44, 1, 0xc0];
        let mut cache = ParameterCache::default();
        for unit in [&vps[..], &old, &pps, &next] {
            cache.observe(unit, Codec::Hevc);
        }
        cache.retain_hevc_configuration(hevc_configuration(&next).unwrap());
        let bytes = cache.bytes().unwrap();
        assert_eq!(hevc_configuration(&bytes), hevc_configuration(&next));
        let mut expected = ParameterCache::default();
        for unit in [&vps[..], &next, &pps] {
            expected.observe(unit, Codec::Hevc);
        }
        assert_eq!(bytes, expected.bytes().unwrap());
        // Repeated size changes cannot resurrect a retired SPS.
        cache.observe(&old, Codec::Hevc);
        cache.retain_hevc_configuration(hevc_configuration(&old).unwrap());
        assert_eq!(
            hevc_configuration(&cache.bytes().unwrap()),
            hevc_configuration(&old)
        );
    }
    #[test]
    fn format_restart_retires_only_pps_referencing_retired_sps_ids() {
        let old = sps_fixture_with_id(1920, 1200, 0, 8, 1);
        let next = sps_fixture(1920, 1080, 0, 8);
        let old_pps = [0, 0, 1, 0x44, 1, 0x4a]; // PPS 1 -> SPS 1
        let next_pps = [0, 0, 1, 0x44, 1, 0xc0]; // PPS 0 -> SPS 0
        let mut cache = ParameterCache::default();
        for unit in [&old[..], &old_pps, &next, &next_pps] {
            cache.observe(unit, Codec::Hevc);
        }
        cache.retain_hevc_configuration(hevc_configuration(&next).unwrap());
        let mut expected = ParameterCache::default();
        for unit in [&next[..], &next_pps] {
            expected.observe(unit, Codec::Hevc);
        }
        assert_eq!(cache.bytes(), expected.bytes());
    }

    fn sps_fixture(width: u32, height: u32, layers: u32, depth: u32) -> Vec<u8> {
        sps_fixture_with_id(width, height, layers, depth, 0)
    }
    fn sps_fixture_with_id(width: u32, height: u32, layers: u32, depth: u32, id: u32) -> Vec<u8> {
        sps_fixture_with_crop(width, height, layers, depth, id, None)
    }
    fn sps_fixture_with_crop(
        width: u32,
        height: u32,
        layers: u32,
        depth: u32,
        id: u32,
        crop: Option<[u32; 4]>,
    ) -> Vec<u8> {
        fn put(bits: &mut Vec<u8>, value: u32, count: u32) {
            for shift in (0..count).rev() {
                bits.push(((value >> shift) & 1) as u8);
            }
        }
        fn ue(bits: &mut Vec<u8>, value: u32) {
            let value = value + 1;
            let count = 32 - value.leading_zeros();
            bits.extend(std::iter::repeat_n(0, (count - 1) as usize));
            put(bits, value, count);
        }
        let mut bits = Vec::new();
        put(&mut bits, 0, 4);
        put(&mut bits, layers, 3);
        put(&mut bits, 1, 1);
        put(&mut bits, 1, 8); // profile_space=0, tier=0, profile_idc=Main
        bits.extend([0; 80]);
        put(&mut bits, 120, 8);
        for _ in 0..layers {
            put(&mut bits, 3, 2);
        }
        if layers > 0 {
            bits.extend(std::iter::repeat_n(0, ((8 - layers) * 2) as usize));
        }
        for _ in 0..layers {
            bits.extend([0; 96]);
        }
        ue(&mut bits, id);
        ue(&mut bits, 1); // SPS id, 4:2:0
        ue(&mut bits, width);
        ue(&mut bits, height);
        put(&mut bits, u32::from(crop.is_some()), 1);
        if let Some(crop) = crop {
            for offset in crop {
                ue(&mut bits, offset);
            }
        }
        ue(&mut bits, depth - 8);
        ue(&mut bits, depth - 8);
        put(&mut bits, 1, 1);
        while bits.len() % 8 != 0 {
            bits.push(0);
        }
        let mut nal = vec![0, 0, 0, 1, 0x42, 1];
        let mut zeros = 0;
        for chunk in bits.chunks(8) {
            let byte = chunk.iter().fold(0, |v, b| (v << 1) | b);
            if zeros == 2 && byte <= 3 {
                nal.push(3);
                zeros = 0;
            }
            nal.push(byte);
            zeros = if byte == 0 { zeros + 1 } else { 0 };
        }
        nal
    }
    #[test]
    fn recovery_cache_preserves_hevc_geometry_and_all_parameter_types() {
        let sps = sps_fixture(1920, 1080, 0, 8);
        let vps = [0, 0, 0, 1, 0x40, 1, 2];
        let pps = [0, 0, 0, 1, 0x44, 1, 2];
        let idr = [0, 0, 0, 1, 0x26, 1, 2];
        let packet = [
            vps.as_slice(),
            sps.as_slice(),
            pps.as_slice(),
            idr.as_slice(),
        ]
        .concat();
        let mut cache = ParameterCache::default();
        cache.observe(&packet, Codec::Hevc);
        let parameters = cache.bytes().unwrap();
        assert_eq!(
            parameters,
            [vps.as_slice(), sps.as_slice(), pps.as_slice()].concat()
        );
        assert_eq!(hevc_configuration(&parameters), hevc_configuration(&sps));
        let parsed = hevc_annex_b(&parameters, false).unwrap();
        assert!(parsed.vps && parsed.pps && !parsed.idr);
    }
    #[test]
    fn hevc_sps_geometry_handles_emulation_prevention_and_sublayers() {
        for layers in [0, 2, 6] {
            let bytes = sps_fixture(1920, 1080, layers, 8);
            assert!(bytes.windows(3).any(|b| b == [0, 0, 3]));
            assert_eq!(
                hevc_configuration(&bytes),
                Some(HevcConfiguration {
                    width: 1920,
                    height: 1080,
                    visible_width: 1920,
                    visible_height: 1080,
                    profile: 1,
                    chroma: 1,
                    bit_depth: 8,
                })
            );
            for end in 0..bytes.len() - 1 {
                assert!(hevc_configuration(&bytes[..end]).is_none());
            }
        }
        for (w, h) in [(0, 1080), (1920, 0), (16385, 1080)] {
            assert!(hevc_configuration(&sps_fixture(w, h, 0, 8)).is_none());
        }
        assert!(hevc_configuration(HEVC_KEY).is_none());
    }
    #[test]
    fn hevc_crop_preserves_coded_size_and_changes_visible_configuration() {
        let make =
            |crop| hevc_configuration(&sps_fixture_with_crop(1920, 1088, 0, 8, 0, Some(crop)));
        let cropped = make([0, 0, 0, 4]).unwrap();
        assert_eq!((cropped.width, cropped.height), (1920, 1088));
        assert_eq!(
            (cropped.visible_width, cropped.visible_height),
            (1920, 1080)
        );
        assert_eq!(Some(cropped), make([0, 0, 0, 4]));
        let changed = make([0, 0, 0, 8]).unwrap();
        assert_eq!((changed.width, changed.height), (1920, 1088));
        assert_ne!(changed, cropped);
        let offset = make([2, 2, 3, 1]).unwrap();
        assert_eq!((offset.visible_width, offset.visible_height), (1912, 1080));
        for bad in [
            [960, 0, 0, 0],
            [961, 0, 0, 0],
            [0, 0, 544, 0],
            [1 << 30, 1 << 30, 0, 0],
        ] {
            assert!(make(bad).is_none());
        }
        let old = sps_fixture_with_crop(1920, 1088, 0, 8, 1, Some([0, 0, 0, 4]));
        let next = sps_fixture_with_crop(1920, 1088, 0, 8, 0, Some([0, 0, 0, 8]));
        let mut cache = ParameterCache::default();
        cache.observe(&old, Codec::Hevc);
        cache.observe(&next, Codec::Hevc);
        cache.retain_hevc_configuration(changed);
        assert_eq!(cache.bytes().unwrap(), next);
    }
    #[test]
    fn cropped_fixture_reports_visible_and_coded_dimensions_separately() {
        let config =
            hevc_configuration(include_bytes!("../crates/video/fixtures/hevc-crop.sps")).unwrap();
        assert_eq!((config.width, config.height), (152, 152));
        assert_eq!((config.visible_width, config.visible_height), (150, 146));
    }
    #[test]
    fn full_chroma_main10_is_identified_from_actual_sps() {
        let config =
            hevc_configuration(include_bytes!("../crates/video/fixtures/hevc-444-10.sps")).unwrap();
        assert_eq!(
            (config.profile, config.chroma, config.bit_depth),
            (4, 3, 10)
        );
        assert!(config.width > 0 && config.height > 0);
    }
    const HEVC_KEY: &[u8] = &[
        0, 0, 0, 1, 0x40, 1, 0x80, // VPS
        0, 0, 1, 0x42, 1, 1, 1, 0x80, // SPS, Main profile
        0, 0, 1, 0x44, 1, 0x80, // PPS
        0, 0, 1, 0x26, 1, 0x80, // IDR_W_RADL
    ];
    #[test]
    fn hevc_requires_advertisement_and_observed_parameter_sets() {
        let mut inspector = Inspector::default();
        assert!(inspector.receive(HEVC_KEY).is_none());
        assert_eq!(inspector.snapshot().codec, None);
        inspector.enable_hevc(true);
        inspector.configure(protocol());
        assert!(inspector.snapshot().hevc_advertised);
        let mut with_aud = Inspector::default();
        with_aud.enable_hevc(true);
        let mut aud = vec![0, 0, 0, 1, 0x46, 1, 0x50];
        aud.extend_from_slice(HEVC_KEY);
        assert_eq!(with_aud.receive(&aud).unwrap().codec, Codec::Hevc);
        let info = inspector.receive(&HEVC_KEY[..7]).unwrap();
        assert_eq!(info.codec, Codec::Hevc);
        assert!(info.parameters); // Separate VPS must reach the decoder before IDR.
        assert!(!info.idr);
        let info = inspector.receive(&HEVC_KEY[7..]).unwrap();
        assert!(info.idr && info.parameters);
        let snapshot = inspector.snapshot();
        assert_eq!(snapshot.codec, Some(Codec::Hevc));
        assert_eq!(snapshot.sps_profile_idc, Some(1));
        assert!(snapshot.vps_observed && snapshot.parameter_sets_and_idr_observed);
        assert!(inspector.receive(&[0, 0, 1, 0x02, 1, 0x80]).is_some());
        assert!(inspector.receive(&[0, 0, 1, 0x02, 0, 0x80]).is_none());
        assert!(inspector.receive(KEY).is_none()); // No ambiguous codec change.
    }
    #[test]
    fn advertising_hevc_still_accepts_host_h264_fallback() {
        let mut inspector = Inspector::default();
        inspector.enable_hevc(true);
        assert_eq!(inspector.receive(KEY).unwrap().codec, Codec::H264);
        assert_eq!(inspector.snapshot().codec, Some(Codec::H264));
        assert!(inspector.receive(HEVC_KEY).is_none());
        for bytes in [
            &[0, 0, 1, 0x40][..],
            &[0, 0, 1, 0x40, 0, 0x80],
            &[0, 0, 1, 0x40, 9, 0x80],
        ] {
            let mut unknown = Inspector::default();
            unknown.enable_hevc(true);
            assert!(unknown.receive(bytes).is_none());
            assert_eq!(unknown.snapshot().codec, None);
        }
    }
    fn protocol() -> VideoProtocol {
        VideoProtocol {
            version: 0,
            message_size: 11,
            version_offset: 0,
            flag_offset: 4,
        }
    }
    fn metadata(key: bool) -> Vec<u8> {
        let mut v = vec![0; 11];
        v[4] = if key { 2 } else { 0 };
        v
    }
    // Header-only synthetic fixture, intentionally not a decodable picture.
    const KEY: &[u8] = &[
        0, 0, 0, 1, 0x67, 100, 0, 40, 0x80, 0, 0, 1, 0x68, 0x80, 0, 0, 0, 1, 0x65, 0x80,
    ];
    #[test]
    fn pinned_headers_do_not_enter_encoded_stream_and_key_flag_is_consumed() {
        let mut inspector = Inspector::default();
        inspector.configure(protocol());
        let _ = inspector.receive(&metadata(true));
        let _ = inspector.receive(KEY);
        let _ = inspector.receive(&[0, 0, 1, 0x41, 0x80]);
        let s = inspector.snapshot();
        assert_eq!(
            (
                s.metadata_messages,
                s.encoded_messages,
                s.key_chunks_announced,
                s.delta_chunks_announced
            ),
            (1, 2, 1, 1)
        );
        assert_eq!((s.annex_b_messages, s.idr_messages), (2, 1));
        assert!(s.parameter_sets_and_idr_observed);
        assert_eq!(s.sps_profile_idc, Some(100));
        assert_eq!(s.sps_level_idc, Some(40));
        assert!(!serde_json::to_string(&s).unwrap().contains("payload"));
    }
    #[test]
    fn headerless_key_fallback_matches_pinned_client_without_claiming_idr() {
        let mut inspector = Inspector::default();
        inspector.configure(protocol());
        let _ = inspector.receive(&[0, 0, 1, 0x41, 0x80]);
        let _ = inspector.receive(&[0, 0, 1, 0x41, 0x80]);
        let s = inspector.snapshot();
        assert_eq!(s.key_chunks_announced, 2);
        assert_eq!(s.idr_messages, 0);
        assert!(!s.parameter_sets_and_idr_observed);
    }
    #[test]
    fn malformed_unknown_and_excessive_nals_are_nonfatal_and_not_trusted() {
        for data in [
            vec![],
            vec![0, 0, 1],
            vec![0, 0, 1, 0x80],
            vec![0, 0, 1, 0x67],
            vec![0, 0, 1, 0x65],
            b"opaque data".to_vec(),
            [0, 0, 1, 0x41, 0x80].repeat(257),
        ] {
            let mut inspector = Inspector::default();
            let _ = inspector.receive(&data);
            let s = inspector.snapshot();
            assert_eq!(s.unrecognized_messages, 1);
            assert_eq!(s.annex_b_messages, 0);
            assert!(!s.parameter_sets_and_idr_observed);
        }
    }
    #[test]
    fn repeated_configuration_preserves_state_and_changed_protocol_resets_it() {
        let mut inspector = Inspector::default();
        inspector.configure(protocol());
        let _ = inspector.receive(KEY);
        inspector.configure(protocol());
        assert_eq!(inspector.snapshot().idr_messages, 1);
        let mut p = protocol();
        p.version = 1;
        inspector.configure(p);
        assert_eq!(inspector.snapshot().protocol_changes, 1);
        assert_eq!(inspector.snapshot().idr_messages, 0);
    }
    #[test]
    fn invalid_offsets_and_wrong_version_never_panic_or_match_metadata() {
        let mut inspector = Inspector::default();
        let mut p = protocol();
        p.flag_offset = u32::MAX;
        inspector.configure(p);
        let _ = inspector.receive(&metadata(true));
        assert_eq!(inspector.snapshot().metadata_messages, 0);
        inspector.configure(protocol());
        let mut v = metadata(true);
        v[0] = 3;
        let _ = inspector.receive(&v);
        assert_eq!(inspector.snapshot().metadata_messages, 0);
    }
}
