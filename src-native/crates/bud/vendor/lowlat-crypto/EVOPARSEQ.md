# BUD-only scope of the pinned crypto dependency

MIT-licensed `nomi-san/lowlat` at
`5f8db5689e55de0d396b0520824caa9fc053dd86`, `crates/crypto`.
The original license is retained. `src/record.rs` is unchanged; `src/lib.rs`
only omits `pub mod cert;`. Retained upstream tests cover credential encodings,
key/nonce material, redacted Debug output, AES-128/256-GCM compatibility with
the protocol core, and rejection of modified records.

EvoParseq's BUD seam calls `credentials`, `key_material`, `transaction_seed`
and `Record::new`. Credentials use the original system RNG and encodings.
Records use the original ring AES-GCM implementation, lengths and nonces.
There is no cryptographic algorithm or wire-format change.

The upstream `cert` module generates the identity for lowlat's separate browser
transport. That transport is not imported here: EvoParseq's WebRTC transport
uses its existing `webrtc`/`dtls` certificate and fingerprint verification.
Neither the BUD wrapper, retained lowlat-client/net/core, nor application calls
the upstream certificate APIs. `cert.rs` and its exclusive p256/ecdsa/x509-cert/
sha2/zeroize dependencies are therefore omitted, rather than carrying an unused
feature that restores the old crypto-bigint chain under `--all-features`.

Both the application and standalone BUD workspace patch the same git package
to this directory. Cargo-deny still checks all features; no advisory exception
is introduced. Upstream browser-certificate consumers must not use this subset.
