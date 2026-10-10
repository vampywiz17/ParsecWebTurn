# Isolated TURN regression fixture

This separate crate tests the same patched ICE/WebRTC crates as the Windows app
without linking the Parsec core, UI, codecs or production settings. It is never
shipped. `bash tests/turn-integration/run.sh` runs on an isolated Ubuntu CI runner
with coturn, OpenSSL and Rust installed. The script installs a one-day fixture CA
in that runner's trust store; do not run it on a personal machine unnecessarily.

Coverage: RFC 8656 stream framing/padding under fragmented and coalesced reads,
close during a partial frame, malformed framing, untrusted certificate rejection,
trusted certificate hostname mismatch, and actual bidirectional 64 KiB messages
on three negotiated SCTP channels through coturn using UDP, TCP and TLS. Relay-only
is used in the fixture to prove traffic traverses TURN; production keeps ICE `all`.

These fixtures prove transport interoperability, not that every Parsec host,
provider or corporate network will accept a session. Test a real host separately.
