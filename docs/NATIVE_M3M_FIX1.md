# M3m-fix1 / 0.16.1 — preserve decode throughput under display pressure

The user's M3m report confirmed 434 H.264 pictures decoded and submitted for
presentation at 1920x1080, with GPU-backed NV12 output on AMD Radeon 780M.
The video worker then stopped with video-input-queue-full. ICE/DTLS/SCTP remained
connected and continued receiving video. No decoder HRESULT or guest trap was
reported. The visible image disappeared when the failed worker hid its layer.

Confirmed failure: the bounded encoded queue overflowed. The report does not
establish which processing operation was slow. The old Present(1,0) synchronously
paced the decoder worker against the display; it could create backpressure,
especially with the separate OpenGL UI also presenting. This fix removes that
avoidable dependency using documented Present(0, DXGI_PRESENT_DO_NOT_WAIT).
DXGI_ERROR_WAS_STILL_DRAWING skips only a display submission. Decoding continues
in order, preserving delta-picture reference dependencies. Positive non-visible
statuses also remain nonfatal; genuine device/driver errors remain failures.
There is no retry/spin loop, forced tearing flag or unbounded input buffering.
The 32-message / 8 MiB compressed queue and fail-closed overload behavior remain.
This does not guarantee recovery from hardware that cannot sustain decode rate.

Reports add busy/non-visible presentation counters, queue peak and maximum
Present/decode-feed durations. Decode-feed time includes output/presentation;
it is not an isolated hardware decoder benchmark. Present counts still mean
accepted submission, not measured monitor scanout. No live pixels are read back.

An offline --sustained GPU probe repeatedly supplies the synthetic eight-picture
1080p High-profile sequence (IDR plus seven deltas), 1,024 pictures at a target
120 pictures/second. It must decode all pictures without encoded drops or queue
failure, while allowing busy display submissions to be skipped. Pixel variation,
resource cleanup and first-failure diagnostics remain checked. Headless CI may
report unavailable GPU video interfaces; actual GPU output needs a local test.
A unit test distinguishes busy/occluded submission from device removal.

Reference:
https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/dxgi-present

Live-host sustained playback with this fix still requires user validation.

Validation (2026-10-09): source fb5be129a790ea6c01e6dac921956d9ed231153a,
Windows CI https://github.com/vampywiz17/ParsecWebTurn/actions/runs/37976936501
completed successfully: formatting, 118 tests, strict Clippy, release build and
all original WASM / loopback transport / security / platform probes. Both GPU
probes retained bounded unavailability/cleanup diagnostics on the headless
runner (ID3D11VideoDevice unavailable); they do not substitute for local GPU proof.

The downloaded release executable passed the original offline login/privacy/
wake-lock cleanup test (nine synthetic steps) and short 1080p GPU output test.
On AMD Radeon 780M, the sustained probe submitted/decoded all 1,024 pictures,
accepted 1,015 presentations and skipped nine busy submissions, with zero encoded
picture drops, no failure, queue peak 14/32, verified synthetic pixel variation
and released resources/window. Maximum Present time was 429 us; maximum decode
feed time (including output/presentation) was 19,535 us. These are one synthetic
local run, not real-host sustained-playback or decode-engine utilization claims.
No new decoded-frame copy or live readback was introduced by this fix.
The GPU-copy requirement, including permission for necessary implementation
copies, is recorded in AGENTS.md for future work.

Real-host validation (user report, 2026-10-09): the user confirmed visible video
with no stall on 0.16.1. The final native output snapshot reported 4,532 pictures
submitted/decoded, 4,413 accepted presentations, 119 busy display submissions,
1920x1080 GPU-backed NV12 output, queue peak 9/32 and no video failure/HRESULT.
The three queued pictures remaining at shutdown were cleared (4,535 queued minus
4,532 submitted); the earlier live ingress snapshot had zero picture drops.
The worker finished, GPU resources were released and the native window closed
without a guest execution error. This validates sustained playback for this
observed session on AMD Radeon 780M, not all drivers, displays or future runs.
The raw report and media are not committed. Hardware-decode engine utilization
remains unknown; busy display skips are not network packet-loss measurements.
