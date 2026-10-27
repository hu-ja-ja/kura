//! Copying secrets to the Windows clipboard.
//!
//! Besides the text itself, Windows expects two registered formats that keep
//! the entry out of clipboard history and the cloud clipboard. They only apply
//! when set in the same clipboard session as the text.
//!
//! The guard lives outside the window, so closing the window to the tray does
//! not cancel the wipe.

use std::time::Duration;

use clipboard_win::{Clipboard, formats, options::NoClear, raw};
use gpui_kit::{AppContext as _, ClipboardItem, Context};
use zeroize::Zeroizing;

/// Put `text` on the clipboard and ask Windows not to keep it.
fn place(text: &str) -> bool {
    let Ok(_clipboard) = Clipboard::new_attempts(5) else {
        return false;
    };
    if raw::empty().is_err() || raw::set_string_with::<NoClear>(text, NoClear).is_err() {
        return false;
    }
    // Best effort: a clipboard manager that wins the race can still keep the
    // text. The timed wipe in the guard is the fallback.
    let zero = 0u32.to_ne_bytes();
    for name in ["CanIncludeInClipboardHistory", "CanUploadToCloudClipboard"] {
        if let Some(format) = raw::register_format(name) {
            let _ = raw::set_without_clear(format.get(), &zero);
        }
    }
    true
}

fn read() -> Option<String> {
    clipboard_win::get_clipboard::<String, _>(formats::Unicode).ok()
}

fn wipe() -> bool {
    place("")
}

/// Holds the last copied value and wipes it when its time is up or the vault
/// locks. Survives the destruction of the vault window.
#[derive(Default)]
pub struct Guard {
    copied: Option<Zeroizing<String>>,
    generation: u64,
}

impl Guard {
    pub fn copy(&mut self, value: String, timeout: Duration, cx: &mut Context<Self>) {
        if value.is_empty() {
            return;
        }
        if !place(&value) {
            cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
        }
        self.copied = Some(Zeroizing::new(value));
        self.generation += 1;
        let generation = self.generation;
        let this = cx.entity();

        cx.spawn(async move |_this, cx| {
            cx.background_executor().timer(timeout).await;
            cx.update_entity(&this, |this, cx| {
                if this.generation == generation {
                    this.clear(cx);
                }
            });
        })
        .detach();
    }

    /// Wipe now, unless something else has replaced the copied value.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        let Some(copied) = self.copied.take() else {
            return;
        };
        self.generation += 1;
        // shortcut: if the clipboard cannot be read at the deadline, leave it
        // alone rather than risk wiping something the user copied elsewhere.
        let current = read();
        if current.as_deref() == Some(copied.as_str()) && !wipe() {
            cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
        }
    }
}
