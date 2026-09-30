'use strict';
const $ = id => document.getElementById(id);
const metric = (value, unit, digits = 1) => Number.isFinite(value) ? `${value.toFixed(digits)} ${unit}` : '—';
let busy = false;
async function update() {
  if (busy) return; busy = true;
  try {
    const stats = await window.__TAURI_INTERNALS__.invoke('get_stats');
    $('state').textContent = stats.stale ? 'Updates paused · last sample is stale' : `${stats.state} · ${stats.peerConnections} WebRTC connection(s)`;
    $('rtt').textContent = metric(stats.rttMs, 'ms'); $('fps').textContent = metric(stats.fps, 'fps', 0);
    $('inbound').textContent = metric(stats.inboundMbps, 'Mbps', 2); $('outbound').textContent = metric(stats.outboundMbps, 'Mbps', 2);
    const rows = [ ['ICE route', stats.route], ['Local candidate', stats.localCandidateType], ['Remote candidate', stats.remoteCandidateType], ['Transport', stats.protocol], ['Video codec', stats.codec], ['Decoder', stats.decoder],
      ['Profile', stats.videoProfile], ['Decoder backend', stats.decoderBackend], ['Hardware decode', stats.hardwareDecode == null ? null : (stats.hardwareDecode ? 'Yes' : 'No')],
      ['Video source', stats.videoSource], ['Resolution', stats.width && stats.height ? `${stats.width} × ${stats.height}` : null], ['Packets lost', stats.packetsLost] ];
    $('details').replaceChildren(...rows.map(([label, value]) => { const row = document.createElement('div'); row.textContent = `${label}: ${value ?? 'Not reported'}`; return row; }));
  } catch { $('state').textContent = 'Statistics unavailable'; }
  finally { busy = false; }
}
update(); setInterval(update, 1000);
