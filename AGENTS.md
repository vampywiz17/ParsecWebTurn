# Development requirements

The user's standing requirement is to keep current and future development standards-compliant and maintainable.

- Use W3C/IETF standards for WebRTC, ICE, STUN and TURN behavior. Preserve native WebIDL validation and the client's connection policy.
- Use documented, supported Tauri, Rust and Windows/WebView2 APIs for platform functionality. Platform APIs are not automatically web standards; describe that distinction accurately.
- Do not introduce production dependencies on private Chromium interfaces, internal browser pages or undocumented switches. The user permits documented vendor diagnostics as an optional extension: keep them disabled by default, clearly label their experimental/vendor status, isolate failures, and retain a standards-based fallback. Never use unstable diagnostic output as a required production contract or a definitive routing proof.
- Missing telemetry is unknown, not negative evidence. Report confirmed direct/TURN routing only when the selected active connection provides sufficient evidence; preserve uncertainty otherwise.
- Keep diagnostic tests isolated from real credentials and profiles. Do not expose production remote debugging ports, weaken browser security/privacy or require administrator rights for ordinary use.
- For each proposed integration, verify its documented support and fallback behavior before implementation. If a required result cannot be established through supported APIs, document the limitation instead of inventing a heuristic presented as certainty.
