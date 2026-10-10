ParsecWebTurn v0.9.0 (dev)

This development build brings custom connection servers, live statistics and a
clearer Settings page to the native Windows client.

Compared with the earlier stable WebView2 client, the native app runs from one
portable EXE without Edge or WebView2. It keeps the familiar Parsec interface,
with native video, Opus audio, keyboard and mouse support.

## What's new

- **Your own STUN/TURN servers:** use Cloudflare or a custom provider, with UDP,
  TCP and encrypted TLS support. The earlier client's `settings.json` remains
  compatible. STUN-only mode excludes TURN for LAN/VPN connections.
- **Connection stats in a separate window:** see delay, traffic, FPS, video/audio
  details, encryption status and CPU/GPU usage. Confirmed direct/relay routes and
  the active relay server are shown when available.
- **Settings that fit the app:** a Parsec-inspired layout, clearer descriptions,
  dark controls and fixes for overlapping text, scrolling and clipped buttons.
- **Help with both version numbers:** view the app version and embedded Parsec
  core version in one place.
- **Reliable saved preferences and disconnect:** sign-in and preferences survive
  restarts, and disconnecting notifies the host. The window stays named
  ParsecWebTurn.
- **Playback latency improvements:** targeted Windows media scheduling and a
  shorter device-supported audio period, with automatic compatibility fallback.
  Actual improvements depend on your device and need live comparison.

## Things to know

- This is a development build, not a new stable release.
- Windows x64 only; Vulkan, D3D12, Linux and Android support are not included.
- Opus audio is supported; RAW playback is not available.
- TURN can be selected even when a direct route exists. Use STUN-only mode to
  exclude relays.
- Some measurements are unavailable. GPU decoder surfaces indicate likely
  hardware decoding, not confirmed hardware execution.
- Client-to-host clipboard paste remains a known issue in the tested Parsec
  session and also reproduced in standalone Edge.

Replace the EXE to update the native app; your saved native sign-in and preferences
remain in place. Moving from the older WebView2 client requires signing in once
in the native app.
