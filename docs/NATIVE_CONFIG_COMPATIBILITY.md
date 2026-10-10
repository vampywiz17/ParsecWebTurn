# Native / Tauri STUN and TURN configuration contract

Standing requirement: future native STUN/TURN settings must remain compatible
with the Tauri client's `settings.json`. Native profile storage is a separate
DPAPI-protected virtual Parsec filesystem; it must not overwrite that file.

The old implementation is retained in the immutable v0.7.0 history, not in the
native source tree:

- [Settings and migration](https://github.com/vampywiz17/ParsecWebTurn/blob/7aa3362f9b3166fa4d6ce5a5728fe0b137563b57/src-tauri/src/settings.rs).
- [DPAPI storage](https://github.com/vampywiz17/ParsecWebTurn/blob/7aa3362f9b3166fa4d6ce5a5728fe0b137563b57/src-tauri/src/storage.rs).
- [Legacy ICE URL validation](https://github.com/vampywiz17/ParsecWebTurn/blob/7aa3362f9b3166fa4d6ce5a5728fe0b137563b57/src-tauri/src/ice.rs).

JSON property names use camelCase:

- `provider`, `stunUrls`, `turnUrls`, `stunOnly`, `customUsername`;
- `encryptedCustomPassword`, `turnKeyId`, `encryptedApiToken`;
- `cacheCredentials`, `mediaDiagnostics`, `ttl`;
- optional legacy `customUrls`, migrated to separate STUN/TURN lists by the old
  client's existing rules, never by silently changing the connection policy.

### Defaults and migration

The v0.7.0 defaults are `provider: "cloudflare"`, empty URL arrays and credential
strings, `stunOnly: false`, `cacheCredentials: false`, `mediaDiagnostics: false`,
and `ttl: 86400`. An empty provider or a zero TTL is normalized to those defaults
on load. Supported provider names are `cloudflare` and `custom`; the old
Cloudflare credential lifetime validator accepts 60 through 172800 seconds.

Migrate `customUrls` only when **both** `stunUrls` and `turnUrls` are empty.
Trim each old URL; a case-insensitive `stun` prefix goes to `stunUrls`, and other
entries go to `turnUrls`. Existing split lists take precedence. The old writer
clears `customUrls` after migration and omits it when empty.

`stunOnly` excludes TURN URLs and does not require a TURN secret. In Cloudflare
STUN-only mode, an empty STUN list uses `stun:stun.cloudflare.com:3478`. For custom
servers, STUN entries use no credentials and TURN entries use `customUsername`
and the decrypted custom password. Preserve these meanings when adding native
settings. New native installations use `provider: "custom"` with empty lists,
meaning Parsec's default STUN server and no TURN relay. Existing files retain
their legacy provider/default semantics.

### Credential encoding and future writes

Secrets use Base64-encoded Windows CurrentUser DPAPI blobs, without optional
entropy. Preserve that byte/encoding contract, unknown fields and existing
credentials when implementing a native reader/writer. Do not put secrets in
logs, reports or committed examples. The old client's defaults and URL/transport
validation remain the baseline. Do not silently enable forced relay.

The native dev client reads and atomically writes `settings.json` beside its EXE,
or in `--data-dir`. Empty password/token edits retain saved secrets; explicit
forget controls remove them. Invalid files are preserved until the user saves a
valid replacement. Unknown JSON properties survive editing. Tests exercise the
legacy migration and actual Base64/CurrentUser-DPAPI encoding with synthetic data.
Browser authentication cookies are distinct from these server settings and are
not migrated into the native core.

### Native transport behavior

The normal native entry point resolves settings once per attempt, supplies
`RTCIceServer` values directly to webrtc-rs and keeps `iceTransportPolicy: all`.
No delayed TURN insertion, automatic ICE restart or private Parsec reconnect
hook is used. STUN-only excludes TURN and bypasses credential decryption/API
requests. Cloudflare generation uses HTTPS, bounded responses/timeouts and no
redirects. `cacheCredentials` caches generated credentials in memory until their
refresh margin; the old disk cache is not read or modified.

Supported relay URLs: `turn:` (UDP by default), `turn:...?transport=tcp`, and
`turns:` (TLS over TCP by default). TLS checks hostname and certificate chain
against public and OS-trusted roots; there is no certificate bypass. UDP relay
allocations are used toward the peer for all three server transports. IPv4 relay
allocation follows the current upstream ICE implementation. `stuns:` discovery,
HTTP proxy tunneling and RFC 6062 TCP allocations are not supported.

See the [isolated transport patch](../src-native/vendor/webrtc-ice-0.14.0/LOCAL-CHANGES.md)
and [TURN integration tests](../tests/turn-integration/README.md).
