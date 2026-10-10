//! Tauri-compatible settings; native RTCIceServer configuration, never guest JS.
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Write,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};
#[cfg(any(test, not(feature = "diagnostics")))]
use std::{io::Read, sync::Arc};
use webrtc::{
    ice::url::{ProtoType, SchemeType, Url},
    ice_transport::ice_server::RTCIceServer,
};

pub const PARSEC_STUN: &str = "stun:stun.parsec.gg:3478";
const MAX_FILE: usize = 65536;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuPreference {
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub subsystem_id: u32,
    pub revision: u32,
    pub luid: u64,
}
impl GpuPreference {
    pub fn same_hardware(&self, other: &Self) -> bool {
        (
            self.vendor_id,
            self.device_id,
            self.subsystem_id,
            self.revision,
        ) == (
            other.vendor_id,
            other.device_id,
            other.subsystem_id,
            other.revision,
        )
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_gpu: Option<GpuPreference>,
    pub provider: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub custom_urls: Vec<String>,
    pub stun_urls: Vec<String>,
    pub turn_urls: Vec<String>,
    pub stun_only: bool,
    pub custom_username: String,
    pub encrypted_custom_password: String,
    pub turn_key_id: String,
    pub encrypted_api_token: String,
    pub cache_credentials: bool,
    pub media_diagnostics: bool,
    pub ttl: u32,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}
impl Default for Settings {
    // Legacy defaults apply to existing JSON; a new native installation uses fresh().
    fn default() -> Self {
        Self {
            video_gpu: None,
            provider: "cloudflare".into(),
            custom_urls: vec![],
            stun_urls: vec![],
            turn_urls: vec![],
            stun_only: false,
            custom_username: String::new(),
            encrypted_custom_password: String::new(),
            turn_key_id: String::new(),
            encrypted_api_token: String::new(),
            cache_credentials: false,
            media_diagnostics: false,
            ttl: 86400,
            extra: BTreeMap::new(),
        }
    }
}
impl Settings {
    #[cfg(any(test, not(feature = "diagnostics")))]
    fn fresh() -> Self {
        Self {
            provider: "custom".into(),
            ..Self::default()
        }
    }
    fn normalize(&mut self) {
        if self.provider.is_empty() {
            self.provider = "cloudflare".into();
        }
        if self.ttl == 0 {
            self.ttl = 86400;
        }
        if self.stun_urls.is_empty() && self.turn_urls.is_empty() {
            for url in &self.custom_urls {
                if url.trim().to_ascii_lowercase().starts_with("stun") {
                    self.stun_urls.push(url.trim().into());
                } else {
                    self.turn_urls.push(url.trim().into());
                }
            }
        }
        self.custom_urls.clear();
    }
    pub fn secret(&self) -> Result<String> {
        if self.stun_only || (self.provider == "custom" && self.turn_urls.is_empty()) {
            return Ok(String::new());
        }
        unprotect(if self.provider == "custom" {
            &self.encrypted_custom_password
        } else {
            &self.encrypted_api_token
        })
    }
    pub fn validate(&self) -> Result<()> {
        if !matches!(self.provider.as_str(), "custom" | "cloudflare") {
            bail!("Choose Custom servers or Cloudflare TURN");
        }
        validate_urls(&self.stun_urls, false)?;
        // Dormant TURN settings survive STUN-only mode unchanged, including secrets.
        if self.stun_only {
            return Ok(());
        }
        let secret = self.secret()?;
        if self.provider == "custom" {
            validate_urls(&self.turn_urls, true)?;
            if !self.turn_urls.is_empty()
                && (self.custom_username.trim().is_empty() || secret.is_empty())
            {
                bail!("TURN requires a username and password");
            }
        } else {
            if self.turn_key_id.is_empty()
                || self.turn_key_id.len() > 256
                || !self
                    .turn_key_id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            {
                bail!("Enter a valid Cloudflare TURN Key ID");
            }
            if secret.trim().is_empty() || secret.contains(['\r', '\n']) {
                bail!("Enter a valid Cloudflare API token");
            }
            if !(60..=172800).contains(&self.ttl) {
                bail!("Credential lifetime must be 60–172800 seconds");
            }
        }
        Ok(())
    }
    pub fn stun_servers(&self) -> Vec<RTCIceServer> {
        let urls = if self.stun_urls.is_empty() {
            // Preserve the old explicit Cloudflare STUN-only configuration.
            vec![if self.stun_only && self.provider == "cloudflare" {
                "stun:stun.cloudflare.com:3478"
            } else {
                PARSEC_STUN
            }
            .into()]
        } else {
            self.stun_urls.clone()
        };
        vec![RTCIceServer {
            urls,
            ..Default::default()
        }]
    }
    fn custom_servers(&self) -> Result<Vec<RTCIceServer>> {
        let mut servers = self.stun_servers();
        if !self.stun_only && !self.turn_urls.is_empty() {
            servers.push(RTCIceServer {
                urls: self.turn_urls.clone(),
                username: self.custom_username.clone(),
                credential: self.secret()?,
            });
        }
        Ok(servers)
    }
}

fn validate_urls(urls: &[String], turn: bool) -> Result<()> {
    if urls.len() > 16 {
        bail!("Use at most 16 URLs per field");
    }
    for raw in urls {
        if raw.len() > 2048 || raw.chars().any(char::is_control) || raw.contains(['/', '#', '@']) {
            bail!("Invalid STUN/TURN URL");
        }
        let url = Url::parse_url(raw).map_err(|_| anyhow::anyhow!("Invalid STUN/TURN URL"))?;
        if url.port == 0
            || if turn {
                !matches!(url.scheme, SchemeType::Turn | SchemeType::Turns)
            } else {
                !matches!(url.scheme, SchemeType::Stun | SchemeType::Stuns)
            }
        {
            bail!("Use stun: in the STUN field and turn: or turns: in the TURN field");
        }
        if url.scheme == SchemeType::Stuns {
            bail!("Secure STUN (stuns:) is not supported; use stun: for discovery or turns: for TLS relay");
        }
        if url.scheme == SchemeType::Turns && url.proto != ProtoType::Tcp {
            bail!("TURN over TLS requires transport=tcp (or no transport parameter)");
        }
    }
    Ok(())
}

fn unprotect(value: &str) -> Result<String> {
    if value.is_empty() {
        return Ok(String::new());
    }
    let bytes = STANDARD
        .decode(value)
        .context("Invalid saved credential encoding")?;
    String::from_utf8(crate::secure_storage::protect(&bytes, false)?)
        .context("Cannot read saved credential for this Windows user")
}
fn protect(value: &str) -> Result<String> {
    Ok(STANDARD.encode(crate::secure_storage::protect(value.as_bytes(), true)?))
}

pub struct Manager {
    path: PathBuf,
    state: Mutex<(Settings, Option<String>)>,
    cache: Mutex<Option<Cached>>,
}
struct Cached {
    key: [u8; 32],
    issued: Instant,
    servers: Vec<RTCIceServer>,
}

impl Manager {
    #[cfg(any(test, not(feature = "diagnostics")))]
    pub fn open(path: PathBuf) -> Arc<Self> {
        let loaded = (|| -> Result<Settings> {
            let file = match std::fs::File::open(&path) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::fresh()),
                Err(_) => bail!("Cannot read settings.json"),
            };
            let mut raw = Vec::new();
            file.take(MAX_FILE as u64 + 1).read_to_end(&mut raw)?;
            if raw.len() > MAX_FILE {
                bail!("settings.json exceeds 64 KiB");
            }
            let mut settings: Settings = serde_json::from_slice(&raw)
                .context("Invalid settings.json; the existing file has been preserved")?;
            settings.normalize();
            Ok(settings)
        })();
        let state = match loaded {
            Ok(s) => (s, None),
            Err(_) => (
                Settings::fresh(),
                Some(
                    "Cannot read settings.json. Fix the file or save valid settings to replace it."
                        .into(),
                ),
            ),
        };
        Arc::new(Self {
            path,
            state: Mutex::new(state),
            cache: Mutex::new(None),
        })
    }
    pub fn view(&self) -> (Settings, Option<String>) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn save(
        &self,
        mut settings: Settings,
        password: &str,
        token: &str,
        forget_password: bool,
        forget_token: bool,
    ) -> Result<()> {
        settings.normalize();
        if forget_password {
            settings.encrypted_custom_password.clear();
        }
        if forget_token {
            settings.encrypted_api_token.clear();
        }
        if !password.is_empty() {
            settings.encrypted_custom_password = protect(password)?;
        }
        if !token.is_empty() {
            settings.encrypted_api_token = protect(token)?;
        }
        settings.validate()?;
        let raw = serde_json::to_vec_pretty(&settings)?;
        if raw.len() > MAX_FILE {
            bail!("Settings exceed 64 KiB");
        }
        atomic_write(&self.path, &raw)?;
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = (settings, None);
        *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(())
    }
    pub async fn resolve(&self) -> Result<Vec<RTCIceServer>> {
        use sha2::{Digest, Sha256};
        let (settings, error) = self.view();
        if error.is_some() {
            bail!("Fix connection settings before connecting");
        }
        settings.validate()?;
        if settings.stun_only || settings.provider == "custom" {
            return settings.custom_servers();
        }
        let secret = settings.secret()?;
        let key: [u8; 32] = Sha256::digest(serde_json::to_vec(&(
            &settings.turn_key_id,
            &secret,
            settings.ttl,
        ))?)
        .into();
        if settings.cache_credentials {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(cache) = cache.as_ref().filter(|c| {
                c.key == key
                    && c.issued.elapsed().as_secs()
                        < u64::from(settings.ttl.saturating_sub((settings.ttl / 4).max(30)))
            }) {
                let mut servers = settings.stun_servers();
                servers.extend(cache.servers.clone());
                return Ok(servers);
            }
        }
        let issued = Instant::now();
        let endpoint = format!(
            "https://rtc.live.cloudflare.com/v1/turn/keys/{}/credentials/generate-ice-servers",
            settings.turn_key_id
        );
        let servers = cloudflare_request(&endpoint, &secret, settings.ttl).await?;
        if settings.cache_credentials {
            *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(Cached {
                key,
                issued,
                servers: servers.clone(),
            });
        }
        let mut result = settings.stun_servers();
        result.extend(servers);
        Ok(result)
    }
}

