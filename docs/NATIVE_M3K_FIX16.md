# M3k-fix16 — optional native screen wake lock

The fix15 user report shows the audio-unavailability worker completing without an error, with ICE/DTLS connected and three data channels open. The UI worker then fails at `env::web_wake_lock`. This fix implements that missing import rather than suppressing worker failures.

The pinned `(i32) -> void` guest import posts a coalesced request to the native window UI thread. Windows `SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED)` requests screen availability. Acquisition and release use that same thread. Minimizing releases the request; restoring reapplies the desired request. Close, destruction and UI-loop exit release it. No system-sleep inhibition or changes to power settings are made. OS denial remains nonfatal and is counted in `native_wake_lock`; requested and applied states are distinct. Without an online native window, the optional request does not invoke the OS.

Documented Windows contract: https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setthreadexecutionstate

Validation covers the exact guest import, visibility transitions, idempotence, rejected acquisition/release and retry, and a native same-thread API call/release. Windows CI also runs all existing transport and guest probes. Real-host streaming must still be retested by the user. Native audio output remains unavailable; this fix adds no audio/video decoder and does not claim a completed streaming client.

Verified: Windows CI 37923922089 (source b1e1cb5690030b02fc931b83f7df2fc011998bdb) passed formatting, 93 tests, strict Clippy, release build and all guest/transport probes. The local original-WASM offline login completed 9 synthetic UI steps and 15 hardware-accelerated frames, released its window, and reported no requested/applied wake lock or power-request failure. The final ZIP verifies all 37 file hashes. Real-host streaming remains awaiting user testing.
