//! The window: login, two-factor and the vault list.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{
    Input, InputContentType, InputEvent, InputState, OtpEvent, OtpInput, OtpState, Textarea,
    TextareaState,
};
use gpui_kit::component::list::{List, ListState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use kura_core::{
    CoreError, ItemKind, TwoFactorChallenge, TwoFactorMethod, VaultFilter, VaultItemDetail,
    VaultSnapshot,
};
use rust_i18n::t;

use crate::backend::{Backend, Command, Event};
use crate::clipboard::Guard as Clipboard;
use crate::controller;
use crate::detail::{DetailRow, otp_digits, rows as detail_rows, tick_totp};
use crate::list::{VaultDelegate, kind_label};
use crate::settings::{Settings, ThemeChoice};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Login,
    TwoFactor,
    Vault,
}

/// A problem shown in the UI.
enum Problem {
    MissingInput,
    Core(CoreError),
}

const TYPE_FILTERS: &[ItemKind] = &[
    ItemKind::Login,
    ItemKind::Card,
    ItemKind::Identity,
    ItemKind::SecureNote,
    ItemKind::SshKey,
    ItemKind::Other,
];

const MASK: &str = "••••••••••••";
const NOTICE_TIME: Duration = Duration::from_secs(2);
const TOTP_RETRY_SECS: u32 = 5;

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// One folder in the filter menu. Built before the menu so the entries can
/// borrow the root's listeners.
struct FolderChoice {
    label: String,
    checked: bool,
    on_click: ClickHandler,
}

/// One entry in a settings choice row.
struct SettingsChoice {
    label: String,
    active: bool,
    on_click: ClickHandler,
}

pub struct Root {
    backend: Backend,
    settings: Settings,
    platform: kura_platform::Platform,
    clipboard: Entity<Clipboard>,
    page: Page,
    locale: String,
    window: AnyWindowHandle,
    server_url: Entity<InputState>,
    email: Entity<InputState>,
    password: Entity<InputState>,
    search: Entity<InputState>,
    otp: Entity<OtpState>,
    notes: Entity<TextareaState>,
    list: Entity<ListState<VaultDelegate>>,
    modal_focus: FocusHandle,
    settings_focus: FocusHandle,
    settings_open: bool,
    snapshot: Arc<VaultSnapshot>,
    filter: VaultFilter,
    show_server: bool,
    busy: bool,
    problem: Option<Problem>,
    sync_problem: Option<Problem>,
    detail: Option<VaultItemDetail>,
    detail_problem: Option<Problem>,
    detail_loading: bool,
    selected_id: Option<String>,
    revealed: HashSet<usize>,
    detail_generation: u64,
    notice: Option<String>,
    notice_error: bool,
    notice_generation: u64,
    challenge: Option<TwoFactorChallenge>,
    method: Option<TwoFactorMethod>,
    email_code_sent: bool,
    _subscriptions: Vec<Subscription>,
}

