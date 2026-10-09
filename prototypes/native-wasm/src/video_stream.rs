//! Bounded inspection of pinned Parsec video framing, not a decoder.
//! Proprietary metadata is separate from H.264 Annex B byte-stream syntax.
use crate::backend::VideoProtocol;
use serde::Serialize;

#[derive(Clone, Default, Serialize)]
pub struct Snapshot {
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
                ..Default::default()
            },
            ..Default::default()
        };
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.clone()
    }

    pub fn receive(&mut self, bytes: &[u8]) {
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
                    return;
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
        let Some(summary) = annex_b(bytes) else {
            self.snapshot.unrecognized_messages =
                self.snapshot.unrecognized_messages.saturating_add(1);
            return;
        };
        self.snapshot.annex_b_messages = self.snapshot.annex_b_messages.saturating_add(1);
        if let Some([profile, constraints, level]) = summary.sps {
            self.snapshot.sps_observed = true;
            self.snapshot.sps_profile_idc = Some(profile);
            self.snapshot.sps_constraint_flags = Some(constraints);
            self.snapshot.sps_level_idc = Some(level);
        }
        self.snapshot.pps_observed |= summary.pps;
        if summary.idr {
            self.snapshot.idr_messages = self.snapshot.idr_messages.saturating_add(1);
        }
        self.snapshot.parameter_sets_and_idr_observed = self.snapshot.sps_observed
            && self.snapshot.pps_observed
            && self.snapshot.idr_messages != 0;
    }
}

#[derive(Default)]
struct NalSummary {
    sps: Option<[u8; 3]>,
    pps: bool,
    idr: bool,
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
    use super::*;
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
        inspector.receive(&metadata(true));
        inspector.receive(KEY);
        inspector.receive(&[0, 0, 1, 0x41, 0x80]);
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
        inspector.receive(&[0, 0, 1, 0x41, 0x80]);
        inspector.receive(&[0, 0, 1, 0x41, 0x80]);
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
            inspector.receive(&data);
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
        inspector.receive(KEY);
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
        inspector.receive(&metadata(true));
        assert_eq!(inspector.snapshot().metadata_messages, 0);
        inspector.configure(protocol());
        let mut v = metadata(true);
        v[0] = 3;
        inspector.receive(&v);
        assert_eq!(inspector.snapshot().metadata_messages, 0);
    }
}
