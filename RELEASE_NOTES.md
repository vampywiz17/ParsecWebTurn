ParsecWebTurn v0.5.0 moves the application to Rust and Tauri 2, with Parsec Web embedded through Windows WebView2.

- Integrated connection settings, Parsec window, developer tools and live connection statistics.
- Cloudflare Realtime, custom TURN services, encrypted caching and local ice.json fallback are implemented entirely in Rust; no Go helper remains.
- Existing settings.json files and CurrentUser DPAPI-encrypted credentials remain compatible.
- UI and ICE scripts are embedded in the executable; no extension directory is needed.
- Live incoming/outgoing traffic, connection RTT, selected ICE route, configured TURN usage and matched server. Ambiguous routes remain unverified.
- Chromium Media events provide codec, profile, decoder, hardware decoding, backend and visible resolution. WebCodecs counts actual decoded FPS for data-channel video; unavailable packet loss stays unknown.
- Parsec starts windowed, with dark App/View menus, F11 fullscreen and Ctrl+Shift+W windowed recovery.
- Automatic startup no longer flashes the settings window. First run, explicit settings launch and preparation errors show the settings form.
- Closing the Parsec window exits the entire application; reconnecting internally replaces the window without exiting.
- ICE configuration preserves native WebIDL behavior and validates STUN/TURN URIs separately. Native command permissions isolate the remote Parsec page from settings and saved secrets.
- The connection policy remains unchanged: direct connections are allowed, with TURN available when needed.

Download **ParsecWebTurn-v0.5.0-win64.zip**, extract to a writable directory, and run `ParsecWebTurn.exe`. **Microsoft Edge WebView2 Runtime must already be installed**; it is separate from the Edge browser. The application does not silently install it.

Keep your settings.json to reuse saved configuration. WebView2 uses a new browser profile, so sign into Parsec once again. The old Edge profile is left untouched.

Credentials are obtained at connection startup, not refreshed during an open session. Select a lifetime longer than the session, or reconnect. This release does not claim to resolve HEVC driver bugs or long-session freezing.

Saved secrets are protected with Windows DPAPI. Active TURN credentials necessarily exist in the web client's memory; do not share private browser profiles or developer-tools exports.

Validation: Windows CI passes 13 Rust tests, 22 JavaScript tests, formatting, strict Clippy and native WebView2 integration checks, including decoded FPS, fullscreen recovery, reconnect, app exit and startup visibility.
