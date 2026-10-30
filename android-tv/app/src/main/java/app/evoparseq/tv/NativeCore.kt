package app.evoparseq.tv

/** Control/lifecycle commands only. Decoded pixels never cross JNI. */
internal object NativeCore {
    init { System.loadLibrary("evoparseq_tv") }
    external fun signIn(email: String, password: String, tfa: String): String
    external fun hosts(): String
    external fun signOut()
    external fun settings(current: String, field: String, index: Int): String
    external fun startStream(surface: android.view.Surface, peer: String, preferences: String,
        decoders: String, width: Int, height: Int): String
    external fun stopStream()
    external fun streamStatus(): String
}
