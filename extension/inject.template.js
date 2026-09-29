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
  const patchConfig = (config) => ({ ...config, iceServers: ICE_SERVERS });

  class PatchedRTCPeerConnection extends OriginalRTCPeerConnection {
    constructor(config = {}, constraints) {
      super(patchConfig(config), constraints);
    }

    setConfiguration(config) {
      return super.setConfiguration(patchConfig(config));
    }
  }

  window.RTCPeerConnection = PatchedRTCPeerConnection;
  if (window.webkitRTCPeerConnection) {
    window.webkitRTCPeerConnection = PatchedRTCPeerConnection;
  }

  window.__parsecWebTurnPatched = true;
  console.log("[ParsecWebTurn] ICE override installed before Parsec startup; server count:", ICE_SERVERS.length);
})();
