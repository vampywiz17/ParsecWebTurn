# EvoParseq on macOS

An experimental native macOS build of EvoParseq. It shares the Rust account,
signaling, WebRTC, BUD, settings and statistics code with the Windows client and
replaces only the platform adapters. WebRTC remains the default transport.

## Requirements and build

macOS 11 or later. Apple Silicon is the tested platform; the Intel target builds
from the same source but has not been tested. Rust (stable) and the Xcode
Command Line Tools are build requirements.

```sh
cargo test --locked --manifest-path src-native/Cargo.toml --target aarch64-apple-darwin
./scripts/package-macos.sh
```

`src-native/.cargo/config.toml` selects the Windows target by default, so macOS
commands pass `--target` explicitly. The script writes an ad-hoc signed
`EvoParseq.app` inside `dist/EvoParseq-v<version>-macos-<arch>.zip`. Without a
Developer ID signature and notarization, Gatekeeper asks you to approve the app
under **System Settings / Privacy & Security / Open Anyway** on first launch.

Settings, the event log and the remembered sign-in live in
`~/Library/Application Support/EvoParseq`; `--data-dir <folder>` selects another
folder. Saved credentials are sealed with ChaCha20-Poly1305 under a random key
kept in the login Keychain (`EvoParseq` / `settings-protection-key`). They do not
decrypt for another macOS user or Mac. macOS may ask once for Keychain access
after the app's signature changes, for example after an update.

## Platform adapters

| Area | macOS implementation |
| --- | --- |
| Windows and controls | AppKit (`objc2`): launcher, stream window, Settings, Stream Settings, Connection stats, menu bar |
| Video | Annex B H.264/HEVC Main → `CMSampleBuffer` → `AVSampleBufferDisplayLayer` |
| Audio | Core Audio default-output AudioUnit, 48 kHz PCM |
| Relative mouse | `CGAssociateMouseAndMouseCursorPosition` with AppKit deltas |
| Cursor | Host cursor images as `NSCursor` at the local arrow cursor's size |
| Clipboard | General pasteboard; sent when the stream window regains focus or on Ctrl+V |
| Display sleep | IOKit `PreventUserIdleDisplaySleep` assertion while a session is visible |
| Protected settings | Security framework Keychain item + `ring` ChaCha20-Poly1305 |
| BUD sockets | `lowlat-net` macOS backend: `poll`, pipe wake, `getifaddrs` |

The display layer decodes with VideoToolbox and presents through Core Animation.
The application copies each compressed access unit once into Core Media's
length-prefixed sample layout and never reads decoded pixels back. The layer does
not report whether VideoToolbox used the hardware decoder, so statistics show
hardware decode as not reported. This path is not the application's Zero copy
mode and makes no zero-copy claim beyond those facts.

## Controls

The stream window forwards physical keys by USB usage, like Windows. Session
shortcuts follow the official Parsec macOS defaults
([Configure Hotkeys](https://support.parsec.app/hc/en-us/articles/32381778420372-Configure-Hotkeys)):

| Shortcut | Action |
| --- | --- |
| ⌘⇧M | Stream settings (Parsec: Menu) |
| ⌘⇧S | Connection stats (Parsec uses it for Add Screen, which this client lacks) |
| ⌘⇧W | Toggle fullscreen (Parsec: Window Mode) |
| ⌘⌥Z | Release / recapture input (Parsec: Detach Input) |
| ⌘⌥\`, ⌘W or ⌘Q | Disconnect |
| ⌃⌥⌫ | Send Ctrl+Alt+Del |

These chords stay local, so ⌘Q in the stream window disconnects rather than
quitting; ⌘Q elsewhere quits. The **Session** menu offers the same actions.
Other Command chords that match a menu item (Settings, the Edit menu) stay
local; the rest reach the host as the Meta key. Parsec's Chat, Switch Display,
Immersive Mode and hosting hotkeys have no counterpart in this client.

## Limits

- Not available on macOS: HEVC Main 10, HEVC 4:4:4, Zero copy, display-timing
  modes, GPU selection, AI upscaling and TCP/TLS QoS marking. Settings shows only
  the options that apply.
- Host orientation (rotated output) is not applied to the video layer yet.
- Audio follows the system default output device; multichannel output is not
  mapped to a device channel layout.
- The floating session button is replaced by the menu bar and Stream Settings
  window.
- BUD and USB HID input have no live validation in this macOS build yet; the
  `lowlat-net` unit tests have not been ported to the macOS backend.
- Trackpad scroll scaling (1.2 wheel units per point) is a starting value.
