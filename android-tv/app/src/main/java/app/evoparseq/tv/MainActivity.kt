package app.evoparseq.tv

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.FocusDirection
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.colorResource
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.input.key.*
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.tv.material3.*

class MainActivity : ComponentActivity() {
    private val model: TvModel get() = androidx.lifecycle.ViewModelProvider(this)[TvModel::class.java]
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            TvTheme { TvApp(model) }
        }
    }
    override fun onStop() { model.closeLocalSetup(); model.stopStream(); super.onStop() }
}

@Composable
private fun TvApp(model: TvModel) {
    var diagnostics by rememberSaveable { mutableStateOf(false) }
    var settingsOpen by rememberSaveable { mutableStateOf(false) }
    var selected by remember { mutableStateOf<Host?>(null) }
    if (selected != null) {
        StreamScreen(model, selected!!) { selected = null }
        return
    }
    BackHandler(diagnostics || settingsOpen || selected != null) {
        diagnostics = false; settingsOpen = false; selected = null
    }
    val first = remember { FocusRequester() }
    LaunchedEffect(model.signedIn, diagnostics, settingsOpen, selected?.peer) { first.requestFocus() }
    LaunchedEffect(model.signedIn) { if (model.signedIn) model.refresh() }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).imePadding()
        .padding(horizontal = 48.dp, vertical = 28.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            Image(painterResource(R.drawable.evoparseq_symbol), null, Modifier.size(48.dp)
                .background(colorResource(R.color.icon_background), CircleShape))
            Column {
                Text("EVO PARSEQ", style = MaterialTheme.typography.headlineMedium)
                Text("Your computers. On your TV.", color = Color(0xFFA8BED7))
            }
        }
        Spacer(Modifier.height(20.dp))
        Box(Modifier.weight(1f).fillMaxWidth()) {
            when {
                settingsOpen -> TvSettings(model, first) { settingsOpen = false }
                diagnostics -> {
                    Column(Modifier.fillMaxSize()) {
                        Button(onClick = { diagnostics = false }, modifier = Modifier.focusRequester(first)) {
                            Text("Back")
                        }
                        Spacer(Modifier.height(16.dp))
                        Text(model.inventory.ifEmpty { "Reading decoder capabilities…" },
                        modifier = Modifier.weight(1f).verticalScroll(rememberScrollState()),
                        style = MaterialTheme.typography.bodyLarge)
                    }
                }
                !model.signedIn -> SignIn(model, first,
                inspect = { diagnostics = true; model.inspectDecoders() },
                settings = { settingsOpen = true })
                else -> {
                    Column(Modifier.fillMaxSize()) {
                        Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                            Button(onClick = model::refresh, enabled = !model.busy,
                            modifier = Modifier.focusRequester(first)) { Text("Refresh computers") }
                            Button(onClick = { diagnostics = true; model.inspectDecoders() },
                            enabled = !model.busy) { Text("Device capabilities") }
                            Button(onClick = { settingsOpen = true }, enabled = !model.busy) { Text("Settings") }
                            Button(onClick = model::signOut, enabled = !model.busy) { Text("Sign out") }
                        }
                        Spacer(Modifier.height(20.dp))
                        Text("Your computers", style = MaterialTheme.typography.headlineSmall)
                        Spacer(Modifier.height(12.dp))
                        LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(12.dp),
                        contentPadding = PaddingValues(8.dp)) {
                            items(model.hosts, key = { it.peer }) { host ->
                                Card(onClick = { selected = host }, modifier = Modifier.fillMaxWidth()) {
                                    Column(Modifier.padding(20.dp)) {
                                        Text(host.name, style = MaterialTheme.typography.titleLarge)
                                        Text("Connect · view and audio", color = LocalContentColor.current.copy(alpha = 0.8f))
                                    }
                                }
                            }
                            if (model.hosts.isEmpty() && !model.busy) item {
                                Text("No computers returned. Check that your host is online.")
                            }
                        }
                    }
                }
            }
        }
        // One footer, always two lines high. Progress and feedback replace the
        // preview caption instead of adding rows and shrinking the form/IME area.
        val feedback = when {
            model.busy -> "Working…"
            model.message.isNotEmpty() -> model.message
            else -> "Android TV streaming preview · controller input is not connected yet"
        }
        Text(feedback,
            modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
            minLines = 2, maxLines = 2, overflow = TextOverflow.Ellipsis,
            fontSize = 16.sp, lineHeight = 20.sp,
            color = when {
                model.busy -> Color(0xFFA8BED7)
                model.message.isNotEmpty() -> Color(0xFFFFC6A0)
                else -> Color(0xFF9CACC0)
            })
    }
}

