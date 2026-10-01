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
  const partial = report(); partial.delete('remote');
  assert.equal(context.summarizeStats(partial, null).sample.route, null);
});

test('encryption comes from the selected transport without certificates or TURN TLS inference', () => {
  const data=report({relay:true});
  Object.assign(data.get('local'),{relayProtocol:'tls'});
  data.set('unused',{id:'unused',type:'transport',dtlsState:'connected',dtlsCipher:'UNUSED'});
  Object.assign(data.get('transport'),{dtlsState:'connected',tlsVersion:'FEFD',
    dtlsCipher:'TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256',localCertificateId:'secret-cert'});
  data.set('secret-cert',{type:'certificate',base64Certificate:'private-certificate'});
  const sample=context.summarizeStats(data,null).sample;
  assert.equal(sample.dtlsState,'connected');assert.equal(sample.tlsVersion,'FEFD');
  assert.equal(sample.dtlsCipher,'TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256');
  assert.equal(sample.srtpCipher,null,'Data channels do not require SRTP');
  assert.ok(!JSON.stringify(sample).includes('private-certificate'));
  delete data.get('transport').dtlsCipher;delete data.get('transport').tlsVersion;
  assert.equal(context.summarizeStats(data,null).sample.dtlsCipher,null,'TURN TLS is not a DTLS cipher');
  Object.assign(data.get('transport'),{srtpCipher:'SRTP_AEAD_AES_128_GCM',dtlsCipher:'bad\nvalue',tlsVersion:'wrong'});
  const invalid=context.summarizeStats(data,null).sample;
  assert.equal(invalid.srtpCipher,'SRTP_AEAD_AES_128_GCM');assert.equal(invalid.dtlsCipher,null);assert.equal(invalid.tlsVersion,null);
});

test('ambiguous transports do not guess encryption and pair transportId links are respected', () => {
  const data=report();delete data.get('transport').selectedCandidatePairId;
  data.get('pair').nominated=true;data.get('pair').transportId='transport';
  data.get('transport').dtlsState='connected';
  data.set('other',{id:'other',type:'transport',dtlsState:'failed'});
  assert.equal(context.summarizeStats(data,null).sample.dtlsState,'connected');
  delete data.get('pair').transportId;
  assert.equal(context.summarizeStats(data,null).sample.dtlsState,null);
});

test('RTP audio codec metadata survives silence and payload rate is not a configured bitrate', () => {
  const data=report();
  data.set('audio',{id:'audio',type:'inbound-rtp',kind:'audio',codecId:'opus',timestamp:1000,bytesReceived:100});
  data.set('opus',{type:'codec',mimeType:'audio/opus',clockRate:48000,channels:2});
  const first=context.summarizeStats(data,null);
  Object.assign(data.get('audio'),{timestamp:2000,bytesReceived:8100});
  const sample=context.summarizeStats(data,first.counters).sample;
  assert.equal(sample.audioCodec,'audio/opus');assert.equal(sample.audioBitrateKbps,null);
  assert.equal(sample.audioChannels,2);assert.equal(sample.audioSource,'WebRTC inbound audio codec');
  data.get('audio').bytesReceived=0;
  assert.equal(context.summarizeStats(data,first.counters).sample.audioBitrateKbps,null);
  assert.equal(context.summarizeStats(data,first.counters).sample.audioCodec,'audio/opus');
});

test('aggregates traffic while selecting the most active connected peer for RTT', () => {
  const sample = (state, inboundMbps, rttMs) => ({state,sample:{inboundMbps,outboundMbps:1,rttMs}});
  const result = context.aggregateStats([sample('connected',3,20),sample('connected',2,30),sample('failed',100,100)]);
  assert.equal(result.inboundMbps,5); assert.equal(result.outboundMbps,2);
  assert.equal(result.rttMs,20); assert.equal(result.peerConnections,3);
});

