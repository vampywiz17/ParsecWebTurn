const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const source = fs.readFileSync(path.join(__dirname, '../scripts/test-direct-first-console.js'), 'utf8');
function setup() {
  const servers = [{ urls: ['stun:example.invalid', 'turn:example.invalid'], username:'private-user', credential:'private-password' }];
  class Native extends EventTarget {
    iceConnectionState = 'new';
    iceGatheringState = 'new';
    offers = 0;
    restarts = 0;
    constructor(config = {}) { super(); this.config = { iceTransportPolicy:'all', iceCandidatePoolSize:0, ...config }; }
    getConfiguration() { return structuredClone(this.config); }
    setConfiguration(config) { this.config = structuredClone(config); }
    restartIce() { this.restarts++; this.dispatchEvent(new Event('negotiationneeded')); }
    createOffer() { this.offers++; return Promise.resolve({type:'offer',sdp:'a=ice-ufrag:test'}); }
    setLocalDescription(value) { this.localDescription=value; return Promise.resolve(); }
    setRemoteDescription(value) { this.remoteDescription=value; return Promise.resolve(); }
    getStats() { return Promise.resolve(new Map()); }
    state(value) { this.iceConnectionState=value;this.dispatchEvent(new Event('iceconnectionstatechange')); }
  }
  class Patched extends Native {
    constructor(config) { super({...config, iceServers:servers}); }
    setConfiguration(config) { super.setConfiguration({...config,iceServers:servers}); }
  }
  const window={location:{origin:'https://web.parsec.app'},__parsecWebTurnPatched:true,RTCPeerConnection:Patched};
  const context=vm.createContext({window,performance:{now:()=>0},console:{log:()=>{}}});
  vm.runInContext(source,context);
  return {window,servers,newPeer:config=>new window.RTCPeerConnection(config)};
}
test('starts STUN-only and preserves policy; nonterminal states cannot trigger fallback', () => {
  const {window,newPeer}=setup(); const peer=newPeer({bundlePolicy:'max-bundle'});
  assert.deepEqual(peer.getConfiguration().iceServers[0].urls,['stun:example.invalid']);
  assert.equal(peer.getConfiguration().bundlePolicy,'max-bundle');
  for(const state of ['checking','connected','disconnected','completed'])peer.state(state);
  assert.equal(peer.restarts,0);assert.equal(window.parsecDirectFirstTest.records.some(r=>r.event==='TURN-added'),false);
});
test('terminal failure restores TURN and requests restart, but never fabricates negotiation', () => {
  const {window,servers,newPeer}=setup(); const peer=newPeer(); peer.state('failed');peer.state('failed');
  assert.deepEqual(peer.getConfiguration().iceServers,servers);assert.equal(peer.restarts,1);assert.equal(peer.offers,0);
  const log=JSON.stringify(window.parsecDirectFirstTest.records);
  assert.match(log,/negotiationneeded/);assert.doesNotMatch(log,/private-user|private-password|example.invalid/);
});
test('another checking peer prevents premature fallback; an automatic new attempt gets TURN', () => {
  const {newPeer,servers}=setup();const a=newPeer(),b=newPeer();a.state('failed');b.state('checking');
  assert.equal(a.restarts,0);b.state('failed');assert.equal(a.restarts,1);assert.equal(b.restarts,1);
  assert.deepEqual(newPeer().getConfiguration().iceServers,servers);
});
test('explicit relay-only and pooled peers are excluded without changing their configuration', () => {
  for(const config of [{iceTransportPolicy:'relay'},{iceCandidatePoolSize:2}]) {
    const {newPeer,servers}=setup();const peer=newPeer(config);peer.state('failed');
    assert.deepEqual(peer.getConfiguration().iceServers,servers);assert.equal(peer.restarts,0);
  }
});
test('client-driven descriptions report credential changes without exposing SDP', async () => {
  const {newPeer,window}=setup(); const peer=newPeer();
  await peer.setLocalDescription({sdp:'a=ice-ufrag:secret-one'});
  peer.state('failed');await peer.setLocalDescription({sdp:'a=ice-ufrag:secret-two'});
  const log=window.parsecDirectFirstTest.records.filter(r=>r.event==='local-description-set');
  assert.equal(log[0].iceCredentialsChanged,false);assert.equal(log[1].iceCredentialsChanged,true);
  assert.doesNotMatch(JSON.stringify(await window.parsecDirectFirstTest.snapshot()),/secret-one|secret-two|private-password/);
});
