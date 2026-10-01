// Count actual decoded video frames, including Parsec's WebCodecs/data-channel path.
// The original output callback retains ownership of each VideoFrame.
const decodedVideos = new Map();
if (window.VideoDecoder) {
  const NativeVideoDecoder = window.VideoDecoder;
  class MeasuredVideoDecoder extends NativeVideoDecoder {
    constructor(...args) {
      const counter = { frames: 0, previous: null };
      const init = args[0];
      if (init != null && ['object', 'function'].includes(typeof init)) {
        // Preserve native WebIDL member reads, inherited callbacks, getter order
        // and errors. Capture the callback at construction, as the native API does.
        args[0] = new Proxy(Object.create(null), {
          get(_target, key) {
            const value = Reflect.get(init, key, init);
            if (key !== 'output' || typeof value !== 'function') return value;
            return function(frame) {
              'use strict';
              counter.frames++;
              return Reflect.apply(value, this, [frame]);
            };
          },
        });
      }
      super(...args);
      decodedVideos.set(this, counter);
    }
    reset() {
      const result = super.reset();
      decodedVideos.get(this).previous = null;
      return result;
    }
    close() {
      const result = super.close();
      decodedVideos.delete(this);
      return result;
    }
  }
  window.VideoDecoder = MeasuredVideoDecoder;
}
function sampleDecodedFps(now) {
  let fps = null;
  for (const [decoder, counter] of decodedVideos) {
    if (decoder.state === 'closed') { decodedVideos.delete(decoder); continue; }
    if (decoder.state !== 'configured') { counter.previous = null; continue; }
    const previous = counter.previous;
    counter.previous = { now, frames: counter.frames };
    if (previous && now > previous.now) {
      const value = (counter.frames - previous.frames) * 1000 / (now - previous.now);
      fps = Math.max(fps ?? 0, value);
    }
  }
  return fps;
}
