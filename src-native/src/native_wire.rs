//! Stock-host account envelopes shared by native platform session adapters.
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::sync::Arc;
pub type SignalingWrite = Arc<dyn Fn(String) -> bool + Send + Sync>;

pub fn envelope(action: &str, peer: &str, id: &str, data: Value) -> Value {
    json!({"version":1,"action":action,"payload":{"to":peer,"attempt_id":id,"data":data}})
}
pub fn offer(peer: &str, id: &str, credentials: &crate::signaling::Credentials) -> String {
    let mut value = envelope(
        "offer",
        peer,
        id,
        json!({
            "ver_data":1,"mode":2,"versions":{"p2p":1,"bud":1,"init":1,"video":1,"audio":1,"control":1},
            "creds":{"ice_ufrag":credentials.ufrag,"ice_pwd":credentials.password,"fingerprint":credentials.fingerprint}
        }),
    );
    value["payload"]["secret"] = json!("");
    value.to_string()
}

pub fn incoming(text: &str, id: &str, transport: &crate::attempt::Attempt) -> Result<bool> {
    let value: Value = serde_json::from_str(text)
        .map_err(|_| anyhow::anyhow!("Invalid account signaling message"))?;
    let payload = &value["payload"];
    if payload["attempt_id"].as_str() != Some(id) {
        return Ok(false);
    }
    let data = &payload["data"];
    match value["action"].as_str() {
        Some("answer_relay") => {
            ensure!(
                payload["approved"] == true,
                "The host declined the connection"
            );
            let creds = &data["creds"];
            let field = |key| {
                creds[key]
                    .as_str()
                    .map(str::to_owned)
                    .context("Host credentials are incomplete")
            };
            transport.begin(
                id,
                crate::signaling::Credentials {
                    ufrag: field("ice_ufrag")?,
                    password: field("ice_pwd")?,
                    fingerprint: field("fingerprint")?,
                }
                .normalize_compact(),
            )?;
        }
        Some("candex_relay") => {
            if data["sync"] == true {
                transport.sync(id)?;
            } else {
                let ip = data["ip"]
                    .as_str()
                    .context("Host candidate address missing")?;
                let port = data["port"]
                    .as_u64()
                    .and_then(|n| u16::try_from(n).ok())
                    .context("Host candidate port invalid")?;
                transport.candidate(
                    id,
                    crate::signaling::Candidate::new(
                        ip,
                        port,
                        data["from_stun"].as_bool().unwrap_or(false),
                    )?,
                )?;
            }
        }
        Some("close") => {
            return Ok(true);
        }
        _ => {}
    }
    Ok(false)
}

pub fn candidate(event: &Value, peer: &str, id: &str) -> String {
    let data = json!({"ver_data":1,"versions":{"p2p":1,"bud":1,"init":1,"video":1,"audio":1,"control":1},
        "ip":event["ip"],"port":event["port"],"lan":event["lan"],"from_stun":event["fromStun"],"sync":event["sync"]});
    envelope("candex", peer, id, data).to_string()
}
