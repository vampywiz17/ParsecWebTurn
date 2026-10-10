//! Optional, version-pinned library diagnostics. Never used to control a peer.
//! Only fixed categories survive; raw messages are neither printed nor retained.
use std::{
    fmt::Write,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

const TARGET: &str = "webrtc::peer_connection::peer_connection_internal";
const CRYPTO_TARGET: &str = "dtls::crypto";
const LIMIT: usize = 16;
static ENABLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, serde::Serialize)]
pub struct Event {
    stage: &'static str,
    reason: &'static str,
}

struct Collector {
    events: Mutex<(Vec<Event>, usize)>,
}
static COLLECTOR: Collector = Collector {
    events: Mutex::new((Vec::new(), 0)),
};

#[cfg(feature = "diagnostics")]
#[derive(serde::Serialize)]
pub struct Snapshot {
    source: &'static str,
    process_scoped: bool,
    logger_installed: bool,
    events: Vec<Event>,
    omitted: usize,
}

#[cfg(feature = "diagnostics")]
pub fn enable() -> bool {
    let installed = log::set_logger(&COLLECTOR).is_ok();
    if installed {
        log::set_max_level(log::LevelFilter::Trace);
        ENABLED.store(true, Ordering::Release);
    }
    installed
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

pub fn contains_reason(reason: &str) -> bool {
    COLLECTOR
        .events
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .0
        .iter()
        .any(|event| event.reason == reason)
}

#[cfg(feature = "diagnostics")]
pub fn snapshot(installed: bool) -> Snapshot {
    let events = COLLECTOR.events.lock().unwrap_or_else(|e| e.into_inner());
    Snapshot {
        source: "webrtc-rs-0.14.0-log-experimental",
        process_scoped: true,
        logger_installed: installed,
        events: events.0.clone(),
        omitted: events.1,
    }
}

// Bounded transient formatting, including for unknown/dynamic library errors.
struct Buffer {
    bytes: [u8; 512],
    len: usize,
}
impl std::fmt::Write for Buffer {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        if text.len() > self.bytes.len() - self.len {
            return Err(std::fmt::Error);
        }
        self.bytes[self.len..self.len + text.len()].copy_from_slice(text.as_bytes());
        self.len += text.len();
        Ok(())
    }
}
impl Drop for Buffer {
    fn drop(&mut self) {
        self.bytes.fill(0);
    }
}

fn classify(target: &str, message: &str) -> Option<Event> {
    if target == CRYPTO_TARGET {
        return crate::transport_diagnostic_errors::signature_reason(message).map(|reason| Event {
            stage: "dtls-signature-verification",
            reason,
        });
    }
    if target != TARGET {
        return None;
    }
    let (stage, error) = [
        ("ice", "Failed to start manager ice: "),
        ("dtls", "Failed to start manager dtls: "),
        ("sctp", "Failed to start SCTP: "),
        ("data-channel", "failed to open data channel: "),
    ]
    .into_iter()
    .find_map(|(stage, prefix)| message.strip_prefix(prefix).map(|e| (stage, e)))?;
    // Compare only exact fixed Display values from the pinned public error enums.
    // Unknown/changed messages remain unknown, never a protocol decision.
    use webrtc::dtls::Error as D;
    use webrtc::Error as W;
    let known = [
        (
            W::ErrNoSRTPProtectionProfile.to_string(),
            "srtp-profile-missing",
        ),
        (
            W::ErrNoRemoteCertificate.to_string(),
            "remote-certificate-missing",
        ),
        (
            W::ErrNoMatchingCertificateFingerprint.to_string(),
            "certificate-fingerprint-mismatch",
        ),
        (
            W::ErrUnsupportedFingerprintAlgorithm.to_string(),
            "fingerprint-algorithm-unsupported",
        ),
        (
            D::ErrCipherSuiteNoIntersection.to_string(),
            "cipher-suite-no-intersection",
        ),
        (
            D::ErrInvalidSignatureAlgorithm.to_string(),
            "signature-algorithm-invalid",
        ),
        (D::ErrKeySignatureMismatch.to_string(), "signature-mismatch"),
        (
            D::ErrClientCertificateRequired.to_string(),
            "client-certificate-required",
        ),
        (
            D::ErrClientCertificateNotVerified.to_string(),
            "client-certificate-not-verified",
        ),
        (
            D::ErrServerNoMatchingSrtpProfile.to_string(),
            "srtp-profile-no-intersection",
        ),
        (
            D::ErrRequestedButNoSrtpExtension.to_string(),
            "srtp-extension-missing",
        ),
        (D::ErrAlertFatalOrClose.to_string(), "peer-alert-or-close"),
        (
            D::ErrUnsupportedProtocolVersion.to_string(),
            "protocol-version-unsupported",
        ),
        (D::ErrDeadlineExceeded.to_string(), "deadline-exceeded"),
        (D::ErrConnClosed.to_string(), "connection-closed"),
        (W::ErrSCTPTransportDTLS.to_string(), "dtls-not-established"),
    ];
    Some(Event {
        stage,
        reason: known
            .into_iter()
            .find(|(text, _)| text == error)
            .map(|(_, reason)| reason)
            .or_else(|| crate::transport_diagnostic_errors::dtls_reason(error))
            .unwrap_or("unknown"),
    })
}

impl log::Log for Collector {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        (metadata.target() == TARGET && metadata.level() <= log::Level::Warn)
            || (metadata.target() == CRYPTO_TARGET && metadata.level() == log::Level::Trace)
    }
    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let mut buffer = Buffer {
            bytes: [0; 512],
            len: 0,
        };
        if write!(&mut buffer, "{}", record.args()).is_err() {
            return;
        }
        let Ok(message) = std::str::from_utf8(&buffer.bytes[..buffer.len]) else {
            return;
        };
        if let Some(event) = classify(record.target(), message) {
            let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
            if events.0.len() < LIMIT {
                events.0.push(event);
            } else {
                events.1 = events.1.saturating_add(1);
            }
        }
    }
    fn flush(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_exact_library_errors_become_fixed_categories() {
        let message = format!(
            "Failed to start manager dtls: {}",
            webrtc::Error::ErrNoSRTPProtectionProfile
        );
        assert_eq!(
            classify(TARGET, &message).unwrap().reason,
            "srtp-profile-missing"
        );
        assert!(classify("other-library", &message).is_none());
        assert!(classify(TARGET, "unrelated secret").is_none());
        let event = classify(TARGET, "Failed to start manager dtls: secret.example/token").unwrap();
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            r#"{"stage":"dtls","reason":"unknown"}"#
        );
    }
    #[test]
    fn log_collection_is_bounded_and_oversized_errors_are_discarded() {
        let collector = Collector {
            events: Mutex::new((Vec::new(), 0)),
        };
        use log::Log;
        for _ in 0..20 {
            collector.log(
                &log::Record::builder()
                    .level(log::Level::Warn)
                    .target(TARGET)
                    .args(format_args!(
                        "Failed to start manager dtls: unrecognized-private-error"
                    ))
                    .build(),
            );
        }
        let oversized = "sensitive".repeat(100);
        collector.log(
            &log::Record::builder()
                .level(log::Level::Warn)
                .target(TARGET)
                .args(format_args!("Failed to start manager dtls: {oversized}"))
                .build(),
        );
        let events = collector.events.lock().unwrap();
        assert_eq!(events.0.len(), LIMIT);
        assert_eq!(events.1, 4);
        assert!(!serde_json::to_string(&events.0)
            .unwrap()
            .contains("private"));
    }
}
