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
    external fun setOverlayVisible(visible: Boolean)
}
