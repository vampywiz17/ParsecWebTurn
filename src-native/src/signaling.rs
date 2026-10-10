//! Pinned Parsec JS ABI -> standard SDP/ICE. No account signaling or credentials
//! are logged. The private field mapping is isolated from the WebRTC engine.
use anyhow::{bail, Context, Result};
use std::{collections::VecDeque, net::IpAddr};
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;

const MAX_CANDIDATES: usize = 64;

// Intentionally no Debug/Serialize: these include ephemeral ICE passwords.
pub struct Credentials {
    pub ufrag: String,
    pub password: String,
    pub fingerprint: String,
}

pub struct Description {
    pub credentials: Credentials,
    pub mid: String,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialError {
    IceUfrag,
    IcePassword,
    FingerprintAlgorithm,
    FingerprintDigest,
}
impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for CredentialError {}

fn token(value: &str, min: usize, max: usize) -> Result<()> {
    if !(min..=max).contains(&value.len())
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"+/".contains(&c))
    {
        bail!("invalid ICE credential token");
    }
    Ok(())
}

// Isolated pinned-Parsec peer compatibility, NOT RFC 8839 ice-char grammar.
// Preserve identity exactly; stripping '=' would break STUN authentication.
fn parsec_padded_ufrag(value: &str) -> bool {
    value.len() == 8 && value.ends_with("==") && token(&value[..6], 6, 6).is_ok()
}

fn remote_ufrag(value: &str) -> Result<()> {
    if parsec_padded_ufrag(value) {
        Ok(())
    } else {
        token(value, 4, 256)
    }
}

/// Aggregate categories only: no bytes, positions, prefixes, suffixes or hashes.
/// In particular, '=' and URL-safe Base64 punctuation are not RFC 8839 ice-char.
fn token_shape(value: &str, min: usize) -> serde_json::Value {
    let mut counts = [0usize; 8];
    for byte in value.bytes() {
        let category = match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' => 0,
            b'=' => 1,
            b'-' | b'_' => 2,
            b' ' | b'\t' => 3,
            b'\r' | b'\n' => 4,
            0..=31 | 127 => 5,
            128..=255 => 6,
            _ => 7,
        };
        counts[category] += 1;
    }
    serde_json::json!({
        "length_valid":(min..=256).contains(&value.len()),
        "ice_char_bytes":counts[0], "equals_bytes":counts[1],
        "url_safe_punctuation_bytes":counts[2], "space_tab_bytes":counts[3],
        "cr_lf_bytes":counts[4], "other_control_bytes":counts[5],
        "non_ascii_bytes":counts[6], "other_ascii_punctuation_bytes":counts[7]
    })
}

impl Credentials {
    /// The pinned ia() splits SDP on LF, retaining the single SDP line-ending
    /// CR in its compact fields. Normalize that representation at the ABI edge;
    /// never strip embedded newlines or relax the standard ICE token grammar.
    pub fn normalize_compact(mut self) -> Self {
        for value in [&mut self.ufrag, &mut self.password, &mut self.fingerprint] {
            if value.ends_with('\r') {
                value.pop();
            }
        }
        if let Some((algorithm, digest)) = self.fingerprint.split_once(' ') {
            if algorithm.eq_ignore_ascii_case("sha-256") {
                self.fingerprint = format!("sha-256 {digest}");
            }
        }
        self
    }

