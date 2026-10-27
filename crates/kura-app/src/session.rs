//! The saved login.
//!
//! It holds the tokens and the password-wrapped keys, so it is never written in
//! the clear: DPAPI seals it to the current Windows user.

use std::path::{Path, PathBuf};

use kura_core::SavedSession;
use windows_dpapi::{Scope, decrypt_data, encrypt_data};

const ENTROPY: &[u8] = b"Kura saved login v1";

fn path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("Kura").join("session.bin"))
}

/// A missing or unreadable file just means there is no saved login.
pub fn load() -> Option<SavedSession> {
    load_from(&path()?)
}

fn load_from(path: &Path) -> Option<SavedSession> {
    let sealed = std::fs::read(path).ok()?;
    let plain = decrypt_data(&sealed, Scope::User, Some(ENTROPY)).ok()?;
    serde_json::from_slice(&plain).ok()
}

pub fn save(session: &SavedSession) -> std::io::Result<()> {
    let Some(path) = path() else {
        return Ok(());
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let plain = serde_json::to_vec(session).map_err(std::io::Error::other)?;
    let sealed = encrypt_data(&plain, Scope::User, Some(ENTROPY))
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    let temp = path.with_extension("bin.tmp");
    std::fs::write(&temp, sealed)?;
    std::fs::rename(&temp, path)
}

pub fn clear() {
    if let Some(path) = path() {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreadable_session_file_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.bin");
        std::fs::write(&path, b"not a sealed session").unwrap();

        assert!(load_from(&path).is_none());
    }

    #[test]
    fn a_missing_session_file_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_from(&dir.path().join("nope.bin")).is_none());
    }
}
