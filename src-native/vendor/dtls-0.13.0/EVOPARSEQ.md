# Targeted DTLS cryptography patch

MIT/Apache-2.0 `dtls` 0.13.0 from crates.io, upstream revision
`a1f8f1919235d8452835852e018efd654f2f8366` (`dtls/`). Original licenses and
source tests are retained; no alternate DTLS engine is added.

Local changes:

- P-256 and P-384 0.14, elliptic-curve 0.14 and SEC1 0.8 replace the DTLS
  curve dependencies. The resolved crypto-bigint is 0.7.10 (the fix for
  RUSTSEC-2026-0387 is available from 0.7.6).
- The two NIST curves use the documented fallible `Generate::try_generate`
  system-RNG API. RNG failure returns a handshake error. X25519 keeps its
  existing RNG/API. No random generator or curve arithmetic is implemented here.
- Enable ECDH, the existing precomputed-table optimization and standard/system
  RNG support only; unused curve ECDSA/PEM features are omitted. DTLS signing
  and signature verification continue to use the existing ring implementation.
- Tests compare wire-format points and shared secrets with ring for P-256,
  P-384 and X25519, and reject malformed/off-curve NIST public keys.

The active DTLS 0.13 connection API, record ciphers, certificate/fingerprint validation,
curve identifiers/order, ICE integration, SCTP, retransmission timers and
queues are unchanged. AES/SHA/HMAC record processing keeps its existing
versions. This is not a dimpl migration and makes no throughput/latency claim.

The application and isolated TURN tests both select this patch.

## Omitted binary state export/import

`State::marshal_binary` and `State::unmarshal_binary` have no callers in the
application, vendored WebRTC transport or retained DTLS sources/tests. They
export/import a local connection snapshot; they are not DTLS record/handshake
parsers. Those two unused methods and their exclusive `bincode` dependency
are omitted, together with the unused serde derives on `SerializedState`.
This is a deliberate reduction of the vendored API, not a serialization-format
migration. External binary-state export consumers cannot use this patch.

The internal `State::clone` -> `serialize`/`deserialize` snapshot remains
unchanged: `DTLSConn::connection_state` uses it, including during the WebRTC
remote-certificate/fingerprint checks. `init_cipher_suite`, key export and
`KeyLogData` serialization also remain. A regression test verifies snapshot
identity, exported key material and independent sequence/certificate storage.

The BUD dependency now uses the same pinned lowlat credentials/record source
without its unrelated browser-certificate module. See
`src-native/crates/bud/vendor/lowlat-crypto/EVOPARSEQ.md`. No advisory exception
or replacement crypto implementation is added.
