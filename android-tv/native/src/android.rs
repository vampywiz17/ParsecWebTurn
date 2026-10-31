use crate::account::{self, Session};
use jni::{
    objects::{JByteArray, JObject, JString, JValue},
    sys::jstring,
    JNIEnv,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

// JNI calls run on the view model's single worker, never Android's UI thread.
// Only one foreground account is supported. Tokens cross JNI only as transient
// byte arrays for the Android Keystore adapter, never UI strings or diagnostics.
static SESSION: Mutex<Option<Arc<Session>>> = Mutex::new(None);

fn reply(env: &mut JNIEnv<'_>, value: Value) -> jstring {
    match env.new_string(value.to_string()) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(), // JNI leaves the allocation exception pending.
    }
}

fn text(env: &mut JNIEnv<'_>, value: &JString<'_>) -> jni::errors::Result<String> {
    Ok(env.get_string(value)?.into())
}

#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_signIn(
    mut env: JNIEnv,
    _object: JObject,
    email: JString,
    password: JString,
    tfa: JString,
) -> jstring {
    let result = (|| -> anyhow::Result<Value> {
        let login = account::Login {
            email: text(&mut env, &email)?,
            password: Zeroizing::new(text(&mut env, &password)?),
            tfa: Zeroizing::new(text(&mut env, &tfa)?),
        };
        let mut slot = SESSION
            .lock()
            .map_err(|_| anyhow::anyhow!("Account unavailable."))?;
        anyhow::ensure!(slot.is_none(), "Sign out before signing in again.");
        *slot = Some(Arc::new(account::login(&login)?));
        Ok(json!({"ok":true}))
    })();
    reply(
        &mut env,
        result.unwrap_or_else(|e| json!({"error":e.to_string()})),
    )
}

#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_rememberSession(
    mut env: JNIEnv,
    _object: JObject,
    vault: JObject,
) -> jstring {
    let result = (|| -> anyhow::Result<Value> {
        let slot = SESSION
            .lock()
            .map_err(|_| anyhow::anyhow!("Account unavailable"))?;
        let session = slot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Sign in first"))?;
        let token = env.byte_array_from_slice(session.token().as_bytes())?;
        env.call_method(&vault, "save", "([B)V", &[JValue::Object(token.as_ref())])?;
        session.remembered();
        Ok(json!({"ok":true}))
    })();
    // Clear pending platform exceptions; never return their text or token data.
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    reply(&mut env, result.unwrap_or_else(|_| json!({"error":"Signed in, but secure storage failed. Sign-in will not be remembered."})))
}

#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_restoreSession(
    mut env: JNIEnv,
    _object: JObject,
    token: JByteArray,
) -> jstring {
    let result = (|| -> anyhow::Result<Value> {
        anyhow::ensure!(
            env.get_array_length(&token)? <= 4096,
            "Invalid saved sign-in"
        );
        let bytes = Zeroizing::new(env.convert_byte_array(&token)?);
        let session = Session::from_token(std::str::from_utf8(&bytes)?)?;
        let mut slot = SESSION
            .lock()
            .map_err(|_| anyhow::anyhow!("Account unavailable"))?;
        anyhow::ensure!(slot.is_none(), "Already signed in");
        *slot = Some(Arc::new(session));
        Ok(json!({"ok":true}))
    })();
    reply(
        &mut env,
        result.unwrap_or_else(|_| json!({"error":"Saved sign-in unavailable. Sign in again."})),
    )
}

#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_hosts(
    mut env: JNIEnv,
    _object: JObject,
) -> jstring {
    let result = (|| -> anyhow::Result<Value> {
        let slot = SESSION
            .lock()
            .map_err(|_| anyhow::anyhow!("Account unavailable."))?;
        let session = slot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Sign in first."))?;
        let hosts = account::hosts(session)?;
        Ok(
            json!({"ok":true,"hosts":hosts.iter().map(|h| json!({"peer":h.peer,"name":h.name})).collect::<Vec<_>>()}),
        )
    })();
    reply(
        &mut env,
        result.unwrap_or_else(|e| json!({"error":e.to_string(),"expired":e.downcast_ref::<account::ExpiredSession>().is_some()})),
    )
}

#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_signOut(_env: JNIEnv, _object: JObject) {
    stop_setup();
    crate::stream::stop();
    if let Ok(mut slot) = SESSION.lock() {
        // Explicit logout revokes remembered sessions too; process exit does not.
        if let Some(session) = slot.take() {
            session.revoke();
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_startStream(
    mut env: JNIEnv,
    _object: JObject,
    surface: JObject,
    peer: JString,
    preferences: JString,
    choices: JString,
    width: jni::sys::jint,
    height: jni::sys::jint,
) -> jstring {
    let result = (|| -> anyhow::Result<Value> {
        stop_setup();
        let session = SESSION
            .lock()
            .map_err(|_| anyhow::anyhow!("Account unavailable"))?
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Sign in first"))?;
        let peer = text(&mut env, &peer)?;
        let prefs = crate::preferences::Preferences::parse(&text(&mut env, &preferences)?)?;
        let choices = text(&mut env, &choices)?;
        anyhow::ensure!(choices.len() <= 4096, "Decoder choices exceed limit");
        let choices: crate::video_android::Choices = serde_json::from_str(&choices)?;
        let size = (u32::try_from(width)?, u32::try_from(height)?);
        anyhow::ensure!(
            (1..=16384).contains(&size.0) && (1..=16384).contains(&size.1),
            "Invalid Surface size"
        );
        // Acquires a reference independent of this JNI local object. The decoder
        // worker retains it through MediaCodec stop/delete, even on UI teardown.
        let window = unsafe {
            ndk_sys::ANativeWindow_fromSurface(env.get_raw().cast(), surface.as_raw().cast())
        };
        anyhow::ensure!(!window.is_null(), "Video Surface unavailable");
        crate::stream::start(
            session,
            peer,
            prefs,
            crate::video_android::NativeWindow(window),
            choices,
            size,
        )?;
        Ok(json!({"ok":true}))
    })();
    reply(
        &mut env,
        result.unwrap_or_else(|e| json!({"error":e.to_string()})),
    )
}

#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_stopStream(_env: JNIEnv, _object: JObject) {
    stop_setup();
    crate::stream::stop();
}

#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_streamStatus(
    mut env: JNIEnv,
    _object: JObject,
) -> jstring {
    reply(&mut env, crate::stream::status())
}

