use crate::{ice, storage};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const DEFAULT_TTL: u32 = 86400;
pub const MAX_TTL: u32 = 172800;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
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
            ttl: DEFAULT_TTL,
        }
    }
}

impl Settings {
    pub fn load(root: &Path) -> Result<Self, String> {
        let mut settings: Self = storage::read_json(&root.join("settings.json"))?;
        if settings.provider.is_empty() {
            settings.provider = "cloudflare".into();
        }
        if settings.ttl == 0 {
            settings.ttl = DEFAULT_TTL;
        }
        settings.migrate_urls();
        Ok(settings)
    }

    fn migrate_urls(&mut self) {
        if self.stun_urls.is_empty() && self.turn_urls.is_empty() {
            for url in &self.custom_urls {
                if url.trim().to_ascii_lowercase().starts_with("stun") {
                    self.stun_urls.push(url.trim().to_owned());
                } else {
                    self.turn_urls.push(url.trim().to_owned());
                }
            }
        }
        self.custom_urls.clear();
    }

    pub fn custom_servers(&self, secret: &str) -> Result<Vec<ice::IceServer>, String> {
        let mut servers = Vec::new();
        if !self.stun_urls.is_empty() {
            servers.extend(ice::custom(&self.stun_urls, "", "")?);
        } else if self.stun_only && self.provider == "cloudflare" {
            servers.extend(ice::custom(
                &["stun:stun.cloudflare.com:3478".into()],
                "",
                "",
            )?);
        }
        if !self.stun_only && !self.turn_urls.is_empty() {
            servers.extend(ice::custom(&self.turn_urls, &self.custom_username, secret)?);
        }
        if servers.is_empty() {
            return Err("Enter at least one STUN server or enable a TURN server".into());
        }
        Ok(servers)
    }

    pub fn secret(&self) -> Result<String, String> {
        if self.stun_only || (self.provider == "custom" && self.turn_urls.is_empty()) {
            return Ok(String::new());
        }
        let encrypted = if self.provider == "custom" {
            &self.encrypted_custom_password
        } else {
            &self.encrypted_api_token
        };
        if encrypted.is_empty() {
            Ok(String::new())
        } else {
            storage::unprotect(encrypted)
        }
    }

    pub fn validate(&self, secret: &str) -> Result<(), String> {
        if !matches!(self.provider.as_str(), "custom" | "cloudflare") {
            return Err("Choose Cloudflare or Custom TURN".into());
        }
        for url in &self.stun_urls {
            if !matches!(
                url.split_once(':')
                    .map(|(s, _)| s.to_ascii_lowercase())
                    .as_deref(),
                Some("stun" | "stuns")
            ) {
                return Err("The STUN field accepts only stun: or stuns: URLs".into());
            }
        }
        if !self.stun_urls.is_empty() {
            ice::custom(&self.stun_urls, "", "")?;
        }
        if self.stun_only {
            self.custom_servers("")?;
            return Ok(());
        }
        match self.provider.as_str() {
            "custom" => {
                for url in &self.turn_urls {
                    if !matches!(
                        url.split_once(':')
                            .map(|(s, _)| s.to_ascii_lowercase())
                            .as_deref(),
                        Some("turn" | "turns")
                    ) {
                        return Err("The TURN field accepts only turn: or turns: URLs".into());
                    }
                }
                self.custom_servers(secret)?;
            }
            "cloudflare" => {
                if self.turn_key_id.is_empty()
                    || !self
                        .turn_key_id
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
                {
                    return Err(
                        "TURN Key ID may contain only letters, digits, hyphens and underscores"
                            .into(),
                    );
                }
                if secret.trim().is_empty() || secret.contains(['\r', '\n']) {
                    return Err("A valid Cloudflare API token is required".into());
                }
                if !(60..=MAX_TTL).contains(&self.ttl) {
                    return Err(format!(
                        "Credential lifetime must be between 60 and {MAX_TTL} seconds"
                    ));
                }
            }
            _ => return Err("Choose Cloudflare or Custom TURN".into()),
        }
        Ok(())
    }
}

// Never send DPAPI ciphertext or saved plaintext secrets to a webview.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub provider: String,
    pub custom_urls: Vec<String>,
    pub stun_urls: Vec<String>,
    pub turn_urls: Vec<String>,
    pub stun_only: bool,
    pub custom_username: String,
    pub turn_key_id: String,
    pub cache_credentials: bool,
    pub media_diagnostics: bool,
    pub ttl: u32,
    pub has_api_token: bool,
    pub has_custom_password: bool,
    pub error: Option<String>,
    pub auto_connect: bool,
    pub version: &'static str,
}

