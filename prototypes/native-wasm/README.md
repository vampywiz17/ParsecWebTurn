# Native Rust / Parsec WASM prototype

This is an **independent native runtime prototype**. It retains the original
Parsec WASM binary and supplies host imports in Rust using Wasmtime. It does not
link Tauri, WebView2, a JavaScript engine, or a browser. The production app is
not changed and this directory is not part of its build or releases.

## Original UI network audit stage M3i

`window-audit <parsecd.wasm> [report.json]` runs the original native UI offline
for the existing bounded eight-second event loop. Its HTTP calls now use the
common native host bridge, rather than a separate window-only failure stub.
`login-audit <parsecd.wasm> [report.json]` is a separate 20-second offline test:
it enters fixed fictitious credentials through native input and presses Log In
on the pinned UI layout. It accepts no real credentials or account arguments.
All guest instances share a bounded network metadata observer. Raw guest stdout
is no longer retained in any mode. This audit mode also omits titles, guest
filesystem paths, raw execution errors and screenshots.

`guest-audit-probe [report.json]` verifies actual HTTP/WebSocket/WASI imports,
shared observations from two guest instances, offline failure outputs and
redaction with synthetic secret sentinels. No account login/external network is
enabled. Read [M3i details](../../docs/NATIVE_M3I.md).

## Native HTTPS/WSS boundary stage M3h

`guest-tls-probe [report.json]` exercises the actual HTTP/WebSocket imports
over native TLS with synthetic credentials on local servers. It verifies
encrypted request/response and session-text exchange, rejection of untrusted
and name-mismatched certificates, offline/exact-origin policy and cleanup.
This does not enable real-account login or external traffic in original-core
modes. Read [M3h details](../../docs/NATIVE_M3H.md).

## Native WebSocket imports stage M3g

`guest-websocket-probe [report.json]` runs the five pinned Matoya WebSocket
imports through a controlled WASM fixture and local native server. It verifies
Unicode/empty messages, ping/pong, application keepalive, read timeout and
buffer retry, close/status handling, message bounds and handle destruction.
The normal original-core modes remain offline. This is signaling transport,
not authenticated original-guest signaling or a real Parsec host connection.
Read [M3g details](../../docs/NATIVE_M3G.md).

## Native HTTP import stage M3f

`guest-http-probe [report.json]` exercises the pinned `MTY_HttpRequest` import
against a local HTTP test server. The controlled WASM fixture verifies binary
and empty responses, status codes, response ownership/cleanup, UTF-8 request
bodies, headers containing colons, timeout/bounds failures and redirect refusal.
The normal original-core modes remain offline. External authentication and
WebSocket signaling are not enabled yet. Read [M3f details](../../docs/NATIVE_M3F.md).

## Native window / graphics stage M2

`window <parsecd.wasm> [report.json] [optional-capture.png]` adds a bounded native
Win32 event loop and an OpenGL GPU adapter for Matoya's original UI imports.
It requires a driver advertising the needed WGL extensions and an accelerated
OpenGL 4.1 compatibility context; there is no generic/software fallback.
The guest remains offline, without a Parsec host session or decoded remote video.
Read [M2 details and test instructions](../../docs/NATIVE_M2.md).
The M0/M1 sections below describe the earlier headless milestones; `boot` retains
its original boundary diagnostic behavior.

## Compact signaling stage M3a

`signaling-probe [report.json]` validates the client-side compact Parsec
ICE/DTLS/candidate mapping against two real native peers. The client reconstructs
a standard SDP answer from compact fields, releases bounded candidates after
description installation and sync, and verifies all three binary channels.
No credentials, real host session or guest live-attempt bridge is enabled yet.
Read [M3a scope and acceptance criteria](../../docs/NATIVE_M3.md).

## WASM offer bridge stage M3b

`guest-offer-probe [report.json]` executes the real native offer import from a
controlled WASM fixture, verifies the shared-memory completion handshake and
actual native credentials, then cancels and checks peer cleanup. The worker is
asynchronous, bounded and isolated from backend locks. The original guest's
remote begin/candidate imports are added in M3c below; login remains later work. This is not a live
Parsec host session. Read [M3b details](../../docs/NATIVE_M3B.md).

