//! The settings card: preferences and the bundled licenses.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::*;
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use rust_i18n::t;

use super::{ClickHandler, Root};
use crate::licenses;
use crate::settings::ThemeChoice;
/// One entry in a settings choice row.
struct SettingsChoice {
    label: String,
    active: bool,
    on_click: ClickHandler,
}
impl Root {
    pub(super) fn render_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let max_height = window.viewport_size().height - px(64.);

        let overlay = super::modal_overlay().on_mouse_down(
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

        let root = cx.entity().downgrade();
        let toggle = |id: &'static str,
                      checked: bool,
                      label: String,
                      set: fn(&mut Root, bool, &mut Context<Root>)| {
            Switch::new(id)
                .checked(checked)
                .accessibility_label(label)
                .on_click({
                    let root = root.clone();
                    move |requested, _, cx| {
                        let requested = *requested;
                        if let Some(root) = root.upgrade() {
                            root.update(cx, |root, cx| set(root, requested, cx));
                        }
                    }
                })
        };
        let autostart = toggle(
            "setting-autostart",
            self.settings.autostart,
            t!("settings.autostart").to_string(),
            Root::set_autostart,
        );
        let start_in_tray = toggle(
            "setting-start-in-tray",
            self.settings.start_in_tray,
            t!("settings.start_in_tray").to_string(),
            Root::set_start_in_tray,
        );
        let destroy_on_hide = toggle(
            "setting-destroy-on-hide",
            self.settings.destroy_on_hide,
            t!("settings.destroy_on_hide").to_string(),
            Root::set_destroy_on_hide,
        );
        // This one needs the window, so it cannot share the plain toggle.
        let capture = {
            let root = root.clone();
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

        let auto_check = self.updates.read(cx).settings().auto_check;
        let auto_install = self.updates.read(cx).settings().auto_install;
        let update_auto_check = Switch::new("setting-update-auto-check")
            .checked(auto_check)
            .accessibility_label(t!("settings.update_auto_check"))
            .on_click(cx.listener(|this, requested: &bool, _, cx| {
                let requested = *requested;
                this.updates
                    .update(cx, |updates, cx| updates.set_auto_check(requested, cx));
            }));
        let update_auto_install = Switch::new("setting-update-auto-install")
            .checked(auto_install)
            .accessibility_label(t!("settings.update_auto_install"))
            .on_click(cx.listener(|this, requested: &bool, _, cx| {
                let requested = *requested;
                this.updates
                    .update(cx, |updates, cx| updates.set_auto_install(requested, cx));
            }));

        let mut server_rows = vec![
            v_flex()
                .gap_1()
                .child(div().text_sm().child(t!("settings.server_url")))
                .child(
                    Input::new(&self.server_url)
                        .w_full()
                        .aria_label(t!("settings.server_url")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t!("settings.server_url_hint")),
                )
                .into_any_element(),
        ];
        // The servers the user has approved sit right under the URL field they
        // belong to, so a mistaken approval can be taken back.
        server_rows.extend(
            self.settings
                .confirmed_servers
                .clone()
                .into_iter()
                .map(|server| approved_server_row(server, muted, cx)),
        );

        let account_rows = vec![
            setting_row(
                t!("settings.saved_login").to_string(),
                t!("settings.saved_login_hint").to_string(),
                muted,
                Button::new("setting-forget-login")
                    .danger()
                    .compact()
                    .label(t!("settings.forget_login"))
                    .accessibility_label(t!("settings.forget_login"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_forget_login(window, cx)),
                    ),
            )
            .into_any_element(),
        ];

        let capture_unavailable = self.capture_unavailable;
        let security_rows = vec![
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
            v_flex()
                .gap_1()
                .child(setting_row(
                    t!("settings.capture").to_string(),
                    t!("settings.capture_hint").to_string(),
                    muted,
                    capture,
                ))
                .when(capture_unavailable, |this| {
                    this.child(
                        div()
                            .id("setting-capture-unavailable")
                            .test_support()
                            .text_xs()
                            .text_color(muted)
                            .child(t!("settings.capture_unavailable")),
                    )
                })
                .into_any_element(),
        ];

        let appearance_rows = vec![
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
        ];

        let startup_rows = vec![
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
        ];

        let update_rows = vec![
            setting_row(
                t!(
                    "settings.update_version",
                    version = self.updates.read(cx).current_version()
                )
                .to_string(),
                String::new(),
                muted,
                Button::new("setting-update-check")
                    .ghost()
                    .compact()
                    .label(t!("settings.update_check_now"))
                    .accessibility_label(t!("settings.update_check_now"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.updates.update(cx, |updates, cx| updates.check_now(cx));
                    })),
            )
            .into_any_element(),
            setting_row(
                t!("settings.update_auto_check").to_string(),
                t!("settings.update_settings_hint").to_string(),
                muted,
                update_auto_check,
            )
            .into_any_element(),
            setting_row(
                t!("settings.update_auto_install").to_string(),
                String::new(),
                muted,
                update_auto_install,
            )
            .into_any_element(),
        ];

        let troubleshooting_rows = {
            let log_hint = crate::logging::folder().map_or_else(
                || t!("settings.logs_unavailable").to_string(),
                |folder| {
                    t!("settings.logs_hint", folder = folder.display().to_string()).to_string()
                },
            );
            vec![
                setting_row(
                    format!("Kura {}", env!("CARGO_PKG_VERSION")),
                    log_hint,
                    muted,
                    Button::new("setting-logs")
                        .ghost()
                        .compact()
                        .label(t!("settings.logs_open"))
                        .accessibility_label(t!("settings.logs_open"))
                        .on_click(cx.listener(|this, _, _, cx| this.open_log_folder(cx))),
                )
                .into_any_element(),
            ]
        };

        let groups = vec![
            setting_group(
                t!("settings.section_server").to_string(),
                server_rows,
                muted,
            ),
            setting_group(
                t!("settings.section_account").to_string(),
                account_rows,
                muted,
            ),
            setting_group(
                t!("settings.section_security").to_string(),
                security_rows,
                muted,
            ),
            setting_group(
                t!("settings.section_appearance").to_string(),
                appearance_rows,
                muted,
            ),
            setting_group(
                t!("settings.section_startup").to_string(),
                startup_rows,
                muted,
            ),
            setting_group(t!("settings.updates").to_string(), update_rows, muted),
            setting_group(
                t!("settings.section_troubleshooting").to_string(),
                troubleshooting_rows,
                muted,
            ),
            setting_row(
                t!("settings.licenses").to_string(),
                t!("settings.licenses_hint").to_string(),
                muted,
                Button::new("setting-licenses")
                    .ghost()
                    .compact()
                    .label(t!("settings.licenses_show"))
                    .accessibility_label(t!("settings.licenses"))
                    .on_click(cx.listener(|this, _, _, cx| this.open_licenses(cx))),
            )
            .into_any_element(),
        ];

        let licenses_view = self.licenses_open;
        let license_open = self.license_text.is_some();
        let title = self
            .license_text
            .as_ref()
            .and_then(|(ix, _)| licenses::crates().get(*ix))
            .map_or_else(
                || {
                    if licenses_view {
                        t!("settings.licenses").to_string()
                    } else {
                        t!("settings.title").to_string()
                    }
                },
                crate_label,
            );
        let back = licenses_view.then(|| {
            Button::new("settings-back")
                .ghost()
                .icon(AssetIcon::ChevronLeft)
                .tooltip(t!("common.back"))
                .accessibility_label(t!("common.back"))
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.license_text.is_some() {
                        this.close_license(cx);
                    } else {
                        this.close_licenses(cx);
                    }
                }))
                .into_any_element()
        });
        let body: AnyElement = if let Some((_, lines)) = self.license_text.clone() {
            license_lines(lines, cx.theme().foreground)
        } else if licenses_view {
            license_crates(muted, cx)
        } else {
            v_flex()
                .flex_1()
                .min_h_0()
                // The scrollbar overlays the right edge, so rows keep clear of it.
                .pr_4()
                .overflow_y_scrollbar()
                .gap_4()
                .children(ruled(groups, border))
                .into_any_element()
        };
        let fills = licenses_view || license_open;

        let card = v_flex()
            .id("settings-dialog")
            .w(px(if fills { 720. } else { 600. }))
            .max_w_full()
            .role(Role::Dialog)
            .aria_label(title.clone())
            .when(fills, |this| this.h(max_height))
            .when(!fills, |this| this.max_h(max_height))
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
                    .gap_2()
                    .when_some(back, |this, back| this.child(back))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .child(title),
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
            .child(body);

        overlay.child(card.focus_trap("settings-dialog-trap", &self.settings_focus))
    }

    /// The confirmation for "forget the saved login". It sits above the
    /// settings card; the backdrop cancels it without closing the settings.
    pub(super) fn render_forget_login(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(dialog) = &self.forget_login else {
            return div();
        };
        let focus = dialog.focus.clone();
        let reset_settings = dialog.reset_settings;
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;

        let overlay = super::modal_overlay().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| this.close_forget_login(window, cx)),
        );

        let card = v_flex()
            .id("forget-login-dialog")
            .w(px(440.))
            .max_w_full()
            .role(Role::Dialog)
            .aria_label(t!("settings.forget_login_title"))
            .bg(cx.theme().background)
            .border_1()
            .border_color(border)
            .rounded_lg()
            .p_5()
            .gap_4()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .child(t!("settings.forget_login_title")),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t!("settings.forget_login_body")),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Switch::new("forget-login-reset")
                            .checked(reset_settings)
                            .accessibility_label(t!("settings.forget_login_reset"))
                            .on_click(cx.listener(|this, requested: &bool, _, cx| {
                                if let Some(dialog) = this.forget_login.as_mut() {
                                    dialog.reset_settings = *requested;
                                }
                                cx.notify();
                            })),
                    )
                    .child(div().text_sm().child(t!("settings.forget_login_reset"))),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("forget-login-cancel")
                            .ghost()
                            .label(t!("common.cancel"))
                            .accessibility_label(t!("common.cancel"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_forget_login(window, cx)
                            })),
                    )
                    .child(
                        Button::new("forget-login-confirm")
                            .danger()
                            .label(t!("settings.forget_login_confirm"))
                            .accessibility_label(t!("settings.forget_login_confirm"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.confirm_forget_login(window, cx)
                            })),
                    ),
            );

        overlay.child(card.focus_trap("forget-login-trap", &focus))
    }
}
/// A titled group of rows. The rule goes between groups, not between rows.
fn setting_group(title: String, rows: Vec<AnyElement>, muted: Hsla) -> AnyElement {
    v_flex()
        .gap_3()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(muted)
                .child(title),
        )
        .children(rows)
        .into_any_element()
}

