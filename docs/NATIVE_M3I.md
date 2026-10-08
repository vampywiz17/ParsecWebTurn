# M3i: original UI request boundary and private offline audit

Prototype 0.12.0 removes the separate window-only `MTY_HttpRequest` failure stub.
Original window/worker instances now use the common native HTTP implementation,
including zeroed failure outputs and the destination policy. Their policy is
still offline. No account requests or authentication data leave this process.

## Observe intent before enabling account traffic

The pinned original core must perform its own login. Its token storage and
private account protocol are not reproduced or guessed. Before enabling those
requests, this stage observes the actual native boundary across all guest
instances, without retaining request values. This is an audit of attempted
requests, not server responses, successful login or proof of endpoint support.

HTTP and WebSocket services share one observer in ThreadRuntime. It records at
most 64 intents and a saturating omitted count. An intent contains only:

- An enum for HTTP/HTTPS/WS/WSS/other and a fixed method enum.
- A fixed service category (API, signaling, public/image assets, loopback, other).
- A fixed route category (authentication, sessions, elevation, SAML, static data,
  other), selected from the pinned snapshot's strings.
- Authorization-header/query presence, body byte count and policy permission.

No URL, host, port, path, query value, custom method, headers, body, token, user
name or password is copied into the observer. Unknown names become `other`.
Categorization is descriptive snapshot metadata, not a documented Parsec API or
network permission. A request still needs the separate exact-origin policy.
Invalid requests rejected before URL/request parsing may have no audit entry.

Raw guest stdout/stderr contents are no longer retained in **any** mode: WASI
fd_write still acknowledges valid writes and reports a saturating byte count.
Existing import/trap diagnostics remain available in normal diagnostic modes.
The dedicated account-flow audit additionally removes window titles, guest
filesystem request paths and raw main/worker exception details from its report.
It accepts no screenshot argument. Its result schema is 3 (stdout text was
replaced with stdout_bytes and network_audit was added).

## Commands and limits

```powershell
./parsec-native-wasm.exe guest-audit-probe ./guest-audit-local.json
./parsec-native-wasm.exe window-audit ./parsecd.wasm ./window-audit-local.json
./parsec-native-wasm.exe login-audit ./parsecd.wasm ./login-audit-local.json
```

The first command is a controlled WASM test, not the original core. Two instances
share memory/services/observer, call the actual HTTP and WebSocket imports with
synthetic secret sentinels, and prove offline zeroed failures. A WASI stdout call
verifies acknowledged byte counts and absence of its contents from serialization.
A separate test fills the observer and proves its limit and omission count.
Windows tests assert the UI cannot intercept HTTP outside the common host bridge.

The second command is the original pinned WASM UI on native Win32/OpenGL, with
the existing bounded eight-second event loop and process deadline. Inspect
network_audit for the attempted requests. An empty list proves only that no
parsed request was observed in that bounded run. It does not prove the core has
no login flow. This is not a usable login window: do not enter real credentials.
There is no real-account login automation or synthetic server response fed to
the core.

The third command is an isolated **offline synthetic input** test of the pinned
UI layout. It uses the existing native input bridge to enter a fixed fictitious
email/password and press Log In. Its eight input stages wait for actual UI
presentations; the layout coordinates are a snapshot-specific diagnostic fixture,
not a production login contract. It does not accept account arguments or enable
network. The event loop is bounded to 20 seconds, watchdog to 24 and process
deadline to 30. It accepts no screenshot argument. synthetic_login_steps reports
only the injected stage count, not successful form submission or authentication;
the observed network intent is the separate evidence of a request attempt.

authentication_integrated, external traffic, host connection and decoded video
remain absent. Next: use observed original-core requests to define the minimal
online account mode, improve native input/lifecycle where needed, and let the
core manage its own login/session. Real-host transport and hardware decode follow.

No browser private interface, JavaScript or WebView2 is introduced. The native
HTTP/WebSocket APIs and WASI ABI are unchanged; only the pinned adapter's
observability/reporting is extended. Production main/dev are unchanged.

## Verified build and original-core observation

Source `d850a2062311789acac45f068fdc7cc8020f33cd`, prototype 0.12.0:
[Windows CI 37852602841](https://github.com/vampywiz17/ParsecWebTurn/actions/runs/37852602841)
passed formatting, all 45 tests, Clippy with warnings denied, release build,
original-core inspect/allocator/bootstrap and all native acceptance probes.

The downloaded diagnostic and release executables both ran the original
`login-audit` locally. All eight synthetic input stages completed and the
**original core itself** constructed a HTTPS POST classified as API/authentication:
133 body bytes, no Authorization header or query, policy_allowed=false. No
request left the process. The final release rendered its own login UI on AMD
Radeon 780M: two shaders, 74 draw calls, 14 presentations, no main execution
error. These are UI counters, not remote-video FPS or decoder performance.

The final release also passed the separate two-instance import/stdio audit
probe with all four verification flags true. Its original-core login report
and controlled-probe report are packaged separately. An earlier idle UI audit
observed no intents; that idle result alone did not reveal the login behavior.

The unchanged core is `150-104a`, SHA-256
`d663dd96df477c65479fb93eb88756c7fcafff581cc93be563625cd195a4b4a6`.
BUILD-INFO.json and SHA256SUMS identify source, documentation and files.
This proves the native input-to-original-login-request boundary **offline**.
It does not prove server acceptance, authentication, MFA, session persistence,
host connection or video decoding. Main/dev were not changed.
