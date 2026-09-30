const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const script=fs.readFileSync(require('node:path').join(__dirname,'../web/video.js'),'utf8');
test('WebCodecs FPS measures delivered frames without changing their ownership or decoder settings',()=>{
  class Decoder {
    static isConfigSupported(config){return config;}
    constructor(init){this.init=init;this.state='unconfigured';}
    configure(config){this.config=config;this.state='configured';}
    reset(){this.state='unconfigured';}
    close(){this.state='closed';}
  }
  const window={VideoDecoder:Decoder};
  const context=vm.createContext({window});vm.runInContext(script,context);
  const delivered=[],error=()=>{};
  const decoder=new window.VideoDecoder({output:frame=>delivered.push(frame),error});
  const config={codec:'avc1.42001e',hardwareAcceleration:'prefer-hardware'};
  decoder.configure(config);assert.equal(decoder.config,config);assert.equal(decoder.init.error,error);
  assert.ok(decoder instanceof Decoder);assert.equal(window.VideoDecoder.isConfigSupported(config),config);
  assert.equal(vm.runInContext('sampleDecodedFps(0)',context),null);
  const frame={close(){throw Error('Instrumentation must not close video frames');}};
  for(let i=0;i<60;i++)decoder.init.output(frame);
  assert.equal(vm.runInContext('sampleDecodedFps(2000)',context),30);assert.equal(delivered[0],frame);
  assert.equal(vm.runInContext('sampleDecodedFps(3000)',context),0);
  decoder.reset();decoder.configure(config);decoder.init.output(frame);
  assert.equal(vm.runInContext('sampleDecodedFps(4000)',context),null);
  for(let i=0;i<30;i++)decoder.init.output(frame);
  assert.equal(vm.runInContext('sampleDecodedFps(5000)',context),30);
  decoder.close();assert.equal(vm.runInContext('sampleDecodedFps(6000)',context),null);
});
