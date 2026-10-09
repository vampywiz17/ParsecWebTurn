# ParsecWebTurn — native Rust client

The `dev` branch now runs the pinned Parsec WebAssembly core in a native Rust
application. It does not require Edge, WebView2, Tauri, a browser extension,
Node.js or a separate WASM file at runtime. Start **ParsecWebTurn.exe** directly.
The stable `main` branch remains the previous Tauri client until a later release.

## What works

- Original Parsec account interface, Computers list and stream overlay.
- Native WebRTC data-channel transport, with certificate fingerprints and
  signatures checked. No forced Cloudflare STUN/TURN configuration.
- H.264 video through Media Foundation, D3D11 NV12 GPU surfaces, GPU colour
  conversion and DXGI presentation. No live decoded-video CPU readback.
- Opus audio, 48 kHz stereo, through shared-mode Windows WASAPI.
- Keyboard, mouse buttons, wheel and absolute video-coordinate input. Relative
  mouse remains controlled by the original Parsec overlay option; focus loss
  releases cursor confinement.
- Persistent native account/session and Parsec preferences. The guest filesystem
  remains virtual; its bounded snapshot is encrypted with Windows DPAPI for the
  current Windows user. No guest-controlled path maps to an arbitrary host file.

The core is pinned to Parsec **150-104a**, SHA-256
`d663dd96df477c65479fb93eb88756c7fcafff581cc93be563625cd195a4b4a6`.
It is unmodified. Its private ABI is an explicit compatibility boundary, not a
public Parsec SDK. A changed upstream core requires another ABI audit.

## Run and saved data

Windows x64 with working D3D11 and OpenGL support is currently required.
Unzip the development package and run `ParsecWebTurn.exe`. Only that executable
is needed to run the app; the other package files are documentation/licenses.
No administrator rights or installation are required.

Native data lives in `%LOCALAPPDATA%\ParsecWebTurn\Native\profile.dpapi`.
It survives restarts and replacing/moving the executable. One instance owns the
profile at a time. Corrupt or inaccessible profiles produce a failure; the app
never silently resets them. Back up the encrypted file while the app is closed.
DPAPI protection is tied to the Windows user; it is not a cross-user portable
profile or protection against programs already running as that same user.

Existing Tauri `settings.json`, credentials and WebView2 profiles are preserved.
Browser cookies cannot be imported into this native session through a supported
API, so the first native launch requires signing in again. The prototype had no
disk-backed session to migrate. Future native STUN/TURN settings will retain the
[Tauri configuration contract](docs/NATIVE_CONFIG_COMPATIBILITY.md); the native
session snapshot does not replace or rewrite that file.

Legacy **RSA-1024 host certificate compatibility is enabled** for the observed
Parsec host handshake. This accepts weaker host identity keys; it does not disable
certificate-fingerprint or signature verification, downgrade HTTPS, or select
1024-bit stream encryption. Network encryption remains the negotiated DTLS
transport. A user-facing compatibility setting is future work.

F11 toggles fullscreen. F8 hides/shows the native video layer to reach the original
interface if needed. The original overlay controls relative mouse and audio volume.

## Current limits

This is the first native development build, not a claim that every original menu
option is supported. Additional audio formats, physical gamepad discovery,
keyboard grabbing, menu compatibility and custom STUN/TURN configuration are the
next development work. Previously saved Tauri relay settings are not applied yet.
The app currently uses the pinned core's ordinary STUN settings and native ICE
behavior. Installing a TURN server or selecting relay mode is not part of this
build. Actual decoder hardware-engine execution is not claimed when unmeasured.

## Build and validate

Install the stable Rust MSVC toolchain, Visual Studio C++ build tools, Windows SDK
and CMake (for the bundled Opus reference decoder), then run:

```powershell
./src-native/fetch-core.ps1
cargo test --locked --manifest-path src-native/Cargo.toml --features diagnostics
cargo build --locked --release --manifest-path src-native/Cargo.toml
./scripts/package-native.ps1
```

`VERSION` and `src-native/Cargo.toml` must match. The normal build embeds the pinned
core and icon, uses a Windows GUI entry point, and strips symbols with thin LTO.
`--version` prints the app/core versions. There is no update downloader or executable
replacement logic in the native client.

Tests, offline login fixtures, synthetic video, pixel readback and probe CLI modes
are compiled only in tests or with `--features diagnostics`. The diagnostic build
is never packaged as `ParsecWebTurn.exe`. CI checks the normal and diagnostic
builds separately. `scripts/smoke-native.ps1` exercises the actual normal executable
with a fresh isolated profile, closes it and starts it again; run it on a Windows
machine with supported graphics. It does not log in or access a real profile.
Historical implementation evidence is under `docs/NATIVE_*.md`.

ParsecWebTurn is an independent project. Parsec and its core belong to their
respective owners. Included Opus and WebRTC license notices must accompany binary
redistribution.
