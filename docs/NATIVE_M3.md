# M3a: native compact signaling adapter

M3a is the first part of the session-integration milestone. It implements the
Parsec-specific compact ICE/DTLS parameter mapping in Rust and tests the resulting
standard WebRTC connection between two real native peers. It does **not** log in
to Parsec, use an account, connect to a Parsec host, decode video or modify the
M2 UI/graphics path. Live guest attempt imports remain explicit boundaries.

## Why this comes before connecting the guest

The retained WASM does not receive a complete remote SDP through its web ABI.
The audited `weblib.js` passes an attempt ID, ICE username/password, DTLS
fingerprint, and candidate address/port/sync fields. The audited `parsec.js`
functions `ia`, `ja`, `ka` and `W` construct the browser transport from them.
Rust must supply this behavior before the guest can use its native transport.
Authentication/HTTP and live attempt lifetime integration are separate work.

`src/signaling.rs` isolates this snapshot-specific mapping:

- Validate bounded ICE credentials, SHA-256 DTLS fingerprints and MID tokens
  before embedding them in SDP. Reject line injection and ambiguous attributes.
- Generate a candidate-free answer with the remote peer's actual credentials
  and fingerprint, the offered MID and the audited active DTLS role/SCTP port.
- Use RFC 8841 `UDP/DTLS/SCTP`, `webrtc-datachannel` and `sctp-port:5000` rather
  than copying the old browser shim's `DTLS/SCTP`/`sctpmap` SDP spelling.
- Preserve fingerprint verification and the native ICE implementation's local
  candidate priorities. Remote compact candidate constants mirror the audited
  mapping; no local priority tweaking or route-selection override is added.
- Buffer at most 64 unique remote candidates per attempt, release only after
  remote-description installation **and** the Parsec sync marker, reject stale
  attempt IDs, and accept subsequent trickled candidates. The marker is a
  control operation, never a connection to its placeholder address.
- Normalize IPv4-mapped IPv6 addresses. This first adapter accepts IP literals
  and UDP host/srflx compact remote fields; unsupported endpoints/types fail
  explicitly. It is not a general SDP parser or a complete ICE signaling API.

ICE passwords, full SDP, addresses and payloads are not included in the probe
report. Candidate objects and credential structs deliberately omit Debug and
Serialize implementations.

## Proof command

```powershell
./parsec-native-wasm.exe signaling-probe ./signaling-local.json
```

Unlike `transport-probe`, this command does **not** hand the test answer's full
SDP to the client peer. It extracts only the compact fields and candidate
endpoints from a locally generated peer, reconstructs the answer through the
adapter, installs it through public `webrtc-rs` APIs, then adds the buffered
candidates through `add_ice_candidate` after sync.

Success requires real ICE/DTLS/SCTP establishment, both peers connected, all
three externally negotiated ordered channels (`control` 0, `video` 1, `audio` 2)
open, six bidirectional verified binary messages, at least one compact candidate
applied, and both peer connections explicitly closed. The report records
`signaling: parsec-compact-parameters-to-standard-sdp`,
`parsec_host_connected: false` and `video_decoded: false`.

The synthetic messages are transport evidence, not Parsec control/video frames.
The other test peer still receives the full native offer: this is a focused
client-side answer/candidate mapping test, not a simulated Parsec server.
The modern SDP spelling has not yet been tested against an actual Parsec host.
Two instances of the same WebRTC library do not prove cross-implementation
compatibility. A real-host session remains a required later acceptance test.

The probe uses ordinary local IPv4 interfaces, no loopback override, STUN, TURN,
credentials, clipboard or browser. It may need normal Windows firewall permission.
Negotiation/exchange has a 20-second limit and cleanup an additional five seconds;
failure and timeout close both peers before reporting an error.

## Next integration

Attach a managed native attempt to the shared WASM backend. Implement the
`parsec_web_new_attempt` asynchronous completion and `MTY_SignalPtr` contract,
`parsec_web_begin_p2p`, `parsec_web_add_candidate`, candidate polling and real
control-channel framing. Keep backend locks out of awaited transport operations;
bound queues and preserve cleanup/reinitialization semantics. Separately implement
the documented native HTTP/TLS platform bridge needed by the original login UI.
Do not fabricate a connected status until a live channel has actually opened.
Native decoding/presentation follows, with the existing GPU-first requirements.

## References

- Pinned audited inputs: `dist/parsec-audit-2026-10-08/parsec.js` and `weblib.js`;
  unchanged WASM SHA-256 `d663dd96df477c65479fb93eb88756c7fcafff581cc93be563625cd195a4b4a6`.
- [Public webrtc-rs peer APIs](https://docs.rs/webrtc/0.14.0/webrtc/peer_connection/struct.RTCPeerConnection.html).
- [RFC 8841: SCTP/DTLS SDP](https://www.rfc-editor.org/rfc/rfc8841.html).
- [RFC 8839: ICE SDP](https://www.rfc-editor.org/rfc/rfc8839.html).
- [RFC 8445: ICE](https://www.rfc-editor.org/rfc/rfc8445.html).

The Parsec field/attempt ABI remains private and pinned; using standard SDP,
ICE/DTLS/SCTP and public native APIs does not turn it into an official Parsec SDK.
