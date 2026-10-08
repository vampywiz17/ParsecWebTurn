# M3d: pinned Parsec control framing

The isolated Rust prototype now implements a limited control layer on top of
the native ICE/DTLS/SCTP transport. The original WASM and the production Tauri
app remain unchanged. This is a compatibility adapter for the audited private
Parsec protocol, not an IETF/WebRTC wire standard.

## Framing and integration

The pinned `parsec.js` functions `O`, `P`, `Q`, `da` and control-channel
`onopen`/`onmessage` define a 13-byte header: three signed big-endian 32-bit
fields followed by one message-type byte. Video protocol metadata uses a
separate little-endian layout; the two must not be confused.

When the guest sets video metadata before creating an attempt, the native
control channel sends the type-11 configuration after it opens. The current
prototype requests the audited 1920×1080/60 Hz fallback; these are requested
settings, not measured or decoded stream properties. A startup event and
status 0 are published only after this send succeeds. This mirrors the web
client's control-open behavior; it does not prove authentication, admission
by a real host or decoded video. The transport-only M3c mode remains pending.

Supported outbound input: keys, mouse buttons, scroll, relative mouse motion,
individual gamepad buttons/axes/removal and the type-9 input mapped by the
pinned client. Absolute mouse mapping and packed gamepad state are explicitly
rejected until implemented. Input values are validated before transmission.

Supported incoming messages: status (10), encode latency (21), rumble (20),
clipboard-request events (16), host mode (28), guest/self metadata (25).
Events and metadata pass through the actual WASM imports. Encode latency
uses the audited microseconds-to-milliseconds conversion; no decoder/network
latency or frame size is invented. Clipboard events alone do not implement
clipboard synchronization.

Malformed frames, invalid JSON, oversized messages and unsupported cursor,
user-data, media or text-channel bridges fail explicitly. Unknown message
types follow the pinned JS default ignore branch after header validation.
Text lengths use UTF-8 byte counts and include the terminal NUL. This avoids
the pinned JS `Q` function's truncation of non-ASCII text by UTF-16 code-unit
count. No generic SDP or packet rewriting is added.

Bounds from M3c still apply: 64 native events, 32 staged guest events, 16 native
receipts, 1 MiB per frame and 4 MiB queued payload. Guest lists are capped at
256 object entries. Failed guest event copies retain their events. Cancelled
attempts cannot publish later control events; initialization uses a weak
channel reference to avoid a callback ownership cycle. Network sends never
hold the WASM Store or backend lock.

## Verification and remaining work

```powershell
./parsec-native-wasm.exe guest-control-probe ./guest-control.json
```

A controlled WASM fixture configures video metadata and performs the existing
compact ICE exchange. A local native test peer verifies the startup header and
configuration, sends six synthetic control frames, and checks a key packet
sent through `parsec_web_send_message`. Guest status, polling, metadata and
metrics imports are checked against those frames. Unsupported absolute mouse
input is rejected. Both native peers are closed after the probe.

Reports deliberately retain `parsec_host_connected: false`,
`original_parsec_guest_attempt_exercised: false` and `video_decoded: false`.
The data represents a synthetic test host, not a captured Parsec session.
Native attempts still have a 30-second diagnostic lifetime.

Next: complete cursor/user-data buffers and control lifecycle, add native
authenticated HTTP/signaling for the original guest UI, then verify a real
host before integrating hardware decoder surfaces/GPU presentation. Media
frames are not silently accepted as decoded output. Graceful protocol-level
shutdown, network inactivity detection and full viewport/input behavior are
also future work; native cancellation/resource cleanup already remains bounded.

References: the retained 2026-10-08 `parsec.js` and `weblib.js` snapshot,
[documented native data-channel API](https://docs.rs/webrtc/0.14.0/webrtc/data_channel/struct.RTCDataChannel.html).

## Verified build, 2026-10-08

Source: `5ae5919c0cfb52d8f1040da74cb35eff48993b48`.
[Windows CI run 37824392982](https://github.com/vampywiz17/ParsecWebTurn/actions/runs/37824392982)
passed formatting, all 29 tests, Clippy with warnings denied, the release build,
the unchanged core diagnostics and every native transport/guest probe.

The same release EXE passed the local Windows `guest-control-probe`: startup
configuration, exact WASM key packet, all six synthetic host control frames,
status/rumble/clipboard-request events, guest/self data, host mode and encode
latency verified. Unsupported absolute mouse input was rejected. Three native
channels opened and both peers closed; the worker finished without failure.
No GPU/video performance or real-host compatibility claim follows from this.
