# M3j / 0.13.2: preserve the pinned web CryptoHash stub

The user's 0.13.1 account report reached ten concurrently active workers and
rejected zero spawns. After login the rendering worker stopped at the explicit
unsupported-import boundary `env::MTY_CryptoHash`, not WASI proc_exit.

The pinned original `matoya-worker.js` defines MTY_CryptoHash with an empty
function body. The pinned WASM signature has seven i32 parameters and no
return value. The host now mirrors that exact unavailability: no hashing,
no output writes, no claimed successful digest, and no fatal missing-import
trap. This is compatibility with a private, version-pinned Matoya interface,
not a new cryptographic implementation or a web-standard API.

A controlled WASM regression invokes the actual host bridge with the exact
signature and checks that guest input, key and output memory are unchanged,
the call is recorded, and no missing-import boundary is reported. No account,
credentials or external requests are involved. Unknown imports still trap.

The original core and account network policy are unchanged. Actual post-login
behavior still needs the user's retest; a later unsupported import may be
reached. User reports and credentials are not committed or packaged.

## Verified build

Source: `c558175e55523bfca06d28b18588f7267350b965`.
[Windows CI run 37858578872](https://github.com/vampywiz17/ParsecWebTurn/actions/runs/37858578872)
passed all 52 tests, including the exact void CryptoHash regression, formatting,
Clippy with warnings denied, release compilation and all native diagnostic
probes with the pinned original core. Its inspection report classifies
MTY_CryptoHash as `unavailable-as-in-web-client`.

The downloaded release executable passed the local original-core login-audit:
eight synthetic input steps, 14 GPU UI presentations, 74 draw calls, two
shaders, no rejected workers, and normal window release. The fixture's
133-byte authentication POST was blocked by offline policy. No real account
was used, and the report contains neither fixture email nor password.
This regression verifies the login UI, not the authenticated post-login view.

## User retest feedback — 2026-10-09

The user reports that 0.13.2 logged in and remained open without crashing;
other menu sections were not functional. This is user-confirmed progress,
not evidence of host connection, decoded video or complete menu support.

The supplied account-report snapshot records no main/worker errors, no
unsupported-import boundary, no proc_exit code and no rejected thread spawns.
It records six UI presentations and normal native-window release. Its network
audit is empty and it contains no CryptoHash call, so this particular snapshot
does not independently capture the reported authentication/post-login phase.
Keep that distinction: the user confirms the visible result; the attached
snapshot only confirms an error-free captured window lifecycle. The original
report and any account data are not copied into the repository.

The user then repeated login and supplied the report before restarting. This
new snapshot captures two authentication POST intents followed by authorized
API GETs and public-asset requests (11 allowed HTTPS intents in total). It
also records two CryptoHash calls with no missing-import boundary, 233 GPU UI
presentations, 1,615 draw calls, a peak of ten active workers, 13 completed
workers, and zero rejected spawns. Main/worker error and exit-code fields are
empty, and the native window was released on shutdown. Three waiting workers
remain unfinished in the shutdown snapshot; this is not an all-workers-joined
claim. The request audit records policy decisions, not HTTP response success.
Together with the user's observed successful login, this verifies the reported
post-login crash fixes for this run. Complete menus, host connection and remote
video remain unverified. No account report or credential values are retained.
