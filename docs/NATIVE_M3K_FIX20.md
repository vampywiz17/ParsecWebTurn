# M3k-fix20 / 0.14.20 — establishment timeout

The real-host 0.14.19 report confirmed `attempt_failure: deadline` after successful native transport establishment. Its retained pre-close snapshot showed connected ICE, DTLS and peer states, followed by peer closure and a deadline failure. No raw user report or private session contents are committed. The worker incorrectly applied its 30-second connection deadline to the entire session. This caused a reported Parsec 6200 failure without a process crash.

The deadline now bounds initial establishment only: connected peer plus all three open negotiated data channels. Once established it is permanently disarmed for that attempt, including transient later disconnects. Unconnected attempts still expire after 30 seconds. Command processing retains bounded 250 ms polling, cancellation, existing transport failures and peer cleanup. This does not suppress actual network failures or add reconnection.

Tests exercise timeout boundaries and long-session/transient-disconnect behavior with a deterministic clock. The controlled native WebRTC/guest probe also waits 31 real seconds after connection, calls guest metrics/status and verifies encrypted keyboard packet delivery afterward. Existing media/control and cleanup checks remain enabled. No account or private capture is used in these tests.

Reports add `connection_deadline_scope: establishment-only` and a latched `connection_established` flag. Video/audio decoding remain unavailable; the separate investigation of absent video packets is unfinished. This change addresses the demonstrated deadline failure only.

Verified: Windows CI 37931189785, source 647c348166ef1cde6634d5f54f074ed0a4062500, passed formatting, 105 tests, strict Clippy, release build and all original-WASM/native transport probes. Both control and buffer integration probes verified delivery after 31 real seconds and orderly peer closure. The local pinned original-WASM offline UI completed 9 synthetic login steps, presented accelerated frames and released its native window without an error; version 0.14.20 was verified. Real-host long-session validation awaits user testing.
