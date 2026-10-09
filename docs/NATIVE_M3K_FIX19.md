# M3k-fix19 — unavailable absolute mouse must not terminate the guest

The new user report has a host/API error at `mty_window_motion`, with `parsec_web_send_message` the last host import. The transport remains connected and the backend status is zero. The adapter explicitly rejected absolute mouse input because decoded video dimensions and a real presented-video viewport are not available. That rejection propagated into WASM and closed the application.

The low-level input encoder now uses a typed unavailable-capability error after validating the event fields. The backend handles only that precise condition nonfatally: no guessed packet is sent, a bounded counter records the omitted event, and a static reason reports `absolute-mouse-requires-presented-video`. Malformed or unsupported input remains strictly rejected; unrelated errors are not swallowed. Relative mouse and keyboard packet encoding are unchanged. Counts reset with the attempt. This is not absolute mouse support: mapping and input delivery require the future real video presentation path.

Reports now include `prototype_version` from Cargo metadata. The submitted report did not contain the media-ingress field introduced in fix18, so its exact build could not be verified. The new version field makes subsequent reports unambiguous. No coordinates, raw input payloads or extra private strings are recorded.

The controlled encrypted native WebRTC/guest probe calls the same send-message import with absolute mouse input, verifies it returns without a trap, checks the unchanged connected status and unavailable counter, and proves no packet reaches the peer. Existing binary keyboard, media-ingress and control proofs remain enabled. Tests separately cover malformed events, relative encoding, saturation, reset and omission of coordinates from diagnostics.

Limitations: no native decoded video or audio output, no absolute mouse mapping. The existing 30-second native-worker bound remains. Real-host behavior still needs testing.

Verified: Windows CI 37928647959 (source 3fb1a51c5a0cc1d49c196b052a8e7f838f5a285c) passed formatting, 103 tests, strict Clippy, release build and all guest/transport probes. The controlled WebRTC probe verified that absolute mouse input without video returns through the real guest import without trapping, sends no packet and preserves connected status. The local original-WASM offline UI completed 9 synthetic steps, presented 15 accelerated frames, released its window without an error and reported prototype version 0.14.19. Real-host behavior awaits user testing.
