# Native latency tuning

This client carries Parsec video/audio on SCTP data channels over DTLS/ICE,
not as RTP media tracks. Generic WebRTC RTP jitter-buffer, playout-delay,
GCC/transport-wide-CC and SDP `ptime` recommendations are not switches for this
pipeline. Parsec's existing framing and reliable ordered negotiated channels
must be preserved. Turning off retransmission or reordering dependent H.264
pictures would require host/protocol changes and recovery behavior.

## Applied optimizations

- Disable automatic extra processing on the D3D11 video processor. Required
  NV12-to-RGB conversion, scaling and overlay composition remain on the GPU.
- Cache input views by decoder texture and array slice, and output views by
  backbuffer identity. Caches are bounded and cleared on stream/processor changes;
  release backbuffer views before ResizeBuffers. Cache validated output geometry
  and color metadata until Media Foundation reports a stream change.
- Preserve input sample ownership: ProcessInput can retain the compressed input.
  Do not overwrite or recycle its storage without a documented release callback.
- Allow explicit hardware adapter selection using DXGI and D3D11CreateDevice with
  D3D_DRIVER_TYPE_UNKNOWN. Automatic retains the default adapter. Save LUID plus
  hardware IDs; after a LUID change, accept only a unique hardware match. Missing
  or ambiguous saved adapters require reselecting in Settings, never a silent
  switch to an arbitrary identical card. This selects the stream GPU, not the
  embedded UI's OpenGL adapter or the host encoder. No cross-GPU-copy claim is made.
- Register the video worker with the documented Windows MMCSS `Playback` task
  and the WASAPI render worker with `Pro Audio`. Registration is optional:
  unavailable MMCSS keeps normal scheduling. RAII guards restore each thread's
  original scheduling on every exit path. No process-wide realtime priority,
  registry edits, system timer-resolution changes or administrator rights.
- Query `IAudioClient3::GetSharedModeEnginePeriod` for the actual 48 kHz stereo
  PCM16 format. Request the shortest supported, fundamental-aligned shared-mode
  period with `InitializeSharedAudioStream(EVENTCALLBACK)`. Unsupported format,
  API or period initialization falls back to a fresh `IAudioClient` and the
  existing event-driven 20 ms buffer request with Windows format conversion.
  No exclusive audio device is acquired. The guest's minimum/maximum audio
  buffering settings remain effective; reducing the device period does not
  remove the host's packetization or application buffering delay.
- Audio snapshots record whether low-latency initialization and MMCSS succeeded,
  the requested accepted engine period and actual device buffer capacity.
  Video snapshots record MMCSS registration separately from codec/GPU evidence.
  These are configuration/output observations, not measured end-to-end latency.

## Already present and retained

- UDP ICE, while custom TURN can use UDP, TCP or verified TLS.
- `TCP_NODELAY` on the TCP socket before TURN/TLS wrapping.
- Media Foundation `CODECAPI_AVLowLatencyMode` request with recorded acceptance.
- GPU-resident decoded textures and direct video-processor presentation.
- Flip-discard swap chain, `Present(0, DXGI_PRESENT_DO_NOT_WAIT)`; a busy display
  skips a presentation, never an encoded reference picture.
- Bounded queues. Queue capacities are safety limits, not a target delay.
- Detached data-channel readers; network callbacks do not depend on UI polling.

## Settings deliberately not forced

- `maxRetransmits=0`, unordered media and undersized SCTP receive windows:
  incompatible or unsafe without host cooperation. UDP transport does not mean
  SCTP retransmissions disappear.
- Aggressively reduced retransmission timers, MTU overrides, artificial ICE
  priorities and delayed TURN injection: no verified cross-network improvement
  and potential reliability/compatibility regressions.
- `IDXGIDevice1::SetMaximumFrameLatency(1)`: Moonlight's D3D11 renderer explicitly
  avoids this with SyncInterval 0 because DWM can introduce blocking. A generic
  frame-queue recipe is not sufficient evidence for this renderer.
- Global GPU process switches and Chromium flags: there is no Chromium runtime
  in this client.

## Network and host tuning

Prefer wired Ethernet and an uncongested UDP route. A TURN relay with a good
route can outperform a poor direct/VPN route; compare RTT, jitter and load rather
than assuming direct always wins. TURN/TCP/TLS remains a connectivity fallback
for restrictive networks, with head-of-line behavior during TCP loss. STUN does
not carry the session traffic and selecting a faster STUN server does not shorten
the established media path.

For controlled A/B testing, keep the host, resolution, refresh rate, bitrate,
codec and route fixed. Measure loaded RTT/jitter, rendering opportunities,
audio underruns and device padding. A high-speed input-to-display recording is
needed to validate full interaction latency; RTT alone is insufficient. No
fixed millisecond gain is claimed from CI or synthetic probes.

## Sources checked

- [Disable video processor automatic processing](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3d11videocontext-videoprocessorsetstreamautoprocessingmode)
- [Explicit D3D11 adapter selection](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-d3d11createdevice)
- [Transform input ownership](https://learn.microsoft.com/en-us/windows/win32/api/mftransform/nf-mftransform-imftransform-processinput)
- [RFC 8831: data-channel reliability, ordering, congestion and large-message interaction](https://www.rfc-editor.org/rfc/rfc8831.html)
- [Microsoft: low latency audio](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/low-latency-audio)
- [Supported shared-mode engine periods](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient3-getsharedmodeengineperiod)
- [InitializeSharedAudioStream flags and failure behavior](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient3-initializesharedaudiostream)
- [Microsoft: MMCSS](https://learn.microsoft.com/en-us/windows/win32/procthread/multimedia-class-scheduler-service)
- [Moonlight D3D11 renderer: buffer/presentation tradeoffs](https://github.com/moonlight-stream/moonlight-qt/blob/master/app/streaming/video/ffmpeg-renderers/d3d11va.cpp)
- [Selkies: remote-desktop network and latency troubleshooting](https://github.com/selkies-project/selkies/blob/main/docs/faq.md)

The external projects use different protocols/pipelines; their settings are
references, not interchangeable Parsec/WebRTC configuration values.
