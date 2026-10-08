# M3h: verified native HTTPS/WSS boundary

Prototype 0.11.0 adds TLS to the native HTTP/WebSocket guest bridges with
documented reqwest, tokio-tungstenite and rustls APIs. Certificate trust and
server-name checks remain enabled. The pinned original WASM is unchanged.

## Why this step precedes account integration

The pinned WASM does **not** import weblib's `bin_user_bin_get/set/delete` cookie
hooks. Adding them would not implement its account lifecycle. Its native
bootstrap uses the ephemeral WASI filesystem instead. The binary contains
`kessel-api.parsec.app`, `/v2/auth`, `/auth/sessions` and a `kessel-ws.` prefix;
these are snapshot observations, not a public authentication API contract.
We do not fabricate endpoint responses, persist a guessed `user.bin` format,
extract browser credentials or claim original-core authenticated signaling.

The next integration must let the original core perform its own login and
session lifecycle and first identify the actual requests it makes. That also
requires reviewing guest stdout/error reporting for credentials before enabling
external authenticated traffic. No real-account mode is enabled by this step.

## Exact destination policy

HTTP and WebSocket bridges share one policy implementation. Default means no
network. A secure policy accepts exact HTTPS/WSS origins only: scheme, normalized
host and effective port must match. There is no domain suffix/wildcard matching,
URL userinfo, fragment, downgrade or automatic redirect. Paths and queries may
vary within an explicitly permitted origin. Policy origins themselves must have
no path/query. Cleartext remains limited to the existing exact loopback fixture.

Policy tests use example Parsec origins to verify matching, not to assert that
the current account/signaling endpoints are supported or reachable. Original-core
bootstrap/window modes remain offline, including the window's explicit HTTP
failure behavior. The secure policy is currently enabled only on fixture ports.

## Acceptance diagnostic

```powershell
./parsec-native-wasm.exe guest-tls-probe ./guest-tls-local.json
```

An in-memory, newly generated certificate and private key serve two local TLS
listeners. Nothing is installed in the OS trust store or written to disk. The
first listener has the correct IP SAN, the second deliberately has a different
name. The controlled WASM calls the actual HTTP and WebSocket imports.

- HTTPS sends synthetic request credentials and verifies the 200 JSON response.
- WSS upgrades with 101 and exchanges the synthetic session text, then destroys
  the socket, zeros its handle and checks that no handles remain.
- Both bridges reject an untrusted certificate.
- Both bridges reject a trusted certificate whose SAN does not match the IP.
- Failed requests leave zeroed guest outputs and no socket handles.
- Both servers are joined; six TLS attempts, four certificate rejections.

Private trust is limited to the exact fixture client/port. The negative WSS
client has an empty root store; the negative HTTP client uses normal public
roots, which do not trust the generated fixture certificate. There is no
`danger_accept_invalid_certs`, custom permissive verifier, global trust-store
modification or certificate-verification fallback. Existing time/message/queue
bounds apply. No URL/header/body/token or TLS error detail is serialized.

`authentication_integrated`, `original_parsec_guest_auth_exercised`,
`external_requests_enabled`, `parsec_host_connected` and `video_decoded` remain
false. The synthetic exchange proves encrypted transport through guest imports,
not real Parsec login, MFA, session revocation or production WSS interoperability.

References: [tokio-tungstenite TLS connector](https://docs.rs/tokio-tungstenite/latest/tokio_tungstenite/fn.connect_async_tls_with_config.html),
[rustls client configuration](https://docs.rs/rustls/latest/rustls/struct.ClientConfig.html),
[reqwest blocking client](https://docs.rs/reqwest/0.12.28/reqwest/blocking/struct.ClientBuilder.html),
[rcgen fixture certificates](https://docs.rs/rcgen/0.13.2/rcgen/fn.generate_simple_self_signed.html).

## Verified build

Source `2c9ba23c8b42f0cb1eeb2d779a46819cd4511133`, prototype 0.11.0:
[Windows CI 37850043589](https://github.com/vampywiz17/ParsecWebTurn/actions/runs/37850043589)
passed formatting, all 42 tests, Clippy with warnings denied, the optimized build,
original WASM inspect/allocator/bootstrap and all native acceptance probes.

Both the downloaded diagnostic executable and the final release executable
passed the TLS probe locally: six attempts, four certificate rejections, all
eight boolean verification checks and both servers joined. The release report
is included separately from the CI report in the package.

The core remains `150-104a`, SHA-256
`d663dd96df477c65479fb93eb88756c7fcafff581cc93be563625cd195a4b4a6`.
BUILD-INFO.json and SHA256SUMS identify source/documentation commits and files.
This evidence is limited to controlled guest imports and synthetic loopback
TLS, not the original core's authenticated account/signaling lifecycle.
Production main/dev are unchanged.
