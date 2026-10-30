# EvoParseq for Android TV

Development branch: `codex/android-tv`. First live device: Xiaomi TV Box S
(3rd Gen) EU, Android 14 (32-bit ARM userspace). Google TV / Android TV interface,
D-pad navigation, two-factor sign-in and shared Rust account/session logic.

## Streaming preview

- Sign in, select a computer, and start a **WebRTC direct/STUN** session with the
  unmodified official Parsec host. No browser, WASM or stock SDK is bundled.
- **Hardware MediaCodec → SurfaceView Surface**, using asynchronous NDK callbacks.
  Decoded video never crosses JNI, becomes a Bitmap, or is read back to CPU memory.
  This does not prove driver/compositor zero copy or direct scanout.
- H.264 8-bit 4:2:0 is the default. Hardware-capable HEVC 8/10-bit 4:2:0 can be
  requested in Settings. The host selects the actual format; Main10 is not HDR.
- Existing Rust Opus decoding, bounded compressed/PCM queues and keyframe recovery;
  48 kHz stereo AAudio output with a low-latency device request. Actual latency
  depends on Android, the TV audio path and the device; no speedup is claimed.
- Back/Disconnect, backgrounding, audio focus loss and Surface destruction stop
  the session. A new Surface requires a deliberate new connection.
- **View and audio only in this step. Controller forwarding follows separately.**
  BUD and configurable TURN relays are not yet connected on Android. BUD is
  disabled and rejected explicitly, never silently replaced by WebRTC.
- Secrets remain in memory; non-secret requests are stored privately on the TV.
  Persistent sign-in is absent until Android Keystore integration exists.

Live video playback is confirmed on the box for H.264 and HEVC 8-bit, and for
HEVC Main10 with an NVIDIA host encoder. The AMD host test returned 8-bit;
requesting Main10 alone is not proof of a 10-bit stream. Continuous audio still
needs validation: the initial sound was audible but later playback was silent.
The next preview primes AAudio before starting and drains ready PCM without
idle gaps, with decoder/output counters to locate remaining failures.
Test sound, Back, Home/background, reconnect, resolution changes and host failure.
No HDR, 4:4:4 or AI upscaling is advertised.

## Shared architecture

The Android crate compiles the existing account, authenticated signaling,
WebRTC/ICE/DTLS/SCTP, control framing, audio decoder, video inspection/queue and
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
