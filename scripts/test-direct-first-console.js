// Paste into the ParsecWebTurn DevTools console BEFORE selecting Connect.
// Explicit diagnostic only; not embedded in the app. Reload the page to undo.
(() => {
  if (window.location.origin !== 'https://web.parsec.app') throw new Error('Open the Parsec webview console.');
  if (!window.__parsecWebTurnPatched) throw new Error('This test requires ParsecWebTurn.');
  if (window.parsecDirectFirstTest) throw new Error('Test already installed; reload to reset it.');
  const Current = window.RTCPeerConnection;
  // The same public native method used by our isolated WebView2 smoke fixture.
  const setNative = Object.getPrototypeOf(Current.prototype).setConfiguration;
  if (typeof setNative !== 'function') throw new Error('Cannot find the standard native setConfiguration method.');
  const records = [];
  const started = performance.now();
  const emit = (record, event, extra = {}) => {
    const entry = { ms: Math.round(performance.now() - started), peer: record?.id ?? null, event, ...extra };
    records.push(entry);
    console.log('[Direct-first test]', entry);
  };
  const peers = new Set();
  let fallback = false;
  let nextId = 1;
  const counts = list => {
    const urls = list.flatMap(s => Array.isArray(s.urls) ? s.urls : [s.urls]);
    return { stunUrls: urls.filter(u => /^stuns?:/i.test(u)).length, turnUrls: urls.filter(u => /^turns?:/i.test(u)).length };
  };
  const credentials = description => description?.sdp?.match(/^a=ice-ufrag:(.+)$/m)?.[1] ?? null;
  function wrap(record, name, completed) {
    const original = record.peer[name];
    record.peer[name] = function (...args) {
      if (this !== record.peer) return Reflect.apply(original, this, args);
      emit(record, name + ':called', { phase: fallback ? 'turn' : 'stun' });
      const result = Reflect.apply(original, this, args);
      return Promise.resolve(result).then(value => { completed?.(); return value; });
    };
  }
  function tryFallback() {
    const live = [...peers].filter(r => r.peer.iceConnectionState !== 'closed');
    if (fallback || !live.length || live.some(r => !r.eligible || r.peer.iceConnectionState !== 'failed')) return;
    fallback = true;
    emit(null, 'all-live-peers-failed:enable-TURN');
    for (const record of live) {
      const peer = record.peer;
      try {
        setNative.call(peer, { ...peer.getConfiguration(), iceServers: record.servers });
        emit(record, 'TURN-added', counts(peer.getConfiguration().iceServers));
        peer.restartIce();
        emit(record, 'restartIce-called');
      } catch (error) { emit(record, 'fallback-error', { name: error.name }); }
    }
  }
  window.RTCPeerConnection = new Proxy(Current, {
    construct(target, args, newTarget) {
      const peer = Reflect.construct(target, args, newTarget);
      const config = peer.getConfiguration();
      const record = { id: nextId++, peer, servers: config.iceServers, eligible: false, local: null, remote: null };
      peers.add(record);
      const stun = config.iceServers.flatMap(server => {
        const urls = (Array.isArray(server.urls) ? server.urls : [server.urls]).filter(u => /^stuns?:/i.test(u));
        return urls.length ? [{ ...server, urls }] : [];
      });
      if (config.iceTransportPolicy === 'all' && (config.iceCandidatePoolSize ?? 0) === 0 && counts(config.iceServers).turnUrls > 0) {
        record.eligible = true;
        if (!fallback) setNative.call(peer, { ...config, iceServers: stun });
      }
      emit(record, 'peer-created', { phase: fallback ? 'turn' : 'stun', eligible: record.eligible, ...counts(peer.getConfiguration().iceServers) });
      wrap(record, 'createOffer');
      wrap(record, 'setLocalDescription', () => {
        const value = credentials(peer.localDescription);
        emit(record, 'local-description-set', { iceCredentialsChanged: !!record.local && value !== record.local });
        record.local = value;
      });
      wrap(record, 'setRemoteDescription', () => {
        const value = credentials(peer.remoteDescription);
        emit(record, 'remote-description-set', { iceCredentialsChanged: !!record.remote && value !== record.remote });
        record.remote = value;
      });
      peer.addEventListener('negotiationneeded', () => emit(record, 'negotiationneeded', { phase: fallback ? 'turn' : 'stun' }));
      peer.addEventListener('iceconnectionstatechange', () => {
        emit(record, 'ice-state', { state: peer.iceConnectionState });
        tryFallback();
      });
      return peer;
    },
  });
  window.parsecDirectFirstTest = {
    records,
    async snapshot() {
      const result = [];
      for (const record of peers) {
        const peer = record.peer;
        try {
          const stats = await peer.getStats();
          const transport = [...stats.values()].find(s => s.type === 'transport' && s.selectedCandidatePairId);
          const pair = transport && stats.get(transport.selectedCandidatePairId);
          const local = pair && stats.get(pair.localCandidateId);
          const remote = pair && stats.get(pair.remoteCandidateId);
          result.push({ peer: record.id, iceState: peer.iceConnectionState, gatheringState: peer.iceGatheringState,
            ...counts(peer.getConfiguration().iceServers), localType: local?.candidateType ?? null,
            remoteType: remote?.candidateType ?? null, relayProtocol: local?.relayProtocol ?? null,
            bytesReceived: pair?.bytesReceived ?? null, bytesSent: pair?.bytesSent ?? null });
        } catch { result.push({ peer: record.id, unavailable: true }); }
      }
      return { fallbackAttempted: fallback, records, peers: result };
    },
  };
  emit(null, 'installed:select-Connect-now');
})();
