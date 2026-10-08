# M3b: managed native offer through the WASM import

This stage implements `env::parsec_web_new_attempt` with the pinned seven-i32,
void-result ABI. A WASM caller can create a real native WebRTC offer and receive
its ICE username/password and SHA-256 DTLS fingerprint through shared memory.
The native worker retains the peer, offer and three negotiated ordered channels.
It does not install a remote description, gather session candidates or claim a
host connection. `parsec_web_begin_p2p` and `parsec_web_add_candidate` are still
explicit unsupported boundaries. Login HTTP/WebSocket and media decoding remain
unimplemented. The normal M2 window remains offline.

## Completion and ownership

- The host import validates attempt ID, capacities, guest ranges, alignment and
  buffer non-overlap before scheduling work. There is one active attempt per
  backend and at most eight offer workers across the diagnostic process.
- Network/offer preparation runs on its own native worker with a Tokio runtime,
  without a Wasmtime Store/Caller or the shared backend lock. Cancellation and
  result publication share a bounded output lease; cancellation takes the lease
  away before guest buffers may be reused. Old workers cannot publish afterward.
- All three credential lengths are checked before writing any credential.
  A failed/cancelled request writes empty C strings and `err=1`. Success writes
  all C strings and `err=0`, then signals `csync`. ICE secrets are not serialized.
- The signal follows the audited Matoya CAS handshake, not a generic event flag.
  If completion wins before wait registration, CAS 0->1 latches it and the guest
  skips its wait. If the guest wins CAS first, notification must wait for the
  actual waiter to register: a single early notify would be lost. The adapter
  retries `SharedMemory::atomic_notify` with 1ms sleeps for at most one second;
  it does not copy the JavaScript shim's unbounded busy-spin.
- Dedicated signal words use aligned AtomicU32 host access throughout the active
  handshake, never concurrent AtomicU8 host access. Other guest buffers retain
  the existing checked atomic-byte access. Tests use actual Wasmtime wait/notify.
- Native peer creation and offer preparation each have a five-second deadline.
  A successfully prepared unused offer is retained for at most 30 seconds, then
  expires as a failure. Close has a two-second deadline. Disconnect, destruction
  or dropping the handle requests close; the diagnostic explicitly waits for it.
  Cancellation may spend up to one second completing a pending guest handshake,
  but it does not wait for network work while holding the backend lock.
- Status 20 means pending preparation/offer, never connected. Failed attempts
  return to idle on status polling. Success does not create a status-0 event.

The guest must retain its output buffers until completion, as required by the
original asynchronous ABI. Invalid pointers trap before scheduling work; they
are not treated as normal network failures. A waiter that fails to register
within the bounded handshake deadline is a diagnostic failure, not a silently
successful completion. Full interactive shutdown and reconnection remain later
integration requirements.

## Actual import proof

```powershell
./parsec-native-wasm.exe guest-offer-probe ./guest-offer-local.json
```

The command executes a controlled WAT fixture with the exact imported function
signature and the Matoya CAS / `memory.atomic.wait32` handshake. It calls the real
Rust bridge, validates the native credentials, checks all three created channels
and pending status, then cancels the retained peer and waits for actual cleanup.
It reports `guest_completion_verified`, `credentials_validated`, `peer_closed`
and `worker_finished`. It does not print credentials, SDP, addresses or payloads.

This is a real WASM-to-native bridge test, **not** a connection attempt initiated
by the original Parsec login UI. The report explicitly sets
`original_parsec_guest_attempt_exercised: false`, `parsec_host_connected: false`
and `video_decoded: false`. The original pinned core's independent allocator,
boot diagnostics and M3a native transport probes remain in Windows CI.

Additional tests exercise completion before wait, notification in the CAS/wait
registration gap, overlapping outputs, insufficient capacity and cancellation
followed by immediate guest-buffer reuse. No fault-injection hooks are embedded
in the normal bridge.

## Next integration

Replace the retained offer's cancel-only wait with a bounded command path for
`begin_p2p` and candidate/sync operations using the existing M3a adapter. Attach
actual local candidate callbacks to guest events, implement native control
framing, and distinguish transport readiness from decoded/presented video.
Implement native HTTP/WebSocket for login separately. No real session or
hardware video performance claim is made at this milestone.

References:

- Audited `weblib.js::parsec_web_new_attempt`, `parsec.js::ia` and Matoya's
  `mty_wait` / `mty_signal` in the retained 2026-10-08 input snapshot.
- [Wasmtime shared memory APIs](https://docs.wasmtime.dev/api/wasmtime/struct.SharedMemory.html).
- [Public native WebRTC peer API](https://docs.rs/webrtc/0.14.0/webrtc/peer_connection/struct.RTCPeerConnection.html).

The Parsec ABI is private and pinned; the Wasmtime/WebRTC platform APIs are
documented. This implementation does not rewrite the original WASM or change
the production Tauri application.
