# M3g: native WebSocket signaling transport

The isolated Rust prototype implements the five pinned Matoya WebSocket imports
with tokio-tungstenite's documented RFC 6455 transport. The original WASM is
unchanged, and there is no JavaScript, browser or WebView2. All original-core
modes remain offline; only the acceptance probe enables its exact loopback
server address/port. This is the transport foundation, not authenticated
Parsec signaling or a real host session.

## ABI and behavior

The import shapes come from the pinned `inspect` report and Matoya worker.
The public [Parsec Matoya header](https://github.com/parsec-cloud/libmatoya/blob/main/src/matoya.h)
also documents the u16 upgrade status, millisecond timeout and destroy-by-reference
contract. Native connect returns a nonzero opaque handle, writes 101 on upgrade
and reports the HTTP status for a rejected handshake. Destroy zeros the guest's
handle slot, requests a normal close and joins the owned worker. Unknown handles
cannot read/write or silently become a subsequent connection; IDs never recycle.

Read returns the pinned browser's `MTY_Async` values: 0 for a copied text message,
1 after normal closure/queue drain, 2 on timeout and 3 for failure. Unlike the
browser wrapper, empty text is delivered, queued text drains before a normal
close, and insufficient capacity retains the message for retry. Invalid output
ranges trap before consuming data. Text copies preserve UTF-8 and append NUL.
Embedded NUL and binary signaling messages are unsupported and fail explicitly.
Write returns true only after the native sink accepts/flushes the text frame;
this does not prove remote application processing. Queued writes have deadlines
and expire without later transmission. A failed in-flight write closes the
socket rather than claiming delivery.

RFC ping/pong handling uses the library's protocol logic. Separately, the worker
sends the pinned Parsec application's text `__ping__` every 60 seconds; this is
an application convention, not an RFC ping. The probe accelerates that timer to
50 ms only on its synthetic server. Pong/control frames do not become guest
application messages. Application responses remain ordinary text.

GetCloseCode retains the peer's normal close code. Locally aborted failures use
1006 for abnormal transport loss, 1009 for capacity/budget errors, 1003 for an
unsupported message and 1002 for a protocol error. Those local values are not
evidence that a remote close frame with that code was received.

## Bounds and lifecycle

Four handles maximum; each connection has one owned OS thread/current-thread
Tokio runtime, a 16-message/1 MiB inbox and an eight-command send queue. Text and
frame limits are 64 KiB; library write buffering is bounded to 128 KiB. Read,
connect and send waits are capped at five seconds. A zero read timeout polls;
zero connect timeout uses the five-second cap. No sleep-based socket polling is
used: native async I/O and condition variables wait for events.

Cancellation precedes new work, attempts a close handshake for up to one second,
and drops the TCP stream if the peer does not cooperate. An in-flight bounded
send may finish first. The last network owner signals all sockets before joining
them, so those time budgets run concurrently. Worker failure wakes readers.
Closed handles count toward the cap until the guest destroys them.

This stage rejects nonempty custom headers/proxy values instead of pretending
to implement the browser wrapper's ignored parameters. No external endpoints,
TLS, cookies, redirects, account credentials or persisted profiles are enabled.
Authenticated WSS and its policy are later work. URL/message/error contents are
not logged or serialized.

## Acceptance diagnostic

```powershell
./parsec-native-wasm.exe guest-websocket-probe ./guest-websocket.json
```

The actual imports are called by a controlled WASM program. The local server
checks Unicode input/output, an empty text frame, RFC ping/pong and the
application keepalive. Guest calls check invalid/small-buffer retry and empty
queue timeout, then deliver final queued text and the peer's normal close code.
A second socket verifies destroy/close acknowledgment and an invalid stale
handle. A third violates the message-size limit. A fourth refuses upgrade with
403. Each worker and server is closed/joined, with no remaining handles.

`authentication_integrated`, `external_requests_enabled`,
`original_parsec_guest_signaling_exercised`, `parsec_host_connected` and
`video_decoded` remain false. This does not prove WSS/TLS interoperability or
original-core authenticated signaling. Earlier original-core bootstrap and
native ICE/DTLS/SCTP/control/buffer/HTTP probes run separately.

Next: authenticated original-core HTTP/WSS policy and session lifecycle,
then a real-host test and hardware decoder/GPU presentation.

References: retained 2026-10-08 Matoya main/worker files, pinned WASM import
signatures, public Matoya header, [tokio-tungstenite documentation](https://docs.rs/tokio-tungstenite/latest/tokio_tungstenite/).
The Matoya/Parsec ABI adapter is snapshot-specific; RFC 6455 handling is delegated
to the native WebSocket library.
