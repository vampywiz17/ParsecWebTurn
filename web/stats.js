// Pure getStats normalization, shared by the webview and regression tests.
function turnEndpoint(value) {
  if (typeof value !== 'string' || value.length > 512) return null;
  const match = /^(turns?):(\[[0-9a-f:.]+\]|[a-z0-9.-]+)(?::([0-9]+))?(?:\?transport=(udp|tcp))?$/i.exec(value);
  if (!match) return null;
  const scheme = match[1].toLowerCase();
  const port = Number(match[3] || (scheme === 'turns' ? 5349 : 3478));
  if (port < 1 || port > 65535) return null;
  return `${scheme}:${match[2].toLowerCase()}:${port}?transport=${(match[4] || (scheme === 'turns' ? 'tcp' : 'udp')).toLowerCase()}`;
}

function routeDetails(local, remote, servers = []) {
  const turnProtocol = ['udp', 'tcp', 'tls'].includes(local?.relayProtocol) ? local.relayProtocol : null;
  // Chromium preserves relayProtocol when a local TURN candidate becomes prflx.
  // A TURN URL alone is insufficient: that server may also provide STUN binding.
  const localRelay = local?.candidateType === 'relay' || turnProtocol != null;
  const remoteRelay = remote?.candidateType === 'relay';
  const direct = ['host', 'srflx'].includes(local?.candidateType) && ['host', 'srflx'].includes(remote?.candidateType);
  const endpoint = localRelay && turnEndpoint(local?.url);
  const configured = endpoint && servers.flatMap(server => typeof server.urls === 'string' ? [server.urls] : server.urls || [])
    .find(url => turnEndpoint(url) === endpoint);
  return {
    route: localRelay || remoteRelay ? 'relay' : direct ? 'direct' : null,
    routeEvidence: turnProtocol ? 'Selected local TURN transport' : localRelay ? 'Selected local relay candidate' :
      remoteRelay ? 'Selected remote relay candidate' : direct ? 'Selected non-relay candidates' : null,
    turnProtocol,
    configuredTurnUsed: localRelay ? (endpoint ? !!configured : null) :
      (['host', 'srflx'].includes(local?.candidateType) ? false : null),
    // Only return an already configured server URL, never arbitrary candidate addresses.
    turnServer: configured || null,
  };
}

function summarizeStats(report, previous, servers = [], selectedPair = null) {
  const entries = [...report.values()];
  const transport = entries.find(entry => entry.type === 'transport' && entry.selectedCandidatePairId);
  const nominated = entries.filter(entry => entry.type === 'candidate-pair' && entry.nominated && entry.state === 'succeeded');
  const pair = (transport && report.get(transport.selectedCandidatePairId)) ||
    entries.find(entry => entry.type === 'candidate-pair' && entry.selected === true && entry.state === 'succeeded') ||
    (nominated.length === 1 ? nominated[0] : null);
  const counter = pair || entries.find(entry => entry.type === 'transport' && Number.isFinite(entry.bytesReceived));
  // Use the ICE transport's selected pair only if getStats cannot identify one.
  const local = pair ? report.get(pair.localCandidateId) : selectedPair?.local && {
    candidateType: selectedPair.local.type, protocol: selectedPair.local.protocol,
    relayProtocol: selectedPair.local.relayProtocol, url: selectedPair.local.url,
  };
  const remote = pair ? report.get(pair.remoteCandidateId) : selectedPair?.remote && {
    candidateType: selectedPair.remote.type, protocol: selectedPair.remote.protocol,
  };
  const video = entries.filter(entry => entry.type === 'inbound-rtp' && (entry.kind || entry.mediaType) === 'video')
    .sort((a, b) => (b.bytesReceived || 0) - (a.bytesReceived || 0))[0];
  const codec = video && report.get(video.codecId);
  const finite = value => Number.isFinite(value) && value >= 0 ? value : null;
  const counters = counter && {
    id: counter.id,
    timestamp: counter.timestamp,
    received: finite(counter.bytesReceived),
    sent: finite(counter.bytesSent),
    frames: video && finite(video.framesDecoded),
    videoId: video && video.id,
  };
  const elapsed = previous && counters && counters.id === previous.id ? (counters.timestamp - previous.timestamp) / 1000 : 0;
  const rate = key => elapsed > 0 && counters[key] !== null && previous[key] !== null && counters[key] >= previous[key]
    ? (counters[key] - previous[key]) * 8 / elapsed / 1e6 : null;
  const protocol = local && local.protocol;
  let fps = video && finite(video.framesPerSecond);
  if (fps == null && elapsed > 0 && video && previous.videoId === video.id &&
      counters.frames != null && previous.frames != null && counters.frames >= previous.frames) {
    fps = (counters.frames - previous.frames) / elapsed;
  }
  return {
    counters,
    sample: {
      localCandidateType: local?.candidateType || null,
      remoteCandidateType: remote?.candidateType || null,
      ...routeDetails(local, remote, servers),
      protocol: ['udp', 'tcp', 'tls'].includes(protocol) ? protocol : null,
      rttMs: pair && finite(pair.currentRoundTripTime) !== null ? pair.currentRoundTripTime * 1000 : null,
      inboundMbps: counters ? rate('received') : null,
      outboundMbps: counters ? rate('sent') : null,
      fps: fps ?? null,
      codec: codec?.mimeType || null,
      decoder: video?.decoderImplementation || null,
      width: video?.frameWidth || null,
      height: video?.frameHeight || null,
      packetsLost: video && Number.isFinite(video.packetsLost) ? Math.max(0, video.packetsLost) : null,
    },
  };
}

function aggregateStats(results) {
  if (!results.length) return { state: 'waiting', peerConnections: 0 };
  const active = results.filter(result => result.state === 'connected');
  const candidates = active.length ? active : results;
  candidates.sort((a, b) => (b.sample.inboundMbps || 0) + (b.sample.outboundMbps || 0) -
    (a.sample.inboundMbps || 0) - (a.sample.outboundMbps || 0));
  const chosen = candidates[0];
  const sum = key => {
    const values = active.map(result => result.sample[key]).filter(Number.isFinite);
    return values.length ? values.reduce((total, value) => total + value, 0) : null;
  };
  return {
    ...chosen.sample, state: chosen.state, peerConnections: results.length,
    inboundMbps: sum('inboundMbps'), outboundMbps: sum('outboundMbps'),
  };
}
