## M3m / 0.16.0

Native H.264 decoding uses Media Foundation with D3D11 NV12 texture output and
GPU video processing / DXGI presentation. No live decoded pixels are read back
to CPU memory. F8 toggles the video layer to expose account controls. Reports
distinguish decoder input, GPU output and presentation, with isolated failures.
Audio and absolute remote mouse mapping are not implemented yet. See
[M3m notes](../../docs/NATIVE_M3M.md).

## M3l / 0.15.0

Pinned video metadata and H.264 Annex B NAL headers are inspected at the receive
boundary without retaining payloads or occupying the control queue. Reports
separate announced key/delta chunks from observed IDR/SPS/PPS headers. This is
preparation for native hardware decoding, not decoded video. See
[M3l notes](../../docs/NATIVE_M3L.md).

## M3k-fix22 / 0.14.22

Complete real-host video reception was confirmed in the fix21 user report.
Unavailable media now bypasses the control queue, with bounded burst regression
coverage. See [fix22 notes](../../docs/NATIVE_M3K_FIX22.md).

## M3k-fix21 / 0.14.21

All native channels now use the documented detached receive API with a bounded
1 MiB message buffer, replacing the library callback's 65,535-byte buffer.
Encrypted fixture video-channel messages up to 128 KiB exercise the guest metrics
path. Reports retain per-channel receive counts/sizes and fixed channel failure
stages with attempt-relative failure/last-send times. See
[fix21 notes](../../docs/NATIVE_M3K_FIX21.md). Real-host video reception was subsequently confirmed; media decoding and presentation remain unavailable.

## M3k-fix19

Valid absolute mouse input without presented video is now reported as unavailable without terminating the WASM loop or sending guessed coordinates. Reports include the prototype version. See [fix19 notes](../../docs/NATIVE_M3K_FIX19.md). Video/audio decoding remains unavailable.

## M3k-fix18

Binary media channels now have explicit unavailable-decoder ingress counters instead of trapping the main WASM callback. Invalid control/channel traffic becomes a connection failure with a retained diagnostic. This build does not decode/play media. See [fix18 notes](../../docs/NATIVE_M3K_FIX18.md).

## M3k-fix17

The new main-thread failure is not yet diagnosed. Reports now retain a privacy-safe execution stage, runtime trap enum and bounded numeric WASM backtrace. See [diagnostic notes](../../docs/NATIVE_M3K_FIX17.md). This is a diagnostic build; native audio output is still unavailable.

## M3k-fix16

Optional screen wake lock now uses the documented Windows API on the window UI thread. Rejection does not terminate the guest; minimize and shutdown release it. See [fix16 notes](../../docs/NATIVE_M3K_FIX16.md). Audio output remains unavailable.

# Native Parsec WASM prototype

M3k-fix15 / 0.14.15 returns an explicit unavailable-output failure from the
previously missing audio factory, instead of trapping the guest worker.
Audio playback is not implemented. Fix14 user telemetry confirmed connected
ICE/DTLS, three open channels and incoming data before this audio boundary.
See [fix15](../../docs/NATIVE_M3K_FIX15.md) and
[the isolated SCTP-only correction](../../docs/NATIVE_M3K_FIX14.md).


M3k-fix12 / 0.14.12 expands opt-in diagnostic classification to all fixed DTLS
errors, exact peer alert descriptions and the generic ring crypto error. It also
reports only an exact known signature-verification algorithm name. A dedicated
offline DTLS failure probe verifies real library logs, certificate rejection and
redaction. No cryptographic acceptance or connection policy changes.
See [M3k-fix12](../../docs/NATIVE_M3K_FIX12.md).

M3k-fix11 / 0.14.11 captures fresh peer/ICE/DTLS failure states and fixed native
failure stages. Optional `account-network-audit` adds bounded, redacted,
experimental webrtc-rs diagnostic categories; default account mode omits them.
No network/cryptographic behavior changes. A wrong-fingerprint native fixture
checks rejection and failure capture. Real-host 6200 is not yet fixed.
See [M3k-fix11](../../docs/NATIVE_M3K_FIX11.md).

