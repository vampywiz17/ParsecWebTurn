package app.evoparseq.tv

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.tv.material3.Button
import androidx.tv.material3.Text

/** SurfaceView is the codec's producer endpoint, never a Bitmap/TextureView. */
@Composable
internal fun StreamScreen(model: TvModel, host: Host, close: () -> Unit) {
    val context = LocalContext.current
    val first = remember { FocusRequester() }
    var focusGranted by remember { mutableStateOf<Boolean?>(null) }
    val status = model.streamStatus
    val video = status?.optJSONObject("video")
    val width = video?.optInt("width") ?: 0
    val height = video?.optInt("height") ?: 0
    val exit = { model.stopStream(); close() }
    BackHandler { exit() }
    LaunchedEffect(Unit) { first.requestFocus() }
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
        onDispose { model.stopStream(); if (granted) audio.abandonAudioFocusRequest(request) }
    }
    BoxWithConstraints(Modifier.fillMaxSize().background(Color.Black)) {
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
        Column(Modifier.align(Alignment.TopStart).padding(16.dp)
            .background(Color(0xD010141C)).padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Button(onClick = { exit() }, modifier = Modifier.focusRequester(first)) { Text("Disconnect") }
            val error = status?.optString("error").orEmpty()
            Text(if (error.isNotEmpty()) error else "${host.name} · ${status?.optString("state") ?: "Connecting…"}", color = Color.White)
            val network = status?.optJSONObject("network")
            if (network != null) {
                val states = network.optJSONObject("states")
                Text("ICE ${states?.optString("ice") ?: "unknown"} · DTLS ${states?.optString("dtls") ?: "unknown"} · RX ${"%.1f".format(network.optDouble("received_mbps"))} Mbit/s", color = Color(0xFFA8BED7))
                if (error.isNotEmpty()) Text("Stage ${network.optString("failure_stage")} · channel ${network.optString("failure_channel")} · ${network.optString("read_error")}\n" +
                    "Control queue ${network.optInt("queued_control_messages")} · max message C/V/A ${network.optJSONArray("max_message_bytes")}", color = Color(0xFFFFB4AB))
                Text("Empty messages C/V/A ${network.optJSONArray("empty_messages")}", color = Color(0xFFA8BED7))
            }
            if (video != null && (!video.isNull("error") || video.optLong("submitted") == 0L)) {
                Text("${video.optString("stage")} · ${video.optString("decoder")}\n" +
                    "Input callbacks ${video.optLong("input_callbacks")} · queued to codec ${video.optLong("inputs")}\n" +
                    "Output callbacks ${video.optLong("output_callbacks")} · released to Surface ${video.optLong("submitted")}\n" +
                    "Queue ${video.optInt("queued")} · dropped ${video.optLong("dropped")} · ${video.optString("recovery_reason")}",
                    color = Color(0xFFA8BED7))
            }
            if (width > 0 && height > 0) Text("${video?.optString("codec")} · $width × $height · ${video?.optInt("depth")}-bit", color = Color(0xFFA8BED7))
            if (video != null && video.optLong("submitted") > 0L) Text("Video queue ${video.optInt("queued")} · wait ${video.optDouble("queue_wait_ms").toInt()} ms · catch-up skipped ${video.optLong("skipped")}\n" +
                "Compressed drops ${video.optLong("dropped")} · Surface frames ${video.optLong("submitted")}", color = Color(0xFFA8BED7))
            val audioStatus = status?.optJSONObject("audio")
            if (focusGranted == false) Text("Android did not grant audio focus", color = Color(0xFFFFB4AB))
            val audioOutput = status?.optJSONObject("audio_output")
            if (audioStatus != null && audioOutput != null) {
                Text("Audio ${audioOutput.optString("state")} · decoded ${audioStatus.optLong("packets_decoded")} · errors ${audioStatus.optLong("decode_errors")}\n" +
                    "Frames written ${audioOutput.optLong("frames_written")} · read ${audioOutput.optLong("frames_read")} · underruns ${audioOutput.optInt("xruns")}\n" +
                    "Callbacks ${audioOutput.optLong("callbacks")} · PCM frames ${audioOutput.optLong("pcm_frames")} · peak ${audioOutput.optInt("peak")}/32768\n" +
                    "Queued ${audioStatus.optInt("queued_frames")} · silence frames ${audioOutput.optLong("silent_frames")} · buffer ${audioOutput.optInt("buffer_frames")} frames",
                    color = Color(0xFFA8BED7))
            }
            Text("View and audio preview · controller input follows next", color = Color(0xFFA8BED7))
        }
    }
}
