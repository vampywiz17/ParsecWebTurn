# M3f: native HTTP guest import

The isolated Rust host implements the pinned `MTY_HttpRequest` import using
reqwest's documented blocking API. No JavaScript/browser is added. This is
the HTTP foundation for later native authentication, not completed login or
WebSocket signaling. The production Tauri app and original WASM are unchanged.

## ABI and ownership

The pinned `matoya-worker.js::MTY_HttpRequest` takes ten i32 parameters: URL,
method, newline-separated headers, body pointer/size, proxy, timeout,
response pointer output, response size output and u16 status output.
The import returns a boolean. Responses including HTTP 4xx/5xx return true;
transport/policy/limit failures return false with zeroed outputs. Empty
responses allocate nothing. Nonempty responses use `mty_system_alloc(size+1,1)`,
preserve binary bytes and append a NUL outside the reported response length.
Successful allocations belong to the guest; invalid allocation ranges are
freed through `mty_system_free` before returning failure. Invalid or overlapping
output ranges trap before network access or partial output writes.

Unlike the audited JS header splitter, parsing splits only the first colon,
preserving values such as authorization parameters. Reqwest validates header
names/values; host/content-length/transfer-encoding/connection framing headers
are reserved for the transport. Headers are bounded to 16 KiB/64 entries,
request and response bodies to 1 MiB, URL to 8191 bytes and concurrent calls
to eight. Oversized responses fail both with a declared length and while
reading an unknown-length stream. Request timeouts use milliseconds, default
to five seconds and are capped at five seconds, including body consumption.
The browser ignored the guest timeout; this prototype enforces it.

No proxy, cookie jar or redirects are enabled. Explicit nonempty proxy input
is rejected rather than silently ignored. Request URL, headers, body and native
error details are not logged or serialized. Certificate verification is not
weakened; HTTPS behavior itself is not exercised by this loopback milestone.

## Network policy and acceptance diagnostic

All normal original-core modes remain offline. Only `guest-http-probe` enables
HTTP to its exact numeric `127.0.0.1` address and ephemeral port. DNS names,
other ports, external origins, URL userinfo and fragments are rejected. A
302 response is returned to the guest without following its external Location.
The network service is shared by host worker instances, but the acceptance
fixture invokes the import on a single controlled WASM instance.

```powershell
./parsec-native-wasm.exe guest-http-probe ./guest-http.json
```

Nine loopback requests verify a UTF-8 POST/header/binary response, empty 204,
401 status/body, external 302 redirect refusal, declared and streamed response
limits, a delayed-body timeout, null allocation and invalid allocation cleanup.
Additional import calls verify offline rejection, output bounds/overlap,
framing-header refusal and oversized guest bodies without issuing requests.
The fixture explicitly frees successful response allocations, and the server
is stopped/joined on both success and failure. The probe uses fixture-only
authorization text, never real credentials or an external endpoint.

`authentication_integrated`, `original_parsec_guest_http_exercised`,
`websocket_signaling_integrated`, `external_requests_enabled`,
`parsec_host_connected` and `video_decoded` remain false. Original guest
allocator/bootstrap probes run separately; the controlled HTTP allocator
does not prove real-core authenticated HTTP interoperability.

Next: bounded native WebSocket signaling and authenticated original-core
HTTP policy/lifecycle, followed by real host interoperability. Hardware
decoder/GPU presentation remains the main target after session transport.

References: retained 2026-10-08 Matoya worker/main HTTP functions and actual
pinned-core import signatures; [reqwest blocking client documentation](https://docs.rs/reqwest/latest/reqwest/blocking/struct.ClientBuilder.html),
[redirect policy](https://docs.rs/reqwest/latest/reqwest/redirect/struct.Policy.html).
The Parsec ABI is snapshot-specific compatibility code, not a public standard.
