# Isolated SCTP-only correction

Base: official crates.io `webrtc` 0.14.0 archive, SHA-256
`08fd686c0920ac08f3a57eacc48e31f0e4ca1ffefba4478784606f78c14e83ad`.
All 92 original archive files are retained. The SCTP corrections are listed
in `SCTP-ONLY.patch`; `ice_candidate_pair.rs` additionally exposes read-only
`local()` and `remote()` getters for selected-route telemetry, without changing
ICE behavior. MIT/Apache-2.0 license texts are copied from the
upstream v0.14.0 tag. The Parsec WASM binary is not changed.

This is a locally maintained library extension, not an upstream-supported API.
`SettingEngine::set_data_channel_only(true)` explicitly selects RFC 8831 SCTP
encapsulated in authenticated DTLS, without the unrelated DTLS-SRTP extension.
It omits SRTP offers, profile selection, RTP endpoints and SRTP session startup.
Local and remote non-application SDP is rejected before state changes.
The default remains false and preserves upstream SRTP negotiation/requirements.

The DTLS library, signature verification, certificate presence check and SDP
fingerprint comparison remain unchanged and must pass before Connected state.
No unsecured SCTP, fabricated SRTP profile or encryption fallback is added.
RSA-1024 acceptance is still a separate, default-off user-authorized option.

`RTCDtlsTransport::negotiated_srtp_profile` is a local read-only diagnostic
extension exposing the actual DTLS result. None is also returned before DTLS
establishment; only connected fixture peers use it as absence evidence.

Tests: SCTP-only guest/native binary exchange without a negotiated SRTP profile;
wrong-fingerprint rejection with strict and legacy RSA settings; RTP SDP
rejection before state changes; default peers still negotiate an SRTP profile.
Primary transport contract: https://www.rfc-editor.org/rfc/rfc8831.html

Keep this patch isolated to the standalone native prototype. On upstream
migration, re-audit and remove it if a supported equivalent becomes available.
