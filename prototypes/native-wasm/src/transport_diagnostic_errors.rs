//! Fixed, version-pinned diagnostic vocabulary. No arbitrary text survives.
use webrtc::dtls::Error as D;

pub fn dtls_reason(error: &str) -> Option<&'static str> {
    let known = [
        (D::ErrConnClosed.to_string(), "dtls-conn-closed"),
        (D::ErrDeadlineExceeded.to_string(), "dtls-deadline-exceeded"),
        (D::ErrBufferTooSmall.to_string(), "dtls-buffer-too-small"),
        (
            D::ErrContextUnsupported.to_string(),
            "dtls-context-unsupported",
        ),
        (
            D::ErrDtlspacketInvalidLength.to_string(),
            "dtls-dtlspacket-invalid-length",
        ),
        (
            D::ErrHandshakeInProgress.to_string(),
            "dtls-handshake-in-progress",
        ),
        (
            D::ErrInvalidContentType.to_string(),
            "dtls-invalid-content-type",
        ),
        (D::ErrInvalidMac.to_string(), "dtls-invalid-mac"),
        (
            D::ErrInvalidPacketLength.to_string(),
            "dtls-invalid-packet-length",
        ),
        (
            D::ErrReservedExportKeyingMaterial.to_string(),
            "dtls-reserved-export-keying-material",
        ),
        (
            D::ErrCertificateVerifyNoCertificate.to_string(),
            "dtls-certificate-verify-no-certificate",
        ),
        (
            D::ErrCipherSuiteNoIntersection.to_string(),
            "dtls-cipher-suite-no-intersection",
        ),
        (
            D::ErrCipherSuiteUnset.to_string(),
            "dtls-cipher-suite-unset",
        ),
        (
            D::ErrClientCertificateNotVerified.to_string(),
            "dtls-client-certificate-not-verified",
        ),
        (
            D::ErrClientCertificateRequired.to_string(),
            "dtls-client-certificate-required",
        ),
        (
            D::ErrClientNoMatchingSrtpProfile.to_string(),
            "dtls-client-no-matching-srtp-profile",
        ),
        (
            D::ErrClientRequiredButNoServerEms.to_string(),
            "dtls-client-required-but-no-server-ems",
        ),
        (
            D::ErrCompressionMethodUnset.to_string(),
            "dtls-compression-method-unset",
        ),
        (D::ErrCookieMismatch.to_string(), "dtls-cookie-mismatch"),
        (D::ErrCookieTooLong.to_string(), "dtls-cookie-too-long"),
        (D::ErrIdentityNoPsk.to_string(), "dtls-identity-no-psk"),
        (
            D::ErrInvalidCertificate.to_string(),
            "dtls-invalid-certificate",
        ),
        (
            D::ErrInvalidCipherSpec.to_string(),
            "dtls-invalid-cipher-spec",
        ),
        (
            D::ErrInvalidCipherSuite.to_string(),
            "dtls-invalid-cipher-suite",
        ),
        (
            D::ErrInvalidClientKeyExchange.to_string(),
            "dtls-invalid-client-key-exchange",
        ),
        (
            D::ErrInvalidCompressionMethod.to_string(),
            "dtls-invalid-compression-method",
        ),
        (
            D::ErrInvalidEcdsasignature.to_string(),
            "dtls-invalid-ecdsasignature",
        ),
        (
            D::ErrInvalidEllipticCurveType.to_string(),
            "dtls-invalid-elliptic-curve-type",
        ),
        (
            D::ErrInvalidExtensionType.to_string(),
            "dtls-invalid-extension-type",
        ),
        (
            D::ErrInvalidHashAlgorithm.to_string(),
            "dtls-invalid-hash-algorithm",
        ),
        (
            D::ErrInvalidNamedCurve.to_string(),
            "dtls-invalid-named-curve",
        ),
        (
            D::ErrInvalidPrivateKey.to_string(),
            "dtls-invalid-private-key",
        ),
        (
            D::ErrNamedCurveAndPrivateKeyMismatch.to_string(),
            "dtls-named-curve-and-private-key-mismatch",
        ),
        (
            D::ErrInvalidSniFormat.to_string(),
            "dtls-invalid-sni-format",
        ),
        (
            D::ErrInvalidSignatureAlgorithm.to_string(),
            "dtls-invalid-signature-algorithm",
        ),
        (
            D::ErrKeySignatureMismatch.to_string(),
            "dtls-key-signature-mismatch",
        ),
        (D::ErrNilNextConn.to_string(), "dtls-nil-next-conn"),
        (
            D::ErrNoAvailableCipherSuites.to_string(),
            "dtls-no-available-cipher-suites",
        ),
        (
            D::ErrNoAvailableSignatureSchemes.to_string(),
            "dtls-no-available-signature-schemes",
        ),
        (D::ErrNoCertificates.to_string(), "dtls-no-certificates"),
        (
            D::ErrNoConfigProvided.to_string(),
            "dtls-no-config-provided",
        ),
        (
            D::ErrNoSupportedEllipticCurves.to_string(),
            "dtls-no-supported-elliptic-curves",
        ),
        (
            D::ErrUnsupportedProtocolVersion.to_string(),
            "dtls-unsupported-protocol-version",
        ),
        (
            D::ErrPskAndCertificate.to_string(),
            "dtls-psk-and-certificate",
        ),
        (
            D::ErrPskAndIdentityMustBeSetForClient.to_string(),
            "dtls-psk-and-identity-must-be-set-for-client",
        ),
        (
            D::ErrRequestedButNoSrtpExtension.to_string(),
            "dtls-requested-but-no-srtp-extension",
        ),
        (
            D::ErrServerMustHaveCertificate.to_string(),
            "dtls-server-must-have-certificate",
        ),
        (
            D::ErrServerNoMatchingSrtpProfile.to_string(),
            "dtls-server-no-matching-srtp-profile",
        ),
        (
            D::ErrServerRequiredButNoClientEms.to_string(),
            "dtls-server-required-but-no-client-ems",
        ),
        (
            D::ErrVerifyDataMismatch.to_string(),
            "dtls-verify-data-mismatch",
        ),
        (
            D::ErrHandshakeMessageUnset.to_string(),
            "dtls-handshake-message-unset",
        ),
        (D::ErrInvalidFlight.to_string(), "dtls-invalid-flight"),
        (
            D::ErrKeySignatureGenerateUnimplemented.to_string(),
            "dtls-key-signature-generate-unimplemented",
        ),
        (
            D::ErrKeySignatureVerifyUnimplemented.to_string(),
            "dtls-key-signature-verify-unimplemented",
        ),
        (D::ErrLengthMismatch.to_string(), "dtls-length-mismatch"),
        (
            D::ErrNotEnoughRoomForNonce.to_string(),
            "dtls-not-enough-room-for-nonce",
        ),
        (D::ErrNotImplemented.to_string(), "dtls-not-implemented"),
        (
            D::ErrSequenceNumberOverflow.to_string(),
            "dtls-sequence-number-overflow",
        ),
        (
            D::ErrUnableToMarshalFragmented.to_string(),
            "dtls-unable-to-marshal-fragmented",
        ),
        (
            D::ErrInvalidFsmTransition.to_string(),
            "dtls-invalid-fsm-transition",
        ),
        (
            D::ErrApplicationDataEpochZero.to_string(),
            "dtls-application-data-epoch-zero",
        ),
        (
            D::ErrUnhandledContextType.to_string(),
            "dtls-unhandled-context-type",
        ),
        (D::ErrContextCanceled.to_string(), "dtls-context-canceled"),
        (D::ErrEmptyFragment.to_string(), "dtls-empty-fragment"),
        (
            D::ErrAlertFatalOrClose.to_string(),
            "dtls-alert-fatal-or-close",
        ),
    ];
    if let Some((_, reason)) = known.into_iter().find(|(text, _)| text == error) {
        return Some(reason);
    }
    if error == ring::error::Unspecified.to_string() {
        return Some("crypto-operation-unspecified");
    }
    alert_reason(error)
}

