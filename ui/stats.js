'use strict';
const $ = id => document.getElementById(id);
const metric = (value, unit, digits = 1) => Number.isFinite(value) ? `${value.toFixed(digits)} ${unit}` : '—';
let busy = false;
async function update() {
  if (busy) return; busy = true;
  try {
    const stats = await window.__TAURI__.core.invoke('get_stats');
    $('state').textContent = stats.stale ? 'Updates paused · last sample is stale' : `${stats.state} · ${stats.peerConnections} WebRTC connection(s)`;
    $('rtt').textContent = metric(stats.rttMs, 'ms'); $('fps').textContent = metric(stats.fps, 'fps', 0);
    $('inbound').textContent = metric(stats.inboundMbps, 'Mbps', 2); $('outbound').textContent = metric(stats.outboundMbps, 'Mbps', 2);
    const route = (stats.route === 'direct' ? 'Direct — no TURN' : stats.route) ?? (stats.localCandidateType === 'prflx' || stats.remoteCandidateType === 'prflx'
      ? 'Unverified (peer-reflexive)' : 'Unknown');
    const rows = [ ['ICE route', route], ['Route evidence', stats.routeEvidence],
      ['Configured TURN in use', stats.configuredTurnUsed == null ? 'Unknown — insufficient evidence' : (stats.configuredTurnUsed ? 'Yes' : 'No')],
      ...(stats.turnServer ? [['TURN server', stats.turnServer]] : []),
      ...(stats.turnProtocol ? [['TURN transport', stats.turnProtocol]] : []),
      ['Local candidate', stats.localCandidateType], ['Remote candidate', stats.remoteCandidateType], ['ICE transport', stats.protocol], ['Video codec', stats.codec],
      ...(stats.mediaDiagnosticsEnabled ? [ ['Decoder', stats.decoder], ['Profile', stats.videoProfile],
        ['Decoder backend', stats.decoderBackend], ['Hardware decode', stats.hardwareDecode == null ? null : (stats.hardwareDecode ? 'Yes' : 'No')],
        ['Video source', stats.videoSource] ] : []),
      ['FPS source', stats.fpsSource], ['Resolution', stats.width && stats.height ? `${stats.width} × ${stats.height}` : null],
      ['Video packets lost', stats.packetsLost ?? (stats.fpsSource === 'WebCodecs decoder' ? 'Not exposed for data-channel video' : null)] ];
    $('details').replaceChildren(...rows.map(([label, value]) => { const row = document.createElement('div'); row.textContent = `${label}: ${value ?? 'Not reported'}`; return row; }));
  } catch { $('state').textContent = 'Statistics unavailable'; }
  finally { busy = false; }
}
update(); setInterval(update, 1000);
