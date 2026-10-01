# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

### Added

- Display standard WebRTC transport encryption statistics: DTLS state, negotiated version and cipher suite, plus the SRTP protection profile when reported. Data-channel DTLS is distinguished from TURN TLS; missing telemetry remains unknown.
- Measure the app and its WebView2 processes' CPU consumption using Windows process times. Show the WebView2 GPU processes' busiest engine and video-decode engine utilization when Windows GPU counters are available.
- Display audio codec, sample rate and channel count using standard WebRTC RTP statistics or WebCodecs decoder configuration. Keep configured metadata visible during silence, including before the first audio packet. The remote configured bitrate remains unknown when the receiving APIs do not expose it; instantaneous throughput is not substituted for it. No audio frames are copied or closed by instrumentation.

## [0.6.0] - 2026-10-01

### Added

- Background GitHub stable-release notifications and App → Check for updates.
- Native update window with release notes and a direct link to the official Windows ZIP, opened in the default browser only after a click.
- Bounded metadata-only HTTPS checks, stable numeric version comparison and exact repository ZIP link validation.

### Changed

- Hide detailed video statistics, including codec and resolution, when optional Media diagnostics are disabled; saved preference changes apply to their visibility immediately.
- Audit the runtime against public web specifications and documented platform APIs; record ongoing requirements in AGENTS.md and findings in docs/STANDARDS.md.
- Replace private Tauri frontend IPC access with the documented global core API; keep native capability restrictions.
- Make experimental Chromium Media diagnostics explicit opt-in, disabled for both new and legacy settings. Standard WebRTC/WebCodecs telemetry remains the baseline.
- Remove production Chromium launch switches and legacy vendor-prefixed WebRTC/statistics fallbacks.
- Preserve native WebCodecs callback dictionary conversion, inherited/frozen callbacks and callback capture semantics.
- Update notifications do not download or run executables, create a helper, replace the app or restart it. Updates are installed manually and follow the browser's organization policies.
- Correlate selected ICE endpoints with the ICE transport and gathered local/remote candidates to identify additional direct paths, including VPN connections. Positive TURN evidence retains priority; incomplete or ambiguous evidence stays unverified.
- Label confirmed direct routes as "Direct — no TURN"; VPN routing underneath WebRTC does not imply TURN use. Candidate addresses remain inside the WebView and are not included in telemetry.

## [0.5.0] - 2026-09-30

### Changed

- Replace the Go launcher and external Edge app window with a Rust/Tauri 2 application using Windows WebView2.
- Integrate settings, the Parsec window, developer tools and connection statistics into the same application.
- Embed the ICE override and UI in the executable; no extension directory or generated credential script is needed.
- Preserve v0.4.0 settings and CurrentUser DPAPI encryption. WebView2 uses a new profile and requires a fresh Parsec sign-in.
- Port Cloudflare requests, custom TURN, encrypted caching, validation and local ice.json fallback to Rust.
- Keep the direct/relay policy selected by Parsec; no forced relay mode.
- Replace ICE configuration spreading with a native WebIDL dictionary adapter, preserving inherited/getter fields, frozen inputs and native argument errors for constructors and setConfiguration.
- Validate STUN and TURN URI syntax separately and normalize case-insensitive scheme names.
- Build and validate the Windows distribution with Cargo and a committed lockfile.

### Added

- Live WebRTC traffic rates, connection RTT, direct/relay route and transport diagnostics.
- Show selected local/remote ICE candidate types; ambiguous candidate pairs remain unknown.
- Read Chromium Media codec, decoder, hardware decoding, profile and visible resolution, including stream reconfiguration.
- Native F11 fullscreen and Ctrl+Shift+W windowed recovery; block automatic web fullscreen and Escape locking.
- Proper App/View submenus and dark native windows.
- Video codec, decoder, FPS, resolution and packet-loss details when provided by the web client.
- Count actual WebCodecs decoded frames for data-channel video FPS; distinguish unavailable video packet loss from zero loss.
- Capture fullscreen recovery shortcuts before Parsec's handlers, including when the native menu is hidden.
- Mark peer-reflexive ICE paths as unverified instead of assuming direct routing.
- Recognize Chromium peer-reflexive TURN paths by the selected relay transport; show configured TURN usage, matched server and route evidence. Use the selected ICE transport pair when report selection is ambiguous.
- Restricted native command access: the remote Parsec page can submit bounded statistics but cannot read or change saved settings.

### Fixed

- Hide connection settings during automatic startup; show the ready form on first run, explicit settings launch or configuration/connection errors.
- Closing the Parsec window exits the application instead of reopening connection settings; internal window replacement during reconnect remains supported.

### Requirements

- Microsoft Edge WebView2 Runtime must be present. The runtime is separate from the Edge browser; the app does not silently install it.
- HEVC decoding and the previously reported long-session freeze remain separate issues; this migration does not claim to fix them.

