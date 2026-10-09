# M3k-fix13 / 0.14.13 — opt-in legacy Parsec RSA compatibility

The fix12 account diagnostic reached connected ICE, then failed DTLS while
selecting the RSA PKCS#1 SHA-256 verifier restricted to 2048–8192-bit keys.
That error alone does not establish the key size. Separately, inspection of
previously supplied handshake captures found a 1024-bit RSA certificate sent
from the Parsec host port. This supports a legacy key-size incompatibility;
the latest host certificate has not been directly measured.

## Explicit compatibility option

`account` and `account-network-audit` accept `--legacy-rsa-1024`, disabled by
default. It may be combined with `--cloudflare-stun` in either order. Unknown
or duplicate flags are rejected. The backend and attempt reports explicitly
record `legacy_rsa_1024_enabled`; the option is not persisted or auto-enabled
following an error. Offline probes retain strict defaults.

The implementation uses the documented public webrtc-rs 0.14
`SettingEngine::allow_insecure_verification_algorithm` API. Its pinned DTLS
implementation accepts the legacy RSA PKCS#1 SHA-256/SHA-512 verification
path for signatures shorter than 256 bytes. RSA SHA-384 remains strict.
Cryptographic signature verification and SDP certificate-fingerprint checking
remain active. This does not enable insecure hashes or SHA-1 negotiation,
disable HTTPS verification, change DTLS roles, or change ICE routing policy.

This is an explicitly authorized legacy security exception, not a modern
cryptographic recommendation. RSA-1024 provides weaker security than RSA-2048.
The lower-level API is native library configuration, not a WebRTC web API.

Primary contracts:
[public SettingEngine API](https://github.com/webrtc-rs/webrtc/blob/v0.14.0/webrtc/src/api/setting_engine/mod.rs),
[pinned DTLS verifier](https://github.com/webrtc-rs/webrtc/blob/v0.14.0/dtls/src/crypto/mod.rs),
[modern TLS/DTLS recommendations](https://www.rfc-editor.org/rfc/rfc9325.html).

## Verification and user test

Synthetic public-only RSA fixtures verify that strict mode rejects a valid
1024-bit signature, legacy mode accepts it, altered messages/signatures still
fail, and strict mode accepts RSA-2048. No private keys, real host certificates,
packet captures or account data are included in the repository.

The existing real native/WASM wrong-fingerprint integration probe also runs
with compatibility enabled. It checks that the option reaches the worker,
the actual fingerprint mismatch is captured, no channels open, and both peers
close. Successful native bridge probes continue to use strict defaults.

Extract the test ZIP into a separate folder. Run
`START-LEGACY-RSA-CLOUDFLARE-DIAGNOSTIC.cmd`, connect once, then close normally
and share `account-network-report.json`. This launcher explicitly enables both
the legacy option and Cloudflare STUN. `START-ACCOUNT.cmd` and the existing
Cloudflare launcher retain strict verification. Real Parsec host connectivity
and video decoding require the user's test; this build does not claim they
have already been verified.

## Verified build

Source `4ea108cc3c01331bb70ed5215b23ec31dd333ced` passed
[Windows CI 37919183769](https://github.com/vampywiz17/ParsecWebTurn/actions/runs/37919183769):
87 tests, formatting, strict Clippy, release compilation and all native bridge
probes. Both strict and compatibility modes rejected the deliberately wrong
fingerprint with zero open channels and both peers closed. The actual library
reported `certificate-fingerprint-mismatch` in both controlled tests.

The local offline original-login test completed nine synthetic steps and
presented 15 hardware-accelerated UI frames, with no external requests,
no start error, clean window shutdown and no rejected thread spawns. The
normal report left legacy compatibility disabled and omitted optional library
diagnostics. Real-host connection and video remain unverified pending retest.
