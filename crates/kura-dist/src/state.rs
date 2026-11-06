//! What the update window renders. The engine returns results; the app owns
//! the transitions between these states.

use std::path::PathBuf;

use crate::{Error, Release};

#[derive(Clone, Debug)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    /// A newer release than the running build.
    Available(Release),
    Downloading {
        release: Release,
        downloaded: u64,
        total: u64,
    },
    /// Verified and written to disk; installing it starts the installer.
    Staged {
        release: Release,
        artifact: PathBuf,
    },
    Failed(Error),
    /// This build cannot update itself, for example a development build.
    Disabled,
}
