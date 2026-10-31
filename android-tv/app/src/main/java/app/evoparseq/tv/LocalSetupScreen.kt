package app.evoparseq.tv

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.tv.material3.Button
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.Text

@Composable
internal fun LocalSetupScreen(model: TvModel) {
    val report = model.localSetup ?: return
    val first = remember { FocusRequester() }
    BackHandler { model.closeLocalSetup() }
    LaunchedEffect(Unit) { first.requestFocus() }
    DisposableEffect(Unit) { onDispose { model.closeLocalSetup() } }
    Row(Modifier.fillMaxSize(), horizontalArrangement = Arrangement.spacedBy(24.dp)) {
        Column(Modifier.width(220.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Button(onClick = model::closeLocalSetup, modifier = Modifier.focusRequester(first)) { Text("Close setup") }
            val qr = report.optJSONArray("qr")
            if (qr != null) Canvas(Modifier.size(210.dp).background(Color.White)) {
                val count = qr.length()
                val cell = size.minDimension / (count + 8)
                repeat(count) { y -> repeat(count) { x ->
                    if (qr.getJSONArray(y).getBoolean(x)) drawRect(Color.Black, Offset((x + 4) * cell, (y + 4) * cell), Size(cell + 0.25f, cell + 0.25f))
                } }
            }
            Text("Pairing code", color = Color(0xFFA8BED7))
            Text(report.optString("pin"), style = MaterialTheme.typography.headlineLarge)
        }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("Set up on your local network", style = MaterialTheme.typography.headlineSmall)
            Text("Scan the QR code or open this address on a phone/computer connected to the same LAN:")
            Text(report.optString("url"), color = Color(0xFF71BCFF))
            Text("HTTPS uses a temporary local certificate. Before entering credentials, compare its SHA-256 fingerprint in the browser's certificate details with this TV:")
            Text(report.optString("fingerprint"), style = MaterialTheme.typography.bodySmall)
            Text("No external setup server. Expires in five minutes. Closing this page stops the local server.", color = Color(0xFFA8BED7))
            if (report.optBoolean("closed")) Text("Setup closed or expired. Close this screen and start again.", color = Color(0xFFFFB4AB))
            val pending = report.optJSONObject("pending")
            if (pending != null) {
                Text("New settings received", style = MaterialTheme.typography.titleLarge)
                Text("Provider: ${pending.optString("provider")} | Force relay: ${pending.optBoolean("force_relay")}")
                Text("The secret values are hidden. Apply only if you submitted these settings.")
                Button(onClick = model::confirmLocalSetup, enabled = !model.busy && !report.optBoolean("closed")) { Text("Approve and save on TV") }
            }
        }
    }
}
