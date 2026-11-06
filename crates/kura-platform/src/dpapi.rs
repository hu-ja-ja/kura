//! Sealing bytes to the current Windows user with DPAPI.
//!
//! `CRYPTPROTECT_UI_FORBIDDEN` keeps both directions silent: this runs from a
//! tray app that has no window to attach a prompt to. The buffer Windows hands
//! back is overwritten before it is freed, so an unsealed login does not stay
//! in the heap.

#![allow(unsafe_code)]

use std::ptr;
use std::slice;

use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use zeroize::Zeroizing;

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

/// Copy out what Windows allocated, then overwrite and free the original.
fn take(output: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    if output.pbData.is_null() {
        return Vec::new();
    }
    let len = output.cbData as usize;
    let copy = unsafe { slice::from_raw_parts(output.pbData, len) }.to_vec();
    unsafe {
        ptr::write_bytes(output.pbData, 0, len);
        LocalFree(output.pbData as HLOCAL);
    }
    copy
}

/// Seal `plain` so that only this user on this machine can open it again.
pub fn seal(plain: &[u8], entropy: &[u8]) -> Result<Vec<u8>, String> {
    let input = blob(plain);
    let entropy = blob(entropy);
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };

    let ok = unsafe {
        CryptProtectData(
            &input,
            ptr::null(),
            &entropy,
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }

    Ok(take(output))
}

/// Open bytes written by [`seal`] for this user.
pub fn unseal(sealed: &[u8], entropy: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
    let input = blob(sealed);
    let entropy = blob(entropy);
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };

    let ok = unsafe {
        CryptUnprotectData(
            &input,
            ptr::null_mut(),
            &entropy,
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }

    Ok(Zeroizing::new(take(output)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTROPY: &[u8] = b"Kura test entropy";

    #[test]
    fn sealed_bytes_open_again_for_the_same_user() {
        let sealed = seal(b"a saved login", ENTROPY).unwrap();

        assert_ne!(sealed, b"a saved login");
        assert_eq!(
            unseal(&sealed, ENTROPY).unwrap().as_slice(),
            b"a saved login"
        );
    }

    #[test]
    fn another_entropy_does_not_open_it() {
        let sealed = seal(b"a saved login", ENTROPY).unwrap();

        assert!(unseal(&sealed, b"some other entropy").is_err());
    }

    #[test]
    fn damaged_bytes_do_not_open() {
        let mut sealed = seal(b"a saved login", ENTROPY).unwrap();
        sealed[0] ^= 0xff;

        assert!(unseal(&sealed, ENTROPY).is_err());
    }

    #[test]
    fn an_empty_value_round_trips() {
        let sealed = seal(b"", ENTROPY).unwrap();

        assert!(unseal(&sealed, ENTROPY).unwrap().is_empty());
    }
}
