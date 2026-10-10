# Embedded client settings audit

Reviewed against the packaged, SHA-256-pinned Parsec WASM **150-104a** and the
matching original web adapters on 2026-10-10. This is a source/interface audit,
not a claim that every option has been tested with a remote host.

The embedded UI contains shared Parsec settings. A visible choice does not
establish that its web/native backend supports it. Keep configuration owned by
the embedded core where possible; implement the required platform service in
Rust rather than adding a second, conflicting copy of the same setting.

| Setting / area | Current native bridge | Further work |
| --- | --- | --- |
| Overlay, overlay button, HID compatibility options | Original core UI is composited over the D3D11 video surface; the core owns menu visibility. | Test individual HID options on a separate host. Visibility alone does not prove an input mode is implemented. |
| Overlay warnings / connection statistics | Resolution, range and host encode latency reach the core. Several metrics remain unavailable or at the adapter's initialization values. | Feed measured native transport/decoder statistics; never interpret an unmeasured zero as a measurement. |
| Window Mode | `web_set_fullscreen` reaches the native fullscreen bridge. | Live regression test changing the core preference and reconnecting. |
| Relative mouse | `web_set_pointer_lock` reaches Win32 raw mouse input. | Keep the existing core overlay switch as the owner of the mode. |
| Immersive Mode | Relative mouse is implemented; enabling `web_set_kb_grab` is not. | Native focused-window keyboard capture, reliable release on focus loss, and a local escape shortcut. Test this before treating Immersive Mode as supported. |
| Hotkeys; Swap Command and Ctrl | Local scan codes, modifiers and text reach the core; its configuration is persisted. | Verify remapping end to end, particularly macOS hosts. OS-reserved combinations also depend on the missing keyboard capture above. |
| Audio Codec: Compressed (Opus) | Native libopus decoder, 48 kHz stereo PCM output through WASAPI. | Existing implemented path. |
| Audio Codec: Uncompressed (RAW) | **Not implemented.** The matching original `parsec.js` also unconditionally configures its audio decoder as Opus. The Rust pipeline does the same. | Establish the actual negotiated format and packet contract before adding a separate PCM path. The UI string `network_raw_audio` and the description “48kHz and 2-channel” do not establish sample representation, byte order or framing. Never detect RAW by guessing from packet bytes or an Opus decode error. |
| Audio buffer settings | The core's output minimum/maximum buffer arguments reach WASAPI. | Validate behaviour and latency under load; no duplicate Rust UI needed. |
| Gamepad, mapping, rumble | No native controller polling/event source or rumble service. | Controller platform bridge, core events, and matching control framing, then hardware tests. A mapping page is not controller support. |
| Video decoder, H.265, 4:4:4, renderer, VSync | Current stream path uses the Media Foundation H.264 decoder and D3D11/NV12 presentation. | Additional codecs/formats/renderers require explicit native implementations. Shared configuration strings do not switch the existing Rust pipeline. Audit presentation pacing before exposing VSync control. |
| Account, login, normal core preferences | Core filesystem/profile persistence is already implemented. | No additional native configuration UI needed. |
| Microphone, camera, USB forwarding / host features | Not implemented as native client services. Optional USB imports remain unavailable. | Separate capability work; do not advertise support based on shared UI/configuration strings. |

Evidence in this tree:

- `src-native/src/host.rs`: implemented import dispatch, fullscreen/pointer lock,
  unavailable keyboard capture, metrics and optional service boundaries.
- `src-native/src/audio_stream.rs`, `audio.rs`, `audio_windows.rs`: Opus,
  PCM output format and buffering.
- `src-native/src/control.rs`, `input.rs`, `window.rs`: input delivery and
  native window behaviour.
- `src-native/src/video_windows.rs`, `overlay_windows.rs`: active video and
  overlay paths.
- Original matching web adapter: `parsec.js`, `ma` and the audio data-channel
  callback configure `codec: "opus"`, `sampleRate: 48000`, `numberOfChannels: 2`.
  These are inspection inputs, not production JavaScript dependencies.

Recommended order: RAW negotiation/format proof, safe keyboard capture,
gamepad support, measured statistics, then additional video/rendering paths.
Each needs its own functional test; this audit does not implement those features.

The native **Help → About ParsecWebTurn** dialog and `--version` share the same
core-version constant. It identifies the packaged core, not an online version
check. The executable verifies the embedded WASM hash at startup.
