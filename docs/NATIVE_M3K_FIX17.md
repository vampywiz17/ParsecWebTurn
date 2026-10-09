# M3k-fix17 — structured execution diagnostics

The fix16 user report has connected ICE/DTLS and three open data channels. No worker reports an unsupported import; the wake-lock call was reached successfully. The main execution fails with `guest-execution-failed`, and the previous privacy filter discarded its exact error. The root cause cannot be established from that report. This build adds diagnostics; it does not claim to fix that new failure.

Main and worker failures now retain a public Wasmtime Trap enum (or an explicit unknown host/API error), a fixed application execution stage, and at most 12 numeric WASM frame locations. Guest symbol names, module names, raw exception text, URLs, credentials and call arguments are excluded. The most recent import name is retained without its arguments; import names belong to the pinned module. Event export stages distinguish keyboard/focus/resize/fullscreen delivery from the regular callback. Session cancellation remains distinguishable from an actual execution failure. Existing sanitization of human-readable errors remains enabled.

This uses Wasmtime's documented `Error::downcast_ref`, `Trap`, `WasmBacktrace::frames` and `FrameInfo` APIs:
https://docs.rs/wasmtime/38.0.4/wasmtime/struct.WasmBacktrace.html
https://docs.rs/wasmtime/38.0.4/wasmtime/struct.FrameInfo.html

Tests exercise an actual unreachable trap, fuel exhaustion, bounded recursive backtrace, cancellation distinction and omission of secret-looking raw messages and guest symbols. No real account is used. Audio output is still unavailable; real streaming is not yet validated.

Verified: Windows CI 37925297116 (source 56f6edcb703b1b26483b1e9bd574ffe5fe10b997) passed formatting, 97 tests, strict Clippy, release build and all guest/transport probes. The local original-WASM offline login completed 9 synthetic UI steps, presented GPU frames and released its native window without a start error or power request. All 38 packaged file hashes were verified. The fix16 main-thread failure remains unresolved pending a new user report.