impl SettingsView {
    pub fn new(settings: Settings, error: Option<String>, auto_connect: bool) -> Self {
        Self {
            provider: settings.provider,
            custom_urls: settings.custom_urls,
            stun_urls: settings.stun_urls,
            turn_urls: settings.turn_urls,
            stun_only: settings.stun_only,
            custom_username: settings.custom_username,
            turn_key_id: settings.turn_key_id,
            cache_credentials: settings.cache_credentials,
            media_diagnostics: settings.media_diagnostics,
            ttl: settings.ttl,
            has_api_token: !settings.encrypted_api_token.is_empty(),
            has_custom_password: !settings.encrypted_custom_password.is_empty(),
            error,
            auto_connect,
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveRequest {
    pub provider: String,
    #[serde(default)]
    pub custom_urls: Vec<String>,
    #[serde(default)]
    pub stun_urls: Vec<String>,
    #[serde(default)]
    pub turn_urls: Vec<String>,
    #[serde(default)]
    pub stun_only: bool,
    pub custom_username: String,
    pub turn_key_id: String,
    pub cache_credentials: bool,
    #[serde(default)]
    pub media_diagnostics: bool,
    pub ttl: u32,
    // Blank means retain the encrypted secret; forgetting is explicit.
    pub api_token: String,
    pub custom_password: String,
    #[serde(default)]
    pub forget_api_token: bool,
    #[serde(default)]
    pub forget_custom_password: bool,
}

pub fn save(root: &Path, input: SaveRequest) -> Result<Settings, String> {
    let mut settings = if root.join("settings.json").exists() {
        Settings::load(root)?
    } else {
        Settings::default()
    };
    settings.provider = input.provider;
    settings.custom_urls = input
        .custom_urls
        .into_iter()
        .map(|u| u.trim().to_owned())
        .filter(|u| !u.is_empty())
        .collect();
    let clean = |urls: Vec<String>| {
        urls.into_iter()
            .map(|u| u.trim().to_owned())
            .filter(|u| !u.is_empty())
            .collect()
    };
    settings.stun_urls = clean(input.stun_urls);
    settings.turn_urls = clean(input.turn_urls);
    settings.stun_only = input.stun_only;
    settings.migrate_urls();
    settings.custom_username = input.custom_username.trim().to_owned();
    settings.turn_key_id = input.turn_key_id.trim().to_owned();
    settings.cache_credentials = input.cache_credentials;
    settings.media_diagnostics = input.media_diagnostics;
    settings.ttl = input.ttl;
    if input.forget_api_token {
        settings.encrypted_api_token.clear();
    }
    if input.forget_custom_password {
        settings.encrypted_custom_password.clear();
    }
    if !input.api_token.trim().is_empty() {
        settings.encrypted_api_token = storage::protect(input.api_token.trim())?;
    }
    if !input.custom_password.is_empty() {
        settings.encrypted_custom_password = storage::protect(&input.custom_password)?;
    }
    let secret = settings.secret()?;
    settings.validate(&secret)?;
    if !settings.stun_urls.is_empty() {
        settings.stun_urls = ice::custom(&settings.stun_urls, "", "")?
            .into_iter()
            .flat_map(|s| s.urls)
            .collect();
    }
    if settings.provider == "custom" && !settings.stun_only && !settings.turn_urls.is_empty() {
        settings.turn_urls = ice::custom(&settings.turn_urls, &settings.custom_username, &secret)?
            .into_iter()
            .flat_map(|s| s.urls)
            .collect();
    }
    storage::write_json(&root.join("settings.json"), &settings)?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_legacy_urls_migrate_without_enabling_stun_only() {
        let root = tempfile::tempdir().unwrap();
        storage::write_json(&root.path().join("settings.json"), &serde_json::json!({
            "provider":"custom", "customUrls":[" STUN:example.com:3478 ","turns:example.com:443"]
        })).unwrap();
        let settings = Settings::load(root.path()).unwrap();
        assert_eq!(settings.stun_urls, ["STUN:example.com:3478"]);
        assert_eq!(settings.turn_urls, ["turns:example.com:443"]);
        assert!(settings.custom_urls.is_empty());
        assert!(!settings.stun_only);
    }

    #[test]
    fn stun_only_ignores_turn_credentials_but_never_accepts_turn_in_stun_field() {
        let mut settings = Settings {
            provider: "custom".into(),
            stun_only: true,
            stun_urls: vec!["stun:example.com:3478".into()],
            turn_urls: vec!["turn:example.com:3478".into()],
            encrypted_custom_password: "unreadable dormant ciphertext".into(),
            ..Default::default()
        };
        assert_eq!(settings.secret().unwrap(), "");
        settings.validate("").unwrap();
        let servers = settings.custom_servers("").unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].urls, ["stun:example.com:3478"]);
        assert!(servers[0].credential.is_empty());
        settings.stun_urls = vec!["turn:example.com:3478".into()];
        assert!(settings.validate("").is_err());
    }

    #[test]
    fn custom_stun_without_turn_needs_no_password() {
        let settings = Settings {
            provider: "custom".into(),
            stun_urls: vec!["stun:example.com".into()],
            ..Default::default()
        };
        settings.validate("").unwrap();
        assert_eq!(
            settings.custom_servers("").unwrap()[0].urls,
            ["stun:example.com"]
        );
    }

    #[test]
    fn cloudflare_stun_only_defaults_to_public_stun_without_token() {
        let settings = Settings {
            stun_only: true,
            encrypted_api_token: "unused ciphertext".into(),
            ..Default::default()
        };
        settings.validate("").unwrap();
        assert_eq!(settings.secret().unwrap(), "");
        assert_eq!(
            settings.custom_servers("").unwrap()[0].urls,
            ["stun:stun.cloudflare.com:3478"]
        );
    }

    #[test]
    fn loads_legacy_dpapi_settings_and_defaults() {
        let root = tempfile::tempdir().unwrap();
        let cipher = storage::protect("legacy-token").unwrap();
        storage::write_json(
            &root.path().join("settings.json"),
            &serde_json::json!({"turnKeyId":"key", "encryptedApiToken":cipher,"ttl":0}),
        )
        .unwrap();
        let settings = Settings::load(root.path()).unwrap();
        assert_eq!(settings.provider, "cloudflare");
        assert_eq!(settings.ttl, DEFAULT_TTL);
        assert!(
            !settings.media_diagnostics,
            "Legacy settings must not opt into vendor diagnostics"
        );
        assert_eq!(settings.secret().unwrap(), "legacy-token");
        settings.validate(&settings.secret().unwrap()).unwrap();
    }

    #[test]
    fn switching_providers_preserves_secrets_and_invalid_save_preserves_file() {
        let root = tempfile::tempdir().unwrap();
        let old = Settings {
            turn_key_id: "key".into(),
            encrypted_api_token: storage::protect("cloud-token").unwrap(),
            ..Default::default()
        };
        storage::write_json(&root.path().join("settings.json"), &old).unwrap();
        let input = |provider: &str, password: &str| SaveRequest {
            provider: provider.into(),
            custom_urls: vec![" turns:example.com:443?transport=tcp ".into()],
            stun_urls: vec![],
            turn_urls: vec![],
            stun_only: false,
            custom_username: "user".into(),
            turn_key_id: "key".into(),
            media_diagnostics: false,
            ttl: DEFAULT_TTL,
            cache_credentials: false,
            api_token: String::new(),
            custom_password: password.into(),
            forget_api_token: false,
            forget_custom_password: false,
        };
        let custom = save(root.path(), input("custom", "custom-pass")).unwrap();
        assert_eq!(custom.secret().unwrap(), "custom-pass");
        let cloud = save(root.path(), input("cloudflare", "")).unwrap();
        assert_eq!(cloud.secret().unwrap(), "cloud-token");
        assert_eq!(
            storage::unprotect(&cloud.encrypted_custom_password).unwrap(),
            "custom-pass"
        );
        let before = std::fs::read(root.path().join("settings.json")).unwrap();
        assert!(save(root.path(), input("unknown", "")).is_err());
        assert_eq!(
            before,
            std::fs::read(root.path().join("settings.json")).unwrap()
        );
        let view = serde_json::to_string(&SettingsView::new(cloud, None, false)).unwrap();
        assert!(!view.contains("cloud-token") && !view.contains("encryptedApiToken"));
    }
}
