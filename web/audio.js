// Observe public WebCodecs inputs without changing configuration or frame ownership.
const decodedAudio = new Map();
if (window.AudioDecoder) {
  const NativeAudioDecoder = window.AudioDecoder;
  class MeasuredAudioDecoder extends NativeAudioDecoder {
    constructor(...args) {
      super(...args);
      decodedAudio.set(this, { bytes: 0, previous: null, lastInput: null, codec: null });
    }
    configure(...args) {
      const config = args[0], observed = {};
      if (config != null && ['object', 'function'].includes(typeof config)) {
        args[0] = new Proxy(Object.create(null), { get(_target, key) {
          const value = Reflect.get(config, key, config);
          if (['codec','sampleRate','numberOfChannels'].includes(key)) observed[key] = value;
          return value;
        } });
      }
      const result = super.configure(...args);
      const counter = decodedAudio.get(this);
      Object.assign(counter, { codec: observed.codec, sampleRate: observed.sampleRate,
        channels: observed.numberOfChannels, previous: null, lastInput: null });
      return result;
    }
    decode(...args) {
      const result = super.decode(...args);
      const counter = decodedAudio.get(this);
      counter.bytes += args[0].byteLength;
      counter.lastInput = performance.now();
      return result;
    }
    reset() {
      const result = super.reset();
      decodedAudio.get(this).previous = null;
      return result;
    }
    close() {
      const result = super.close();
      decodedAudio.delete(this);
      return result;
    }
  }
  window.AudioDecoder = MeasuredAudioDecoder;
}
function sampleDecodedAudio(now) {
  const active = [];
  for (const [decoder, counter] of decodedAudio) {
    if (decoder.state === 'closed') { decodedAudio.delete(decoder); continue; }
    if (decoder.state !== 'configured' || counter.lastInput == null || now - counter.lastInput > 3000) {
      counter.previous = null; continue;
    }
    const previous = counter.previous;
    counter.previous = { now, bytes: counter.bytes };
    active.push({ ...counter, bitrate: previous && now > previous.now ?
      (counter.bytes - previous.bytes) * 8 / (now - previous.now) : null });
  }
  if (active.length !== 1) return null; // Do not attribute mixed decoders to one stream.
  const audio = active[0];
  return { audioCodec: typeof audio.codec === 'string' && /^[A-Za-z0-9_.-]{1,128}$/.test(audio.codec) ? audio.codec : null,
    audioBitrateKbps: audio.bitrate,
    audioSampleRate: Number.isInteger(audio.sampleRate) && audio.sampleRate > 0 && audio.sampleRate <= 768000 ? audio.sampleRate : null,
    audioChannels: Number.isInteger(audio.channels) && audio.channels > 0 && audio.channels <= 32 ? audio.channels : null,
    audioSource: 'WebCodecs encoded audio' };
}
