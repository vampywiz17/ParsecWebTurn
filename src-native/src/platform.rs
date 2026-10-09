//! Local desktop services, separate from account transport and remote clipboard.
use reqwest::Url;
use serde::Serialize;
use std::sync::Mutex;

pub const MAX_TEXT: usize = 1024 * 1024;

pub trait Desktop: Send + Sync {
    fn read_text(&self) -> Option<String>;
    fn write_text(&self, text: &str) -> bool;
    fn open_url(&self, url: &str) -> bool;
    fn alert(&self, title: &str, message: &str) -> bool;
}

#[derive(Clone, Default, Serialize)]
pub struct Snapshot {
    pub clipboard_reads: u64,
    pub clipboard_writes: u64,
    pub links_opened: u64,
    pub alerts_shown: u64,
    pub unavailable_or_rejected: u64,
}

#[derive(Default)]
pub struct Services {
    desktop: Option<Box<dyn Desktop>>,
    stats: Mutex<Snapshot>,
}

impl Services {
    pub fn new(desktop: Box<dyn Desktop>) -> Self {
        Self {
            desktop: Some(desktop),
            stats: Default::default(),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn outcome(&self, success: bool, update: impl FnOnce(&mut Snapshot)) {
        let mut stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        if success {
            update(&mut stats);
        } else {
            stats.unavailable_or_rejected = stats.unavailable_or_rejected.saturating_add(1);
        }
    }

    pub fn read_text(&self) -> String {
        let text = self
            .desktop
            .as_ref()
            .and_then(|d| d.read_text())
            .filter(|s| s.len() <= MAX_TEXT && !s.contains('\0'));
        self.outcome(text.is_some(), |s| {
            s.clipboard_reads = s.clipboard_reads.saturating_add(1)
        });
        text.unwrap_or_default()
    }

    pub fn write_text(&self, text: &str) {
        let ok = text.len() <= MAX_TEXT
            && !text.contains('\0')
            && self.desktop.as_ref().is_some_and(|d| d.write_text(text));
        self.outcome(ok, |s| {
            s.clipboard_writes = s.clipboard_writes.saturating_add(1)
        });
    }

    pub fn open_url(&self, raw: &str) {
        // Shell execution receives only a parsed absolute HTTPS URL, never a
        // path, executable, command arguments or custom protocol handler.
        let url = if raw.len() <= 16 * 1024 && !raw.chars().any(char::is_control) {
            Url::parse(raw).ok().filter(|u| {
                u.scheme() == "https"
                    && u.host_str().is_some()
                    && u.username().is_empty()
                    && u.password().is_none()
            })
        } else {
            None
        };
        let ok = url.as_ref().is_some_and(|u| {
            self.desktop
                .as_ref()
                .is_some_and(|d| d.open_url(u.as_str()))
        });
        self.outcome(ok, |s| s.links_opened = s.links_opened.saturating_add(1));
    }

    pub fn alert(&self, title: &str, message: &str) {
        let ok = title.len() <= 1024
            && message.len() <= 16 * 1024
            && !title.contains('\0')
            && !message.contains('\0')
            && self
                .desktop
                .as_ref()
                .is_some_and(|d| d.alert(title, message));
        self.outcome(ok, |s| s.alerts_shown = s.alerts_shown.saturating_add(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture;
    impl Desktop for Fixture {
        fn read_text(&self) -> Option<String> {
            Some("Árvíztűrő 🦀 secret-sentinel".into())
        }
        fn write_text(&self, _: &str) -> bool {
            true
        }
        fn open_url(&self, _: &str) -> bool {
            true
        }
        fn alert(&self, _: &str, _: &str) -> bool {
            true
        }
    }
    #[test]
    fn local_services_reject_shell_targets_and_keep_only_counters() {
        let s = Services::new(Box::new(Fixture));
        for url in [
            "file:///C:/Windows/notepad.exe",
            "javascript:alert(1)",
            "parsec://host",
            "https://user:pass@example.invalid/",
            "https://example.invalid/\n",
            "cmd.exe",
        ] {
            s.open_url(url);
        }
        s.open_url("https://example.invalid/?secret-sentinel=1");
        assert!(s.read_text().contains("🦀"));
        s.write_text("secret-sentinel");
        s.write_text("bad\0text");
        s.alert("secret-sentinel", "secret-sentinel");
        let stats = s.snapshot();
        assert_eq!(stats.links_opened, 1);
        assert_eq!(stats.clipboard_reads, 1);
        assert_eq!(stats.clipboard_writes, 1);
        assert_eq!(stats.alerts_shown, 1);
        assert_eq!(stats.unavailable_or_rejected, 7);
        assert!(!serde_json::to_string(&stats)
            .unwrap()
            .contains("secret-sentinel"));
    }
    #[test]
    fn offline_services_have_no_os_side_effects() {
        let s = Services::default();
        assert_eq!(s.read_text(), "");
        s.write_text("fixture");
        s.open_url("https://example.invalid/");
        s.alert("fixture", "fixture");
        assert_eq!(s.snapshot().unavailable_or_rejected, 4);
    }
}
