ParsecWebTurn v0.6.0 adds update notifications and improves connection-route diagnostics.

- Background checks for the latest stable GitHub release and **App → Check for updates**.
- Release notes in an integrated window. **Download ZIP from GitHub** opens the official release ZIP in your default browser; the app only fetches release metadata.
- No automatic EXE downloads, update helper, self-replacement or update restart. Your current session continues, and browser/organization download policies apply.
- More direct paths can be identified by matching the selected ICE endpoints against transport and gathered-candidate information, including VPN paths. Positive TURN evidence wins; insufficient information remains unverified.
- Confirmed direct routes are labeled **Direct — no TURN**. This describes WebRTC routing; a VPN may carry the connection underneath it.
- Update-window commands remain restricted to the trusted local window.

Download **ParsecWebTurn-v0.6.0-win64.zip**, extract it to a writable directory, and run `ParsecWebTurn.exe`. **Microsoft Edge WebView2 Runtime must already be installed.** No VPN or administrator rights are required when the runtime is present and the app directory is writable.

**Upgrading:** close the app and replace only the EXE manually. Keep `settings.json` and `WebView2Profile` to retain saved credentials and the Parsec sign-in. Version notifications do not install the update for you.

Background check failures do not interrupt startup. Use the menu check to see network or GitHub rate-limit errors. Set `PARSECWEBTURN_NO_UPDATE_CHECK=1` to disable automatic checks; manual checks remain available. Organizational policies may still block GitHub downloads or unsigned binaries.

The normal direct/TURN fallback policy remains unchanged. This release does not claim to resolve HEVC driver bugs or long-session freezing.
