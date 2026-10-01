# Provider-dependent ICE selection investigation

Date: 2026-10-01. This records an investigation, not a confirmed routing fix.

## Observations

The user reports the same host and VPN connecting directly with STUN-only or
ExpressTURN configuration, but carrying stream traffic through Cloudflare TURN
with the Cloudflare configuration. ExpressTURN has a separate STUN URL and a
`turn:` URL with neither an explicit transport nor a `turns:` URL. The exact
ExpressTURN hostnames and ports have not been supplied.

The app's custom provider passes the configured URLs through the common ICE
validator. Its Cloudflare provider passes the credential API's URLs through the
same validator, excluding browser-blocked port 53. Neither path changes
`iceTransportPolicy`, candidate priority or SDP. Custom configuration puts the
URLs in one RTCIceServer dictionary; Cloudflare can return separate STUN and TURN
dictionaries. A difference in dictionary grouping is not evidence of a changed
connection policy.

Cloudflare's documented example includes STUN UDP/3478 and TURN UDP/3478,
UDP/443, TCP/3478, TCP/80, TLS/5349 and TLS/443. Actual API responses should be
checked rather than assuming the example is the exact current server list.
Chromium's WebRTC URL parser defaults a plain `turn:` URL to UDP. Thus the
reported configurations differ in offered transports as well as provider.

Read-only inspection of the public `https://web.parsec.app/lib/parsec.js`, fetched
on this date, found that its outgoing candidate callback forwards UDP candidate
addresses/ports and host/server-reflexive flags without the original candidate
priority. Its incoming candidate adapter constructs a candidate with a fixed
priority. This observation does not establish how the native host selects a
path and is not a supported extension contract. Production code must not depend
on or modify these implementation details.

## Controlled comparison

Keep the host, VPN, app/runtime version and client ICE policy unchanged. Use
fresh connections and repeat each condition:

1. Cloudflare STUN plus only `turn:turn.cloudflare.com:3478?transport=udp`.
2. The same STUN plus Cloudflare's full returned TURN URL list.
3. Cloudflare STUN plus the ExpressTURN UDP URL.
4. ExpressTURN STUN plus the same ExpressTURN UDP URL.

Use valid provider credentials locally; do not include credentials in reports.
Condition 1 still permits direct and relay paths. It does not disable relay or
force direct. Do not change the application's default server list on the basis
of this untested hypothesis; removing TCP/TLS would remove useful fallback paths
on restricted networks.

Compare standard selected candidate pairs, candidate types/priorities,
relayProtocol, successful/failed connectivity checks and candidate arrival times.
Inactive TURN allocations alone do not establish that stream traffic is relayed.
Multiple relay candidates or earlier availability can affect negotiation, but
neither proves why this particular connection selected relay. The exact cause
remains unconfirmed until affected connections are compared.

## Primary references

- [Cloudflare credential API and example URL list](https://developers.cloudflare.com/realtime/turn/generate-credentials/)
- [ExpressTURN supported ports and transports](https://www.expressturn.com/)
- [Chromium WebRTC ICE server parser](https://webrtc.googlesource.com/src/+/refs/heads/main/pc/ice_server_parsing.cc)
- [RFC 8445 ICE priorities and nomination](https://www.rfc-editor.org/rfc/rfc8445.html)
- [W3C WebRTC statistics](https://www.w3.org/TR/webrtc-stats/)
