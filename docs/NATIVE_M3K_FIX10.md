# M3k-fix10 / 0.14.10 — Cloudflare STUN test and retained transport evidence

The user's fix9 report confirms `parsec_padded_ufrag_compatibility: true` and
`remote_validation_error: null`, with no worker errors. The credential rejection
has been removed. It records two new-attempt/begin calls and twelve candidate
calls, but the final backend is status 20 with no retained attempt diagnostic.
Serialization omitted the active attempt; retry preparation discarded the prior
diagnostic. This report cannot establish the cause of the later 6200 error.
The user confirms VPN is active, has not verified STUN-only connectivity, and
requests Cloudflare STUN because other STUN services were inaccessible here.

## Opt-in STUN

Account modes accept `--cloudflare-stun` after the explicit report path. The
standard public native `RTCConfiguration.ice_servers` API receives only
`stun:stun.cloudflare.com:3478`, documented in
[Cloudflare's service endpoints](https://developers.cloudflare.com/realtime/turn/).
There is no TURN URL, password, automatic fallback or relay-only policy. ICE
transport policy remains `all`. Normal account invocation and all automated
fixtures retain zero configured ICE servers unless this flag is explicitly set.
No public STUN requests are made by tests. HTTPS/WSS allowlists are unchanged;
STUN is performed by the native ICE engine over UDP, not the HTTP bridge.

`START-CLOUDFLARE-STUN-DIAGNOSTIC.cmd` explicitly selects this mode. The report
includes the configuration flag and fixed provider label, plus candidate counts.
Configuration alone does not prove a STUN response or successful direct path.
`local_srflx_candidates` counts successful native server-reflexive candidate
callbacks separately from `local_host_candidates`; zero is not itself a network
failure verdict. ICE transport policy is explicitly set to `all` rather than
relying on the library's `unspecified` configuration default.

## Reporting repair

The account report uses `Backend::diagnostic`, adding a read-only
`active_attempt_diagnostic` without consuming/cancelling the attempt or pumping
events. `previous_attempt_diagnostics` retains at most four prior snapshots,
fixed failure categories and redacted credential shapes when retrying. An omitted
counter records eviction. No attempt IDs, account names or endpoint addresses
are added; credential strings, hashes and raw SDP remain absent.

Snapshots record installed local/remote descriptions, the bridge sync marker,
candidate/channel counts and public native ICE gathering/connection, signaling,
peer and DTLS state enums. Getters are sampled every 250 ms while awaiting
commands and immediately before closure. DTLS event handlers owned by webrtc-rs
are not replaced. Fixed native failure stages are propagated instead of the
generic `worker` label. Missing state remains unknown, not routing proof.

These use the public webrtc-rs 0.14
[`RTCPeerConnection`](https://github.com/webrtc-rs/webrtc/blob/v0.14.0/webrtc/src/peer_connection/mod.rs)
and [`RTCDtlsTransport::state`](https://github.com/webrtc-rs/webrtc/blob/v0.14.0/webrtc/src/dtls_transport/mod.rs)
APIs. Native library APIs are distinguished from web standards; no browser
internals are needed. The isolated padded-ufrag compatibility rule is unchanged.

The snapshot explicitly reports UDP/IPv4 and the existing 30-second worker
deadline. This patch does not extend that deadline. It currently also bounds an
established worker: a known prototype lifetime limitation to revisit before
persistent streaming, not a demonstrated cause of this user's failure.

## Verification and retest

New tests cover bounded history/ID redaction, exact CLI opt-in and STUN-only
configuration without credentials or relay policy. The actual offline native/
WASM session probe checks live SDP/sync phases and connected DTLS without
consuming the attempt. CI requires this proof alongside padded-ufrag negotiation.

Extract into a new folder, run `START-CLOUDFLARE-STUN-DIAGNOSTIC.cmd`, log in and
press Connect once. After the error or result, close normally without retrying
and share `account-network-report.json`. The STUN configuration is implemented;
real host connectivity, Cloudflare reachability here and decoding remain unverified.