    /// Shapes and fixed categories only, never credential bytes or hashes.
    #[cfg(any(test, feature = "diagnostics"))]
    pub fn diagnostic(&self) -> serde_json::Value {
        serde_json::json!({
            "ufrag_bytes":self.ufrag.len(), "password_bytes":self.password.len(),
            "fingerprint_bytes":self.fingerprint.len(),
            "ufrag_terminal_cr":self.ufrag.ends_with('\r'),
            "password_terminal_cr":self.password.ends_with('\r'),
            "fingerprint_terminal_cr":self.fingerprint.ends_with('\r'),
            "ufrag_valid":token(&self.ufrag,4,256).is_ok(),
            "password_valid":token(&self.password,22,256).is_ok(),
            "ufrag_shape":token_shape(&self.ufrag,4),
            "password_shape":token_shape(&self.password,22),
            "parsec_padded_ufrag_compatibility":parsec_padded_ufrag(&self.ufrag),
            "remote_validation_error":self.validate_parsec_remote().err().and_then(|e|e.downcast_ref::<CredentialError>().copied()),
            "validation_error":self.validate().err().and_then(|e|e.downcast_ref::<CredentialError>().copied())
        })
    }
    pub fn validate(&self) -> Result<()> {
        token(&self.ufrag, 4, 256).context(CredentialError::IceUfrag)?;
        self.validate_password_fingerprint()
    }
    /// Accept the observed peer's padded shape only at the Parsec remote edge.
    /// Local credential generation/SDP parsing retains strict RFC validation.
    pub fn validate_parsec_remote(&self) -> Result<()> {
        remote_ufrag(&self.ufrag).context(CredentialError::IceUfrag)?;
        self.validate_password_fingerprint()
    }
    fn validate_password_fingerprint(&self) -> Result<()> {
        token(&self.password, 22, 256).context(CredentialError::IcePassword)?;
        let digest = self
            .fingerprint
            .strip_prefix("sha-256 ")
            .context(CredentialError::FingerprintAlgorithm)?;
        let parts: Vec<_> = digest.split(':').collect();
        if parts.len() != 32
            || parts
                .iter()
                .any(|s| s.len() != 2 || !s.bytes().all(|c| c.is_ascii_hexdigit()))
        {
            return Err(CredentialError::FingerprintDigest.into());
        }
        Ok(())
    }
}

fn mid_valid(mid: &str) -> Result<()> {
    if mid.is_empty()
        || mid.len() > 64
        || !mid
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
    {
        bail!("unsupported SDP MID");
    }
    Ok(())
}

impl Description {
    // Narrow, single data-channel media section only; not a general SDP parser.
    // The resulting answer is parsed again by webrtc-rs's public SDP API.
    pub fn from_sdp(sdp: &str) -> Result<Self> {
        Self::parse(sdp, false)
    }
    /// Remote-only parsing for the isolated Parsec interoperability fixture.
    pub fn from_parsec_remote_sdp(sdp: &str) -> Result<Self> {
        Self::parse(sdp, true)
    }
    fn parse(sdp: &str, parsec_remote: bool) -> Result<Self> {
        if sdp.len() > 64 * 1024 {
            bail!("SDP exceeds the adapter limit");
        }
        let media: Vec<_> = sdp.lines().filter(|l| l.starts_with("m=")).collect();
        if media.len() != 1 || !media[0].starts_with("m=application ") {
            bail!("expected one SCTP application media section");
        }
        fn one(sdp: &str, prefix: &str) -> Result<String> {
            let mut values = sdp.lines().filter_map(|l| l.strip_prefix(prefix));
            let first = values.next().context("required SDP attribute missing")?;
            if values.next().is_some() {
                bail!("ambiguous duplicate SDP attribute");
            }
            Ok(first.to_owned())
        }
        let description = Self {
            credentials: Credentials {
                ufrag: one(sdp, "a=ice-ufrag:")?,
                password: one(sdp, "a=ice-pwd:")?,
                fingerprint: one(sdp, "a=fingerprint:")?,
            },
            mid: one(sdp, "a=mid:")?,
        };
        if parsec_remote {
            description.credentials.validate_parsec_remote()?;
        } else {
            description.credentials.validate()?;
        }
        mid_valid(&description.mid)?;
        Ok(description)
    }

