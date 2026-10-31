package app.evoparseq.tv

/** Control/lifecycle commands only. Decoded pixels never cross JNI. */
internal object NativeCore {
    init { System.loadLibrary("evoparseq_tv") }
    external fun signIn(email: String, password: String, tfa: String): String
    external fun rememberSession(vault: SessionVault): String
    external fun restoreSession(token: ByteArray): String
    external fun startLocalSetup(ip: String): String
    external fun localSetupStatus(): String
    external fun confirmLocalSetup(vault: SessionVault): String
    external fun stopLocalSetup()
    external fun restoreNetwork(bytes: ByteArray): String
    external fun resetNetwork(): String
    external fun hosts(): String
    external fun signOut()
    external fun releaseSession()
    external fun settings(current: String, field: String, index: Int): String
    external fun startStream(surface: android.view.Surface, peer: String, preferences: String,
        decoders: String, width: Int, height: Int): String
    external fun stopStream()
    external fun streamStatus(): String
    external fun controllerAdapter(adapter: ControllerInput?)
    external fun controllerFocus(active: Boolean)
    external fun controllerButton(id: Int, code: Int, down: Boolean): Boolean
    external fun controllerRemoved(id: Int)
    external fun controllerAxes(id: Int, x: Float, y: Float, rx: Float, ry: Float, lt: Float, rt: Float,
        hx: Float, hy: Float, fx: Float, fy: Float, frx: Float, fry: Float, flt: Float, frt: Float)
    external fun setOverlayVisible(visible: Boolean)
}
