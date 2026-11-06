//! Copying secrets to the Windows clipboard.
//!
//! Besides the text itself, Windows expects two registered formats that keep
//! the entry out of clipboard history and the cloud clipboard. They only apply
//! when set in the same clipboard session as the text, so they are written
//! first: a watcher that reacts to the first change must not see a copy
//! without them.
//!
//! The guard lives outside the window, so closing the window to the tray does
//! not cancel the wipe.

use std::time::Duration;

use clipboard_win::{Clipboard, formats, options::NoClear, raw};
use gpui_kit::{AppContext as _, ClipboardItem, Context};
use zeroize::Zeroizing;

/// The formats that opt a copy out of the clipboard history and the cloud
/// clipboard.
const OPT_OUT_FORMATS: [&str; 2] = ["CanIncludeInClipboardHistory", "CanUploadToCloudClipboard"];

/// Put `text` on the clipboard with the opt-out formats already set.
///
/// A copy without the formats would land in the history and the cloud
/// clipboard, so a failure here is reported instead of falling back.
fn place(text: &str) -> bool {
    let Ok(_clipboard) = Clipboard::new_attempts(5) else {
        return false;
    };
    if raw::empty().is_err() {
        return false;
    }

    let zero = 0u32.to_ne_bytes();
    for name in OPT_OUT_FORMATS {
        let Some(format) = raw::register_format(name) else {
            return false;
        };
        if raw::set_without_clear(format.get(), &zero).is_err() {
            return false;
        }
    }

    // `NoClear` keeps the formats that were just set in the same session.
    raw::set_string_with::<NoClear>(text, NoClear).is_ok()
}

fn read() -> Option<String> {
    clipboard_win::get_clipboard::<String, _>(formats::Unicode).ok()
}

fn wipe() -> bool {
    let Ok(_clipboard) = Clipboard::new_attempts(5) else {
        return false;
    };
    raw::empty().is_ok()
}

/// Holds the last copied value and wipes it when its time is up or the vault
/// locks. Survives the destruction of the vault window.
#[derive(Default)]
pub struct Guard {
    copied: Option<Zeroizing<String>>,
    generation: u64,
}

impl Guard {
    /// Put `value` on the clipboard. False means nothing was copied.
    pub fn copy(&mut self, value: &str, timeout: Duration, cx: &mut Context<Self>) -> bool {
        if value.is_empty() || !place(value) {
            return false;
        }
        self.copied = Some(Zeroizing::new(value.to_string()));
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

        true
    }

    /// Wipe now, unless something else has replaced the copied value. A wipe
    /// that cannot run keeps the value so the next lock or quit retries it.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        let Some(copied) = self.copied.as_ref().map(|copied| copied.as_str()) else {
            return;
        };
        let still_ours = read().as_deref() == Some(copied);
        if still_ours && !wipe() {
            // The raw wipe could not run. Writing an empty string carries no
            // secret, so it is a safe last try; the value stays so a later
            // lock or quit checks again.
            cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
            return;
        }
        self.copied = None;
        self.generation += 1;
    }
}
