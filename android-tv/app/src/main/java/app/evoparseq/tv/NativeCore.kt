package app.evoparseq.tv

/** Account commands only. Decoded pixels must never cross this JNI boundary. */
internal object NativeCore {
    init { System.loadLibrary("evoparseq_tv") }
    external fun signIn(email: String, password: String, tfa: String): String
    external fun hosts(): String
    external fun signOut()
    external fun settings(current: String, field: String, index: Int): String
}
