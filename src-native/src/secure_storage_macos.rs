//! Documented Security framework Keychain item holding a random per-user key.
//! Like DPAPI, protected values do not decrypt for another user or machine.
// Tests use a fixed key and never reach the user's Keychain.
#![cfg_attr(test, allow(dead_code))]
use anyhow::{bail, Context, Result};
use objc2::{rc::Retained, runtime::AnyObject};
use objc2_foundation::{NSData, NSDictionary, NSNumber, NSString};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305, NONCE_LEN};
use std::ffi::c_void;

#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecClass: &'static NSString;
    static kSecClassGenericPassword: &'static NSString;
    static kSecAttrService: &'static NSString;
    static kSecAttrAccount: &'static NSString;
    static kSecReturnData: &'static NSString;
    static kSecValueData: &'static NSString;
    fn SecItemCopyMatching(query: *const c_void, result: *mut *const c_void) -> i32;
    fn SecItemAdd(attributes: *const c_void, result: *mut *const c_void) -> i32;
}
const NOT_FOUND: i32 = -25300;
const DUPLICATE: i32 = -25299;
const SERVICE: &str = "EvoParseq";
const ACCOUNT: &str = "settings-protection-key";

fn query(extra: &[(&NSString, &AnyObject)]) -> Retained<NSDictionary<NSString, AnyObject>> {
    let service = NSString::from_str(SERVICE);
    let account = NSString::from_str(ACCOUNT);
    let mut keys: Vec<&NSString> = unsafe { vec![kSecClass, kSecAttrService, kSecAttrAccount] };
    let mut values: Vec<&AnyObject> = vec![unsafe { kSecClassGenericPassword }, &service, &account];
    for (key, value) in extra {
        keys.push(key);
        values.push(value);
    }
    NSDictionary::from_slices(&keys, &values)
}

fn read_key() -> Result<Option<Vec<u8>>> {
    let yes = NSNumber::new_bool(true);
    let query = query(&[(unsafe { kSecReturnData }, &yes)]);
    let mut result = std::ptr::null();
    let status = unsafe { SecItemCopyMatching(Retained::as_ptr(&query).cast(), &mut result) };
    if status == NOT_FOUND {
        return Ok(None);
    }
    if status != 0 || result.is_null() {
        bail!("Keychain protection key unavailable ({status})");
    }
    // A data query returns one owned CFData, toll-free bridged to NSData.
    let data = unsafe { Retained::from_raw(result as *mut NSData) }.context("Keychain data")?;
    Ok(Some(data.to_vec()))
}

fn key() -> Result<Vec<u8>> {
    if let Some(key) = read_key()? {
        return Ok(key);
    }
    let mut key = vec![0; 32];
    getrandom::fill(&mut key).map_err(|_| anyhow::anyhow!("Cannot create protection key"))?;
    let data = NSData::with_bytes(&key);
    let attributes = query(&[(unsafe { kSecValueData }, &data)]);
    match unsafe { SecItemAdd(Retained::as_ptr(&attributes).cast(), std::ptr::null_mut()) } {
        0 => Ok(key),
        // Another instance created it first; use the stored key.
        DUPLICATE => read_key()?.context("Keychain protection key unavailable"),
        status => bail!("Cannot store the Keychain protection key ({status})"),
    }
}

pub fn protect(input: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    // Tests never touch the user's Keychain or real profile protection.
    #[cfg(test)]
    let key = vec![7; 32];
    #[cfg(not(test))]
    let key = zeroize::Zeroizing::new(key()?);
    seal(&key, input, encrypt)
}

fn seal(key: &[u8], input: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    let key = LessSafeKey::new(
        UnboundKey::new(&CHACHA20_POLY1305, key).map_err(|_| anyhow::anyhow!("invalid key"))?,
    );
    if encrypt {
        let mut nonce = [0; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|_| anyhow::anyhow!("Cannot create nonce"))?;
        let mut sealed = input.to_vec();
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::empty(),
            &mut sealed,
        )
        .map_err(|_| anyhow::anyhow!("macOS profile protection failed"))?;
        Ok([&nonce[..], &sealed].concat())
    } else {
        if input.len() < NONCE_LEN {
            bail!("macOS profile protection failed");
        }
        let (nonce, sealed) = input.split_at(NONCE_LEN);
        let mut sealed = sealed.to_vec();
        let plain = key
            .open_in_place(
                Nonce::try_assume_unique_for_key(nonce).unwrap(),
                Aad::empty(),
                &mut sealed,
            )
            .map_err(|_| anyhow::anyhow!("macOS profile protection failed"))?;
        Ok(plain.to_vec())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn values_round_trip_and_tampering_or_another_key_is_rejected() {
        let sealed = super::protect(b"token", true).unwrap();
        assert_ne!(&sealed[12..17], b"token");
        assert_eq!(super::protect(&sealed, false).unwrap(), b"token");
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(super::protect(&tampered, false).is_err());
        assert!(super::seal(&[8; 32], &sealed, false).is_err());
        assert!(super::protect(b"short", false).is_err());
    }
}
