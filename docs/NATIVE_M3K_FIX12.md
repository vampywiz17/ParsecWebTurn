# M3k-fix12 / 0.14.12 — classify previously unknown DTLS errors

The user's fix11 report establishes a native `dtls-transport` failure:
ICE remains `connected`, DTLS and peer are `failed` at the actual callback,
and no data channels open. Cloudflare STUN produced one srflx candidate.
The optional collector recorded DTLS and SCTP startup failures but classified
both as `unknown`. This proves the failure stage, not its specific cause.

## Diagnostic changes only

The optional version-pinned library diagnostic now recognizes all 65 fixed,
parameter-free public DTLS 0.13 error variants using their exact Display values.
It also recognizes the exact ring `Unspecified` Display value and finite DTLS
peer alert descriptions. Ring's error explicitly carries no detailed reason:
`crypto-operation-unspecified` does not prove an invalid signature or key size.
The SCTP `DTLS not established` error is classified as a downstream startup error.

For the isolated `dtls::crypto` target only, trace diagnostics accept an exact
known `Picked an algorithm ...` value computed from ring's public verification
algorithm constants. The retained value is a fixed algorithm label, never a
certificate, key, signature or peer address. Default account mode enables none
of this. Although the Rust log facade's maximum level is trace for the opt-in
mode, the consumer rejects every target/level except the existing transport
warnings and this isolated crypto trace target. Unknown messages stay unknown;
the formatter still has a 512-byte bound, and the process-scoped history remains
bounded to 16 fixed events. Diagnostics never change connection decisions.

No DTLS role, cipher preference, signature acceptance, fingerprint verification,
STUN configuration or timeout is changed. No insecure compatibility setting is
enabled. The existing padded Parsec ufrag exception is unchanged.

Primary source contracts reviewed:
[fixed DTLS errors](https://github.com/webrtc-rs/webrtc/blob/v0.14.0/dtls/src/error.rs),
[peer alert formatting](https://github.com/webrtc-rs/webrtc/blob/v0.14.0/dtls/src/alert/mod.rs),
[signature verification diagnostic](https://github.com/webrtc-rs/webrtc/blob/v0.14.0/dtls/src/crypto/mod.rs)
and ring 0.17.14's local published `src/error/unspecified.rs`.
These are explicitly experimental, pinned library diagnostics, not a web standard.

## Verification and retest

`guest-dtls-failure-probe` runs an offline native/WASM peer exchange with an
intentionally wrong synthetic fingerprint. It must capture the real library's
fingerprint failure and ECDSA P-256 signature-verification label, retain fresh
failed transport states and close both peers. CI now requires this integration
proof in addition to all successful native bridge probes. During this synthetic
probe only, collection waits at most 500 ms for the upstream warning emitted
after the awaited failure callback; actual account behavior is unchanged.

Tests cover previously unmapped signature and generic crypto errors, exact alert
matching, rejection of injected suffixes and exact known algorithm metadata.

Extract into a separate folder, run `START-CLOUDFLARE-STUN-DIAGNOSTIC.cmd`, select
Connect once and close normally after the result. Share the resulting
`account-network-report.json`. This is still a diagnostic build; real-host
connectivity and decoding are not claimed to be fixed.
