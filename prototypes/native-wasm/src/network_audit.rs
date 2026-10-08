//! Bounded metadata only. Never retain URLs, paths, queries, headers or bodies.
use reqwest::Url;
use serde::Serialize;
use std::sync::Mutex;

#[derive(Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Service {
    Api,
    Signaling,
    PublicAssets,
    ImageAssets,
    Loopback,
    Other,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Route {
    Authentication,
    Sessions,
    Elevation,
    Saml,
    StaticData,
    Other,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Http,
    Https,
    Ws,
    Wss,
    Other,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
    Connect,
    Other,
}
#[derive(Clone, Serialize)]
pub struct Intent {
    protocol: Protocol,
    service: Service,
    route: Route,
    method: Method,
    has_authorization: bool,
    has_query: bool,
    body_bytes: usize,
    policy_allowed: bool,
}
#[derive(Clone, Default, Serialize)]
pub struct Snapshot {
    pub intents: Vec<Intent>,
    pub omitted: u64,
}
#[derive(Default)]
pub struct Audit(Mutex<Snapshot>);
impl Audit {
    pub fn record(
        &self,
        url: &Url,
        method: &str,
        authorization: bool,
        body_bytes: usize,
        allowed: bool,
    ) {
        let service = match url.host_str() {
            Some("kessel-api.parsec.app") => Service::Api,
            Some("kessel-ws.parsec.app") => Service::Signaling,
            Some("public.parsec.app" | "web.parsec.app") => Service::PublicAssets,
            Some("parsecusercontent.com") => Service::ImageAssets,
            Some("127.0.0.1") => Service::Loopback,
            _ => Service::Other,
        };
        let route = match url.path() {
            "/v2/auth" => Route::Authentication,
            "/auth/sessions" => Route::Sessions,
            "/auth/sessions/elevate" => Route::Elevation,
            path if path.starts_with("/v2/saml/") => Route::Saml,
            path if path.starts_with("/data/") => Route::StaticData,
            _ => Route::Other,
        };
        let protocol = match url.scheme() {
            "http" => Protocol::Http,
            "https" => Protocol::Https,
            "ws" => Protocol::Ws,
            "wss" => Protocol::Wss,
            _ => Protocol::Other,
        };
        let method = match method {
            "GET" => Method::Get,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "PATCH" => Method::Patch,
            "DELETE" => Method::Delete,
            "HEAD" => Method::Head,
            "OPTIONS" => Method::Options,
            "CONNECT" => Method::Connect,
            _ => Method::Other,
        };
        let mut snapshot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if snapshot.intents.len() >= 64 {
            snapshot.omitted = snapshot.omitted.saturating_add(1);
            return;
        }
        snapshot.intents.push(Intent {
            protocol,
            service,
            route,
            method,
            has_authorization: authorization,
            has_query: url.query().is_some(),
            body_bytes,
            policy_allowed: allowed,
        });
    }
    pub fn snapshot(&self) -> Snapshot {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_never_retains_secret_url_or_method_and_is_bounded() {
        let audit = Audit::default();
        let url = Url::parse("https://secret-user:secret-password@secret-host.invalid/secret-path?token=secret-query#secret-fragment").unwrap();
        for _ in 0..70 {
            audit.record(&url, "secret-method", true, 123, false);
        }
        let snapshot = audit.snapshot();
        assert_eq!(snapshot.intents.len(), 64);
        assert_eq!(snapshot.omitted, 6);
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("secret-"));
        assert!(json.contains("\"has_authorization\":true"));
    }
}
