const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
test('audio monitoring measures encoded inputs and preserves native configuration and ownership',()=>{
  let now=0;
  class Decoder {
    constructor(init){this.init=init;this.state='unconfigured';}
    configure(config){this.config={codec:config.codec,sampleRate:config.sampleRate,numberOfChannels:config.numberOfChannels};this.state='configured';}
    decode(chunk){if(chunk.invalid)throw Error('Native failure');this.chunk=chunk;}
    reset(){this.state='unconfigured';}
    close(){this.state='closed';}
  }
  const window={AudioDecoder:Decoder};
  const context=vm.createContext({window,performance:{now:()=>now}});
  vm.runInContext(fs.readFileSync(require('node:path').join(__dirname,'../web/audio.js'),'utf8'),context);
  const output=()=>{}, decoder=new window.AudioDecoder({output,error:()=>{}});
  decoder.configure(Object.freeze(Object.create({codec:'opus',sampleRate:48000,numberOfChannels:2})));
  assert.equal(decoder.init.output,output);
  const sample=()=>vm.runInContext(`sampleDecodedAudio(${now})`,context);
  decoder.decode({byteLength:8000});assert.equal(sample().audioBitrateKbps,null);
  now=1000;decoder.decode({byteLength:8000});assert.equal(sample().audioBitrateKbps,64);
  assert.equal(sample().audioCodec,'opus');assert.equal(sample().audioSampleRate,48000);assert.equal(sample().audioChannels,2);
  assert.throws(()=>decoder.decode({invalid:true,byteLength:1e9}));
  now=5000;assert.equal(sample(),null,'Stopped audio must not retain current bitrate');
  decoder.reset();assert.equal(sample(),null);decoder.close();assert.equal(sample(),null);
});
