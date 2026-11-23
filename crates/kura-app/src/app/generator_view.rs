//! The password and passphrase generator dialog. The form and the vault
//! header both open it; only the primary action differs.

use std::rc::Rc;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use kura_core::{
    GeneratorMode, MAX_MIN_CHARS, MAX_PASSPHRASE_WORDS, MAX_PASSWORD_LENGTH, MIN_PASSPHRASE_WORDS,
    PasswordOptions, effective_password_length, minimum_password_length,
};
use rust_i18n::t;
use zeroize::Zeroizing;

use super::{GeneratorPane, Problem, Root, modal_overlay};

const SEPARATORS: [&str; 5] = ["-", " ", ".", "_", ","];

type BoolSetter = Rc<dyn Fn(&mut Root, bool, &mut Context<Root>)>;
type U8Setter = Rc<dyn Fn(&mut Root, u8, &mut Context<Root>)>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PasswordClass {
    Uppercase,
    Lowercase,
    Numbers,
    Special,
}

impl PasswordClass {
    fn enabled(self, options: &PasswordOptions) -> bool {
        match self {
            Self::Uppercase => options.uppercase,
            Self::Lowercase => options.lowercase,
            Self::Numbers => options.numbers,
            Self::Special => options.special,
        }
    }

    fn set(self, options: &mut PasswordOptions, enabled: bool) {
        match self {
            Self::Uppercase => options.uppercase = enabled,
            Self::Lowercase => options.lowercase = enabled,
            Self::Numbers => options.numbers = enabled,
            Self::Special => options.special = enabled,
        }
    }

    fn others_enabled(self, options: &PasswordOptions) -> u8 {
        [
            Self::Uppercase,
            Self::Lowercase,
            Self::Numbers,
            Self::Special,
        ]
        .into_iter()
        .filter(|class| *class != self)
        .filter(|class| class.enabled(options))
        .count() as u8
    }
}

impl Root {
    pub(super) fn open_generator(
        &mut self,
        from_editor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let password = &mut self.settings.generator.password;
        password.length = effective_password_length(password);
        let (value, problem) = match kura_core::generate(&self.settings.generator) {
            Ok(value) => (value, None),
            Err(err) => (Zeroizing::new(String::new()), Some(Problem::Generator(err))),
        };
        let focus = cx.focus_handle();
        self.generator = Some(GeneratorPane {
            from_editor,
            value,
            problem,
            focus: focus.clone(),
        });
        window.defer(cx, move |window, cx| focus.focus(window, cx));
        cx.notify();
    }

    pub(super) fn close_generator(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        let Some(pane) = self.generator.take() else {
            return;
        };
        if pane.from_editor {
            if let Some(password) = self.editor.as_ref().map(|editor| editor.password.clone()) {
                self.in_window(window, cx, move |window, cx| {
                    password.update(cx, |input, cx| input.focus_handle(cx).focus(window, cx));
                });
            }
        } else {
            self.focus_after_modal(window, cx);
        }
        cx.notify();
    }

    fn regenerate(&mut self, cx: &mut Context<Self>) {
        let result = kura_core::generate(&self.settings.generator);
        let Some(pane) = self.generator.as_mut() else {
            return;
        };
        match result {
            Ok(value) => {
                pane.value = value;
                pane.problem = None;
            }
            Err(err) => {
                pane.value = Zeroizing::new(String::new());
                pane.problem = Some(Problem::Generator(err));
            }
        }
        cx.notify();
    }

