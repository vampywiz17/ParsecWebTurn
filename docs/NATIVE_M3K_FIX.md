# M3k synchronization fix (0.14.1)

The user verified M3k login, links, paste and Tab. Starting a connection then
stopped the render worker at the unimplemented `env::MTY_WaitPtr` import, after
`parsec_web_new_attempt` and native status 20. This was before video decoding.

The bridge now implements the original pinned Matoya 0/1 synchronization latch
using Wasmtime's public shared-memory atomic wait API. An early completion is
consumed immediately; otherwise the worker waits for notification and resets the
latch. Polling timeouts never count as completion. Live account waits check window
shutdown every 25 ms without an arbitrary connection deadline. Offline probes
have a five-second safety deadline. The pinned notifier retries notification,
covering the gaps between cancellation checks.

This is a version-specific Parsec/Matoya ABI bridge, not a web standard. The
implementation uses documented WebAssembly atomic operations through Wasmtime:
https://github.com/bytecodealliance/wasmtime/blob/v38.0.4/crates/wasmtime/src/runtime/memory.rs

The guest-offer probe calls the actual host import and verifies native credential
completion, latch reset and peer cleanup. Unit tests cover early completion,
delayed notification across polling intervals, cancellation, deadline and invalid
memory/state. No real account, clipboard content or user report is embedded in
the tests or package.

Real-host connection and remote video/audio playback remain unverified. This fix
removes the reported boundary; it does not establish that later stream-startup
stages are complete. The prototype currently gathers native UDP host candidates;
production STUN/TURN settings are not yet integrated into this separate prototype.
