//! Serializes tests that change the process-wide i18n locale.

use std::sync::{Mutex, MutexGuard};

static LOCK: Mutex<()> = Mutex::new(());

/// Hold this for the whole test. The locale is global, so a test that resets
/// it must not overlap a test that renders labels.
pub fn serial() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
