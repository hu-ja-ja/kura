//! Win32 integration that has to outlive the vault window.
//!
//! The tray icon, the automatic lock triggers and the single-instance guard live
//! on a thread of their own, so closing the window does not close them. The
//! crate also owns the two places where the process touches secrets and the
//! operating system directly: DPAPI sealing for the saved login, and the
//! startup hardening that keeps a crash dump from carrying the vault.

#![cfg(windows)]

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

mod crash;
mod dpapi;
mod windows;

pub use crash::disable_dumps;
pub use dpapi::{seal as dpapi_seal, unseal as dpapi_unseal};

/// The window class the second instance looks for.
pub(crate) const WINDOW_CLASS: &str = "Kura.TrayWindow";

/// Something the tray asked the application to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Left click or the "show" menu item.
    Show,
    /// The tray "check for updates" item.
    CheckForUpdates,
    /// The tray lock item, the session lock, sleep or the idle timer.
    Lock,
    Quit,
}

/// How the tray menu follows the theme.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Appearance {
    /// Follow the Windows setting.
    #[default]
    System,
    Light,
    Dark,
}

/// The tray menu labels. Kept here so the platform crate has no i18n.
#[derive(Debug, Clone, Default)]
pub struct Labels {
    pub show: String,
    pub check_updates: String,
    pub lock: String,
    pub quit: String,
}

/// The handle the application keeps while it runs.
#[derive(Clone)]
pub struct Platform {
    labels: Arc<Mutex<Labels>>,
    auto_lock: Arc<AtomicU64>,
}

impl Platform {
    /// A handle with no tray thread behind it, for tests.
    pub fn detached() -> Self {
        Self {
            labels: Arc::new(Mutex::new(Labels::default())),
            auto_lock: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Replace the menu labels, for example after a language change.
    pub fn set_labels(&self, labels: Labels) {
        if let Ok(mut current) = self.labels.lock() {
            *current = labels;
        }
    }

    /// Change how long the vault may stay idle. Zero turns the timer off.
    pub fn set_auto_lock(&self, seconds: u64) {
        self.auto_lock.store(seconds, Ordering::Relaxed);
    }

    /// Change how the tray menu is drawn. The next right click picks it up.
    pub fn set_appearance(&self, appearance: Appearance) {
        windows::set_appearance(appearance);
    }
}

/// Keeps the named mutex open for the life of the process.
pub use windows::Instance;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    /// Another Kura is already running. It has been asked to show its window.
    AlreadyRunning,
    Failed(String),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => write!(f, "another Kura is already running"),
            Self::Failed(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for StartError {}

/// Start the tray. The returned instance guard must live until the process exits.
pub fn start(
    events: tokio::sync::mpsc::UnboundedSender<Event>,
    labels: Labels,
    auto_lock_seconds: u64,
) -> Result<(Platform, Instance), StartError> {
    let instance = windows::acquire_instance()?;

    let labels = Arc::new(Mutex::new(labels));
    let auto_lock = Arc::new(AtomicU64::new(auto_lock_seconds));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let thread_labels = labels.clone();
    let thread_timer = auto_lock.clone();

    std::thread::Builder::new()
        .name("kura-platform".to_string())
        .spawn(move || windows::run(events, thread_labels, thread_timer, ready_tx))
        .map_err(|err| StartError::Failed(format!("could not start the tray thread: {err}")))?;

    match ready_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(message)) => return Err(StartError::Failed(message)),
        Err(_) => return Err(StartError::Failed("the tray thread stopped".to_string())),
    }

    Ok((Platform { labels, auto_lock }, instance))
}

/// Add or remove the current executable from the user's startup entry.
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    windows::set_autostart(enabled)
}

/// Show a blocking error box for a startup failure that happens before any
/// window exists. The caller is about to exit.
pub fn show_error(message: &str) {
    windows::show_error(message)
}

/// Keep the window out of screen captures (Windows 10 2004 and later).
pub fn set_capture_protection(hwnd: isize, enabled: bool) -> Result<(), String> {
    windows::set_capture_protection(hwnd, enabled)
}

/// Hide a window without destroying it.
pub fn hide_window(hwnd: isize) {
    windows::hide_window(hwnd);
}

/// Show and focus a window that was hidden with [`hide_window`].
pub fn show_window(hwnd: isize) {
    windows::show_window(hwnd);
}

/// Give a window the icon the taskbar and Alt+Tab show.
pub fn set_window_icon(hwnd: isize) {
    windows::set_window_icon(hwnd);
}

/// Release resident pages while the app sits in the tray.
pub fn trim_working_set() {
    windows::trim_working_set();
}
