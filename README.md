# ParsecWebTurn

A portable Windows application for **Parsec Web**, written in **Rust with Tauri 2**. It opens the web client inside the app and lets you use Cloudflare Realtime or your own STUN/TURN servers, including coturn, eturnal and ExpressTURN.

The connection can be direct when the network allows it, or use a TURN relay when needed. No VPN or driver installation is required. A TLS TURN endpoint on port 443 can help on restrictive networks, but the network must also permit Parsec's HTTPS/WebSocket services and the selected relay.

<img width="1803" height="863" alt="image" src="https://github.com/user-attachments/assets/d2f2d812-1f3d-4a25-8ee9-92254c3334d2" />

## Requirements and first run

- Windows 10/11 x64, with **Microsoft Edge WebView2 Runtime** already installed.
- A writable directory for the executable, settings and browser profiles.
- Cloudflare TURN credentials or an existing custom TURN account.

The WebView2 runtime is **separate from the Microsoft Edge browser**. Windows 11 normally includes it, but managed computers may have different configurations. ParsecWebTurn does not silently install a runtime or require administrator rights when the runtime is present. If it is missing, ask your administrator or use Microsoft's [WebView2 Runtime installer](https://developer.microsoft.com/en-us/microsoft-edge/webview2/).

1. Extract the release ZIP to a writable directory.
2. Run `ParsecWebTurn.exe`.
3. Choose Cloudflare Realtime or Custom server, enter the relay details, and click **Save & connect**.
4. Sign into Parsec inside the application.

Web assets and the ICE override are embedded in the executable. There is no browser extension to install or `extension` directory to copy. The native app menu provides **Connection settings**, **Connection stats**, **Developer tools** and **Exit**. Settings can be saved without interrupting an active connection; **Save & connect** replaces the current session.

On subsequent launches, valid saved settings open Parsec automatically. Use `ParsecWebTurn.exe --settings` or `/settings` to start with the settings window instead. `--data-dir <directory>` selects a different portable configuration/profile directory.

## Application updates

On startup the application checks the latest stable GitHub release metadata in the background and notifies you when a newer version is available. The update window shows release notes and **Download ZIP from GitHub** opens the official release ZIP's direct HTTPS link in your default browser. **App → Check for updates** checks manually and shows network/rate-limit errors; background failures do not interrupt startup.

The app does not download EXEs, create an update helper, replace itself or restart for updates. Download and extract the ZIP, close the app, and replace only `ParsecWebTurn.exe` manually. Keep `settings.json` and `WebView2Profile` to retain credentials and the Parsec sign-in. Your browser and your organization's download controls handle the download. An organization may still block unsigned software or GitHub downloads; this feature does not bypass those policies. Published `SHA256SUMS.txt` checksums establish file integrity, not publisher identity.

Set `PARSECWEBTURN_NO_UPDATE_CHECK=1` to skip automatic checks; the menu check remains available. Pre-update-notification releases such as v0.5.0 need one manual EXE upgrade to gain version notifications.

## Upgrading from v0.4.0

Keep your existing `settings.json` beside the new executable. The field names and Windows DPAPI / CurrentUser format remain compatible, so saved API tokens and TURN passwords continue to work under the same Windows account.

Tauri uses a new `WebView2Profile` directory. The previous Edge `Profile` is not modified or imported; sign into Parsec again once. The settings and Parsec windows share a browser environment, with separate native permissions. Keep the profile private. Only one app instance can use the same data directory at a time.

## Cloudflare Realtime

Create a Cloudflare Realtime TURN key and enter its **Key ID** and **API token**. ParsecWebTurn requests a temporary username/password directly from Cloudflare over HTTPS. The default lifetime is 24 hours; accepted values are 60–172800 seconds (maximum 48 hours).

**Reuse valid credentials for faster startup** enables an optional DPAPI-encrypted cache. It is reused only when the key, token and lifetime match, the clock has not moved behind the issue time, and more than a quarter of the requested lifetime remains (at least five minutes). Cache corruption or write failures do not prevent a fresh request. Existing v0.4.0 cache entries can also be read.

Cloudflare requests have a 20-second timeout, reject redirects and limit responses to 1 MiB. Errors do not include API tokens or reflected server response bodies.

