//! A small appender for `%LOCALAPPDATA%\Kura\logs`.
//!
//! Two rolled files are kept. The logger never records request bodies or
//! secret values; call sites log what happened, not what the vault holds.
//! Messages stay to static text, paths, version numbers and error codes:
//! never a password, a code, a token, an email, an item value or a server
//! response body.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const LIMIT: u64 = 1_000_000;

/// The folder that holds `kura.log` and its rotated copies.
pub fn folder() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(local).join("Kura").join("logs"))
}

/// Format a path for the log without the Windows profile name.
pub(crate) fn display(path: &Path) -> String {
    display_with(path, &roots())
}

/// Remove known profile paths from an error text before it is logged.
pub(crate) fn redact(text: &str) -> String {
    redact_with(text, &roots())
}

fn roots() -> Vec<(&'static str, PathBuf)> {
    [
        ("APPDATA", "APPDATA"),
        ("LOCALAPPDATA", "LOCALAPPDATA"),
        ("USERPROFILE", "USERPROFILE"),
    ]
    .into_iter()
    .filter_map(|(name, variable)| {
        std::env::var_os(variable)
            .map(PathBuf::from)
            .map(|root| (name, root))
    })
    .collect()
}

fn display_with(path: &Path, roots: &[(&str, PathBuf)]) -> String {
    for (name, root) in roots {
        if root.as_os_str().is_empty() {
            continue;
        }
        if let Ok(rest) = path.strip_prefix(root) {
            let rest = rest.display().to_string();
            return if rest.is_empty() {
                format!("%{name}%")
            } else {
                format!("%{name}%\\{rest}")
            };
        }
    }
    path.display().to_string()
}

fn redact_with(text: &str, roots: &[(&str, PathBuf)]) -> String {
    let mut text = text.to_string();
    for (name, root) in roots {
        if root.as_os_str().is_empty() {
            continue;
        }
        text = text.replace(&root.display().to_string(), &format!("%{name}%"));
    }
    text
}

pub fn init() {
    let Some(path) = log_path() else {
        return;
    };
    if let Some(dir) = path.parent()
        && std::fs::create_dir_all(dir).is_err()
    {
        return;
    }
    let Ok(logger) = Logger::open(path) else {
        return;
    };
    if log::set_boxed_logger(Box::new(logger)).is_err() {
        return;
    }
    log::set_max_level(log::LevelFilter::Info);

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        log::logger().flush();
        previous(info);
    }));
}

fn log_path() -> Option<PathBuf> {
    Some(folder()?.join("kura.log"))
}

struct Logger {
    path: PathBuf,
    file: Mutex<File>,
    written: AtomicU64,
}

impl Logger {
    fn open(path: PathBuf) -> std::io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let written = file.metadata()?.len();
        Ok(Self {
            path,
            file: Mutex::new(file),
            written: AtomicU64::new(written),
        })
    }
}

/// The target for a log line. GPUI logs dependency errors with an empty
/// target, so fall back to the source location rather than leaving a blank.
fn source(record: &log::Record<'_>) -> String {
    if !record.target().is_empty() {
        return record.target().to_string();
    }
    match (record.file(), record.line()) {
        (Some(file), Some(line)) => format!("{file}:{line}"),
        (Some(file), None) => file.to_string(),
        _ => "unknown".to_string(),
    }
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {} {}: {}\n",
            timestamp(SystemTime::now()),
            record.level(),
            source(record),
            record.args()
        );
        let mut file = match self.file.lock() {
            Ok(file) => file,
            Err(_) => return,
        };
        let written = self.written.load(Ordering::Relaxed);
        if written > 0
            && written + line.len() as u64 > LIMIT
            && rotate(&self.path).is_ok()
            && let Ok(new_file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
        {
            *file = new_file;
            self.written.store(0, Ordering::Relaxed);
        }
        if file.write_all(line.as_bytes()).is_ok() {
            self.written.fetch_add(line.len() as u64, Ordering::Relaxed);
        }
    }

    fn flush(&self) {
        if let Ok(mut file) = self.file.lock() {
            let _ = file.flush();
        }
    }
}

/// Keep `kura.log` and one older file. `kura.log.2` is replaced.
fn rotate(path: &Path) -> std::io::Result<()> {
    let second = path.with_extension("log.2");
    let first = path.with_extension("log.1");
    let _ = std::fs::remove_file(&second);
    if first.exists() {
        std::fs::rename(&first, &second)?;
    }
    if path.exists() {
        std::fs::rename(path, &first)?;
    }
    Ok(())
}

fn timestamp(now: SystemTime) -> String {
    let seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        (rest / 60) % 60,
        rest % 60
    )
}

/// Howard Hinnant's days-to-civil conversion.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_convert_from_epoch_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
    }

    #[test]
    fn a_timestamp_is_utc() {
        assert_eq!(
            timestamp(UNIX_EPOCH + std::time::Duration::from_secs(0)),
            "1970-01-01T00:00:00Z"
        );
    }

    #[test]
    fn a_path_under_app_data_loses_the_profile_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Kura").join("session.bin");
        let roots = [("APPDATA", dir.path().to_path_buf())];

        assert_eq!(display_with(&path, &roots), "%APPDATA%\\Kura\\session.bin");
    }

    #[test]
    fn a_path_outside_the_known_roots_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("elsewhere");

        assert_eq!(display_with(&path, &[]), path.display().to_string());
    }

    #[test]
    fn the_profile_itself_is_only_the_variable() {
        let dir = tempfile::tempdir().unwrap();
        let roots = [("APPDATA", dir.path().to_path_buf())];

        assert_eq!(display_with(dir.path(), &roots), "%APPDATA%");
    }

    #[test]
    fn an_error_text_loses_the_profile_name() {
        let dir = tempfile::tempdir().unwrap();
        let roots = [("LOCALAPPDATA", dir.path().to_path_buf())];
        let text = format!(
            "{} could not be written",
            dir.path().join("Kura").join("updates").display()
        );

        assert_eq!(
            redact_with(&text, &roots),
            "%LOCALAPPDATA%\\Kura\\updates could not be written"
        );
    }

    #[test]
    fn rotation_keeps_one_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kura.log");
        let first = path.with_extension("log.1");
        let second = path.with_extension("log.2");
        std::fs::write(&path, b"new").unwrap();
        std::fs::write(&first, b"old").unwrap();

        rotate(&path).unwrap();

        assert!(!path.exists());
        assert_eq!(std::fs::read(&first).unwrap(), b"new");
        assert_eq!(std::fs::read(&second).unwrap(), b"old");
    }

    #[test]
    fn an_empty_target_falls_back_to_the_source_location() {
        let record = log::Record::builder()
            .target("")
            .file(Some("src/app.rs"))
            .line(Some(93))
            .args(format_args!("RefCell already borrowed"))
            .level(log::Level::Error)
            .build();

        assert_eq!(source(&record), "src/app.rs:93");
    }

    #[test]
    fn a_named_target_is_kept() {
        let record = log::Record::builder()
            .target("kura::backend")
            .file(Some("src/backend.rs"))
            .line(Some(93))
            .args(format_args!("signed in"))
            .level(log::Level::Info)
            .build();

        assert_eq!(source(&record), "kura::backend");
    }
}
