//! Windows CurrentUser DPAPI shared by native profiles and legacy server settings.
use anyhow::{bail, Result};
use windows_sys::Win32::{Foundation::LocalFree, Security::Cryptography::*};
pub fn protect(input: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    let blob = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(input.len())?,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut output: CRYPT_INTEGER_BLOB = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &blob,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &blob,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        bail!("Windows profile protection failed");
    }
    // DPAPI allocates this buffer; copy before releasing with the documented allocator.
    let bytes = if output.cbData == 0 {
        Vec::new()
    } else if output.pbData.is_null() {
        bail!("Windows profile protection returned no buffer");
    } else {
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() }
    };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(bytes)
}
