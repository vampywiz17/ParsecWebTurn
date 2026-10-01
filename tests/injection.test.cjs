const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function setup({ available = true, legacy = true, origin = 'https://web.parsec.app' } = {}) {
  const logs = [];
  function readDictionary(config) {
    if (config != null && !['object','function'].includes(typeof config)) throw new TypeError('Invalid dictionary');
    const result={iceServers:[],iceTransportPolicy:'all',bundlePolicy:'balanced'};
    for (const key of ['bundlePolicy','certificates','iceCandidatePoolSize','iceServers','iceTransportPolicy','rtcpMuxPolicy']) {
      const value=config?.[key];if(value!==undefined)result[key]=value;
    }
    return structuredClone(result);
  }
  class NativePeerConnection {
    static generateCertificate() { return 'certificate'; }
    constructor(config, constraints) { this.config = readDictionary(config); this.constraints = constraints; }
    setConfiguration(config) { this.config = readDictionary(config); }
    getConfiguration() { return structuredClone(this.config); }
  }
  const window = available ? { RTCPeerConnection: NativePeerConnection } : {};
  if (available && legacy) window.webkitRTCPeerConnection = NativePeerConnection;
  window.location = { origin };
  const context = vm.createContext({ window, setInterval: () => 1, console: { log: (...args) => logs.push(args), error: (...args) => logs.push(args) } });
  const servers = [{ urls: ['turn:example.invalid:3478'], username: 'test-user', credential: 'test-secret' }];
  const script = fs.readFileSync(path.join(__dirname, '../web/inject.js'), 'utf8')
    .replace('__STATS_HELPER__', fs.readFileSync(path.join(__dirname, '../web/stats.js'), 'utf8'))
    .replace('__VIDEO_HELPER__', fs.readFileSync(path.join(__dirname, '../web/video.js'), 'utf8'))
    .replace('__ICE_SERVERS__', JSON.stringify(servers));
  vm.runInContext(script, context);
  return { window, logs, context, script, NativePeerConnection, servers };
}

test('constructor preserves policy, constraints, inheritance and static methods', () => {
  const { window, NativePeerConnection, servers } = setup();
  const input = { iceServers: [{ urls: 'stun:original.invalid' }], iceTransportPolicy: 'all', bundlePolicy: 'max-bundle' };
  const pc = new window.RTCPeerConnection(input, 'constraints');
  assert.deepEqual(pc.getConfiguration().iceServers, servers);
  assert.equal(pc.config.iceTransportPolicy, 'all');
  assert.equal(pc.config.bundlePolicy, 'max-bundle');
  assert.equal(pc.constraints, 'constraints');
  assert.ok(pc instanceof NativePeerConnection);
  assert.ok(pc instanceof window.RTCPeerConnection);
  assert.equal(window.RTCPeerConnection.generateCertificate(), 'certificate');
  assert.equal(input.iceServers[0].urls, 'stun:original.invalid');
  assert.equal(window.webkitRTCPeerConnection, NativePeerConnection,'The vendor-prefixed alias is left untouched');
});

test('later setConfiguration cannot replace the ICE override', () => {
  const { window, servers } = setup();
  const pc = new window.RTCPeerConnection();
  const input = { iceServers: [], iceTransportPolicy: 'relay' };
  pc.setConfiguration(input);
  assert.deepEqual(pc.getConfiguration().iceServers, servers);
  assert.equal(pc.config.iceTransportPolicy, 'relay');
  assert.deepEqual(input.iceServers, []);
});

test('duplicate injection is ignored and credentials never enter logs', () => {
  const { window, context, script, logs } = setup();
  const first = window.RTCPeerConnection;
  new first();
  vm.runInContext(script, context);
  assert.equal(window.RTCPeerConnection, first);
  assert.ok(!JSON.stringify(logs).includes('test-secret'));
  assert.ok(!JSON.stringify(logs).includes('test-user'));
});

test('missing WebRTC does not mark the patch as installed', () => {
  const { window } = setup({ available: false });
  assert.equal(window.__parsecWebTurnPatched, undefined);
});

test('absent legacy alias is not introduced; null and omitted configs work', () => {
  const { window, servers } = setup({ legacy: false });
  assert.equal(window.webkitRTCPeerConnection, undefined);
  for (const config of [undefined, null]) {
    assert.deepEqual(new window.RTCPeerConnection(config).getConfiguration().iceServers, servers);
  }
});

test('credentials are not installed on unrelated origins', () => {
  const { window, NativePeerConnection } = setup({ origin: 'https://example.invalid' });
  assert.equal(window.RTCPeerConnection, NativePeerConnection);
  assert.equal(window.__parsecWebTurnPatched, undefined);
});

test('dictionary adapter preserves inherited/non-enumerable fields and getter receiver', () => {
  const {window,servers}=setup();
  const inherited={iceTransportPolicy:'relay',bundlePolicy:'max-bundle'};
  const input=Object.create(inherited);
  Object.defineProperty(input,'iceCandidatePoolSize',{value:2});
  Object.defineProperty(input,'certificates',{get(){assert.equal(this,input);return [];}});
  Object.defineProperty(input,'unrelated',{enumerable:true,get(){throw Error('Unknown field must not be read');}});
  const pc=new window.RTCPeerConnection(input);
  assert.equal(pc.config.iceTransportPolicy,'relay');assert.equal(pc.config.bundlePolicy,'max-bundle');
  assert.equal(pc.config.iceCandidatePoolSize,2);assert.deepEqual(pc.config.iceServers,servers);
  pc.setConfiguration(input);assert.equal(pc.config.iceTransportPolicy,'relay');
});

test('frozen input is supported, original ICE getter is not evaluated, and native errors survive', () => {
  const {window,servers}=setup();
  const input=Object.freeze({iceTransportPolicy:'relay',get iceServers(){throw Error('Replaced field must not be read');}});
  const pc=new window.RTCPeerConnection(input);
  assert.deepEqual(pc.config.iceServers,servers);assert.equal(pc.config.iceTransportPolicy,'relay');
  for(const value of [42,true,'invalid',Symbol('invalid')]) {
    assert.throws(()=>new window.RTCPeerConnection(value),TypeError);
    assert.throws(()=>pc.setConfiguration(value),TypeError);
  }
  const failure=new Error('Original getter failure');
  assert.throws(()=>new window.RTCPeerConnection({get bundlePolicy(){throw failure;}}),error=>error===failure);
});
