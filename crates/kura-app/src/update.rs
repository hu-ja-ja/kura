//! The update service: a signed manifest, a verified download, and the Inno
//! Setup handoff.
//!
//! The engine lives in `kura-dist`. This module owns the user's preferences,
//! the schedule, and the state the update window renders.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::{AppContext as _, Context};
use kura_dist::{Error, Release, State, Updater, Version};
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use url::Url;

/// The signed manifest, served from GitHub Pages.
const FEED_URL: &str = "https://hu-ja-ja.github.io/kura/manifest.json";

/// The Ed25519 key that signs releases. Set `ED25519_PUBLIC_KEY` for a
/// production build; without it the development key is used and a release
/// build refuses to update.
const TEST_PUBLIC_KEY: &str = "X4kvjOoTZUsPKjjF0W/qzutLOb3d9LPmYZ4szZGg8wI=";
const PUBLIC_KEY: &str = match option_env!("ED25519_PUBLIC_KEY") {
    Some(key) => key,
    None => TEST_PUBLIC_KEY,
};

const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(10);
const REMIND_INTERVAL_MS: u64 = 24 * 60 * 60 * 1000;

/// Messages that need the window controller rather than this entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Open the update window. Background requests wait while the main
    /// window is in the tray.
    Open { background: bool },
    /// Close the update window.
    Close,
    /// The installer is running; quit through the normal path.
    Quit,
}

/// Preferences owned by the updater. Kept separate from `settings.json` so
/// the main window and the update service never overwrite each other.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    pub auto_check: bool,
    pub auto_install: bool,
    pub skipped_version: Option<String>,
    pub remind_after_unix_ms: Option<u64>,
    pub last_check_unix_ms: Option<u64>,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            auto_check: true,
            auto_install: false,
            skipped_version: None,
            remind_after_unix_ms: None,
            last_check_unix_ms: None,
        }
    }
}

impl UpdateSettings {
    pub fn path() -> Option<PathBuf> {
        let appdata = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(appdata).join("Kura").join("update.json"))
    }

    pub fn load() -> Self {
        Self::path().map_or_else(Self::default, |path| Self::load_from(&path))
    }

    pub fn load_from(path: &std::path::Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Self::default();
            }
            Err(err) => {
                log::warn!(
                    "could not read the update settings at {}: {err}",
                    crate::logging::display(path)
                );
                return Self::default();
            }
        };
        match serde_json::from_str(&text) {
            Ok(settings) => settings,
            Err(err) => {
                log::warn!(
                    "ignoring the broken update settings at {}: {err}",
                    crate::logging::display(path)
                );
                Self::default()
            }
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Self::path() else {
            return Ok(());
        };
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        crate::json_file::save_atomic(path, self)
    }
}

/// A short, localized message for an update error. The engine's English
/// diagnostic still goes to the log.
pub(crate) fn error_text(error: &Error) -> String {
    let key = match error {
        Error::Manifest(_) => "update.error_manifest",
        Error::Signature(_) => "update.error_verify",
        Error::Download(_) => "update.error_download",
        Error::Install(_) => "update.error_installer",
        Error::Configuration(_) => "update.error_configuration",
    };
    t!(key).to_string()
}

/// What the background download reports back to the window.
enum Progress {
    Bytes(u64),
    Done {
        release: Release,
        artifact: Result<PathBuf, Error>,
    },
}

/// The update engine and the state the window renders.
pub struct UpdateService {
    updater: Option<Updater>,
    state: State,
    settings: UpdateSettings,
    feedback: Option<String>,
    installing: bool,
    persist: bool,
    requests: UnboundedSender<Request>,
}