test('ambiguous nominated pairs never guess direct or relay', () => {
  const data = report();
  data.get('transport').selectedCandidatePairId = undefined;
  data.get('pair').nominated = true;
  data.set('other', {...data.get('pair'),id:'other'});
  assert.equal(context.summarizeStats(data, null).sample.route, null);
  data.get('other').selected = true;
  data.get('local').candidateType = 'relay';
  assert.equal(context.summarizeStats(data,null).sample.route,null,'Nonstandard selected flag must not select a pair');
  data.get('transport').selectedCandidatePairId='other';
  const selected = context.summarizeStats(data, null).sample;
  assert.equal(selected.route, 'relay');
  assert.equal(selected.localCandidateType, 'relay');
  assert.equal(selected.remoteCandidateType, 'srflx');
});

test('observed peer-reflexive pairs cannot prove direct routing', () => {
  for (const [local,remote] of [['prflx','prflx'],['prflx','host'],['host','prflx']]) {
    const data=report();data.get('local').candidateType=local;data.get('remote').candidateType=remote;
    const result=context.summarizeStats(data,null).sample;
    assert.equal(result.route,null);
    assert.equal(result.rttMs,24);
    assert.equal(result.localCandidateType,local);
    assert.equal(result.remoteCandidateType,remote);
  }
  const data=report();data.get('local').candidateType='prflx';data.get('remote').candidateType='relay';
  assert.equal(context.summarizeStats(data,null).sample.route,'relay');
});

test('Chromium peer-reflexive TURN path is confirmed and matched to configured server', () => {
  const data=report();
  Object.assign(data.get('local'),{candidateType:'prflx',relayProtocol:'tls',url:'turns:relay.example:5349?transport=tcp'});
  data.get('remote').candidateType='prflx';
  const servers=[{urls:['stun:relay.example','turns:RELAY.example'],username:'private-user',credential:'private-secret'}];
  const sample=context.summarizeStats(data,null,servers).sample;
  assert.equal(sample.route,'relay');assert.equal(sample.configuredTurnUsed,true);
  assert.equal(sample.turnServer,'turns:RELAY.example');assert.equal(sample.turnProtocol,'tls');
  assert.equal(sample.protocol,'udp');assert.equal(sample.routeEvidence,'Selected local TURN transport');
  assert.ok(!JSON.stringify(sample).includes('private-'));
  data.get('local').url='turns:another.example:5349?transport=tcp';
  const other=context.summarizeStats(data,null,servers).sample;
  assert.equal(other.route,'relay');assert.equal(other.configuredTurnUsed,false);assert.equal(other.turnServer,null);
  delete data.get('local').url;
  assert.equal(context.summarizeStats(data,null,servers).sample.configuredTurnUsed,null);
});

test('available but unselected TURN candidates and TURN STUN binding do not imply relay use', () => {
  const data=report();
  data.set('unused-relay',{id:'unused-relay',type:'local-candidate',candidateType:'relay',relayProtocol:'tls',url:'turns:relay.example'});
  data.get('local').candidateType='srflx';data.get('local').url='turn:relay.example';
  const servers=[{urls:['turn:relay.example']}];
  const direct=context.summarizeStats(data,null,servers).sample;
  assert.equal(direct.route,'direct');assert.equal(direct.configuredTurnUsed,false);assert.equal(direct.turnServer,null);
  data.get('local').candidateType='prflx';
  const unknown=context.summarizeStats(data,null,servers).sample;
  assert.equal(unknown.route,null);assert.equal(unknown.configuredTurnUsed,null);
});

test('selected ICE transport pair supplies route when report selection is ambiguous', () => {
  const data=report();delete data.get('transport').selectedCandidatePairId;
  data.get('pair').nominated=true;data.set('other',{...data.get('pair'),id:'other'});
  const selected={local:{type:'prflx',protocol:'udp',relayProtocol:'tcp',url:'turn:relay.example:3478?transport=tcp'},remote:{type:'host',protocol:'udp'}};
  const sample=context.summarizeStats(data,null,[{urls:['turn:relay.example?transport=tcp']}],selected).sample;
  assert.equal(sample.route,'relay');assert.equal(sample.configuredTurnUsed,true);assert.equal(sample.turnProtocol,'tcp');
  assert.equal(sample.rttMs,null,'Route fallback must not attribute another candidate pair RTT');
});

