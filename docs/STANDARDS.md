# Standards and supported API audit

Date: 2026-10-01. Scope: current Rust/Tauri application, embedded web assets, settings, update notifications and development scripts on `dev`. This is a source/API audit with isolated runtime checks, not a certification of Parsec's external client or all third-party dependencies.

The baseline uses public web specifications and documented platform APIs. The user explicitly permits documented vendor diagnostics as an optional extension. Such diagnostics are disabled by default, labeled experimental, and must not become a required connection or routing contract. Ongoing rules are in [AGENTS.md](../AGENTS.md).

| Area | Contract | Audit result |
| --- | --- | --- |
| ICE configuration | W3C RTCPeerConnection/RTCConfiguration/RTCIceServer and WebIDL | The document-start adapter changes only iceServers, preserving native validation, inherited/getter dictionary members and the client's connection policy. It is application instrumentation, not an official Parsec extension API. |
| STUN/TURN URLs and transport | RFC 7064/7065 and the browser's ICE implementation | URL validation supports the configured browser transport subset. Port 53 filtering is an explicit browser compatibility policy, not an RFC syntax restriction. No custom wire protocol or SDP manipulation is implemented. |
| Route, traffic and RTT | W3C getStats and RTCIceTransport | Use selectedCandidatePairId, nominated/succeeded candidates and getSelectedCandidatePair, plus exact endpoint correlation. Removed nonstandard candidate-pair.selected and obsolete mediaType fallbacks. Missing/private address data cannot establish a direct path. |
| TURN evidence | Standard relay candidate type and relayProtocol/url fields | Positive selected TURN evidence wins. Chromium sometimes preserves relayProtocol on a prflx candidate; this is a browser observation using public fields, not a separate private API. No inactive TURN allocation or DNS lookup proves active relay use. |
| Video FPS | Public WebCodecs VideoDecoder callback | Counts delivered frames without closing them or changing configuration. Fixed dictionary spreading: native callback conversion, inherited/frozen/getter fields and construction-time callback capture are now preserved. WebCodecs is on the W3C standards track; it is not claimed to be a final Recommendation. |
| Frontend/native IPC | Documented Tauri 2 core.invoke and capabilities | Replaced direct access to __TAURI_INTERNALS__ with window.__TAURI__.core.invoke through withGlobalTauri. Native local-origin/window checks and remote capability restrictions remain. |
| Windows storage and downloads | Documented DPAPI, filesystem and ShellExecuteW APIs | Windows-specific public platform APIs, not web standards. Update checks retrieve bounded GitHub release metadata; the browser handles explicitly requested ZIP downloads. No self-update helper or executable replacement is present. |
| Window/fullscreen behavior | Public DOM events/fullscreen APIs and Tauri window APIs | The app intentionally owns fullscreen and reserves recovery shortcuts. Optional keyboard API access is feature-detected; it is not a connection dependency. |
| Detailed decoder metadata | Documented WebView2 DevTools bridge; experimental Chromium Media domain | Optional extension only. A saved mediaDiagnostics preference defaults to false, including legacy settings. Enable Detailed decoder diagnostics (experimental), then reconnect. Runtime/message-format failures remain best-effort and do not block normal connection/statistics. The displayed source identifies this vendor extension. |
| Browser launch configuration | WebView2 defaults | Removed production autoplay/background-throttling/private Edge feature switches. No production remote debugging port, disabled mDNS privacy or browser-security override is configured. |
| Native development smoke | CDP in a disposable synthetic profile | Debugging and the mDNS test override are confined to development fixtures, never packaged application launch behavior. CI's documented per-EXE WebView2 policy workaround is confined to the disposable runner and removed afterwards. |

## Internal ICE diagnostic investigation

Post-v0.6.0 additions on dev use RTCTransportStats.dtlsState/tlsVersion/dtlsCipher/srtpCipher for encryption, and inbound RTP statistics or public WebCodecs AudioDecoder input sizes/configuration for audio metrics. Missing SRTP does not imply missing data-channel DTLS encryption; certificates and keys are not exported. Native performance readings use documented WebView2 Environment8.GetProcessInfos, Windows GetProcessTimes and PDH GPU Engine counters. CPU covers the app plus its WebView2 environment; GPU covers only processes of kind GPU. The GPU summary is the busiest reported adapter/engine, with a separate video-decode reading. Absent counters/permission/runtime support remain unknown. CPU is normalized across all logical processors. No browser flags, shell commands or private diagnostics are used for these features.

An isolated two-peer, data-channel/video fixture was run with WebView2 `Edg/154.0.4258.48`. The internal `edge://webrtc-internals/` page opened and saw both synthetic connections. `chrome://webrtc-internals/` resolved to the Edge equivalent. No real account or user profile was inspected.

The official CDP domain definitions expose no dedicated public ICE/WebRTC transport-diagnostics domain. The observed runtime Schema domain list also exposed no ICE domain; that list is not treated as an exhaustive capability proof. The runtime returned no matching webrtc/p2p/ice tracing categories for this probe.

Chromium's WebRTC Internals stats collection calls the standard native stats collector. The internal UI also has browser-specific update/event/recording mechanisms; its page globals and chrome.send messages are not a supported application API. No page scraping or private message calls have been integrated into the shipped application. This investigation does not establish a new supported method for proving the user's VPN path direct. It does not assume that every internal dump is identical to the web-visible report or that missing relay data excludes TURN.

## Sources and limits

- [W3C WebRTC](https://www.w3.org/TR/webrtc/) and [WebRTC statistics](https://www.w3.org/TR/webrtc-stats/).
- [W3C WebCodecs](https://www.w3.org/TR/webcodecs/), including the native dictionary/callback behavior and hardwareAcceleration preferences. A preference is not proof of actual hardware decoding.
- [RFC 7064](https://www.rfc-editor.org/rfc/rfc7064), [RFC 7065](https://www.rfc-editor.org/rfc/rfc7065), [RFC 8445](https://www.rfc-editor.org/rfc/rfc8445) and [RFC 8656](https://www.rfc-editor.org/rfc/rfc8656).
- [Tauri core API](https://v2.tauri.app/reference/javascript/api/namespacecore/) and [withGlobalTauri configuration](https://v2.tauri.app/reference/config/#withglobaltauri).
- [WebView2 DevTools protocol](https://learn.microsoft.com/en-us/microsoft-edge/webview2/how-to/chromium-devtools-protocol) and [browser flags guidance](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/webview-features-flags).
- [Official experimental Media definition](https://github.com/ChromeDevTools/devtools-protocol/blob/master/pdl/domains/Media.pdl) and [CDP domain definitions](https://github.com/ChromeDevTools/devtools-protocol/tree/master/pdl/domains).
- [Chromium WebRTC Internals collection](https://chromium.googlesource.com/chromium/src/+/main/content/browser/webrtc/webrtc_internals.cc) and [PeerConnectionTracker](https://chromium.googlesource.com/chromium/src/third_party/+/refs/heads/main/blink/renderer/modules/peerconnection/peer_connection_tracker.cc).

Standards-track drafts, optional fields and documented vendor APIs are identified separately; this audit does not claim that every feature is a final W3C standard or portable to every browser. Actual VPN/remote-TURN ambiguity still needs data from the affected connection. Unknown remains unknown.
