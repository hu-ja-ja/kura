//! A small appender for `%LOCALAPPDATA%\Kura\logs`.
//!
//! Two rolled files are kept. The logger never records request bodies or
//! secret values; call sites log what happened, not what the vault holds.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const LIMIT: u64 = 1_000_000;

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
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(local)
            .join("Kura")
            .join("logs")
            .join("kura.log"),
    )
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
            record.target(),
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
}
