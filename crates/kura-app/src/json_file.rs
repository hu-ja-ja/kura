//! Atomic JSON writes shared by the preference files.

use std::path::Path;

use serde::Serialize;

/// Write `value` as pretty JSON through a temporary file and a rename, so a
/// crash cannot leave a half-written file.
pub fn save_atomic<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path)
}
