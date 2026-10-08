# M3e: cursor and user-data guest buffers

The isolated native Rust prototype now handles the pinned control protocol's
cursor (9) and user-data (17) messages. The original Parsec WASM and production
Tauri app remain unchanged. This is private, snapshot-specific compatibility
code; the network transport still uses documented native WebRTC APIs.

## Audited ABI

`parsec.js::ea` reads a 34-byte cursor header: image length at offset 16,
width/height at 20/22, signed position at 24/26, hotspot at 28/30 and flags
at 32. The cursor body starts at 34. Fields are big endian; relative/hidden
flags are 256/512. A metadata-only cursor has key 0 and no image allocation.
The image is opaque at this stage, not decoded or displayed.

`fa` reads user-data length and ID from the 13-byte control header and exposes
the body unchanged, including embedded zero bytes or a terminating NUL.
An empty user-data body still receives an opaque handle, matching the pinned
JS buffer-store behavior. The declared body must fit inside the received
frame; unused trailing bytes follow the audited view boundaries and remain
included in the retention budget.

`parsec_web_get_buffer_size` returns the payload length or 0 for an unknown
handle. `parsec_web_get_buffer` copies to checked shared guest memory and only
then consumes the handle. An invalid destination retains the buffer for
retry; a consumed/unknown handle is a no-op and never writes guest memory.
The ABI supplies no destination length: validation checks the guest's linear
memory bounds, not an unavailable allocation-size contract. The guest must
allocate at least the size it requested.

The Rust store retains `Bytes` slices of incoming frames without another
Rust-side payload copy. Limits: 16 outstanding handles, 1 MiB per retained
frame and 4 MiB total retained frame bytes, including unused headers/trailers.
Even empty handles count toward the count limit. Exhaustion rejects the
operation; buffer-budget failure in control processing cancels the attempt
and clears buffers/events rather than silently dropping data.

Handles are never recycled across a new attempt, disconnect, destroy or
reinitialization of the same backend. Handle-space exhaustion fails before
wraparound. Disconnect removes pending buffer events and payloads. Destroy,
new-attempt preparation and failed control processing also release payloads.
Guest copies remain atomic-byte accesses to shared linear memory, as in the
existing runtime bridge.

`parsec_web_send_user_data` sends type-17 UTF-8 text on the native control
channel when status permits. The payload length uses UTF-8 bytes and includes
the terminal NUL. This implements the guest transport import, not operating
system clipboard read/write or full clipboard synchronization.

## Acceptance diagnostic

```powershell
./parsec-native-wasm.exe guest-buffer-probe ./guest-buffer.json
```

A controlled WASM fixture establishes the existing local ICE/DTLS/SCTP
connection and verifies the M3d control exchange. A native test peer then
sends binary user data, opaque synthetic cursor-image bytes, a metadata-only
cursor, an empty user-data body and a payload intentionally left unread.
The fixture verifies exact copied bytes and cursor fields, retry after an
invalid pointer, one-shot consumption, no writes for missing handles,
outbound Unicode text and buffer release via the actual disconnect import.
The retained native attempt is independently cancelled/closed by the harness;
the disconnect assertion specifically verifies guest buffer/event cleanup.

Unit tests verify negative/truncated cursor and user-data lengths, buffer
range/retention/count budgets, zero-copy slices and handle lifetime across
backend reinitialization. The cursor fixture is deliberately opaque data;
this does not prove image-format decoding or native cursor presentation.

Reports retain `parsec_host_connected: false`,
`original_parsec_guest_attempt_exercised: false`, `video_decoded: false`,
`cursor_rendered: false` and `clipboard_synchronized: false`.
The native attempt still has a 30-second diagnostic lifetime.

Next: native authenticated HTTP/signaling for the original guest UI and
complete control/session lifecycle, then real-host interoperability and
hardware decoder surfaces/GPU presentation. Native cursor presentation and
clipboard integration can build on these verified buffer imports.

References: retained 2026-10-08 `parsec.js::ea`, `fa`, `T`, `X.Z`, `X.X`,
`weblib.js::parsec_web_get_buffer_size`, `parsec_web_get_buffer` and
`parsec_web_send_user_data`; actual import signatures from the pinned core's
`inspect` report. No private browser diagnostics or JavaScript engine is added.

## Verified build

Source: `93d91fbc573aaf7e616260423f1c185273ac34c7`, prototype version 0.8.0.
[Windows CI](https://github.com/vampywiz17/ParsecWebTurn/actions/runs/37827192142)
passed formatting, all 35 tests, Clippy with warnings denied, release build,
the original pinned core's bootstrap probes and every native transport/import
probe. The unchanged core identifies as 150-104a, SHA-256
`d663dd96df477c65479fb93eb88756c7fcafff581cc93be563625cd195a4b4a6`.

The downloaded release executable also passed `guest-buffer-probe` locally:
all eleven buffer verification flags were true, all three channels opened,
both peers closed and the native worker finished. Cursor rendering, clipboard
synchronization, original-guest connection and decoded video remain false.
This evidence covers synthetic local interoperability, not a real Parsec host.
