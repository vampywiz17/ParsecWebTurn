# M3n: native audio, control and Parsec overlay

This isolated prototype continues the working M3m-fix1 GPU video path. It does
not change the main/dev Tauri application or the pinned, unmodified Parsec core.

## Implementation

- The pinned `parsec.js` audio channel consists of raw Opus packets configured
  as 48 kHz stereo. A dedicated reference-libopus decoder worker supplies bounded
  interleaved PCM16 packets to `parsec_web_poll_audio`. This is the existing
  private Parsec ABI, not an RTP audio stream or a newly invented packet format.
- The original WASM retains its volume/mute logic and calls the original
  `MTY_AudioCreate/Queue/Reset/GetQueued/Destroy` imports. Native output uses
  documented shared-mode, event-driven WASAPI. Windows performs device-format
  conversion when required. Factory failure returns a null context and remains
  nonfatal. `Create` latency arguments are milliseconds; `GetQueued` returns
  audio frames, matching the pinned AudioWorklet adapter.
- Compressed audio and decoded PCM queues are bounded. A stalled consumer drops
  old decoded audio instead of accumulating unlimited latency. Cancellation
  stops the decoder; output handles validate ownership and release COM objects
  on their audio worker. Reports contain counters/categories, never PCM or
  clipboard/account contents. Device loss is reported, not claimed as playback.
- Absolute mouse input uses the same source dimensions and letterbox rectangle
  as native video presentation. It remains unavailable before an actual video
  presentation. The guest metrics import now receives the decoded image size.
- Relative mouse is **controlled by the original Parsec overlay option** through
  `web_set_pointer_lock`. It is not enabled automatically by the native layer.
  Documented foreground Raw Input supplies deltas; focus loss, F8, and shutdown
  release cursor confinement. The guest receives the resulting mode state.
  Keyboard/buttons/wheel continue through the original WASM's UI/input routing;
  input is not sent behind the overlay's back. Mouse capture preserves drags
  outside the client area and releases held buttons on focus/capture loss.
- The original Matoya GUI's CPU-generated vertices, indices, projection, atlas
  uploads and clipping rectangles are mirrored through the pinned GLES adapter.
  A D3D11 GUI pass draws this data over the video backbuffer with alpha blending.
  Original GL rendering remains available for account/login screens and F8.
  Unsupported GUI commands disable composition and produce a bounded diagnostic;
  they do not trap the guest or stop the decoder.
- The H.264 decoder still supplies D3D11 NV12 textures directly to the existing
  VideoProcessor conversion/presentation pass. GUI uploads contain **only UI
  geometry and atlases**. No decoded video readback, intermediate decoded-video
  texture copy or browser is added. The necessary conversion/output write and
  GUI blending are not described as zero-copy. Decode-engine hardware execution
  remains unknown when not measured.

## Scope and remaining validation

Current audio integration implements the format used by the pinned browser
adapter (Opus, 48 kHz stereo). Additional menu-selectable audio formats, device
selection, keyboard grabbing, physical gamepad discovery and other Parsec menu
options require a subsequent compatibility audit after the live test succeeds.
This milestone does not claim those options are implemented.

Synthetic checks cover reference Opus decoding, PCM boundary/copy behavior,
control framing, letterbox input mapping and GUI command validation. A local
GPU probe verifies both decoded-picture variation and the composited overlay
pixels. Readback exists only inside that explicit synthetic probe. A silent
WASAPI probe verifies device initialization, PCM writes/reset and cleanup without
playing an audible tone. CI runners may have no supported GPU/audio endpoint;
absence is reported and does not count as hardware validation.

Live host audio, original overlay interaction and remote keyboard/mouse operation
must still be confirmed with the new package. No real account report is committed.

## Documented APIs and upstream references

- [Opus reference decoder API](https://opus-codec.org/docs/opus_api-1.6.pdf)
- [WASAPI stream initialization](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient-initialize)
- [WASAPI rendering](https://learn.microsoft.com/en-us/windows/win32/coreaudio/rendering-a-stream)
- [Windows Raw Input](https://learn.microsoft.com/en-us/windows/win32/inputdev/raw-input)
- [GLES 2.0 texture sampling semantics, section 3.8.2](https://registry.khronos.org/OpenGL/specs/es/2.0/es_full_spec_2.0.pdf)
- [D3D11 alpha blending](https://learn.microsoft.com/en-us/windows/win32/direct3d11/d3d10-graphics-programming-guide-blend-state)
- Pinned `parsec.js`, `weblib.js`, `matoya-worker.js` and WASM GUI shader source,
  downloaded in the isolated audit directory. These define the private ABI;
  they are not presented as a public Parsec SDK or an Internet standard.

The executable statically links the Opus reference library through `libopus_sys`.
The test package must include both wrapper and libopus license notices.
