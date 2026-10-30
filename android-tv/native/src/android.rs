use crate::account::{self, Session};
use jni::{
    objects::{JObject, JString},
    sys::jstring,
    JNIEnv,
};
use serde_json::{json, Value};
use std::sync::Mutex;
use zeroize::Zeroizing;

// JNI calls run on the view model's single worker, never Android's UI thread.
// Only one foreground account is supported. No token crosses JNI or goes to disk.
static SESSION: Mutex<Option<Session>> = Mutex::new(None);

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
        *slot = Some(account::login(&login)?);
        Ok(json!({"ok":true}))
    })();
    reply(
        &mut env,
        result.unwrap_or_else(|e| json!({"error":e.to_string()})),
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
        result.unwrap_or_else(|e| json!({"error":e.to_string()})),
    )
}

#[no_mangle]
pub extern "system" fn Java_app_evoparseq_tv_NativeCore_signOut(_env: JNIEnv, _object: JObject) {
    if let Ok(mut slot) = SESSION.lock() {
        // Drop follows the existing bounded remote revocation and zeroization.
        drop(slot.take());
    }
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
