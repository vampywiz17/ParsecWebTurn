//! Optional vendor runtime installation. Never called from the video worker.
//! Only the exact pinned NVIDIA archive is accepted. Its two needed entries
//! are fixed deflate streams; this is deliberately not a general ZIP extractor.
use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::Duration,
};

pub const DLL_HASH: &str = "c3d88eea5ff7a548edefa66414cf6e77464d0947277c904f324dd23abf58a1ed";
const ARCHIVE_HASH: &str = "abf4f34e2b5a618e355b0d5a0365d8ecc3db4396e756e4c850a867e1ae2ed69e";
const ARCHIVE_SIZE: u64 = 63_493_768;
const DLL_SIZE: u64 = 19_140_144;
const URL: &str = "https://api.ngc.nvidia.com/v2/models/nvidia/multimedia/dlpp/versions/1.5/files/RTX_Video_SDK_v1.1.0.zip";
pub const PROMPT: &str = "Download the optional NVIDIA RTX Video Super Resolution runtime from NVIDIA?\n\nThe download is about 61 MB. The checked NVIDIA DLL (about 19 MB) will be stored under %LOCALAPPDATA%/EvoParseq/vsr/1.1.0 for your Windows user (no administrator rights required). Sign-in opens after setup finishes.\n\nVSR requires a compatible RTX GPU/driver, is off by default and may increase latency. The NVIDIA SDK license is included in the application package. Choosing No keeps ordinary GPU video available.";
pub const RESTART_HINT: &str =
    " Restart the app to download the approved NVIDIA VSR runtime before sign-in.";