impl UpdateService {
    pub fn new(requests: UnboundedSender<Request>, cx: &mut Context<Self>) -> Self {
        let settings = UpdateSettings::load();
        let (updater, state) = match build_updater() {
            Ok(updater) => (Some(updater), State::Idle),
            Err(error) => {
                if cfg!(debug_assertions) {
                    log::info!("updates are unavailable: {error}");
                } else {
                    log::warn!("updates are unavailable: {error}");
                }
                // The localized Disabled text is enough for the window; the
                // build-specific English detail goes to the log only.
                (None, State::Disabled)
            }
        };

        #[cfg(debug_assertions)]
        let preview = std::env::var("KURA_UPDATE_PREVIEW")
            .ok()
            .and_then(|kind| preview_state(&kind));
        #[cfg(not(debug_assertions))]
        let preview: Option<State> = None;

        // A preview must not check the network or replace the fake state.
        let persist = preview.is_none();
        let (updater, state) = match preview {
            Some(state) => (None, state),
            None => (updater, state),
        };

        let service = Self {
            updater,
            state,
            settings,
            feedback: None,
            installing: false,
            persist,
            requests,
        };

        if !service.persist {
            let _ = service.requests.send(Request::Open { background: false });
        }
        if service.updater.is_some() {
            service.schedule_auto_check(cx);
        }

        service
    }