impl Root {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        backend: Backend,
        settings: Settings,
        clipboard: Entity<Clipboard>,
        platform: kura_platform::Platform,
        saved_email: Option<String>,
    ) -> Self {
        let locale = settings.locale();
        rust_i18n::set_locale(&locale);
        Theme::change(theme_mode(settings.theme, window), Some(window), cx);
        if settings.screenshot_protection
            && let Some(hwnd) = controller::window_hwnd(window)
            && let Err(err) = kura_platform::set_capture_protection(hwnd, true)
        {
            log::warn!("could not turn on capture protection: {err}");
        }
        let unlocked = backend.is_unlocked();
        let show_server = settings.server_url.is_some();

        let server_url =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://vault.example.com"));
        if let Some(url) = settings.server_url.clone() {
            server_url.update(cx, |input, cx| input.set_value(url, window, cx));
        }
        let email = cx.new(|cx| InputState::new(window, cx));
        if let Some(saved) = saved_email.clone() {
            email.update(cx, |input, cx| input.set_value(saved, window, cx));
        }
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("vault.search")));
        let otp = cx.new(|cx| OtpState::new(6, window, cx));
        let notes = cx.new(|cx| TextareaState::new(window, cx));

        let list = cx.new(|cx| {
            ListState::new(
                VaultDelegate::new(Arc::new(VaultSnapshot::default())),
                window,
                cx,
            )
        });
        let modal_focus = cx.focus_handle();
        let settings_focus = cx.focus_handle();
        let root = cx.entity().downgrade();
        list.update(cx, |list, _| {
            list.delegate_mut().set_on_open(move |id, window, cx| {
                let _ = root.update(cx, |root, cx| {
                    root.open_item(id.to_string(), window, cx);
                });
            });
        });
        let copy_root = cx.entity().downgrade();
        let password_root = copy_root.clone();
        list.update(cx, |list, _| {
            list.delegate_mut()
                .set_on_copy_text(move |value, _window, cx| {
                    let _ = copy_root.update(cx, |root, cx| {
                        root.copy_value(value.to_string(), cx);
                    });
                });
            list.delegate_mut()
                .set_on_copy_password(move |id, _window, cx| {
                    let _ = password_root.update(cx, |root, _| {
                        root.copy_item_password(id.to_string());
                    });
                });
        });

        let mut subscriptions = vec![];

        subscriptions.push(cx.subscribe_in(
            &email,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_login(cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &password,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_login(cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &server_url,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_login(cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &search,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let query = this.search.read(cx).value().to_string();
                    this.list.update(cx, |list, cx| {
                        list.set_query(&query, window, cx);
                    });
                    // The clear button appears and disappears with the query.
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    let selected = this.list.read(cx).delegate().selected_item_id();
                    this.list.update(cx, |list, cx| list.focus(window, cx));
                    if let Some(id) = selected {
                        this.open_item(id, window, cx);
                    }
                }
                _ => {}
            },
        ));
        subscriptions.push(
            cx.subscribe_in(&otp, window, |this, _, event: &OtpEvent, _, cx| {
                if matches!(event, OtpEvent::Complete) {
                    this.submit_two_factor(cx);
                }
            }),
        );

        subscriptions.push(
            cx.observe_keystrokes(|this, event: &KeystrokeEvent, window, cx| {
                let keystroke = &event.keystroke;

                // Locking works from every screen, including the settings dialog.
                if keystroke.key == "l" && keystroke.modifiers.control {
                    cx.stop_propagation();
                    this.lock(window, cx);
                    return;
                }

                if this.settings_open {
                    if keystroke.key == "escape" {
                        cx.stop_propagation();
                        this.close_settings(cx);
                    }
                    return;
                }

                if keystroke.key == "f" && keystroke.modifiers.control {
                    // Swallow the key so the letter does not land in the search field.
                    cx.stop_propagation();
                    if this.selected_id.is_none() {
                        this.focus_search(window, cx);
                    }
                    return;
                }

                // The OTP input has no paste support of its own.
                if this.page == Page::TwoFactor
                    && keystroke.key == "v"
                    && keystroke.modifiers.control
                    && this.otp.read(cx).focus_handle(cx).is_focused(window)
                {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        let length = this.otp.read(cx).len();
                        let digits = otp_digits(&text, length);
                        if !digits.is_empty() {
                            window.prevent_default();
                            cx.stop_propagation();
                            let complete = digits.chars().count() == length;
                            this.otp
                                .update(cx, |otp, cx| otp.set_value(digits, window, cx));
                            if complete {
                                this.submit_two_factor(cx);
                            }
                        }
                    }
                    return;
                }

                if this.page != Page::Vault {
                    return;
                }
                if keystroke.key == "escape" && this.selected_id.is_some() {
                    cx.stop_propagation();
                    this.close_detail(window, cx);
                    return;
                }

                if !this.search.read(cx).focus_handle(cx).is_focused(window) {
                    return;
                }

                match keystroke.key.as_str() {
                    "down" => {
                        cx.stop_propagation();
                        this.list.update(cx, |list, cx| list.focus(window, cx));
                    }
                    "escape" => {
                        cx.stop_propagation();
                        if !this.search.read(cx).value().is_empty() {
                            this.search
                                .update(cx, |search, cx| search.set_value("", window, cx));
                        } else {
                            this.list.update(cx, |list, cx| list.focus(window, cx));
                        }
                    }
                    _ => {}
                }
            }),
        );

        // A system theme follows the OS while the window is alive.
        subscriptions.push(cx.observe_window_appearance(window, |this, window, cx| {
            if this.settings.theme == ThemeChoice::System {
                Theme::change(theme_mode(ThemeChoice::System, window), Some(window), cx);
            }
        }));

        // Closing the window is a trip to the tray, never an exit.
        let closing = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            if controller::quitting() {
                return true;
            }
            if let Some(root) = closing.upgrade() {
                root.update(cx, |root, cx| root.close_to_tray(window, cx));
            }
            false
        });

        if unlocked {
            list.update(cx, |list, cx| {
                list.delegate_mut().set_loading(true);
                cx.notify();
            });
            backend.send(Command::Sync);
        }

        // Put the cursor in the field the user still has to fill once the window
        // has drawn: the search when the vault is already open, otherwise the
        // password for a saved login, otherwise the email.
        let has_saved = saved_email.is_some();
        cx.defer_in(window, move |this, window, cx| {
            let input = if unlocked {
                this.search.clone()
            } else if has_saved {
                this.password.clone()
            } else {
                this.email.clone()
            };
            input.update(cx, |input, cx| {
                input.focus_handle(cx).focus(window, cx);
            });
        });

        Self {
            backend,
            settings,
            platform,
            clipboard,
            page: if unlocked { Page::Vault } else { Page::Login },
            locale,
            window: window.window_handle(),
            server_url,
            email,
            password,
            search,
            otp,
            notes,
            list,
            modal_focus,
            settings_focus,
            settings_open: false,
            snapshot: Arc::new(VaultSnapshot::default()),
            filter: VaultFilter::All,
            show_server,
            busy: unlocked,
            problem: None,
            sync_problem: None,
            detail: None,
            detail_problem: None,
            detail_loading: false,
            selected_id: None,
            revealed: HashSet::new(),
            detail_generation: 0,
            notice: None,
            notice_error: false,
            notice_generation: 0,
            challenge: None,
            method: None,
            email_code_sent: false,
            _subscriptions: subscriptions,
        }
    }

    // ----------------------------------------------------------------- events

    pub(crate) fn handle_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::LoggedIn => {
                self.busy = false;
                self.page = Page::Vault;
                self.settings_open = false;
                self.problem = None;
                self.sync_problem = None;
                self.challenge = None;
                self.method = None;
                self.clear_password(None, cx);
                // The server URL was accepted, so keep it for the next start.
                self.save_settings(cx);
                self.list.update(cx, |list, cx| {
                    list.delegate_mut().set_loading(true);
                    cx.notify();
                });
            }
            Event::Locked => {
                self.reset_to_login(None, cx);
            }
            Event::TwoFactorRequired(challenge) => {
                self.busy = false;
                self.settings_open = false;
                self.method = challenge.methods.first().cloned();
                self.challenge = Some(challenge);
                self.page = Page::TwoFactor;
                self.problem = None;
                self.email_code_sent = false;
                self.clear_password(None, cx);
                let otp = self.otp.clone();
                self.in_window(None, cx, move |window, cx| {
                    otp.update(cx, |state, cx| {
                        state.focus_handle(cx).focus(window, cx);
                    });
                });
            }
            Event::Snapshot(snapshot) => {
                // A late answer from the session that was just locked.
                if self.page != Page::Vault {
                    return;
                }
                self.busy = false;
                self.sync_problem = None;

                // A folder filter whose folder disappeared falls back to all.
                let folder_gone = match &self.filter {
                    VaultFilter::Folder(id) => {
                        !snapshot.folders.iter().any(|folder| &folder.id == id)
                    }
                    _ => false,
                };
                if folder_gone {
                    self.filter = VaultFilter::All;
                    self.list.update(cx, |list, cx| {
                        list.delegate_mut().set_filter(VaultFilter::All);
                        cx.notify();
                    });
                }

                self.snapshot = snapshot.clone();
                self.list.update(cx, |list, cx| {
                    list.delegate_mut().set_snapshot(snapshot);
                    list.delegate_mut().set_loading(false);
                    cx.notify();
                });

                // Keep the open detail in step with the new snapshot.
                if let Some(id) = self.selected_id.clone() {
                    if self.snapshot.items.iter().any(|item| item.id == id) {
                        let row = self.list.read(cx).delegate().row_of(&id);
                        let list = self.list.clone();
                        self.in_window(None, cx, move |window, cx| {
                            list.update(cx, |list, cx| {
                                list.set_selected_index(row.map(IndexPath::new), window, cx);
                            });
                        });
                        if !self.detail_loading {
                            self.request_detail(id);
                        }
                    } else {
                        self.forget_detail(None, cx);
                    }
                }
            }
            Event::EmailCodeSent => {
                self.busy = false;
                self.email_code_sent = true;
            }
            Event::Detail(detail) => {
                if self.selected_id.as_deref() == Some(detail.id.as_str()) {
                    self.detail_loading = false;
                    self.detail_problem = None;
                    self.detail = Some(*detail);
                    let notes = self
                        .detail
                        .as_ref()
                        .and_then(|detail| detail.notes.clone())
                        .unwrap_or_default();
                    let notes_state = self.notes.clone();
                    self.in_window(None, cx, move |window, cx| {
                        notes_state.update(cx, |state, cx| state.set_value(notes, window, cx));
                    });
                    self.start_totp_ticks(cx);
                }
            }
            Event::Totp { id, totp } => {
                if self.selected_id.as_deref() == Some(id.as_str())
                    && let Some(detail) = self.detail.as_mut()
                    && let Some(current) = detail.totp.as_mut()
                {
                    *current = totp;
                    self.detail_problem = None;
                }
            }
            Event::DetailFailed { id, err } => {
                if self.selected_id.as_deref() == Some(id.as_str()) {
                    self.detail_loading = false;
                    self.detail_problem = Some(Problem::Core(err));
                }
            }
            Event::PasswordCopied(result) => {
                if self.page == Page::Vault {
                    match result {
                        Ok(password) => self.copy_value(password, cx),
                        Err(err) => {
                            let problem = Problem::Core(err);
                            self.show_notice(problem_text(Some(&problem)), true, cx);
                        }
                    }
                }
            }
            Event::TotpFailed { id, err } => {
                if self.selected_id.as_deref() == Some(id.as_str()) {
                    let no_totp = err == CoreError::NoTotp;
                    self.detail_problem = Some(Problem::Core(err));
                    if no_totp {
                        if let Some(detail) = self.detail.as_mut() {
                            detail.totp = None;
                        }
                    } else if let Some(totp) =
                        self.detail.as_mut().and_then(|detail| detail.totp.as_mut())
                    {
                        // Try again shortly instead of freezing on a stale code.
                        totp.remaining = TOTP_RETRY_SECS;
                    }
                }
            }
            Event::Failed(err) => {
                let was_busy = self.busy;
                self.busy = false;
                let problem = Problem::Core(err);
                if self.page == Page::Vault {
                    self.sync_problem = Some(problem);
                    self.list.update(cx, |list, cx| {
                        list.delegate_mut().set_loading(false);
                        cx.notify();
                    });
                } else if was_busy {
                    // A late failure from a session that was already locked
                    // must not appear as a login error.
                    self.problem = Some(problem);
                    if self.page == Page::TwoFactor {
                        // A rejected code is worth typing again from scratch.
                        self.clear_code(None, cx);
                    }
                }
            }
        }
        cx.notify();
    }

    // ---------------------------------------------------------------- actions

    fn submit_login(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let email = self.email.read(cx).value().trim().to_string();
        let password = self.password.read(cx).value().to_string();
        let server_url = self.server_url.read(cx).value().trim().to_string();
        if email.is_empty() || password.is_empty() {
            self.problem = Some(Problem::MissingInput);
            return;
        }

        self.problem = None;
        self.busy = true;
        self.settings.server_url = (!server_url.is_empty()).then(|| server_url.clone());
        self.backend.send(Command::Login {
            server_url,
            email,
            password,
        });
    }

    fn submit_two_factor(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let code = self.otp.read(cx).value().to_string();
        let Some(method) = self.method.clone() else {
            return;
        };
        if code.trim().is_empty() {
            return;
        }

        self.problem = None;
        self.busy = true;
        self.backend.send(Command::SubmitTwoFactor { method, code });
    }

    fn send_email_code(&mut self) {
        self.busy = true;
        self.email_code_sent = false;
        self.backend.send(Command::SendEmailCode);
    }

    fn sync(&mut self) {
        self.busy = true;
        self.backend.send(Command::Sync);
    }

    fn open_item(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_id.as_deref() == Some(id.as_str())
            && (self.detail.is_some() || self.detail_loading)
        {
            return;
        }
        self.selected_id = Some(id.clone());
        self.detail = None;
        self.detail_problem = None;
        self.notice = None;
        self.request_detail(id);
        let focus = self.modal_focus.clone();
        window.defer(cx, move |window, cx| focus.focus(window, cx));
        cx.notify();
    }

    fn request_detail(&mut self, id: String) {
        self.revealed.clear();
        self.detail_loading = true;
        self.detail_problem = None;
        self.detail_generation += 1;
        self.backend.send(Command::Detail { id });
    }

    /// Drop the decrypted item and everything derived from it.
    fn forget_detail(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        self.detail = None;
        self.detail_problem = None;
        self.detail_loading = false;
        self.selected_id = None;
        self.revealed.clear();
        self.detail_generation += 1;
        self.notice = None;
        self.notice_error = false;
        let notes = self.notes.clone();
        self.in_window(window, cx, move |window, cx| {
            notes.update(cx, |state, cx| state.set_value("", window, cx));
        });
    }

    fn close_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.forget_detail(Some(window), cx);
        self.list.update(cx, |list, cx| {
            list.set_selected_index(None, window, cx);
            list.focus(window, cx);
        });
        cx.notify();
    }

    /// Manual lock. Dropping the session drops the keys; the UI drops the
    /// decrypted data it was holding.
    fn lock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reset_to_login(Some(window), cx);
        self.backend.lock();
    }

    /// Return to the locked login screen. Used by the manual lock and by the
    /// worker's answer to a tray or automatic lock.
    fn reset_to_login(&mut self, mut window: Option<&mut Window>, cx: &mut Context<Self>) {
        self.page = Page::Login;
        self.settings_open = false;
        self.busy = false;
        self.problem = None;
        self.sync_problem = None;
        self.challenge = None;
        self.method = None;
        self.filter = VaultFilter::All;
        self.snapshot = Arc::new(VaultSnapshot::default());
        self.forget_detail(window.as_deref_mut(), cx);
        self.clipboard
            .update(cx, |clipboard, cx| clipboard.clear(cx));
        self.clear_password(window.as_deref_mut(), cx);
        self.clear_code(window.as_deref_mut(), cx);

        let list = self.list.clone();
        let search = self.search.clone();
        let email = self.email.clone();
        let empty = Arc::new(VaultSnapshot::default());
        self.in_window(window, cx, move |window, cx| {
            list.update(cx, |list, cx| {
                list.delegate_mut().set_snapshot(empty);
                list.delegate_mut().set_filter(VaultFilter::All);
                list.delegate_mut().set_loading(false);
                list.set_query("", window, cx);
                list.set_selected_index(None, window, cx);
            });
            search.update(cx, |search, cx| search.set_value("", window, cx));
            email.update(cx, |email, cx| {
                email.focus_handle(cx).focus(window, cx);
            });
        });
        cx.notify();
    }

    /// Put a value on the clipboard and wipe it again after the configured
    /// delay, unless something else replaces it by then.
    fn copy_value(&mut self, value: String, cx: &mut Context<Self>) {
        if value.is_empty() {
            return;
        }
        let timeout = self.settings.clipboard_clear();
        let seconds = self.settings.clipboard_seconds();
        self.clipboard
            .update(cx, |clipboard, cx| clipboard.copy(value, timeout, cx));
        self.show_notice(
            t!("detail.copied", seconds = seconds).to_string(),
            false,
            cx,
        );
    }

    fn show_notice(&mut self, text: String, error: bool, cx: &mut Context<Self>) {
        self.notice = Some(text);
        self.notice_error = error;
        self.notice_generation += 1;
        let notice_generation = self.notice_generation;

        let this = cx.entity();
        cx.spawn(async move |_this, cx| {
            cx.background_executor().timer(NOTICE_TIME).await;
            cx.update_entity(&this, |this, cx| {
                if this.notice_generation == notice_generation {
                    this.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();

        cx.notify();
    }

    fn copy_item_password(&mut self, id: String) {
        self.backend.send(Command::CopyPassword { id });
    }

    fn open_detail_url(&mut self, url: String, cx: &mut Context<Self>) {
        if let Some(url) = kura_core::openable_url(&url) {
            cx.open_url(&url);
        }
    }

    /// Keep the TOTP countdown moving and ask for a fresh code at zero.
    fn start_totp_ticks(&self, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail else {
            return;
        };
        if detail.totp.is_none() {
            return;
        }
        let generation = self.detail_generation;
        let id = detail.id.clone();
        let this = cx.entity();

        cx.spawn(async move |_this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep_going = cx.update_entity(&this, |this, cx| {
                    if this.detail_generation != generation || this.page != Page::Vault {
                        return false;
                    }
                    let Some(detail) = this.detail.as_mut() else {
                        return false;
                    };
                    let Some(totp) = detail.totp.as_mut() else {
                        return false;
                    };
                    let before = totp.remaining;
                    if tick_totp(totp) {
                        this.backend.send(Command::Totp { id: id.clone() });
                    }
                    if totp.remaining != before {
                        cx.notify();
                    }
                    true
                });
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    fn back_to_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.backend.cancel();
        self.page = Page::Login;
        self.challenge = None;
        self.method = None;
        self.problem = None;
        self.busy = false;
        self.clear_code(Some(window), cx);
    }

    fn set_language(&mut self, locale: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.locale == locale {
            return;
        }
        self.locale = locale.clone();
        rust_i18n::set_locale(&self.locale);
        self.settings.language = Some(locale);
        self.platform.set_labels(controller::labels());

        let placeholder = t!("vault.search").to_string();
        self.search.update(cx, |search, cx| {
            search.set_placeholder(placeholder, window, cx)
        });
        self.save_settings(cx);
        cx.notify();
    }

    fn set_filter(&mut self, filter: VaultFilter, window: &mut Window, cx: &mut Context<Self>) {
        if self.filter == filter {
            return;
        }
        self.filter = filter.clone();
        let query = self.search.read(cx).value().to_string();
        self.list.update(cx, |list, cx| {
            list.delegate_mut().set_filter(filter);
            list.set_query(&query, window, cx);
        });
        cx.notify();
    }

    fn clear_password(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        let password = self.password.clone();
        self.in_window(window, cx, move |window, cx| {
            password.update(cx, |input, cx| input.set_value("", window, cx));
        });
    }

    fn clear_code(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        let otp = self.otp.clone();
        self.in_window(window, cx, move |window, cx| {
            otp.update(cx, |state, cx| state.set_value("", window, cx));
        });
    }

    /// Run `f` against this window. A handler that is already inside a window
    /// event has to pass that window: the app cannot lend out the same window
    /// while it is being updated, and refuses without telling anyone.
    fn in_window<R>(
        &self,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Window, &mut App) -> R,
    ) -> Option<R> {
        match window {
            Some(window) => Some(f(window, cx)),
            None => cx
                .update_window(self.window, move |_, window, cx| f(window, cx))
                .ok(),
        }
    }

    /// Ctrl+F puts the cursor in the search field.
    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Page::Vault {
            return;
        }
        self.search.update(cx, |search, cx| {
            search.focus_handle(cx).focus(window, cx);
        });
    }

    /// The window was shown from the tray. Fetch a fresh list as well.
    pub(crate) fn on_shown(&mut self, cx: &mut Context<Self>) {
        if self.page == Page::Vault {
            self.sync();
            cx.notify();
        }
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            return;
        }
        self.settings_open = true;
        let focus = self.settings_focus.clone();
        window.defer(cx, move |window, cx| focus.focus(window, cx));
        cx.notify();
    }

    fn close_settings(&mut self, cx: &mut Context<Self>) {
        if !self.settings_open {
            return;
        }
        self.settings_open = false;
        self.commit_server_url(cx);
        self.save_settings(cx);
        cx.notify();
    }

    fn commit_server_url(&mut self, cx: &Context<Self>) {
        let value = self.server_url.read(cx).value().trim().to_string();
        self.settings.server_url = (!value.is_empty()).then_some(value);
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        if let Err(err) = self.settings.save() {
            log::warn!("could not save the settings: {err}");
            self.show_notice(t!("settings.save_failed").to_string(), true, cx);
        }
    }

    fn set_theme(&mut self, choice: ThemeChoice, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.theme = choice;
        Theme::change(theme_mode(choice, window), Some(window), cx);
        self.save_settings(cx);
        cx.notify();
    }

    fn set_auto_lock(&mut self, minutes: u16, cx: &mut Context<Self>) {
        self.settings.auto_lock_minutes = minutes;
        self.platform
            .set_auto_lock(self.settings.auto_lock().as_secs());
        self.save_settings(cx);
        cx.notify();
    }

    fn set_clipboard_seconds(&mut self, seconds: u16, cx: &mut Context<Self>) {
        self.settings.clipboard_seconds = seconds;
        self.save_settings(cx);
        cx.notify();
    }

    fn set_autostart(&mut self, enabled: bool, cx: &mut Context<Self>) {
        match kura_platform::set_autostart(enabled) {
            Ok(()) => {
                self.settings.autostart = enabled;
                self.save_settings(cx);
            }
            Err(err) => {
                log::warn!("could not change the startup entry: {err}");
                self.show_notice(t!("settings.autostart_failed").to_string(), true, cx);
            }
        }
        cx.notify();
    }

    fn set_start_in_tray(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.start_in_tray = enabled;
        self.save_settings(cx);
        cx.notify();
    }

    fn set_destroy_on_hide(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.destroy_on_hide = enabled;
        self.save_settings(cx);
        cx.notify();
    }

    fn set_capture_protection(
        &mut self,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(hwnd) = controller::window_hwnd(window) else {
            self.show_notice(t!("settings.capture_failed").to_string(), true, cx);
            return;
        };
        match kura_platform::set_capture_protection(hwnd, enabled) {
            Ok(()) => {
                self.settings.screenshot_protection = enabled;
                self.save_settings(cx);
            }
            Err(err) => {
                log::warn!("could not change capture protection: {err}");
                self.show_notice(t!("settings.capture_failed").to_string(), true, cx);
            }
        }
        cx.notify();
    }

    /// Save the window position, then destroy or hide the window. Called from
    /// the native close button and from Alt+F4.
    fn close_to_tray(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_server_url(cx);
        self.settings.window = Some(crate::settings::placement(window));
        self.save_settings(cx);
        if self.page != Page::Vault {
            // Do not keep a half-finished login, and its master password, alive
            // while the window sits in the tray.
            self.backend.cancel();
        }
        if self.settings.destroy_on_hide {
            window.defer(cx, |window, _| window.remove_window());
        } else if let Some(hwnd) = controller::window_hwnd(window) {
            kura_platform::hide_window(hwnd);
        }
    }

    // ------------------------------------------------------------------ views

    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.snapshot.items.len();
        h_flex()
            .items_center()
            .justify_between()
            .h(px(44.))
            .px_4()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .child(div().text_sm().font_weight(FontWeight::BOLD).child("Kura"))
                    .when(self.page == Page::Vault, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(t!("vault.item_count", count = count)),
                        )
                    }),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .when(self.page == Page::Vault, |this| {
                        this.child(
                            Button::new("sync")
                                .ghost()
                                .compact()
                                .icon(IconName::RefreshCw)
                                .label(t!("vault.sync"))
                                .loading(self.busy)
                                .on_click(cx.listener(|this, _, _, _| this.sync())),
                        )
                        .child(
                            Button::new("lock")
                                .ghost()
                                .compact()
                                .icon(AssetIcon::Lock)
                                .label(t!("vault.lock"))
                                .tooltip(t!("vault.lock_hint"))
                                .on_click(cx.listener(|this, _, window, cx| this.lock(window, cx))),
                        )
                    })
                    .child(
                        Button::new("settings")
                            .ghost()
                            .compact()
                            .icon(IconName::Settings)
                            .tooltip(t!("settings.open"))
                            .accessibility_label(t!("settings.open"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_settings(window, cx)),
                            ),
                    ),
            )
    }

    fn render_login(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;

        v_flex().flex_1().items_center().justify_center().child(
            v_flex()
                .w(px(380.))
                .gap_3()
                .p_6()
                .rounded_lg()
                .border_1()
                .border_color(cx.theme().border)
                .child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(FontWeight::BOLD)
                                        .child(t!("login.title")),
                                )
                                .child(
                                    Button::new("server-toggle")
                                        .text()
                                        .small()
                                        .icon(if self.show_server {
                                            IconName::ChevronUp
                                        } else {
                                            IconName::ChevronDown
                                        })
                                        .label(t!("login.self_hosted"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.show_server = !this.show_server;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child(t!("login.subtitle")),
                        ),
                )
                .when(self.show_server, |this| {
                    this.child(field(
                        t!("login.server_url").to_string(),
                        Input::new(&self.server_url),
                        muted,
                    ))
                })
                .child(field(
                    t!("login.email").to_string(),
                    Input::new(&self.email),
                    muted,
                ))
                .child(field(
                    t!("login.password").to_string(),
                    {
                        // The toggle gpui-component draws is fixed at 12px, so the app
                        // draws its own at the same size as its other icon buttons.
                        let masked = self.password.read(cx).presentation().is_masked();
                        let (icon, label) = if masked {
                            (IconName::Eye, t!("login.show_password"))
                        } else {
                            (IconName::EyeOff, t!("login.hide_password"))
                        };
                        Input::new(&self.password)
                            .content_type(InputContentType::Password)
                            .suffix(
                                Button::new("toggle-mask")
                                    .ghost()
                                    .icon(icon)
                                    .tooltip(label.clone())
                                    .accessibility_label(label)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.password.update(cx, |state, cx| {
                                            state.toggle_masked(window, cx);
                                        });
                                        cx.notify();
                                    })),
                            )
                    },
                    muted,
                ))
                .child(
                    div()
                        .min_h(px(20.))
                        .text_sm()
                        .text_color(danger)
                        .child(problem_text(self.problem.as_ref())),
                )
                .child(
                    Button::new("sign-in")
                        .primary()
                        .w_full()
                        .label(t!("login.submit"))
                        .loading(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| this.submit_login(cx))),
                ),
        )
    }

    fn render_two_factor(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let methods = self
            .challenge
            .as_ref()
            .map(|challenge| challenge.methods.clone())
            .unwrap_or_default();
        let address = methods
            .iter()
            .find_map(|method| match method {
                TwoFactorMethod::Email { address } => Some(address.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let email_selected = matches!(self.method, Some(TwoFactorMethod::Email { .. }));

        v_flex().flex_1().items_center().justify_center().child(
            v_flex()
                .w(px(380.))
                .gap_4()
                .p_6()
                .rounded_lg()
                .border_1()
                .border_color(cx.theme().border)
                .child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::BOLD)
                                .child(t!("two_factor.title")),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child(t!("two_factor.description")),
                        ),
                )
                .when(methods.len() > 1, |this| {
                    let address = address.clone();
                    this.child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("method-authenticator")
                                    .with_variant(if email_selected {
                                        ButtonVariant::Secondary
                                    } else {
                                        ButtonVariant::Primary
                                    })
                                    .label(t!("two_factor.method.authenticator"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.method = Some(TwoFactorMethod::Authenticator);
                                        this.problem = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("method-email")
                                    .with_variant(if email_selected {
                                        ButtonVariant::Primary
                                    } else {
                                        ButtonVariant::Secondary
                                    })
                                    .label(t!("two_factor.method.email"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.method = Some(TwoFactorMethod::Email {
                                            address: address.clone(),
                                        });
                                        this.problem = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                })
                .when(email_selected && !address.is_empty(), |this| {
                    this.child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(muted)
                                    .child(t!("two_factor.sent_to", address = address.clone())),
                            )
                            .child(
                                Button::new("send-code")
                                    .secondary()
                                    .label(t!("two_factor.send_email"))
                                    .loading(self.busy)
                                    .on_click(cx.listener(|this, _, _, _| this.send_email_code())),
                            )
                            .when(self.email_code_sent, |this| {
                                this.child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().success)
                                        .child(t!("two_factor.email_sent")),
                                )
                            }),
                    )
                })
                .child(
                    h_flex()
                        .justify_center()
                        .child(OtpInput::new(&self.otp).groups(1)),
                )
                .child(
                    div()
                        .min_h(px(20.))
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(problem_text(self.problem.as_ref())),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("back")
                                .secondary()
                                .label(t!("common.back"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.back_to_login(window, cx)
                                })),
                        )
                        .child(
                            Button::new("verify")
                                .primary()
                                .label(t!("two_factor.verify"))
                                .loading(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| this.submit_two_factor(cx))),
                        ),
                ),
        )
    }

    fn render_search(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_query = !self.search.read(cx).value().is_empty();
        h_flex()
            .items_center()
            .gap_3()
            .px_4()
            .py_2()
            .child(
                Input::new(&self.search)
                    .prefix(Icon::new(IconName::Search).small())
                    .when(has_query, |this| {
                        this.suffix(
                            Button::new("clear-search")
                                .ghost()
                                .icon(AssetIcon::X)
                                .tooltip(t!("vault.clear_search"))
                                .accessibility_label(t!("vault.clear_search"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.search.update(cx, |search, cx| {
                                        search.set_value("", window, cx);
                                    });
                                    this.focus_search(window, cx);
                                })),
                        )
                    })
                    .flex_1(),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Ctrl+F"),
            )
    }

    fn render_filters(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.filter.clone();
        let folders = self.snapshot.folders.clone();

        let mut type_chips = Vec::new();
        for kind in TYPE_FILTERS {
            let kind = *kind;
            type_chips.push(filter_button(
                kind_id(kind),
                kind_label(kind),
                None,
                filter == VaultFilter::Kind(kind),
                VaultFilter::Kind(kind),
                cx,
            ));
        }

        let active_folder = match &filter {
            VaultFilter::Folder(id) => folders
                .iter()
                .find(|folder| &folder.id == id)
                .map(|folder| folder.name.clone()),
            _ => None,
        };
        let folder_label = active_folder.unwrap_or_else(|| t!("filter.folder").to_string());
        let has_folders = !folders.is_empty();

        let mut choices = Vec::new();
        if has_folders {
            choices.push(FolderChoice {
                label: t!("filter.all_folders").to_string(),
                checked: !matches!(filter, VaultFilter::Folder(_)),
                on_click: Rc::new(cx.listener(|this, _, window, cx| {
                    this.set_filter(VaultFilter::All, window, cx);
                })),
            });
            for folder in folders {
                let id = folder.id.clone();
                let active = filter == VaultFilter::Folder(id.clone());
                choices.push(FolderChoice {
                    label: folder.name,
                    checked: active,
                    on_click: Rc::new(cx.listener(move |this, _, window, cx| {
                        this.set_filter(VaultFilter::Folder(id.clone()), window, cx);
                    })),
                });
            }
        }
        let choices = Rc::new(choices);

        let folder_button = Button::new("filter-folder")
            .ghost()
            .compact()
            .icon(IconName::Folder)
            .label(folder_label)
            .dropdown_caret(true)
            .selected(matches!(filter, VaultFilter::Folder(_)))
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu.min_w(px(220.)).max_h(px(320.)).scrollable(true);
                for choice in choices.iter() {
                    let on_click = choice.on_click.clone();
                    menu = menu.item(
                        PopupMenuItem::new(choice.label.clone())
                            .icon(IconName::Folder)
                            .checked(choice.checked)
                            .on_click(move |event, window, cx| on_click(event, window, cx)),
                    );
                }
                menu
            });

        h_flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(filter_button(
                "filter-all",
                t!("filter.all").to_string(),
                None,
                filter == VaultFilter::All,
                VaultFilter::All,
                cx,
            ))
            .child(filter_button(
                "filter-favorites",
                t!("filter.favorites").to_string(),
                Some(IconName::Star),
                filter == VaultFilter::Favorites,
                VaultFilter::Favorites,
                cx,
            ))
            .child(divider(cx))
            .children(type_chips)
            .when(has_folders, |this| {
                this.child(divider(cx)).child(folder_button)
            })
    }

    fn render_vault(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let failures = self.snapshot.decrypt_failures;
        let has_sync_error = self.sync_problem.is_some();
        let sync_error = problem_text(self.sync_problem.as_ref());
        let border = cx.theme().border;

        v_flex()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(self.render_search(cx))
            .child(self.render_filters(cx))
            .when(failures > 0, |this| {
                this.child(
                    div()
                        .px_4()
                        .py_2()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child(t!("vault.decrypt_failures", count = failures)),
                )
            })
            .when(has_sync_error, |this| {
                this.child(
                    h_flex()
                        .items_center()
                        .gap_3()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(border)
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(cx.theme().danger)
                                .child(sync_error),
                        )
                        .child(
                            Button::new("retry-sync")
                                .secondary()
                                .compact()
                                .label(t!("common.retry"))
                                .on_click(cx.listener(|this, _, _, _| this.sync())),
                        ),
                )
            })
            .child(List::new(&self.list).flex_1())
    }

    fn render_detail_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let danger = cx.theme().danger;
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let max_height = window.viewport_size().height - px(64.);
        let card_height = (window.viewport_size().height * 0.8).min(max_height);

        let data = self.detail.as_ref().map(|detail| {
            let folder = detail.folder_id.as_ref().and_then(|id| {
                self.snapshot
                    .folders
                    .iter()
                    .find(|folder| &folder.id == id)
                    .map(|folder| folder.name.clone())
            });
            (
                detail.name.clone(),
                detail.kind,
                detail.favorite,
                folder,
                detail_rows(detail),
            )
        });
        let detail_problem = self
            .detail_problem
            .as_ref()
            .map(|problem| problem_text(Some(problem)));
        let loading = self.detail_loading;

        let overlay = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .p_6()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.35))
            .occlude()
            .track_focus(&self.modal_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.close_detail(window, cx);
                }),
            );

        let card = v_flex()
            .w(px(560.))
            .max_w_full()
            .max_h(max_height)
            .overflow_hidden()
            .bg(cx.theme().background)
            .border_1()
            .border_color(border)
            .rounded_lg()
            .p_5()
            .gap_4()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        let Some((name, kind, favorite, folder, rows)) = data else {
            let (title, hint) = if loading {
                (t!("detail.loading"), t!("detail.loading_hint"))
            } else {
                (t!("detail.empty"), t!("detail.empty_hint"))
            };
            let mut body = v_flex()
                .gap_3()
                .child(div().text_sm().text_color(muted).child(hint));
            if let Some(problem) = detail_problem {
                body = body.child(div().text_sm().text_color(danger).child(problem));
                if let Some(id) = self.selected_id.clone() {
                    body = body.child(
                        Button::new("retry-detail")
                            .secondary()
                            .compact()
                            .label(t!("common.retry"))
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.request_detail(id.clone());
                            })),
                    );
                }
            }
            return overlay.child(
                card.child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(div().text_lg().font_weight(FontWeight::BOLD).child(title))
                        .child(
                            Button::new("detail-close")
                                .ghost()
                                .icon(AssetIcon::X)
                                .tooltip(t!("detail.close"))
                                .accessibility_label(t!("detail.close"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_detail(window, cx);
                                })),
                        ),
                )
                .child(body),
            );
        };

        let meta = if let Some(folder) = &folder {
            format!("{} · {folder}", kind_label(kind))
        } else {
            kind_label(kind)
        };

        let close = Button::new("detail-close")
            .ghost()
            .icon(AssetIcon::X)
            .tooltip(t!("detail.close"))
            .accessibility_label(t!("detail.close"))
            .on_click(cx.listener(|this, _, window, cx| {
                this.close_detail(window, cx);
            }));

        let header = h_flex()
            .items_start()
            .justify_between()
            .gap_3()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .text_lg()
                                    .font_weight(FontWeight::BOLD)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(name),
                            )
                            .when(favorite, |this| {
                                this.child(
                                    Icon::new(IconName::StarFill)
                                        .with_size(gpui_kit::component::Size::Medium)
                                        .text_color(cx.theme().warning),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(meta),
                    ),
            )
            .child(close);

        let mut card = card.h(card_height).child(header);

        if let Some(problem) = detail_problem {
            let retry_id = self.selected_id.clone();
            let mut banner = h_flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().text_sm().text_color(danger).child(problem));
            if let Some(id) = retry_id {
                banner = banner.child(
                    Button::new("retry-detail")
                        .secondary()
                        .compact()
                        .label(t!("common.retry"))
                        .on_click(cx.listener(move |this, _, _, _| {
                            this.request_detail(id.clone());
                        })),
                );
            }
            card = card.child(banner);
        }

        let mut children = Vec::new();
        for (ix, row) in rows.into_iter().enumerate() {
            children.push(self.render_detail_row(ix, row, cx).into_any_element());
        }
        card = card.child(
            v_flex()
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .gap_4()
                .children(children),
        );

        overlay.child(card)
    }

    fn render_detail_row(
        &mut self,
        ix: usize,
        row: DetailRow,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let revealed = !row.secret || self.revealed.contains(&ix);
        let value = if revealed {
            row.value.clone()
        } else {
            MASK.to_string()
        };

        let mut actions = h_flex().items_center().gap_1();
        if row.secret {
            let icon = if revealed {
                IconName::EyeOff
            } else {
                IconName::Eye
            };
            let tooltip = if revealed {
                t!("detail.hide").to_string()
            } else {
                t!("detail.reveal").to_string()
            };
            actions = actions.child(
                Button::new(SharedString::from(format!("reveal-{ix}")))
                    .ghost()
                    .icon(icon)
                    .tooltip(tooltip.clone())
                    .accessibility_label(tooltip)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.revealed.remove(&ix) {
                            this.revealed.insert(ix);
                        }
                        cx.notify();
                    })),
            );
        }
        if row.copyable {
            let copy_value = row.copy_value.clone().unwrap_or_else(|| row.value.clone());
            actions = actions.child(
                Button::new(SharedString::from(format!("copy-{ix}")))
                    .ghost()
                    .icon(IconName::Copy)
                    .tooltip(t!("detail.copy"))
                    .accessibility_label(t!("detail.copy"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.copy_value(copy_value.clone(), cx);
                    })),
            );
        }
        if let Some(url) = row.open_url.clone() {
            actions = actions.child(
                Button::new(SharedString::from(format!("open-{ix}")))
                    .ghost()
                    .icon(IconName::ExternalLink)
                    .tooltip(t!("detail.open"))
                    .accessibility_label(t!("detail.open"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_detail_url(url.clone(), cx);
                    })),
            );
        }

        let value_element: AnyElement = if row.selectable {
            Textarea::new(&self.notes)
                .readonly(true)
                .appearance(false)
                .bordered(false)
                .h(px(140.))
                .text_sm()
                .w_full()
                .into_any_element()
        } else {
            div()
                .text_sm()
                .when(row.large, |this| {
                    this.text_lg().font_weight(FontWeight::MEDIUM)
                })
                .when(row.multiline, |this| this.whitespace_normal())
                .when(!row.multiline, |this| {
                    this.overflow_hidden().text_ellipsis()
                })
                .child(value)
                .into_any_element()
        };

        let suffix = row.suffix.clone();
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(row.label),
            )
            .child(
                h_flex()
                    .gap_2()
                    // A tall button centres against a single line; wrapped or
                    // boxed values keep their first line at the top.
                    .when(row.multiline || row.selectable, |this| this.items_start())
                    .when(!row.multiline && !row.selectable, |this| {
                        this.items_center()
                    })
                    .child(div().flex_1().min_w_0().child(value_element))
                    .child(actions),
            )
            .when_some(suffix, |this, suffix| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(suffix),
                )
            })
    }

    fn render_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let max_height = window.viewport_size().height - px(64.);

        let overlay = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .p_6()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.35))
            .occlude()
            .track_focus(&self.settings_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close_settings(cx)),
            );

        let auto_lock_choices = {
            let mut choices = Vec::new();
            for minutes in [1u16, 5, 15, 30, 60] {
                choices.push(SettingsChoice {
                    label: t!("settings.minutes", minutes = minutes).to_string(),
                    active: self.settings.auto_lock_minutes == minutes,
                    on_click: Rc::new(cx.listener(move |this, _, _, cx| {
                        this.set_auto_lock(minutes, cx);
                    })),
                });
            }
            Rc::new(choices)
        };
        let auto_lock = choice_menu(
            "setting-auto-lock",
            t!(
                "settings.minutes",
                minutes = self.settings.auto_lock_minutes
            )
            .to_string(),
            auto_lock_choices,
        );

        let clipboard_choices = {
            let mut choices = Vec::new();
            for seconds in [15u16, 30, 60] {
                choices.push(SettingsChoice {
                    label: t!("settings.seconds", seconds = seconds).to_string(),
                    active: self.settings.clipboard_seconds == seconds,
                    on_click: Rc::new(cx.listener(move |this, _, _, cx| {
                        this.set_clipboard_seconds(seconds, cx);
                    })),
                });
            }
            Rc::new(choices)
        };
        let clipboard = choice_menu(
            "setting-clipboard",
            t!(
                "settings.seconds",
                seconds = self.settings.clipboard_seconds
            )
            .to_string(),
            clipboard_choices,
        );

        let theme_choices = {
            let choices = vec![
                SettingsChoice {
                    label: t!("settings.theme_system").to_string(),
                    active: self.settings.theme == ThemeChoice::System,
                    on_click: Rc::new(cx.listener(|this, _, window, cx| {
                        this.set_theme(ThemeChoice::System, window, cx);
                    })),
                },
                SettingsChoice {
                    label: t!("settings.theme_light").to_string(),
                    active: self.settings.theme == ThemeChoice::Light,
                    on_click: Rc::new(cx.listener(|this, _, window, cx| {
                        this.set_theme(ThemeChoice::Light, window, cx);
                    })),
                },
                SettingsChoice {
                    label: t!("settings.theme_dark").to_string(),
                    active: self.settings.theme == ThemeChoice::Dark,
                    on_click: Rc::new(cx.listener(|this, _, window, cx| {
                        this.set_theme(ThemeChoice::Dark, window, cx);
                    })),
                },
            ];
            Rc::new(choices)
        };
        let theme_label = match self.settings.theme {
            ThemeChoice::System => t!("settings.theme_system"),
            ThemeChoice::Light => t!("settings.theme_light"),
            ThemeChoice::Dark => t!("settings.theme_dark"),
        }
        .to_string();
        let theme = choice_menu("setting-theme", theme_label, theme_choices);

        let language_choices = {
            let choices = vec![
                SettingsChoice {
                    label: "日本語".to_string(),
                    active: self.locale == "ja",
                    on_click: Rc::new(cx.listener(|this, _, window, cx| {
                        this.set_language("ja".to_string(), window, cx);
                    })),
                },
                SettingsChoice {
                    label: "English".to_string(),
                    active: self.locale == "en",
                    on_click: Rc::new(cx.listener(|this, _, window, cx| {
                        this.set_language("en".to_string(), window, cx);
                    })),
                },
            ];
            Rc::new(choices)
        };
        let language = choice_menu(
            "setting-language",
            if self.locale == "ja" {
                "日本語".to_string()
            } else {
                "English".to_string()
            },
            language_choices,
        );

        let autostart = {
            let root = cx.entity().downgrade();
            Switch::new("setting-autostart")
                .checked(self.settings.autostart)
                .accessibility_label(t!("settings.autostart"))
                .on_click(move |requested, _, cx| {
                    let requested = *requested;
                    if let Some(root) = root.upgrade() {
                        root.update(cx, |root, cx| root.set_autostart(requested, cx));
                    }
                })
        };
        let start_in_tray = {
            let root = cx.entity().downgrade();
            Switch::new("setting-start-in-tray")
                .checked(self.settings.start_in_tray)
                .accessibility_label(t!("settings.start_in_tray"))
                .on_click(move |requested, _, cx| {
                    let requested = *requested;
                    if let Some(root) = root.upgrade() {
                        root.update(cx, |root, cx| root.set_start_in_tray(requested, cx));
                    }
                })
        };
        let destroy_on_hide = {
            let root = cx.entity().downgrade();
            Switch::new("setting-destroy-on-hide")
                .checked(self.settings.destroy_on_hide)
                .accessibility_label(t!("settings.destroy_on_hide"))
                .on_click(move |requested, _, cx| {
                    let requested = *requested;
                    if let Some(root) = root.upgrade() {
                        root.update(cx, |root, cx| root.set_destroy_on_hide(requested, cx));
                    }
                })
        };
        let capture = {
            let root = cx.entity().downgrade();
            Switch::new("setting-capture")
                .checked(self.settings.screenshot_protection)
                .accessibility_label(t!("settings.capture"))
                .on_click(move |requested, window, cx| {
                    let requested = *requested;
                    if let Some(root) = root.upgrade() {
                        root.update(cx, |root, cx| {
                            root.set_capture_protection(requested, window, cx);
                        });
                    }
                })
        };

        let rows = vec![
            v_flex()
                .gap_1()
                .child(div().text_sm().child(t!("settings.server_url")))
                .child(Input::new(&self.server_url).w_full())
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t!("settings.server_url_hint")),
                )
                .into_any_element(),
            setting_row(
                t!("settings.auto_lock").to_string(),
                t!("settings.auto_lock_hint").to_string(),
                muted,
                auto_lock,
            )
            .into_any_element(),
            setting_row(
                t!("settings.clipboard").to_string(),
                t!("settings.clipboard_hint").to_string(),
                muted,
                clipboard,
            )
            .into_any_element(),
            setting_row(
                t!("settings.theme").to_string(),
                String::new(),
                muted,
                theme,
            )
            .into_any_element(),
            setting_row(
                t!("settings.language").to_string(),
                String::new(),
                muted,
                language,
            )
            .into_any_element(),
            setting_row(
                t!("settings.autostart").to_string(),
                t!("settings.autostart_hint").to_string(),
                muted,
                autostart,
            )
            .into_any_element(),
            setting_row(
                t!("settings.start_in_tray").to_string(),
                String::new(),
                muted,
                start_in_tray,
            )
            .into_any_element(),
            setting_row(
                t!("settings.destroy_on_hide").to_string(),
                t!("settings.destroy_on_hide_hint").to_string(),
                muted,
                destroy_on_hide,
            )
            .into_any_element(),
            setting_row(
                t!("settings.capture").to_string(),
                t!("settings.capture_hint").to_string(),
                muted,
                capture,
            )
            .into_any_element(),
        ];

        let card = v_flex()
            .w(px(600.))
            .max_w_full()
            .max_h(max_height)
            .overflow_hidden()
            .bg(cx.theme().background)
            .border_1()
            .border_color(border)
            .rounded_lg()
            .p_5()
            .gap_4()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .child(t!("settings.title")),
                    )
                    .child(
                        Button::new("settings-close")
                            .ghost()
                            .icon(AssetIcon::X)
                            .tooltip(t!("detail.close"))
                            .accessibility_label(t!("detail.close"))
                            .on_click(cx.listener(|this, _, _, cx| this.close_settings(cx))),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .gap_4()
                    .children(rows),
            );

        overlay.child(card)
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page: AnyElement = match self.page {
            Page::Login => self.render_login(cx).into_any_element(),
            Page::TwoFactor => self.render_two_factor(cx).into_any_element(),
            Page::Vault => self.render_vault(cx).into_any_element(),
        };
        let modal = self
            .selected_id
            .is_some()
            .then(|| self.render_detail_dialog(window, cx).into_any_element());
        let settings = self
            .settings_open
            .then(|| self.render_settings(window, cx).into_any_element());
        let notice = self.notice.clone();
        let notice_color = if self.notice_error {
            cx.theme().danger
        } else {
            cx.theme().success
        };
        let border = cx.theme().border;

        v_flex()
            .relative()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .font_family("IBM Plex Sans JP")
            .child(self.render_header(cx))
            .child(page)
            .when_some(modal, |this, modal| this.child(modal))
            .when_some(settings, |this, settings| this.child(settings))
            .when_some(notice, |this, notice| {
                this.child(
                    div()
                        .absolute()
                        .bottom_3()
                        .left_3()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(cx.theme().background)
                        .border_1()
                        .border_color(border)
                        .text_xs()
                        .text_color(notice_color)
                        .child(notice),
                )
            })
    }
}

