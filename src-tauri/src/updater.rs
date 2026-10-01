use serde::{Deserialize, Serialize};
use std::{sync::Mutex, time::Duration};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const API: &str = "https://api.github.com/repos/vampywiz17/ParsecWebTurn/releases/latest";
const REPOSITORY: &str = "https://github.com/vampywiz17/ParsecWebTurn";
const MAX_METADATA: usize = 1024 * 1024;

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub current_version: String,
    pub version: Option<String>,
    pub status: String,
    pub message: String,
    pub notes: String,
    pub busy: bool,
    pub download_url: Option<String>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
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

pub struct State {
    inner: Mutex<View>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            inner: Mutex::new(View {
                current_version: env!("CARGO_PKG_VERSION").into(),
                status: "idle".into(),
                message: "Ready to check GitHub releases.".into(),
                ..View::default()
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

fn download_url(release: &Release) -> Result<String, String> {
    version(&release.tag_name)?;
    if release.draft || release.prerelease {
        return Err("Only published stable releases are supported.".into());
    }
    let number = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name);
    let name = format!("ParsecWebTurn-v{number}-win64.zip");
    let matches: Vec<_> = release
        .assets
        .iter()
        .filter(|asset| asset.name == name)
        .collect();
    if matches.len() != 1 {
        return Err("Release must contain exactly one Windows release ZIP.".into());
    }
    let expected = format!("{REPOSITORY}/releases/download/{}/{name}", release.tag_name);
    if matches[0].browser_download_url != expected {
        return Err("Download link is outside the expected repository.".into());
    }
    Ok(expected)
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("ParsecWebTurn/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Cannot initialize update check client.".into())
}

async fn metadata(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "Cannot reach GitHub. The network may block update checks.")?;
    if !response.status().is_success() {
        return Err("GitHub update check failed, was blocked, or was rate limited.".into());
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_METADATA as u64)
    {
        return Err("Release metadata is too large.".into());
    }
    let mut result = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Update check was interrupted.")?
    {
        if result.len() + chunk.len() > MAX_METADATA {
            return Err("Release metadata is too large.".into());
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

async fn latest() -> Result<Option<Release>, String> {
    let release: Release = serde_json::from_slice(&metadata(&client()?, API).await?)
        .map_err(|_| "Invalid GitHub release response.")?;
    if release.draft || release.prerelease {
        return Err("Only published stable releases are supported.".into());
    }
    if version(&release.tag_name)? <= version(env!("CARGO_PKG_VERSION"))? {
        return Ok(None);
    }
    download_url(&release)?;
    Ok(Some(release))
}

pub async fn check(app: &tauri::AppHandle, manual: bool) -> Result<(), String> {
    if manual {
        show(app).await?;
    }
    {
        let state = app.state::<State>();
        let mut view = state.inner.lock().map_err(|_| "Update lock failed")?;
        if view.busy {
            return Ok(());
        }
        view.busy = true;
        view.status = "checking".into();
        view.download_url = None;
        view.message = "Checking the latest stable GitHub release…".into();
    }
    let result = latest().await;
    let available;
    {
        let state = app.state::<State>();
        let mut view = state.inner.lock().map_err(|_| "Update lock failed")?;
        view.busy = false;
        view.version = None;
        view.notes.clear();
        match result {
            Ok(Some(release)) => {
                view.download_url = Some(download_url(&release)?);
                view.version = Some(release.tag_name);
                view.notes = release
                    .body
                    .unwrap_or_default()
                    .chars()
                    .take(12000)
                    .collect();
                view.status = "available".into();
                view.message = "A new version is available. Download the ZIP through your browser, then close the app and replace the EXE manually.".into();
                available = true;
            }
            Ok(None) => {
                view.status = "current".into();
                view.message = "You are using the latest stable version.".into();
                available = false;
            }
            Err(error) => {
                view.status = "error".into();
                view.message = error;
                available = false;
            }
        }
    }
    if available {
        show(app).await?;
    }
    Ok(())
}

// Only the trusted local update window can open this fixed, validated HTTPS URL.
// Windows' default browser handles the download and organization policies.
pub fn open_download(app: &tauri::AppHandle) -> Result<(), String> {
    let url = view(app)?
        .download_url
        .ok_or("No newer release download is available.")?;
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: isize,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> isize;
    }
    let operation: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
    let url: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            0,
            operation.as_ptr(),
            url.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        )
    };
    if result <= 32 {
        return Err(
            "Cannot open the download in your browser. Your organization may restrict it.".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn release(tag: &str) -> Release {
        serde_json::from_value(serde_json::json!({
            "tag_name":tag, "draft":false, "prerelease":false,
            "assets":[{"name":format!("ParsecWebTurn-{tag}-win64.zip"),
                "browser_download_url":format!("{REPOSITORY}/releases/download/{tag}/ParsecWebTurn-{tag}-win64.zip")}]
        })).unwrap()
    }
    #[test]
    fn compares_stable_versions_numerically() {
        assert!(version("v0.10.0").unwrap() > version("0.9.9").unwrap());
        for invalid in ["v1.0.0-beta", "01.0.0", "1.2", "1.2.3.4", "../1.2.3"] {
            assert!(version(invalid).is_err());
        }
    }
    #[test]
    fn permits_only_the_unique_stable_repository_zip() {
        let mut data = release("v0.7.0");
        assert_eq!(
            download_url(&data).unwrap(),
            data.assets[0].browser_download_url
        );
        data.assets[0].browser_download_url = "https://other.example/app.zip".into();
        assert!(download_url(&data).is_err());
        let mut data = release("v0.7.0");
        data.assets.push(Asset {
            name: data.assets[0].name.clone(),
            browser_download_url: data.assets[0].browser_download_url.clone(),
        });
        assert!(download_url(&data).is_err());
        for tag in ["v0.7.0-beta", "../v0.7.0"] {
            assert!(download_url(&release(tag)).is_err());
        }
        let mut data = release("v0.7.0");
        data.prerelease = true;
        assert!(download_url(&data).is_err());
        data.prerelease = false;
        data.draft = true;
        assert!(download_url(&data).is_err());
        data.draft = false;
        data.assets.clear();
        assert!(download_url(&data).is_err());
    }
    #[tokio::test]
    async fn rejects_redirects_and_oversized_metadata() {
        use std::io::{Read, Write};
        for response in [
            "HTTP/1.1 302 Found\r\nLocation: https://other.example/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 1048577\r\nConnection: close\r\n\r\n",
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut request = [0u8;2048]; assert!(stream.read(&mut request).unwrap() > 0);
                stream.write_all(response.as_bytes()).unwrap();
            });
            let client = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();
            assert!(metadata(&client, &url).await.is_err());
            server.join().unwrap();
        }
    }
}
