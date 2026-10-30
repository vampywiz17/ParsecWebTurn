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
    private val preferences = application.getSharedPreferences("tv-requests", Context.MODE_PRIVATE)
    private val worker = Executors.newSingleThreadExecutor()
    private val main = Handler(Looper.getMainLooper())
    private val disposed = AtomicBoolean(false)

    init { readSettings() }

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
        val error = reply.optString("error")
        return@request { signedIn = ok; message = error }
    }

    fun refresh() = request {
        val reply = JSONObject(NativeCore.hosts())
        val next = reply.optJSONArray("hosts")
        val updated = if (next == null) null else (0 until next.length()).map {
            val host = next.getJSONObject(it)
            Host(host.getString("peer"), host.getString("name"))
        }
        val error = reply.optString("error")
        return@request { if (updated != null) hosts = updated; message = error }
    }

    fun signOut() = request {
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
                message = if (saved) "Saved on this TV. Streaming is not connected in this preview." else feedback
            }
        }
    }

    override fun onCleared() {
        disposed.set(true)
        // Runs after any in-flight sign-in, so a late result cannot leak a session.
        worker.execute { runCatching { NativeCore.signOut() } }
        worker.shutdown()
    }
}
