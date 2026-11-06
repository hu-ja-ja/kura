//! The login and two-factor pages.

use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputContentType, OtpInput};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use kura_core::TwoFactorMethod;
use rust_i18n::t;

use super::{Problem, Root, problem_text};
impl Root {
    pub(super) fn render_login(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let typed = self.server_url.read(cx).value().to_string();
        let server =
            kura_core::sign_in_display(Some(&typed)).unwrap_or_else(|_| typed.trim().to_string());
        let accessing = t!("login.accessing", server = server).to_string();

        v_flex()
            .relative()
            .flex_1()
            .items_center()
            .justify_center()
            // Leave room for the absolute line below, or the card covers it.
            .pb_8()
            .child(
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
                            Input::new(&self.server_url)
                                .id("login-server")
                                .aria_label(t!("login.server_url")),
                            muted,
                        ))
                    })
                    .child(field(
                        t!("login.email").to_string(),
                        Input::new(&self.email)
                            .id("login-email")
                            .aria_label(t!("login.email")),
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
                                .id("login-password")
                                .aria_label(t!("login.password"))
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
                    .child(problem_banner(self.problem.as_ref(), danger))
                    .when_some(self.sign_in_server.clone(), |this, server| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(t!("login.master_password_goes_to", server = server)),
                        )
                    })
                    .child(
                        Button::new("sign-in")
                            .primary()
                            .w_full()
                            .label(match self.confirm_server.clone() {
                                Some(server) => t!("login.sign_in_to", server = server).to_string(),
                                None => t!("login.submit").to_string(),
                            })
                            .loading(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.submit_login(cx))),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .bottom_3()
                    .left_0()
                    .right_0()
                    .text_center()
                    .text_xs()
                    .text_color(muted)
                    .id("login-accessing")
                    .aria_label(accessing.clone())
                    .child(accessing)
                    .test_support(),
            )
    }

    pub(super) fn render_two_factor(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
                .child(problem_banner(self.problem.as_ref(), cx.theme().danger))
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
}
fn field(label: String, input: impl IntoElement, muted: Hsla) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_sm().text_color(muted).child(label))
        .child(input)
}

/// The reserved error line under the login and two-factor forms.
fn problem_banner(problem: Option<&Problem>, color: Hsla) -> impl IntoElement {
    div()
        .id("problem")
        .role(Role::Alert)
        .aria_label(problem_text(problem))
        .test_support()
        .min_h(px(20.))
        .text_sm()
        .text_color(color)
        .child(problem_text(problem))
}