M3k-fix10 / 0.14.10 adds opt-in Cloudflare STUN to account modes via
`--cloudflare-stun` after the report path. It fixes missing active-attempt reports
and preserves four previous attempt diagnostics across retries, including native
ICE/DTLS states and SDP/sync phases. Tests remain offline and no TURN is added.
See [M3k-fix10](../../docs/NATIVE_M3K_FIX10.md).

M3k-fix9 / 0.14.9 adds an isolated Parsec remote-ufrag compatibility case:
six standard ICE characters followed by `==`, preserved byte-for-byte through
SDP and STUN authentication. This is explicitly outside RFC 8839's ICE grammar;
local credentials and password/fingerprint validation remain strict. A controlled
peer exercises the same shape with real native ICE/DTLS/SCTP. Real host acceptance
still needs a user retest. See [M3k-fix9](../../docs/NATIVE_M3K_FIX9.md).

M3k-fix8 / 0.14.8 adds aggregate credential character categories. The user's fix7
report narrows the rejection to an eight-byte remote ufrag with valid length,
no terminal CR and a matching attempt ID. No acceptance rules are changed and
the offending character is not yet established. See
[M3k-fix8](../../docs/NATIVE_M3K_FIX8.md).

M3k-fix7 / 0.14.7 normalizes the pinned JS compact SDP line-ending representation
at the ABI boundary, preserving strict ICE/DTLS validation. Redacted remote-begin
diagnostics distinguish credential fields and attempt-ID mismatches. The user's
specific `remote-begin` rejection still requires a real-account retest. See
[M3k-fix7](../../docs/NATIVE_M3K_FIX7.md).

M3k-fix6 / 0.14.6 contains remote signaling/ICE attempt errors instead of letting
them stop the guest worker and app. It retains bounded, redacted attempt failure
stages for the next real-account diagnostic. Host connectivity is still unverified.
See [M3k-fix6](../../docs/NATIVE_M3K_FIX6.md).

M3k-fix5 / 0.14.5 adds the exact `wss://kessel-ws-v2.parsec.app` signaling
origin observed in the user's opt-in diagnostic. This fixes that specific local
policy rejection; a real host connection remains to be verified. See
[M3k-fix5](../../docs/NATIVE_M3K_FIX5.md).

For a rejected signaling destination, `account-network-audit <parsecd.wasm>
[report.json]` explicitly adds destination origins (scheme, hostname and port)
to the account network report. It omits URL credentials, paths and query tokens;
normal `account` reports still omit origins. Both modes report the transport
bridge. This diagnostic does not expand network permissions or implement a
signaling fix. See [M3k-fix4](../../docs/NATIVE_M3K_FIX4.md).

This is an **independent native runtime prototype**. It retains the original
Parsec WASM binary and supplies host imports in Rust using Wasmtime. It does not
link Tauri, WebView2, a JavaScript engine, or a browser. The production app is
not changed and this directory is not part of its build or releases.

## Local desktop stage M3k / 0.14.0

The user confirmed login and the Computers host list in M3j. This stage adds
local UI keyboard press/release (including Tab, Shift+Tab, Enter, Escape and
Ctrl+C/V/X), modifier and focus-loss handling, and vertical/horizontal wheel
events. Unicode text still comes from the active Windows keyboard layout.
Only account mode enables the Windows text clipboard, default-browser HTTPS
links and native informational dialogs. Other modes remain isolated from the
real clipboard and browser. These services do not implement remote clipboard
forwarding or decoded host video.

`guest-platform-probe [report.json]` exercises the real WASM imports with a
synthetic desktop: Unicode copy/paste ownership, secure link validation,
informational dialogs, forward-only key aliases and disabled-service fallback.
The offline `login-audit` now reaches the password field with Tab and pastes
the fixed fixture password with Ctrl+V from a synthetic clipboard in nine
input stages. Read [M3k details](../../docs/NATIVE_M3K.md).

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
