# M3k-fix21 / 0.14.21 — complete video-channel messages

The user's 0.14.20 report confirmed established ICE/DTLS/SCTP, incoming control
and audio, but zero video messages and a generic worker failure. It has no event
timestamps, so it cannot establish whether a click caused the disconnect.

The pinned original parsec.js startup configuration and negotiated channel IDs
match the native adapter. The bundled upstream webrtc 0.14.0 callback reader,
however, allocates only u16::MAX (65,535) bytes per complete SCTP message. Its
read error closes that channel before the application's 1 MiB guard/counters.
This is a concrete receive-path defect, not a confirmed diagnosis of the user's
particular host session. No raw report, credentials or media payload is committed.

All three native channels now use upstream's documented detach_data_channels /
RTCDataChannel::detach / DataChannel::read_data_channel APIs. Detached and callback
receive modes are not mixed. Each reader has a reusable bounded 1 MiB buffer;
the existing 16-message / 4 MiB application queue limits remain. Oversized messages,
read errors, unexpected channel closure and queue overflow fail the connection
with fixed categories. Local cancellation/peer cleanup does not become a remote
channel failure. First failure information is retained. No library patch is added.
The supported SCTP send-size setting is also bounded to the adapter's existing
1 MiB limit; ICE, DTLS verification, startup JSON and channel reliability stay as
before. The original Parsec WASM and video protocol version are unchanged.

Reports add channel_receive_api, channel_message_limit_bytes, three-element
channel_messages_received/channel_bytes_received/channel_max_message_bytes arrays
(control/video/audio), failure_channel, failure_elapsed_ms, last_send_elapsed_ms
and last_sent_control_kind. Times are monotonic milliseconds since this attempt
started, not wall-clock times. The last-send fields describe a successfully sent
control message; they do not prove receipt or causation. Only the fixed control
type byte is retained, never key values, coordinates, text or payload. Transport
counts precede the UI queue, so an unprocessed video receipt remains observable.

Regression coverage: real authenticated DTLS/SCTP fixture delivery of complete
65,535-, 65,536-, 131,072-, 524,288- and 1,048,576-byte video-channel messages,
through the actual WASM metrics import, with exact byte counts and connected
status. These are synthetic bytes, not valid encoded video. Existing control,
audio, input-after-31-seconds and shutdown probes remain. Unit tests cover size
and queue bounds, fixed failure categories, first-failure retention and local
shutdown. Real-host video receipt remains unverified until user testing.

Video/audio decoding and remote-video presentation are not implemented in this
build. The next acceptance target is nonzero channel 1/video ingress counters.

Reference: https://docs.rs/webrtc/0.14.0/webrtc/data_channel/struct.RTCDataChannel.html#method.detach