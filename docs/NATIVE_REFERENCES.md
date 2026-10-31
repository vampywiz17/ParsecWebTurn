# EvoParseq development references

EvoParseq is an independent implementation for compatible Parsec hosts. External
projects are research inputs, not compatibility guarantees.

## lowlat

[nomi-san/lowlat](https://github.com/nomi-san/lowlat), MIT, provides the pinned
protocol, parser, GPU-driver and BUD transport components identified in the local
`EVOPARSEQ.md` provenance notices. Original copyrights and licenses are retained.
Only the components required by this application are included; no lowlat SDK,
host, capture stack or second application/session implementation is embedded.

The GPU driver/parser reference is revision
`5f8db5689e55de0d396b0520824caa9fc053dd86`. TURN stream IO improvements also use
reviewed upstream revisions `befd5a29c7a331111f11378c0b5623de788e9769`,
`d5d8d9ed4b1208f00de9fe8f90286919b15a0240` and
`b174dc6ac8d4923f45abe0c3a0a0a41fc0b76c49`. The receive-only cursor cache
reference is `e548d48c81ae50937940b2b47f700557cd9d50a4`.

The AMF initialization guard follows the finding in
[`565f375`](https://github.com/nomi-san/lowlat/commit/565f375136c7fe3da925ae8c4e9cbbba0b43b87e):
AMF may accept dimensions that the decode engine cannot use. EvoParseq's own
HEVC-only adapter checks the coded size and Main/NV12 or Main10/P010 profile
with a temporary D3D11 decoder on that same device, releasing it before AMF
initialization. AMF keeps its visible-size initialization. There is no frame
submission, pixel copy, extra queue or per-frame probe; failure uses the existing
logged Media Foundation fallback. This is a D3D11 configuration check, not proof
that every compressed stream will decode successfully.

## Native APIs and protocols

- Microsoft documented D3D11 video, DXGI, Windows Presentation API, WASAPI,
  Win32 input and CurrentUser DPAPI.
- W3C/IETF WebRTC, ICE, STUN, TURN, DTLS and SCTP; local third-party patches
  retain their upstream licenses and document changes in `EVOPARSEQ.md`.
  The DTLS 0.13 patch upgrades its NIST curve dependencies without replacing
  the protocol engine; see `src-native/vendor/dtls-0.13.0/EVOPARSEQ.md` for the
  exact scope and omitted, unused binary state export/import. The pinned BUD
  crypto subset retains its credentials and ring record code while omitting
  lowlat's unrelated browser-certificate generator; see its `EVOPARSEQ.md`.
- Vendor codec documentation and runtime capability queries. Device vendor
  names alone never establish an implemented decoder/output format.

Refer to [native architecture](native-core.md) and the [root README](../README.md)
for implemented behavior and the distinction between live and synthetic testing.


## Stock Windows host display order

Single-stream output selection follows lowlat's
[owner configuration transaction](https://github.com/nomi-san/lowlat/blob/3a48023fc08492806fe10ea149f745a569599ff0/examples/client/session.cc#L147)
(application query/reply pairs 10/12 for outputs and 9/11 for settings; write 11).
No lowlat SDK/session/UI implementation or dependency was added.

Read-only inspection of stock `parsecd-150-105c.dll`, SHA-256
`dbd59739cfb84a239860a30480b91281f0ebd316dfb5bf58bd4539e82c92c425`,
corroborated the output reply at RVA `0x71988` and its enumerator `0x2a90`.
The enumerator calls `IDXGIFactory1::EnumAdapters1` (vtable slot 12), then
`IDXGIAdapter::EnumOutputs` (slot 7), appending outputs in that order. The reply
serializes this array without sorting. The settings-query handler at `0x71926`
passes `1` to the serializer at `0xa7c00`, which writes `hostOS` in each slot.
No stock binary bytes or implementation were copied into the client.

Microsoft documents primary-first order for both
[EnumAdapters1](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/nf-dxgi-idxgifactory1-enumadapters1)
and [EnumOutputs](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/nf-dxgi-idxgiadapter-enumoutputs).
Together with the examined host enumeration, this supports interpreting its
first listed output as primary. This is a Windows stock-host compatibility
contract, not a general property of every remote JSON display list. Other host
OS values do not enable HID primary selection. Live multi-output verification
is still required, including virtual displays and primary/display hotplug changes.

## Stock session option presets

The current stock Windows host output reply contains output identifiers, names
and adapters; its settings reply reports configured values. Neither inspected
reply declares supported resolution, encoder-rate or bandwidth option lists.
A current setting is not treated as a capability limit. EvoParseq therefore uses
stock-client presets, not invented capability fields or a local-monitor scan.
The host still determines whether a requested mode can be fulfilled.

Read-only inspection of the same stock DLL and hash documented above found the
public UI resolution labels at file offsets `0x334640..0x335a80` (client/host
policies and 15 fixed sizes), and bandwidth labels at `0x336370..0x33715c`
(3, 5, 7, 10, 15, 20, 25, 30, 35, 40, 45, 50 Mbps). The FPS selector builds
six numeric choices, 30, 60, 100, 120, 144, 240, around RVA `0x15d2d..0x15fac`.
Only these option values were recorded; no binary bytes or implementation were
copied. The shared session model owns these presets; the Win32 view uses native
selection-only combo boxes. A previously saved non-preset value is shown as a
keep-current action and retained until the user explicitly selects a replacement;
this preserves an existing value without allowing entry of a new custom preset.

Parsec's [Windows documentation](https://support.parsec.app/hc/en-us/articles/32381199341716-Parsec-App-for-Windows)
also explains that resolution requests depend on Windows display support, and
that requesting an FPS does not establish support for that display refresh rate.

## Shared controller support

The 16-button full-state framing follows lowlat main revision
`b69a338db855ec6503090f5015e4f8019724e0d1`,
[protocol section 11.1](https://github.com/nomi-san/lowlat/blob/b69a338db855ec6503090f5015e4f8019724e0d1/docs/01-protocol.md#111-control-messages)
and `crates/client/src/input.rs`: opcode 23 names the pad in argument 0,
then three initialized padding bytes, big-endian button bits/four signed sticks,
and two byte triggers; vertical whole-state axes point up. Opcode 6 unplugs the
pad in argument 2; opcode 20 returns pad/low/high motor bytes. EvoParseq owns
its shared state, bounded event handoff, lifecycle and platform adapters; no
lowlat SDL/demo/SDK input implementation or extra controller dependency is added.

Parsec's [Setup Gamepad](https://support.parsec.app/hc/en-us/articles/32381705301908-Setup-Gamepad)
documents client face-button activation and host-side virtual controllers.
Read-only inspection of the stock Windows DLL with the hash above found an
`XInputSetState` symbol; that is a research clue, not proof of every input API
or of our wire compatibility. No stock binary code is copied or patched.
The official Android app is closed source; its documented controller behavior,
not unverified implementation details, is the comparison point.

Platform adapters use Microsoft's documented
[XInput](https://learn.microsoft.com/en-us/windows/win32/xinput/getting-started-with-xinput)
and Android's documented
[controller events](https://developer.android.com/games/sdk/game-controller/controller-input).
Synthetic wire/lifecycle tests do not establish stock-host live compatibility.
