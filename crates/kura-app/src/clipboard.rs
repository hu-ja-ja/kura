//! Copying secrets to the Windows clipboard.
//!
//! Besides the text itself, Windows expects two registered formats that keep
//! the entry out of clipboard history and the cloud clipboard. They only apply
//! when set in the same clipboard session as the text.

use clipboard_win::{Clipboard, formats, options::NoClear, raw};

/// Put `text` on the clipboard and ask Windows not to keep it.
pub fn copy(text: &str) -> bool {
    let Ok(_clipboard) = Clipboard::new_attempts(5) else {
        return false;
    };
    if raw::empty().is_err() || raw::set_string_with::<NoClear>(text, NoClear).is_err() {
        return false;
    }
    // Best effort: a clipboard manager that wins the race can still keep the
    // text. The 30 second wipe in the caller is the fallback.
    let zero = 0u32.to_ne_bytes();
    for name in ["CanIncludeInClipboardHistory", "CanUploadToCloudClipboard"] {
        if let Some(format) = raw::register_format(name) {
            let _ = raw::set_without_clear(format.get(), &zero);
        }
    }
    true
}

pub fn text() -> Option<String> {
    clipboard_win::get_clipboard::<String, _>(formats::Unicode).ok()
}

pub fn clear() -> bool {
    copy("")
}
