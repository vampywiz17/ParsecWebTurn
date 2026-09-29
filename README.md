# ParsecWebTurn

Portable, no-install launcher for **Parsec Web** using **Microsoft Edge** and a custom WebRTC STUN/TURN configuration.

## What it does

- launches `https://web.parsec.app/` as an Edge app window
- uses a separate portable Edge profile in the same folder
- loads a small unpacked Manifest V3 extension only for `web.parsec.app`
- injects the ICE override at `document_start`, before the Parsec page scripts run
- replaces Parsec's default ICE list with the contents of `ice.json`
- requires **no administrator rights** and installs nothing

The launcher uses the Microsoft Edge already present on Windows. No separate .NET runtime, WebView2 install, VPN driver, WARP client, or browser extension installation is required.

## Setup

1. Download or build `ParsecWebTurn.exe`.
2. Copy `ice.example.json` to `ice.json`.
3. Put your short-lived Cloudflare TURN `username` and `credential` into `ice.json`.
4. Keep the `extension` directory next to the EXE.
5. Start `ParsecWebTurn.exe`.

Example:

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

## Build

Requires Go.

```powershell
$env:GOOS = "windows"
$env:GOARCH = "amd64"
go build -trimpath -ldflags "-s -w -H=windowsgui" -o ParsecWebTurn.exe ./src
```

A GitHub Actions workflow is included and produces a Windows x64 artifact on pushes and manual runs.

## Notes

- Cloudflare TURN credentials are short-lived. When they expire, update `ice.json` and restart the app.
- `ice.json`, generated `extension/inject.js`, and `Profile/` are ignored by Git.
- The generated `extension/inject.js` contains the active TURN username/credential. Treat it as sensitive while the credentials are valid.
- If an organization policy blocks command-line loaded extensions, the launcher cannot override that policy.
- This project intentionally launches Microsoft Edge because Parsec H.264 rendering can differ between Chromium builds/drivers.

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

## Security

Do **not** commit real TURN credentials. Use only short-lived credentials on the client and keep the Cloudflare TURN API token/server-side key out of this project.

## License

MIT
