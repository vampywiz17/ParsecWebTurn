# ParsecWebTurn

Portable, no-install launcher for **Parsec Web** using **Microsoft Edge** and either Cloudflare Realtime or your own WebRTC STUN/TURN servers, including coturn and eturnal.

## What it does

- launches `https://web.parsec.app/` as an Edge app window
- requires **no administrator rights**
- installs no VPN, driver, .NET runtime, or WebView2 runtime
- uses the Microsoft Edge already present on Windows
- creates a separate local Edge profile beside the application
- injects the ICE override at `document_start`, before the Parsec page scripts run
- requests short-lived TURN credentials directly from Cloudflare, with optional encrypted caching
- stores the long-lived TURN API token locally using **Windows DPAPI / Current User**
- supports a local `ice.json` fallback
- supports custom STUN/TURN URLs with Windows-protected passwords and no Cloudflare dependency
- uses the Parsec icon for the Windows executable

## First run

1. Download and extract the release ZIP to any writable directory.
2. Run `ParsecWebTurn.exe`.
3. Choose **Cloudflare Realtime** and enter:
   - **Cloudflare TURN Key ID**
   - **Cloudflare TURN Key API Token**
   - credential TTL in seconds (default 86400 / 24 hours, maximum 172800 / 48 hours)
4. Click **Save & launch Parsec**.

Alternatively choose **Custom server / coturn, eturnal** and configure your own server as described below.

ParsecWebTurn stores the TURN Key ID and TTL in `settings.json`. The API token is encrypted with Windows DPAPI and can only be decrypted in the same Windows user context.

By default, every start calls Cloudflare's TURN credential endpoint and obtains a new short-lived `username` / `credential` pair. Existing settings remain compatible.

Enable **Reuse valid TURN credentials for faster startup** in Settings to avoid unnecessary API calls. The cache in `.turn-cache.json` is encrypted with DPAPI for the current Windows user. It is reused only when the Key ID, API token and TTL match, the local clock has not moved behind its issue time, and more than a quarter of the requested lifetime remains (at least five minutes). Expired, corrupt or incompatible caches trigger a fresh request. A cache write failure does not prevent a valid session from starting. A cached credential can have less remaining lifetime than the configured TTL; disable caching if your next session needs the full lifetime.

The launcher does not refresh credentials in an already open browser. Set the TTL longer than your expected session, then close the dedicated Parsec Edge windows and launch again when you need fresh credentials. Launching while that profile is active shows a clear message instead of silently reusing old page configuration. If Edge keeps the profile active after closing its windows, disable Startup boost and background extensions in that dedicated profile.

## Cloudflare setup

Create a Cloudflare Realtime TURN key and use its **Key ID** and **API token** in the first-run settings dialog.

The application calls:

```text
POST https://rtc.live.cloudflare.com/v1/turn/keys/<TURN_KEY_ID>/credentials/generate-ice-servers
Authorization: Bearer <TURN_KEY_API_TOKEN>
Content-Type: application/json
```

with a body such as:

```json
{"ttl":86400}
```

The returned `iceServers` list is injected into Parsec Web.

## Custom TURN servers (coturn, eturnal and compatible services)

Choose **Custom server / coturn, eturnal** in Settings. Enter one STUN/TURN URL per line, for example:

```text
stun:turn.example.com:3478
turn:turn.example.com:3478?transport=udp
turn:turn.example.com:3478?transport=tcp
turns:turn.example.com:5349?transport=tcp
```

Enter the TURN **username** and **password / credential** accepted by your server. All TURN URLs in this form share the same credentials. STUN-only setups can leave both fields empty. Custom mode makes no Cloudflare API calls, requires no Cloudflare key and does not apply Cloudflare credential caching or TTL settings.

The password is stored in `encryptedCustomPassword` using Windows DPAPI; server URLs and username are stored in `settings.json`. Existing Cloudflare settings migrate automatically, and switching providers preserves both sets of configuration.

