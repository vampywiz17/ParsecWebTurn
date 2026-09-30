use crate::{ice, storage};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const DEFAULT_TTL: u32 = 86400;
pub const MAX_TTL: u32 = 172800;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub provider: String,
    pub custom_urls: Vec<String>,
    pub custom_username: String,
    pub encrypted_custom_password: String,
    pub turn_key_id: String,
    pub encrypted_api_token: String,
    pub cache_credentials: bool,
    pub ttl: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: "cloudflare".into(),
            custom_urls: vec![],
            custom_username: String::new(),
            encrypted_custom_password: String::new(),
            turn_key_id: String::new(),
            encrypted_api_token: String::new(),
            cache_credentials: false,
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
        Ok(settings)
    }

    pub fn secret(&self) -> Result<String, String> {
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
        match self.provider.as_str() {
            "custom" => {
                ice::custom(&self.custom_urls, &self.custom_username, secret)?;
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
    pub custom_username: String,
    pub turn_key_id: String,
    pub cache_credentials: bool,
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
            custom_username: settings.custom_username,
            turn_key_id: settings.turn_key_id,
            cache_credentials: settings.cache_credentials,
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
    pub custom_urls: Vec<String>,
    pub custom_username: String,
    pub turn_key_id: String,
    pub cache_credentials: bool,
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
    settings.custom_username = input.custom_username.trim().to_owned();
    settings.turn_key_id = input.turn_key_id.trim().to_owned();
    settings.cache_credentials = input.cache_credentials;
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
    if settings.provider == "custom" {
        settings.custom_urls =
            ice::custom(&settings.custom_urls, &settings.custom_username, &secret)?[0]
                .urls
                .clone();
    }
    storage::write_json(&root.join("settings.json"), &settings)?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            custom_username: "user".into(),
            turn_key_id: "key".into(),
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
