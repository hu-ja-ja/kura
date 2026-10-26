//! The window: login, two-factor and the vault list.

use std::sync::Arc;

use gpui_kit::component::button::*;
use gpui_kit::component::input::{
    Input, InputContentType, InputEvent, InputState, OtpEvent, OtpInput, OtpState,
};
use gpui_kit::component::list::{List, ListState};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use kura_core::{CoreError, TwoFactorChallenge, TwoFactorMethod, VaultSnapshot};
use rust_i18n::t;

use crate::backend::{Backend, Command, Event};
use crate::list::VaultDelegate;
use crate::settings::Settings;

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

pub struct Root {
    backend: Backend,
    settings: Settings,
    page: Page,
    locale: String,
    window: AnyWindowHandle,
    server_url: Entity<InputState>,
    email: Entity<InputState>,
    password: Entity<InputState>,
    otp: Entity<OtpState>,
    list: Entity<ListState<VaultDelegate>>,
    snapshot: Arc<VaultSnapshot>,
    busy: bool,
    problem: Option<Problem>,
    sync_problem: Option<Problem>,
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
        events: tokio::sync::mpsc::UnboundedReceiver<Event>,
        settings: Settings,
    ) -> Self {
        let locale = settings
            .language
            .clone()
            .filter(|locale| locale == "ja" || locale == "en")
            .unwrap_or_else(system_locale);
        rust_i18n::set_locale(&locale);

        let server_url =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://vault.example.com"));
        if let Some(url) = settings.server_url.clone() {
            server_url.update(cx, |input, cx| input.set_value(url, window, cx));
        }
        let email = cx.new(|cx| InputState::new(window, cx));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let otp = cx.new(|cx| OtpState::new(6, window, cx));
        let list = cx.new(|cx| {
            ListState::new(
                VaultDelegate::new(Arc::new(VaultSnapshot::default())),
                window,
                cx,
            )
            .searchable(true)
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
                if keystroke.key == "f" && keystroke.modifiers.control {
                    // Swallow the key so the letter does not land in the search field.
                    cx.stop_propagation();
                    this.focus_search(window, cx);
                }
            }),
        );

        // The worker answers on its own task. Bring every answer to the UI thread.

        let this = cx.entity();
        cx.spawn(async move |_this, cx| {
            let mut events = events;
            while let Some(event) = events.recv().await {
                cx.update_entity(&this, |this, cx| this.handle_event(event, cx));
            }
        })
        .detach();

        Self {
            backend,
            settings,
            page: Page::Login,
            locale,
            window: window.window_handle(),
            server_url,
            email,
            password,
            otp,
            list,
            snapshot: Arc::new(VaultSnapshot::default()),
            busy: false,
            problem: None,
            sync_problem: None,
            challenge: None,
            method: None,
            email_code_sent: false,
            _subscriptions: subscriptions,
        }
    }

    // ----------------------------------------------------------------- events

    fn handle_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::LoggedIn => {
                self.busy = false;
                self.page = Page::Vault;
                self.problem = None;
                self.challenge = None;
                self.method = None;
                self.clear_password(cx);
                // The server URL was accepted, so keep it for the next start.
                let _ = self.settings.save();
                self.list.update(cx, |list, cx| {
                    list.delegate_mut().set_loading(true);
                    cx.notify();
                });
            }
            Event::TwoFactorRequired(challenge) => {
                self.busy = false;
                self.method = challenge.methods.first().cloned();
                self.challenge = Some(challenge);
                self.page = Page::TwoFactor;
                self.problem = None;
                self.email_code_sent = false;
                self.clear_password(cx);
            }
            Event::Snapshot(snapshot) => {
                self.busy = false;
                self.sync_problem = None;
                self.snapshot = snapshot.clone();
                self.list.update(cx, |list, cx| {
                    list.delegate_mut().set_snapshot(snapshot);
                    list.delegate_mut().set_loading(false);
                    cx.notify();
                });
            }
            Event::EmailCodeSent => {
                self.busy = false;
                self.email_code_sent = true;
            }
            Event::Failed(err) => {
                self.busy = false;
                let problem = Problem::Core(err);
                if self.page == Page::Vault {
                    self.sync_problem = Some(problem);
                } else {
                    self.problem = Some(problem);
                    if self.page == Page::TwoFactor {
                        // A rejected code is worth typing again from scratch.
                        self.clear_code(cx);
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

    fn back_to_login(&mut self, cx: &mut Context<Self>) {
        self.backend.send(Command::Cancel);
        self.page = Page::Login;
        self.challenge = None;
        self.method = None;
        self.problem = None;
        self.busy = false;
        self.clear_code(cx);
    }

    fn toggle_language(&mut self, cx: &mut Context<Self>) {
        self.locale = if self.locale == "ja" {
            "en".to_string()
        } else {
            "ja".to_string()
        };
        rust_i18n::set_locale(&self.locale);
        self.settings.language = Some(self.locale.clone());
        let _ = self.settings.save();
        cx.notify();
    }

    fn clear_password(&mut self, cx: &mut Context<Self>) {
        let password = self.password.clone();
        self.in_window(cx, move |window, cx| {
            password.update(cx, |input, cx| input.set_value("", window, cx));
        });
    }

    fn clear_code(&mut self, cx: &mut Context<Self>) {
        let otp = self.otp.clone();
        self.in_window(cx, move |window, cx| {
            otp.update(cx, |state, cx| state.set_value("", window, cx));
        });
    }

    fn in_window<R>(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Window, &mut App) -> R,
    ) -> Option<R> {
        cx.update_window(self.window, move |_, window, cx| f(window, cx))
            .ok()
    }

    /// Ctrl+F puts the cursor in the search field.
    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Page::Vault {
            return;
        }
        self.list.update(cx, |list, cx| {
            list.focus(window, cx);
        });
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
                    })
                    .child(
                        Button::new("language")
                            .ghost()
                            .compact()
                            .label(if self.locale == "ja" {
                                "English"
                            } else {
                                "日本語"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_language(cx))),
                    ),
            )
    }

    fn render_login(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        v_flex().flex_1().items_center().justify_center().child(
            v_flex()
                .w(px(400.))
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
                                .child(t!("login.title")),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child(t!("login.subtitle")),
                        ),
                )
                .child(field(
                    t!("login.server_url").to_string(),
                    Input::new(&self.server_url),
                    muted,
                ))
                .child(field(
                    t!("login.email").to_string(),
                    Input::new(&self.email),
                    muted,
                ))
                .child(field(
                    t!("login.password").to_string(),
                    Input::new(&self.password)
                        .mask_toggle()
                        .content_type(InputContentType::Password),
                    muted,
                ))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
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
                .w(px(400.))
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
                                .on_click(cx.listener(|this, _, _, cx| this.back_to_login(cx))),
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

    fn render_vault(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let failures = self.snapshot.decrypt_failures;
        v_flex()
            .flex_1()
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
            .child(
                div()
                    .px_4()
                    .py_2()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(problem_text(self.sync_problem.as_ref())),
            )
            .child(
                List::new(&self.list)
                    .search_placeholder(t!("vault.search"))
                    .flex_1(),
            )
    }
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page: AnyElement = match self.page {
            Page::Login => self.render_login(cx).into_any_element(),
            Page::TwoFactor => self.render_two_factor(cx).into_any_element(),
            Page::Vault => self.render_vault(cx).into_any_element(),
        };

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_header(cx))
            .child(page)
    }
}

fn field(label: String, input: impl IntoElement, muted: Hsla) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_sm().text_color(muted).child(label))
        .child(input)
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
            CoreError::Server(message) => {
                format!("{}: {message}", t!("error.server"))
            }
            CoreError::InvalidUrl => t!("error.invalid_url").to_string(),
            CoreError::InsecureUrl => t!("error.insecure_url").to_string(),
            CoreError::Internal => t!("error.internal").to_string(),
        },
    }
}

/// The language of the system, when it is one Kura speaks.
fn system_locale() -> String {
    match sys_locale::get_locale() {
        Some(locale) if locale.starts_with("ja") => "ja".to_string(),
        _ => "en".to_string(),
    }
}