@Composable
private fun SignIn(model: TvModel, first: FocusRequester, inspect: () -> Unit, settings: () -> Unit) {
    var email by rememberSaveable { mutableStateOf("") }
    // Secrets deliberately do not use rememberSaveable / SavedStateHandle.
    var password by remember { mutableStateOf("") }
    var code by remember { mutableStateOf("") }
    Column(Modifier.fillMaxSize()) {
        Text("Sign in to your Parsec account", style = MaterialTheme.typography.headlineSmall)
        Spacer(Modifier.height(12.dp))
        // The heading stays above the scroll viewport. D-pad/IME focus may move
        // the form, but cannot leave a half-clipped heading below the app header.
        Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("Independent client · not affiliated with Parsec/Unity.\n" +
                "Sign-in is encrypted on this TV. Your password and two-factor code are not saved.")
            AccountField("Email", email, { if (it.length <= 320) email = it },
                KeyboardType.Email, modifier = Modifier.focusRequester(first))
            AccountField("Password", password, { if (it.length <= 4096) password = it },
                KeyboardType.Password, secret = true)
            AccountField("Two-factor code (if enabled)", code, { if (it.length <= 32) code = it },
                KeyboardType.NumberPassword, secret = true, done = true)
            Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                Button(onClick = {
                    model.signIn(email, password, code)
                    // Keep the password only in this unsaved form while retrying an
                    // expired 2FA code. Leaving the form/signing in drops the fields.
                    code = ""
                }, enabled = !model.busy && email.isNotBlank() && password.isNotEmpty()) { Text("Sign in") }
                Button(onClick = inspect, enabled = !model.busy) { Text("Device capabilities") }
                Button(onClick = settings, enabled = !model.busy) { Text("Settings") }
            }
        }
    }
}

@Composable
private fun AccountField(label: String, value: String, change: (String) -> Unit,
    type: KeyboardType, modifier: Modifier = Modifier, secret: Boolean = false, done: Boolean = false) {
    var focused by remember { mutableStateOf(false) }
    val focus = LocalFocusManager.current
    val keyboard = LocalSoftwareKeyboardController.current
    val keyboardVisible = WindowInsets.ime.getBottom(LocalDensity.current) > 0
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(label, style = MaterialTheme.typography.labelLarge)
        BasicTextField(value, change, singleLine = true,
            textStyle = TextStyle(color = Color.White, fontSize = 18.sp),
            cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
            keyboardOptions = KeyboardOptions(keyboardType = type,
                imeAction = if (done) ImeAction.Done else ImeAction.Next),
            keyboardActions = KeyboardActions(onNext = {
                keyboard?.hide(); focus.moveFocus(FocusDirection.Down)
            }, onDone = {
                keyboard?.hide(); focus.moveFocus(FocusDirection.Down)
            }),
            visualTransformation = if (secret) PasswordVisualTransformation() else VisualTransformation.None,
            modifier = modifier.width(480.dp).onFocusChanged { focused = it.isFocused }
                .semantics { contentDescription = label }
                .onPreviewKeyEvent { event ->
                    if (event.type != KeyEventType.KeyDown || keyboardVisible) false
                    else when (event.key) {
                        Key.DirectionDown -> focus.moveFocus(FocusDirection.Down)
                        Key.DirectionUp -> focus.moveFocus(FocusDirection.Up)
                        Key.DirectionCenter, Key.Enter -> { keyboard?.show(); true }
                        else -> false
                    }
                }
                .border(2.dp, if (focused) Color(0xFF71BCFF) else Color(0xFF465569), RoundedCornerShape(8.dp))
                .background(Color(0xFF202936), RoundedCornerShape(8.dp)).padding(14.dp))
    }
}

