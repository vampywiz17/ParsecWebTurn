# EvoParseq for Android TV

Development branch: `codex/android-tv`, based on dev `090be04`.
First validation device: **Xiaomi TV Box S (3rd Gen) EU, Android 14**.
The interface follows **Google's Android TV / Google TV** conventions, not a
Xiaomi-specific skin. EvoParseq retains its own blue branding.

## Current preview

This is a TV/account foundation, **not a working streaming client yet**:

- Android TV launcher entry, app banner, landscape layout and dark launch window.
- Compose for TV buttons/cards, D-pad focus, large typography and Back navigation.
- Real Parsec sign-in, optional two-factor code, host discovery/refresh and sign-out,
  using the **same Rust account implementation** as the Windows app. No WebView,
  WASM, stock SDK, host patch, lowlat host or parallel authentication protocol.
- D-pad-reachable settings with selection-only choices, return-to-row focus and
  private local persistence of non-secret requests. Resolution/FPS/bandwidth presets,
  parsing and limits use the shared Windows Rust model, not Kotlin copies.
  These are saved requests for the future session backend, not active stream controls.
- Credentials/token stay in memory. Password and 2FA are not saved in Android's
  instance state; native secrets use the existing zeroization/revocation policy.
  Force-stopping/killing a process cannot guarantee a remote logout. Persistent
  sign-in is intentionally absent until Android Keystore storage is implemented.
- On-demand device/ABI and hardware H.264/HEVC decoder capability inventory,
  including declared 1080p60, HEVC Main10 and low-latency support. These queries
  do not prove successful decoding or a measured latency.
- ARM64 and ARMv7 libraries: a 64-bit SoC alone does not establish Android userspace
  bitness. Android 10 / API 29 is the initial API floor; Android 14 is the first
  intended live target. Older supported API levels are not yet live-validated.

The first on-device inventory (Xiaomi MiTV-AFMU0, Android 14 / API 34) reports
**armeabi-v7a/armeabi userspace**, `c2.amlogic.avc.decoder` and
`c2.amlogic.hevc.decoder`. Both declare 1080p60 and low-latency support; HEVC
also declares Main10. App launch/inventory rendering has been observed on this
device. Hardware playback, Surface behavior and latency remain unverified.

Select a computer to inspect it. Streaming controls are **not offered as working**
until the Android media/session backend exists. No synthetic/fake hosts or
hard-coded successful connection status are used.

## Architecture and media rules

Kotlin is the thin Android UI/platform adapter; account/protocol/session rules
remain in Rust. The small JNI boundary currently carries account commands and
host descriptions only. The shared account source is compiled directly from
`src-native/src/native_core/account.rs`, with the existing optional event logger;
there is no copied account implementation.
Video request types, validation, presets and panel parsing now live in
`src-native/src/session_preferences.rs`; the Windows profile/panel re-export the
same types and functions without an intended behavioral change. Android-specific
API calls must stay in adapters; do not fork these shared Rust policies.
The independent Cargo manifest excludes Windows codecs, Opus, WebRTC and BUD
until they are actually connected. The packaged MIT/lowlat attribution remains.

Next implementation steps, in order:

1. Port the existing signaling and **WebRTC (default)** / explicit BUD session
   adapters without changing their route/fallback policy. Preserve stock,
   unmodified Parsec host compatibility, including TURN UDP/TLS/TCP.
2. Add the Android media backend: **MediaCodec hardware decoder → SurfaceView
   Surface → Android compositor**. Supply compressed access units to codec input;
   prefer Rust's documented NDK MediaCodec/ANativeWindow bindings, attaching the
   Android Surface at lifecycle boundaries instead of forwarding every frame via JNI.
   do not copy decoded frames to Rust/Java arrays, Bitmap, CPU YUV/RGB buffers or
   a TextureView/OpenGL intermediate. Keep UI composition separate from video.
   Hardware selection and format/rate capabilities must be checked at runtime;
   request documented low-latency mode only where supported. Do not silently
   substitute software decoding or build an unbounded input/output queue.
3. Bind audio, keyboard/mouse/gamepad and lifecycle. Test focus loss, Home/Back,
   suspend/resume, Surface replacement, HDMI mode changes and disconnect cleanup.
   Keep UI remote keys local when menus are open; do not steal Android Home/system
   controls. Existing Windows HID behavior is not presumed Android-compatible.
4. Add opt-in saved session protection through Android Keystore/private app storage,
   network/video/audio settings and bounded operational logging. Never copy Windows
   DPAPI blobs to Android or keep account passwords as configuration.

MediaCodec Surface output avoids an **application-side decoded-frame copy**. It is
not proof of compositor/driver direct scanout, lack of internal conversions or our
strict Windows **Zero copy** mode. Do not label it proven zero copy without the
corresponding evidence. GPU/compositor work is necessary; network, compressed
input, input devices and UI still use the CPU. No speedup is claimed yet.

Start validation with **1080p60 H.264 8-bit 4:2:0 SDR**, then hardware-capable HEVC
8/10-bit 4:2:0. Do not infer HDR from Main10, or add AV1/4:4:4/AI processing from
box marketing capabilities. The host still chooses the actual stream format.

## Build / install

CI: **EvoParseq Android TV preview** builds a debug-signed side-load APK and runs
Rust tests/Clippy plus Android lint. This is not a Play Store release. The Windows
CI and production client remain unchanged. No local Android SDK/JDK was available
when this branch was started; consult the CI result for build validation.

Local prerequisites: JDK 17, Gradle 8.13, Android SDK platform 36/build-tools 36.0.0,
NDK 28.2.13676358, Rust and cargo-ndk 4.1.2. Set `ANDROID_HOME` and
`ANDROID_NDK_HOME`; do not commit `local.properties` or signing keys.

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi
cargo install cargo-ndk --version 4.1.2 --locked
cd android-tv/native
cargo ndk -t arm64-v8a -t armeabi-v7a --platform 29 -o ../app/src/main/jniLibs build --locked --release
cd ..
gradle assembleDebug lintDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

APK: `app/build/outputs/apk/debug/app-debug.apk`. No extra DLLs or manual native
library installs. No root/admin permission on the TV is required to run it.
Use Android's normal installation/developer settings if side-loading.

First live check: D-pad-only navigation and Back; sign-in/2FA; host refresh and
sign-out; launch/background/reopen; device capabilities. Do not enter production
credentials in an emulator/screenshot test or record account fields in a bug report.
Actual playback, latency, memory and input validation follow the media backend.

## Platform references

- [Create a TV app](https://developer.android.com/training/tv/get-started/create)
- [Compose for TV](https://developer.android.com/jetpack/androidx/releases/tv)
- [TV design](https://developer.android.com/design/ui/tv)
- [MediaCodec Surface output](https://developer.android.com/reference/android/media/MediaCodec)
- [Documented low-latency flag](https://developer.android.com/reference/android/media/MediaCodecInfo.CodecCapabilities#FEATURE_LowLatency)
- [KEY_LOW_LATENCY](https://developer.android.com/reference/android/media/MediaFormat#KEY_LOW_LATENCY)

EvoParseq is an independent community project, not affiliated with or endorsed by
Parsec/Unity, Xiaomi, Google or lowlat. Parsec account/service terms still apply.
