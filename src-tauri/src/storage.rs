use base64::{engine::general_purpose::STANDARD, Engine};
use std::{fs, io::Write, path::Path};

// DPAPI has the same byte format as v0.4.0: CurrentUser, no optional entropy.
#[repr(C)]
struct Blob {
    length: u32,
    data: *mut u8,
}

#[link(name = "crypt32")]
extern "system" {
    fn CryptProtectData(
        input: *const Blob,
        description: *const u16,
        entropy: *const Blob,
        reserved: *const std::ffi::c_void,
        prompt: *const std::ffi::c_void,
        flags: u32,
        output: *mut Blob,
    ) -> i32;
    fn CryptUnprotectData(
        input: *const Blob,
        description: *mut *mut u16,
        entropy: *const Blob,
        reserved: *const std::ffi::c_void,
        prompt: *const std::ffi::c_void,
        flags: u32,
        output: *mut Blob,
    ) -> i32;
}
#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(memory: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
}

fn crypt(bytes: &[u8], protect: bool) -> Result<Vec<u8>, String> {
    let input = Blob {
        length: u32::try_from(bytes.len()).map_err(|_| "Secret is too large")?,
        data: bytes.as_ptr() as *mut u8,
    };
    let mut output = Blob {
        length: 0,
        data: std::ptr::null_mut(),
    };
    // The input is alive for the call; DPAPI owns the output until LocalFree.
    let success = unsafe {
        if protect {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                &mut output,
            )
        }
    };
    if success == 0 {
        return Err("Windows could not protect/unprotect credentials for this user".into());
    }
    let data = unsafe {
        let result = if output.length == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(output.data, output.length as usize).to_vec()
        };
        LocalFree(output.data.cast());
        result
    };
    Ok(data)
}

pub fn protect(value: &str) -> Result<String, String> {
    crypt(value.as_bytes(), true).map(|bytes| STANDARD.encode(bytes))
}

pub fn unprotect(value: &str) -> Result<String, String> {
    let bytes = STANDARD
        .decode(value)
        .map_err(|_| "Invalid encrypted credential encoding")?;
    String::from_utf8(crypt(&bytes, false)?).map_err(|_| "Invalid decrypted credential".into())
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let raw = fs::read(path).map_err(|e| {
        format!(
            "Cannot read {}: {e}",
            path.file_name().unwrap_or_default().to_string_lossy()
        )
    })?;
    serde_json::from_slice(&raw).map_err(|_| {
        format!(
            "Invalid JSON in {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        )
    })
}

pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let data = serde_json::to_vec_pretty(value).map_err(|_| "Cannot serialize configuration")?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(path.parent().ok_or("Missing parent directory")?)
            .map_err(|e| format!("Cannot create configuration file: {e}"))?;
    temporary.write_all(&data).map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    // tempfile uses an atomic replacement on Windows, preserving the old file
    // if a browser/antivirus holds it open without delete sharing.
    temporary
        .persist(path)
        .map_err(|e| format!("Cannot replace configuration file: {}", e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dpapi_roundtrip_and_bad_ciphertext() {
        let secret = "árvíztűrő-test-secret";
        let protected = protect(secret).unwrap();
        assert_ne!(protected, secret);
        assert_eq!(unprotect(&protected).unwrap(), secret);
        assert!(unprotect("broken").is_err());
    }

    #[test]
    fn replacement_never_truncates_locked_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        write_json(&path, &serde_json::json!({"value": "old"})).unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert!(write_json(&path, &serde_json::json!({"value": "new"})).is_err());
        let old: serde_json::Value = read_json(&path).unwrap();
        assert_eq!(old["value"], "old");
        drop(lock);
        write_json(&path, &serde_json::json!({"value": "new"})).unwrap();
    }
}
