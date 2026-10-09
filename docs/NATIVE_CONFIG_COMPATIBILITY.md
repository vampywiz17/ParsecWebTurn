# Native / Tauri STUN and TURN configuration contract

Standing requirement: future native STUN/TURN settings must remain compatible
with the Tauri client's `settings.json`. Native profile storage is a separate
DPAPI-protected virtual Parsec filesystem; it must not overwrite that file.

The baseline is `src-tauri/src/settings.rs` and `src-tauri/src/storage.rs` from
v0.7.0. JSON property names use camelCase:

- `provider`, `stunUrls`, `turnUrls`, `stunOnly`, `customUsername`;
- `encryptedCustomPassword`, `turnKeyId`, `encryptedApiToken`;
- `cacheCredentials`, `mediaDiagnostics`, `ttl`;
- optional legacy `customUrls`, migrated to separate STUN/TURN lists by the old
  client's existing rules, never by silently changing the connection policy.

Secrets use Base64-encoded Windows CurrentUser DPAPI blobs, without optional
entropy. Preserve that byte/encoding contract, unknown fields and existing
credentials when implementing a native reader/writer. Do not put secrets in
logs, reports or committed examples. The old client's defaults and URL/transport
validation remain the baseline. Do not silently enable forced relay.

The first native dev build preserves existing settings and browser profiles but
**does not yet read, migrate, generate or apply STUN/TURN credentials**. A later
implementation must add cross-client round-trip fixtures with synthetic values
before enabling writes. Browser authentication cookies are distinct from these
server settings and are not migrated into the native core.
