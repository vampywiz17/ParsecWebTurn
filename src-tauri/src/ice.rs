use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{net::Ipv6Addr, sync::OnceLock};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub credential: String,
}

pub fn parse(raw: &[u8]) -> Result<Vec<IceServer>, String> {
    let value: Value = serde_json::from_slice(raw).map_err(|_| "Invalid ICE JSON")?;
    let entries = value["iceServers"]
        .as_array()
        .filter(|entries| !entries.is_empty())
        .ok_or("iceServers must be a non-empty array")?;
    let mut result = Vec::new();
    static PATTERNS: OnceLock<(Regex, Regex)> = OnceLock::new();
    let (stun_pattern, turn_pattern) = PATTERNS.get_or_init(|| {
        // STUN has no query component (RFC 7064). WebRTC supports the UDP/TCP
        // transport subset of TURN URIs (RFC 7065). Scheme names ignore case.
        (
            Regex::new(r"^((?i:stuns?)):(\[[0-9a-fA-F:.]+\]|[a-zA-Z0-9.-]+)(?::([0-9]+))?$").unwrap(),
            Regex::new(r"^((?i:turns?)):(\[[0-9a-fA-F:.]+\]|[a-zA-Z0-9.-]+)(?::([0-9]+))?(?:\?transport=(udp|tcp))?$").unwrap(),
        )
    });
    for entry in entries {
        if !entry.is_object() {
            return Err("Each ICE server must be an object".into());
        }
        let string_field = |key: &str| -> Result<String, String> {
            match entry.get(key) {
                None => Ok(String::new()),
                Some(Value::String(value)) => Ok(value.clone()),
                _ => Err(format!("ICE {key} must be a string")),
            }
        };
        let kind = string_field("credentialType")?;
        if !kind.is_empty() && kind != "password" {
            return Err("Only password TURN credentials are supported".into());
        }
        let urls: Vec<String> = match &entry["urls"] {
            Value::String(value) => vec![value.clone()],
            Value::Array(values) => values
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or("ICE URLs must be strings")
                })
                .collect::<Result<_, _>>()?,
            _ => return Err("ICE urls must be a string or an array".into()),
        };
        if urls.is_empty() {
            return Err("ICE URLs cannot be empty".into());
        }
        let mut server = IceServer {
            urls: Vec::new(),
            username: string_field("username")?,
            credential: string_field("credential")?,
        };
        for url in urls {
            let parts = stun_pattern
                .captures(&url)
                .or_else(|| turn_pattern.captures(&url))
                .ok_or("Invalid STUN/TURN URL")?;
            let scheme = parts[1].to_ascii_lowercase();
            let host = &parts[2];
            if host.starts_with('[') {
                host[1..host.len() - 1]
                    .parse::<Ipv6Addr>()
                    .map_err(|_| "Invalid IPv6 address")?;
            } else if host.starts_with('.') || host.contains("..") || host == "-" {
                return Err("Invalid ICE hostname".into());
            }
            if let Some(port) = parts.get(3) {
                let port = port
                    .as_str()
                    .parse::<u16>()
                    .map_err(|_| "Invalid ICE port")?;
                if port == 0 {
                    return Err("ICE port must be between 1 and 65535".into());
                }
                if port == 53 {
                    // Existing WebView compatibility policy, not an RFC URI restriction.
                    continue;
                }
            }
            if scheme.starts_with("turn")
                && (server.username.trim().is_empty() || server.credential.trim().is_empty())
            {
                return Err("TURN requires a username and password".into());
            }
            server
                .urls
                .push(format!("{scheme}:{}", url.split_once(':').unwrap().1));
        }
        if !server.urls.is_empty() {
            result.push(server);
        }
    }
    if result.is_empty() {
        return Err("No browser-compatible ICE URLs remain".into());
    }
    Ok(result)
}

pub fn custom(urls: &[String], username: &str, password: &str) -> Result<Vec<IceServer>, String> {
    let urls: Vec<_> = urls
        .iter()
        .map(|u| u.trim())
        .filter(|u| !u.is_empty())
        .collect();
    parse(
        &serde_json::to_vec(
            &json!({"iceServers": [{"urls": urls, "username": username, "credential": password}]}),
        )
        .unwrap(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_filters_blocked_port() {
        let servers = parse(br#"{"iceServers":[{"urls":["stun:example.com:53","stun:[::1]:3478"]},{"urls":"turns:example.com:443?transport=tcp","username":"user","credential":"secret"}]}"#).unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].urls, ["stun:[::1]:3478"]);
        assert_eq!(servers[1].credential, "secret");
    }

    #[test]
    fn rejects_bad_configuration() {
        for raw in [
            r#"{"iceServers":[]}"#,
            r#"{"iceServers":[null]}"#,
            r#"{"iceServers":[{}]}"#,
            r#"{"iceServers":[{"urls":null}]}"#,
            r#"{"iceServers":[{"urls":["stun:example.com",3]}]}"#,
            r#"{"iceServers":[{"urls":"turn:example.com"}]}"#,
            r#"{"iceServers":[{"urls":"stun:example.com:70000"}]}"#,
            r#"{"iceServers":[{"urls":"stun:[::::]:3478"}]}"#,
            r#"{"iceServers":[{"urls":"https://example.com"}]}"#,
            r#"{"iceServers":[{"urls":"stun:example.com:53"}]}"#,
            r#"{"iceServers":[{"urls":"stun:example.com?transport=udp"}]}"#,
            r#"{"iceServers":[{"urls":"stuns:example.com?transport=tcp"}]}"#,
            r#"{"iceServers":[{"urls":"TURN:example.com"}]}"#,
            r#"{"iceServers":[{"urls":"turn:example.com?transport=UDP","username":"u","credential":"p"}]}"#,
            r#"{"iceServers":[{"urls":"stun:example.com"}]}{}"#,
        ] {
            assert!(parse(raw.as_bytes()).is_err(), "Accepted {raw}");
        }
    }

    #[test]
    fn accepts_case_insensitive_schemes_and_normalizes_them_for_webview() {
        let servers = parse(br#"{"iceServers":[{"urls":["STUN:example.com:3478","StUnS:[::1]:5349"]},{"urls":["TURN:example.com?transport=udp","TuRnS:example.com:443?transport=tcp"],"username":"u","credential":"p"}]}"#).unwrap();
        assert_eq!(
            servers[0].urls,
            ["stun:example.com:3478", "stuns:[::1]:5349"]
        );
        assert_eq!(
            servers[1].urls,
            [
                "turn:example.com?transport=udp",
                "turns:example.com:443?transport=tcp"
            ]
        );
    }
}
