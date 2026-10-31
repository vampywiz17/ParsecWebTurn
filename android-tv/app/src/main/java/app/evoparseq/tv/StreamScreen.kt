package app.evoparseq.tv

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.focusable
import androidx.compose.foundation.layout.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.input.key.onPreviewKeyEvent
import kotlinx.coroutines.delay
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.tv.material3.Button
import androidx.tv.material3.Text

/** SurfaceView is the codec's producer endpoint, never a Bitmap/TextureView. */
@Composable
internal fun StreamScreen(model: TvModel, host: Host, close: () -> Unit) {
    val context = LocalContext.current
    val activity = context as MainActivity
    val controllers = activity.controllerInput
    val first = remember { FocusRequester() }
    var focusGranted by remember { mutableStateOf<Boolean?>(null) }
    val status = model.streamStatus
    val video = status?.optJSONObject("video")
    val width = video?.optInt("width") ?: 0
    val height = video?.optInt("height") ?: 0
    val exit = { model.stopStream(); close() }
    var visible by remember { mutableStateOf(true) }
    var interaction by remember { mutableStateOf(0) }
    var consumeCenterUp by remember { mutableStateOf(false) }
    val root = remember { FocusRequester() }
    val error = status?.optString("error").orEmpty()
    val streaming = status?.optString("state") == "Streaming"
    BackHandler { if (!visible) { visible = true; interaction++ } else exit() }
    LaunchedEffect(visible, streaming, error, activity.controllerWindowFocused) {
        controllers.enabled = !visible && streaming && error.isEmpty() && activity.controllerWindowFocused
    }
    LaunchedEffect(visible) {
        NativeCore.setOverlayVisible(visible)
        if (visible) first.requestFocus() else root.requestFocus()
    }
    LaunchedEffect(visible, interaction, streaming, error) {
        if (visible && streaming && error.isEmpty()) {
            delay(status?.optJSONObject("overlay")?.optLong("auto_hide_ms", 5000) ?: 5000)
            visible = false
        }
    }
    LaunchedEffect(error) { if (error.isNotEmpty()) { visible = true; interaction++ } }
    DisposableEffect(Unit) {
        val audio = context.getSystemService(Context.AUDIO_SERVICE) as AudioManager
        val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_GAME)
                .setContentType(AudioAttributes.CONTENT_TYPE_MOVIE).build())
            .setOnAudioFocusChangeListener { change ->
                if (change == AudioManager.AUDIOFOCUS_LOSS || change == AudioManager.AUDIOFOCUS_LOSS_TRANSIENT) model.stopStream()
            }.build()
        val granted = audio.requestAudioFocus(request) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
        focusGranted = granted
        onDispose { controllers.enabled = false; model.stopStream(); if (granted) audio.abandonAudioFocusRequest(request) }
    }
    BoxWithConstraints(Modifier.fillMaxSize().background(Color.Black)
        .focusRequester(root).onPreviewKeyEvent { event ->
            if (controllers.key(event.nativeKeyEvent)) return@onPreviewKeyEvent true
            val key = event.nativeKeyEvent
            val center = key.keyCode == android.view.KeyEvent.KEYCODE_DPAD_CENTER || key.keyCode == android.view.KeyEvent.KEYCODE_ENTER
            when {
                key.keyCode == android.view.KeyEvent.KEYCODE_MENU && key.action == android.view.KeyEvent.ACTION_UP -> true
                center && consumeCenterUp && key.action == android.view.KeyEvent.ACTION_UP -> { consumeCenterUp = false; true }
                key.action == android.view.KeyEvent.ACTION_DOWN && key.repeatCount == 0 && key.keyCode == android.view.KeyEvent.KEYCODE_MENU -> { visible = !visible; interaction++; true }
                key.action == android.view.KeyEvent.ACTION_DOWN && !visible && center -> { consumeCenterUp = true; visible = true; interaction++; true }
                else -> { if (key.action == android.view.KeyEvent.ACTION_DOWN) interaction++; false }
            }
        }.focusable()) {
        val ratio = if (width > 0 && height > 0) width.toFloat() / height else maxWidth / maxHeight
        val videoSize = if (maxWidth / maxHeight > ratio)
            Modifier.height(maxHeight).width(maxHeight * ratio)
        else Modifier.width(maxWidth).height(maxWidth / ratio)
        AndroidView(modifier = videoSize.align(Alignment.Center), factory = { ctx ->
            SurfaceView(ctx).also { surface ->
                // MediaCodec owns buffer geometry. Only the View's layout
                // follows the visible aspect ratio; do not resize its producer
                // buffers in response to codec-format callbacks.
                surface.keepScreenOn = true
                surface.isFocusable = false
                var started = false
                surface.holder.addCallback(object : SurfaceHolder.Callback {
                    override fun surfaceCreated(holder: SurfaceHolder) = Unit
                    override fun surfaceChanged(holder: SurfaceHolder, format: Int, w: Int, h: Int) {
                        if (!started && w > 0 && h > 0) {
                            started = true
                            model.startStream(holder.surface, host, w, h)
                        }
                    }
                    override fun surfaceDestroyed(holder: SurfaceHolder) { model.stopStream() }
                })
            }
        })
        if (visible) Column(Modifier.align(Alignment.TopStart).padding(24.dp)
            .background(Color(0xE6171D26)).padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                Button(onClick = { visible = false }, modifier = Modifier.focusRequester(first)) { Text("Hide overlay") }
                Button(onClick = { exit() }) { Text("Disconnect") }
            }
            Text(if (error.isNotEmpty()) error else "${host.name} | ${status?.optString("state") ?: "Connecting..."}", color = Color.White)
            val metrics = status?.optJSONObject("overlay")
            Row(horizontalArrangement = Arrangement.spacedBy(24.dp)) {
                Text(metrics?.optString("fps") ?: "N/A FPS", color = Color(0xFF71BCFF))
                Text("RTT ${metrics?.optString("rtt") ?: "N/A ms"}", color = Color.White)
                Text("RX ${metrics?.optString("bitrate") ?: "N/A Mbit/s"}", color = Color.White)
            }
            if (metrics != null) Text(metrics.optString("format"), color = Color(0xFFA8BED7))
            val network = status?.optJSONObject("network")
            Text("${network?.optString("transport", "WebRTC") ?: "Connecting"} | ${network?.optString("route", "Unverified") ?: "Unverified"}", color = Color(0xFFA8BED7))
            if (focusGranted == false) Text("Android did not grant audio focus", color = Color(0xFFFFB4AB))
            Text("Hides after 5 seconds. Press OK, Menu or Back to show controls.", color = Color(0xFFA8BED7))
        }
    }
}