    pub fn answer(&self, remote: &Credentials) -> Result<String> {
        remote.validate_parsec_remote()?;
        mid_valid(&self.mid)?;
        // Parsec's pinned ja() uses legacy DTLS/SCTP + sctpmap. RFC 8841's
        // UDP/DTLS/SCTP + sctp-port represents the same SCTP port with the
        // modern standardized spelling. Confirm interoperability in the probe.
        Ok(format!(
            "v=0\r\no=- 0 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\na=group:BUNDLE {mid}\r\n\
             m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\nc=IN IP4 0.0.0.0\r\n\
             a=ice-ufrag:{ufrag}\r\na=ice-pwd:{pwd}\r\na=ice-options:trickle\r\n\
             a=fingerprint:{fingerprint}\r\na=setup:active\r\na=mid:{mid}\r\n\
             a=sendrecv\r\na=sctp-port:5000\r\na=max-message-size:1048576\r\n",
            mid = self.mid,
            ufrag = remote.ufrag,
            pwd = remote.password,
            fingerprint = remote.fingerprint
        ))
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Candidate {
    ip: IpAddr,
    port: u16,
    from_stun: bool,
}

impl Candidate {
    pub fn new(ip: &str, port: u16, from_stun: bool) -> Result<Self> {
        let mut ip: IpAddr = ip.parse().context("candidate must contain an IP literal")?;
        if let IpAddr::V6(v6) = ip {
            if let Some(v4) = v6.to_ipv4_mapped() {
                ip = IpAddr::V4(v4);
            }
        }
        if port == 0 || ip.is_unspecified() || ip.is_multicast() {
            bail!("invalid candidate endpoint");
        }
        Ok(Self {
            ip,
            port,
            from_stun,
        })
    }

    pub fn from_sdp_line(line: &str) -> Result<Self> {
        if line.len() > 1024 {
            bail!("candidate line exceeds the adapter limit");
        }
        let fields: Vec<_> = line
            .strip_prefix("a=candidate:")
            .context("candidate prefix missing")?
            .split_ascii_whitespace()
            .collect();
        if fields.len() < 8
            || fields[1] != "1"
            || !fields[2].eq_ignore_ascii_case("udp")
            || fields[6] != "typ"
            || !matches!(fields[7], "host" | "srflx")
        {
            bail!("candidate cannot be represented by the pinned compact ABI");
        }
        Self::new(fields[4], fields[5].parse()?, fields[7] == "srflx")
    }

    fn rtc(&self, mid: &str, ufrag: &str) -> RTCIceCandidateInit {
        // Constants are the pinned JS W() mapping for remote compact fields,
        // not overrides of priorities generated by the native ICE engine.
        RTCIceCandidateInit {
            candidate: format!(
                "candidate:2395300328 1 udp 2113937151 {} {} typ {} generation 0 ufrag {}",
                self.ip,
                self.port,
                if self.from_stun { "srflx" } else { "host" },
                ufrag
            ),
            sdp_mid: Some(mid.to_owned()),
            sdp_mline_index: Some(0),
            username_fragment: Some(ufrag.to_owned()),
        }
    }
}

// Parsec releases buffered candidates only after both begin_p2p and its
// candidate-sync marker. Do not interpret the marker's placeholder address as
// an endpoint. Queues are per attempt and bounded even before synchronization.
pub struct CandidateGate {
    attempt: String,
    pending: VecDeque<Candidate>,
    seen: Vec<Candidate>,
    synced: bool,
    remote: Option<(String, String)>,
}

impl CandidateGate {
    pub fn new(attempt: &str) -> Result<Self> {
        if attempt.is_empty() || attempt.len() > 256 || attempt.chars().any(char::is_control) {
            bail!("invalid attempt identifier");
        }
        Ok(Self {
            attempt: attempt.into(),
            pending: Default::default(),
            seen: Vec::new(),
            synced: false,
            remote: None,
        })
    }
    fn check(&self, attempt: &str) -> Result<()> {
        if attempt != self.attempt {
            bail!("candidate belongs to a different attempt");
        }
        Ok(())
    }
    pub fn push(&mut self, attempt: &str, candidate: Candidate) -> Result<()> {
        self.check(attempt)?;
        if self.seen.contains(&candidate) {
            return Ok(());
        }
        if self.seen.len() == MAX_CANDIDATES {
            bail!("candidate limit reached");
        }
        self.seen.push(candidate.clone());
        self.pending.push_back(candidate);
        Ok(())
    }
    pub fn sync(&mut self, attempt: &str) -> Result<()> {
        self.check(attempt)?;
        self.synced = true;
        Ok(())
    }
    pub fn remote_ready(&mut self, attempt: &str, mid: &str, ufrag: &str) -> Result<()> {
        self.check(attempt)?;
        mid_valid(mid)?;
        remote_ufrag(ufrag)?;
        if self.remote.is_some() {
            bail!("remote description already registered");
        }
        self.remote = Some((mid.into(), ufrag.into()));
        Ok(())
    }
    pub fn pop_ready(&mut self) -> Option<RTCIceCandidateInit> {
        if !self.synced {
            return None;
        }
        let (mid, ufrag) = self.remote.as_ref()?;
        self.pending
            .pop_front()
            .map(|candidate| candidate.rtc(mid, ufrag))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn creds() -> Credentials {
        Credentials {
            ufrag: "abcd".into(),
            password: "abcdefghijklmnopqrstuv".into(),
            fingerprint: format!("sha-256 {}", ["AB"; 32].join(":")),
        }
    }
    #[test]
    fn padded_parsec_remote_identity_survives_sdp_and_candidate_gate() {
        let mut c = creds();
        c.ufrag = "dGVzdA==".into();
        assert!(c.validate().is_err());
        c.validate_parsec_remote().unwrap();
        let diagnostic = c.diagnostic();
        assert_eq!(diagnostic["parsec_padded_ufrag_compatibility"], true);
        assert!(diagnostic["remote_validation_error"].is_null());
        assert_eq!(diagnostic["validation_error"], "ice-ufrag");
        assert!(!diagnostic.to_string().contains(&c.ufrag));
        let local = Description {
            credentials: creds(),
            mid: "0".into(),
        };
        let sdp = local.answer(&c).unwrap();
        assert!(Description::from_sdp(&sdp).is_err());
        let parsed = Description::from_parsec_remote_sdp(&sdp).unwrap();
        assert_eq!(parsed.credentials.ufrag, c.ufrag);
        let mut gate = CandidateGate::new("padded-fixture").unwrap();
        gate.push(
            "padded-fixture",
            Candidate::new("192.0.2.1", 1234, false).unwrap(),
        )
        .unwrap();
        gate.sync("padded-fixture").unwrap();
        gate.remote_ready("padded-fixture", "0", &c.ufrag).unwrap();
        let candidate = gate.pop_ready().unwrap();
        assert_eq!(
            candidate.username_fragment.as_deref(),
            Some(c.ufrag.as_str())
        );
        assert!(candidate.candidate.ends_with(&format!("ufrag {}", c.ufrag)));
    }
    #[test]
    fn parsec_compatibility_rejects_other_grammar_and_preserves_authentication_checks() {
        for value in [
            "te==stAA",
            "dGVzdA=",
            "dGVzdA===",
            "dGVzdA== ",
            "dGVzdA==\n",
            "dGVzdA==\r\na=setup:passive",
            "test-_==",
            "短短==",
        ] {
            let mut c = creds();
            c.ufrag = value.into();
            assert!(c.validate_parsec_remote().is_err());
        }
        let mut c = creds();
        c.ufrag = "dGVzdA==".into();
        c.password = "abcdefghijklmnopqrstuv==".into();
        assert!(c.validate_parsec_remote().is_err());
        c.password = creds().password;
        c.fingerprint = "sha-256 AA:BB".into();
        assert!(c.validate_parsec_remote().is_err());
    }
    #[test]
    fn token_shape_classifies_rejection_without_disclosing_credential() {
        let synthetic = "Ab09+/==-_ \t\r\n\u{1}\u{7f}é:!";
        let shape = token_shape(synthetic, 4);
        for (name, count) in [
            ("ice_char_bytes", 6),
            ("equals_bytes", 2),
            ("url_safe_punctuation_bytes", 2),
            ("space_tab_bytes", 2),
            ("cr_lf_bytes", 2),
            ("other_control_bytes", 2),
            ("non_ascii_bytes", 2),
            ("other_ascii_punctuation_bytes", 2),
        ] {
            assert_eq!(shape[name], count);
        }
        assert_eq!(shape["length_valid"], true);
        for value in ["abc", &"a".repeat(257)] {
            assert_eq!(token_shape(value, 4)["length_valid"], false);
        }
        let mut c = creds();
        c.ufrag = "testAA==".into();
        let diagnostic = c.diagnostic();
        assert_eq!(diagnostic["ufrag_shape"]["equals_bytes"], 2);
        assert_eq!(diagnostic["validation_error"], "ice-ufrag");
        assert!(!diagnostic.to_string().contains(&c.ufrag));
        // Diagnosing padding or punctuation must not silently change acceptance.
        for value in ["testAA==", "test-_AA", "abcd efgh", "abcd:efgh"] {
            c.ufrag = value.into();
            assert!(c.normalize_compact().validate().is_err());
            c = creds();
        }
    }
    #[test]
    fn compact_sdp_line_endings_normalize_without_relaxing_ice_validation() {
        let mut c = creds();
        c.ufrag.push('\r');
        c.password.push('\r');
        c.fingerprint = c.fingerprint.replacen("sha-256", "SHA-256", 1) + "\r";
        let raw = c.diagnostic();
        assert_eq!(raw["ufrag_terminal_cr"], true);
        assert_eq!(raw["validation_error"], "ice-ufrag");
        let c = c.normalize_compact();
        c.validate().unwrap();
        assert_eq!(c.ufrag, creds().ufrag);
        assert_eq!(c.password, creds().password);
        assert_eq!(c.fingerprint, creds().fingerprint);
        let d = Description {
            credentials: creds(),
            mid: "0".into(),
        };
        Description::from_sdp(&d.answer(&c).unwrap()).unwrap();
        let mut boundary = creds();
        boundary.ufrag = "a".repeat(256) + "\r";
        boundary.password = "b".repeat(256) + "\r";
        boundary.normalize_compact().validate().unwrap();
    }
    #[test]
    fn compact_normalization_rejects_injection_and_diagnostics_do_not_retain_credentials() {
        for suffix in ["\n", "\r\r", "\r\na=setup:passive", " ", "\t"] {
            let mut c = creds();
            c.password.push_str(suffix);
            assert!(c.normalize_compact().validate().is_err());
        }
        let c = creds();
        let json = c.diagnostic().to_string();
        assert!(!json.contains(&c.ufrag));
        assert!(!json.contains(&c.password));
        assert!(!json.contains(&c.fingerprint));
        let mut bad = creds();
        bad.password = "short".into();
        assert_eq!(bad.diagnostic()["validation_error"], "ice-password");
        bad = creds();
        bad.fingerprint = "sha-1 AA:BB".into();
        assert_eq!(
            bad.diagnostic()["validation_error"],
            "fingerprint-algorithm"
        );
        bad.fingerprint = "sha-256 AA:BB".into();
        assert_eq!(bad.diagnostic()["validation_error"], "fingerprint-digest");
    }
    #[test]
    fn rejects_sdp_injection_and_malformed_fingerprints() {
        let mut c = creds();
        c.validate().unwrap();
        c.password.push_str("\r\na=setup:passive");
        assert!(c.validate().is_err());
        c = creds();
        c.fingerprint = "sha-256 AA:BB".into();
        assert!(c.validate().is_err());
        c = creds();
        c.ufrag = "短".into();
        assert!(c.validate().is_err());
    }
    #[test]
    fn answer_roundtrips_credentials_without_candidates() {
        let d = Description {
            credentials: creds(),
            mid: "0".into(),
        };
        let answer = d.answer(&creds()).unwrap();
        let extracted = Description::from_sdp(&answer).unwrap();
        assert_eq!(extracted.credentials.fingerprint, creds().fingerprint);
        assert!(!answer.contains("a=candidate:"));
        assert!(Description::from_sdp(&(answer.clone() + "a=ice-ufrag:abcd\r\n")).is_err());
        assert!(Description::from_sdp(&(answer + "m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n")).is_err());
    }
    #[test]
    fn candidates_wait_for_both_gates_and_reject_stale_attempts() {
        let mut gate = CandidateGate::new("test").unwrap();
        let candidate = Candidate::new("192.0.2.1", 1234, false).unwrap();
        assert!(gate.push("old", candidate.clone()).is_err());
        gate.push("test", candidate.clone()).unwrap();
        gate.push("test", candidate).unwrap();
        assert!(gate.pop_ready().is_none());
        gate.sync("test").unwrap();
        assert!(gate.pop_ready().is_none());
        assert!(gate.remote_ready("old", "0", "abcd").is_err());
        gate.remote_ready("test", "0", "abcd").unwrap();
        assert!(gate
            .pop_ready()
            .unwrap()
            .candidate
            .contains("192.0.2.1 1234 typ host"));
        assert!(gate.pop_ready().is_none());
        // New candidates after sync are immediately eligible too.
        gate.push(
            "test",
            Candidate::new("::ffff:192.0.2.2", 1234, true).unwrap(),
        )
        .unwrap();
        assert!(gate
            .pop_ready()
            .unwrap()
            .candidate
            .contains("192.0.2.2 1234 typ srflx"));
    }
    #[test]
    fn rejects_invalid_endpoints_and_bounds_candidate_history() {
        assert!(Candidate::new("1.2.3.4\r\na=setup:active", 1, false).is_err());
        assert!(Candidate::new("0.0.0.0", 1234, false).is_err());
        assert!(Candidate::new("192.0.2.1", 0, false).is_err());
        assert!(
            Candidate::from_sdp_line("a=candidate:1 1 udp 1 192.0.2.1 1234 typ relay").is_err()
        );
        let mut gate = CandidateGate::new("test").unwrap();
        for port in 1..=64 {
            gate.push("test", Candidate::new("192.0.2.1", port, false).unwrap())
                .unwrap();
        }
        assert!(gate
            .push("test", Candidate::new("192.0.2.1", 65, false).unwrap())
            .is_err());
    }
}