fn alert_reason(error: &str) -> Option<&'static str> {
    let description = error
        .strip_prefix("Error of Alert Alert LevelFatal: ")
        .or_else(|| error.strip_prefix("Error of Alert Alert LevelWarning: "))?;
    // Full equality only: never preserve arbitrary peer-provided text.
    Some(match description {
        "CloseNotify" => "peer-alert-close-notify",
        "UnexpectedMessage" => "peer-alert-unexpected-message",
        "BadRecordMac" => "peer-alert-bad-record-mac",
        "DecryptionFailed" => "peer-alert-decryption-failed",
        "RecordOverflow" => "peer-alert-record-overflow",
        "DecompressionFailure" => "peer-alert-decompression-failure",
        "HandshakeFailure" => "peer-alert-handshake-failure",
        "NoCertificate" => "peer-alert-no-certificate",
        "BadCertificate" => "peer-alert-bad-certificate",
        "UnsupportedCertificate" => "peer-alert-unsupported-certificate",
        "CertificateRevoked" => "peer-alert-certificate-revoked",
        "CertificateExpired" => "peer-alert-certificate-expired",
        "CertificateUnknown" => "peer-alert-certificate-unknown",
        "IllegalParameter" => "peer-alert-illegal-parameter",
        "UnknownCA" => "peer-alert-unknown-ca",
        "AccessDenied" => "peer-alert-access-denied",
        "DecodeError" => "peer-alert-decode-error",
        "DecryptError" => "peer-alert-decrypt-error",
        "ExportRestriction" => "peer-alert-export-restriction",
        "ProtocolVersion" => "peer-alert-protocol-version",
        "InsufficientSecurity" => "peer-alert-insufficient-security",
        "InternalError" => "peer-alert-internal-error",
        "UserCanceled" => "peer-alert-user-canceled",
        "NoRenegotiation" => "peer-alert-no-renegotiation",
        "UnsupportedExtension" => "peer-alert-unsupported-extension",
        "UnknownPskIdentity" => "peer-alert-unknown-psk-identity",
        _ => return None,
    })
}