Credentials are resolved when connecting, **not refreshed within an already open session**. Choose a lifetime longer than the session, or reconnect to obtain a new pair. Cached credentials may have less time remaining.

## Custom TURN (coturn, eturnal, ExpressTURN)

Enter one STUN/TURN URL per line, for example:

```text
stun:turn.example.com:3478
turn:turn.example.com:3478?transport=udp
turn:turn.example.com:3478?transport=tcp
turns:turn.example.com:443?transport=tcp
```

All configured TURN URLs share the supplied username/password. STUN-only configurations can leave both empty. STUN/STUNS URIs accept a host and optional port, without a query. TURN/TURNS URIs additionally accept `?transport=udp` or `?transport=tcp`, the supported WebRTC subset. Scheme names are case-insensitive and normalized to lowercase before passing them to WebView2. The existing port 53 filter is an application compatibility policy, not an RFC requirement or a complete list of browser port restrictions.

The document-start hook replaces the Parsec ICE server list before the native RTCPeerConnection constructor runs. Its dictionary adapter replaces only `iceServers`: the native WebIDL converter reads all other fields, including inherited/non-enumerable fields and getters with their original receiver, and rejects invalid primitive arguments. Frozen inputs are supported without mutation. The same adapter applies to `setConfiguration`. This is an application integration hook, not an official Parsec configuration API; SDP, candidates and priorities are left to the client/browser. Changing active ICE credentials can require an ICE restart and offer/answer negotiation; the app does not initiate this automatically.

Custom mode makes no Cloudflare calls. Use static credentials or a temporary username/password generated by your TURN service. A TURN REST `static-auth-secret` or eturnal shared `secret` is **not a client password**. The app does not generate or renew custom REST credentials.

