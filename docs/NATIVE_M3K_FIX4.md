# M3k-fix4 / 0.14.4 — opt-in signaling destination diagnostic

The 0.14.3 account report records two rejected WSS destinations, without a
guest/runtime crash. Neither a completed native ICE negotiation nor the exact
rejected origin can be established from that report. The normal audit deliberately
omits hostnames; the pinned core contains a configurable `ws_host` and constructs
the signaling destination at runtime. We must identify the requested destination
before changing the exact-origin connection policy.

`account-network-audit <parsecd.wasm> [report.json]` runs the same account flow and
connection policy as `account`, with explicit destination-origin reporting.
Each network intent then includes `destination_origin` (scheme, hostname and
non-default port only). Both modes also record `bridge` (`http` or `web-socket`),
so a WSS request dispatched through the wrong transport can be distinguished from
a rejected WebSocket destination.

The opt-in report can identify network infrastructure. Review it before sharing.
It never includes URL usernames, passwords, paths, fragments, query strings,
headers or message bodies. Normal `account` reports still omit destination
origins. Both modes retain the 64-intent bound. Opting into reporting does not
authorize additional destinations, follow redirects or change TLS validation.

This is a diagnostic build, not a confirmed signaling fix. Real-account testing
is needed to determine the exact rejected origin and the bridge used. STUN/TURN
integration and remote video/audio decoding remain separate unfinished work.

## Validation

New regression tests exercise opt-in redaction/bounding and a rejected destination
through the native WebSocket connection entry point. The latter verifies that no
socket is created and that the original rejection is preserved. Existing normal
audit privacy tests now also assert that destination origins are absent.
Windows CI runs formatting, unit tests, strict Clippy, release compilation and
the original-core/native bridge probes. Test results and the source commit are
recorded in the accompanying test package's BUILD-INFO.json after verification.

## User test

Run `START-NETWORK-DIAGNOSTIC.cmd`, log in and select Connect once. Close the
application normally, then inspect/share `account-network-report.json`. Preserve
that file before another run overwrites it. The ordinary `START-ACCOUNT.cmd`
continues to use the more private default report.