    #[cfg(test)]
    pub fn detached(state: State) -> Self {
        let (requests, _receiver) = tokio::sync::mpsc::unbounded_channel();
        Self {
            updater: None,
            state,
            settings: UpdateSettings::default(),
            feedback: None,
            installing: false,
            persist: false,
            requests,
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn feedback(&self) -> Option<&str> {
        self.feedback.as_deref()
    }

    pub fn settings(&self) -> &UpdateSettings {
        &self.settings
    }

    pub fn current_version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    pub fn check_now(&mut self, cx: &mut Context<Self>) {
        self.check(true, cx);
    }

    pub fn set_auto_check(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.auto_check = enabled;
        if !enabled {
            self.settings.auto_install = false;
        }
        self.save_settings();
        if enabled {
            self.check(false, cx);
        }
        cx.notify();
    }

    pub fn set_auto_install(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.auto_install = enabled;
        if enabled {
            self.settings.auto_check = true;
        }
        self.save_settings();
        cx.notify();
    }

    /// Return the update preferences to their defaults and persist them.
    pub fn reset_settings(&mut self, cx: &mut Context<Self>) {
        self.settings = UpdateSettings::default();
        self.save_settings();
        cx.notify();
    }

    pub fn download(&mut self, cx: &mut Context<Self>) {
        self.download_inner(true, cx);
    }

    pub fn restart(&mut self, cx: &mut Context<Self>) {
        self.install(cx);
    }

    pub fn remind(&mut self, cx: &mut Context<Self>) {
        self.settings.remind_after_unix_ms = Some(now_ms().saturating_add(REMIND_INTERVAL_MS));
        self.save_settings();
        let _ = self.requests.send(Request::Close);
        cx.notify();
    }

    pub fn later(&mut self, cx: &mut Context<Self>) {
        let _ = self.requests.send(Request::Close);
        cx.notify();
    }

    pub fn skip(&mut self, cx: &mut Context<Self>) {
        if let State::Available(release) = &self.state {
            self.settings.skipped_version = Some(release.version.to_string());
            self.save_settings();
        }
        let _ = self.requests.send(Request::Close);
        cx.notify();
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.feedback = None;
        cx.notify();
    }

    /// Called from the application quit hook. A staged update is installed
    /// while the app exits so the installer can restart into the new version.
    pub fn install_on_quit(&mut self) {
        let State::Staged { release, artifact } = self.state.clone() else {
            return;
        };
        let Some(updater) = self.updater.clone() else {
            return;
        };
        if self.installing {
            return;
        }
        self.installing = true;
        match updater.install(&release, &artifact) {
            Ok(()) => log::info!("handed a staged update to the installer while quitting"),
            Err(error) => {
                self.installing = false;
                log::warn!(
                    "could not install the staged update while quitting: {}",
                    crate::logging::redact(&error.to_string())
                );
            }
        }
    }

    fn check(&mut self, manual: bool, cx: &mut Context<Self>) {
        if self.is_busy() {
            if manual {
                let _ = self.requests.send(Request::Open { background: false });
            }
            return;
        }

        let Some(updater) = self.updater.clone() else {
            self.feedback = Some(t!("update.cannot_check").to_string());
            if manual {
                let _ = self.requests.send(Request::Open { background: false });
            }
            cx.notify();
            return;
        };

        if manual {
            self.feedback = None;
            let _ = self.requests.send(Request::Open { background: false });
        }
        self.state = State::Checking;

        let current = current_version();
        let this = cx.entity();
        cx.spawn(async move |_this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { updater.check(current) })
                .await;
            cx.update_entity(&this, |this, cx| {
                this.after_check(result, manual, cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn after_check(
        &mut self,
        result: Result<Option<Release>, Error>,
        manual: bool,
        cx: &mut Context<Self>,
    ) {
        self.settings.last_check_unix_ms = Some(now_ms());
        self.save_settings();

        match result {
            Ok(Some(release)) => self.state = State::Available(release),
            Ok(None) => self.state = State::UpToDate,
            Err(error) => {
                if manual {
                    self.feedback = Some(error_text(&error));
                    log::warn!("manual update check failed: {error}");
                } else {
                    log::warn!("background update check failed: {error}");
                }
                self.state = State::Failed(error);
            }
        }

        if manual {
            let _ = self.requests.send(Request::Open { background: false });
        } else if let State::Available(release) = self.state.clone()
            && self.should_notify(&release)
        {
            if self.settings.auto_install {
                self.download_inner(false, cx);
            } else {
                let _ = self.requests.send(Request::Open { background: true });
            }
        }
        cx.notify();
    }

    fn download_inner(&mut self, open_after: bool, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let State::Available(release) = self.state.clone() else {
            return;
        };
        let Some(updater) = self.updater.clone() else {
            self.feedback = Some(t!("update.unavailable").to_string());
            cx.notify();
            return;
        };
        let Some(staging) = staging_dir() else {
            self.feedback = Some(t!("update.unavailable").to_string());
            cx.notify();
            return;
        };

        self.state = State::Downloading {
            release: release.clone(),
            downloaded: 0,
            total: release.size,
        };

        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let completion = sender.clone();
        cx.background_executor()
            .spawn(async move {
                let artifact = updater.download(&release, &staging, move |downloaded, _total| {
                    let _ = sender.send(Progress::Bytes(downloaded));
                });
                let _ = completion.send(Progress::Done { release, artifact });
            })
            .detach();

        let this = cx.entity();
        cx.spawn(async move |_this, cx| {
            while let Some(progress) = receiver.recv().await {
                cx.update_entity(&this, |this, cx| {
                    this.on_progress(progress, open_after, cx);
                });
            }
        })
        .detach();
        cx.notify();
    }

    fn on_progress(&mut self, progress: Progress, open_after: bool, cx: &mut Context<Self>) {
        match progress {
            Progress::Bytes(downloaded) => {
                if let State::Downloading {
                    downloaded: current,
                    ..
                } = &mut self.state
                {
                    *current = downloaded;
                }
            }
            Progress::Done { release, artifact } => match artifact {
                Ok(artifact) => {
                    self.state = State::Staged { release, artifact };
                    self.feedback = None;
                    if open_after {
                        let _ = self.requests.send(Request::Open { background: false });
                    }
                }
                Err(error) => {
                    self.feedback = Some(error_text(&error));
                    log::warn!(
                        "could not download the update: {}",
                        crate::logging::redact(&error.to_string())
                    );
                    self.state = State::Failed(error);
                }
            },
        }
        cx.notify();
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        let State::Staged { release, artifact } = self.state.clone() else {
            return;
        };
        let Some(updater) = self.updater.clone() else {
            self.feedback = Some(t!("update.unavailable").to_string());
            cx.notify();
            return;
        };
        if self.installing {
            return;
        }
        self.installing = true;

        let this = cx.entity();
        cx.spawn(async move |_this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { updater.install(&release, &artifact) })
                .await;
            cx.update_entity(&this, |this, cx| {
                match result {
                    Ok(()) => {
                        let _ = this.requests.send(Request::Quit);
                    }
                    Err(error) => {
                        this.installing = false;
                        this.feedback = Some(error_text(&error));
                        log::warn!(
                            "could not start the update installer: {}",
                            crate::logging::redact(&error.to_string())
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn schedule_auto_check(&self, cx: &mut Context<Self>) {
        let this = cx.entity();
        cx.spawn(async move |_this, cx| {
            cx.background_executor().timer(FIRST_CHECK_DELAY).await;
            loop {
                cx.update_entity(&this, |this, cx| {
                    if this.settings.auto_check && !this.is_busy() && this.last_check_elapsed() {
                        this.check(false, cx);
                    }
                });
                cx.background_executor().timer(CHECK_INTERVAL).await;
            }
        })
        .detach();
    }

    fn is_busy(&self) -> bool {
        matches!(self.state, State::Checking | State::Downloading { .. })
    }

    fn last_check_elapsed(&self) -> bool {
        self.settings
            .last_check_unix_ms
            .is_none_or(|last| now_ms().saturating_sub(last) >= CHECK_INTERVAL.as_millis() as u64)
    }

    fn should_notify(&self, release: &Release) -> bool {
        let version = release.version.to_string();
        if self.settings.skipped_version.as_deref() == Some(version.as_str()) {
            return false;
        }
        !self
            .settings
            .remind_after_unix_ms
            .is_some_and(|after| now_ms() < after)
    }

    fn save_settings(&self) {
        if !self.persist {
            return;
        }
        if let Err(error) = self.settings.save() {
            log::warn!("could not save update settings: {error}");
        }
    }
}

#[cfg(debug_assertions)]
fn preview_state(kind: &str) -> Option<State> {
    fn release() -> Release {
        Release::new(
            Version::new(0, 2, 0),
            "https://example.invalid/Kura-0.2.0-setup.exe".to_owned(),
            16 * 1024 * 1024,
            String::new(),
            "# Kura 0.2.0\n\n- Faster search\n- A calmer update window".to_owned(),
        )
    }

    match kind {
        "available" => Some(State::Available(release())),
        "downloading" => Some(State::Downloading {
            release: release(),
            downloaded: 7 * 1024 * 1024,
            total: 16 * 1024 * 1024,
        }),
        "staged" => Some(State::Staged {
            release: release(),
            artifact: PathBuf::from("Kura-0.2.0-setup.exe"),
        }),
        "failed" => Some(State::Failed(Error::Download(
            "The download failed. Check the network and retry.".to_owned(),
        ))),
        _ => None,
    }
}

/// The configuration a build runs with: the release key and the manifest.
fn build_updater() -> Result<Updater, String> {
    if cfg!(debug_assertions) && std::env::var_os("KURA_UPDATE_DEBUG").is_none() {
        return Err("Updates are unavailable in a development build.".to_owned());
    }
    if !cfg!(debug_assertions) && PUBLIC_KEY.trim() == TEST_PUBLIC_KEY {
        return Err("This build has the development update key and cannot update.".to_owned());
    }

    let feed =
        Url::parse(FEED_URL).map_err(|error| format!("the update feed URL is invalid: {error}"))?;
    Updater::new(feed, PUBLIC_KEY).map_err(|error| error.to_string())
}

fn staging_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|root| PathBuf::from(root).join("Kura").join("updates"))
}

fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION"))
        .expect("the package version is a release version (x.y.z)")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

#[cfg(test)]
mod tests;