## Native attempt signaling stage M3c

`guest-session-probe [report.json]` exercises `new_attempt`, `begin_p2p`,
`add_candidate` and `poll_events` through a controlled WASM guest. It connects
the retained native peer to a local test peer using only compact credentials
and guest candidate events. It verifies three ICE/DTLS/SCTP channels, six
binary messages, the candidate sync acknowledgment, failed-buffer-copy retry
and cancellation/cleanup. No browser or JavaScript participates.

This is a bounded transport diagnostic, not a working Parsec host session:
the original guest UI attempt, account signaling, Parsec control framing,
video decoding and audio playback are not exercised. Read
[M3c details](../../docs/NATIVE_M3C.md).

## Control framing stage M3d

`guest-control-probe [report.json]` uses the controlled WASM fixture and a native
test peer to verify the pinned Parsec control header, startup configuration,
WASM key input, status events, guest/self metadata, host mode, encode latency,
rumble and clipboard-request events. No JavaScript or browser is used.
Metadata configured before an attempt enables this limited control mode;
the raw M3c transport diagnostic remains unchanged. This does not verify real
host compatibility, video, audio or a functioning clipboard.
Read [M3d scope and limitations](../../docs/NATIVE_M3D.md).

## Guest buffers stage M3e

`guest-buffer-probe [report.json]` extends the controlled native connection
with cursor metadata/image bytes, binary and empty user data, one-shot guest
buffer copies, retry after invalid destinations and disconnect cleanup.
It also verifies outbound UTF-8 user data through the actual WASM import.
Opaque handles are bounded and never recycled during backend reinitialization.
Payloads retain the incoming frame through `Bytes` slices, avoiding another
Rust-side payload copy. No real cursor is displayed and clipboard integration,
account signaling and media decoding remain separate work.
Read [M3e scope and verification](../../docs/NATIVE_M3E.md).

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
connect to a host, decode video or play sound.** `boot` is a
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

The original core now reaches `MTY_AppRun`. It stops explicitly at the native
event-loop bridge (`web_run_and_yield`) or the concurrently started graphics
bridge (`web_set_gfx`). Which is recorded first can depend on thread scheduling;
CI checks both the initialized backend and these next boundaries. The libc
`poll_oneoff` clock subscription used by its signal thread is implemented with
the WASI preview1 layout: one realtime/monotonic clock, relative or absolute
deadline, maximum one second of host waiting. Other polling requests return
INVAL/NOSYS; they do not invent file/network readiness. All guest instances
share the same monotonic clock epoch.

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

M3j / 0.13.0 adds `account <parsecd.wasm> [report.json]`: an original-core login
window that runs until close, with exact HTTPS/WSS destinations and an ephemeral
guest filesystem. The original core owns authentication; live account acceptance
and remote video remain unverified. `session-audit` tests the persistent lifecycle
offline for 35 seconds. See [M3j](../../docs/NATIVE_M3J.md) for network scope,
privacy, shutdown behavior and testing. No credentials belong in CLI arguments
or reports. All other diagnostic modes remain offline.

1. Extend the M2 platform/input bridge as required by an actual session.
   The Matoya UI and remote-video surface remain separate components.
2. Integrate the native data-channel transport with the guest's live-attempt
   bridge: signaling, explicit STUN/TURN configuration and control-message framing.
   The WASM module alone does not supply the JavaScript WebRTC implementation.
3. Decode incoming H.264 and Opus natively and present real frames/audio.
   Verify a first frame on a real host before claiming client compatibility.
4. Add full session lifecycle, cleanup, clipboard and controlled live diagnostics.

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
- https://github.com/WebAssembly/WASI/tree/wasi-0.1
- https://github.com/WebAssembly/wasi-libc/blob/main/libc-bottom-half/headers/public/wasi/wasip1.h
- https://github.com/WebAssembly/wasi-threads
- https://web.parsec.app/lib/matoya-worker.js
- https://web.parsec.app/lib/weblib.js
- https://web.parsec.app/lib/parsec.js
- https://github.com/webrtc-rs/webrtc/tree/v0.14.0
- https://www.w3.org/TR/webrtc/#rtcdatachannel
- https://www.rfc-editor.org/rfc/rfc8831