    fn update_generator(
        &mut self,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut kura_core::GeneratorOptions),
    ) {
        update(&mut self.settings.generator);
        let password = &mut self.settings.generator.password;
        password.length = effective_password_length(password);
        self.save_settings(cx);
        self.regenerate(cx);
        cx.notify();
    }

    fn set_generator_mode(&mut self, mode: GeneratorMode, cx: &mut Context<Self>) {
        self.update_generator(cx, |options| options.mode = mode);
    }

    fn set_generator_length(&mut self, value: u8, cx: &mut Context<Self>) {
        self.update_generator(cx, |options| options.password.length = value);
    }

    fn set_generator_min_number(&mut self, value: u8, cx: &mut Context<Self>) {
        self.update_generator(cx, |options| {
            options.password.min_number = value.clamp(1, MAX_MIN_CHARS);
        });
    }

    fn set_generator_min_special(&mut self, value: u8, cx: &mut Context<Self>) {
        self.update_generator(cx, |options| {
            options.password.min_special = value.clamp(1, MAX_MIN_CHARS);
        });
    }

    fn set_generator_class(&mut self, class: PasswordClass, enabled: bool, cx: &mut Context<Self>) {
        if !enabled && class.others_enabled(&self.settings.generator.password) == 0 {
            cx.notify();
            return;
        }
        self.update_generator(cx, |options| class.set(&mut options.password, enabled));
    }

    fn set_generator_avoid_ambiguous(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.update_generator(cx, |options| options.password.avoid_ambiguous = enabled);
    }

    fn set_generator_words(&mut self, value: u8, cx: &mut Context<Self>) {
        self.update_generator(cx, |options| {
            options.passphrase.words = value.clamp(MIN_PASSPHRASE_WORDS, MAX_PASSPHRASE_WORDS);
        });
    }

    fn set_generator_separator(&mut self, separator: &'static str, cx: &mut Context<Self>) {
        self.update_generator(cx, |options| {
            options.passphrase.separator = separator.to_string();
        });
    }

    fn set_generator_capitalize(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.update_generator(cx, |options| options.passphrase.capitalize = enabled);
    }

    fn set_generator_include_number(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.update_generator(cx, |options| options.passphrase.include_number = enabled);
    }

    pub(super) fn use_generator(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.generator.as_mut() else {
            return;
        };
        if !pane.from_editor || pane.value.is_empty() {
            return;
        }
        let value = pane.value.clone();
        let Some(password) = self.editor.as_ref().map(|editor| editor.password.clone()) else {
            return;
        };
        password.update(cx, |input, cx| input.set_value(value.as_str(), window, cx));
        self.close_generator(Some(window), cx);
    }

    pub(super) fn copy_generator(&mut self, cx: &mut Context<Self>) {
        let Some(pane) = self.generator.take() else {
            return;
        };
        if self.copy_value(pane.value.as_str(), cx) {
            self.focus_after_modal(None, cx);
        } else {
            // A failed clipboard write must not take the value with it.
            self.generator = Some(pane);
        }
        cx.notify();
    }

    pub(super) fn render_generator(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(pane) = self.generator.as_ref() else {
            return div();
        };
        let max_height = window.viewport_size().height - px(48.);
        let focus = pane.focus.clone();
        let from_editor = pane.from_editor;
        // shortcut: the value is also plain text in the widget tree while the
        // dialog is open. `Zeroizing` covers the pane copy only.
        let value = pane.value.to_string();
        let value_empty = value.is_empty();
        let problem = pane
            .problem
            .as_ref()
            .map(|problem| super::problem_text(Some(problem)));
        let options = self.settings.generator.clone();
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let danger = cx.theme().danger;
        let overlay = modal_overlay().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| this.close_generator(Some(window), cx)),
        );

        let mode = ToggleGroup::new("generator-mode")
            .segmented()
            .single()
            .on_click(
                cx.listener(|this, checks: &Vec<bool>, _, cx| match checks.as_slice() {
                    [true, false] => this.set_generator_mode(GeneratorMode::Password, cx),
                    [false, true] => this.set_generator_mode(GeneratorMode::Passphrase, cx),
                    _ => {}
                }),
            )
            .child(
                Toggle::new("generator-mode-password")
                    .label(t!("generator.mode_password"))
                    .checked(options.mode == GeneratorMode::Password),
            )
            .child(
                Toggle::new("generator-mode-passphrase")
                    .label(t!("generator.mode_passphrase"))
                    .checked(options.mode == GeneratorMode::Passphrase),
            );

        let password_options = v_flex()
            .gap_3()
            .child(stepper_row(
                "generator-length",
                t!("generator.length").to_string(),
                options.password.length,
                minimum_password_length(&options.password),
                MAX_PASSWORD_LENGTH,
                true,
                cx,
                Rc::new(Root::set_generator_length),
            ))
            .child(stepper_row(
                "generator-min-number",
                t!("generator.min_numbers").to_string(),
                options.password.min_number,
                1,
                MAX_MIN_CHARS,
                options.password.numbers,
                cx,
                Rc::new(Root::set_generator_min_number),
            ))
            .child(stepper_row(
                "generator-min-special",
                t!("generator.min_special").to_string(),
                options.password.min_special,
                1,
                MAX_MIN_CHARS,
                options.password.special,
                cx,
                Rc::new(Root::set_generator_min_special),
            ))
            .child(
                h_flex()
                    .gap_6()
                    .child(class_switch(
                        "generator-uppercase",
                        PasswordClass::Uppercase,
                        &options.password,
                        cx,
                    ))
                    .child(class_switch(
                        "generator-lowercase",
                        PasswordClass::Lowercase,
                        &options.password,
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .gap_6()
                    .child(class_switch(
                        "generator-numbers",
                        PasswordClass::Numbers,
                        &options.password,
                        cx,
                    ))
                    .child(class_switch(
                        "generator-special",
                        PasswordClass::Special,
                        &options.password,
                        cx,
                    )),
            )
            .child(switch_row(
                "generator-avoid-ambiguous",
                t!("generator.avoid_ambiguous").to_string(),
                options.password.avoid_ambiguous,
                false,
                muted,
                cx,
                Rc::new(Root::set_generator_avoid_ambiguous),
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("generator.ambiguous_hint")),
            );

        let separator = ToggleGroup::new("generator-separator")
            .segmented()
            .single()
            .on_click(cx.listener(|this, checks: &Vec<bool>, _, cx| {
                if let Some(index) = checks.iter().position(|checked| *checked) {
                    this.set_generator_separator(SEPARATORS[index], cx);
                }
            }))
            .children(SEPARATORS.into_iter().enumerate().map(|(index, choice)| {
                let name = if choice == " " {
                    t!("generator.separator_space").to_string()
                } else {
                    choice.to_string()
                };
                Toggle::new(SharedString::from(format!("generator-separator-{index}")))
                    .label(name.clone())
                    .tooltip(t!("generator.separator_choice", choice = name))
                    .checked(options.passphrase.separator == choice)
            }));

        let passphrase_options = v_flex()
            .gap_3()
            .child(stepper_row(
                "generator-words",
                t!("generator.words").to_string(),
                options.passphrase.words,
                MIN_PASSPHRASE_WORDS,
                MAX_PASSPHRASE_WORDS,
                true,
                cx,
                Rc::new(Root::set_generator_words),
            ))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child(t!("generator.separator")),
                    )
                    .child(separator),
            )
            .child(switch_row(
                "generator-capitalize",
                t!("generator.capitalize").to_string(),
                options.passphrase.capitalize,
                false,
                muted,
                cx,
                Rc::new(Root::set_generator_capitalize),
            ))
            .child(switch_row(
                "generator-include-number",
                t!("generator.include_number").to_string(),
                options.passphrase.include_number,
                false,
                muted,
                cx,
                Rc::new(Root::set_generator_include_number),
            ));

        let primary = if from_editor {
            Button::new("generator-use")
                .primary()
                .icon(AssetIcon::Check)
                .label(t!("generator.use"))
                .disabled(value_empty)
                .on_click(cx.listener(|this, _, window, cx| this.use_generator(window, cx)))
        } else {
            Button::new("generator-copy")
                .primary()
                .icon(AssetIcon::Copy)
                .label(t!("generator.copy"))
                .disabled(value_empty)
                .on_click(cx.listener(|this, _, _, cx| this.copy_generator(cx)))
        };
        let cancel = if from_editor {
            Button::new("generator-cancel")
                .ghost()
                .label(t!("common.cancel"))
                .on_click(cx.listener(|this, _, window, cx| this.close_generator(Some(window), cx)))
        } else {
            Button::new("generator-close")
                .ghost()
                .label(t!("detail.close"))
                .on_click(cx.listener(|this, _, window, cx| this.close_generator(Some(window), cx)))
        };

        let card = v_flex()
            .id("generator-dialog")
            .w(px(520.))
            .max_w_full()
            .max_h(max_height)
            .role(Role::Dialog)
            .aria_label(t!("generator.title"))
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
                            .child(t!("generator.title")),
                    )
                    .child(
                        Button::new("generator-close-x")
                            .ghost()
                            .icon(AssetIcon::X)
                            .tooltip(t!("detail.close"))
                            .accessibility_label(t!("detail.close"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_generator(Some(window), cx)
                            })),
                    ),
            )
            .child(mode)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("generator-value")
                            .test_support()
                            .flex_1()
                            .min_w_0()
                            .font_family("monospace")
                            .text_sm()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .bg(cx.theme().muted)
                            .child(value),
                    )
                    .child(
                        Button::new("generator-regenerate")
                            .ghost()
                            .icon(AssetIcon::RefreshCw)
                            .tooltip(t!("generator.regenerate"))
                            .accessibility_label(t!("generator.regenerate"))
                            .on_click(cx.listener(|this, _, _, cx| this.regenerate(cx))),
                    ),
            )
            .child(
                v_flex()
                    .id("generator-body")
                    .flex_1()
                    .min_h_0()
                    .pr_4()
                    .gap_3()
                    .overflow_y_scrollbar()
                    .child(if options.mode == GeneratorMode::Password {
                        password_options.into_any_element()
                    } else {
                        passphrase_options.into_any_element()
                    })
                    .when_some(problem, |this, problem| {
                        this.child(
                            div()
                                .id("generator-problem")
                                .role(Role::Alert)
                                .test_support()
                                .aria_label(problem.clone())
                                .text_sm()
                                .text_color(danger)
                                .child(problem),
                        )
                    }),
            )
            .child(h_flex().justify_end().gap_2().child(cancel).child(primary));

        overlay.child(card.focus_trap("generator-dialog-trap", &focus))
    }
}

