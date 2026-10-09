# M3k-fix6 / 0.14.6 — contain signaling/ICE attempt failures

The user's 0.14.5 report confirms that v2 signaling reached `begin_p2p` and two
`add_candidate` calls. The signaling worker then failed and the global worker
failure policy stopped the app. Its redacted error and missing boundary do not
establish whether candidate validation or a closed native command queue caused
the failure. The private report is not retained in the repository.

Remote credential/candidate validation errors and native attempt command failures
now terminate the affected attempt with a Parsec WebRTC connection-error event
(-6200), rather than escaping into the guest worker as Wasmtime traps. Bounded
guest-memory reads and unexpected runtime/unsupported-bridge errors still fail
explicitly. Late begin/candidate callbacks after attempt cleanup cannot recreate
the attempt. Both pre-connect and connected failure events clear old events and
buffers and leave the application available for another attempt.

The backend retains `attempt_failure` and an `attempt_diagnostic` snapshot after
cleanup. Native worker snapshots add a fixed `failure_stage` enum for local/remote
description application, candidate gating/application, command queue and deadline
failures. Underlying Rust/WebRTC errors, candidate addresses, ICE credentials,
attempt identifiers and SDP are never included in these diagnostic fields. The
worker publishes its failure stage before dropping its command receiver to retain
the cause when a producer detects a closed queue. A new attempt resets diagnostics.

This contains the known error path and improves diagnosis; it does not establish
the exact cause of the user's transport failure or claim that a real host now
connects. STUN/TURN integration and remote video/audio decoding remain unfinished.

Regression coverage includes an actual controlled WASM guest supplying an invalid
remote candidate and repeating it after cleanup, with no guest trap, no private
address retained and a connection failure status. A separate test verifies that
failure replaces a full event queue, clears unread buffers and emits the correct
connecting/connected failure event. Existing native negotiation tests remain.

User retest: extract the new package, run `START-NETWORK-DIAGNOSTIC.cmd`, log in,
select Connect once, then close normally and preserve `account-network-report.json`.
The connection may still fail; the app should remain open for this handled path.
Both account modes include the v2 signaling fix; origin reporting stays opt-in.

## Verified build

Source: `97c6eb032fce5b033b2a7c216b763140697ef86e`.
[Windows CI 37903372095](https://github.com/vampywiz17/ParsecWebTurn/actions/runs/37903372095)
passed all 71 tests, formatting, strict Clippy, release compilation and the
original-core/native bridge probes. Both targeted failure regressions passed.
The downloaded release EXE completed all nine offline original-login fixture
steps and presented 15 GPU UI frames on the AMD Radeon 780M. Its window was
released without a guest error or rejected worker spawn; default reporting
omitted origins and fixture credentials. No real account or external host was
used in these automated/local checks. The user's failing connection still needs
to be retested; these results verify containment of the tested error paths.
