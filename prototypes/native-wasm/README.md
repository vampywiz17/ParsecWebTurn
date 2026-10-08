# Native Rust / Parsec WASM prototype

This is an **independent native runtime prototype**. It retains the original
Parsec WASM binary and supplies host imports in Rust using Wasmtime. It does not
link Tauri, WebView2, a JavaScript engine, or a browser. The production app is
not changed and this directory is not part of its build or releases.

## Runtime milestone M0, 2026-10-08

On Windows, the unchanged Parsec core compiles, instantiates, allocates/frees
memory, creates native WASM threads and enters `main_entry_client_start`.
Its own startup log reports `Parsec release (150-104a, Service: -1, Loader: -1)`.
The M0 build stopped explicitly at **`env::parsec_web_init`**: this is where the original
weblib.js creates the JavaScript Parsec backend and its remote-video canvas.

This proves the standalone Rust host can run the real core's client startup
without WebView2. It does **not** prove a functioning UI or connection. The next
substantial components are the native implementation behind `parsec_web_*`
and the native graphics/window bridge. M1 below advances the idle backend;
CI checks that the original core actually initializes it.

## What this first milestone proves

- Compile the actual audited Parsec WASM in a native WASM runtime.
- Inspect every import's actual type/signature, rather than guessing from JavaScript.
- Instantiate the module with its imported shared memory and correctly typed host functions.
- Call the guest allocator, write/read through shared memory, then call the guest free function.
- Enter `_start` and report the first host operation that is not implemented.
- Provide a small WASI preview1 subset, OS randomness, guest hostname/platform and keyboard mapping.
- Start native OS threads with fresh WASM instances and the same shared memory,
  following the legacy WASI-threads ABI used by the binary.
- Fail explicitly at missing Parsec/WebRTC, audio or graphics bridges.

**This is not yet a functioning remote desktop client. It does not log in,
connect to a host, decode video, render a window, or play sound.** `boot` is a
bounded startup diagnostic, not a fake successful connection. JSON reports
explicitly record `network_enabled: false` and `video_rendered: false`.

## Build and run on Windows

Requirements: Rust 1.90+ and the normal MSVC C++ build tools/Windows SDK.
The independent GitHub workflow builds on `windows-latest`.

```powershell
./prototypes/native-wasm/fetch-core.ps1
cargo test --locked --manifest-path prototypes/native-wasm/Cargo.toml
cargo build --locked --release --manifest-path prototypes/native-wasm/Cargo.toml
$exe = './prototypes/native-wasm/target/release/parsec-native-wasm.exe'
$core = './prototypes/native-wasm/vendor/parsecd.wasm'
& $exe inspect $core imports.json
& $exe allocator $core allocator.json
& $exe boot $core bootstrap.json
```

An already downloaded audit copy may be used instead of fetching again.
Both paths require this SHA-256:

`d663dd96df477c65479fb93eb88756c7fcafff581cc93be563625cd195a4b4a6`

The core is loaded as external input; it is not compiled into the executable or
committed here. A different hash fails before execution. If the public web client
changes, fetching intentionally fails rather than silently accepting a new ABI.
Use the retained audit copy or audit and explicitly pin the replacement.

## Isolation and limits

- Only a synthetic `/` directory is preopened for WASI libc startup.
  Guest-created files stay in bounded memory, shared between the guest threads,
  and are discarded when the CLI exits.
  No host filesystem, environment variables, real clipboard or user profile access.
- No guest HTTP/WebSocket/WebRTC session or account credentials. The separate
  native WebRTC transport probe below uses only two peers on this machine.
- Unsupported functions are correctly typed traps, **not zero-returning success stubs**.
  The optional maintenance/USB hooks already empty in the audited web client are an
  explicit exception: they retain their zero/unavailable handle and are labeled
  `unavailable-as-in-web-client`, never as a working maintenance or USB service.
- Guest pointers/strings/iovecs are bounds checked. Host shared-memory access uses atomic bytes.
- Guest instruction fuel and a five-second epoch deadline limit bootstrap execution.
  A separate 15-second process deadline also handles blocking guest atomic waits;
  it exits with code 124 and does not claim a completed report.
- At most eight native WASM threads may be created during a bootstrap run.
  The legacy WASI-threads adapter is snapshot-specific, not a claim of support
  for every WASI threading proposal. No indefinite host-side waits are implemented.
- Captured stdout is limited to 64 KiB. Import tracing records names/counts;
  offline filesystem diagnostics additionally record at most 32 guest virtual
  path requests with their WASI error codes, never file contents or host paths.
- Missing bridges and WASM errors are represented in `start_error`/`host.boundary`
  and the per-thread `threads` records. A thread error interrupts other running
  guest loops; blocked atomic waits remain subject to the process deadline.

