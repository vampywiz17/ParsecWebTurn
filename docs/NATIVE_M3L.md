# M3l / 0.15.0 — encoded video framing inspection

Review since 2d4c5bf: four clean committed changes were present on the isolated
native branch. Fix21 replaced the 65,535-byte callback reader with the documented
bounded detached receive API, retained first channel failures, and separated
large-receive tests from an upstream synthetic sender queue limit. Fix22 moved
unavailable media accounting before the shared control queue. The existing
fix22 package records Windows CI 37936743804 with 109 passing tests and the
real encrypted large-message, unpolled burst and 31-second session proofs.
These changes are retained. No main/dev or vendor source modification is added.
The fix22 notes record actual fix21 host video receipts, including an 88,841-byte
message; receipt is no longer merely hypothetical. Sustained fix22 host behavior
still requires testing.

This milestone separates pinned Parsec metadata messages from encoded video
messages using the guest-provided version, size and little-endian field offsets,
matching the pinned parsec.js. It tracks announced key/delta chunks separately
from H.264 Annex B IDR headers. A bounded allocation-free scan observes start
codes, SPS profile/constraint/level bytes, PPS and IDR headers. Unknown or malformed
syntax is reported as unrecognized and never breaks transport. The scan is capped
at 256 NAL units per message; the transport retains its existing 1 MiB bound.
No raw video, parameter-set payloads or image contents are kept or serialized.

New data is under native_backend.media_ingress.video_stream. These are framing
observations, not full H.264 bitstream validation: parameter_sets_and_idr_observed
only records that those NAL headers have been seen for the current protocol.
It does not prove a coherent/decodable picture, decoder initialization, hardware
support or dimensions. Profile/level describe the latest observed SPS header.
No decoder readiness, decoded FPS, resolution or actual keyframe is invented.
Repeated protocol configuration preserves state; changed protocol configuration
resets framing observations and increments protocol_changes. Configuration is
also forwarded to an already active native attempt.

A review also found that a startup send could finish after a receive failure
and still publish a successful control-ready event. The startup callback now
checks the failure flag before publishing success, preserving the first failure.

Tests cover pinned metadata, one-shot key flags, headerless upstream behavior,
mixed start codes, truncated/unknown input, excessive NAL counts, invalid offsets,
configuration changes and payload-free snapshots. Encrypted guest-control and
buffer probes additionally send metadata, SPS/PPS/IDR headers and a delta NAL
through the real receive path and verify observations through guest metrics.
Synthetic header fixtures are intentionally not claimed decodable video.

Next renderer step: documented Media Foundation/D3D11 decoding with actual
device-backed output, then GPU presentation. Video/audio decoding and remote-video
presentation are not implemented in this milestone. The existing UI renderer
continues to draw only the account UI.

References:
- Pinned original adapter: audited parsec.js, video-channel callback (not a web standard).
- W3C AVC registration: https://www.w3.org/TR/webcodecs-avc-codec-registration/
- Microsoft H.264 decoder Annex B input: https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-decoder
