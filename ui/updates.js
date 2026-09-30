'use strict';
const $ = id => document.getElementById(id);
const invoke = command => window.__TAURI_INTERNALS__.invoke(command);
let polling = false;
async function refresh() {
  if (polling) return;
  polling = true;
  try {
    const update = await invoke('get_update');
    $('version').textContent = `Installed: v${update.currentVersion}${update.version ? ` · Available: ${update.version}` : ''}`;
    $('status').textContent = update.message;
    $('check').disabled = update.busy;
    $('install').hidden = update.status !== 'ready';
    $('notes').textContent = update.notes;
    $('notes-title').hidden = !update.notes;
  } catch (error) { $('error').hidden = false; $('error').textContent = String(error); }
  finally { polling = false; }
}
$('check').addEventListener('click', async () => {
  $('check').disabled = true;
  try { await invoke('check_update'); }
  catch (error) { $('error').hidden = false; $('error').textContent = String(error); }
  await refresh();
});
$('install').addEventListener('click', async () => {
  $('install').disabled = true;
  try { await invoke('install_update'); }
  catch (error) { $('error').hidden = false; $('error').textContent = String(error); $('install').disabled = false; }
});
$('later').addEventListener('click', () => invoke('dismiss_update'));
setInterval(refresh, 1000);
refresh();
