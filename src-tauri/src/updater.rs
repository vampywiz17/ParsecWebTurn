use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::PathBuf, process::Command, sync::Mutex, time::Duration};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const API: &str = "https://api.github.com/repos/vampywiz17/ParsecWebTurn/releases/latest";
const REPOSITORY: &str = "https://github.com/vampywiz17/ParsecWebTurn";
const MAX_EXE: usize = 128 * 1024 * 1024;

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub current_version: String,
    pub version: Option<String>,
    pub status: String,
    pub message: String,
    pub notes: String,
    pub busy: bool,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    size: u64,
    digest: Option<String>,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    body: Option<String>,
    assets: Vec<Asset>,
}

struct Staged {
    directory: tempfile::TempDir,
    digest: String,
}

pub struct State {
    inner: Mutex<Inner>,
}

struct Inner {
    view: View,
    staged: Option<Staged>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                view: View {
                    current_version: env!("CARGO_PKG_VERSION").into(),
                    status: "idle".into(),
                    message: "Ready to check GitHub releases.".into(),
                    ..View::default()
                },
                staged: None,
            }),
        }
    }
}

fn version(value: &str) -> Result<[u64; 3], String> {
    let parts: Vec<_> = value
        .strip_prefix('v')
        .unwrap_or(value)
        .split('.')
        .collect();
    if parts.len() != 3
        || parts.iter().any(|p| {
            p.is_empty()
                || !p.bytes().all(|b| b.is_ascii_digit())
                || (p.len() > 1 && p.starts_with('0'))
        })
    {
        return Err("Release version is not a stable semantic version.".into());
    }
    Ok([
        parts[0].parse().map_err(|_| "Invalid version")?,
        parts[1].parse().map_err(|_| "Invalid version")?,
        parts[2].parse().map_err(|_| "Invalid version")?,
    ])
}

fn digest(value: &str) -> Result<String, String> {
    let value = value.strip_prefix("sha256:").unwrap_or(value);
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Release has no valid SHA-256 digest.".into());
    }
    Ok(value.to_ascii_lowercase())
}

fn verify(data: &[u8], expected_size: u64, expected_digest: &str) -> Result<(), String> {
    if data.len() > MAX_EXE
        || data.len() as u64 != expected_size
        || !data.starts_with(b"MZ")
        || format!("{:x}", Sha256::digest(data)) != digest(expected_digest)?
    {
        return Err("Update executable integrity verification failed.".into());
    }
    Ok(())
}

fn asset<'a>(release: &'a Release, name: &str) -> Result<&'a Asset, String> {
    let selected: Vec<_> = release.assets.iter().filter(|a| a.name == name).collect();
    if selected.len() != 1 {
        return Err(format!("Release must contain exactly one {name}."));
    }
    let asset = selected[0];
    let expected = format!("{REPOSITORY}/releases/download/{}/{name}", release.tag_name);
    if asset.browser_download_url != expected {
        return Err("Release asset is outside the expected repository.".into());
    }
    Ok(asset)
}

fn checksum(text: &str) -> Result<String, String> {
    let entries: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            (fields.len() == 2 && fields[1] == "ParsecWebTurn.exe").then(|| fields[0])
        })
        .collect();
    if entries.len() != 1 {
        return Err("Missing or duplicate EXE checksum.".into());
    }
    digest(entries[0])
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("ParsecWebTurn/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let url = attempt.url();
            if attempt.previous().len() >= 5
                || url.scheme() != "https"
                || !matches!(
                    url.host_str(),
                    Some(
                        "github.com"
                            | "release-assets.githubusercontent.com"
                            | "objects.githubusercontent.com"
                            | "github-releases.githubusercontent.com"
                    )
                )
            {
                attempt.error("Untrusted update redirect")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| "Cannot initialize update network client.".into())
}

async fn bytes(client: &reqwest::Client, url: &str, max: usize) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "Cannot reach GitHub. Try again later.")?
        .error_for_status()
        .map_err(|_| "GitHub update request failed or was rate limited.")?;
    if response
        .content_length()
        .is_some_and(|size| size > max as u64)
    {
        return Err("Update response is too large.".into());
    }
    let mut result = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Update download was interrupted.")?
    {
        if result.len() + chunk.len() > max {
            return Err("Update response is too large.".into());
        }
        result.extend_from_slice(&chunk);
    }
    Ok(result)
}

