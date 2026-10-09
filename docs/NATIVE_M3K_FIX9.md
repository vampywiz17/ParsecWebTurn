# M3k-fix9 / 0.14.9 — preserve padded remote ICE identity

The user's fix8 report identifies exactly six standard ICE-character bytes and
two equals signs in the eight-byte remote ufrag. All length checks pass; no
whitespace, CR/LF, control or non-ASCII bytes are present. Attempt IDs match and
the password passes strict validation. The strict ufrag grammar is the confirmed
local rejection point, before native connectivity checks. The report records
counts rather than character positions, so terminal Base64 padding is a strong
hypothesis that this fix explicitly checks, not a directly recorded fact.

## Standards and compatibility boundary

[RFC 8839 section 5.4](https://www.rfc-editor.org/rfc/rfc8839.html#section-5.4)
does not include equals signs in `ice-char`.
[RFC 4648 section 4](https://www.rfc-editor.org/rfc/rfc4648.html#section-4)
describes Base64 padding, but does not make padded tokens valid ICE SDP tokens.
This is a documented, isolated Parsec interoperability exception in the native
prototype, not a claim of standards compliance for the peer's credential format.

Only remote Parsec ufrags with exactly six standard ICE characters followed by
`==` are additionally accepted. The original token is retained unchanged in the
answer, candidate username fragment and native ICE/STUN authentication. It is
never decoded, re-encoded or stripped of padding. Other punctuation, interior
equals, additional padding, whitespace, embedded newlines and malformed lengths
are still rejected. Password and SHA-256 fingerprint validation are unchanged;
TLS/DTLS certificate verification and cryptographic checks are not disabled.

Strict `Credentials::validate` and local SDP parsing retain RFC grammar. Remote
validation is explicitly named `validate_parsec_remote`; the candidate gate and
remote answer use the same narrow rule. The redacted report keeps the strict
`validation_error` and adds `remote_validation_error` plus the fixed boolean
`parsec_padded_ufrag_compatibility`, so a compatibility acceptance is not mislabeled
as strict validity. No credentials, fingerprints, hashes or raw SDP are serialized.

## Verification approach

Two new tests verify exact token preservation through SDP/candidate mapping and
rejection of unrelated malformed credentials. The existing actual WASM/native
session probe configures a synthetic peer's static padded ufrag using the public
[`SettingEngine::set_ice_credentials`](https://github.com/webrtc-rs/webrtc/blob/v0.14.0/webrtc/src/api/setting_engine/mod.rs)
API. It must complete real local ICE authentication, DTLS/SCTP and six binary
messages; manipulating SDP alone is not accepted as proof. Control/buffer probes
retain standard credentials. No real account, host or external server is used.

The previous invalid-remote-ufrag containment fixture now uses interior equals
instead of the newly accepted terminal pattern, retaining its error/redaction
checks. CI explicitly requires the padded-ufrag native negotiation proof.

Run `START-NETWORK-DIAGNOSTIC.cmd` from a new extracted folder, log in and try
Connect once. Close normally and share `account-network-report.json`. If the
token matches the narrow padding rule, this removes the confirmed early grammar
rejection. Real host connectivity, STUN/TURN and media decoding remain unverified;
a subsequent failure can identify the next boundary rather than prove completion.