`boot` exits successfully when it produces a diagnostic report, even when `_start`
traps. Consumers must inspect the report; process exit code is not an app-readiness
indicator. An allocator failure, input mismatch, compilation or instantiation
error instead produces a nonzero exit code.

## Native backend and transport milestone M1

The next stage implements the idle `parsec_web_*` lifecycle and getter ABI in
Rust. A single backend is shared between the main WASM instance and its worker
instances, matching the original web client's global Parsec object. Initialization
is idempotent; destruction clears state; reinitialization starts a fresh generation.
Idle status is the audited `-3`, not a fabricated connected state. Video protocol
metadata is checked before storing it, and guest output buffers are bounds checked.
Empty audio/events/buffers and zero metrics represent the **idle backend only**.
Live-attempt imports continue to trap until their actual signaling is implemented.

The separate `transport-probe` command creates **two real native WebRTC peers on
the same machine**, using `webrtc-rs` 0.14.0 and Tokio. It creates the audited
negotiated, ordered binary channels: `control` (0), `video` (1), and `audio` (2).
It exchanges and checks synthetic binary bytes in both directions on every
channel, then explicitly closes both peer connections. The report records six
verified messages, peer connection states and cleanup. It never labels these
bytes as decoded video/audio or the test peer as a real Parsec host.

```powershell
& $exe transport-probe transport.json
```

No STUN/TURN servers, accounts, Parsec signaling, external host addresses, camera
or microphone are used. Ordinary IPv4 UDP host candidates from this machine's
interfaces are used; this can require local firewall permission. The probe does
not enable the library's loopback-candidate override, disable DTLS fingerprint
validation, or alter ICE candidate priorities. A machine without usable IPv4
interfaces or one whose firewall blocks this local traffic may fail the probe;
the test reports failure rather than replacing networking with a success stub.
Negotiation/exchange is limited to 20 seconds and cleanup to another five seconds.

The native transport proof is deliberately separate from the WASM idle backend:
it proves ICE/DTLS/SCTP and channel configuration between native peers, **not
compatibility with a real Parsec host**. The next integration must implement
Parsec's attempt signaling, candidate events and binary control protocol before
connecting this transport to the guest. Native H.264/Opus decoding and graphics
remain later stages. The boot report remains `network_enabled: false` and
`video_rendered: false`; the isolated transport report is a separate file.

The adapter currently presents `web.parsec.app` and `Win32` to the guest to match
the audited Windows web ABI; this does not establish an origin, permissions or
browser sandbox. The root starts empty; nonexistent files return WASI NOENT,
and guest-created files never map to real host paths. Unsupported filesystem
operations fail explicitly. This is
deliberately different from the browser shim's localStorage-backed virtual files.

## Next milestones

1. Complete the minimum platform imports needed to reach the Matoya event loop.
   Extend the isolated virtual filesystem only as required before attempting login.
2. Bridge the WASM UI's GLES/WebGL-style commands to a documented native graphics
   implementation, with a native Rust window and real keyboard/mouse events.
   The existing canvas UI and remote-video surface are separate.
3. Integrate the native data-channel transport with the guest's live-attempt
   bridge: signaling, explicit STUN/TURN configuration and control-message framing.
   The WASM module alone does not supply the JavaScript WebRTC implementation.
4. Decode incoming H.264 and Opus natively and present real frames/audio.
   Verify a first frame on a real host before claiming client compatibility.
5. Add full session lifecycle, cleanup, clipboard and controlled live diagnostics.

The public Wasmtime/Windows/WebRTC interfaces can be supported APIs. The
**Parsec-specific WASM import ABI remains private and version-dependent**. Keeping
the WASM core avoids rewriting all its logic, but cannot turn its import names or
wire protocol into a documented Parsec SDK contract. Keep that adapter isolated.

The Rust files in this directory are project code. The separately downloaded
Parsec binary retains its original ownership/licensing; the repository's MIT
license is not a license grant for that binary.

References:
- https://docs.wasmtime.dev/api/wasmtime/struct.SharedMemory.html
- https://docs.wasmtime.dev/api/wasmtime/struct.Linker.html
- https://github.com/WebAssembly/WASI/blob/main/legacy/preview1/docs.md
- https://github.com/WebAssembly/wasi-threads
- https://web.parsec.app/lib/matoya-worker.js
- https://web.parsec.app/lib/weblib.js
- https://web.parsec.app/lib/parsec.js
- https://github.com/webrtc-rs/webrtc/tree/v0.14.0
- https://www.w3.org/TR/webrtc/#rtcdatachannel
- https://www.rfc-editor.org/rfc/rfc8831