async fn cloudflare_request(endpoint: &str, token: &str, ttl: u32) -> Result<Vec<RTCIceServer>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("ParsecWebTurn/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut response = client
        .post(endpoint)
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .body(serde_json::to_vec(&serde_json::json!({"ttl":ttl}))?)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Cannot reach Cloudflare TURN credential service"))?;
    if !response.status().is_success() {
        bail!(
            "Cloudflare TURN credentials could not be generated (HTTP {})",
            response.status().as_u16()
        );
    }
    let mut raw = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("Cannot read Cloudflare response"))?
    {
        if raw.len() + chunk.len() > 1048576 {
            bail!("Cloudflare response exceeds 1 MiB");
        }
        raw.extend_from_slice(&chunk);
    }
    parse_cloudflare(&raw)
}

fn parse_cloudflare(raw: &[u8]) -> Result<Vec<RTCIceServer>> {
    let value: serde_json::Value =
        serde_json::from_slice(raw).context("Invalid Cloudflare response")?;
    let entries = value["iceServers"]
        .as_array()
        .filter(|a| a.len() <= 32)
        .context("Invalid Cloudflare ICE servers")?;
    let mut result = Vec::new();
    for entry in entries {
        let urls: Vec<String> = if let Some(url) = entry["urls"].as_str() {
            vec![url.into()]
        } else {
            entry["urls"]
                .as_array()
                .context("Invalid Cloudflare ICE URLs")?
                .iter()
                .map(|u| {
                    u.as_str()
                        .map(str::to_owned)
                        .context("Invalid Cloudflare ICE URL")
                })
                .collect::<Result<_>>()?
        };
        let turn: Vec<_> = urls
            .into_iter()
            .filter(|u| u.starts_with("turn:") || u.starts_with("turns:"))
            .collect();
        if turn.is_empty() {
            continue;
        }
        validate_urls(&turn, true)?;
        let username = entry["username"]
            .as_str()
            .context("Missing TURN username")?;
        let credential = entry["credential"]
            .as_str()
            .context("Missing TURN password")?;
        if username.is_empty() || credential.is_empty() {
            bail!("Empty TURN credentials");
        }
        result.push(RTCIceServer {
            urls: turn,
            username: username.into(),
            credential: credential.into(),
        });
    }
    if result.is_empty() {
        bail!("Cloudflare returned no TURN servers");
    }
    Ok(result)
}

