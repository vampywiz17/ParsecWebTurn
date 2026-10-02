# Explicit direct-first / TURN fallback experiment

This console-only diagnostic is not embedded in the application. It tests whether
the current Parsec web client responds to a standard ICE restart and renegotiates
with its host; support is not assumed. No private Parsec function, signaling
message, candidate priority or SDP content is changed.

1. Open ParsecWebTurn with a working STUN + TURN configuration. Disconnect any
   active session. Open the Parsec webview's Developer Tools (F12).
2. Paste `scripts/test-direct-first-console.js` into its Console **before**
   selecting Connect. It affects only subsequently created peer connections.
3. Connect with VPN available. The first attempt should report zero TURN URLs
   and retain STUN. A connected peer, a temporary `disconnected` state or elapsed
   time never triggers fallback.
4. Reload to reset, paste again, and try without a working direct path. Only
   when every live eligible peer reaches `iceConnectionState === 'failed'` does
   the script restore TURN servers and call `restartIce()` on the same peers.
   Do not click Connect again during this experiment.
5. Collect results with:

   ```js
   console.log(JSON.stringify(await window.parsecDirectFirstTest.snapshot(), null, 2))
   ```

An automatic fallback is established only if Parsec handles the restart, sets
new local/remote ICE descriptions and a TURN path subsequently carries increasing
application bytes. `negotiationneeded` or changed server configuration alone is
not success. If Parsec closes the peer before reporting `failed`, or never calls
`createOffer` after the restart, this script does not simulate either operation.
Reloading removes the test; normal app behavior and saved settings are unchanged.

Explicit relay-only policy and candidate pooling are excluded. The test fails
conservatively rather than modifying the client's policy. Reports omit addresses,
server URLs, SDP and credentials. Do not paste additional raw browser logs with
credentials. This is experimental instrumentation, not an official Parsec API.

References: [W3C restartIce](https://www.w3.org/TR/webrtc/#dom-rtcpeerconnection-restartice)
and [configuration changes](https://www.w3.org/TR/webrtc/#set-the-configuration).
