(() => {
  if (window.__parsecWebTurnPatched) {
    console.log("[ParsecWebTurn] ICE override already active");
    return;
  }

  const OriginalRTCPeerConnection = window.RTCPeerConnection;
  if (!OriginalRTCPeerConnection) {
    console.error("[ParsecWebTurn] RTCPeerConnection is not available");
    return;
  }

  const ICE_SERVERS = __ICE_SERVERS__;

  class PatchedRTCPeerConnection extends OriginalRTCPeerConnection {
    constructor(config = {}, constraints) {
      const patchedConfig = {
        ...config,
        iceServers: ICE_SERVERS
      };

      console.log("[ParsecWebTurn] Cloudflare STUN/TURN override:", patchedConfig.iceServers);
      super(patchedConfig, constraints);
    }
  }

  try {
    Object.setPrototypeOf(PatchedRTCPeerConnection, OriginalRTCPeerConnection);
  } catch (_) {}

  window.RTCPeerConnection = PatchedRTCPeerConnection;
  if (window.webkitRTCPeerConnection) {
    window.webkitRTCPeerConnection = PatchedRTCPeerConnection;
  }

  window.__parsecWebTurnPatched = true;
  console.log("[ParsecWebTurn] ICE override installed before Parsec startup");
})();
