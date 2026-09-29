# ParsecWebTurn

Portable, no-install launcher for **Parsec Web** using **Microsoft Edge** and a custom Cloudflare Realtime WebRTC STUN/TURN configuration.

## What it does

- launches `https://web.parsec.app/` as an Edge app window
- requires **no administrator rights**
- installs no VPN, driver, .NET runtime, or WebView2 runtime
- uses the Microsoft Edge already present on Windows
- creates a separate local Edge profile beside the application
- injects the ICE override at `document_start`, before the Parsec page scripts run
- automatically requests fresh short-lived TURN credentials directly from Cloudflare
- stores the long-lived TURN API token locally using **Windows DPAPI / Current User**
- supports a local `ice.json` fallback
- uses the Parsec icon for the Windows executable

## First run

1. Download and extract the release ZIP to any writable directory.
2. Run `ParsecWebTurn.exe`.
3. Enter:
   - **Cloudflare TURN Key ID**
   - **Cloudflare TURN Key API Token**
   - credential TTL in seconds (default 86400 / 24 hours, maximum 172800 / 48 hours)
4. Click **Save & Start**.

ParsecWebTurn stores the TURN Key ID and TTL in `settings.json`. The API token is encrypted with Windows DPAPI and can only be decrypted in the same Windows user context.

On every normal start the app calls Cloudflare's TURN credential endpoint and obtains a new short-lived `username` / `credential` pair automatically.

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
        "turns:turn.cloudflare.com:5349?transport=tcp"
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
- only short-lived TURN credentials are written into the generated `extension/inject.js`
- `settings.json`, `ice.json`, `extension/inject.js`, and `Profile/` must not be committed or shared

For environments where the client device itself is not trusted, use a backend credential broker instead.

## Build

Requires Go. The GitHub Actions workflow also downloads the Parsec SVG icon and embeds it in the Windows executable.

```powershell
$env:GOOS = "windows"
$env:GOARCH = "amd64"
go build -trimpath -ldflags "-s -w -H=windowsgui" -o ParsecWebTurn.exe ./src
```

For a local build with the same EXE icon as the release, generate a Windows resource first; see `.github/workflows/build.yml`.

## Troubleshooting

Open Edge DevTools with `F12` and check the Console for:

```text
[ParsecWebTurn] Cloudflare STUN/TURN override:
```

For WebRTC diagnostics open:

```text
edge://webrtc-internals
```

A successful TURN path should show a selected candidate pair with `state=succeeded` and a relay candidate.

## Icon / trademark note

The executable uses the Parsec SVG icon requested for this project, sourced from SVG Repo. See `assets/README.md` for the source link.

ParsecWebTurn is an independent community project and is **not affiliated with, endorsed by, or sponsored by Parsec or Unity**.

## License

MIT
