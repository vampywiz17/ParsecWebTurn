# Synthetic H.264 decode fixture

Generated locally from FFmpeg's synthetic testsrc2 filter; no captured media,
account data, credentials or external video source. Eight 64x64 intra pictures.
The fixture is test input, not third-party program code. FFmpeg is not shipped.

Generation:
```
ffmpeg -f lavfi -i testsrc2=size=64x64:rate=30 -frames:v 8 -c:v libx264 -pix_fmt yuv420p -profile:v high -preset ultrafast -tune zerolatency -x264-params keyint=1:repeat-headers=1:aud=1 -f h264 synthetic-64x64.h264
```

The native hardware probe splits on access unit delimiters, runs the actual
Media Foundation/D3D11 path, and optionally samples the synthetic GPU backbuffer
once to verify pixel variation. Readback is disabled in live sessions.