fn field(label: String, input: impl IntoElement, muted: Hsla) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_sm().text_color(muted).child(label))
        .child(input)
}

fn setting_row(
    label: String,
    hint: String,
    muted: Hsla,
    control: impl IntoElement,
) -> impl IntoElement {
    h_flex()
        .items_center()
        .justify_between()
        .gap_4()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(div().text_sm().child(label))
                .when(!hint.is_empty(), |this| {
                    this.child(div().text_xs().text_color(muted).child(hint))
                }),
        )
        .child(control)
}

fn choice_menu(
    id: &'static str,
    label: String,
    choices: Rc<Vec<SettingsChoice>>,
) -> impl IntoElement {
    Button::new(id)
        .ghost()
        .compact()
        .label(label)
        .dropdown_caret(true)
        .dropdown_menu(move |menu, _, _| {
            let mut menu = menu.min_w(px(180.));
            for choice in choices.iter() {
                let on_click = choice.on_click.clone();
                menu = menu.item(
                    PopupMenuItem::new(choice.label.clone())
                        .checked(choice.active)
                        .on_click(move |event, window, cx| on_click(event, window, cx)),
                );
            }
            menu
        })
}

fn theme_mode(choice: ThemeChoice, window: &Window) -> ThemeMode {
    match choice {
        ThemeChoice::System => window.appearance().into(),
        ThemeChoice::Light => ThemeMode::Light,
        ThemeChoice::Dark => ThemeMode::Dark,
    }
}

