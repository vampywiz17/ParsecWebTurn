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

// Match only complete endpoints on the same ICE generation. Never infer routing
// from a VPN interface name, candidate priority, TURN DNS or unused candidates.
function sameEndpoint(a, b) {
  return typeof a?.address === 'string' && a.address.length > 0 &&
    typeof b?.address === 'string' && a.address.toLowerCase() === b.address.toLowerCase() &&
    Number.isInteger(a.port) && a.port > 0 && a.port === b.port &&
    ['udp', 'tcp'].includes(a.protocol) && a.protocol === b.protocol &&
    (!a.usernameFragment || !b.usernameFragment || a.usernameFragment === b.usernameFragment);
}

function iceCandidate(candidate) {
  return candidate && { ...candidate, candidateType: candidate.type,
    address: candidate.address, port: candidate.port, protocol: candidate.protocol,
    usernameFragment: candidate.usernameFragment, relayProtocol: candidate.relayProtocol,
    url: candidate.url };
}

function resolveCandidate(stats, selected, gathered) {
  const current = stats || selected;
  if (!current) return null;
  const matched = selected && (!stats || sameEndpoint(stats, selected)) ? selected : null;
  // Positive relay evidence always wins, including Chromium's prflx relayProtocol.
  const isRelay = candidate => candidate?.candidateType === 'relay' ||
    ['udp', 'tcp', 'tls'].includes(candidate?.relayProtocol);
  if (isRelay(current)) return current;
  if (isRelay(matched)) return { ...current, ...matched };
  if (['host', 'srflx'].includes(current.candidateType)) return current;
  if (matched && ['host', 'srflx'].includes(matched.candidateType)) {
    return { ...current, ...matched, correlated: true };
  }
  const matches = gathered.filter(candidate => sameEndpoint(current, candidate));
  const relay = matches.find(isRelay);
  if (relay) return { ...current, ...relay, correlated: true };
  if (matches.length && matches.every(candidate => ['host', 'srflx'].includes(candidate.candidateType))) {
    return { ...current, ...matches[0], correlated: true };
  }
  return current;
}

function summarizeStats(report, previous, servers = [], selectedPair = null, candidates = {}) {
  const entries = [...report.values()];
  const transport = entries.find(entry => entry.type === 'transport' && entry.selectedCandidatePairId);
  const nominated = entries.filter(entry => entry.type === 'candidate-pair' && entry.nominated && entry.state === 'succeeded');
  const pair = (transport && report.get(transport.selectedCandidatePairId)) ||
    (nominated.length === 1 ? nominated[0] : null);
  const transports = entries.filter(entry => entry.type === 'transport');
  const linked = pair?.transportId && report.get(pair.transportId);
  const securityTransport = transport || (linked?.type === 'transport' ? linked :
    (transports.length === 1 && !transports[0].selectedCandidatePairId ? transports[0] : null));
  const securityName = value => typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(value) ? value : null;
  const counter = pair || entries.find(entry => entry.type === 'transport' && Number.isFinite(entry.bytesReceived));
  const observedLocal = pair && report.get(pair.localCandidateId);
  const observedRemote = pair && report.get(pair.remoteCandidateId);
  const selectedLocal = iceCandidate(selectedPair?.local);
  const selectedRemote = iceCandidate(selectedPair?.remote);
  // Both endpoints must match before enriching a report pair: selection can
  // change while asynchronous getStats is running.
  const samePair = !pair || (sameEndpoint(observedLocal, selectedLocal) &&
    sameEndpoint(observedRemote, selectedRemote));
  const local = resolveCandidate(observedLocal, samePair ? selectedLocal : null,
    (candidates.local || []).map(iceCandidate));
  const remote = resolveCandidate(observedRemote, samePair ? selectedRemote : null,
    (candidates.remote || []).map(iceCandidate));
  const route = routeDetails(local, remote, servers);
  if (route.route === 'direct' && (local?.correlated || remote?.correlated)) {
    route.routeEvidence = 'Selected endpoints matched non-relay ICE candidates';
  }
  const video = entries.filter(entry => entry.type === 'inbound-rtp' && entry.kind === 'video')
    .sort((a, b) => (b.bytesReceived || 0) - (a.bytesReceived || 0))[0];
  const codec = video && report.get(video.codecId);
  const audio = entries.filter(entry => entry.type === 'inbound-rtp' && entry.kind === 'audio')
    .sort((a,b) => (b.bytesReceived || 0) - (a.bytesReceived || 0))[0];
  const audioCodec = audio && report.get(audio.codecId);
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
      localCandidateType: observedLocal?.candidateType || (samePair && selectedLocal?.candidateType) || null,
      remoteCandidateType: observedRemote?.candidateType || (samePair && selectedRemote?.candidateType) || null,
      ...route,
      protocol: ['udp', 'tcp', 'tls'].includes(protocol) ? protocol : null,
      dtlsState: ['new', 'connecting', 'connected', 'closed', 'failed'].includes(securityTransport?.dtlsState) ? securityTransport.dtlsState : null,
      tlsVersion: typeof securityTransport?.tlsVersion === 'string' && /^[0-9A-F]{4}$/.test(securityTransport.tlsVersion) ? securityTransport.tlsVersion : null,
      dtlsCipher: securityName(securityTransport?.dtlsCipher),
      srtpCipher: securityName(securityTransport?.srtpCipher),
      audioCodec: audioCodec?.mimeType || null,
      audioBitrateKbps: null, // Inbound statistics do not expose the remote encoder's configured target.
      audioSampleRate: audioCodec?.clockRate || null, audioChannels: audioCodec?.channels || null,
      audioSource: audio ? 'WebRTC inbound audio codec' : null,
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
