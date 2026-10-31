//! Shared STUN/TURN validation and Cloudflare credential exchange; no storage/UI.
use anyhow::{bail, Context, Result};
use std::time::Duration;
use webrtc::{
    ice::url::{ProtoType, SchemeType, Url},
    ice_transport::ice_server::RTCIceServer,
};

pub(crate) fn validate_urls(urls: &[String], turn: bool) -> Result<()> {
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

pub(crate) async fn cloudflare_request(
    endpoint: &str,
    token: &str,
    ttl: u32,
) -> Result<Vec<RTCIceServer>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("EvoParseq/", env!("CARGO_PKG_VERSION")))
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

pub(crate) fn parse_cloudflare(raw: &[u8]) -> Result<Vec<RTCIceServer>> {
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
