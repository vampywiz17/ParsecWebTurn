# M3k-fix7 / 0.14.7 — compact credential compatibility and precise validation diagnostics

The user's 0.14.6 report confirms that the app remained open, with no failed WASM
worker. It records `attempt_failure: remote-begin` before any local/remote candidate
submission or native transport connection. That category can mean rejected ICE/
DTLS credentials or an attempt identifier mismatch; it does not establish a
direct-route/TURN failure. The private report is not stored in the repository.

The pinned public `parsec.js` function `ia()` splits an SDP offer on LF and keeps
the preceding CR in the compact `ice_ufrag`, `ice_pwd` and fingerprint strings.
The previous strict validator rejects these terminal line-ending characters.
This is a verified representation incompatibility, but the existing user report
does not prove that it caused their remote-begin rejection.

At the private ABI boundary only, the adapter now removes at most one terminal
CR from each compact credential field and canonicalizes a SHA-256 algorithm name
to lowercase. It does not trim whitespace, embedded CR/LF, repeated CR, change
the credential itself, fabricate missing fields or accept invalid/unsupported
fingerprints. Normalized credentials still pass the existing strict checks.
The bounded reads allow the maximum 256-byte ICE token plus the optional CR and
NUL terminator. The native SDP/WebRTC engine continues to receive canonical data.

[RFC 8839 section 5.4](https://www.rfc-editor.org/rfc/rfc8839.html#section-5.4)
defines the ICE token lengths and alphabet;
[RFC 8122 section 5](https://www.rfc-editor.org/rfc/rfc8122.html#section-5)
defines SDP fingerprint syntax. Other fingerprint algorithms remain explicitly
unsupported by this prototype rather than being relabeled SHA-256.

The report now retains `remote_begin_diagnostic`: raw and normalized lengths,
terminal-CR flags, ICE-token validity, a fixed credential-validation error enum,
and an attempt-ID equality boolean. No credential strings, hashes, SDP, actual
IDs or underlying error messages are serialized. An attempt-ID mismatch now has
its own fixed failure category. The diagnostic resets at the next attempt.

Two new tests cover normalization/boundaries and injection/privacy rejection.
The existing controlled WASM/native transport probe now submits LF-split-style
compact credentials with terminal CR and uppercase SHA-256 through the actual
begin import, verifies normalization metadata, then requires the existing local
ICE/DTLS/SCTP connection and binary exchange. Control/buffer probes retain their
canonical credential inputs. No real credentials or external servers are used.

Extract the new package and run `START-NETWORK-DIAGNOSTIC.cmd`. Log in, select
Connect once, close the app normally, then preserve/share `account-network-report.json`.
This may fix the observed rejection if it was caused by representation differences;
otherwise the new fields identify the exact validation failure. A real host
connection is still unverified. STUN/TURN and media decoding remain separate work.

## Verified build

Source commit: `e5648bd2dfd2d149ff787661fcc5f474b0b54726`.
[Windows CI run 37905059016](https://github.com/vampywiz17/ParsecWebTurn/actions/runs/37905059016)
passed formatting, strict Clippy, all 73 tests, the optimized release build and
the bridge probes. The compact-credential probe connected two local native peers,
verified six binary messages and closed both peers cleanly; it did not connect
to a real Parsec host.

The downloaded release executable also passed the offline original-core login
fixture locally: nine synthetic input steps, including Tab and paste, 15 GPU
frames on AMD Radeon 780M, and clean native-window release. There was no startup
error or rejected thread spawn. Networking remained disabled, and the serialized
fixture report contained neither fixture credentials nor destination origins.
Real-account acceptance of this fix remains pending the next user test.
