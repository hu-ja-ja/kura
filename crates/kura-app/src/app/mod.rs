//! The window: login, two-factor and the vault list.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState, OtpEvent, OtpState, TextareaState};
use gpui_kit::component::list::ListState;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use kura_core::{
    CoreError, TwoFactorChallenge, TwoFactorMethod, VaultFilter, VaultItemDetail, VaultSnapshot,
};
use rust_i18n::t;
use zeroize::Zeroizing;

use crate::backend::{Backend, Command, Event};
use crate::clipboard::Guard as Clipboard;
use crate::controller;
use crate::detail::{DetailRow, otp_digits, tick_totp};
use crate::licenses;
use crate::list::VaultDelegate;
use crate::settings::{Settings, ThemeChoice};

#[cfg(debug_assertions)]
pub(crate) mod demo;
mod detail_view;
mod login;
mod settings_view;
#[cfg(test)]
mod ui_test;
mod vault_view;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Login,
    TwoFactor,
    Vault,
}
/// A problem shown in the UI.
pub(super) enum Problem {
    MissingInput,
    Core(CoreError),
}
const NOTICE_TIME: Duration = Duration::from_secs(2);
const ERROR_NOTICE_TIME: Duration = Duration::from_secs(6);
const TOTP_RETRY_SECS: u32 = 5;
pub(super) type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The confirmation for "forget the saved login". `reset_settings` is the
/// second choice in the dialog.
struct ForgetLogin {
    reset_settings: bool,
    focus: FocusHandle,
}

/// The floating toast. The generation invalidates stale hide timers.
#[derive(Default)]
struct Notice {
    text: Option<String>,
    error: bool,
    generation: u64,
}

/// Everything tied to the detail dialog. `clear` drops the decrypted item
/// and everything derived from it; the notes editor and the focus handle
/// are infrastructure and survive.
struct DetailPane {
    item: Option<VaultItemDetail>,
    /// Built once per detail and per TOTP update. The countdown only changes
    /// `item.totp.remaining`, which the render reads directly.
    rows: Arc<Vec<DetailRow>>,
    problem: Option<Problem>,
    loading: bool,
    selected_id: Option<String>,
    revealed: HashSet<String>,
    generation: u64,
    notes: Entity<TextareaState>,
    focus: FocusHandle,
}

impl DetailPane {
    fn clear(&mut self) {
        self.item = None;
        self.rows = Arc::new(Vec::new());
        self.problem = None;
        self.loading = false;
        self.selected_id = None;
        self.revealed.clear();
        self.generation += 1;
    }
}

