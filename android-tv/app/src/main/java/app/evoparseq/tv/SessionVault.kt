package app.evoparseq.tv

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Platform storage only. Rust owns token validation and session lifetime. */
internal class SessionVault(context: Context) {
    private val file = AtomicFile(File(context.noBackupFilesDir, "account-session.bin"))
    private val alias = "evoparseq-account-v1"
    private fun key(create: Boolean): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(alias, null) as? SecretKey)?.let { return it }
        check(create) { "Saved sign-in key unavailable" }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setKeySize(256).setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setRandomizedEncryptionRequired(true).build())
        }.generateKey()
    }
    // Called once after successful login, never from a media callback.
    fun save(token: ByteArray) {
        try {
            require(token.size in 1..4096)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, key(true))
            cipher.updateAAD(alias.toByteArray(Charsets.UTF_8))
            val encrypted = cipher.doFinal(token)
            check(cipher.iv.size == 12)
            val stream = file.startWrite()
            try {
                stream.write(1); stream.write(cipher.iv); stream.write(encrypted)
                file.finishWrite(stream)
            } catch (error: Exception) { file.failWrite(stream); throw error }
        } finally { token.fill(0) }
    }
    fun load(): ByteArray? {
        if (!file.baseFile.exists() && !File(file.baseFile.path + ".bak").exists()) return null
        val bytes = file.openRead().use { input ->
            val buffer = ByteArray(4126)
            var count = 0
            while (count < buffer.size) {
                val read = input.read(buffer, count, buffer.size - count)
                if (read < 0) break
                count += read
            }
            require(count in 30..4125 && buffer[0] == 1.toByte())
            buffer.copyOf(count)
        }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(false), GCMParameterSpec(128, bytes, 1, 12))
        cipher.updateAAD(alias.toByteArray(Charsets.UTF_8))
        return cipher.doFinal(bytes, 13, bytes.size - 13)
    }
    fun clear() {
        file.delete()
        KeyStore.getInstance("AndroidKeyStore").apply { load(null); deleteEntry(alias) }
    }
}