For [coturn](https://github.com/coturn/coturn/blob/master/examples/etc/turnserver.conf), use a long-term user or a generated REST pair. For [eturnal](https://eturnal.net/doc/), use configured static credentials or a generated pair. For ExpressTURN, enter the host, transport, port and credentials supplied by the service. TLS endpoints need a certificate trusted by WebView2; server firewall and relay-port setup remain server-side responsibilities.

Switching providers preserves each provider's saved credentials. Password fields stay blank in the settings UI: leave them blank to retain the saved value, enter a replacement, or select **Forget saved…** to remove it. Only valid settings are written.

Closing the Parsec window exits the entire application, including hidden settings and open statistics. Closing settings while Parsec is running hides only the settings window. Reconnecting from settings replaces the Parsec window internally without exiting the app.

Automatic startup keeps settings hidden to avoid a brief settings-window flash. The ready settings form appears on first run, with `--settings`, or when saved settings or connection preparation fail.

## Connection statistics

Open **Connection stats** from the native menu. The application samples `RTCPeerConnection.getStats()` once per second and displays:

- Incoming/outgoing WebRTC traffic in Mbps, calculated from byte-counter changes.
- RTT of the selected ICE candidate pair, when available.
- Direct/relay route and transport of the most active connection.
- Video FPS, codec, decoder, resolution and lost packets, when the web client exposes standard inbound-video statistics.

A document-start hook counts frames delivered through the public WebCodecs VideoDecoder API when RTP video statistics are absent. This is decoded FPS, not display refresh rate; the original callback keeps ownership of every frame. Native WebIDL callback validation, inherited/getter members and frozen inputs are preserved.

**Detailed decoder diagnostics (experimental)** in Connection settings is an optional, disabled-by-default extension. It uses WebView2's documented DevTools protocol API and Chromium's experimental Media domain to supplement codec, decoder, hardware decoding, profile, backend and visible resolution. These are vendor diagnostics, not W3C WebRTC data, and availability/message formats can vary by runtime. Apply the setting on your next connection. Unavailable information remains unknown and Media failures do not block the standard statistics or connection. No remote debugging port is opened in the shipped application.

Video packet loss is available only when inbound RTP statistics provide it. Parsec's data-channel video does not expose an equivalent counter: decoded/dropped frames, ICE checks and local send discards cannot establish end-to-end packet loss. Missing loss statistics remain unknown rather than zero.

Route describes the selected ICE candidates, whose local/remote types are displayed. Configuring TURN does not force a relay; ambiguous candidate-pair selection stays unknown.

Relay detection checks the selected candidate types **and the selected local TURN transport**. Chromium can rename a local relay candidate to peer-reflexive (`prflx`) while preserving `relayProtocol` and its TURN URL; this still confirms relay use. The selected server URL is matched to the configured TURN endpoints with default port/transport normalization. The panel shows route evidence, whether our configured TURN is in use, the matched server and the TURN transport separately from the ICE transport. A remote-only relay does not mean our configured TURN is used.

If report selection is ambiguous, the data-channel ICE transport's `getSelectedCandidatePair()` supplies route information without guessing another pair's RTT. When the report identifies a pair, both complete endpoints must match before transport data can enrich it. Peer-reflexive candidates can also be correlated against gathered local/remote candidates using exact address, port and protocol matches; conflicting ICE generations or ambiguous origins remain unverified. Positive TURN evidence always wins. A confirmed **Direct — no TURN** route can run through a VPN underneath WebRTC; this label describes TURN use, not the physical network path. Candidate addresses are used only inside the WebView and are not exported in statistics.

Merely gathering a relay candidate or reaching a TURN server does not prove that the stream uses it. A TURN URL on a STUN-derived candidate is also insufficient. No connection policy or live session is changed to test routing.

Parsec starts in a normal window. The application owns fullscreen: **F11** toggles it, **Ctrl+Shift+W** restores windowed mode, and the settings screen has a recovery button. Document-start capture handlers reserve these shortcuts even when the Parsec canvas has focus and the native menu is hidden. Automatic HTML fullscreen and locking Escape/F11/KeyW are blocked. The **App** and **View** native submenus remain separate from Parsec's controls; the menu is hidden during fullscreen.

Traffic is **actual usage, not a bandwidth-capacity test**. RTT is a WebRTC path measurement, not a separate ICMP ping. Parsec may carry video through data channels; in that case traffic/RTT can be available while video-specific fields remain unknown. Multiple connected peers have their traffic rates summed; route/RTT come from the most active connected peer. Stale samples are marked and their rate/RTT values cleared after five seconds.

The remote Parsec view can submit bounded statistics and toggle or exit its own fullscreen mode. It cannot invoke settings, credential storage, fallback or connection commands. Diagnostics never include candidate IP addresses or credentials.

This migration does not claim to fix HEVC driver issues or long-session freezing. WebView2 still uses a browser engine for the web client. Developer tools are available from the menu for troubleshooting.

## Local fallback

If the provider cannot supply usable credentials, the settings window offers **Connect using local ice.json instead**. Copy `ice.example.json` to `ice.json` and supply a valid `iceServers` object. The fallback is explicit and uses the same validation as provider responses.

## Security

Saved secrets and the credential cache use Windows DPAPI for the current user. They are not portable to another Windows account. The web client's ICE configuration necessarily contains the active TURN username/password in memory; do not share developer-tools exports or browser profiles containing private data.

The Parsec page has no native settings/storage privileges. The local settings and stats pages have separate, narrowly scoped Tauri capabilities and a content security policy. Only `https://web.parsec.app` receives the ICE override. GPU acceleration is left to WebView2; the application does not disable the browser sandbox or TLS verification.

## Build

Install stable Rust with the MSVC toolchain, Visual Studio C++ Build Tools / Windows SDK, WebView2 Runtime and Node.js (for regression tests). No Go toolchain, npm frontend framework or Tauri CLI is required.

```powershell
rustup component add rustfmt clippy
./scripts/build.ps1
```

The script checks version consistency and formatting, runs Rust and JavaScript tests, runs Clippy, builds the embedded application and packages an explicit file allowlist. `src-tauri/Cargo.lock` locks dependencies. The Windows CI uses the same script; `v*` tags publish releases after successful validation.

`node scripts/smoke-webview.cjs` tests the packaged app with an isolated WebView2 profile, a mock Parsec document and real loopback WebRTC traffic. It checks DPAPI saving, initialization order, traffic/RTT reporting and remote command isolation. The debugging port is enabled only in this test process; normal app launches do not expose it. CI runs this test before a release can be published.

For development:

```powershell
cargo run --manifest-path src-tauri/Cargo.toml -- --settings --data-dir ./dev-data
```

## Attribution

ParsecWebTurn is an independent community project, not affiliated with Parsec or Unity. The [Parsec icon](assets/README.md) identifies the Parsec-focused application. Parsec trademarks belong to their owners.