fn directory_under(local: &Path) -> PathBuf {
    local.join("EvoParseq").join("vsr").join("1.1.0")
}
fn local_directory() -> Result<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .context("Windows local application data folder unavailable")?;
    let local = PathBuf::from(local);
    ensure!(
        local.is_absolute(),
        "Windows local application data folder must be absolute"
    );
    Ok(local)
}
pub fn directory() -> Result<PathBuf> {
    Ok(directory_under(&local_directory()?))
}
pub fn present() -> bool {
    directory().is_ok_and(|root| root.join("nvngx_vsr.dll").is_file())
}
fn hash(mut file: &File) -> Result<String> {
    let mut digest = Sha256::new();
    std::io::copy(&mut file, &mut digest)?;
    Ok(format!("{:x}", digest.finalize()))
}
fn verify(path: &Path, size: u64, expected: &str) -> Result<()> {
    let file = File::open(path)?;
    ensure!(
        file.metadata()?.len() == size && hash(&file)? == expected,
        "NVIDIA runtime checksum mismatch"
    );
    Ok(())
}
pub fn validated_directory() -> Result<PathBuf> {
    let root = directory()?;
    verify(&root.join("nvngx_vsr.dll"), DLL_SIZE, DLL_HASH)
        .context("VSR runtime missing or invalid; approve its download in Settings and restart")?;
    Ok(std::fs::canonicalize(root)?)
}
/// Reuse only our previously pinned runtime, not arbitrary driver/application DLLs.
pub fn reuse_previous(data: &Path) -> Result<bool> {
    let root = directory()?;
    if present() {
        validated_directory()?;
        return Ok(true);
    }
    let local = local_directory()?;
    let candidates = [
        data.join("vsr/1.1.0"),
        local.join("EvoParseq/rtx-video").join(DLL_HASH),
    ];
    reuse_from(&root, &candidates)
}
fn reuse_from(root: &Path, candidates: &[PathBuf]) -> Result<bool> {
    for old in candidates {
        let source = old.join("nvngx_vsr.dll");
        if !source.is_file() {
            continue;
        }
        verify(&source, DLL_SIZE, DLL_HASH)?;
        std::fs::create_dir_all(root)?;
        let temporary = Temporary(root.join("runtime.tmp"));
        std::fs::copy(&source, &temporary.0)?;
        verify(&temporary.0, DLL_SIZE, DLL_HASH)?;
        std::fs::rename(&temporary.0, root.join("nvngx_vsr.dll"))?;
        let license = old.join("NVIDIA-RTX-VIDEO-LICENSE.pdf");
        if license.is_file() {
            let _ = std::fs::copy(license, root.join("NVIDIA-RTX-VIDEO-LICENSE.pdf"));
        }
        // Leave the old cache intact: another running/older EXE may still use it.
        return Ok(true);
    }
    Ok(false)
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub fn install() -> Result<()> {
    let root = directory()?;
    std::fs::create_dir_all(&root)?;
    let temporary = Temporary(root.join("sdk-download.tmp"));
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.url().scheme() != "https" || attempt.previous().len() > 5 {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()?;
    let response = client
        .get(URL)
        .send()
        .context("Cannot contact NVIDIA download service")?
        .error_for_status()
        .context("NVIDIA download service rejected the request")?;
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .context("NVIDIA download location missing")?
        .to_str()?;
    let location = reqwest::Url::parse(location)?;
    ensure!(
        location.scheme() == "https",
        "NVIDIA download must use HTTPS"
    );
    let response = client
        .get(location)
        .send()
        .map_err(|_| anyhow::anyhow!("NVIDIA download failed"))?
        .error_for_status()
        .map_err(|_| anyhow::anyhow!("NVIDIA download was rejected"))?;
    let mut archive = File::create(&temporary.0)?;
    let count = std::io::copy(&mut response.take(ARCHIVE_SIZE + 1), &mut archive)?;
    drop(archive);
    ensure!(count == ARCHIVE_SIZE, "NVIDIA SDK download size mismatch");
    verify(&temporary.0, ARCHIVE_SIZE, ARCHIVE_HASH)?;
    // These offsets are valid ONLY after checking the complete immutable SDK
    // 1.1.0 archive hash above. No filenames from an archive become filesystem paths.
    extract(
        &temporary.0,
        &root.join("NVIDIA-RTX-VIDEO-LICENSE.pdf"),
        62_433_254,
        131_214,
        136_535,
    )?;
    let dll = Temporary(root.join("runtime.tmp"));
    extract(&temporary.0, &dll.0, 37_964_664, 14_455_982, DLL_SIZE)?;
    verify(&dll.0, DLL_SIZE, DLL_HASH)?;
    std::fs::rename(&dll.0, root.join("nvngx_vsr.dll"))?;
    crate::event_log::record(
        "INFO",
        "vsr_runtime_installed",
        serde_json::json!({"sdk_version":"1.1.0","sha256":DLL_HASH}),
    );
    Ok(())
}
fn extract(archive: &Path, output: &Path, offset: u64, compressed: u64, size: u64) -> Result<()> {
    let mut input = File::open(archive)?;
    input.seek(SeekFrom::Start(offset))?;
    let decoder = flate2::read::DeflateDecoder::new(input.take(compressed));
    let mut file = File::create(output)?;
    ensure!(
        std::io::copy(&mut decoder.take(size + 1), &mut file)? == size,
        "NVIDIA runtime extraction size mismatch"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_previous_cache_is_not_migrated() {
        let fixture =
            std::env::temp_dir().join(format!("evoparseq-vsr-migrate-{}", std::process::id()));
        let old = fixture.join("portable/vsr/1.1.0");
        let target = directory_under(&fixture.join("local-app-data"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("nvngx_vsr.dll"), b"not NVIDIA code").unwrap();
        assert!(reuse_from(&target, std::slice::from_ref(&old)).is_err());
        assert!(!target.exists());
        assert!(old.join("nvngx_vsr.dll").exists());
        std::fs::remove_dir_all(fixture).unwrap();
    }
    #[test]
    fn mismatched_vendor_runtime_is_rejected_before_loading() {
        let path =
            std::env::temp_dir().join(format!("evoparseq-vsr-corrupt-{}", std::process::id()));
        std::fs::write(&path, b"not NVIDIA code").unwrap();
        assert!(verify(&path, 15, DLL_HASH).is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn pinned_archive_stream_offsets_reproduce_the_official_runtime() {
        let Some(sdk) = std::env::var_os("EVOPARSEQ_RTX_VIDEO_SDK") else {
            return;
        };
        let archive = PathBuf::from(sdk)
            .parent()
            .unwrap()
            .join("RTX_Video_SDK_v1.1.0.zip");
        verify(&archive, ARCHIVE_SIZE, ARCHIVE_HASH).unwrap();
        let path =
            std::env::temp_dir().join(format!("evoparseq-vsr-extract-{}", std::process::id()));
        extract(&archive, &path, 37_964_664, 14_455_982, DLL_SIZE).unwrap();
        verify(&path, DLL_SIZE, DLL_HASH).unwrap();
        let migrated = path.with_extension("cache");
        let previous = path.with_extension("old");
        std::fs::create_dir_all(&previous).unwrap();
        std::fs::rename(&path, previous.join("nvngx_vsr.dll")).unwrap();
        assert!(reuse_from(&migrated, std::slice::from_ref(&previous)).unwrap());
        verify(&migrated.join("nvngx_vsr.dll"), DLL_SIZE, DLL_HASH).unwrap();
        assert!(previous.join("nvngx_vsr.dll").exists());
        std::fs::remove_dir_all(previous).unwrap();
        std::fs::remove_dir_all(migrated).unwrap();
    }
}
