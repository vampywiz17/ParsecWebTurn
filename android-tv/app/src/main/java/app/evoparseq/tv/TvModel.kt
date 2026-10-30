package app.evoparseq.tv

import android.os.Handler
import android.os.Looper
import android.app.Application
import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import org.json.JSONObject
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

internal data class Host(val peer: String, val name: String)

/** Retained through Activity recreation. One bounded account request at a time. */
internal class TvModel(application: Application) : AndroidViewModel(application) {
    var signedIn by mutableStateOf(false)
        private set
    var busy by mutableStateOf(false)
        private set
    var hosts by mutableStateOf(emptyList<Host>())
        private set
    var message by mutableStateOf("")
        private set
    var inventory by mutableStateOf("")
        private set
    var settings by mutableStateOf<JSONObject?>(null)
        private set
    var streamStatus by mutableStateOf<JSONObject?>(null)
        private set
    private var streamGeneration = 0
    private val preferences = application.getSharedPreferences("tv-requests", Context.MODE_PRIVATE)
    private val worker = Executors.newSingleThreadExecutor()
    private val main = Handler(Looper.getMainLooper())
    private val disposed = AtomicBoolean(false)

    private val vault = SessionVault(application)
    init {
        request {
            val saved = preferences.getString("requests", "") ?: ""
            val reply = JSONObject(NativeCore.settings(saved, "", 0))
            var restored = false
            var feedback = reply.optString("error")
            try {
                val token = vault.load()
                if (token != null) {
                    val auth = try { JSONObject(NativeCore.restoreSession(token)) } finally { token.fill(0) }
                    restored = auth.optBoolean("ok")
                    if (!restored) { vault.clear(); feedback = auth.optString("error") }
                }
            } catch (_: Exception) {
                runCatching { vault.clear() }
                feedback = "Saved sign-in could not be restored. Sign in again."
            }
            return@request { if (reply.optBoolean("ok")) settings = reply; signedIn = restored; message = feedback }
        }
    }

    private fun request(work: () -> (() -> Unit)) {
        if (busy || disposed.get()) return
        busy = true
        message = ""
        worker.execute {
            // Do not expose Java/JNI exception messages or raw HTTP response bodies.
            val complete = try { work() } catch (_: Exception) {
                { message = "The request could not be completed. Please try again." }
            } catch (_: LinkageError) {
                { message = "The native core is unavailable for this device." }
            }
            main.post {
                if (!disposed.get()) { complete(); busy = false }
            }
        }
    }

    fun signIn(email: String, password: String, code: String) = request {
        val reply = JSONObject(NativeCore.signIn(email, password, code))
        val ok = reply.optBoolean("ok")
        val feedback = if (ok) JSONObject(NativeCore.rememberSession(vault)).optString("error") else reply.optString("error")
        return@request { signedIn = ok; message = feedback }
    }

    fun refresh() = request {
        val reply = JSONObject(NativeCore.hosts())
        val next = reply.optJSONArray("hosts")
        val updated = if (next == null) null else (0 until next.length()).map {
            val host = next.getJSONObject(it)
            Host(host.getString("peer"), host.getString("name"))
        }
        val expired = reply.optBoolean("expired")
        if (expired) { vault.clear(); NativeCore.signOut() }
        val error = reply.optString("error")
        return@request { if (updated != null) hosts = updated; if (expired) { signedIn = false; hosts = emptyList() }; message = error }
    }

    fun signOut() = request {
        vault.clear()
        NativeCore.signOut()
        return@request { signedIn = false; hosts = emptyList(); message = "" }
    }

    fun inspectDecoders() = request {
        val text = DecoderInventory.describe()
        return@request { inventory = text }
    }

    fun readSettings() = request {
        val saved = preferences.getString("requests", "") ?: ""
        val reply = JSONObject(NativeCore.settings(saved, "", 0))
        val error = reply.optString("error")
        return@request { if (reply.optBoolean("ok")) settings = reply; message = error }
    }

    fun selectSetting(key: String, index: Int) {
        val current = settings?.getJSONObject("settings")?.toString() ?: return
        request {
            val reply = JSONObject(NativeCore.settings(current, key, index))
            var feedback = reply.optString("error")
            val saved = reply.optBoolean("ok") && preferences.edit()
                .putString("requests", reply.getJSONObject("settings").toString()).commit()
            if (reply.optBoolean("ok") && !saved) feedback = "Could not save TV settings."
            return@request {
                if (saved) settings = reply
                message = if (saved) "Saved on this TV. Applies to the next connection." else feedback
            }
        }
    }

    fun startStream(surface: android.view.Surface, host: Host, width: Int, height: Int) {
        val current = settings?.getJSONObject("settings")?.toString() ?: return
        val generation = ++streamGeneration
        streamStatus = JSONObject().put("state", "Connecting…")
        worker.execute {
            val reply = runCatching {
                JSONObject(NativeCore.startStream(surface, host.peer, current, DecoderInventory.choices(), width, height))
            }.getOrElse { JSONObject().put("error", "Could not start hardware streaming on this device.") }
            main.post {
                if (generation == streamGeneration && !disposed.get()) {
                    streamStatus = if (reply.has("error")) reply.put("ended", true) else streamStatus
                    if (!reply.has("error")) pollStream(generation)
                }
            }
        }
    }

    private fun pollStream(generation: Int) {
        if (generation != streamGeneration || disposed.get()) return
        worker.execute {
            val status = runCatching { JSONObject(NativeCore.streamStatus()) }.getOrElse {
                JSONObject().put("error", "Stream status unavailable").put("ended", true)
            }
            main.post {
                if (generation == streamGeneration && !disposed.get()) {
                    streamStatus = status
                    if (!status.optBoolean("ended")) main.postDelayed({ pollStream(generation) }, 500)
                }
            }
        }
    }

    fun stopStream() {
        ++streamGeneration
        // Serialized after any pending JNI start: a destroyed Surface cannot
        // leave a late session alive. This stop only signals, never joins UI.
        worker.execute { runCatching { NativeCore.stopStream() } }
    }

    override fun onCleared() {
        disposed.set(true)
        // Saved sign-in survives normal Activity/process shutdown; only explicit logout revokes it.
        worker.execute { runCatching { NativeCore.releaseSession() } }
        worker.shutdown()
    }
}
