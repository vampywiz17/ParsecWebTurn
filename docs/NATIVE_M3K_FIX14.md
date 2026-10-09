# M3k-fix14 / 0.14.14 — SCTP-only DTLS without SRTP

The user's fix13 report confirms `legacy_rsa_1024_enabled: true` and the legacy
RSA PKCS#1 SHA-256 verifier. The earlier crypto-operation failure is gone.
ICE reaches connected, but transport startup now fails with
`srtp-profile-missing`, then SCTP reports DTLS not established.

The pinned webrtc-rs 0.14 transport checks SRTP profile selection immediately
after the DTLS handshake, before certificate-fingerprint validation. Therefore
this report proves progress past the earlier DTLS signature failure; it does
not yet prove successful fingerprint validation or an established data channel.

## Transport correction

RFC 8831 data channels use SCTP over DTLS. SRTP protects RTP media and is not
required for a connection carrying only SCTP. This prototype carries Parsec
control/video/audio as three binary data channels, without RTP transceivers.

There is no supported SCTP-only switch in the reviewed webrtc-rs 0.14 API;
the reviewed v0.17.1 transport retains the same unconditional requirement.
Instead of fabricating a negotiated SRTP profile or suppressing an error after
failure, the isolated prototype now uses a vendored 0.14.0 with a small local
extension. This is explicitly a maintained library correction, not an upstream
public API or undocumented Chromium feature.

`SettingEngine::set_data_channel_only(true)` selects SCTP-only behavior:
no use_srtp extension is offered, no SRTP profile is required, and no RTP
endpoints or SRTP sessions start. Non-application local/remote SDP is rejected
before signaling changes. DTLS signature verification, remote certificate
presence and SDP fingerprint validation remain mandatory before Connected.
Upstream defaults preserve SRTP behavior for regular peers.

The original crate archive was verified against the existing Cargo.lock SHA256.
Only four source files differ; original source, licenses, provenance and the
reviewable patch are under `prototypes/native-wasm/vendor/webrtc-0.14.0`.
This dependency override applies only to the standalone prototype.
The pinned Parsec WASM, main/dev, ICE policy, and default-off legacy RSA option
are unchanged. This fixes a transport requirement, not video decoding itself.

Primary contracts:
[RFC 8831](https://www.rfc-editor.org/rfc/rfc8831.html),
[upstream DTLS transport](https://github.com/webrtc-rs/webrtc/blob/v0.14.0/webrtc/src/dtls_transport/mod.rs).

## Verification and retest

The native/WASM guest probe now exchanges binary data over three connected
SCTP-only channels without a negotiated SRTP profile. Both strict and legacy
RSA negative tests must reject a wrong fingerprint on this no-SRTP path.
A policy test rejects RTP SDP before signaling-state changes. Separate default
native peers must still negotiate SRTP and exchange data.

Extract into a separate folder and run
`START-LEGACY-RSA-CLOUDFLARE-DIAGNOSTIC.cmd`. Connect once, close normally and
share `account-network-report.json`. Real host connectivity and video decoding
remain unverified until this user test.

## Verified build

Source `6ac47627296d3778e95eb6ac0f41bf91140908f0` passed
[Windows CI 37920786086](https://github.com/vampywiz17/ParsecWebTurn/actions/runs/37920786086):
88 tests, formatting, strict Clippy, release compilation and all bridge probes.
The SCTP-only native/WASM probe exchanged six binary messages over three
channels, with no negotiated SRTP profile and both peers closed. Both strict
and legacy RSA wrong-fingerprint probes rejected the peer on this SCTP-only
path with zero open channels. The separate default transport probe still
negotiated an SRTP profile and successfully exchanged binary messages.

The local offline original-login test completed nine synthetic steps and
presented 15 accelerated UI frames. There were no external requests, start
errors or rejected thread spawns, and the native window closed cleanly.
Default account settings still leave RSA compatibility disabled and omit
optional library diagnostics. Actual host connection and video await retest.
