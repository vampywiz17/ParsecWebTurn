'use strict';
const $ = id => document.getElementById(id);
const invoke = (command, args) => window.__TAURI__.core.invoke(command, args);
let busy = false;
function panel() {
  $('cloudflare').hidden = $('provider').value !== 'cloudflare'; $('custom').hidden = $('provider').value !== 'custom';
  for (const field of document.querySelectorAll('[data-turn-setting]')) field.disabled = $('stun-only').checked;
}
function showError(error) { $('error').textContent = String(error); $('error').hidden = false; $('status').textContent = ''; }
function request() {
  return { provider: $('provider').value, stunUrls: $('stun-urls').value.split(/\r?\n/), turnUrls: $('turn-urls').value.split(/\r?\n/), stunOnly: $('stun-only').checked, customUsername: $('username').value,
    turnKeyId: $('key').value, ttl: Number($('ttl').value), cacheCredentials: $('cache').checked,
    mediaDiagnostics: $('media-diagnostics').checked,
    apiToken: $('token').value, customPassword: $('password').value,
    forgetApiToken: $('forget-token').checked, forgetCustomPassword: $('forget-password').checked };
}
async function load() {
  const config = await invoke('get_configuration');
  $('provider').value = config.provider;
  $('stun-urls').value = config.stunUrls.join('\n'); $('turn-urls').value = config.turnUrls.join('\n'); $('stun-only').checked = config.stunOnly; $('username').value = config.customUsername;
  $('key').value = config.turnKeyId; $('ttl').value = config.ttl; $('cache').checked = config.cacheCredentials;
  $('media-diagnostics').checked = config.mediaDiagnostics;
  $('token').value = ''; $('password').value = ''; $('forget-token').checked = false; $('forget-password').checked = false;
  $('token').placeholder = config.hasApiToken ? 'Saved token · leave blank to keep' : 'Cloudflare TURN API token';
  $('password').placeholder = config.hasCustomPassword ? 'Saved password · leave blank to keep' : 'TURN password';
  $('version').textContent = `v${config.version}`; panel();
  if (config.error) showError(config.error);
  return config;
}
async function action(operation) {
  if (busy) return; busy = true;
  for (const button of document.querySelectorAll('button')) button.disabled = true;
  $('error').hidden = true; $('fallback').hidden = true;
  try { await operation(); }
  catch (error) {
    showError(error); $('fallback').hidden = false;
    // Startup is hidden until the form is ready; failures must remain visible.
    await invoke('show_configuration');
  }
  finally { busy = false; for (const button of document.querySelectorAll('button')) button.disabled = false; }
}
async function save(connect) {
  $('status').textContent = 'Saving settings…';
  await invoke('save_configuration', { input: request() });
  await load();
  $('status').textContent = connect ? 'Preparing your connection…' : 'Settings saved. Changes apply on your next connection.';
  if (connect) { await invoke('connect_saved'); $('status').textContent = 'Parsec opened. Return here using Connection settings in the app menu.'; }
}
$('provider').addEventListener('change', panel);
$('stun-only').addEventListener('change', panel);
$('windowed').addEventListener('click', () => action(async () => { await invoke('set_parsec_window_mode', {fullscreen:false}); $('status').textContent='Parsec restored to windowed mode. Use F11 to toggle fullscreen.'; }));
$('stats').addEventListener('click', () => action(() => invoke('open_stats')));
$('save').addEventListener('click', () => action(() => save(false)));
$('settings').addEventListener('submit', event => { event.preventDefault(); action(() => save(true)); });
$('fallback').addEventListener('click', () => action(async () => { $('status').textContent = 'Opening local fallback…'; await invoke('connect_fallback'); $('status').textContent = 'Parsec opened using ice.json.'; }));
action(async () => {
  const config = await load();
  if (config.autoConnect) {
    $('status').textContent = 'Preparing your connection…';
    await invoke('connect_saved');
    $('status').textContent = 'Parsec opened.';
  } else {
    await invoke('show_configuration');
  }
});