pub fn signature_reason(message: &str) -> Option<&'static str> {
    use ring::signature::*;
    macro_rules! algorithm {
        ($value:ident, $reason:literal) => {
            if message == format!("Picked an algorithm {:?}", $value) {
                return Some($reason);
            }
        };
    }
    algorithm!(ED25519, "verify-ed25519");
    algorithm!(ECDSA_P256_SHA256_ASN1, "verify-ecdsa-p256-sha256");
    algorithm!(ECDSA_P384_SHA384_ASN1, "verify-ecdsa-p384-sha384");
    algorithm!(
        RSA_PKCS1_2048_8192_SHA256,
        "verify-rsa-pkcs1-sha256-2048-8192"
    );
    algorithm!(
        RSA_PKCS1_2048_8192_SHA384,
        "verify-rsa-pkcs1-sha384-2048-8192"
    );
    algorithm!(
        RSA_PKCS1_2048_8192_SHA512,
        "verify-rsa-pkcs1-sha512-2048-8192"
    );
    algorithm!(
        RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY,
        "verify-rsa-pkcs1-sha1-legacy"
    );
    algorithm!(
        RSA_PKCS1_1024_8192_SHA256_FOR_LEGACY_USE_ONLY,
        "verify-rsa-pkcs1-sha256-legacy"
    );
    algorithm!(
        RSA_PKCS1_1024_8192_SHA512_FOR_LEGACY_USE_ONLY,
        "verify-rsa-pkcs1-sha512-legacy"
    );
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn previously_unmapped_crypto_and_signature_failures_are_recognized() {
        assert_eq!(
            dtls_reason(&D::ErrNoAvailableSignatureSchemes.to_string()),
            Some("dtls-no-available-signature-schemes")
        );
        assert_eq!(
            dtls_reason(&ring::error::Unspecified.to_string()),
            Some("crypto-operation-unspecified")
        );
        assert_eq!(
            dtls_reason("Error of Alert Alert LevelFatal: BadCertificate"),
            Some("peer-alert-bad-certificate")
        );
        assert!(
            dtls_reason("Error of Alert Alert LevelFatal: BadCertificate secret-token").is_none()
        );
        assert!(dtls_reason("private.example/password").is_none());
    }
    #[test]
    fn signature_metadata_requires_an_exact_known_algorithm() {
        let algorithm: &dyn ring::signature::VerificationAlgorithm =
            &ring::signature::ECDSA_P256_SHA256_ASN1;
        let message = format!("Picked an algorithm {algorithm:?}");
        assert_eq!(signature_reason(&message), Some("verify-ecdsa-p256-sha256"));
        assert!(signature_reason(&(message + " private-key-material")).is_none());
        assert!(signature_reason("private-key-material").is_none());
    }
}