## [0.4.0] - 2026-09-30

### Added

- Custom STUN/TURN provider configuration for standard services such as coturn and eturnal.
- Multiple server URLs with a common username/password; static and externally generated credentials, UDP/TCP/TLS endpoints and STUN-only configurations are supported.
- Windows DPAPI protection for saved custom TURN passwords.
- Provider switching retains Cloudflare and custom configuration without requiring re-entry of secrets.
- Custom provider, legacy migration, encrypted storage, no-network startup and native form-save tests.

### Changed

- Suppress Edge's automatic translation offer in the dedicated app profile while preserving other browser preferences.
- Modern settings window with a dark header, Parsec-inspired pink accents, a spaced connection card and inline validation.
- Provider-specific fields are shown only when relevant; custom mode requires no Cloudflare key, TTL or API request.
- Generalize startup errors to cover both providers.

### Security

- Custom credentials are encrypted in settings, but the generated browser injection necessarily contains the active TURN credential. For static custom accounts this credential is long-lived. Do not share the generated injection or profile.
- TURN REST shared secrets are not accepted as client passwords; generate the temporary username/password on your server and enter those values instead.

## [0.3.0] - 2026-09-29

### Added

- Optional DPAPI-encrypted TURN credential cache with conservative expiry checks and invalidation on Key ID, API token or TTL changes.
- Dedicated Edge profile detection and launcher locking to prevent stale configuration reuse and concurrent writes during startup.
- Go tests covering ICE/API validation, Windows DPAPI, credential caching, file replacement, profile locking and injection generation.
- JavaScript regression tests for construction, subsequent configuration updates, inheritance, duplicate injection and credential-free logging.

### Fixed

- ICE overrides now also apply to `RTCPeerConnection.setConfiguration()`.
- Invalid ICE entries and missing TURN authentication are rejected before Edge starts.
- Browser-blocked port 53 endpoints are removed from ICE configurations.
- TURN usernames/passwords are no longer printed to the console.
- Windows messages use real newlines; fallback errors retain both the API and fallback failure.
- Settings, cache and generated injection use temporary files with Windows replacement semantics.
- Local builds embed the same Common Controls v6 manifest as release builds.

### Changed

- Split the launcher into small files for settings, UI, DPAPI, Cloudflare, ICE, cache, files, profile, extension and Edge operations.
- Replace version-specific release workflows with one tag-triggered release workflow and a shared build/package script.
- Commit dependency checksums, pin the resource generator and store the icon locally; remove build-time icon downloads/conversion and `go mod tidy`.
- Add a TLS TURN port 443 fallback to the example configuration.
- Inject the executable version into the API User-Agent at build time.

### Compatibility

- Existing settings files still work; credential caching is disabled unless enabled in Settings.
- Credentials are refreshed at launch, not automatically within an open browser session. Close the dedicated Edge profile before relaunching and choose a TTL longer than your expected session.

## [0.2.1] - 2026-09-29

### Fixed

- Fixed `TTM_ADDTOOL failed` when opening the native settings window on affected Windows systems.
- Embedded a Windows application manifest enabling Microsoft Common Controls v6, which the Walk GUI toolkit requires for reliable tooltip/control initialization.


## [0.2.0] - 2026-09-29

### Added

- First-run native Windows settings dialog.
- Direct Cloudflare Realtime TURN credential generation using TURN Key ID + API token.
- Automatic fresh short-lived TURN username/password generation on every launch.
- Configurable credential TTL up to 172800 seconds / 48 hours.
- Windows DPAPI Current User protection for the stored Cloudflare API token.
- `--settings` and `/settings` options to reopen configuration.
- Local `ice.json` fallback when the Cloudflare API cannot be reached.
- Parsec icon embedded in the Windows executable.
- Explicit third-party/trademark notice for the icon.

### Changed

- `ice.json` is no longer required for normal use.
- Normal startup now obtains `iceServers` from Cloudflare before launching Edge.
- README expanded with Cloudflare setup and security model.

### Security

- The long-lived API token is not stored in plaintext.
- The token is encrypted using Windows DPAPI and bound to the current Windows user context.
- Short-lived TURN credentials may still appear in generated `extension/inject.js` for the duration of their validity.
- Backend-free mode assumes the local Windows user/device is trusted.

## [0.1.0] - 2026-09-29

### Added

- Initial public release of ParsecWebTurn.
- Portable Windows x64 launcher with no administrator rights required.
- Microsoft Edge app-mode launcher for `https://web.parsec.app/`.
- Automatic WebRTC ICE override at `document_start`.
- Cloudflare STUN/TURN support through external `ice.json` configuration.
- Separate local Edge profile stored beside the application.
- Example TURN configuration in `ice.example.json`.
- Manifest V3 helper extension scoped to `https://web.parsec.app/*`.
- GitHub Actions build workflow.
- MIT license.
