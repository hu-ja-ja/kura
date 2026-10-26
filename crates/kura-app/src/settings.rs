//! Persisted settings.
//!
//! Only preferences live here. No secret is ever written to this file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// UI language: "ja" or "en". Empty means the system language.
    pub language: Option<String>,
    /// Base URL of a self-hosted server. Empty means bitwarden.com.
    pub server_url: Option<String>,
}

impl Settings {
    /// Where the settings live. `None` when the system has no app data folder.
    pub fn path() -> Option<PathBuf> {
        let appdata = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(appdata).join("Kura").join("settings.json"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        Self::load_from(&path)
    }

    /// A broken file is not worth failing over: start with the defaults.
    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Self::path() else {
            return Ok(());
        };
        self.save_to(&path)
    }

    /// Write through a temporary file so a crash cannot leave a half file.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, text)?;
        std::fs::rename(&temp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        let settings = Settings {
            language: Some("ja".to_string()),
            server_url: Some("https://vault.example.com".to_string()),
        };
        settings.save_to(&path).unwrap();

        assert_eq!(Settings::load_from(&path), settings);
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Settings::load_from(&dir.path().join("nope.json")),
            Settings::default()
        );
    }

    #[test]
    fn a_broken_file_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
    }

    #[test]
    fn saving_twice_keeps_the_file_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        Settings::default().save_to(&path).unwrap();
        let settings = Settings {
            language: Some("en".to_string()),
            server_url: None,
        };
        settings.save_to(&path).unwrap();

        assert_eq!(Settings::load_from(&path), settings);
    }
}