#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_settings(
    mut env: JNIEnv,
    _object: JObject,
    current: JString,
    field: JString,
    index: jni::sys::jint,
) -> jstring {
    let result = (|| -> anyhow::Result<Value> {
        let current = text(&mut env, &current)?;
        let mut value = if current.is_empty() {
            crate::preferences::Preferences::default()
        } else {
            crate::preferences::Preferences::parse(&current)?
        };
        let field = text(&mut env, &field)?;
        if !field.is_empty() {
            value.select(&field, usize::try_from(index)?)?;
        }
        Ok(value.view())
    })();
    // Local malformed settings must not leak their raw content to the screen.
    reply(
        &mut env,
        result.unwrap_or_else(|_| json!({"error":"Could not read or change TV settings."})),
    )
}

#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_releaseSession(
    _env: JNIEnv,
    _object: JObject,
) {
    stop_setup();
    crate::stream::stop();
    if let Ok(mut slot) = SESSION.lock() {
        drop(slot.take());
    }
}

static LOCAL_SETUP: Mutex<Option<crate::local_setup::Setup>> = Mutex::new(None);
fn stop_setup() {
    if let Some(mut setup) = LOCAL_SETUP.lock().unwrap_or_else(|e| e.into_inner()).take() {
        setup.stop();
    }
}

#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_startLocalSetup(
    mut env: JNIEnv,
    _object: JObject,
    ip: JString,
) -> jstring {
    let result = (|| -> anyhow::Result<Value> {
        anyhow::ensure!(
            !crate::stream::active(),
            "Disconnect the stream before local setup"
        );
        stop_setup();
        let setup = crate::local_setup::Setup::start(text(&mut env, &ip)?.parse()?)?;
        let report = setup.snapshot();
        *LOCAL_SETUP.lock().unwrap_or_else(|e| e.into_inner()) = Some(setup);
        Ok(report)
    })();
    reply(&mut env,result.unwrap_or_else(|_|json!({"error":"Could not start local HTTPS setup. Connect the TV to a private LAN and try again."})))
}
#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_localSetupStatus(
    mut env: JNIEnv,
    _object: JObject,
) -> jstring {
    let report = LOCAL_SETUP
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|s| s.snapshot())
        .unwrap_or_else(|| json!({"closed":true}));
    reply(&mut env, report)
}
#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_stopLocalSetup(
    _env: JNIEnv,
    _object: JObject,
) {
    stop_setup();
}
#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_confirmLocalSetup(
    mut env: JNIEnv,
    _object: JObject,
    vault: JObject,
) -> jstring {
    let result = (|| -> anyhow::Result<Value> {
        let network = LOCAL_SETUP
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|s| s.pending())
            .ok_or_else(|| anyhow::anyhow!("No pending network settings"))?;
        let raw = Zeroizing::new(serde_json::to_vec(&network)?);
        let bytes = env.byte_array_from_slice(&raw)?;
        env.call_method(&vault, "save", "([B)V", &[JValue::Object(bytes.as_ref())])?;
        let report = json!({"ok":true,"network":network.summary()});
        *crate::network::CURRENT
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(network));
        stop_setup();
        Ok(report)
    })();
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    reply(
        &mut env,
        result.unwrap_or_else(
            |_| json!({"error":"Could not securely save network settings. Nothing was applied."}),
        ),
    )
}
#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_restoreNetwork(
    mut env: JNIEnv,
    _object: JObject,
    bytes: JByteArray,
) -> jstring {
    let result = (|| -> anyhow::Result<Value> {
        anyhow::ensure!(
            env.get_array_length(&bytes)? <= 32768,
            "Network settings exceed limit"
        );
        let raw = Zeroizing::new(env.convert_byte_array(&bytes)?);
        let n = crate::network::Network::parse(&raw)?;
        let report = json!({"ok":true,"network":n.summary()});
        *crate::network::CURRENT
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(n));
        Ok(report)
    })();
    reply(
        &mut env,
        result.unwrap_or_else(
            |_| json!({"error":"Saved network settings unavailable. Configure the relay again."}),
        ),
    )
}
#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_resetNetwork(
    mut env: JNIEnv,
    _object: JObject,
) -> jstring {
    stop_setup();
    *crate::network::CURRENT
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    reply(
        &mut env,
        json!({"ok":true,"network":crate::network::current().summary()}),
    )
}

#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_setOverlayVisible(
    _env: JNIEnv,
    _object: JObject,
    visible: jni::sys::jboolean,
) {
    crate::overlay::REQUESTED.store(visible != 0, std::sync::atomic::Ordering::Relaxed);
}
