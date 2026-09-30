const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const context = vm.createContext({});
vm.runInContext(fs.readFileSync(require('node:path').join(__dirname, '../web/stats.js'), 'utf8'), context);

function report({ timestamp = 1000, received = 100, sent = 50, relay = false, video = false, id = 'pair' } = {}) {
  const values = [
    { id: 'transport', type: 'transport', selectedCandidatePairId: id },
    { id, type: 'candidate-pair', state: 'succeeded', timestamp, bytesReceived: received, bytesSent: sent,
      localCandidateId: 'local', remoteCandidateId: 'remote', currentRoundTripTime: .024 },
    { id: 'local', type: 'local-candidate', candidateType: relay ? 'relay' : 'host', protocol: 'udp' },
    { id: 'remote', type: 'remote-candidate', candidateType: 'srflx' },
  ];
  if (video) values.push({id:'video',type:'inbound-rtp',kind:'video',codecId:'codec',frameWidth:1920,frameHeight:1080,framesPerSecond:60,packetsLost:2},
    {id:'codec',type:'codec',mimeType:'video/H264'});
  return new Map(values.map(value => [value.id, value]));
}

test('selected pair reports RTT, relay and actual traffic without RTP video', () => {
  const first = context.summarizeStats(report(), null);
  assert.equal(first.sample.inboundMbps, null);
  const second = context.summarizeStats(report({timestamp:2000, received:1000100, sent:250050, relay:true}), first.counters);
  assert.equal(second.sample.inboundMbps, 8);
  assert.equal(second.sample.outboundMbps, 2);
  assert.equal(second.sample.rttMs, 24);
  assert.equal(second.sample.route, 'relay');
  assert.equal(second.sample.fps, null);
  assert.equal(second.sample.codec, null);
  assert.equal(second.sample.decoder, null);
});

test('counter resets and pair changes do not produce false bandwidth', () => {
  const first = context.summarizeStats(report({received:1000}), null);
  assert.equal(context.summarizeStats(report({timestamp:2000,received:10}), first.counters).sample.inboundMbps, null);
  assert.equal(context.summarizeStats(report({timestamp:2000,id:'new-pair'}), first.counters).sample.inboundMbps, null);
  assert.equal(context.summarizeStats(report({timestamp:1000}), first.counters).sample.inboundMbps, null);
});

test('video details only appear when reported and remote relay is detected', () => {
  const data = report({video:true}); data.get('remote').candidateType = 'relay';
  const result = context.summarizeStats(data, null);
  assert.equal(result.sample.route, 'relay');
  assert.equal(result.sample.codec, 'video/H264');
  assert.equal(result.sample.fps, 60);
  assert.equal(result.sample.width, 1920);
});

test('missing metrics remain unknown rather than zero', () => {
  const result = context.summarizeStats(new Map(), null);
  for (const value of Object.values(result.sample)) assert.equal(value, null);
  assert.equal(context.aggregateStats([]).state, 'waiting');
});

test('aggregates traffic while selecting the most active connected peer for RTT', () => {
  const sample = (state, inboundMbps, rttMs) => ({state,sample:{inboundMbps,outboundMbps:1,rttMs}});
  const result = context.aggregateStats([sample('connected',3,20),sample('connected',2,30),sample('failed',100,100)]);
  assert.equal(result.inboundMbps,5); assert.equal(result.outboundMbps,2);
  assert.equal(result.rttMs,20); assert.equal(result.peerConnections,3);
});