fn class_switch(
    id: &'static str,
    class: PasswordClass,
    options: &PasswordOptions,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let label = match class {
        PasswordClass::Uppercase => t!("generator.uppercase").to_string(),
        PasswordClass::Lowercase => t!("generator.lowercase").to_string(),
        PasswordClass::Numbers => t!("generator.numbers").to_string(),
        PasswordClass::Special => t!("generator.symbols").to_string(),
    };
    let checked = class.enabled(options);
    let muted = cx.theme().muted_foreground;
    switch_row(
        id,
        label,
        checked,
        false,
        muted,
        cx,
        Rc::new(move |this, enabled, cx| this.set_generator_class(class, enabled, cx)),
    )
}

fn switch_row(
    id: &'static str,
    label: String,
    checked: bool,
    disabled: bool,
    muted: Hsla,
    cx: &mut Context<Root>,
    on_click: BoolSetter,
) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap_2()
        .child(
            Switch::new(id)
                .checked(checked)
                .disabled(disabled)
                .accessibility_label(label.clone())
                .on_click(cx.listener(move |this, requested: &bool, _, cx| {
                    on_click(this, *requested, cx);
                })),
        )
        .child(div().text_sm().text_color(muted).child(label))
}

#[allow(clippy::too_many_arguments)]
fn stepper_row(
    id: &'static str,
    label: String,
    value: u8,
    min: u8,
    max: u8,
    enabled: bool,
    cx: &mut Context<Root>,
    set: U8Setter,
) -> impl IntoElement {
    let value = value.clamp(min, max);
    let muted = cx.theme().muted_foreground;
    let decrease = set.clone();
    let increase = set;
    h_flex()
        .items_center()
        .justify_between()
        .gap_4()
        .child(div().text_sm().text_color(muted).child(label))
        .child(
            h_flex()
                .items_center()
                .gap_1()
                .child(
                    Button::new(SharedString::from(format!("{id}-minus")))
                        .ghost()
                        .compact()
                        .icon(AssetIcon::Minus)
                        .tooltip(t!("generator.decrease"))
                        .accessibility_label(t!("generator.decrease"))
                        .disabled(!enabled || value <= min)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            decrease(this, value.saturating_sub(1).max(min), cx);
                        })),
                )
                .child(
                    div()
                        .w(px(28.))
                        .text_center()
                        .text_sm()
                        .child(value.to_string()),
                )
                .child(
                    Button::new(SharedString::from(format!("{id}-plus")))
                        .ghost()
                        .compact()
                        .icon(AssetIcon::Plus)
                        .tooltip(t!("generator.increase"))
                        .accessibility_label(t!("generator.increase"))
                        .disabled(!enabled || value >= max)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            increase(this, value.saturating_add(1).min(max), cx);
                        })),
                ),
        )
}