pub fn view(app: &tauri::AppHandle) -> Result<View, String> {
    Ok(app
        .state::<State>()
        .inner
        .lock()
        .map_err(|_| "Update lock failed")?
        .view
        .clone())
}

pub async fn show(app: &tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("updates") {
        window.show().map_err(|e| e.to_string())?;
        let _ = window.unminimize();
        let _ = window.set_focus();
        return Ok(());
    }
    let root = app.state::<crate::AppState>().root.clone();
    WebviewWindowBuilder::new(app, "updates", WebviewUrl::App("updates.html".into()))
        .title("ParsecWebTurn — Updates")
        .theme(Some(tauri::Theme::Dark))
        .inner_size(620.0, 650.0)
        .min_inner_size(500.0, 440.0)
        .data_directory(root.join("WebView2Profile"))
        .additional_browser_args(crate::BROWSER_ARGS)
        .on_navigation(crate::local_url)
        .build()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub async fn check(app: &tauri::AppHandle, manual: bool) -> Result<(), String> {
    if manual {
        show(app).await?;
    }
    {
        let state = app.state::<State>();
        let mut inner = state.inner.lock().map_err(|_| "Update lock failed")?;
        if inner.view.busy || inner.staged.is_some() {
            return Ok(());
        }
        inner.view.busy = true;
        inner.view.status = "checking".into();
        inner.view.message = "Checking the latest stable GitHub release…".into();
    }
    let result = prepare(app).await;
    let available;
    {
        let state = app.state::<State>();
        let mut inner = state.inner.lock().map_err(|_| "Update lock failed")?;
        inner.view.busy = false;
        match result {
            Ok(Some(staged)) => {
                inner.staged = Some(staged);
                inner.view.status = "ready".into();
                inner.view.message = "Update downloaded and verified. Restart when you are ready; your current session will disconnect.".into();
                available = true;
            }
            Ok(None) => {
                inner.view.status = "current".into();
                inner.view.message = "You are using the latest stable version.".into();
                available = false;
            }
            Err(error) => {
                inner.view.status = "error".into();
                inner.view.message = error;
                available = false;
            }
        }
    }
    if available {
        show(app).await?;
    }
    Ok(())
}

async fn prepare(app: &tauri::AppHandle) -> Result<Option<Staged>, String> {
    let client = client()?;
    let release: Release = serde_json::from_slice(&bytes(&client, API, 1024 * 1024).await?)
        .map_err(|_| "Invalid GitHub release response.")?;
    let latest = version(&release.tag_name)?;
    if release.draft || release.prerelease {
        return Err("Only published stable releases are supported.".into());
    }
    if latest <= version(env!("CARGO_PKG_VERSION"))? {
        return Ok(None);
    }
    let exe = asset(&release, "ParsecWebTurn.exe")?;
    let sum = asset(&release, "SHA256SUMS.txt")?;
    let expected = digest(
        exe.digest
            .as_deref()
            .ok_or("GitHub asset digest is unavailable.")?,
    )?;
    if exe.size == 0 || exe.size > MAX_EXE as u64 {
        return Err("Invalid release executable size.".into());
    }
    {
        let state = app.state::<State>();
        let mut inner = state.inner.lock().map_err(|_| "Update lock failed")?;
        inner.view.version = Some(release.tag_name.clone());
        inner.view.notes = release
            .body
            .as_deref()
            .unwrap_or_default()
            .chars()
            .take(12000)
            .collect();
        inner.view.status = "downloading".into();
        inner.view.message = "Downloading the new executable from GitHub…".into();
    }
    let checksums = bytes(&client, &sum.browser_download_url, 64 * 1024).await?;
    if checksum(std::str::from_utf8(&checksums).map_err(|_| "Invalid checksum file.")?)? != expected
    {
        return Err("GitHub asset digest and published checksum disagree.".into());
    }
    let data = bytes(&client, &exe.browser_download_url, MAX_EXE).await?;
    verify(&data, exe.size, &expected)?;
    let target = std::env::current_exe().map_err(|e| e.to_string())?;
    let directory = tempfile::Builder::new()
        .prefix(".parsec-update-")
        .tempdir_in(target.parent().ok_or("Cannot locate app directory")?)
        .map_err(|_| "The app directory is not writable. Download the update manually.")?;
    let mut file =
        fs::File::create(directory.path().join("update.exe")).map_err(|e| e.to_string())?;
    file.write_all(&data)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(Some(Staged {
        directory,
        digest: expected,
    }))
}

#[derive(Serialize, Deserialize)]
struct Plan {
    target_name: String,
    root: PathBuf,
    settings: bool,
    parent_pid: u32,
    digest: String,
}

pub fn install(app: &tauri::AppHandle) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let state = app.state::<State>();
    let mut inner = state.inner.lock().map_err(|_| "Update lock failed")?;
    let staged = inner
        .staged
        .as_ref()
        .ok_or("No verified update is ready.")?;
    let target = std::env::current_exe().map_err(|e| e.to_string())?;
    let plan = Plan {
        target_name: target
            .file_name()
            .ok_or("Invalid executable name")?
            .to_str()
            .ok_or("Invalid executable name")?
            .into(),
        root: fs::canonicalize(&app.state::<crate::AppState>().root).map_err(|e| e.to_string())?,
        settings: std::env::args()
            .any(|a| a.eq_ignore_ascii_case("--settings") || a.eq_ignore_ascii_case("/settings")),
        parent_pid: std::process::id(),
        digest: staged.digest.clone(),
    };
    crate::storage::write_json(&staged.directory.path().join("plan.json"), &plan)?;
    let helper = staged.directory.path().join("helper.exe");
    fs::copy(&target, &helper).map_err(|e| e.to_string())?;
    Command::new(helper)
        .arg("--apply-update")
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|e| format!("Cannot start update helper: {e}"))?;
    // The helper owns these files after handoff. Do not delete them on app exit.
    let _ = inner.staged.take().unwrap().directory.keep();
    app.exit(0);
    Ok(())
}

