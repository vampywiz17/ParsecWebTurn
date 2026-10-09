//! Synthetic public-only RSA verification-policy fixtures.
use ring::signature::{
    UnparsedPublicKey, RSA_PKCS1_1024_8192_SHA256_FOR_LEGACY_USE_ONLY, RSA_PKCS1_2048_8192_SHA256,
};

const MESSAGE: &[u8] = b"Parsec-native-WASM synthetic RSA policy audit v1";
const PUBLIC_1024: &[u8] = include_bytes!("../tests/fixtures/rsa-policy/rsa1024-public.der");
const SIGNATURE_1024: &[u8] = include_bytes!("../tests/fixtures/rsa-policy/rsa1024-signature.bin");
const PUBLIC_2048: &[u8] = include_bytes!("../tests/fixtures/rsa-policy/rsa2048-public.der");
const SIGNATURE_2048: &[u8] = include_bytes!("../tests/fixtures/rsa-policy/rsa2048-signature.bin");

#[test]
fn valid_rsa1024_signature_reproduces_the_strict_policy_failure() {
    let strict_1024 = UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, PUBLIC_1024);
    let error = strict_1024.verify(MESSAGE, SIGNATURE_1024).unwrap_err();
    assert_eq!(
        crate::transport_diagnostic_errors::dtls_reason(&error.to_string()),
        Some("crypto-operation-unspecified")
    );

    // Establish that rejection above is a policy/key-size boundary, rather
    // than a corrupt synthetic signature. The app requires explicit opt-in.
    let legacy_1024 =
        UnparsedPublicKey::new(&RSA_PKCS1_1024_8192_SHA256_FOR_LEGACY_USE_ONLY, PUBLIC_1024);
    legacy_1024.verify(MESSAGE, SIGNATURE_1024).unwrap();
    assert!(legacy_1024
        .verify(b"modified-message", SIGNATURE_1024)
        .is_err());
    let mut altered = SIGNATURE_1024.to_vec();
    altered[0] ^= 1;
    assert!(legacy_1024.verify(MESSAGE, &altered).is_err());

    // Modern RSA remains accepted by the application's existing strict policy.
    UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, PUBLIC_2048)
        .verify(MESSAGE, SIGNATURE_2048)
        .unwrap();
}
