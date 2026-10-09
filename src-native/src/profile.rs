//! Bounded guest profile, protected for the current Windows user with DPAPI.
//! Guest paths remain virtual; no guest-controlled string becomes a host path.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use windows_sys::Win32::{
    Foundation::LocalFree, Security::Cryptography::*, Storage::FileSystem::*,
};
const MAX_PROFILE: usize = 4 * 1024 * 1024;

#[derive(Default, Serialize, Deserialize)]
struct Data {
    version: u32,
    files: BTreeMap<String, Vec<u8>>,
    directories: BTreeSet<String>,
}

pub struct Profile {
    path: PathBuf,
    _lock: File,
}
impl Profile {
    pub fn user() -> Result<Arc<Self>> {
        let root = std::env::var_os("LOCALAPPDATA")
            .context("Windows local application data unavailable")?;
        Self::open(Path::new(&root).join("ParsecWebTurn").join("Native"))
    }
    fn open(directory: PathBuf) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&directory).context("Cannot create native profile directory")?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .share_mode(0)
            .open(directory.join("profile.lock"))
            .context("Native profile is already in use or inaccessible")?;
        Ok(Arc::new(Self {
            path: directory.join("profile.dpapi"),
            _lock: lock,
        }))
    }
    pub fn load(&self) -> Result<crate::filesystem::VirtualFs> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
            Err(_) => bail!("Cannot read native profile"),
        };
        let mut encrypted = Vec::new();
        file.take((MAX_PROFILE + 65537) as u64)
            .read_to_end(&mut encrypted)?;
        if encrypted.len() > MAX_PROFILE + 65536 {
            bail!("Native profile exceeds limit");
        }
        let plain = protect(&encrypted, false)
            .context("Cannot unlock native profile; existing data was preserved")?;
        if plain.len() > MAX_PROFILE {
            bail!("Native profile exceeds limit");
        }
        let data: Data = serde_json::from_slice(&plain)
            .context("Invalid native profile; existing data was preserved")?;
        if data.version != 1 || data.files.len() > 32 || data.directories.len() > 32 {
            bail!("Unsupported native profile");
        }
        let mut fs = crate::filesystem::VirtualFs::default();
        for path in &data.directories {
            if crate::filesystem::VirtualFs::path(path.as_bytes())
                .ok()
                .as_deref()
                != Some(path)
            {
                bail!("Invalid virtual profile directory");
            }
        }
        for (path, bytes) in &data.files {
            if crate::filesystem::VirtualFs::path(path.as_bytes())
                .ok()
                .as_deref()
                != Some(path)
                || bytes.len() > crate::filesystem::LIMIT
                || data.directories.contains(path)
            {
                bail!("Invalid virtual profile file");
            }
            let parent = path
                .rsplit_once('/')
                .map(|(p, _)| if p.is_empty() { "/" } else { p })
                .unwrap_or("/");
            if parent != "/" && !data.directories.contains(parent) {
                bail!("Virtual profile parent missing");
            }
        }
        fs.files = data.files;
        fs.directories.extend(data.directories);
        Ok(fs)
    }
    pub fn save(&self, fs: &crate::filesystem::VirtualFs) -> Result<()> {
        let data = Data {
            version: 1,
            files: fs
                .files
                .iter()
                .filter(|(p, _)| {
                    !p.starts_with('\0') && !p.ends_with(".lock") && p.as_str() != "/lock"
                })
                .map(|(p, b)| (p.clone(), b.clone()))
                .collect(),
            directories: fs.directories.clone(),
        };
        let plain = serde_json::to_vec(&data)?;
        if plain.len() > MAX_PROFILE {
            bail!("Native profile exceeds limit");
        }
        let encrypted = protect(&plain, true)?;
        let mut random = [0u8; 16];
        getrandom::fill(&mut random)
            .map_err(|_| anyhow::anyhow!("Profile random identifier unavailable"))?;
        let suffix = random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let temporary = self.path.with_file_name(format!("profile-{suffix}.tmp"));
        let result = (|| -> Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&encrypted)?;
            file.sync_all()?;
            drop(file);
            let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
            let destination: Vec<u16> =
                self.path.as_os_str().encode_wide().chain(Some(0)).collect();
            // Same-directory replacement; no partially written snapshot is published.
            if unsafe {
                MoveFileExW(
                    source.as_ptr(),
                    destination.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            } == 0
            {
                bail!("Cannot replace native profile");
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.context("Cannot save native profile")
    }
}

fn protect(input: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    let blob = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(input.len())?,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut output: CRYPT_INTEGER_BLOB = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &blob,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &blob,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        bail!("Windows profile protection failed");
    }
    // DPAPI allocates this buffer; copy before releasing with the documented allocator.
    let bytes = if output.cbData == 0 {
        Vec::new()
    } else if output.pbData.is_null() {
        bail!("Windows profile protection returned no buffer");
    } else {
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() }
    };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypted_profile_survives_restart_deletion_and_rejects_corruption() {
        let directory = std::env::temp_dir().join(format!(
            "parsec-profile-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let profile = Profile::open(directory.clone()).unwrap();
        assert!(Profile::open(directory.clone()).is_err());
        let mut fs = profile.load().unwrap();
        let handle = fs.open(b"appdata.json", 1, 2 | 64, 0).unwrap();
        fs.write(handle, b"synthetic-session-value").unwrap();
        fs.close(handle);
        profile.save(&fs).unwrap();
        let encrypted = std::fs::read(&profile.path).unwrap();
        assert!(!encrypted
            .windows(23)
            .any(|v| v == b"synthetic-session-value"));
        drop(profile);
        let profile = Profile::open(directory.clone()).unwrap();
        let mut restored = profile.load().unwrap();
        assert_eq!(restored.files["/appdata.json"], b"synthetic-session-value");
        restored.unlink(b"appdata.json").unwrap();
        profile.save(&restored).unwrap();
        assert!(profile.load().unwrap().files.is_empty());
        std::fs::write(&profile.path, b"corrupted-test-profile").unwrap();
        assert!(profile.load().is_err());
        assert_eq!(
            std::fs::read(&profile.path).unwrap(),
            b"corrupted-test-profile"
        );
        drop(profile);
        // Only the explicitly created, unique test directory is removed.
        std::fs::remove_dir_all(directory).unwrap();
    }
}
