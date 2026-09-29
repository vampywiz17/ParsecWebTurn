# Changelog

All notable changes to this project will be documented in this file.

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