fn filter_button(
    id: impl Into<ElementId>,
    label: String,
    icon: Option<IconName>,
    active: bool,
    filter: VaultFilter,
    cx: &mut Context<Root>,
) -> Button {
    let mut button = Button::new(id)
        .ghost()
        .compact()
        .selected(active)
        .label(label);
    if let Some(icon) = icon {
        button = button.icon(icon);
    }
    button.on_click(cx.listener(move |this, _, window, cx| {
        this.set_filter(filter.clone(), window, cx);
    }))
}

fn divider(cx: &Context<Root>) -> impl IntoElement {
    div().w(px(1.)).h(px(16.)).bg(cx.theme().border)
}

fn kind_id(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Login => "type-login",
        ItemKind::Card => "type-card",
        ItemKind::Identity => "type-identity",
        ItemKind::SecureNote => "type-note",
        ItemKind::SshKey => "type-ssh",
        ItemKind::Other => "type-other",
    }
}

fn problem_text(problem: Option<&Problem>) -> String {
    match problem {
        None => String::new(),
        Some(Problem::MissingInput) => t!("error.missing_input").to_string(),
        Some(Problem::Core(err)) => match err {
            CoreError::InvalidCredentials => t!("error.invalid_credentials").to_string(),
            CoreError::TwoFactorFailed => t!("error.two_factor_failed").to_string(),
            CoreError::TwoFactorUnsupported => t!("error.two_factor_unsupported").to_string(),
            CoreError::Rejected(detail) => {
                format!("{}: {detail}", t!("error.rejected"))
            }
            CoreError::KeyNotLoaded => t!("error.key_not_loaded").to_string(),
            CoreError::Network => t!("error.network").to_string(),
            CoreError::Timeout => t!("error.timeout").to_string(),
            CoreError::InvalidResponse => t!("error.invalid_response").to_string(),
            CoreError::NotFound => t!("error.not_found").to_string(),
            CoreError::NoTotp => t!("error.no_totp").to_string(),
            CoreError::NoPassword => t!("error.no_password").to_string(),
            CoreError::Server(message) => {
                format!("{}: {message}", t!("error.server"))
            }
            CoreError::InvalidUrl => t!("error.invalid_url").to_string(),
            CoreError::InsecureUrl => t!("error.insecure_url").to_string(),
            CoreError::Internal => t!("error.internal").to_string(),
        },
    }
}
