//! Keep decrypted memory out of crash dumps.
//!
//! A dump taken while the vault is open carries the decrypted items and the
//! user key, so the process leaves Windows Error Reporting altogether and asks
//! for no heap in any dump that is still written. A machine-wide `LocalDumps`
//! policy is an administrator's decision and cannot be overridden from here.

#![allow(unsafe_code)]

use windows_sys::Win32::System::Diagnostics::Debug::{
    SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
};
use windows_sys::Win32::System::ErrorReporting::{
    WER_FAULT_REPORTING_FLAG_NOHEAP, WER_FAULT_REPORTING_NO_UI, WerAddExcludedApplication,
    WerSetFlags,
};

use crate::windows::wide;

/// Drop the crash dialog and the dump for this process.
pub fn disable_dumps() {
    // No "the program stopped working" box on a fault, and no critical-error
    // box on a missing file.
    unsafe {
        SetErrorMode(SEM_NOGPFAULTERRORBOX | SEM_FAILCRITICALERRORS);
    }

    // Belt and braces for a dump that is written anyway: no heap, no UI.
    let flags = WER_FAULT_REPORTING_FLAG_NOHEAP | WER_FAULT_REPORTING_NO_UI;
    if unsafe { WerSetFlags(flags) } < 0 {
        log::warn!("the Windows Error Reporting flags could not be set");
    }

    let Some(name) = exe_name() else {
        log::warn!("the executable name is unavailable; crash dumps stay enabled");
        return;
    };
    let name = wide(&name);
    if unsafe { WerAddExcludedApplication(name.as_ptr(), 0) } < 0 {
        log::warn!("Kura could not be excluded from Windows Error Reporting");
    } else {
        // A hard crash no longer leaves a dialog or a dump, so this line is the
        // only trace that the protection is on.
        log::info!("crash reporting is off for this process");
    }
}

fn exe_name() -> Option<String> {
    let path = std::env::current_exe().ok()?;
    Some(path.file_name()?.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_executable_name_is_available() {
        assert!(exe_name().is_some_and(|name| name.ends_with(".exe")));
    }

    /// The startup call has to survive on its own: a fault here would take the
    /// app down before the vault ever opens.
    #[test]
    fn the_hardening_can_run() {
        disable_dumps();
    }
}
