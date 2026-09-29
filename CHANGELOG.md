# Changelog

All notable changes to this project will be documented in this file.

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

### Security

- Real `ice.json`, generated `extension/inject.js`, and `Profile/` are excluded from Git.
- TURN API keys are not embedded. Only short-lived TURN credentials should be placed in the local `ice.json`.

### Notes

This first release is intended for Windows x64 and relies on Microsoft Edge being available on the machine. It installs nothing and does not require a separate .NET or WebView2 installation.
