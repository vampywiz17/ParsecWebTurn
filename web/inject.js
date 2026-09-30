(() => {
  if (window.location.origin !== 'https://web.parsec.app') return;
  if (window.__parsecWebTurnPatched) return;
  const OriginalRTCPeerConnection = window.RTCPeerConnection;
  if (!OriginalRTCPeerConnection) {
    console.error('[ParsecWebTurn] RTCPeerConnection is not available');
    return;
  }
  const ICE_SERVERS = __ICE_SERVERS__;
  const patchConfig = config => ({ ...config, iceServers: ICE_SERVERS });
  const peers = new Map();
  __STATS_HELPER__

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
          const normalized = summarizeStats(await peer.getStats(), previous);
          peers.set(peer, normalized.counters);
          const state = peer.connectionState || (peer.iceConnectionState === 'completed' ? 'connected' : peer.iceConnectionState) || 'new';
          results.push({ state, sample: normalized.sample });
        } catch { /* An individual peer can close during sampling. */ }
      }
      await window.__TAURI_INTERNALS__.invoke('report_stats', { sample: aggregateStats(results) });
    } catch { /* Diagnostics must never interrupt a Parsec session. */ }
    finally { sampling = false; }
  }, 1000);
})();
