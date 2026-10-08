# Native WASM prototype: window and GPU bridge (M2)

This stage belongs to the isolated `codex/native-wasm-prototype` branch. It runs
the pinned, unchanged Parsec WASM using Rust/Wasmtime and a native Win32 window.
No Tauri, WebView2, browser JavaScript or Chromium GPU process is involved.

The original Matoya imports issue GLES/WebGL-style commands. The adapter maps
their handles and checked guest buffers onto an owned native OpenGL context.
It uses documented Win32/WGL APIs and OpenGL 4.1 compatibility, including ES2
shader compatibility for the original `#version 100` sources. Shader sources
are not rewritten. The Parsec import ABI itself remains snapshot-specific.

## Run

In the extracted test package, from PowerShell:

```powershell
./parsec-native-wasm.exe window ./parsecd.wasm ./window-local.json
```

The event-loop test lasts at most eight seconds after startup, or ends when the
window is closed. A separate 15-second process deadline bounds blocked guest
execution. The prototype has no account/session/profile persistence and does
not connect to a Parsec host. Guest HTTP requests explicitly fail offline.

Optional one-time rendered-backbuffer capture for testing:

```powershell
./parsec-native-wasm.exe window ./parsecd.wasm ./window-local.json ./window-local.png
```

This captures the eleventh presentation only. It is not a per-frame readback
path; the normal rendering path uses native GPU buffers/textures and SwapBuffers.
No image is produced if execution stops before that frame.

## Evidence and limits

- `graphics.vendor`, `renderer`, and `version` come from the actual driver.
- Both the pixel format and WGL acceleration query must confirm driver
  acceleration. Missing OpenGL 4.1 support or unverifiable acceleration fails;
  there is no implicit generic/software renderer fallback.
- `graphics.shaders_compiled` and `draw_calls` count successful native shader
  compilation and issued draw commands. `frames_presented` counts successful
  SwapBuffers calls, not decoded video frames or displayed remote-stream FPS.
- Native pointer/button, size/focus and basic text events reach WASM exports.
  Full keyboard state, relative mouse, controllers, fullscreen, clipboard and
  DPI transitions are not yet a complete application input layer.
- Idle input messages are parsed and discarded, matching the audited
  `clientSendMessage` condition requiring a connected transport. The diagnostic
  counter never implies delivery to a host. Live message transport still fails
  explicitly until it is implemented.
- GPU handles, uploads, image sizes and input queues are bounded. Shared guest
  memory is accessed atomically; graphics contexts stay on their creating thread.
  The HWND is retained while a context is active.
- Guest callback handoff is intentional; `_start` does not return normally.
  Unsupported imports, shader errors and other failures are reported rather
  than hidden. Inspect `start_error`, `host.boundary`, and worker errors.
  Cancellation interrupts during bounded test shutdown are distinct from a
  bridge failure during execution; blocked guest threads are not a complete
  long-lived session shutdown implementation.

**This is not a usable remote desktop client.** `network_enabled` and
`video_rendered` remain false. A dark idle surface is not proof of decoded video
or a complete login/host selection UI. Graphics availability and actual calls
must be checked in the JSON report.

This OpenGL adapter concerns Matoya's UI graphics. It does not implement or
select the planned Windows Media Foundation/D3D11 video decoder and renderer.
Those remain separate components under [the rendering design](NATIVE_PROTOTYPE_DESIGN.md).
No CPU/memory improvement or 1080p/60 playback performance is claimed here.

References:

- [Win32 pixel format descriptor](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-pixelformatdescriptor)
- [Khronos WGL context creation](https://wikis.khronos.org/opengl/Creating_an_OpenGL_Context)
- [WGL_ARB_pixel_format](https://registry.khronos.org/OpenGL/extensions/ARB/WGL_ARB_pixel_format.txt)
- [GL_ARB_ES2_compatibility](https://registry.khronos.org/OpenGL/extensions/ARB/ARB_ES2_compatibility.txt)
- Audited original `matoya-worker.js` and `parsec.js`, pinned with the unchanged
  WASM SHA-256 `d663dd96df477c65479fb93eb88756c7fcafff581cc93be563625cd195a4b4a6`.
