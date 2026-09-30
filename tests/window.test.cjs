const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const script = fs.readFileSync(require('node:path').join(__dirname,'../web/window.js'),'utf8');
test('Parsec cannot enter fullscreen automatically or lock recovery keys', async () => {
  const keys=[];
  class Element { requestFullscreen(){throw new Error('Unexpected automatic fullscreen');} }
  const context=vm.createContext({location:{origin:'https://web.parsec.app'},Element,DOMException,
    navigator:{keyboard:{lock:async value=>keys.push(...value)}}});
  vm.runInContext(script,context);
  await assert.rejects(new Element().requestFullscreen(),{name:'NotAllowedError'});
  await context.navigator.keyboard.lock(['Escape','F11','KeyA']);
  assert.deepEqual(keys,['KeyA']);
});
