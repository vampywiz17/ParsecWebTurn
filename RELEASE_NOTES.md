ParsecWebTurn v0.3.0 improves startup reliability and simplifies building and releasing the portable Windows launcher.

- ICE overrides cover both WebRTC construction and later `setConfiguration()` calls.
- Invalid server configurations are rejected before launch; browser-blocked port 53 URLs are filtered.
- Optional DPAPI-encrypted credential caching speeds up repeat launches while conservatively checking expiry and settings changes.
- Running profiles and concurrent launchers are detected so the generated configuration is not silently replaced while Edge is active.
- TURN credentials are no longer logged to the browser console.
- Settings and runtime files use safe replacement writes.
- Local and CI builds share one script, embed the Common Controls v6 manifest and checked-in icon, and run Go and JavaScript regression tests.

Download **ParsecWebTurn-v0.3.0-win64.zip**, extract it to a writable directory, and run `ParsecWebTurn.exe`. Existing `settings.json` files remain compatible; caching is opt-in in Settings. The standalone EXE also needs the `extension` files provided in the ZIP.

Close existing ParsecWebTurn Edge windows before starting this version. Credentials are refreshed at launch; choose a TTL longer than your expected session. There is no automatic credential refresh within an already open session.

The direct-to-Cloudflare, no-install design is unchanged. The API token and optional cache are protected by Windows DPAPI for the current user. Generated `extension/inject.js` still contains temporary TURN credentials and should not be shared.
