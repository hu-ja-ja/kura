//! The saved login.
//!
//! It holds the tokens and the password-wrapped keys, so it is never written in
//! the clear: DPAPI seals it to the current Windows user. Any process running as
//! that user can still open it, which is the same trade-off Bitwarden Desktop
//! makes: the file is safe against another account, a backup and another
//! machine, not against code already running as the user.

use std::path::{Path, PathBuf};

use kura_core::SavedSession;
use zeroize::Zeroizing;

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
    let sealed = match std::fs::read(path) {
        Ok(sealed) => sealed,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            log::warn!(
                "could not read the saved login at {}: {err}",
                crate::logging::display(path)
            );
            return None;
        }
    };
    let plain = match kura_platform::dpapi_unseal(&sealed, ENTROPY) {
        Ok(plain) => plain,
        Err(err) => {
            log::warn!(
                "could not unseal the saved login at {}: {err}",
                crate::logging::display(path)
            );
            return None;
        }
    };
    match serde_json::from_slice(&plain) {
        Ok(session) => Some(session),
        Err(err) => {
            log::warn!(
                "ignoring the broken saved login at {}: {err}",
                crate::logging::display(path)
            );
            None
        }
    }
}

pub fn save(session: &SavedSession) -> std::io::Result<()> {
    let Some(path) = path() else {
        return Ok(());
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let plain = Zeroizing::new(serde_json::to_vec(session).map_err(std::io::Error::other)?);
    let sealed = kura_platform::dpapi_seal(&plain, ENTROPY).map_err(std::io::Error::other)?;
    let temp = path.with_extension("bin.tmp");
    std::fs::write(&temp, sealed)?;
    std::fs::rename(&temp, path)
}

pub fn clear() -> std::io::Result<()> {
    match path() {
        Some(path) => clear_at(&path),
        None => Ok(()),
    }
}

fn clear_at(path: &Path) -> std::io::Result<()> {
    remove_if_present(path)?;
    // `save` writes the sealed login here before the rename. A crash or a
    // failed rename can leave a complete, sealed copy behind.
    remove_if_present(&path.with_extension("bin.tmp"))
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(std::io::Error::new(
            err.kind(),
            crate::logging::redact(&format!("{}: {err}", path.display())),
        )),
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

    #[test]
    fn clearing_removes_the_saved_login_and_its_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.bin");
        let temp = path.with_extension("bin.tmp");
        std::fs::write(&path, b"sealed").unwrap();
        std::fs::write(&temp, b"sealed").unwrap();

        clear_at(&path).unwrap();

        assert!(!path.exists());
        assert!(!temp.exists());
    }

    #[test]
    fn clearing_a_missing_saved_login_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        assert!(clear_at(&dir.path().join("nope.bin")).is_ok());
    }

    #[test]
    fn a_saved_login_that_cannot_be_removed_reports_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.bin");
        std::fs::create_dir(&path).unwrap();

        let error = clear_at(&path).unwrap_err();

        assert!(error.to_string().contains("session.bin"));
    }
}