pub struct Root {
    backend: Backend,
    settings: Settings,
    /// Whether preferences may be written to disk. Off in UI tests.
    persist: bool,
    platform: kura_platform::Platform,
    clipboard: Entity<Clipboard>,
    updates: Entity<crate::update::UpdateService>,
    page: Page,
    locale: String,
    window: AnyWindowHandle,
    server_url: Entity<InputState>,
    email: Entity<InputState>,
    password: Entity<InputState>,
    search: Entity<InputState>,
    otp: Entity<OtpState>,
    list: Entity<ListState<VaultDelegate>>,
    settings_focus: FocusHandle,
    settings_open: bool,
    /// The open "forget the saved login" confirmation.
    forget_login: Option<ForgetLogin>,
    /// The settings card shows the license list instead of the preferences.
    licenses_open: bool,
    /// The expanded license: the crate index and its text split into display rows.
    license_text: Option<(usize, Arc<Vec<licenses::Line>>)>,
    snapshot: Arc<VaultSnapshot>,
    filter: VaultFilter,
    show_server: bool,
    /// The base URL a sign-in would send the master password to. None for the
    /// servers Kura ships with.
    sign_in_server: Option<String>,
    /// The base URL the user has just been asked to approve. The next submit
    /// is the approval.
    confirm_server: Option<String>,
    busy: bool,
    problem: Option<Problem>,
    sync_problem: Option<Problem>,
    /// The last attempt to change capture protection failed, so the settings
    /// row says the feature is unavailable here.
    capture_unavailable: bool,
    /// The open detail dialog and its derived state.
    detail: DetailPane,
    notice: Notice,
    challenge: Option<TwoFactorChallenge>,
    method: Option<TwoFactorMethod>,
    email_code_sent: bool,
    _subscriptions: Vec<Subscription>,
}
impl Root {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        backend: Backend,
        settings: Settings,
        clipboard: Entity<Clipboard>,
        platform: kura_platform::Platform,
        saved_email: Option<String>,
        saved_server: Option<String>,
        updates: Entity<crate::update::UpdateService>,
    ) -> Self {
        // A missing settings file must not send the saved login's master
        // password to bitwarden.com.
        let mut settings = settings;
        if settings
            .server_url
            .as_deref()
            .is_none_or(|url| url.trim().is_empty())
        {
            settings.server_url = saved_server.filter(|server| !server.trim().is_empty());
        }
        let locale = settings.locale();
        rust_i18n::set_locale(&locale);
        Theme::change(theme_mode(settings.theme, window), Some(window), cx);
        let mut capture_unavailable = false;
        if settings.screenshot_protection
            && let Some(hwnd) = controller::window_hwnd(window)
            && let Err(err) = kura_platform::set_capture_protection(hwnd, true)
        {
            log::warn!("could not turn on capture protection: {err}");
            capture_unavailable = true;
        }
        let unlocked = backend.is_unlocked();
        let sign_in_server = kura_core::sign_in_base(settings.server_url.as_deref())
            .ok()
            .flatten();
        // Only a server the user must confirm opens the field; the official
        // EU server is still named by the login screen's bottom line.
        let show_server = sign_in_server.is_some();

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
        let notes = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 500));

        let list = cx.new(|cx| {
            ListState::new(
                VaultDelegate::new(Arc::new(VaultSnapshot::default())),
                window,
                cx,
            )
        });
        let modal_focus = cx.focus_handle();
        let settings_focus = cx.focus_handle();
        setup_list(&list, cx);

        let mut subscriptions = vec![];
        subscriptions.push(cx.observe(&updates, |_, _, cx| cx.notify()));
        for input in [&email, &password, &server_url] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.submit_login(cx);
                    }
                },
            ));
        }
        subscriptions.push(cx.subscribe_in(
            &server_url,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.refresh_sign_in_server(cx);
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

        subscriptions.push(cx.observe_keystrokes(handle_keystroke));

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
            backend.send(Command::Snapshot);
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
            persist: true,
            platform,
            clipboard,
            updates,
            page: if unlocked { Page::Vault } else { Page::Login },
            locale,
            window: window.window_handle(),
            server_url,
            email,
            password,
            search,
            otp,
            list,
            settings_focus,
            settings_open: false,
            forget_login: None,
            licenses_open: false,
            license_text: None,
            snapshot: Arc::new(VaultSnapshot::default()),
            filter: VaultFilter::All,
            show_server,
            sign_in_server,
            confirm_server: None,
            busy: unlocked,
            problem: None,
            sync_problem: None,
            capture_unavailable,
            detail: DetailPane {
                item: None,
                rows: Arc::new(Vec::new()),
                problem: None,
                loading: false,
                selected_id: None,
                revealed: HashSet::new(),
                generation: 0,
                notes,
                focus: modal_focus,
            },
            notice: Notice::default(),
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
                self.forget_login = None;
                self.problem = None;
                self.sync_problem = None;
                self.challenge = None;
                self.method = None;
                self.confirm_server = None;
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
            Event::ConfirmServer { server } => {
                // The master password has not been sent. Name the server and
                // let the user approve it; the next submit is that approval.
                self.page = Page::Login;
                self.busy = false;
                self.problem = None;
                self.challenge = None;
                self.method = None;
                self.settings_open = false;
                self.forget_login = None;
                self.licenses_open = false;
                self.confirm_server = Some(server);
            }
            Event::TwoFactorRequired(challenge) => {
                self.busy = false;
                self.settings_open = false;
                self.forget_login = None;
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
                if let Some(id) = self.detail.selected_id.clone() {
                    if self.snapshot.items.iter().any(|item| item.id == id) {
                        let row = self.list.read(cx).delegate().row_of(&id);
                        let list = self.list.clone();
                        self.in_window(None, cx, move |window, cx| {
                            list.update(cx, |list, cx| {
                                list.set_selected_index(row.map(IndexPath::new), window, cx);
                            });
                        });
                        if !self.detail.loading {
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
                if self.detail.selected_id.as_deref() == Some(detail.id.as_str()) {
                    self.detail.loading = false;
                    self.detail.problem = None;
                    self.detail.item = Some(*detail);
                    self.detail.rows = Arc::new(
                        self.detail
                            .item
                            .as_ref()
                            .map(crate::detail::rows)
                            .unwrap_or_default(),
                    );
                    let notes = self
                        .detail
                        .item
                        .as_ref()
                        .and_then(|detail| detail.notes.as_ref())
                        .map(|notes| notes.to_string())
                        .unwrap_or_default();
                    let notes_state = self.detail.notes.clone();
                    self.in_window(None, cx, move |window, cx| {
                        notes_state.update(cx, |state, cx| state.set_value(notes, window, cx));
                    });
                    self.start_totp_ticks(cx);
                }
            }
            Event::Totp { id, totp } => {
                if self.detail.selected_id.as_deref() == Some(id.as_str()) {
                    let mut changed = false;
                    if let Some(detail) = self.detail.item.as_mut()
                        && let Some(current) = detail.totp.as_mut()
                    {
                        *current = totp;
                        changed = true;
                    }
                    if changed {
                        self.detail.problem = None;
                        self.detail.rows = Arc::new(
                            self.detail
                                .item
                                .as_ref()
                                .map(crate::detail::rows)
                                .unwrap_or_default(),
                        );
                    }
                }
            }
            Event::DetailFailed { id, err } => {
                if self.detail.selected_id.as_deref() == Some(id.as_str()) {
                    self.detail.loading = false;
                    let not_found = err == CoreError::NotFound;
                    self.detail.problem = Some(Problem::Core(err));
                    // A refresh can fail after the open item's ticker was
                    // stopped by request_detail. The old TOTP is still shown,
                    // so keep its countdown honest.
                    if !not_found
                        && self
                            .detail
                            .item
                            .as_ref()
                            .is_some_and(|detail| detail.totp.is_some())
                    {
                        self.start_totp_ticks(cx);
                    }
                }
            }
            Event::PasswordCopied(result) => {
                if self.page == Page::Vault {
                    match result {
                        Ok(password) => self.copy_value(&password, cx),
                        Err(err) => {
                            let problem = Problem::Core(err);
                            self.show_notice(problem_text(Some(&problem)), true, cx);
                        }
                    }
                }
            }
            Event::SessionNotSaved => {
                self.show_notice(t!("error.session_not_saved").to_string(), true, cx);
            }
            Event::SavedLoginForgotten(removed) => {
                if removed {
                    self.show_notice(t!("settings.saved_login_removed").to_string(), false, cx);
                } else {
                    self.show_notice(
                        t!("settings.saved_login_remove_failed").to_string(),
                        true,
                        cx,
                    );
                }
            }
            Event::TotpFailed { id, err } => {
                if self.detail.selected_id.as_deref() == Some(id.as_str()) {
                    let no_totp = err == CoreError::NoTotp;
                    self.detail.problem = Some(Problem::Core(err));
                    if no_totp {
                        if let Some(detail) = self.detail.item.as_mut() {
                            detail.totp = None;
                        }
                        self.detail.rows = Arc::new(
                            self.detail
                                .item
                                .as_ref()
                                .map(crate::detail::rows)
                                .unwrap_or_default(),
                        );
                    } else if let Some(totp) = self
                        .detail
                        .item
                        .as_mut()
                        .and_then(|detail| detail.totp.as_mut())
                    {
                        // Try again shortly instead of freezing on a stale code.
                        totp.remaining = TOTP_RETRY_SECS;
                    }
                }
            }
            Event::Failed(err) => {
                let was_busy = self.busy;
                self.busy = false;
                self.confirm_server = None;
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
        let password = Zeroizing::new(self.password.read(cx).value().to_string());
        let server_url = self.server_url.read(cx).value().trim().to_string();
        if email.is_empty() || password.is_empty() {
            self.problem = Some(Problem::MissingInput);
            return;
        }

        self.problem = None;
        self.busy = true;
        // The approval is what the user just gave, so it is written now. The
        // server URL itself is still only kept once a sign-in succeeds.
        if let Some(server) = self.confirm_server.clone()
            && !self.settings.server_confirmed(&server)
        {
            self.settings.confirmed_servers.push(server);
            self.save_settings(cx);
        }
        self.settings.server_url = (!server_url.is_empty()).then(|| server_url.clone());
        self.backend.send(Command::Login {
            server_url,
            email,
            password,
            confirmed_server: self.confirm_server.clone(),
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
        if self.detail.selected_id.as_deref() == Some(id.as_str())
            && (self.detail.item.is_some() || self.detail.loading)
        {
            return;
        }
        self.forget_detail(Some(window), cx);
        self.detail.selected_id = Some(id.clone());
        self.request_detail(id);
        let focus = self.detail.focus.clone();
        window.defer(cx, move |window, cx| focus.focus(window, cx));
        cx.notify();
    }

    fn request_detail(&mut self, id: String) {
        self.detail.revealed.clear();
        self.detail.loading = true;
        self.detail.problem = None;
        self.detail.generation += 1;
        self.backend.send(Command::Detail { id });
    }

    /// Drop the decrypted item and everything derived from it.
    fn forget_detail(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        self.detail.clear();
        self.notice.text = None;
        self.notice.error = false;
        let notes = self.detail.notes.clone();
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
        self.forget_login = None;
        self.busy = false;
        self.problem = None;
        self.sync_problem = None;
        self.challenge = None;
        self.method = None;
        self.confirm_server = None;
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
    fn copy_value(&mut self, value: &str, cx: &mut Context<Self>) {
        if value.is_empty() {
            return;
        }
        let timeout = self.settings.clipboard_clear();
        let seconds = self.settings.clipboard_seconds();
        let copied = self
            .clipboard
            .update(cx, |clipboard, cx| clipboard.copy(value, timeout, cx));
        if copied {
            self.show_notice(
                t!("detail.copied", seconds = seconds).to_string(),
                false,
                cx,
            );
        } else {
            log::warn!("the clipboard could not be written");
            self.show_notice(t!("detail.copy_failed").to_string(), true, cx);
        }
    }

    fn show_notice(&mut self, text: String, error: bool, cx: &mut Context<Self>) {
        if !error && self.notice.error && self.notice.text.is_some() {
            // A success must not hide a failure that is still on screen.
            return;
        }
        self.notice.generation += 1;
        let generation = self.notice.generation;
        let duration = notice_time(error);
        self.notice = Notice {
            text: Some(text),
            error,
            generation,
        };

        let this = cx.entity();
        cx.spawn(async move |_this, cx| {
            cx.background_executor().timer(duration).await;
            cx.update_entity(&this, |this, cx| {
                if this.notice.generation == generation {
                    this.notice.text = None;
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

    /// Copy one detail row. The row is looked up when the button is clicked so
    /// the render does not clone every secret into the button handlers.
    fn copy_detail_row(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(row) = self.detail.rows.iter().find(|row| row.id.as_str() == id) else {
            return;
        };
        let value = row.copy_value.as_ref().map_or_else(
            || row.value.as_str().to_string(),
            |value| value.as_str().to_string(),
        );
        self.copy_value(&value, cx);
    }

    fn open_detail_url(&mut self, url: String, cx: &mut Context<Self>) {
        if let Some(url) = kura_core::openable_url(&url) {
            cx.open_url(&url);
        }
    }

    /// Keep the TOTP countdown moving and ask for a fresh code at zero.
    fn start_totp_ticks(&self, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail.item else {
            return;
        };
        if detail.totp.is_none() {
            return;
        }
        let generation = self.detail.generation;
        let id = detail.id.clone();
        let this = cx.entity();

        cx.spawn(async move |_this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep_going = cx.update_entity(&this, |this, cx| {
                    if this.detail.generation != generation || this.page != Page::Vault {
                        return false;
                    }
                    let Some(detail) = this.detail.item.as_mut() else {
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

    /// The window was shown from the tray. Refresh the open vault in the
    /// background; the list it already holds stays visible.
    pub(crate) fn on_shown(&mut self, cx: &mut Context<Self>) {
        if self.page == Page::Vault {
            self.backend.send(Command::Refresh);
            cx.notify();
        }
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            return;
        }
        self.settings_open = true;
        self.forget_login = None;
        self.licenses_open = false;
        self.license_text = None;
        let focus = self.settings_focus.clone();
        window.defer(cx, move |window, cx| focus.focus(window, cx));
        cx.notify();
    }

    fn close_settings(&mut self, cx: &mut Context<Self>) {
        if !self.settings_open {
            return;
        }
        self.settings_open = false;
        self.forget_login = None;
        self.licenses_open = false;
        self.license_text = None;
        self.commit_server_url(cx);
        self.save_settings(cx);
        cx.notify();
    }

    fn open_forget_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = cx.focus_handle();
        self.forget_login = Some(ForgetLogin {
            reset_settings: false,
            focus: focus.clone(),
        });
        window.defer(cx, move |window, cx| focus.focus(window, cx));
        cx.notify();
    }

    fn close_forget_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.forget_login = None;
        // Keep the settings card's trap alive after the dialog's focus handle
        // is dropped.
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    fn confirm_forget_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let reset = self
            .forget_login
            .as_ref()
            .is_some_and(|dialog| dialog.reset_settings);
        self.forget_login = None;
        self.backend.send(Command::ForgetSavedLogin);
        if reset {
            self.reset_settings(window, cx);
        }
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    /// Return the preferences to their defaults and apply the ones that are
    /// live: the tray labels, auto-lock, theme, locale, server URL and update
    /// preferences. The window placement is cleared with the rest. The email
    /// input is not a setting, so it stays.
    fn reset_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let previous = std::mem::take(&mut self.settings);
        if previous.autostart
            && let Err(err) = kura_platform::set_autostart(false)
        {
            log::warn!("could not remove the startup entry: {err}");
            // The Run value is still there, so keep the preference on: the UI
            // must not claim the app no longer starts with Windows.
            self.settings.autostart = true;
            self.show_notice(t!("settings.autostart_failed").to_string(), true, cx);
        }
        // The worker keeps the approved servers in memory; drop them too, or
        // the next sign-in would still skip the confirmation.
        for server in previous.confirmed_servers {
            self.backend.send(Command::ForgetServer { server });
        }

        self.locale = self.settings.locale();
        rust_i18n::set_locale(&self.locale);
        self.platform.set_labels(controller::labels());
        self.platform
            .set_auto_lock(self.settings.auto_lock().as_secs());
        self.platform.set_appearance(self.settings.theme.into());
        Theme::change(theme_mode(self.settings.theme, window), Some(window), cx);

        match controller::window_hwnd(window) {
            Some(hwnd) => {
                if let Err(err) =
                    kura_platform::set_capture_protection(hwnd, self.settings.screenshot_protection)
                {
                    log::warn!("could not apply capture protection: {err}");
                    self.capture_unavailable = true;
                    self.settings.screenshot_protection = previous.screenshot_protection;
                    self.show_notice(t!("settings.capture_failed").to_string(), true, cx);
                } else {
                    self.capture_unavailable = false;
                }
            }
            None => {
                self.settings.screenshot_protection = previous.screenshot_protection;
                self.show_notice(t!("settings.capture_failed").to_string(), true, cx);
            }
        }

        self.server_url
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.show_server = false;
        self.sign_in_server = None;
        self.confirm_server = None;

        let placeholder = t!("vault.search").to_string();
        self.search.update(cx, |search, cx| {
            search.set_placeholder(placeholder, window, cx)
        });

        self.updates
            .update(cx, |updates, cx| updates.reset_settings(cx));
        self.save_settings(cx);
        cx.notify();
    }

    fn open_licenses(&mut self, cx: &mut Context<Self>) {
        self.licenses_open = true;
        self.license_text = None;
        cx.notify();
    }

    /// Open the folder that holds `kura.log` in Explorer. GPUI runs this on
    /// its background executor; a synchronous ShellExecuteW here re-enters the
    /// window procedure while the app is borrowed.
    fn open_log_folder(&mut self, cx: &mut Context<Self>) {
        if let Some(folder) = crate::logging::folder() {
            cx.open_with_system(&folder);
        }
    }

    fn open_license(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(krate) = licenses::crates().get(ix) else {
            return;
        };
        self.license_text = Some((ix, Arc::new(krate.lines())));
        cx.notify();
    }

    fn close_license(&mut self, cx: &mut Context<Self>) {
        self.license_text = None;
        cx.notify();
    }

    fn close_licenses(&mut self, cx: &mut Context<Self>) {
        self.licenses_open = false;
        self.license_text = None;
        cx.notify();
    }

    fn commit_server_url(&mut self, cx: &Context<Self>) {
        let value = self.server_url.read(cx).value().trim().to_string();
        self.settings.server_url = (!value.is_empty()).then_some(value);
    }

    /// Recompute where a sign-in would send the master password. Changing the
    /// server withdraws a confirmation that was given for the old one.
    fn refresh_sign_in_server(&mut self, cx: &mut Context<Self>) {
        let url = self.server_url.read(cx).value().trim().to_string();
        let server = kura_core::sign_in_base(Some(&url)).ok().flatten();
        if server.is_some() {
            // A custom server has to be visible, or the user cannot check it.
            self.show_server = true;
        }
        if server != self.sign_in_server {
            self.sign_in_server = server;
            self.confirm_server = None;
        }
        // The sign-in button and the "password goes to" hint follow the field.
        cx.notify();
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        if !self.persist {
            return;
        }
        if let Err(err) = self.settings.save() {
            log::warn!("could not save the settings: {err}");
            self.show_notice(t!("settings.save_failed").to_string(), true, cx);
        }
    }

    /// Take back an approval: the next sign-in to `server` asks again.
    fn forget_server(&mut self, server: String, cx: &mut Context<Self>) {
        self.settings.forget_server(&server);
        if self.confirm_server.as_deref() == Some(server.as_str()) {
            self.confirm_server = None;
        }
        self.save_settings(cx);
        self.backend.send(Command::ForgetServer { server });
        cx.notify();
    }

    fn set_theme(&mut self, choice: ThemeChoice, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.theme = choice;
        self.platform.set_appearance(choice.into());
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
                self.capture_unavailable = false;
                self.settings.screenshot_protection = enabled;
                self.save_settings(cx);
            }
            Err(err) => {
                self.capture_unavailable = true;
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
            // Do not keep a half-finished login, and its master password,
            // alive while the window sits in the tray. The reset also clears
            // the busy flag so a hidden window is usable when it returns.
            self.backend.cancel();
            self.reset_to_login(Some(window), cx);
        }
        if self.settings.destroy_on_hide {
            window.defer(cx, |window, _| window.remove_window());
        } else if let Some(hwnd) = controller::window_hwnd(window) {
            kura_platform::hide_window(hwnd);
        }
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
            .detail
            .selected_id
            .is_some()
            .then(|| self.render_detail_dialog(window, cx).into_any_element());
        let settings = self
            .settings_open
            .then(|| self.render_settings(window, cx).into_any_element());
        let forget_login = (self.settings_open && self.forget_login.is_some())
            .then(|| self.render_forget_login(window, cx).into_any_element());
        let notice = self.notice.text.clone();
        let notice_color = if self.notice.error {
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
            .when_some(forget_login, |this, forget_login| this.child(forget_login))
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

/// Wire the list's open and copy actions back to the root.
fn setup_list(list: &Entity<ListState<VaultDelegate>>, cx: &mut Context<Root>) {
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
                    root.copy_value(value, cx);
                });
            });
        list.delegate_mut()
            .set_on_copy_password(move |id, _window, cx| {
                let _ = password_root.update(cx, |root, _| {
                    root.copy_item_password(id.to_string());
                });
            });
    });
}

/// Global keys: Ctrl+L locks from every screen, Escape closes the dialogs,
/// Ctrl+F reaches the search field, Ctrl+V fills the OTP input, and the
/// search field hands navigation to the list.
fn handle_keystroke(
    this: &mut Root,
    event: &KeystrokeEvent,
    window: &mut Window,
    cx: &mut Context<Root>,
) {
    let keystroke = &event.keystroke;

    // Locking works from every screen, including the settings dialog.
    if keystroke.key == "l" && keystroke.modifiers.control {
        cx.stop_propagation();
        this.lock(window, cx);
        return;
    }

    if this.forget_login.is_some() {
        if keystroke.key == "escape" {
            cx.stop_propagation();
            this.close_forget_login(window, cx);
        }
        return;
    }

    if this.settings_open {
        if keystroke.key == "escape" {
            cx.stop_propagation();
            if this.license_text.is_some() {
                this.close_license(cx);
            } else if this.licenses_open {
                this.close_licenses(cx);
            } else {
                this.close_settings(cx);
            }
        }
        return;
    }

    if keystroke.key == "f" && keystroke.modifiers.control {
        // Swallow the key so the letter does not land in the search field.
        cx.stop_propagation();
        if this.detail.selected_id.is_none() {
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
    if keystroke.key == "escape" && this.detail.selected_id.is_some() {
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
                // `set_value` does not emit a change event.
                this.list
                    .update(cx, |list, cx| list.set_query("", window, cx));
                cx.notify();
            } else {
                this.list.update(cx, |list, cx| list.focus(window, cx));
            }
        }
        _ => {}
    }
}

/// How long a notice stays up. Errors need longer: the text is often a full
/// sentence and the user may be looking elsewhere when it appears.
fn notice_time(error: bool) -> Duration {
    if error {
        ERROR_NOTICE_TIME
    } else {
        NOTICE_TIME
    }
}

/// The dimmed backdrop behind the modal cards. The call sites attach their
/// own mouse handler because each card closes differently. The card's
/// `focus_trap` already tracks the dialog focus, so the backdrop stays out of
/// the accessibility tree.
pub(super) fn modal_overlay() -> Div {
    div()
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
}

fn theme_mode(choice: ThemeChoice, window: &Window) -> ThemeMode {
    match choice {
        ThemeChoice::System => window.appearance().into(),
        ThemeChoice::Light => ThemeMode::Light,
        ThemeChoice::Dark => ThemeMode::Dark,
    }
}
pub(super) fn problem_text(problem: Option<&Problem>) -> String {
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
            CoreError::AccountDeleted => t!("error.account_deleted").to_string(),
            CoreError::InvalidUrl => t!("error.invalid_url").to_string(),
            CoreError::InsecureUrl => t!("error.insecure_url").to_string(),
            CoreError::Internal(_) => t!("error.internal").to_string(),
        },
    }
}
