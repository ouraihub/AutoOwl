//! At-rest encryption for stored cookies, ported from Aether
//! `crates/aether-support/src/secret.rs`. Copied, not depended on: `aether-support` is
//! Aether's private crate ("hosts never depend on it directly") and no public crate
//! re-exports `seal`/`open`.
//!
//! A sealed BLOB is one format byte plus a platform payload:
//! `0x01` = Windows DPAPI `CryptProtectData` output (current-user scope).
//! Rows written before encryption hold raw cookie bytes, which never start with
//! `0x01` (cookies are printable ASCII), so [`open`] reports them as [`Opened::Legacy`].

const MARKER: u8 = 0x01;

/// What [`open`] found in a stored BLOB.
pub enum Opened {
    Sealed(String),
    /// Pre-encryption plaintext row; the caller should re-seal it.
    Legacy(String),
}

/// Encrypts `plain` into an opaque BLOB for storage.
pub fn seal(plain: &str) -> Result<Vec<u8>, String> {
    #[cfg(windows)]
    return dpapi::protect(plain.as_bytes());
    #[cfg(not(windows))]
    {
        // ponytail: plaintext off Windows (app only ships for Windows); add keyring when it doesn't.
        Ok(plain.as_bytes().to_vec())
    }
}

/// Decrypts a BLOB produced by [`seal`], or passes a legacy plaintext row through.
pub fn open(stored: &[u8]) -> Result<Opened, String> {
    match stored.split_first() {
        #[cfg(windows)]
        Some((&MARKER, payload)) => dpapi::unprotect(payload).map(Opened::Sealed),
        #[cfg(not(windows))]
        Some((&MARKER, _)) => Err("凭据由其他平台加密，无法读取".to_owned()),
        _ => String::from_utf8(stored.to_vec())
            .map(Opened::Legacy)
            .map_err(|_| "凭据格式无法识别".to_owned()),
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod dpapi {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    use super::MARKER;

    pub(super) fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
        let input = blob(plain)?;
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: `input` borrows `plain` for the call only; optional pointers are null;
        // on success DPAPI LocalAlloc's `out`, which `take` frees.
        unsafe {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        }
        .map_err(|error| format!("凭据加密失败: {error}"))?;
        Ok(take(out, |bytes| {
            let mut sealed = Vec::with_capacity(1 + bytes.len());
            sealed.push(MARKER);
            sealed.extend_from_slice(bytes);
            sealed
        }))
    }

    pub(super) fn unprotect(payload: &[u8]) -> Result<String, String> {
        let input = blob(payload)?;
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: same contract as `protect`; the description out-pointer is not requested.
        unsafe {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        }
        .map_err(|error| format!("凭据解密失败（可能属于其他 Windows 用户）: {error}"))?;
        take(out, |bytes| String::from_utf8(bytes.to_vec()))
            .map_err(|_| "凭据解密结果不是 UTF-8".to_owned())
    }

    fn blob(data: &[u8]) -> Result<CRYPT_INTEGER_BLOB, String> {
        Ok(CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(data.len()).map_err(|_| "凭据过长".to_owned())?,
            pbData: data.as_ptr().cast_mut(),
        })
    }

    /// Hands DPAPI's output to `copy`, then wipes and frees it.
    fn take<T>(out: CRYPT_INTEGER_BLOB, copy: impl FnOnce(&[u8]) -> T) -> T {
        if out.pbData.is_null() {
            return copy(&[]);
        }
        // SAFETY: after a successful call `pbData` points to `cbData` initialized bytes
        // owned by this process until `LocalFree`.
        let bytes = unsafe { std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize) };
        let value = copy(bytes);
        bytes.fill(0);
        // SAFETY: `pbData` was LocalAlloc'ed by DPAPI and is freed exactly once, after last use.
        unsafe { LocalFree(Some(HLOCAL(out.pbData.cast()))) };
        value
    }
}

#[cfg(test)]
mod tests {
    use super::{open, seal, Opened};

    #[cfg(windows)]
    #[test]
    fn sealed_roundtrips_and_hides_plaintext() {
        let plain = "MUSIC_U=abc123; __csrf=密钥";
        let sealed = seal(plain).expect("seal");
        assert_eq!(sealed[0], 0x01);
        assert!(!sealed.windows(plain.len()).any(|w| w == plain.as_bytes()));
        assert!(matches!(open(&sealed), Ok(Opened::Sealed(v)) if v == plain));
    }

    #[cfg(windows)]
    #[test]
    fn tampered_blob_is_rejected() {
        let mut sealed = seal("uin=123").expect("seal");
        let middle = sealed.len() / 2;
        sealed[middle] ^= 0x01;
        assert!(open(&sealed).is_err());
    }

    #[test]
    fn legacy_plaintext_row_is_flagged_for_reseal() {
        assert!(matches!(open(b"uin=123"), Ok(Opened::Legacy(v)) if v == "uin=123"));
    }
}
