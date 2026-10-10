//! Bounded metadata; destination origins require explicit diagnostic opt-in.
//! Never retain URL credentials, paths, queries, headers or bodies.
use reqwest::Url;
#[cfg(any(test, feature = "diagnostics"))]
use {serde::Serialize, std::sync::Mutex};

#[cfg(any(test, feature = "diagnostics"))]
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
#[cfg(any(test, feature = "diagnostics"))]
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
#[cfg(any(test, feature = "diagnostics"))]
#[derive(Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Http,
    Https,
    Ws,
    Wss,
    Other,
}
#[cfg(any(test, feature = "diagnostics"))]
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
#[cfg(any(test, feature = "diagnostics"))]
#[derive(Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Bridge {
    Http,
    WebSocket,
}
#[cfg(any(test, feature = "diagnostics"))]
#[derive(Clone, Serialize)]
pub struct Intent {
    bridge: Bridge,
    #[serde(skip_serializing_if = "Option::is_none")]
    destination_origin: Option<String>,
    protocol: Protocol,
    service: Service,
    route: Route,
    method: Method,
    has_authorization: bool,
    has_query: bool,
    body_bytes: usize,
    policy_allowed: bool,
}
#[cfg(any(test, feature = "diagnostics"))]
#[derive(Clone, Default, Serialize)]
pub struct Snapshot {
    pub intents: Vec<Intent>,
    pub omitted: u64,
}
#[derive(Default)]
pub struct Audit {
    #[cfg(any(test, feature = "diagnostics"))]
    snapshot: Mutex<Snapshot>,
    #[cfg(any(test, feature = "diagnostics"))]
    destination_origins: bool,
}
impl Audit {
    /// Diagnostic mode only: origins exclude userinfo, paths and query tokens.
    /// This changes reporting, never connection authorization.
    #[cfg(any(test, feature = "diagnostics"))]
    pub fn with_destination_origins() -> Self {
        Self {
            destination_origins: true,
            ..Default::default()
        }
    }
    pub fn record(
        &self,
        _url: &Url,
        _method: &str,
        _authorization: bool,
        _body_bytes: usize,
        _allowed: bool,
    ) {
        // Normal builds preserve the shared bridge API without collecting traces.
        #[cfg(any(test, feature = "diagnostics"))]
        self.record_bridge(
            _url,
            _method,
            _authorization,
            _body_bytes,
            _allowed,
            Bridge::Http,
        );
    }
    pub fn record_websocket(&self, _url: &Url, _allowed: bool) {
        #[cfg(any(test, feature = "diagnostics"))]
        self.record_bridge(_url, "GET", false, 0, _allowed, Bridge::WebSocket);
    }
    #[cfg(any(test, feature = "diagnostics"))]
    fn record_bridge(
        &self,
        url: &Url,
        method: &str,
        authorization: bool,
        body_bytes: usize,
        allowed: bool,
        bridge: Bridge,
    ) {
        let service = match url.host_str() {
            Some("kessel-api.parsec.app") => Service::Api,
            Some("kessel-ws.parsec.app" | "kessel-ws-v2.parsec.app") => Service::Signaling,
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
        let mut snapshot = self.snapshot.lock().unwrap_or_else(|e| e.into_inner());
        if snapshot.intents.len() >= 64 {
            snapshot.omitted = snapshot.omitted.saturating_add(1);
            return;
        }
        snapshot.intents.push(Intent {
            bridge,
            destination_origin: self
                .destination_origins
                .then(|| url.origin().ascii_serialization()),
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
    #[cfg(any(test, feature = "diagnostics"))]
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
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
        assert!(!json.contains("destination_origin"));
        assert!(json.contains("\"has_authorization\":true"));
    }
    #[test]
    fn opt_in_reports_exact_origins_and_bridge_without_url_secrets() {
        let audit = Audit::with_destination_origins();
        let url = Url::parse("wss://secret-user:secret-password@signal.example.invalid:8443/secret-path?token=secret-query#secret-fragment").unwrap();
        audit.record_websocket(&url, false);
        // The same URL through the wrong bridge must be distinguishable.
        audit.record(&url, "GET", false, 0, false);
        let json = serde_json::to_value(audit.snapshot()).unwrap();
        assert_eq!(
            json["intents"][0]["destination_origin"],
            "wss://signal.example.invalid:8443"
        );
        assert_eq!(json["intents"][0]["bridge"], "web-socket");
        assert_eq!(json["intents"][1]["bridge"], "http");
        assert_eq!(json["intents"][0]["policy_allowed"], false);
        assert!(!json.to_string().contains("secret-"));
        for _ in 0..70 {
            audit.record_websocket(&url, false);
        }
        assert_eq!(audit.snapshot().intents.len(), 64);
        assert_eq!(audit.snapshot().omitted, 8);
    }
}