test('remote-only relay does not claim our configured TURN server is used', () => {
  const data=report();data.get('remote').candidateType='relay';
  const sample=context.summarizeStats(data,null,[{urls:['turn:relay.example']}]).sample;
  assert.equal(sample.route,'relay');assert.equal(sample.configuredTurnUsed,false);
  assert.equal(sample.routeEvidence,'Selected remote relay candidate');
});

function endpoints(data) {
  Object.assign(data.get('local'), {candidateType:'prflx',address:'10.8.0.2',port:5100,protocol:'udp',usernameFragment:'generation'});
  Object.assign(data.get('remote'), {candidateType:'prflx',address:'10.8.0.3',port:5200,protocol:'udp',usernameFragment:'remote-generation'});
  return {local:{...data.get('local'),type:'host'},remote:{...data.get('remote'),type:'host'}};
}

test('matching selected transport endpoints confirm non-TURN VPN paths without exporting addresses', () => {
  const data=report();const selected=endpoints(data);
  const sample=context.summarizeStats(data,null,[],selected).sample;
  assert.equal(sample.route,'direct');assert.equal(sample.configuredTurnUsed,false);
  assert.equal(sample.routeEvidence,'Selected endpoints matched non-relay ICE candidates');
  assert.equal(sample.localCandidateType,'prflx','Preserve the raw diagnostic type');
  assert.equal(sample.remoteCandidateType,'prflx');
  assert.equal(sample.rttMs,24);
  assert.ok(!JSON.stringify(sample).includes('10.8.'));
  assert.ok(!JSON.stringify(sample).includes('generation'));
});

test('exact gathered endpoint matches can resolve peer-reflexive routes', () => {
  const data=report();const selected=endpoints(data);
  const candidates={local:[selected.local],remote:[selected.remote]};
  assert.equal(context.summarizeStats(data,null,[],null,candidates).sample.route,'direct');
  candidates.local.push({type:'relay',address:'203.0.113.5',port:6000,protocol:'udp'});
  assert.equal(context.summarizeStats(data,null,[],null,candidates).sample.route,'direct','Unused TURN does not change the selected route');
  candidates.remote=[];
  assert.equal(context.summarizeStats(data,null,[],null,candidates).sample.route,null,'A local match alone does not prove both ends');
});

test('mismatched pair snapshots and incomplete endpoints never prove direct', () => {
  for(const mutate of [pair=>pair.remote.port++,pair=>pair.local.protocol='tcp',
    pair=>pair.local.usernameFragment='old-generation',pair=>delete pair.remote.address]) {
    const data=report();const selected=endpoints(data);mutate(selected);
    assert.equal(context.summarizeStats(data,null,[],selected).sample.route,null);
  }
  const data=report();const selected=endpoints(data);
  delete data.get('remote').address;
  assert.equal(context.summarizeStats(data,null,[],selected).sample.route,null);
});

test('positive TURN evidence wins over non-relay endpoint correlation', () => {
  const data=report();const selected=endpoints(data);
  Object.assign(data.get('local'),{relayProtocol:'tls',url:'turns:relay.example'});
  const sample=context.summarizeStats(data,null,[{urls:['turns:relay.example']}],selected).sample;
  assert.equal(sample.route,'relay');assert.equal(sample.configuredTurnUsed,true);
  assert.equal(sample.routeEvidence,'Selected local TURN transport');
  delete data.get('local').relayProtocol;delete data.get('local').url;
  const candidates={local:[selected.local,{...selected.local,type:'relay',relayProtocol:'tls',url:'turns:relay.example'}],remote:[selected.remote]};
  assert.equal(context.summarizeStats(data,null,[],null,candidates).sample.route,'relay');
});

test('an exact gathered match with unknown origin stays unverified', () => {
  const data=report();const selected=endpoints(data);
  const candidates={local:[selected.local,{...selected.local,type:'prflx'}],remote:[selected.remote]};
  assert.equal(context.summarizeStats(data,null,[],null,candidates).sample.route,null);
  candidates.local=[{...selected.local,usernameFragment:'previous-generation'}];
  assert.equal(context.summarizeStats(data,null,[],null,candidates).sample.route,null);
});
