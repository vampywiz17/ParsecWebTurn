(() => {
  if (window.location.origin !== 'https://web.parsec.app') return;
  if (window.__parsecWebTurnPatched) return;
  const OriginalRTCPeerConnection = window.RTCPeerConnection;
  if (!OriginalRTCPeerConnection) {
    console.error('[ParsecWebTurn] RTCPeerConnection is not available');
    return;
  }
  const ICE_SERVERS = __ICE_SERVERS__;
  const patchConfig = config => {
    // Let the native WebIDL converter reject primitive dictionary arguments.
    if (config != null && !['object', 'function'].includes(typeof config)) return config;
    const original = config ?? {};
    // WebIDL reads known members, including inherited/non-enumerable getters.
    // An empty target avoids invariants imposed by frozen input properties.
    return new Proxy(Object.create(null), {
      get(_target, key) {
        return key === 'iceServers' ? ICE_SERVERS : Reflect.get(original, key, original);
      },
    });
  };
  const peers = new Map();
  __STATS_HELPER__
  __VIDEO_HELPER__

  class PatchedRTCPeerConnection extends OriginalRTCPeerConnection {
    constructor(config = {}, constraints) {
      super(patchConfig(config), constraints);
      peers.set(this, null);
    }
    setConfiguration(config) {
      return super.setConfiguration(patchConfig(config));
    }
    close() {
      peers.delete(this);
      return super.close();
    }
  }
  window.RTCPeerConnection = PatchedRTCPeerConnection;
  if (window.webkitRTCPeerConnection) window.webkitRTCPeerConnection = PatchedRTCPeerConnection;
  window.__parsecWebTurnPatched = true;
  console.log('[ParsecWebTurn] ICE override installed before Parsec startup; server count:', ICE_SERVERS.length);

  let sampling = false;
  setInterval(async () => {
    if (sampling || !window.__TAURI_INTERNALS__) return;
    sampling = true;
    try {
      const results = [];
      for (const [peer, previous] of peers) {
        if (peer.connectionState === 'closed') { peers.delete(peer); continue; }
        try {
          let selectedPair = null;
          try { selectedPair = peer.sctp?.transport?.iceTransport?.getSelectedCandidatePair?.(); } catch {}
          const normalized = summarizeStats(await peer.getStats(), previous, ICE_SERVERS, selectedPair);
          peers.set(peer, normalized.counters);
          const state = peer.connectionState || (peer.iceConnectionState === 'completed' ? 'connected' : peer.iceConnectionState) || 'new';
          results.push({ state, sample: normalized.sample });
        } catch { /* An individual peer can close during sampling. */ }
      }
      const sample = aggregateStats(results);
      const decodedFps = sampleDecodedFps(performance.now());
      if (sample.state === 'connected' && sample.fps == null && decodedFps != null) {
        sample.fps = decodedFps;
        sample.fpsSource = 'WebCodecs decoder';
      } else if (sample.fps != null) sample.fpsSource = 'WebRTC inbound video';
      await window.__TAURI_INTERNALS__.invoke('report_stats', { sample });
    } catch { /* Diagnostics must never interrupt a Parsec session. */ }
    finally { sampling = false; }
  }, 1000);
})();
