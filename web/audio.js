// Observe public WebCodecs configuration, including while the stream is silent.
const decodedAudio = new Map();
if (window.AudioDecoder) {
  const NativeAudioDecoder = window.AudioDecoder;
  class MeasuredAudioDecoder extends NativeAudioDecoder {
    constructor(...args) {
      super(...args);
      decodedAudio.set(this, { codec: null });
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
        channels: observed.numberOfChannels });
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
function sampleDecodedAudio() {
  const active = [];
  for (const [decoder, counter] of decodedAudio) {
    if (decoder.state === 'closed') { decodedAudio.delete(decoder); continue; }
    if (decoder.state !== 'configured') continue;
    active.push(counter);
  }
  if (active.length !== 1) return null; // Do not attribute mixed decoders to one stream.
  const audio = active[0];
  return { audioCodec: typeof audio.codec === 'string' && /^[A-Za-z0-9_.-]{1,128}$/.test(audio.codec) ? audio.codec : null,
    // AudioDecoderConfig does not expose the remote encoder's target bitrate.
    audioBitrateKbps: null,
    audioSampleRate: Number.isInteger(audio.sampleRate) && audio.sampleRate > 0 && audio.sampleRate <= 768000 ? audio.sampleRate : null,
    audioChannels: Number.isInteger(audio.channels) && audio.channels > 0 && audio.channels <= 32 ? audio.channels : null,
    audioSource: 'WebCodecs decoder configuration' };
}
