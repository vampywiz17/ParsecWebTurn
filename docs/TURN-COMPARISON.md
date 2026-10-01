# Provider-dependent ICE selection investigation

Date: 2026-10-01. This records an investigation, not a confirmed routing fix.

## Observations

The user reports the same host and VPN connecting directly with STUN-only or
ExpressTURN configuration, but carrying stream traffic through Cloudflare TURN
with the Cloudflare configuration. ExpressTURN has a separate STUN URL and a
`turn:` URL with neither an explicit transport nor a `turns:` URL. The exact
ExpressTURN hostnames and ports have not been supplied.

The user subsequently tested Cloudflare STUN with ExpressTURN TURN on the same
VPN and again obtained a direct connection. This weakens the hypothesis that
the Cloudflare STUN endpoint alone causes the difference. The remaining
comparison is the TURN provider/URL set, including its offered transports and
the resulting candidate/check timing. It does not yet isolate Cloudflare UDP
TURN from its additional TCP/TLS URLs.

The user then reported manually configuring only the Cloudflare TURN 3478
endpoint and obtaining relay even with the VPN. This means the full multiport
TCP/TLS URL list is not necessary to reproduce the behavior. Confirm the exact
URI/transport when collecting a diagnostic report. Also verify that the current
ExpressTURN credentials actually yield a working relay path when direct access
is unavailable; otherwise its apparent direct preference could be absence of a
usable relay alternative rather than different selection among working paths.

The user confirmed that the current ExpressTURN configuration relays successfully
without the VPN. Both providers therefore offer working relay service; the
working-direct/selected-relay difference remains provider-dependent.

For each connected session, open the app's developer console and run:

```js
console.log(JSON.stringify(await window.parsecWebTurnIceDiagnostics(), null, 2));
```

This explicitly requested snapshot uses getConfiguration/getStats only. It
includes the client policy, ICE role, pair selection/check state, candidate
type/priority/transport and check/traffic counters. It excludes endpoint
addresses, URLs, credentials, SDP and certificates. At most eight connections
and 64 pairs per connection are returned, with selected pairs first. Missing
fields remain null. A succeeded direct pair with relay selected is different
evidence from a failed direct pair; a single snapshot cannot reconstruct all
candidate arrival times or prove why nomination occurred.

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
