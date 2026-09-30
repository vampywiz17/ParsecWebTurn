// The native application owns window/fullscreen transitions. Parsec otherwise
// enters HTML fullscreen automatically and locks Escape during host connection.
// Keep its canvas in the normal WebView viewport; native F11 still resizes it.
(() => {
  if (location.origin !== 'https://web.parsec.app') return;
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
      const allowed = (keys || []).filter(key => !['Escape', 'F11'].includes(key));
      return allowed.length ? lock(allowed) : Promise.resolve();
    };
  }
})();
