# M3j: interactive native account window

Prototype 0.13.0 adds an explicit `account` mode. It runs the original pinned
Parsec WASM UI on Win32/OpenGL until the user closes the window. Rust implements
the audited HTTP/WebSocket imports; the original core constructs authentication
requests and consumes responses. There is no browser, JavaScript runtime,
replacement login protocol, credential command-line argument or session format.

```powershell
./parsec-native-wasm.exe account ./parsecd.wasm ./account-report.json
```

Type credentials locally into the original UI. Do not send credentials or
tokens with diagnostic reports. This milestone enables an account test; it
does not prove successful real-account authentication, MFA, host listing,
remote host compatibility or decoded video. Live account acceptance requires
a separate user-run test. All existing diagnostic modes remain offline.

## Network permission and privacy

Only account mode enables exact origins: `https://kessel-api.parsec.app`,
`https://public.parsec.app`, `https://parsecusercontent.com`, and
`wss://kessel-ws.parsec.app`, on their normal TLS port. The HTTPS authentication
origin was observed in the M3i original-core offline test. The signaling origin
is a pinned-snapshot expectation, not a real-account verification. Unknown or
regional origins are denied; there is no suffix matching, redirect following,
TLS downgrade, disabled certificate verification or proxy interception.
Public TLS trust uses the libraries' default WebPKI roots. Private fixture
certificates remain scoped to controlled loopback diagnostics only.

The shared observer keeps bounded enum metadata, byte counts and permission
results, never request/response content or URL/query/header/token values.
Account and session-audit reports omit raw guest errors, window titles and
filesystem paths. Guest stdout/stderr retain only acknowledged byte counts.
These modes accept no screenshot argument. Guest files and session data live
in the bounded, in-memory WASI filesystem and are not persisted to disk.

## Lifetime and shutdown

All guest instances share one stop signal and one 100 ms epoch ticker. The
documented Wasmtime epoch callback renews the execution budget while running
and traps executing guest code after cancellation. Frame callbacks retain
their existing bounded fuel budgets. This is a cooperative lifetime, not a
promise to interrupt blocking guest atomic waits.

Closing the window signals cancellation, refuses new HTTP/WebSocket work,
cancels and joins WebSocket actors, and releases the WGL context before
destroying its HWND. In-flight blocking HTTP has its existing five-second
timeout. A ten-second shutdown-only process deadline bounds guest atomic waits;
it never limits the interactive account lifetime. The report distinguishes
shutdown requested and actual native-window release. It does not claim that
every guest thread joined or that a server session was revoked.

`session-audit` exercises the same persistent UI/epoch lifecycle offline and
requests shutdown after 35 seconds, beyond the old window/process limits.
It submits no login and enables no external destination:

```powershell
./parsec-native-wasm.exe session-audit ./parsecd.wasm ./session-audit.json
```

Tests cover cancellation before/after wait registration, live epoch continuation
and cancellation of guest execution, HTTP rejection before client creation
after shutdown, and actual local WebSocket close/join with reconnect refusal.
Existing native transport, HTTP/WSS/TLS and original-core diagnostics remain
required. Local original-core UI validation and CI results are recorded below
once run. No real credentials are used by automated tests.

The Wasmtime, Win32, HTTP/TLS and WebSocket APIs are supported interfaces.
The Parsec WASM import ABI and endpoint expectations remain private,
version-dependent details isolated to the pinned adapter.

References:
- https://github.com/bytecodealliance/wasmtime/blob/v38.0.4/crates/wasmtime/src/runtime/store.rs
- https://docs.rs/reqwest/0.12.28/reqwest/blocking/struct.ClientBuilder.html
- https://docs.rs/tokio-tungstenite/0.30.0/tokio_tungstenite/fn.connect_async_tls_with_config.html
- https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-destroywindow
