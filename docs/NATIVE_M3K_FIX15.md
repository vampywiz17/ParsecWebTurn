# M3k-fix15 / 0.14.15 — report unavailable audio without a factory trap

The user's fix14 account report confirms native transport connected:
ICE and DTLS connected, three open data channels, 13 received messages, and
no transport failure. This proves authenticated data-channel connectivity;
it does not establish a fully initialized Parsec media session or video.

The process closed because guest worker 16 reached the missing
`env::MTY_AudioCreate` import. Its explicit unsupported-bridge trap was recorded
as `guest-worker-failed`; the live worker supervisor consequently stopped the
window. There was no recorded graphics or DTLS error.

## Bounded unavailable-output behavior

Until native audio playback is implemented, AudioCreate now returns the
Matoya factory's documented null failure result, instead of trapping. No
usable handle is invented, no device is opened, and no PCM is read or queued.
AudioDestroy implements only the valid null/zero pointer-to-context case;
nonzero fabricated contexts remain errors. Queue/reset/queued playback APIs
remain unimplemented: the prototype has no context on which to use them.

The pinned WASM import has five arguments, with a format pointer first.
Current upstream Matoya's native declaration has six arguments and a different
format layout. This change does not copy that ABI. It relies only on the
common documented null-on-create-failure and null-safe-destroy contracts:
[Matoya header](https://github.com/snowcone-ltd/libmatoya/blob/master/src/matoya.h),
[Windows factory implementation](https://github.com/snowcone-ltd/libmatoya/blob/master/src/windows/audio.c).
The pinned web ABI was independently checked against the audited matoya-worker.js.

Import inspection labels the two operations `audio-output-unavailable`, not
an implemented renderer. Per-worker reports retain fixed audio availability
metadata and bounded counters, even after the worker finishes. No audio,
format pointer, device ID or guest-provided text enters the diagnostic.
The general policy for genuinely failing workers is unchanged.

This is a temporary graceful-unavailability fix, not audio playback. The
original guest's behavior after null audio creation still requires a user
retest. Video decoding/presentation may reach a subsequent missing boundary.

## Verification and retest

A synthetic WASM guest calls the exact five-argument import repeatedly,
receives null, destroys null handles, and executes a subsequent export without
trapping. It must reject a fabricated context. The test never opens a device
or plays audio. Existing authenticated SCTP-only and fingerprint-rejection
probes remain required.

Extract into a separate folder; run
`START-LEGACY-RSA-CLOUDFLARE-DIAGNOSTIC.cmd`. Connect once, observe whether the
window remains open and any image appears, close normally if needed, and share
`account-network-report.json`. Audio is unavailable in this test build.
