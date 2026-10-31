//! Validated relay requests, shared transport APIs, platform-owned encrypted storage.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use webrtc::{
    ice_transport::ice_server::RTCIceServer,
    peer_connection::{
        configuration::RTCConfiguration, policy::ice_transport_policy::RTCIceTransportPolicy,
    },
};
use zeroize::Zeroize;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_stun")]
    pub stun_urls: Vec<String>,
    #[serde(default)]
    pub turn_urls: Vec<String>,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub key_id: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub force_relay: bool,
}
fn default_provider() -> String {
    "direct".into()
}
fn default_stun() -> Vec<String> {
    vec!["stun:stun.parsec.gg:3478".into()]
}
impl Default for Network {
    fn default() -> Self {
        Self {
            provider: "direct".into(),
            stun_urls: vec!["stun:stun.parsec.gg:3478".into()],
            turn_urls: vec![],
            username: String::new(),
            password: String::new(),
            key_id: String::new(),
            token: String::new(),
            force_relay: false,
        }
    }
}
impl Drop for Network {
    fn drop(&mut self) {
        self.password.zeroize();
        self.token.zeroize();
    }
}
impl Network {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() <= 32768, "Network settings exceed limit");
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|_| anyhow::anyhow!("Invalid network settings"))?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            matches!(self.provider.as_str(), "direct" | "custom" | "cloudflare"),
            "Invalid relay provider"
        );
        crate::turn_configuration::validate_urls(&self.stun_urls, false)?;
        crate::turn_configuration::validate_urls(&self.turn_urls, true)?;
        ensure!(
            self.username.len() <= 512 && self.password.len() <= 4096 && self.token.len() <= 4096,
            "Relay credential exceeds limit"
        );
        ensure!(
            !self.username.chars().any(char::is_control)
                && !self.password.chars().any(char::is_control)
                && !self.token.chars().any(char::is_control),
            "Invalid relay credential"
        );
        if self.provider == "cloudflare" {
            ensure!(
                !self.key_id.is_empty()
                    && self.key_id.len() <= 128
                    && self
                        .key_id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                    && !self.token.is_empty(),
                "Cloudflare key ID and API token are required"
            );
        }
        if self.force_relay {
            ensure!(
                self.provider == "cloudflare"
                    || (self.provider == "custom" && !self.turn_urls.is_empty()),
                "Force relay requires TURN configuration"
            );
        }
        Ok(())
    }
    pub fn summary(&self) -> Value {
        json!({"provider":self.provider,"force_relay":self.force_relay,"stun_count":self.stun_urls.len(),"turn_count":self.turn_urls.len(),"configured":self.provider!="direct"})
    }
    pub async fn servers(&self) -> Result<Vec<RTCIceServer>> {
        self.validate()?;
        let mut servers = vec![RTCIceServer {
            urls: self.stun_urls.clone(),
            ..Default::default()
        }];
        if self.provider == "cloudflare" {
            let endpoint = format!(
                "https://rtc.live.cloudflare.com/v1/turn/keys/{}/credentials/generate-ice-servers",
                self.key_id
            );
            servers.extend(
                crate::turn_configuration::cloudflare_request(&endpoint, &self.token, 86400)
                    .await?,
            );
        } else if self.provider == "custom" && !self.turn_urls.is_empty() {
            servers.push(RTCIceServer {
                urls: self.turn_urls.clone(),
                username: self.username.clone(),
                credential: self.password.clone(),
            });
        }
        servers.retain(|s| !s.urls.is_empty());
        Ok(servers)
    }
}
impl crate::rtc_configuration::NetworkSettings for Network {
    fn resolve(&self) -> futures_util::future::BoxFuture<'_, Result<RTCConfiguration>> {
        Box::pin(async move {
            Ok(RTCConfiguration {
                ice_servers: self.servers().await?,
                ice_transport_policy: if self.force_relay {
                    RTCIceTransportPolicy::Relay
                } else {
                    RTCIceTransportPolicy::All
                },
                ..Default::default()
            })
        })
    }
    fn tcp_qos(&self) -> bool {
        false
    }
}
#[cfg(target_os = "android")]
pub static CURRENT: std::sync::Mutex<Option<std::sync::Arc<Network>>> = std::sync::Mutex::new(None);
#[cfg(target_os = "android")]
pub fn current() -> std::sync::Arc<Network> {
    CURRENT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(|| std::sync::Arc::new(Network::default()))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn forced_custom_turn_keeps_relay_only_policy_and_survives_storage() {
        use crate::rtc_configuration::NetworkSettings;
        let n=Network::parse(br#"{"provider":"custom","turn_urls":["turns:relay.example:443?transport=tcp"],"username":"fixture","password":"secret-fixture","force_relay":true}"#).unwrap();
        let restored = Network::parse(&serde_json::to_vec(&n).unwrap()).unwrap();
        let rtc = restored.resolve().await.unwrap();
        assert_eq!(rtc.ice_transport_policy, RTCIceTransportPolicy::Relay);
        assert!(rtc
            .ice_servers
            .iter()
            .any(|s| s.urls.iter().any(|u| u.starts_with("turns:"))));
        assert!(restored.force_relay);
        assert!(!restored.summary().to_string().contains("secret-fixture"));
        assert_eq!(
            Network::default()
                .resolve()
                .await
                .unwrap()
                .ice_transport_policy,
            RTCIceTransportPolicy::All
        );
    }

    #[test]
    fn reject_unsafe_urls_incomplete_tokens_and_keep_summary_secret_free() {
        let mut n = Network::default();
        assert!(n.validate().is_ok());
        n.force_relay = true;
        assert!(n.validate().is_err());
        n.provider = "cloudflare".into();
        n.key_id = "valid-id".into();
        n.token = "secret-fixture".into();
        assert!(n.validate().is_ok());
        assert!(!n.summary().to_string().contains("secret-fixture"));
        n.key_id = "../unsafe".into();
        assert!(n.validate().is_err());
        assert!(
            Network::parse(br#"{"provider":"custom","turn_urls":["http://invalid"]}"#).is_err()
        );
        let n = Network::default();
        assert_eq!(
            Network::parse(&serde_json::to_vec(&n).unwrap())
                .unwrap()
                .provider,
            "direct"
        );
    }
}
