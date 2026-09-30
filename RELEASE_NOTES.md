ParsecWebTurn v0.5.0 moves the application to Rust and Tauri 2, with Parsec Web embedded through Windows WebView2.

- Integrated connection settings, Parsec window, developer tools and live connection statistics.
- Cloudflare Realtime, custom TURN services, encrypted caching and local ice.json fallback are implemented entirely in Rust; no Go helper remains.
- Existing settings.json files and CurrentUser DPAPI-encrypted credentials remain compatible.
- UI and ICE scripts are embedded in the executable; no extension directory is needed.
- WebRTC traffic rates, RTT and direct/relay routing are shown when available. Video details depend on what Parsec exposes through WebRTC.
- The connection policy remains unchanged: direct connections are allowed, with TURN available when needed.

Download **ParsecWebTurn-v0.5.0-win64.zip**, extract to a writable directory, and run `ParsecWebTurn.exe`. **Microsoft Edge WebView2 Runtime must already be installed**; it is separate from the Edge browser. The application does not silently install it.

Keep your settings.json to reuse saved configuration. WebView2 uses a new browser profile, so sign into Parsec once again. The old Edge profile is left untouched.

Credentials are obtained at connection startup, not refreshed during an open session. Select a lifetime longer than the session, or reconnect. This release does not claim to resolve HEVC driver bugs or long-session freezing.

Saved secrets are protected with Windows DPAPI. Active TURN credentials necessarily exist in the web client's memory; do not share private browser profiles or developer-tools exports.
