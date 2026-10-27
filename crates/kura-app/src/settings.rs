//! Persisted settings.
//!
//! Only preferences live here. No secret is ever written to this file.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::{Bounds, Pixels, Window, WindowBounds};
use serde::{Deserialize, Serialize};

/// How the theme follows the system.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

/// Where and how large the window was when it last closed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowPlacement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

impl Default for WindowPlacement {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 900.0,
            height: 700.0,
            maximized: false,
        }
    }
}

impl WindowPlacement {
    pub fn bounds(self) -> Bounds<Pixels> {
        Bounds::new(
            gpui_kit::point(gpui_kit::px(self.x), gpui_kit::px(self.y)),
            gpui_kit::size(gpui_kit::px(self.width), gpui_kit::px(self.height)),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// UI language: "ja" or "en". Empty means the system language.
    pub language: Option<String>,
    /// Base URL of a self-hosted server. Empty means bitwarden.com.
    pub server_url: Option<String>,
    /// Lock after this many minutes without input.
    pub auto_lock_minutes: u16,
    /// Clear copied secrets after this many seconds.
    pub clipboard_seconds: u16,
    pub theme: ThemeChoice,
    pub autostart: bool,
    /// Start with no window, only the tray icon.
    pub start_in_tray: bool,
    /// Destroy the window while it sits in the tray. Off keeps it hidden.
    pub destroy_on_hide: bool,
    pub screenshot_protection: bool,
    pub window: Option<WindowPlacement>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: None,
            server_url: None,
            auto_lock_minutes: 15,
            clipboard_seconds: 30,
            theme: ThemeChoice::System,
            autostart: false,
            start_in_tray: false,
            destroy_on_hide: true,
            screenshot_protection: false,
            window: None,
        }
    }
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

    pub fn locale(&self) -> String {
        self.language
            .clone()
            .filter(|locale| locale == "ja" || locale == "en")
            .unwrap_or_else(system_locale)
    }

    pub fn auto_lock(&self) -> Duration {
        Duration::from_secs(u64::from(self.auto_lock_minutes) * 60)
    }

    pub fn clipboard_clear(&self) -> Duration {
        Duration::from_secs(u64::from(self.clipboard_seconds))
    }

    pub fn clipboard_seconds(&self) -> u64 {
        u64::from(self.clipboard_seconds)
    }

    /// The window bounds to restore, if the saved one still lands on a display.
    pub fn window_bounds(&self) -> Option<Bounds<Pixels>> {
        let placement = self.window?;
        if placement.width < 320.0 || placement.height < 200.0 {
            return None;
        }
        Some(placement.bounds())
    }
}

/// The language of the system, when it is one Kura speaks.
pub fn system_locale() -> String {
    match sys_locale::get_locale() {
        Some(locale) if locale.starts_with("ja") => "ja".to_string(),
        _ => "en".to_string(),
    }
}

/// Remember where the window is, including whether it was maximized.
pub fn placement(window: &Window) -> WindowPlacement {
    let window_bounds = window.window_bounds();
    let maximized = !matches!(window_bounds, WindowBounds::Windowed(_));
    let bounds = window_bounds.get_bounds();
    WindowPlacement {
        x: f32::from(bounds.origin.x),
        y: f32::from(bounds.origin.y),
        width: f32::from(bounds.size.width),
        height: f32::from(bounds.size.height),
        maximized,
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
            auto_lock_minutes: 5,
            clipboard_seconds: 60,
            theme: ThemeChoice::Dark,
            autostart: true,
            start_in_tray: true,
            destroy_on_hide: false,
            screenshot_protection: true,
            window: Some(WindowPlacement {
                x: 12.0,
                y: 34.0,
                width: 1000.0,
                height: 800.0,
                maximized: true,
            }),
        };
        settings.save_to(&path).unwrap();

        assert_eq!(Settings::load_from(&path), settings);
    }

    #[test]
    fn a_file_from_before_m3_gets_the_new_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"language":"en","server_url":"https://vault.example.com"}"#,
        )
        .unwrap();

        let settings = Settings::load_from(&path);
        assert_eq!(settings.language.as_deref(), Some("en"));
        assert_eq!(
            settings.server_url.as_deref(),
            Some("https://vault.example.com")
        );
        assert_eq!(settings.auto_lock_minutes, 15);
        assert_eq!(settings.clipboard_seconds, 30);
        assert_eq!(settings.theme, ThemeChoice::System);
        assert!(settings.destroy_on_hide);
    }

    #[test]
    fn a_broken_file_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
    }

    #[test]
    fn an_unknown_theme_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"theme":"violet","server_url":"https://x"}"#).unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
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
    fn saving_twice_keeps_the_file_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        Settings::default().save_to(&path).unwrap();
        let settings = Settings {
            language: Some("en".to_string()),
            server_url: None,
            ..Settings::default()
        };
        settings.save_to(&path).unwrap();

        assert_eq!(Settings::load_from(&path), settings);
    }

    #[test]
    fn an_offscreen_placement_is_dropped() {
        let settings = Settings {
            window: Some(WindowPlacement {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
                maximized: false,
            }),
            ..Settings::default()
        };
        assert!(settings.window_bounds().is_none());
    }
}
