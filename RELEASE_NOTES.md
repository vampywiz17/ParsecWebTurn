ParsecWebTurn v0.6.0 adds in-app updates for the portable Rust/Tauri application.

- Background checks for the latest stable GitHub release, automatic downloads, and **App → Check for updates**.
- Release notes in an integrated update window. **Restart and install** requires an explicit click; **Later** keeps your current Parsec session running.
- GitHub SHA-256 asset metadata and `SHA256SUMS.txt` must agree with the downloaded executable. Unexpected asset URLs, oversized downloads, prereleases and older versions are rejected.
- A Rust helper waits for the app to exit, replaces only its EXE, preserves the executable filename and configuration/profile directory, and restarts with the existing launch mode.
- The previous executable is kept in `.parsec-update-*/previous.exe` for recovery. Replacement or launch failures attempt to restore the previous EXE.
- Update commands are restricted to the local update window; the remote Parsec client cannot initiate updates.

Download **ParsecWebTurn-v0.6.0-win64.zip**, extract it to a writable directory, and run `ParsecWebTurn.exe`. **Microsoft Edge WebView2 Runtime must already be installed.** No VPN, updater installer or administrator rights are required when the runtime is present and the app directory is writable.

**Upgrading from v0.5.0:** replace the EXE manually once to gain in-app updates. Keep `settings.json` and `WebView2Profile`; saved credentials and the existing Parsec sign-in remain available. Future updates can be applied from inside the app. Restarting disconnects an active session.

Background update failures do not interrupt normal startup. Use the menu check to see network or GitHub rate-limit errors. Set `PARSECWEBTURN_NO_UPDATE_CHECK=1` to disable automatic checks; manual checks remain available. Download integrity relies on GitHub HTTPS and release metadata, not a separate publisher signature.

The existing TURN/STUN and direct/relay behavior remains unchanged. This release does not claim to resolve HEVC driver bugs or long-session freezing.
