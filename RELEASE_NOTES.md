ParsecWebTurn v0.8.0 (dev)

## 0.8.0 — Native Rust client (dev)

- Replace the WebView2 launcher with the native Rust client using the pinned Parsec WASM core (150-104a).
- Embed the core in one executable; no separate WASM file, launcher script or WebView2 runtime is required.
- Keep sign-in and client preferences in an encrypted, current-Windows-user profile across restarts. Previous Tauri configuration and profiles remain untouched.
- Present GPU-resident decoded video through the native Windows graphics path; add native Opus/WASAPI audio, keyboard, pointer and the original Parsec overlay. Relative pointer capture remains controlled by the overlay.
- Enable legacy RSA-1024 host identity compatibility while retaining signature and certificate fingerprint verification.
- Remove the prototype's forced Cloudflare STUN setting and exclude diagnostic commands, synthetic fixtures and test-only readback from the normal executable.

This dev client does not yet expose custom STUN/TURN settings or the former Tauri statistics panel. Future STUN/TURN configuration will retain the previous settings schema and DPAPI format. Additional audio formats, menu integration and remote input testing remain follow-up work.