pub fn apply() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let directory = executable.parent().ok_or("Cannot locate update stage")?;
    if executable.file_name().and_then(|n| n.to_str()) != Some("helper.exe")
        || !directory
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(".parsec-update-"))
    {
        return Err("Invalid update helper location.".into());
    }
    let plan: Plan = crate::storage::read_json(&directory.join("plan.json"))?;
    let name = std::path::Path::new(&plan.target_name);
    if name.components().count() != 1
        || name.file_name() != Some(name.as_os_str())
        || !plan.target_name.to_ascii_lowercase().ends_with(".exe")
    {
        return Err("Invalid update target name.".into());
    }
    let candidate = directory.join("update.exe");
    if fs::metadata(&candidate).map_err(|e| e.to_string())?.len() > MAX_EXE as u64 {
        return Err("Staged executable is too large.".into());
    }
    let data = fs::read(&candidate).map_err(|e| e.to_string())?;
    verify(&data, data.len() as u64, &plan.digest)?;
    wait_for_exit(plan.parent_pid)?;
    let target = directory
        .parent()
        .ok_or("Cannot locate app directory")?
        .join(name);
    let backup = directory.join("previous.exe");
    swap(&candidate, &target, &backup)?;
    let mut command = Command::new(&target);
    command
        .arg("--data-dir")
        .arg(&plan.root)
        .creation_flags(0x08000000);
    if plan.settings {
        command.arg("--settings");
    }
    if let Err(error) = command.spawn() {
        // Roll back the replacement if Windows cannot start the new executable.
        fs::rename(&target, &candidate)
            .map_err(|e| format!("Update launch failed and rollback failed: {e}"))?;
        fs::rename(&backup, &target).map_err(|e| format!("Update rollback failed: {e}"))?;
        let _ = command.spawn();
        return Err(format!(
            "New executable could not start; restored the previous version: {error}"
        ));
    }
    // Keep previous.exe for recovery. helper.exe can be removed after it exits.
    Ok(())
}

