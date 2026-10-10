# Local TURN stream transport integration

Base: crates.io `webrtc-ice` 0.14.0 (MIT OR Apache-2.0). Original licenses and
upstream sources remain in this directory. This patch does not change ICE pair
priorities, nomination, candidate filtering or the application's `all` policy.

- `src/turn_stream.rs`: implements the existing `webrtc-util::Conn` contract
  over Tokio TCP and rustls TLS. RFC 8656 sections 7 and 11 define the STUN and
  ChannelData framing and stream padding; there is no additional RFC 4571
  length prefix. Fragmented reads, coalesced messages, serialized writes and
  terminal framing failures are handled explicitly. Close interrupts a pending
  read and shuts down the stream.
- `src/agent/agent_gather.rs`: allows `turn:...?transport=tcp` and
  `turns:...?transport=tcp` alongside the unchanged UDP path. The allocation
  still requests UDP toward the peer, exactly as defined by TURN. ICE continues
  to advertise a UDP relay candidate. RFC 6062 TCP allocations and ICE-TCP are
  different features and are not implemented by this patch.
- TLS validates hostname and certificate chain with rustls, public trust roots
  and OS-trusted roots. The legacy Parsec RSA compatibility option affects DTLS
  only and cannot weaken TURN TLS. TLS establishment is limited to ten seconds.
- Stream transport ownership follows allocation lifetime, including failed
  setup and explicit candidate shutdown. The already-connected server address
  is passed to the upstream TURN client to avoid a second round-robin DNS
  lookup choosing a different endpoint.
- The companion `turn-0.11.0` patch selects reliable STUN transactions for TCP
  and TLS: no UDP retransmissions, a 39.5-second transaction timeout, and immediate
  pending-request failure on transport closure (RFC 8489 section 6.2.2).
- `src/lib.rs` exports the transport for isolated regression tests.
- Read-only native telemetry: relay candidates retain their allocation URL (without
  credentials) and actual client-to-server transport. Authenticated, transaction-
  and source-matched STUN success responses update RTT and response counters for
  both ICE roles. No extra probes, candidate changes or nomination changes are
  introduced. Host/remote candidates do not fabricate relay transport information.
  The coturn fixture verifies selected-pair RTT and UDP/TCP/TLS provenance.

Limitations: relay gathering currently follows upstream's IPv4 allocation
path. TLS uses rustls; OS roots are imported, but this is not a Windows Schannel
connection and does not reproduce every enterprise trust policy. No HTTP proxy
or CONNECT tunneling is added. `stuns:` discovery remains unsupported.

Tests: `src-native/tests/turn_stream.rs` verifies framing, padding, malformed
frames, shutdown and untrusted TLS rejection. `tests/turn-integration` uses a
separate loopback coturn server and a temporary trusted CA to exchange real
DTLS/SCTP messages over UDP, TCP and TLS, and reject hostname mismatch. It is
never embedded in the client executable.

References: [TURN RFC 8656](https://www.rfc-editor.org/rfc/rfc8656),
[STUN RFC 8489](https://www.rfc-editor.org/rfc/rfc8489),
[TURN URI RFC 7065](https://www.rfc-editor.org/rfc/rfc7065).
