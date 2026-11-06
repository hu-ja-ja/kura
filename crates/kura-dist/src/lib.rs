//! The update engine and the release tooling around it.
//!
//! The library half checks a signed manifest, downloads and verifies the
//! installer, and hands a verified file to Inno Setup. The binary half builds
//! the installer and signs the manifest; it never ships with the app.

mod fetch;
mod installer;
mod manifest;
mod state;

pub use manifest::{Release, Version};
pub use state::State;

use std::fmt;
use std::path::{Path, PathBuf};

use ed25519_dalek::VerifyingKey;
use url::Url;

/// Everything the engine can fail at. The text is for the log; the variant
/// picks the localized message in the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Manifest(String),
    Signature(String),
    Download(String),
    Install(String),
    Configuration(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(detail) => write!(f, "the update manifest could not be read: {detail}"),
            Self::Signature(detail) => write!(f, "the update did not verify: {detail}"),
            Self::Download(detail) => write!(f, "the update could not be downloaded: {detail}"),
            Self::Install(detail) => write!(f, "the installer could not be started: {detail}"),
            Self::Configuration(detail) => write!(f, "the updater is not configured: {detail}"),
        }
    }
}

impl std::error::Error for Error {}

/// The update engine. `check`, `download` and `install` block; the app runs
/// them on the background executor.
#[derive(Clone)]
pub struct Updater {
    feed: Url,
    key: VerifyingKey,
    http: fetch::Http,
}

impl Updater {
    /// `public_key` is the base64 Ed25519 key the manifest is signed with.
    /// Plain HTTP is accepted only when the feed itself is plain HTTP, which
    /// is how the loopback tests run.
    pub fn new(feed: Url, public_key: &str) -> Result<Self, Error> {
        let key = manifest::verifying_key(public_key)?;
        let http = fetch::Http::new(feed.scheme() == "https")?;
        Ok(Self { feed, key, http })
    }

    /// The newest release, whatever the installed version is.
    pub fn latest(&self) -> Result<Release, Error> {
        let release = Release::from_json(&self.http.fetch(&self.feed)?, &self.key)?;
        release.validate(self.http.is_secure())?;
        Ok(release)
    }

    pub fn check(&self, current: Version) -> Result<Option<Release>, Error> {
        let release = self.latest()?;
        Ok((release.version > current).then_some(release))
    }

    /// Downloads the release into `staging`, reporting the bytes received so
    /// far. The file only appears there once it matches the manifest.
    pub fn download(
        &self,
        release: &Release,
        staging: &Path,
        progress: impl FnMut(u64, u64),
    ) -> Result<PathBuf, Error> {
        self.http.download(release, staging, progress)
    }

    /// Checks the staged file again and starts the installer.
    pub fn install(&self, release: &Release, artifact: &Path) -> Result<(), Error> {
        installer::launch(release, artifact)
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    text
}
