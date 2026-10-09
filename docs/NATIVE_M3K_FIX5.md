# M3k-fix5 / 0.14.5 — allow the observed Parsec v2 signaling origin

The user's opt-in 0.14.4 diagnostic identified two WebSocket requests to exactly
`wss://kessel-ws-v2.parsec.app`. Both used the native WebSocket bridge and were
rejected by the prototype's connection policy. Authentication and HTTPS API
requests were allowed, and the report contained no guest/runtime error. The
previous policy permitted only the older `wss://kessel-ws.parsec.app` signaling
origin. This establishes a local policy blocker independently of any subsequent
direct/relay connectivity issue. The private report is not stored in the repo.

The account policy now also permits the observed v2 WSS origin on port 443.
The audit classifies both exact hosts as `signaling`. The old origin remains
supported. There are no wildcard domains, automatic endpoint authorization,
redirects, plaintext fallback or changes to certificate validation. Both account
modes share the same updated policy; destination reporting is still opt-in.

The regression test checks both signaling origins, implicit/explicit port 443,
offline denial, plaintext and HTTPS schemes, other ports, lookalike hosts,
unrelated Parsec subdomains, URL credentials and fragments. The controlled guest
audit fixture now uses the v2 destination, verifies the `signaling` classification
and still rejects it while offline. No real accounts or hosts are used in tests.

The fix removes this specific rejection. A real account retest must establish
whether signaling and the subsequent connection stages work. STUN/TURN settings
and remote video/audio decoding remain separate unfinished prototype work.

Run `START-NETWORK-DIAGNOSTIC.cmd`, log in, select Connect once, close the app,
then preserve/share `account-network-report.json`. It includes destination
origins only, without URL paths, credentials or query tokens. The normal
`START-ACCOUNT.cmd` uses the same fix without origin reporting.
