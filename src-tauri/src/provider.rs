use crate::{
    ice::{self, IceServer},
    settings::Settings,
    storage,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration as Timeout};

const MAX_RESPONSE: usize = 1 << 20;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Cache {
    fingerprint: String,
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    ice_servers: Vec<IceServer>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    encrypted_credentials: String,
}

fn fingerprint(settings: &Settings, secret: &str) -> String {
    let raw = serde_json::to_vec(&(settings.turn_key_id.as_str(), secret, settings.ttl)).unwrap();
    format!("{:x}", Sha256::digest(raw))
}

fn usable(cache: &Cache, settings: &Settings, secret: &str, now: DateTime<Utc>) -> bool {
    let margin = Duration::seconds(i64::from((settings.ttl / 4).max(300)));
    cache.fingerprint == fingerprint(settings, secret)
        && now >= cache.issued_at
        && cache.expires_at - now > margin
        && cache.expires_at - cache.issued_at == Duration::seconds(i64::from(settings.ttl))
}

fn cached(root: &Path, settings: &Settings, secret: &str) -> Option<Vec<IceServer>> {
    let envelope: Envelope = storage::read_json(&root.join(".turn-cache.json")).ok()?;
    let cache: Cache =
        serde_json::from_str(&storage::unprotect(&envelope.encrypted_credentials).ok()?).ok()?;
    if !usable(&cache, settings, secret, Utc::now()) {
        return None;
    }
    ice::parse(&serde_json::to_vec(&serde_json::json!({"iceServers":cache.ice_servers})).ok()?).ok()
}

fn save_cache(
    root: &Path,
    settings: &Settings,
    secret: &str,
    servers: &[IceServer],
    issued_at: DateTime<Utc>,
) -> Result<(), String> {
    let cache = Cache {
        fingerprint: fingerprint(settings, secret),
        issued_at,
        expires_at: issued_at + Duration::seconds(i64::from(settings.ttl)),
        ice_servers: servers.to_vec(),
    };
    let envelope = Envelope {
        encrypted_credentials: storage::protect(
            &serde_json::to_string(&cache).map_err(|e| e.to_string())?,
        )?,
    };
    storage::write_json(&root.join(".turn-cache.json"), &envelope)
}

pub async fn request(endpoint: &str, token: &str, ttl: u32) -> Result<Vec<IceServer>, String> {
    let client = reqwest::Client::builder()
        .timeout(Timeout::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("ParsecWebTurn/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| "Cannot initialize HTTPS client")?;
    let mut response = client
        .post(endpoint)
        .bearer_auth(token)
        .json(&serde_json::json!({"ttl":ttl}))
        .send()
        .await
        .map_err(|_| "Cloudflare request failed; check connectivity")?;
    if !response.status().is_success() {
        return Err(format!(
            "Cloudflare returned HTTP {}; check credentials or service availability",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE as u64)
    {
        return Err("Cloudflare response exceeds 1 MiB".into());
    }
    let mut raw = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Cannot read Cloudflare response")?
    {
        if raw.len() + chunk.len() > MAX_RESPONSE {
            return Err("Cloudflare response exceeds 1 MiB".into());
        }
        raw.extend_from_slice(&chunk);
    }
    ice::parse(&raw).map_err(|e| format!("Invalid Cloudflare response: {e}"))
}

pub async fn resolve(root: &Path, settings: &Settings) -> Result<Vec<IceServer>, String> {
    let secret = settings.secret()?;
    settings.validate(&secret)?;
    if settings.provider == "custom" {
        return ice::custom(&settings.custom_urls, &settings.custom_username, &secret);
    }
    if settings.cache_credentials {
        if let Some(servers) = cached(root, settings, &secret) {
            return Ok(servers);
        }
    }
    let issued_at = Utc::now();
    let endpoint = format!(
        "https://rtc.live.cloudflare.com/v1/turn/keys/{}/credentials/generate-ice-servers",
        settings.turn_key_id
    );
    let servers = request(&endpoint, &secret, settings.ttl).await?;
    if settings.cache_credentials {
        let _ = save_cache(root, settings, &secret, &servers, issued_at);
    }
    Ok(servers)
}

pub fn fallback(root: &Path) -> Result<Vec<IceServer>, String> {
    let path = root.join("ice.json");
    let raw = std::fs::read(path).map_err(|_| "No readable ice.json fallback found")?;
    if raw.len() > MAX_RESPONSE {
        return Err("ice.json exceeds 1 MiB".into());
    }
    ice::parse(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::DEFAULT_TTL;
    use std::io::{Read, Write};

    #[test]
    fn cache_expiry_clock_rollback_and_token_changes() {
        let s = Settings {
            turn_key_id: "key".into(),
            ..Default::default()
        };
        let issued = Utc::now();
        let cache = Cache {
            fingerprint: fingerprint(&s, "token"),
            issued_at: issued,
            expires_at: issued + Duration::seconds(i64::from(s.ttl)),
            ice_servers: vec![],
        };
        assert!(usable(&cache, &s, "token", issued));
        assert!(!usable(&cache, &s, "different", issued));
        assert!(!usable(&cache, &s, "token", issued - Duration::seconds(1)));
        assert!(!usable(
            &cache,
            &s,
            "token",
            cache.expires_at - Duration::minutes(5)
        ));
    }

    #[tokio::test]
    async fn custom_provider_resolves_without_network() {
        let root = tempfile::tempdir().unwrap();
        let s = Settings {
            provider: "custom".into(),
            custom_urls: vec!["turns:example.com:443?transport=tcp".into()],
            custom_username: "user".into(),
            encrypted_custom_password: storage::protect("pass").unwrap(),
            ..Default::default()
        };
        let servers = resolve(root.path(), &s).await.unwrap();
        assert_eq!(servers[0].credential, "pass");
        assert!(!root.path().join(".turn-cache.json").exists());
    }

    // Real HTTP verifies headers, response normalization, bounded reads and
    // redirect refusal without requiring a Cloudflare token.
    #[tokio::test]
    async fn http_request_and_safe_failures() {
        for (status, body, extra, success) in [
            (
                "201 Created",
                r#"{"iceServers":[{"urls":"stun:example.com"}]}"#.to_owned(),
                "",
                true,
            ),
            (
                "401 Unauthorized",
                "reflected-sensitive-token".to_owned(),
                "",
                false,
            ),
            (
                "302 Found",
                String::new(),
                "Location: http://127.0.0.1:1/\r\n",
                false,
            ),
            ("200 OK", " ".repeat(MAX_RESPONSE + 1), "", false),
            ("200 OK", r#"{"iceServers":[null]}"#.to_owned(), "", false),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Timeout::from_secs(5)))
                    .unwrap();
                let mut raw = Vec::new();
                let mut buffer = [0; 4096];
                while !String::from_utf8_lossy(&raw).contains("{\"ttl\":86400}") {
                    let n = stream.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    raw.extend_from_slice(&buffer[..n]);
                }
                let request = String::from_utf8(raw).unwrap().to_lowercase();
                assert!(request.starts_with("post / "));
                assert!(request.contains("authorization: bearer test-token"));
                assert!(request.contains("user-agent: parsecwebturn/"));
                let response = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}",body.len());
                let _ = stream.write_all(response.as_bytes());
            });
            let result = request(&format!("http://{address}/"), "test-token", DEFAULT_TTL).await;
            assert_eq!(result.is_ok(), success);
            if let Err(error) = result {
                assert!(!error.contains("reflected-sensitive-token"));
            }
            server.join().unwrap();
        }
    }
}