@Composable
private fun TvSettings(model: TvModel, first: FocusRequester, back: () -> Unit) {
    var category by remember { mutableStateOf("Network") }
    var choosing by remember { mutableStateOf<String?>(null) }
    var restoreKey by remember { mutableStateOf("") }
    val rowFocus = remember { FocusRequester() }
    val optionFocus = remember { FocusRequester() }
    val rows = model.settings?.getJSONArray("rows")
    val values = if (rows == null) emptyList() else (0 until rows.length()).map { rows.getJSONObject(it) }
    val choice = values.find { it.getString("key") == choosing }
    if (model.localSetup != null) { LocalSetupScreen(model); return }
    LaunchedEffect(Unit) { first.requestFocus() }
    BackHandler(choosing != null) { choosing = null }
    LaunchedEffect(choosing, model.busy) {
        if (!model.busy && choosing != null) optionFocus.requestFocus()
        else if (!model.busy && restoreKey.isNotEmpty()) rowFocus.requestFocus()
    }
    Row(Modifier.fillMaxSize(), horizontalArrangement = Arrangement.spacedBy(28.dp)) {
        Column(Modifier.width(180.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Button(onClick = back, modifier = Modifier.focusRequester(first).fillMaxWidth()) { Text("Back") }
            Text("SETTINGS", style = MaterialTheme.typography.titleMedium, color = Color(0xFF71BCFF))
            listOf("Network", "Video").forEach { name ->
                Button(onClick = { category = name; choosing = null; restoreKey = "" }, modifier = Modifier.fillMaxWidth()) {
                    Text((if (category == name) "> " else "") + name)
                }
            }
        }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Text(choice?.getString("label") ?: category, style = MaterialTheme.typography.headlineSmall)
            Text("Requests apply to your next connection. The host chooses the actual video format.", color = Color(0xFFA8BED7))
            if (model.settings == null) Button(onClick = model::readSettings, enabled = !model.busy) { Text("Reload settings") }
            if (choice == null) {
                values.filter { it.optString("group") == category }.forEach { row ->
                    val key = row.getString("key")
                    val choices = row.getJSONArray("choices")
                    Button(onClick = { restoreKey = key; choosing = key }, enabled = !model.busy,
                        modifier = Modifier.fillMaxWidth().then(if (key == restoreKey) Modifier.focusRequester(rowFocus) else Modifier)) {
                        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            Text(row.getString("label"), style = MaterialTheme.typography.labelMedium)
                            Text(choices.getString(row.getInt("selected")))
                        }
                    }
                }
                if (category == "Network") {
                    Spacer(Modifier.height(8.dp))
                    Text("STUN / TURN relay", style = MaterialTheme.typography.titleLarge)
                    Text("Provider: ${model.networkSummary.optString("provider", "direct")} | Force relay: ${model.networkSummary.optBoolean("force_relay")}")
                    Button(onClick = model::openLocalSetup, enabled = !model.busy, modifier = Modifier.fillMaxWidth()) { Text("Set up from phone or computer") }
                    Button(onClick = model::resetNetwork, enabled = !model.busy, modifier = Modifier.fillMaxWidth()) { Text("Remove saved relay settings") }
                    Text("Local HTTPS pairing only. Credentials are encrypted on this TV.", color = Color(0xFFA8BED7))
                } else {
                    Text("Dynamic FPS reduces traffic for still images. Fixed FPS requests continuous frames; it does not duplicate frames on the TV.", color = Color(0xFFA8BED7))
                }
            } else {
                val choices = choice.getJSONArray("choices")
                val selected = choice.getInt("selected")
                repeat(choices.length()) { index ->
                    Button(onClick = { model.selectSetting(choice.getString("key"), index); choosing = null }, enabled = !model.busy,
                        modifier = Modifier.fillMaxWidth().then(if (index == selected) Modifier.focusRequester(optionFocus) else Modifier)) {
                        Text((if (index == selected) "> " else "") + choices.getString(index))
                    }
                }
            }
        }
    }
}
