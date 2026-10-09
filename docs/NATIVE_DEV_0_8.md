# Native dev 0.8.0 promotion evidence

Build source: `cc5d7b9b0eff96c63a070dc629f5db7a375065bd` on `dev`.
[Windows validation run](https://github.com/vampywiz17/ParsecWebTurn/actions/runs/37991691938).
The complete Windows run passed: 128 tests, formatting, strict Clippy for the
normal and diagnostic targets, release packaging and all offline regression probes.
The Parsec core remains the audited, unmodified 150-104a binary pinned in the
[root README](../README.md).

## Distributed executable

- Normal GUI EXE: 20,205,568 bytes; ZIP: 7,226,849 bytes.
- EXE SHA-256: `5da48ba22831bdcab36294479fc26ab8bb77b220f47c4c85f720900a282faffe`.
- ZIP SHA-256: `a2d24a81cf051cccaa62142aaf54ac8100314192a2c92d8d3785cdd34287d748`.
- The ZIP contains one executable and eight documentation/license files, with
  no WASM, DLL, JSON report, encoded test stream or launch script.
- Binary inspection confirmed the entire pinned WASM is embedded; synthetic
  video and diagnostic command/fixture markers are absent. PE import inspection
  found Windows system DLLs only, with no VCRuntime, Opus or WebView2 DLL.
- The forced Cloudflare diagnostic STUN endpoint is absent from the normal
  binary. The production default matches the pinned public `parsec.js`:
  `stun:stun.parsec.gg:3478`, with ordinary ICE `all` policy and no TURN server.
- The actual release EXE was copied to a directory containing only that EXE,
  launched with a fresh isolated profile, closed normally and started again.
  Both launches saved a nonempty encrypted profile and exited with status 0.
  No real account or existing profile was used by this test.

## Persistent data and compatibility

The bounded virtual guest filesystem is encrypted using current-user Windows
DPAPI and published through same-directory atomic replacement. Exclusive profile
ownership prevents concurrent instances writing it. Tests cover restart/load,
unlink with an open descriptor, omission of orphaned file data, corrupt ciphertext,
unsupported schema and path traversal rejection; invalid stored data is preserved.

The native session snapshot is distinct from the old Tauri server settings.
Existing `settings.json`, encrypted secrets and browser profiles are untouched.
The [STUN/TURN compatibility contract](NATIVE_CONFIG_COMPATIBILITY.md) specifies
future use of the existing JSON properties and Base64/CurrentUser DPAPI secret
format. Reading/applying those settings is subsequent development work.

## Separate local media and interface diagnostics

The optimized diagnostic executable from `b1f2c8a` was tested on AMD Radeon 780M.
Video, audio, graphics and desktop-loop source files are identical between that
build and `cc5d7b9`; the intervening runtime change adds the production STUN default.
These probes and their fixtures are excluded from the distributed EXE.

- Synthetic H.264: 1,024 1920Ă—1080 frames decoded to GPU surfaces; 1,012 presented,
  12 busy presentation opportunities, zero encoded-frame drops and no decoder or
  presentation failure. All 1,024 synthetic overlay draw calls were composited.
  GPU resources/window were released and the worker finished. Diagnostic-only
  pixel variation was verified. Live CPU readback remains disabled; the actual
  decoder hardware engine is unreported, so hardware execution is not claimed.
- Silent WASAPI: 9,600 PCM frames queued and written, zero drops/underruns, no
  HRESULT failure and resources released. This verifies output plumbing, not
  audible remote audio quality.
- Original WASM login interface: nine synthetic input steps, 15 rendered frames,
  no guest execution error and no external network/account access.

The previous M3n real-session result was accepted by the user as the promotion
baseline. This change does not add a new real-host input/audio certification.
Additional audio formats, menu compatibility, gamepad/keyboard grabbing and
custom STUN/TURN settings remain follow-up work on `dev`.
