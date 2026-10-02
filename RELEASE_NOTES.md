ParsecWebTurn v0.7.0 adds explicit STUN-only connections and expands connection statistics.

- Separate STUN and custom TURN address fields. Existing mixed address lists migrate automatically.
- **STUN only — no TURN relay** supports direct LAN/VPN/internet connections without offering a TURN relay. With Cloudflare selected and no custom STUN address, it uses Cloudflare's public STUN endpoint without an API token or generated TURN credentials.
- Custom STUN addresses can also be used alongside Cloudflare's generated TURN configuration. When TURN is enabled, native ICE selects the route; a reachable direct path does not guarantee that ICE selects it.
- Connection statistics include reported DTLS state, version, cipher and SRTP profile, plus application/WebView2 CPU and WebView2 GPU utilization when available.
- Audio codec, sample rate and channel metadata remain visible after detection, including during silence. The sender's configured bitrate remains unknown when receiving APIs do not expose it; measured throughput is not substituted for it.
- Grant clipboard-read permission for the exact Parsec web origin through Microsoft's documented WebView2 profile API. Windows clipboard reads now pass the native test.
- Remove temporary ICE console diagnostics and abandoned automatic TURN-restart experiments. No automatic direct-first reconnection is included.

**Known limitation:** client-to-host clipboard paste remains broken in the tested Parsec session and also reproduces in standalone Edge. The WebView2 permission fix does not resolve Parsec's end-to-end clipboard synchronization. HEVC driver issues and long-session freezing are not claimed to be fixed.

Download **ParsecWebTurn-v0.7.0-win64.zip**, extract it to a writable directory, and run `ParsecWebTurn.exe`. **Microsoft Edge WebView2 Runtime must be installed.** Ordinary use requires no administrator rights when the runtime is present; network connectivity depends on the selected STUN/TURN configuration.

**Upgrading:** close the app and replace the EXE. Keep `settings.json` and `WebView2Profile` to preserve configuration, encrypted credentials and Parsec sign-in. Update notifications remain metadata-only; ZIP downloads and installation are manual.
