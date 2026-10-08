# M3j / 0.13.1: release completed worker capacity

The user's first real-account test logged in and immediately closed the native
window. Its redacted report contains eight guest thread records (IDs 2–9).
The main instance recorded five spawn calls and the rendering worker four:
nine calls reached the old eight-record lifetime limit.
Two HTTP workers finished normally, the rendering worker called WASI proc_exit,
and no unimplemented-import boundary was reported. Authentication requests and
an authorized API GET were permitted. This is consistent with the old lifetime
limit of eight guest workers being exhausted by short-lived account requests.
The old report contains neither spawn-rejection counts nor the guest's numeric
exit code, so it cannot conclusively attribute that proc_exit to this limit.

The thread runtime now caps **concurrently active** workers at 16, leaving room
for the native HTTP service's eight simultaneous requests plus the core's
long-lived workers. Completed workers release capacity. The last 64 records are
retained; running records are never evicted. IDs remain unique, monotonic and
within the legacy WASI-thread range [1, 2^29), with main reserved as 1.
Shutdown-on-trap/proc_exit remains; errors are not suppressed to keep a broken
guest running. One engine watchdog is shared by bounded diagnostic instances.

Reports add `thread_runtime` counters for active/peak workers, completed workers,
rejected spawns and omitted historical records. `threads[].exit_code` and
`host.guest_exit_code` retain only the numeric WASI exit code; the account report
still omits raw guest strings, paths, titles, request bodies and credentials.

The isolated `guest-thread-probe` uses a controlled WASM fixture, not a real
account. It verifies 80 successive workers through the actual guest spawn
import, 16 concurrent workers waiting on standard shared-memory atomics,
rejection of the 17th, capacity recovery after completion, unique shared-memory
TIDs, 64-record retention, and a synthetic numeric proc_exit code. In total 97
workers complete, one spawn is rejected and 33 history entries are omitted.
A separate test checks exhaustion of the legacy TID range without wraparound.

The fixed original-core build still needs the user's login retest. No automated
test uses real credentials or claims post-login host/stream compatibility.
The original pinned core is unchanged. Main/dev remain separate from this
prototype branch.

Reference: https://github.com/WebAssembly/wasi-threads (legacy preview1 proposal;
retained for the pinned core, not claimed to be a current WASI v0.2 interface).

## Verified build

Source commit: `ede46a331d9bfd019c3d66c4ee31496940d04a90`.
[Windows CI run 37857243591](https://github.com/vampywiz17/ParsecWebTurn/actions/runs/37857243591)
passed formatting, Clippy with warnings denied, all 51 tests, the release build
and the native diagnostic probes.

The downloaded release executable also passed the local guest-thread probe:
97 workers completed, peak concurrency was 16, the deliberate 17th concurrent
spawn was rejected, capacity recovered, and 33 completed history entries were
omitted. No account or external request was used.

The original-core 35-second offline window test presented 59 frames using
295 draw calls and two shaders on the AMD Radeon 780M accelerated OpenGL
context. It released the native window normally and rejected no worker spawns.
These are core UI frames, not decoded remote video. Waiting guest workers can
remain in the shutdown snapshot; this test does not claim every worker joined.

The release login-audit regression also passed all eight synthetic input steps.
The original core generated the expected authentication POST with a 133-byte
fixture body; offline policy blocked the request. The window presented 14 UI
frames, shut down normally, and rejected no workers. The exported report does
not contain the fixture email or password. Actual post-login behavior still
requires the user's retest with this build.
