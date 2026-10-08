//! Exact origins, no suffix matching, redirects, credentials or TLS downgrade.
use anyhow::{bail, Result};
use reqwest::Url;

#[derive(Clone, Default)]
pub struct Policy(Vec<(String, String, u16)>);
impl Policy {
    /// Exact origins from the pinned core audit, never a wildcard Parsec domain.
    /// Signaling must still be confirmed against a real account by the user.
    pub fn account() -> Self {
        Self::secure(&[
            "https://kessel-api.parsec.app",
            "https://public.parsec.app",
            "https://parsecusercontent.com",
            "wss://kessel-ws.parsec.app",
        ])
        .expect("fixed secure origins")
    }
    pub fn loopback(port: u16) -> Self {
        Self(vec![
            ("http".into(), "127.0.0.1".into(), port),
            ("ws".into(), "127.0.0.1".into(), port),
        ])
    }
    pub fn secure(origins: &[&str]) -> Result<Self> {
        let mut entries = Vec::new();
        for origin in origins {
            let url = Url::parse(origin)?;
            if !matches!(url.scheme(), "https" | "wss")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
                || url.query().is_some()
                || url.path() != "/"
            {
                bail!("Secure policy requires an exact HTTPS/WSS origin");
            }
            entries.push((
                url.scheme().into(),
                url.host_str()
                    .ok_or_else(|| anyhow::anyhow!("Origin missing host"))?
                    .into(),
                url.port_or_known_default()
                    .ok_or_else(|| anyhow::anyhow!("Origin missing port"))?,
            ));
        }
        if entries.len() > 8 {
            bail!("Origin policy limit");
        }
        Ok(Self(entries))
    }
    pub fn allows(&self, url: &Url) -> bool {
        url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && self.0.iter().any(|(scheme, host, port)| {
                url.scheme() == scheme
                    && url.host_str() == Some(host.as_str())
                    && url.port_or_known_default() == Some(*port)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secure_origins_are_exact_and_never_downgrade() {
        let policy = Policy::secure(&[
            "https://kessel-api.parsec.app",
            "wss://kessel-ws.parsec.app",
        ])
        .unwrap();
        for text in [
            "https://kessel-api.parsec.app/v2/auth",
            "wss://kessel-ws.parsec.app/?fixture=1",
        ] {
            let url = Url::parse(text).unwrap();
            assert!(policy.allows(&url));
            assert!(!Policy::default().allows(&url));
        }
        for text in [
            "http://kessel-api.parsec.app/v2/auth",
            "https://kessel-api.parsec.app.evil.invalid",
            "https://kessel-api.parsec.app:444",
            "https://user@kessel-api.parsec.app",
            "https://kessel-api.parsec.app/#token",
            "wss://another.parsec.app",
        ] {
            assert!(!policy.allows(&Url::parse(text).unwrap()));
        }
        for origin in [
            "http://127.0.0.1",
            "https://example.com/path",
            "https://example.com/?q=1",
        ] {
            assert!(Policy::secure(&[origin]).is_err());
        }
    }
}
