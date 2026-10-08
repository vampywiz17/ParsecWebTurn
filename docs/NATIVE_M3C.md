# M3c: WASM/native attempt signaling

The isolated Rust prototype retains the native peer created by
`parsec_web_new_attempt`, installs the compact remote answer through
`parsec_web_begin_p2p`, and passes validated candidates through
`parsec_web_add_candidate`. The original Parsec core is unchanged. This
milestone does not change the production Tauri app.

## Implemented boundary

- The attempt identifier is preserved and stale commands are rejected.
- Native signaling work runs outside the WASM Store and backend mutex.
- Candidates wait for both remote-description installation and the pinned
  Parsec sync marker. The marker's address and port are never used as endpoints.
- UDP host/srflx candidates from the native ICE engine become type-8 guest
  events with the audited private ABI fields. This diagnostic configures no
  external STUN/TURN service and rejects unrepresentable candidate types.
- The pinned JS's 500 ms sync acknowledgment is reproduced once per attempt;
  cancellation prevents late publication into a later attempt.
- Guest event polling copies before consuming. An undersized or invalid
  destination cannot silently drop a candidate.
- Commands/events are bounded to 64 each, the backend event staging queue to
  32. Native diagnostic receipts have limits of 16 messages, 1 MiB per message
  and 4 MiB total. Overflow fails rather than growing indefinitely.
- A diagnostic attempt expires after 30 seconds; individual native operations
  have five-second deadlines and peer closure has a two-second deadline.
  This lifetime is intentionally unsuitable for a persistent remote session.

Public webrtc-rs APIs perform ICE, DTLS and SCTP. The compact fields are mapped
to modern SDP using RFC 8839/8841 semantics. The Parsec `parsec_web_*` ABI and
its sync acknowledgment are snapshot-specific compatibility code, not a
public standard. No SDP, credential or candidate address is logged in reports.

## Acceptance diagnostic

```powershell
./parsec-native-wasm.exe guest-session-probe guest-session.json
```

A controlled WASM fixture calls the actual native imports. A local native
answering peer receives a synthesized offer from the returned credentials
and the actual offer MID; its remote candidates come only from guest event
polling. The guest receives answer credentials and compact candidates through
the actual imports, including candidates buffered before begin and a sync
marker with an intentionally invalid address pointer.

The diagnostic verifies real connected peer state, all three negotiated
channels, exact binary contents in both directions (six messages), a retained
event after failed copy, a sync acknowledgment, and closure of both peers.
The native probe connects only to a local test peer, using ordinary host UDP
candidate interfaces; a restrictive firewall may prevent this test.

The fixture's straight-line functions remain fuel-limited and its atomic
wait is limited to four seconds. Calls can continue beyond the generic
bootstrap's five-second watchdog; the probe's native waits remain bounded.

## Limits and next boundary

The report always distinguishes native transport from a Parsec session:
`original_parsec_guest_attempt_exercised: false`,
`parsec_host_connected: false`, `video_decoded: false`.
The backend stays pending (20), even after native transport connects, because
Parsec control-channel initialization/framing is not wired yet. Diagnostic
binary payloads are not interpreted as Parsec video/control messages.

Remaining work includes native authenticated HTTP/signaling, an attempt from
the original guest UI, real host control/message framing, then hardware
decoder surfaces and native GPU presentation. Existing OpenGL UI drawing
does not prove remote video rendering or hardware decoding performance.

App version and upstream WASM version should remain separate. The currently
pinned original core reports `150-104a`; its SHA-256 is
`d663dd96df477c65479fb93eb88756c7fcafff581cc93be563625cd195a4b4a6`.
An upstream update must pass the ABI/compatibility checks before it is adopted.

## Verified build, 2026-10-08

Source: `c86e07a612dc653ed249b88747f4f98f38dc9130`.
[Windows CI run 37821713825](https://github.com/vampywiz17/ParsecWebTurn/actions/runs/37821713825)
passed formatting, all 26 tests, Clippy with warnings denied, the release build,
the unchanged original core's diagnostics, both prior native transport probes,
the guest offer probe and the new guest session probe.

The same release EXE also passed the local Windows guest session probe:
three compact candidates in each direction, three open channels, six exact
binary messages, sync acknowledgment and copy retry verified. Both peers
closed; the attempt worker finished without failure. The connected and
closed snapshots are separate, and the closed snapshot reports zero open
channels and `transport_connected: false`.

These results prove the controlled guest/native transport boundary only.
They do not establish real Parsec host compatibility or hardware decoding.

References:

- [Public native WebRTC peer API](https://docs.rs/webrtc/0.14.0/webrtc/peer_connection/struct.RTCPeerConnection.html).
- [RFC 8839: ICE SDP usage](https://www.rfc-editor.org/rfc/rfc8839.html).
- [RFC 8841: SDP for SCTP over DTLS](https://www.rfc-editor.org/rfc/rfc8841.html).
- Pinned `weblib.js::parsec_web_begin_p2p`, `parsec_web_add_candidate`,
  `parsec_web_poll_events` and `parsec.js::U`, `ja`, `ka`.
