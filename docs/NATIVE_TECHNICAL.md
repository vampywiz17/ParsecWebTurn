# Native client: technical guide

For downloads, everyday use and the roadmap, start with the
[project README](../README.md).

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
disk-backed session to migrate. Native STUN/TURN settings retain the
[Tauri configuration contract](NATIVE_CONFIG_COMPATIBILITY.md) in `settings.json`
beside the EXE (or `--data-dir`). The native session snapshot remains separate.

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
keyboard grabbing and further menu compatibility are future work. Native
STUN/TURN configuration is available from Connection settings (Ctrl+,), including
custom discovery, STUN-only, custom TURN and Cloudflare credential generation.
An unconfigured installation uses `stun:stun.parsec.gg:3478` and no TURN. Native
ICE keeps its automatic `all` policy; STUN-only omits relay servers. TURN supports
UDP, TCP and TLS client-to-server connections with UDP allocations toward the
host. IPv4 relay gathering follows upstream; secure STUN discovery and HTTP
proxy tunneling are unsupported. See the
[transport patch](../src-native/vendor/webrtc-ice-0.14.0/LOCAL-CHANGES.md).
Actual decoder hardware-engine execution is not claimed when unmeasured.

## Build and validate

Install the stable Rust MSVC toolchain, Visual Studio C++ build tools, Windows SDK
and CMake (for the bundled Opus reference decoder), then run from the repository root:

```powershell
./src-native/fetch-core.ps1
cargo test --locked --manifest-path src-native/Cargo.toml
cargo test --locked --manifest-path src-native/Cargo.toml --features diagnostics
cargo build --locked --release --manifest-path src-native/Cargo.toml
./scripts/package-native.ps1
```

`VERSION` and `src-native/Cargo.toml` must match. The normal build embeds the pinned
core and icon, uses a Windows GUI entry point, and strips symbols with thin LTO. The Windows C runtime is statically linked
using Rust's documented `crt-static` target feature; no separate Visual C++
runtime installer is needed.
`--version` prints the app/core versions. There is no update downloader or executable
replacement logic in the native client.

Tests, offline login fixtures, synthetic video, pixel readback and probe CLI modes
are compiled only in tests or with `--features diagnostics`. The diagnostic build
is never packaged as `ParsecWebTurn.exe`. CI checks the normal and diagnostic
builds separately. `scripts/smoke-native.ps1` exercises the actual normal executable
with a fresh isolated profile, closes it and starts it again; run it on a Windows
machine with supported graphics. It does not log in or access a real profile.
Detailed WASM call tracing, filesystem request traces, network audit records and
connection diagnostic history are also limited to tests and diagnostic builds.
Diagnostic-only direct dependencies are optional under `diagnostics` and listed
as dev-dependencies for ordinary unit tests. Wasmtime's text-format (`wat`)
parser is enabled only for those builds. Shared networking libraries may still
require some of these packages transitively in the normal client.
Unused-code and unused-direct-dependency warnings are enabled; both build
variants, including unit-test targets, must pass strict Clippy.
Historical implementation evidence is under `docs/NATIVE_*.md`.

## Native connection statistics

Connection stats (Ctrl+Shift+S) opens a separate, modeless Win32 window. A bounded
read-only task samples the active peer once per second while this window is visible.
Session generation IDs prevent late samples from replacing a newer connection.
No packet capture, administrator access, video readback or additional GPU texture
copy is needed. GDI double buffering applies only to the statistics window.

The selected ICE pair provides route evidence: either relay candidate confirms TURN;
local host/srflx/prflx with remote host/srflx confirms a direct ICE path. Remote prflx
alone remains unverified because a remote relay may be hidden. Missing data and stale
samples never prove a direct path. For local TURN, allocation provenance reports
the actual server URL and UDP/TCP/TLS transport without credentials.

Small read-only extensions in the existing vendored ICE library expose selected
candidate getters, allocation provenance, and RTT from transaction-matched,
authenticated STUN connectivity-check responses. They do not alter candidate
priority, nomination, transport policy, signaling or packet formats. The upstream
RTT placeholder zero is never displayed as a measurement. Loopback coturn fixtures
validate these fields over UDP, TCP and TLS with real encrypted SCTP traffic.

Traffic uses ICE transport byte deltas (not an available-bandwidth estimate); FPS
uses native decoder frame deltas. Video metadata comes from the existing H.264 SPS
inspector and Media Foundation/D3D11 snapshots. Audio format reflects the actual
native Opus pipeline, persists during silence, and distinguishes measured traffic
from the unknown host encoder bitrate. SCTP data channels do not expose RTP packet
loss; local queue drops are labeled separately. DTLS state is available, but its
version and cipher are not exposed by the current safe transport API.

CPU uses documented GetProcessTimes, normalized across logical processors. GPU uses
the documented Windows PDH GPU Engine counters for this process only, showing the
busiest engine and VideoDecode separately. Missing counters remain unknown. There
are no WebView2 child processes in this client.

References: [WebRTC statistics](https://www.w3.org/TR/webrtc-stats/),
[GetProcessTimes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocesstimes),
[PDH formatted counter arrays](https://learn.microsoft.com/en-us/windows/win32/api/pdh/nf-pdh-pdhgetformattedcounterarrayw).

ParsecWebTurn is an independent project. Parsec and its core belong to their
respective owners. Included Opus and WebRTC license notices must accompany binary
redistribution.
