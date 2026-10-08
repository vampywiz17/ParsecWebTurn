# M3j / 0.13.1: release completed worker capacity

The user's first real-account test logged in and immediately closed the native
window. Its redacted report contains eight guest thread records (IDs 2–9).
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
