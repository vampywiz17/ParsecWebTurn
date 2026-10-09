# M3m / 0.16.0 — native H.264 GPU output

The submitted M3l/0.15.0 report confirmed 814 Annex B video messages, SPS/PPS
and IDR headers, with no transport failure. This milestone connects those
encoded messages to a real native Windows decoder/presenter. No user report,
credentials or captured media is committed.

A dedicated worker owns COM, Media Foundation, the Microsoft H.264 transform,
a hardware D3D11 device, DXGI device manager, video processor and swap chain.
The documented D3D11-aware MFT receives the device manager. Output is accepted
only as IMFDXGIBuffer-backed NV12 D3D11 textures. Software pixel buffers are
rejected. The video processor reads decoder textures directly (including array
slices), scales/letterboxes and converts to the BGRA swap-chain backbuffer on
the GPU. Live sessions never map/read decoded pixels or upload CPU pixel images.
The compressed-input copy required by the MFT is separate from decoded pixels.
Low-latency decoding is requested via the documented ICodecAPI property using
the H.264 decoder's required VT_UI4 type. Acceptance is reported; unsupported
requests do not prevent decoding.

The child video HWND is created/resized/shown/hidden only by the existing UI
thread. It overlays the OpenGL account UI after a real successful DXGI present;
F8 toggles the video layer to expose the underlying controls. F11 remains the
parent fullscreen shortcut. The child cannot take keyboard/mouse focus. This
first video step does not implement absolute remote-pointer mapping or audio.

Compressed input is bounded to 32 messages / 8 MiB (headroom for initial device creation), each at most 1 MiB.
Initial delta pictures wait for an observed IDR; parameter sets may precede it.
If queue overflow would break the reference chain, this video attempt stops
with a fixed diagnostic rather than submitting corrupt deltas. Reconnect to
retry; automatic video recovery is not implemented. Transport/control remain
running, and decoder errors never become guest traps or connection failures.
Unknown framing does not enter the decoder. No payload is serialized.

Media type changes renegotiate NV12. Frame size/display aperture are validated,
non-fractional crop and aspect ratio are applied on the GPU. Reported MF matrix
and nominal-range metadata determine BT.601/709 and full/limited conversion;
missing metadata uses the documented prototype default BT.709 limited, with
source values left null. Other color matrices/ranges are explicitly unsupported.
Device-loss and unsupported driver/format errors are retained as fixed stage
plus HRESULT, with no silent WARP/CPU-output fallback.

Reports distinguish queued/submitted/decoded/presented counts, decoder creation,
actual GPU-surface output, dimensions, adapter, queue state and resource release.
A successful DXGI present is a submitted presentation, not proof of monitor scanout.
Hardware-decode utilization is unknown (null): D3D11-backed output alone is not
presented as a measured GPU decode-engine utilization or independent proof.

The standalone video-hardware-probe uses a plain Win32 window without a WGL
pixel-format requirement; the production account UI still requires accelerated
OpenGL. The probe uses eight locally generated synthetic
1920x1080 High-profile H.264 pictures (one IDR, seven delta pictures) and no network/account. It tests actual decode, NV12 texture
output, color conversion and presentation. Only this offline probe may read a
small set of synthetic backbuffer pixels once to verify variation; live sessions
have that switch disabled. Headless CI may report GPU unavailability, but must
release resources and provide a bounded failure stage. A local hardware result
is required separately before claiming verified presentation.

Lifecycle: cancellation wakes the worker, queued bytes are released, transform
flush/destruction precedes DXGI manager/device destruction and balanced MF/COM
shutdown. The window lifetime guard prevents HWND destruction while GPU objects
are active. Reconnection creates a fresh worker. Native driver hangs cannot be
safely forcibly terminated in-process; cleanup diagnostics retain that limitation.

References:
- https://learn.microsoft.com/en-us/windows/win32/medfound/codecapi-avlowlatencymode
- https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-decoder
- https://learn.microsoft.com/en-us/windows/win32/medfound/supporting-direct3d-11-video-decoding-in-media-foundation
- https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3d11videocontext-videoprocessorblt
- https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgifactory2-createswapchainforhwnd

Validation (2026-10-09): source aa5255bada5ebf7d1c0a5b5cfa6171c02aaa6ab6,
Windows CI https://github.com/vampywiz17/ParsecWebTurn/actions/runs/37974516265
passed formatting, 117 tests, strict Clippy, release build and all original
WASM / loopback transport / fingerprint-rejection / platform probes. The headless
runner reported ID3D11VideoDevice unavailable (0x80004002), with bounded failure,
finished worker and released window/resources; this is not GPU presentation proof.

The downloaded release executable was tested locally on AMD Radeon 780M Graphics:
eight 1920x1080 H.264 High-profile pictures (one IDR, seven delta) submitted,
decoded and presented; NV12 GPU-surface output and synthetic backbuffer pixel
variation verified, no dropped frames/failure, low-latency request accepted,
window and GPU resources released. The original offline login UI also completed
all nine synthetic steps with native accelerated UI frames, no network requests,
no guest failure, report privacy checks and wake-lock cleanup preserved. Local
proofs are included in the test package, not committed as user/network captures.

Real-host presentation, sustained frame pacing, resize/fullscreen during live
video, reconnect and Intel driver behavior still require the user's next test.
This milestone implements the first video path; it does not claim a fully
functional replacement client or measured low CPU/GPU utilization.