fn swap(
    candidate: &std::path::Path,
    target: &std::path::Path,
    backup: &std::path::Path,
) -> Result<(), String> {
    fs::rename(target, backup)
        .map_err(|e| format!("Cannot replace the running executable: {e}"))?;
    if let Err(error) = fs::rename(candidate, target) {
        fs::rename(backup, target).map_err(|e| {
            format!(
                "Installation and restoration failed. Previous executable is at {}: {e}",
                backup.display()
            )
        })?;
        return Err(format!(
            "Cannot install update; restored previous executable: {error}"
        ));
    }
    Ok(())
}

fn wait_for_exit(pid: u32) -> Result<(), String> {
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn WaitForSingleObject(handle: isize, milliseconds: u32) -> u32;
        fn CloseHandle(handle: isize) -> i32;
    }
    // Only wait; never terminate the application or request elevated rights.
    unsafe {
        let handle = OpenProcess(0x00100000, 0, pid);
        if handle == 0 {
            return Ok(());
        }
        let result = WaitForSingleObject(handle, 60000);
        CloseHandle(handle);
        if result != 0 {
            return Err("The running application did not exit; update was not installed.".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_modified_incomplete_and_non_executable_downloads() {
        let data = b"MZsynthetic executable";
        let hash = format!("{:x}", Sha256::digest(data));
        assert!(verify(data, data.len() as u64, &hash).is_ok());
        assert!(verify(b"MZmodified executable", data.len() as u64, &hash).is_err());
        assert!(verify(data, data.len() as u64 + 1, &hash).is_err());
        let other = b"not an executable";
        assert!(verify(
            other,
            other.len() as u64,
            &format!("{:x}", Sha256::digest(other))
        )
        .is_err());
    }
    #[tokio::test]
    async fn bounds_announced_and_chunked_http_responses() {
        use std::io::Read;
        for response in [
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n12345",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\n123\r\n3\r\n456\r\n0\r\n\r\n",
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut request = [0u8; 2048];
                stream.read(&mut request).unwrap();
                stream.write_all(response.as_bytes()).unwrap();
            });
            let client = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(5)).build().unwrap();
            assert!(bytes(&client, &url, 4).await.is_err());
            server.join().unwrap();
        }
    }
    #[test]
    fn compares_stable_versions_numerically() {
        assert!(version("v0.10.0").unwrap() > version("0.9.9").unwrap());
        assert!(version("1.0.0").unwrap() > version("0.99.0").unwrap());
        for invalid in ["v1.0.0-beta", "01.0.0", "1.2", "1.2.3.4", "../1.2.3"] {
            assert!(version(invalid).is_err());
        }
    }
    #[test]
    fn requires_unique_exe_checksum() {
        let hash = "a".repeat(64);
        assert_eq!(
            checksum(&format!("{hash}  ParsecWebTurn.exe\n")).unwrap(),
            hash
        );
        assert!(checksum(&format!("{hash}  Other.exe")).is_err());
        assert!(checksum(&format!(
            "{hash} ParsecWebTurn.exe\n{hash} ParsecWebTurn.exe"
        ))
        .is_err());
        assert!(digest("sha256:abc").is_err());
    }
    #[test]
    fn rejects_assets_outside_repository() {
        let release: Release = serde_json::from_value(serde_json::json!({
            "tag_name":"v0.6.0", "draft":false, "prerelease":false,
            "assets":[{"name":"ParsecWebTurn.exe", "size":123,"digest":null,
            "browser_download_url":"https://other.example/ParsecWebTurn.exe"}]
        }))
        .unwrap();
        assert!(asset(&release, "ParsecWebTurn.exe").is_err());
    }
    #[test]
    fn failed_swap_restores_original_executable() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app.exe");
        let backup = dir.path().join("previous.exe");
        fs::write(&target, b"original").unwrap();
        assert!(swap(&dir.path().join("missing.exe"), &target, &backup).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert!(!backup.exists());
    }
    #[test]
    fn successful_swap_keeps_recovery_copy() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app.exe");
        let candidate = dir.path().join("new.exe");
        let backup = dir.path().join("previous.exe");
        fs::write(&target, b"original").unwrap();
        fs::write(&candidate, b"replacement").unwrap();
        swap(&candidate, &target, &backup).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"replacement");
        assert_eq!(fs::read(&backup).unwrap(), b"original");
    }
}
