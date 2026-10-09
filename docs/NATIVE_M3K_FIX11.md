# M3k-fix11 / 0.14.11 — capture the actual transport failure

The user's fix10 report records a Cloudflare STUN srflx candidate, installed
local/remote SDP, six remote candidates and ICE `connected`. The last periodic
sample is DTLS `connecting`, with no open data channels. Both retained attempts
fail under the generic `worker` category. This narrows the observed boundary;
it does not yet establish a cipher, certificate or network failure.

## Public native state capture

The peer failure callback now samples public native peer/ICE/DTLS state getters
before publishing failure. It retains a separate `transport_states_at_failure`
snapshot and reports `dtls-transport`, `ice-transport` or `peer-transport` from
the current enums. Periodic sampling and teardown cannot overwrite that snapshot.
A weak peer reference avoids an ownership cycle. Internal DTLS callbacks remain
owned by webrtc-rs. No cryptographic checks, SDP roles or network policy change.

## Optional pinned-library diagnostics

Only `account-network-audit` installs a Rust `log` consumer. The report adds
`native_transport_diagnostics`, explicitly marked experimental and process-scoped.
This supplements public state evidence; events are not attributed to an individual
attempt and never drive connection behavior. Default `account` does not enable it.

The consumer accepts only warning/error records from webrtc-rs 0.14.0's transport
startup module. It formats at most 512 transient bytes, discards oversized records
and stores at most 16 fixed stage/reason categories plus an omitted counter.
Exact known public error Display values map to static reasons, such as certificate
fingerprint mismatch, missing SRTP profile or no shared cipher. Unrecognized errors
become `unknown`; raw messages, endpoints, SDP, certificates and keys are never
printed or retained. Changed/missing log output remains unknown. A logger conflict
is reported and cannot prevent startup. The main connection needs none of this.

Sources: public [state APIs](https://github.com/webrtc-rs/webrtc/blob/v0.14.0/webrtc/src/peer_connection/mod.rs),
[transport startup diagnostics](https://github.com/webrtc-rs/webrtc/blob/v0.14.0/webrtc/src/peer_connection/peer_connection_internal.rs),
[DTLS transport validation](https://github.com/webrtc-rs/webrtc/blob/v0.14.0/webrtc/src/dtls_transport/mod.rs)
and [Rust logging facade](https://docs.rs/log/0.4.34/log/).

## Verification and scope

A negative controlled WASM/native session advertises a synthetic wrong fingerprint
while preserving valid ICE credentials. It must reject DTLS, open no channels,
retain fresh failed DTLS/peer states and close both peers. Tests also check category
redaction, ignored targets, bounded collection and oversized-message rejection.
The existing successful native session and bridge probes remain required.

This is a diagnostic improvement, not a verified fix for the real host's 6200.
The pinned original WASM, Cloudflare STUN flag and existing 30-second worker
lifetime remain unchanged. Real host connectivity and video decoding remain unverified.

Extract the new test ZIP into a separate folder. Run
`START-CLOUDFLARE-STUN-DIAGNOSTIC.cmd`, select Connect once, wait for the result,
close normally and share `account-network-report.json`.
