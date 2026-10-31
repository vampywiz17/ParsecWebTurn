# EvoParseq for Android TV

Development branch: `codex/android-tv`. First live device: Xiaomi TV Box S
(3rd Gen) EU, Android 14 (32-bit ARM userspace). Google TV / Android TV interface,
D-pad navigation, two-factor sign-in and shared Rust account/session logic.

## Streaming preview

- Connect to an unmodified official Parsec host using **WebRTC direct/STUN**
  (default) or **BUD direct UDP** (new, awaiting live Android validation).
  Configure Cloudflare or custom STUN/TURN in Network settings. WebRTC uses its
  existing ICE selection; BUD tries direct UDP and configured TURN carriers in
  UDP/TLS/TCP order (or TURN only when forced). Neither transport silently falls
  back to the other. No browser, WASM or stock SDK is bundled.
- Hardware **MediaCodec to SurfaceView Surface**, using asynchronous NDK callbacks.
  Decoded video never crosses JNI, becomes a Bitmap, or is read back to CPU memory.
  This does not prove driver/compositor zero copy or direct scanout.
- H.264 8-bit 4:2:0 is the default. Request HEVC 8/10-bit 4:2:0 in Settings when
  supported. The host chooses the actual format; Main10 is not HDR.
- Shared Rust Opus decoder, bounded queues and keyframe recovery; stereo 48 kHz
  AAudio pull output. The callback never waits on the session worker or allocates.
- Sign-in is remembered using AES-256-GCM and a non-exportable Android Keystore
  key. Only the session token is saved, encrypted in app-private no-backup storage;
  passwords and 2FA codes are never saved. Sign out removes local credentials and
  requests server-side revocation. Invalid/expired credentials require sign-in.
- Back/Disconnect, backgrounding, audio focus loss and Surface destruction stop
  the stream. Reconnect explicitly when a new Surface is created.
- Network/Video settings are grouped for D-pad use. Fixed FPS requests host
  `fullFPS`; dynamic FPS allows the host to reduce frame production when idle.
- The overlay shows sampled Surface submission FPS, selected-pair RTT, received
  payload bitrate and actual codec/depth. It hides after five idle seconds;
  OK/Menu/Back restores it, with Hide and Disconnect buttons. RTT queries pause
  while hidden; no GPU timing or pixel readback is introduced.
- Shared Rust controller state/lifecycle and full-state protocol on BUD and WebRTC.
  Android reads standard gamepad/joystick events; Windows uses native XInput.
  Up to four pads; press a face button after hiding the overlay to join the host.
  Local menus/focus loss release controllers; removal sends an explicit unplug.
  Xbox-style buttons, D-pad, sticks and triggers are supported. Sony raw touch,
  gyro and adaptive-trigger passthrough are not implemented. Device compatibility
  and vibration require live testing. Android vibration uses the controller's
  own vibrator, if exposed, without assuming separate motor identities.
  No HDR, 4:4:4, AI upscaling or rotated BUD output is advertised.

Live video is confirmed on the Xiaomi box for H.264 and HEVC 8-bit, and HEVC
Main10 with NVIDIA encoding. The AMD host test returned 8-bit. The user found
30 Mbps stable and treats approximately 30-35 Mbps as this setup's practical
ceiling; sustained 50 Mbps caused growing delay and video-channel closure on
Ethernet and Wi-Fi. This is not a proven universal decoder/hardware limit.
A later apparent audio regression was traced to the box's own audio output;
it was not established as a client regression.

Temporary callback/PCM peak counters and verbose test overlays have been removed.
Useful errors, bounded recovery and native-peer/queue regression tests remain.
Decoded-output catch-up still releases every codec index exactly once and never
skips compressed reference pictures. Empty WebRTC messages remain distinct from
stream EOF, as required by RFC 8831. Test both transports, audio, reconnect and
remembered sign-in on the actual device; builds alone do not validate hardware.

## Local relay setup

Open **Settings > Network > Set up from phone or computer**. The TV and browser
must be on the same private LAN. Scan the QR code or enter the displayed HTTPS
address. This is an expiring server on the TV, not an Internet setup service.

The certificate is generated locally for this setup session and is not publicly
trusted. Before entering any credentials, inspect its SHA-256 fingerprint in the
browser's certificate details and compare it with the TV. Only trust this exact
local certificate after verifying the fingerprint; do not disable browser TLS
checking globally. If your browser cannot show the certificate details, use a
browser/device that can. A pairing code alone does not authenticate the HTTPS
page against an active intermediary.

Enter the TV's six-digit code, choose direct/STUN, Cloudflare (TURN key ID and API
token), or custom TURN (URLs, username/password), then send. **Approve and save
on the TV** before anything is applied. Credentials are encrypted separately
from account sign-in using Android Keystore, excluded from backup and never
included in status/log output. Network settings take effect on the next stream.
Closing setup, leaving the app, starting a stream, or the five-minute deadline
stops the server. No port forwarding, public binding or remote setup is provided.

This new pairing and relay integration still needs live device validation.
HTTPS, origin/Host checks, bounds, wrong-code rejection and pending approval have
isolated tests; they do not replace real browser/TV and TURN tests.

## Shared architecture

The Android crate compiles the existing account, authenticated signaling,
WebRTC/ICE/DTLS/SCTP and BUD, control framing, audio decoder, video inspection/queue and
request-validation sources directly. Android-specific code only handles JNI,
Surface/MediaCodec, AAudio and TV UI/lifecycle. Windows retains its own media and
input backends. No decoded-frame conversion/copy layer is added to Windows.

## Build / install

The `EvoParseq Android TV preview` workflow runs common Rust tests/Clippy,
ARM64/ARMv7 builds and Clippy, and Android assemble/lint. It publishes a debug-signed
side-load APK, not a Play Store release. Each CI build can use a different debug
certificate: uninstall an older preview if Android rejects the update signature.

Prerequisites: JDK17, Gradle8.13, SDK36/build-tools36.0.0, NDK28.2.13676358,
Rust and cargo-ndk4.1.2. See `.github/workflows/android-tv.yml` for the exact
per-ABI Opus CMake toolchain and build commands. No local signing keys or
`local.properties` belong in git.

APK: `android-tv/app/build/outputs/apk/debug/app-debug.apk`. Both ARM64 and ARMv7
native libraries and Opus are included; no extra installation/root is required.

References: [TV design](https://developer.android.com/design/ui/tv),
[MediaCodec](https://developer.android.com/ndk/reference/group/media),
[AAudio](https://developer.android.com/ndk/guides/audio/aaudio/aaudio).

EvoParseq is an independent community project, not affiliated with or endorsed by
Parsec/Unity, Xiaomi, Google or lowlat. Parsec account/service terms still apply.

## Controller preview validation

Pair an Android-recognized USB/Bluetooth gamepad with the TV, connect, wait for
(or hide) the overlay, then press A/B/X/Y. Use the TV remote to reopen controls;
controller buttons are forwarded only while the overlay is hidden. After focus
loss, release held buttons before joining again. On Windows, use an XInput
controller and focus the stream with local menus closed. DirectInput-only pads
need a separate future Windows adapter; no remapping driver is installed here.
The unchanged official host needs its normal gamepad driver and guest permission.
Test both transports, button chords, sticks/triggers, unplug/reconnect, focus
loss, local menu isolation and device-supported rumble. Synthetic tests/builds
are not evidence of live controller compatibility.