For **coturn**, use a configured long-term user account (`lt-cred-mech` / `user`) or a temporary username/password generated using your server's TURN REST authentication setup. For **eturnal**, use its configured static `credentials` or a temporary pair from your credential service / `eturnalctl credentials`. A `static-auth-secret` / shared `secret` is a server secret, not a password you can paste into this client. The app does not generate or refresh custom REST credentials; supply a pair valid for your session.

See the official [coturn configuration reference](https://github.com/coturn/coturn/blob/master/examples/etc/turnserver.conf) and [eturnal documentation](https://eturnal.net/doc/). TURN over TLS requires a certificate trusted by Edge; server routing, firewall and relay-port configuration remain server-side responsibilities.

## Change settings later

Run:

```powershell
.\ParsecWebTurn.exe --settings
```

You can also use:

```powershell
.\ParsecWebTurn.exe /settings
```

## Local ICE fallback

If the Cloudflare API request fails, the app can fall back to a local `ice.json`.

Copy `ice.example.json` to `ice.json` and fill in a valid short-lived TURN username and credential:

```json
{
  "iceServers": [
    {
      "urls": [
        "stun:stun.cloudflare.com:3478",
        "turn:turn.cloudflare.com:3478?transport=udp",
        "turn:turn.cloudflare.com:3478?transport=tcp",
        "turns:turn.cloudflare.com:5349?transport=tcp",
        "turns:turn.cloudflare.com:443?transport=tcp"
      ],
      "username": "YOUR_USERNAME",
      "credential": "YOUR_CREDENTIAL"
    }
  ]
}
```

## Security model

The direct-to-Cloudflare design is intentionally backend-free and portable.

That means the long-lived TURN API token exists on the client PC. To reduce exposure:

- it is never embedded in the executable or GitHub repository
- it is stored with Windows DPAPI for the current user
- the active TURN username and credential are written into generated `extension/inject.js`; Cloudflare credentials are temporary, while custom static credentials can be long-lived
- `settings.json`, `.turn-cache.json`, `ice.json`, `extension/inject.js`, and `Profile/` must not be committed or shared
- console messages do not include TURN usernames or passwords

For environments where the client device itself is not trusted, use a backend credential broker instead.

## Build

Requires Windows, Go (CI uses 1.27.1), Node.js 24 or newer, and PowerShell. The same script is used locally and in CI; it embeds the checked-in icon and required Common Controls v6 manifest, runs Go and JavaScript tests, and creates the executable, portable ZIP and SHA-256 checksums.

```powershell
.\scripts\build.ps1
```

The first build downloads the Go dependencies and `github.com/akavel/rsrc@v0.10.2`. Dependency checksums are committed in `go.sum`; builds use `-mod=readonly` and do not run `go mod tidy`. The icon is not downloaded or converted during builds.

For a release, update `VERSION`, `CHANGELOG.md` and `RELEASE_NOTES.md`, merge the validated changes into `main`, then push the matching `vX.Y.Z` tag. The release workflow validates the version and reruns the shared build before publishing assets. ZIP packaging uses an explicit file allowlist so local runtime files cannot enter a release.

## Troubleshooting

Open Edge DevTools with `F12` and check the Console for:

```text
[ParsecWebTurn] ICE override installed before Parsec startup; server count:
```

For WebRTC diagnostics open:

```text
edge://webrtc-internals
```

A successful TURN path should show a selected candidate pair with `state=succeeded` and a relay candidate. The override applies both at construction and to subsequent `setConfiguration()` calls. It preserves the application's transport policy; configuring TURN servers alone does not force a relay connection.

Both API responses and `ice.json` are validated before launch. Browser-blocked port 53 URLs are filtered out; an empty usable configuration is rejected.

## Icon / trademark note

The executable uses a checked-in Parsec favicon. See `assets/README.md` for its source and checksum.

ParsecWebTurn is an independent community project and is **not affiliated with, endorsed by, or sponsored by Parsec or Unity**.

## License

MIT
