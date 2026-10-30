package app.evoparseq.tv

import android.media.MediaCodecInfo
import android.media.MediaCodecList

internal object DecoderInventory {
    fun choices(): String {
        val codecs = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos
        fun choose(mime: String): MediaCodecInfo? = codecs.firstOrNull { !it.isEncoder &&
            it.isHardwareAccelerated && it.supportedTypes.any { type -> type.equals(mime, true) } }
        fun choice(codec: MediaCodecInfo, mime: String) = org.json.JSONObject().apply {
            put("name", codec.name)
            put("low_latency", android.os.Build.VERSION.SDK_INT >= 30 &&
                codec.getCapabilitiesForType(mime).isFeatureSupported(MediaCodecInfo.CodecCapabilities.FEATURE_LowLatency))
        }
        val avc = choose("video/avc") ?: error("No hardware AVC decoder")
        val hevc = choose("video/hevc")
        return org.json.JSONObject().apply {
            put("avc", choice(avc, "video/avc"))
            put("hevc", hevc?.let { choice(it, "video/hevc") } ?: org.json.JSONObject.NULL)
            put("main10", hevc?.getCapabilitiesForType("video/hevc")?.profileLevels?.any {
                it.profile == MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10 } == true)
        }.toString()
    }
    /** Enumerate once on demand. A declaration is not a successful live decode. */
    fun describe(): String = buildString {
        appendLine("Android ${android.os.Build.VERSION.RELEASE} · API ${android.os.Build.VERSION.SDK_INT}")
        appendLine("${android.os.Build.MANUFACTURER} ${android.os.Build.MODEL}")
        appendLine("ABIs: ${android.os.Build.SUPPORTED_ABIS.joinToString()}")
        appendLine()
        val codecs = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos
        for ((mime, label) in listOf("video/avc" to "H.264", "video/hevc" to "HEVC")) {
            val matches = codecs.filter { !it.isEncoder && it.isHardwareAccelerated &&
                it.supportedTypes.any { type -> type.equals(mime, ignoreCase = true) } }
            appendLine(label)
            if (matches.isEmpty()) appendLine("No hardware decoder declared.")
            for (codec in matches) {
                appendLine(codec.name)
                runCatching {
                    val caps = codec.getCapabilitiesForType(mime)
                    val lowLatency = if (android.os.Build.VERSION.SDK_INT >= 30) {
                        caps.isFeatureSupported(MediaCodecInfo.CodecCapabilities.FEATURE_LowLatency).toString()
                    } else "not queryable on this Android version"
                    val supports1080 = caps.videoCapabilities.areSizeAndRateSupported(1920, 1080, 60.0)
                    appendLine("1080p60: $supports1080 · low-latency feature: $lowLatency")
                    if (mime == "video/hevc") {
                        val main10 = caps.profileLevels.any {
                            it.profile == MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10
                        }
                        appendLine("Main 10 profile: $main10")
                    }
                }.onFailure { appendLine("Capability query unavailable.") }
            }
            appendLine()
        }
        append("Declared capabilities only; Surface output and latency need live validation.")
    }
}
