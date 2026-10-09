# Synthetic RSA verification-policy fixtures

These files contain newly generated synthetic RSA public keys (PKCS#1 DER)
and SHA-256/PKCS#1-v1.5 signatures over the fixed test message
`Parsec-native-WASM synthetic RSA policy audit v1`.

Private keys were generated in memory and were not saved. No certificate,
real host public key, capture, account, credentials or private key is included.
The test compares strict and explicitly legacy verification only in `cfg(test)`;
it does not change application cryptographic settings. Tampering must still fail.