/// Puts a rule between the items.
fn ruled(items: Vec<AnyElement>, border: Hsla) -> Vec<AnyElement> {
    let mut ruled = Vec::with_capacity(items.len() * 2);
    for (ix, item) in items.into_iter().enumerate() {
        if ix > 0 {
            ruled.push(div().h(px(1.)).w_full().bg(border).into_any_element());
        }
        ruled.push(item);
    }
    ruled
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

/// One server the user approved, with a button that takes the approval back.
fn approved_server_row(server: String, muted: Hsla, cx: &mut Context<Root>) -> AnyElement {
    let id = SharedString::from(format!("forget-{server}"));
    let forget = server.clone();
    setting_row(
        server,
        t!("settings.confirmed_server_hint").to_string(),
        muted,
        Button::new(id)
            .ghost()
            .compact()
            .icon(AssetIcon::X)
            .tooltip(t!("settings.forget_server"))
            .accessibility_label(t!("settings.forget_server"))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.forget_server(forget.clone(), cx);
            })),
    )
    .into_any_element()
}

/// One fixed-height row per line so the text can be virtualized.
fn license_lines(lines: Arc<Vec<licenses::Line>>, foreground: Hsla) -> AnyElement {
    uniform_list("license-lines", lines.len(), move |range, _, _| {
        range
            .map(|ix| {
                let line = &lines[ix];
                div()
                    .px_1()
                    .h(px(20.))
                    .text_sm()
                    .whitespace_nowrap()
                    .text_color(foreground)
                    .when(line.heading, |this| this.font_weight(FontWeight::MEDIUM))
                    .child(line.text.clone())
            })
            .collect::<Vec<_>>()
    })
    .flex_1()
    .min_h_0()
    .into_any_element()
}
fn crate_label(krate: &licenses::Crate) -> String {
    format!("{} {}", krate.name, krate.version)
        .trim_end()
        .to_string()
}
fn license_crates(muted: Hsla, cx: &mut Context<Root>) -> AnyElement {
    let hover = cx.theme().list_hover;
    let root = cx.entity().downgrade();
    let crates = licenses::crates();
    uniform_list("license-crates", crates.len(), move |range, _, _| {
        range
            .map(|ix| {
                let krate = &crates[ix];
                let root = root.clone();
                h_flex()
                    .id(SharedString::from(format!("license-crate-{ix}")))
                    .cursor_pointer()
                    .items_center()
                    .gap_3()
                    .w_full()
                    .h(px(26.))
                    .px_2()
                    .rounded_md()
                    .hover(move |this| this.bg(hover))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_sm()
                            .child(crate_label(krate)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(krate.license_names()),
                    )
                    .on_click(move |_, _, cx: &mut App| {
                        let _ = root.update(cx, |root, cx| root.open_license(ix, cx));
                    })
            })
            .collect::<Vec<_>>()
    })
    .flex_1()
    .min_h_0()
    .into_any_element()
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
