const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const script = fs.readFileSync(require('node:path').join(__dirname,'../web/window.js'),'utf8');
test('Parsec cannot enter fullscreen automatically or lock recovery keys', async () => {
  const keys=[];
  class Element { requestFullscreen(){throw new Error('Unexpected automatic fullscreen');} }
  const context=vm.createContext({location:{origin:'https://web.parsec.app'},Element,DOMException,
    window:{addEventListener(){}},
    navigator:{keyboard:{lock:async value=>keys.push(...value)}}});
  vm.runInContext(script,context);
  await assert.rejects(new Element().requestFullscreen(),{name:'NotAllowedError'});
  await context.navigator.keyboard.lock(['Escape','F11','KeyA']);
  assert.deepEqual(keys,['KeyA']);
});

test('window recovery captures the shortcut before Parsec and ignores repeats', async () => {
  const handlers={},calls=[];
  const window={addEventListener(type,handler,capture){assert.equal(capture,true);handlers[type]=handler;},
    __TAURI__:{core:{invoke:async(command,args)=>calls.push({command,...args})}}};
  vm.runInNewContext(script,{location:{origin:'https://web.parsec.app'},Element:{prototype:{}},navigator:{},window});
  const event={code:'KeyW',ctrlKey:true,shiftKey:true,preventDefault(){this.prevented=true;},stopImmediatePropagation(){this.stopped=true;}};
  handlers.keydown(event);handlers.keydown({...event,repeat:true});handlers.keyup(event);
  await new Promise(resolve=>setImmediate(resolve));
  assert.equal(event.prevented,true);assert.equal(event.stopped,true);
  assert.deepEqual(calls,[{command:'parsec_window_shortcut',toggle:false}]);
  handlers.keydown({...event,code:'F11',ctrlKey:false,shiftKey:false});
  await new Promise(resolve=>setImmediate(resolve));
  assert.equal(calls[1].toggle,true);
});
