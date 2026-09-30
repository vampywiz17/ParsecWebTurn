// The native application owns window/fullscreen transitions. Parsec otherwise
// enters HTML fullscreen automatically and locks Escape during host connection.
// Keep its canvas in the normal WebView viewport; native F11 still resizes it.
(() => {
  if (location.origin !== 'https://web.parsec.app') return;
  const recoveryKey = event => event.code === 'F11' ||
    (event.code === 'KeyW' && event.ctrlKey && event.shiftKey && !event.altKey && !event.metaKey);
  let changingMode = false;
  for (const type of ['keydown', 'keyup']) {
    window.addEventListener(type, event => {
      if (!recoveryKey(event) || !window.__TAURI_INTERNALS__) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      if (type !== 'keydown' || event.repeat || changingMode) return;
      changingMode = true;
      window.__TAURI_INTERNALS__.invoke('parsec_window_shortcut', { toggle: event.code === 'F11' })
        .catch(() => {}).finally(() => { changingMode = false; });
    }, true);
  }
  if (Element.prototype.requestFullscreen) {
    Element.prototype.requestFullscreen = function () {
      const denied = Promise.reject(new DOMException('Use the app View menu or F11 for fullscreen.', 'NotAllowedError'));
      // Parsec's current matoya.js ignores the returned promise.
      denied.catch(() => {});
      return denied;
    };
  }
  if (navigator.keyboard?.lock) {
    const lock = navigator.keyboard.lock.bind(navigator.keyboard);
    navigator.keyboard.lock = keys => {
      const allowed = (keys || []).filter(key => !['Escape', 'F11', 'KeyW'].includes(key));
      return allowed.length ? lock(allowed) : Promise.resolve();
    };
  }
})();