fn atomic_write(path: &std::path::Path, raw: &[u8]) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let mut random = [0; 16];
    getrandom::fill(&mut random)
        .map_err(|_| anyhow::anyhow!("Cannot allocate settings transaction"))?;
    let name: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let temporary = path.with_file_name(format!("settings-{name}.tmp"));
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary).context("Cannot write settings beside the executable; use --data-dir with a writable folder")?;
        file.write_all(raw)?;
        file.sync_all()?;
        drop(file);
        let from: Vec<_> = temporary.as_os_str().encode_wide().chain([0]).collect();
        let to: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            bail!("Cannot replace settings.json; existing settings were preserved");
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let mut nonce = [0; 16];
            getrandom::fill(&mut nonce).unwrap();
            let name: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
            let path = std::env::temp_dir().join(format!("parsec-settings-{name}"));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn file(&self) -> PathBuf {
            self.0.join("settings.json")
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn fresh_install_uses_parsec_without_turn_or_file_write() {
        let dir = Directory::new();
        let manager = Manager::open(dir.file());
        let settings = manager.view().0;
        assert_eq!(settings.provider, "custom");
        assert_eq!(settings.stun_servers()[0].urls, [PARSEC_STUN]);
        assert_eq!(settings.custom_servers().unwrap().len(), 1);
        assert!(!dir.file().exists());
    }

    #[test]
    fn legacy_split_migration_and_unknown_fields_round_trip() {
        let dir = Directory::new();
        std::fs::write(dir.file(), br#"{"provider":"custom","stunOnly":true,"customUrls":[" STUN:example.org:3478 ","turn:example.org:3478"],"futureOption":{"keep":true}}"#).unwrap();
        let manager = Manager::open(dir.file());
        let s = manager.view().0;
        assert_eq!(s.stun_urls, ["STUN:example.org:3478"]);
        assert_eq!(s.turn_urls, ["turn:example.org:3478"]);
        manager.save(s, "", "", false, false).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.file()).unwrap()).unwrap();
        assert_eq!(value["futureOption"]["keep"], true);
        assert!(value.get("customUrls").is_none());
        let mut s: Settings = serde_json::from_str(
            r#"{"customUrls":["stun:old.example"],"stunUrls":["stun:new.example"]}"#,
        )
        .unwrap();
        s.normalize();
        assert_eq!(s.stun_urls, ["stun:new.example"]);
        assert!(s.custom_urls.is_empty());
    }

    #[tokio::test]
    async fn stun_only_never_requests_cloudflare_or_decrypts_dormant_secret() {
        let dir = Directory::new();
        let manager = Manager::open(dir.file());
        let s = Settings {
            stun_only: true,
            encrypted_api_token: "not-base64".into(),
            ..Settings::default()
        };
        manager.save(s, "", "", false, false).unwrap();
        let servers = manager.resolve().await.unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].urls, ["stun:stun.cloudflare.com:3478"]);
        assert!(servers[0].credential.is_empty());
    }

    #[test]
    fn saved_secrets_use_legacy_current_user_dpapi_and_survive_blank_edits() {
        let dir = Directory::new();
        let manager = Manager::open(dir.file());
        let s = Settings {
            turn_urls: vec!["turns:localhost:5349?transport=tcp".into()],
            custom_username: "fixture".into(),
            ..Settings::fresh()
        };
        manager
            .save(s, "synthetic-password", "", false, false)
            .unwrap();
        let s = manager.view().0;
        let blob = s.encrypted_custom_password.clone();
        // The old client's reader is Base64 -> DPAPI CurrentUser, no entropy.
        let decrypted =
            crate::secure_storage::protect(&STANDARD.decode(&blob).unwrap(), false).unwrap();
        assert_eq!(decrypted, b"synthetic-password");
        manager.save(s, "", "", false, false).unwrap();
        assert_eq!(
            Manager::open(dir.file()).view().0.encrypted_custom_password,
            blob
        );
        assert!(!String::from_utf8(std::fs::read(dir.file()).unwrap())
            .unwrap()
            .contains("synthetic-password"));
        let mut s = manager.view().0;
        s.stun_only = true;
        manager.save(s, "", "", true, false).unwrap();
        assert!(manager.view().0.encrypted_custom_password.is_empty());
    }

    #[test]
    fn invalid_file_and_invalid_edit_preserve_original_bytes() {
        let dir = Directory::new();
        std::fs::write(dir.file(), b"malformed").unwrap();
        let manager = Manager::open(dir.file());
        assert!(manager.view().1.is_some());
        let mut s = Settings::fresh();
        s.stun_urls = vec!["https://not-stun.example".into()];
        assert!(manager.save(s, "", "", false, false).is_err());
        assert_eq!(std::fs::read(dir.file()).unwrap(), b"malformed");
    }

    #[tokio::test]
    async fn credential_request_uses_post_and_preserves_generated_tls_urls() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0; 1024];
                let n = socket.read(&mut buffer).await.unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buffer[..n]);
                if request.ends_with(b"{\"ttl\":60}") {
                    break;
                }
                assert!(request.len() < 8192);
            }
            let body = br#"{"iceServers":[{"urls":"turns:localhost:5349?transport=tcp","username":"fixture","credential":"fixture-password"}]}"#;
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.write_all(body).await.unwrap();
            String::from_utf8(request).unwrap()
        });
        let servers =
            cloudflare_request(&format!("http://{address}/fixture"), "synthetic-token", 60)
                .await
                .unwrap();
        assert_eq!(servers[0].urls, ["turns:localhost:5349?transport=tcp"]);
        let request = server.await.unwrap();
        assert!(request.starts_with("POST /fixture HTTP/1.1"));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer synthetic-token"));
        assert!(request.ends_with(r#"{"ttl":60}"#));
    }

    #[tokio::test]
    async fn credential_service_redirects_are_not_followed() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = socket.read(&mut [0; 8192]).await.unwrap();
            socket.write_all(format!("HTTP/1.1 302 Found\r\nLocation: http://{address}/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            drop(socket);
            assert!(
                tokio::time::timeout(Duration::from_millis(200), listener.accept())
                    .await
                    .is_err()
            );
        });
        let error = cloudflare_request(&format!("http://{address}/fixture"), "synthetic-token", 60)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("HTTP 302"));
        assert!(!error.contains("synthetic-token"));
        server.await.unwrap();
    }

    #[test]
    fn supported_turn_transports_are_validated_without_forcing_relay() {
        for url in [
            "turn:example.org:3478",
            "turn:example.org:3478?transport=tcp",
            "turns:example.org:443",
            "turns:example.org:5349?transport=tcp",
        ] {
            validate_urls(&[url.into()], true).unwrap();
        }
        for url in [
            "turns:example.org?transport=udp",
            "turn:example.org:0",
            "turn:user:password@example.org",
            "turn://example.org",
        ] {
            assert!(validate_urls(&[url.into()], true).is_err(), "{url}");
        }
        assert!(validate_urls(&["stuns:example.org".into()], false).is_err());
        let servers = parse_cloudflare(br#"{"iceServers":[{"urls":["stun:stun.example.org"]},{"urls":["turn:example.org:3478?transport=udp","turn:example.org:3478?transport=tcp","turns:example.org:443?transport=tcp"],"username":"fixture","credential":"fixture-password"}]}"#).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].urls.len(), 3);
        assert!(parse_cloudflare(
            br#"{"iceServers":[{"urls":"turns:example.org","username":"fixture"}]}"#
        )
        .is_err());
    }
}
