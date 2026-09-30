// Pure getStats normalization, shared by the webview and regression tests.
function summarizeStats(report, previous) {
  const entries = [...report.values()];
  const transport = entries.find(entry => entry.type === 'transport' && entry.selectedCandidatePairId);
  const pair = (transport && report.get(transport.selectedCandidatePairId)) ||
    entries.find(entry => entry.type === 'candidate-pair' && entry.nominated && entry.state === 'succeeded');
  const counter = pair || entries.find(entry => entry.type === 'transport' && Number.isFinite(entry.bytesReceived));
  const local = pair && report.get(pair.localCandidateId);
  const remote = pair && report.get(pair.remoteCandidateId);
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
  const protocol = local && (local.relayProtocol || local.protocol);
  let fps = video && finite(video.framesPerSecond);
  if (fps == null && elapsed > 0 && video && previous.videoId === video.id &&
      counters.frames != null && previous.frames != null && counters.frames >= previous.frames) {
    fps = (counters.frames - previous.frames) / elapsed;
  }
  return {
    counters,
    sample: {
      route: pair ? (local?.candidateType === 'relay' || remote?.candidateType === 'relay' ? 'relay' : 'direct') : null,
      protocol: ['udp', 'tcp', 'tls'].includes(protocol) ? protocol : null,
      rttMs: pair && Number.isFinite(pair.currentRoundTripTime) ? pair.currentRoundTripTime * 1000 : null,
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
