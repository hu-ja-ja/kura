//! The add/edit form and the move-to-trash confirmation.

use std::rc::Rc;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use kura_core::{
    LoginDraft, LoginEditor, LoginFieldDraft, LoginFieldKind, LoginUriDraft, UriMatch,
};
use rust_i18n::t;
use zeroize::Zeroizing;

use super::{ClickHandler, DeleteConfirm, EditorField, EditorPane, EditorUri, Root};
use crate::backend::Command;

struct EditorChoice {
    label: String,
    active: bool,
    on_click: ClickHandler,
}

impl EditorPane {
    pub(super) fn load(&mut self, editor: LoginEditor, window: &mut Window, cx: &mut App) {
        self.id = Some(editor.id);
        self.revision = Some(editor.revision);
        self.loading = false;
        self.saving = false;
        self.problem = None;
        let draft = editor.draft;
        set_input(&self.name, &draft.name, window, cx);
        set_input(&self.username, &draft.username, window, cx);
        set_input(&self.password, &draft.password, window, cx);
        set_input(&self.totp, &draft.totp, window, cx);
        self.notes.update(cx, |notes, cx| {
            notes.set_value(draft.notes.as_str(), window, cx)
        });
        self.uris = draft
            .uris
            .into_iter()
            .map(|uri| EditorUri {
                uri: input(window, cx, &uri.uri, false),
                match_type: uri.match_type,
            })
            .collect();
        self.fields = draft
            .fields
            .into_iter()
            .map(|field| {
                let masked = field.kind == LoginFieldKind::Hidden;
                let value = if field.kind == LoginFieldKind::Boolean {
                    normalize_boolean(&field.value)
                } else {
                    field.value.to_string()
                };
                EditorField {
                    name: input(window, cx, &field.name, false),
                    value: input(window, cx, &value, masked),
                    kind: field.kind,
                    base: Some(field),
                }
            })
            .collect();
        self.folder_id = draft.folder_id;
        self.favorite = draft.favorite;
        let focus = self.name.clone();
        window.defer(cx, move |window, cx| {
            focus.update(cx, |input, cx| input.focus_handle(cx).focus(window, cx));
        });
    }

    fn draft(&self, cx: &gpui_kit::App) -> LoginDraft {
        let fields = self
            .fields
            .iter()
            .map(|field| {
                let mut draft = field
                    .base
                    .clone()
                    .unwrap_or_else(|| LoginFieldDraft::new(field.kind));
                draft.name = field.name.read(cx).value().to_string();
                draft.value = Zeroizing::new(field.value.read(cx).value().to_string());
                draft.kind = field.kind;
                draft
            })
            .collect();
        let uris = self
            .uris
            .iter()
            .map(|uri| LoginUriDraft {
                uri: uri.uri.read(cx).value().to_string(),
                match_type: uri.match_type,
            })
            .collect();
        LoginDraft {
            name: self.name.read(cx).value().to_string(),
            username: self.username.read(cx).value().to_string(),
            password: Zeroizing::new(self.password.read(cx).value().to_string()),
            totp: Zeroizing::new(self.totp.read(cx).value().to_string()),
            uris,
            notes: Zeroizing::new(self.notes.read(cx).value().to_string()),
            folder_id: self.folder_id.clone(),
            favorite: self.favorite,
            fields,
        }
    }
}

fn input(window: &mut Window, cx: &mut App, value: &str, masked: bool) -> Entity<InputState> {
    cx.new(|cx| {
        let mut input = InputState::new(window, cx).masked(masked);
        input.set_value(value, window, cx);
        input
    })
}

fn set_input(input: &Entity<InputState>, value: &str, window: &mut Window, cx: &mut App) {
    input.update(cx, |input, cx| input.set_value(value, window, cx));
}

impl Root {
    fn next_write_generation() -> u64 {
        super::NEXT_WRITE_GENERATION.fetch_add(1, super::Ordering::Relaxed)
    }

    fn make_editor(
        &mut self,
        id: Option<String>,
        generation: u64,
        loading: bool,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> EditorPane {
        let name = input(window, cx, "", false);
        let username = input(window, cx, "", false);
        let password = input(window, cx, "", true);
        let totp = input(window, cx, "", true);
        let notes = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 300));
        let name_subscription =
            cx.subscribe_in(&name, window, |_this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            });
        let focus = name.read(cx).focus_handle(cx);
        EditorPane {
            id,
            revision: None,
            generation,
            loading,
            saving: false,
            problem: None,
            name,
            username,
            password,
            totp,
            notes,
            uris: Vec::new(),
            fields: Vec::new(),
            folder_id: None,
            favorite: false,
            focus,
            _name_subscription: name_subscription,
        }
    }

    pub(super) fn open_new_item(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let generation = Self::next_write_generation();
        let pane = self.make_editor(None, generation, false, window, cx);
        self.editor = Some(pane);
        self.delete_confirm = None;
        let focus = self
            .editor
            .as_ref()
            .map(|pane| pane.name.clone())
            .expect("the new editor is open");
        window.defer(cx, move |window, cx| {
            focus.update(cx, |input, cx| input.focus_handle(cx).focus(window, cx));
        });
        cx.notify();
    }

    pub(super) fn request_editor(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        let generation = Self::next_write_generation();
        let pane = self.make_editor(Some(id.clone()), generation, true, window, cx);
        self.editor = Some(pane);
        self.delete_confirm = None;
        self.backend.send(Command::Editor { id, generation });
        cx.notify();
    }

    pub(super) fn close_editor(&mut self, window: Option<&mut Window>, cx: &mut Context<Root>) {
        if self.editor.as_ref().is_some_and(|pane| pane.saving) {
            return;
        }
        self.editor = None;
        self.focus_after_modal(window, cx);
        cx.notify();
    }

    pub(super) fn save_editor(&mut self, cx: &mut Context<Root>) {
        let Some(pane) = self.editor.as_mut() else {
            return;
        };
        if pane.loading || pane.saving || pane.name.read(cx).value().trim().is_empty() {
            return;
        }
        pane.saving = true;
        pane.problem = None;
        let draft = pane.draft(cx);
        let id = pane.id.clone();
        let revision = pane.revision.clone();
        let generation = pane.generation;
        self.backend.send(Command::Save {
            id,
            revision,
            draft,
            generation,
        });
        cx.notify();
    }

    pub(super) fn add_uri_row(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let Some(pane) = self.editor.as_mut().filter(|pane| !pane.saving) else {
            return;
        };
        pane.uris.push(EditorUri {
            uri: input(window, cx, "", false),
            match_type: None,
        });
        cx.notify();
    }

    pub(super) fn remove_uri_row(&mut self, ix: usize, cx: &mut Context<Root>) {
        if let Some(pane) = self.editor.as_mut().filter(|pane| !pane.saving)
            && ix < pane.uris.len()
        {
            pane.uris.remove(ix);
            cx.notify();
        }
    }

    pub(super) fn set_uri_match(
        &mut self,
        ix: usize,
        match_type: Option<UriMatch>,
        cx: &mut Context<Root>,
    ) {
        if let Some(pane) = self.editor.as_mut().filter(|pane| !pane.saving)
            && let Some(uri) = pane.uris.get_mut(ix)
        {
            uri.match_type = match_type;
            cx.notify();
        }
    }

    pub(super) fn add_field_row(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        let Some(pane) = self.editor.as_mut().filter(|pane| !pane.saving) else {
            return;
        };
        pane.fields.push(EditorField {
            name: input(window, cx, "", false),
            value: input(window, cx, "", false),
            kind: LoginFieldKind::Text,
            base: None,
        });
        cx.notify();
    }

    pub(super) fn remove_field_row(&mut self, ix: usize, cx: &mut Context<Root>) {
        if let Some(pane) = self.editor.as_mut().filter(|pane| !pane.saving)
            && ix < pane.fields.len()
        {
            pane.fields.remove(ix);
            cx.notify();
        }
    }

    pub(super) fn set_field_kind(
        &mut self,
        ix: usize,
        kind: LoginFieldKind,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        let Some(pane) = self.editor.as_mut().filter(|pane| !pane.saving) else {
            return;
        };
        let Some(field) = pane.fields.get_mut(ix) else {
            return;
        };
        field.kind = kind;
        let want_masked = kind == LoginFieldKind::Hidden;
        field.value.update(cx, |value, cx| {
            let current = value.value().to_string();
            let normalized = match kind {
                LoginFieldKind::Boolean => Some(normalize_boolean(&current)),
                _ => None,
            };
            if let Some(normalized) = normalized
                && normalized != current
            {
                value.set_value(normalized, window, cx);
            }
            if value.presentation().is_masked() != want_masked {
                value.toggle_masked(window, cx);
            }
        });
        cx.notify();
    }

    pub(super) fn set_field_boolean(
        &mut self,
        ix: usize,
        value: bool,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        let Some(pane) = self.editor.as_mut().filter(|pane| !pane.saving) else {
            return;
        };
        let Some(field) = pane.fields.get_mut(ix) else {
            return;
        };
        field.value.update(cx, |input, cx| {
            input.set_value(if value { "true" } else { "false" }, window, cx);
        });
        cx.notify();
    }

    pub(super) fn toggle_editor_secret(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        if let Some(pane) = self.editor.as_mut().filter(|pane| !pane.saving)
            && let Some(field) = pane.fields.get(ix)
        {
            field
                .value
                .update(cx, |input, cx| input.toggle_masked(window, cx));
            cx.notify();
        }
    }

    pub(super) fn set_editor_folder(&mut self, folder_id: Option<String>, cx: &mut Context<Root>) {
        if let Some(pane) = self.editor.as_mut().filter(|pane| !pane.saving) {
            pane.folder_id = folder_id;
            cx.notify();
        }
    }

    pub(super) fn toggle_editor_favorite(&mut self, favorite: bool, cx: &mut Context<Root>) {
        if let Some(pane) = self.editor.as_mut().filter(|pane| !pane.saving) {
            pane.favorite = favorite;
            cx.notify();
        }
    }

    pub(super) fn open_delete_confirm(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        let generation = Self::next_write_generation();
        let focus = cx.focus_handle();
        let deferred = focus.clone();
        window.defer(cx, move |window, cx| deferred.focus(window, cx));
        self.delete_confirm = Some(DeleteConfirm {
            id,
            generation,
            busy: false,
            problem: None,
            focus,
        });
        self.editor = None;
        cx.notify();
    }

    pub(super) fn close_delete_confirm(
        &mut self,
        window: Option<&mut Window>,
        cx: &mut Context<Root>,
    ) {
        if self
            .delete_confirm
            .as_ref()
            .is_some_and(|dialog| dialog.busy)
        {
            return;
        }
        self.delete_confirm = None;
        self.focus_after_modal(window, cx);
        cx.notify();
    }

    pub(super) fn confirm_delete(&mut self, cx: &mut Context<Root>) {
        let Some(dialog) = self.delete_confirm.as_mut() else {
            return;
        };
        if dialog.busy {
            return;
        }
        dialog.busy = true;
        dialog.problem = None;
        let id = dialog.id.clone();
        let generation = dialog.generation;
        self.backend.send(Command::Delete { id, generation });
        cx.notify();
    }

    pub(super) fn focus_after_modal(&self, window: Option<&mut Window>, cx: &mut Context<Root>) {
        if self.detail.selected_id.is_some() {
            let detail = self.detail.focus.clone();
            if let Some(window) = window {
                detail.focus(window, cx);
            } else {
                self.in_window(None, cx, move |window, cx| {
                    detail.focus(window, cx);
                });
            }
            return;
        }
        let list = self.list.clone();
        if let Some(window) = window {
            list.update(cx, |list, cx| list.focus(window, cx));
        } else {
            self.in_window(None, cx, move |window, cx| {
                list.update(cx, |list, cx| list.focus(window, cx));
            });
        }
    }

    pub(super) fn render_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> impl IntoElement {
        let Some(pane) = &self.editor else {
            return div();
        };
        let max_height = window.viewport_size().height - px(48.);
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let border = cx.theme().border;
        let loading = pane.loading;
        let title = if loading {
            t!("editor.loading")
        } else if pane.id.is_some() {
            t!("editor.edit_title")
        } else {
            t!("editor.add_title")
        };
        let focus = pane.focus.clone();
        let saving = pane.saving;
        let overlay = super::modal_overlay().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| this.close_editor(Some(window), cx)),
        );

        if loading {
            let card = v_flex()
                .id("editor-dialog")
                .w(px(520.))
                .role(Role::Dialog)
                .aria_label(title.clone())
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
                        .child(div().text_lg().font_weight(FontWeight::BOLD).child(title))
                        .child(
                            Button::new("editor-close")
                                .ghost()
                                .icon(AssetIcon::X)
                                .tooltip(t!("detail.close"))
                                .accessibility_label(t!("detail.close"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_editor(Some(window), cx)
                                })),
                        ),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(t!("editor.loading_hint")),
                );
            return overlay.child(card.focus_trap("editor-dialog-trap", &focus));
        }

        let (name_empty, favorite, problem, name, username, password, totp, notes) = {
            let pane = self.editor.as_ref().expect("the editor is open");
            (
                pane.name.read(cx).value().trim().is_empty(),
                pane.favorite,
                pane.problem
                    .as_ref()
                    .map(|problem| super::problem_text(Some(problem))),
                pane.name.clone(),
                pane.username.clone(),
                pane.password.clone(),
                pane.totp.clone(),
                pane.notes.clone(),
            )
        };
        let uri_rows = self.render_uri_rows(cx);
        let field_rows = self.render_field_rows(cx);
        let folder_choices = self.editor_folder_choices(cx);
        let folder_label = self.editor_folder_label();

        let card = v_flex()
            .id("editor-dialog")
            .w(px(640.))
            .max_w_full()
            .max_h(max_height)
            .role(Role::Dialog)
            .aria_label(title.clone())
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
                    .child(div().text_lg().font_weight(FontWeight::BOLD).child(title))
                    .child(
                        Button::new("editor-close")
                            .ghost()
                            .icon(AssetIcon::X)
                            .tooltip(t!("detail.close"))
                            .accessibility_label(t!("detail.close"))
                            .disabled(saving)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_editor(Some(window), cx)
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("editor-body")
                    .flex_1()
                    .min_h_0()
                    .pr_4()
                    .gap_3()
                    .overflow_y_scrollbar()
                    .child(field(
                        t!("editor.name").to_string(),
                        Input::new(&name)
                            .id("editor-name")
                            .aria_label(t!("editor.name"))
                            .disabled(saving),
                        muted,
                    ))
                    .child(field(
                        t!("editor.username").to_string(),
                        Input::new(&username)
                            .id("editor-username")
                            .aria_label(t!("editor.username"))
                            .disabled(saving),
                        muted,
                    ))
                    .child(field(
                        t!("editor.password").to_string(),
                        Input::new(&password)
                            .id("editor-password")
                            .aria_label(t!("editor.password"))
                            .disabled(saving)
                            .suffix(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        Button::new("editor-password-generate")
                                            .ghost()
                                            .icon(AssetIcon::Dices)
                                            .tooltip(t!("editor.generate"))
                                            .accessibility_label(t!("editor.generate"))
                                            .disabled(saving)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.open_generator(true, window, cx)
                                            })),
                                    )
                                    .child(secret_toggle("editor-password-mask", saving, cx)),
                            ),
                        muted,
                    ))
                    .child(field(
                        t!("editor.totp").to_string(),
                        Input::new(&totp)
                            .id("editor-totp")
                            .aria_label(t!("editor.totp"))
                            .disabled(saving)
                            .suffix(secret_toggle("editor-totp-mask", saving, cx)),
                        muted,
                    ))
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                h_flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div().text_sm().text_color(muted).child(t!("editor.uris")),
                                    )
                                    .child(
                                        Button::new("editor-add-uri")
                                            .ghost()
                                            .compact()
                                            .icon(AssetIcon::Plus)
                                            .label(t!("editor.add_uri"))
                                            .disabled(saving)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.add_uri_row(window, cx)
                                            })),
                                    ),
                            )
                            .children(uri_rows),
                    )
                    .child(field(
                        t!("editor.notes").to_string(),
                        Textarea::new(&notes)
                            .aria_label(t!("editor.notes"))
                            .disabled(saving),
                        muted,
                    ))
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                h_flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(muted)
                                            .child(t!("editor.custom_fields")),
                                    )
                                    .child(
                                        Button::new("editor-add-field")
                                            .ghost()
                                            .compact()
                                            .icon(AssetIcon::Plus)
                                            .label(t!("editor.add_field"))
                                            .disabled(saving)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.add_field_row(window, cx)
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(t!("editor.linked_hint")),
                            )
                            .children(field_rows),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_4()
                            .child(
                                h_flex()
                                    .flex_1()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .child(div().text_sm().child(t!("editor.folder")))
                                    .child(editor_menu(
                                        "editor-folder",
                                        folder_label,
                                        folder_choices,
                                        saving,
                                    )),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Switch::new("editor-favorite")
                                            .checked(favorite)
                                            .disabled(saving)
                                            .accessibility_label(t!("editor.favorite"))
                                            .on_click(cx.listener(
                                                |this, requested: &bool, _, cx| {
                                                    this.toggle_editor_favorite(*requested, cx);
                                                },
                                            )),
                                    )
                                    .child(div().text_sm().child(t!("editor.favorite"))),
                            ),
                    )
                    .when_some(problem, |this, problem| {
                        this.child(
                            div()
                                .id("editor-problem")
                                .role(Role::Alert)
                                .text_sm()
                                .text_color(danger)
                                .child(problem),
                        )
                    }),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("editor-cancel")
                            .ghost()
                            .label(t!("common.cancel"))
                            .disabled(saving)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_editor(Some(window), cx)
                            })),
                    )
                    .child(
                        Button::new("editor-save")
                            .primary()
                            .label(t!("editor.save"))
                            .loading(saving)
                            .disabled(name_empty || saving)
                            .on_click(cx.listener(|this, _, _, cx| this.save_editor(cx))),
                    ),
            );

        overlay.child(card.focus_trap("editor-dialog-trap", &focus))
    }

    fn render_uri_rows(&mut self, cx: &mut Context<Root>) -> Vec<AnyElement> {
        let Some(pane) = &self.editor else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for (ix, row) in pane.uris.iter().enumerate() {
            let uri = row.uri.clone();
            let label = match_label(row.match_type);
            let choices = match_choices(ix, row.match_type, cx);
            rows.push(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Input::new(&uri)
                            .id(SharedString::from(format!("editor-uri-{ix}")))
                            .aria_label(t!("editor.uri"))
                            .disabled(self.editor.as_ref().is_some_and(|pane| pane.saving)),
                    )
                    .child(editor_menu(
                        SharedString::from(format!("editor-uri-match-{ix}")),
                        label,
                        choices,
                        self.editor.as_ref().is_some_and(|pane| pane.saving),
                    ))
                    .child(
                        Button::new(SharedString::from(format!("editor-uri-remove-{ix}")))
                            .ghost()
                            .icon(AssetIcon::X)
                            .tooltip(t!("editor.remove"))
                            .accessibility_label(t!("editor.remove"))
                            .disabled(self.editor.as_ref().is_some_and(|pane| pane.saving))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_uri_row(ix, cx);
                            })),
                    )
                    .into_any_element(),
            );
        }
        rows
    }

    fn render_field_rows(&mut self, cx: &mut Context<Root>) -> Vec<AnyElement> {
        let Some(pane) = &self.editor else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for (ix, field) in pane.fields.iter().enumerate() {
            let name = field.name.clone();
            let value = field.value.clone();
            let kind = field.kind;
            let type_label = field_kind_label(kind);
            let type_choices = field_kind_choices(ix, kind, cx);
            let value_element: AnyElement = match kind {
                LoginFieldKind::Text | LoginFieldKind::Hidden => {
                    let mut value_input = Input::new(&value)
                        .id(SharedString::from(format!("editor-field-value-{ix}")))
                        .aria_label(t!("editor.field_value"))
                        .disabled(self.editor.as_ref().is_some_and(|pane| pane.saving));
                    if kind == LoginFieldKind::Hidden {
                        value_input = value_input.suffix(
                            Button::new(SharedString::from(format!("editor-field-mask-{ix}")))
                                .ghost()
                                .icon(AssetIcon::Eye)
                                .tooltip(t!("detail.reveal"))
                                .accessibility_label(t!("detail.reveal"))
                                .disabled(self.editor.as_ref().is_some_and(|pane| pane.saving))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.toggle_editor_secret(ix, window, cx);
                                })),
                        );
                    }
                    value_input.into_any_element()
                }
                LoginFieldKind::Boolean => {
                    let checked = value.read(cx).value() == "true";
                    let field_name = name.read(cx).value().trim().to_string();
                    let label = if field_name.is_empty() {
                        t!("editor.custom_field").to_string()
                    } else {
                        field_name
                    };
                    Switch::new(SharedString::from(format!("editor-field-bool-{ix}")))
                        .checked(checked)
                        .disabled(self.editor.as_ref().is_some_and(|pane| pane.saving))
                        .accessibility_label(t!("editor.boolean_value", name = label))
                        .on_click(cx.listener(move |this, requested: &bool, window, cx| {
                            this.set_field_boolean(ix, *requested, window, cx);
                        }))
                        .into_any_element()
                }
            };
            rows.push(
                v_flex()
                    .gap_2()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Input::new(&name)
                                    .id(SharedString::from(format!("editor-field-name-{ix}")))
                                    .aria_label(t!("editor.field_name"))
                                    .disabled(self.editor.as_ref().is_some_and(|pane| pane.saving)),
                            )
                            .child(editor_menu(
                                SharedString::from(format!("editor-field-kind-{ix}")),
                                type_label,
                                type_choices,
                                self.editor.as_ref().is_some_and(|pane| pane.saving),
                            ))
                            .child(
                                Button::new(SharedString::from(format!(
                                    "editor-field-remove-{ix}"
                                )))
                                .ghost()
                                .icon(AssetIcon::X)
                                .tooltip(t!("editor.remove"))
                                .accessibility_label(t!("editor.remove"))
                                .disabled(self.editor.as_ref().is_some_and(|pane| pane.saving))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.remove_field_row(ix, cx);
                                    },
                                )),
                            ),
                    )
                    .child(value_element)
                    .into_any_element(),
            );
        }
        rows
    }

    fn editor_folder_choices(&self, cx: &mut Context<Root>) -> Rc<Vec<EditorChoice>> {
        let mut choices = vec![EditorChoice {
            label: t!("editor.no_folder").to_string(),
            active: self
                .editor
                .as_ref()
                .is_none_or(|pane| pane.folder_id.is_none()),
            on_click: Rc::new(cx.listener(|this, _, _, cx| {
                this.set_editor_folder(None, cx);
            })),
        }];
        for folder in &self.snapshot.folders {
            let id = folder.id.clone();
            let active = self
                .editor
                .as_ref()
                .is_some_and(|pane| pane.folder_id.as_deref() == Some(id.as_str()));
            choices.push(EditorChoice {
                label: folder.name.clone(),
                active,
                on_click: Rc::new(cx.listener(move |this, _, _, cx| {
                    this.set_editor_folder(Some(id.clone()), cx);
                })),
            });
        }
        Rc::new(choices)
    }

    fn editor_folder_label(&self) -> String {
        let Some(pane) = &self.editor else {
            return String::new();
        };
        pane.folder_id
            .as_ref()
            .and_then(|id| {
                self.snapshot
                    .folders
                    .iter()
                    .find(|folder| &folder.id == id)
                    .map(|folder| folder.name.clone())
            })
            .unwrap_or_else(|| t!("editor.no_folder").to_string())
    }

    pub(super) fn render_delete_confirm(&mut self, cx: &mut Context<Root>) -> impl IntoElement {
        let Some(dialog) = &self.delete_confirm else {
            return div();
        };
        let focus = dialog.focus.clone();
        let busy = dialog.busy;
        let problem = dialog
            .problem
            .as_ref()
            .map(|problem| super::problem_text(Some(problem)));
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let border = cx.theme().border;
        let overlay = super::modal_overlay().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| this.close_delete_confirm(Some(window), cx)),
        );
        let card = v_flex()
            .id("delete-dialog")
            .w(px(440.))
            .max_w_full()
            .role(Role::Dialog)
            .aria_label(t!("editor.delete_title"))
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
                    .child(t!("editor.delete_title")),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t!("editor.delete_body")),
            )
            .when_some(problem, |this, problem| {
                this.child(
                    div()
                        .id("delete-problem")
                        .role(Role::Alert)
                        .text_sm()
                        .text_color(danger)
                        .child(problem),
                )
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("delete-cancel")
                            .ghost()
                            .label(t!("common.cancel"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_delete_confirm(Some(window), cx)
                            })),
                    )
                    .child(
                        Button::new("delete-confirm")
                            .danger()
                            .label(t!("editor.delete_confirm"))
                            .loading(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.confirm_delete(cx))),
                    ),
            );
        overlay.child(card.focus_trap("delete-dialog-trap", &focus))
    }
}

fn field(label: String, input: impl IntoElement, muted: Hsla) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_sm().text_color(muted).child(label))
        .child(input)
}

fn secret_toggle(id: &'static str, disabled: bool, cx: &mut Context<Root>) -> Button {
    Button::new(id)
        .ghost()
        .icon(AssetIcon::Eye)
        .tooltip(t!("detail.reveal"))
        .accessibility_label(t!("detail.reveal"))
        .disabled(disabled)
        .on_click(cx.listener(move |this, _, window, cx| {
            if let Some(pane) = this.editor.as_mut() {
                let input = if id == "editor-password-mask" {
                    pane.password.clone()
                } else {
                    pane.totp.clone()
                };
                input.update(cx, |input, cx| input.toggle_masked(window, cx));
                cx.notify();
            }
        }))
}

fn editor_menu(
    id: impl Into<ElementId>,
    label: String,
    choices: Rc<Vec<EditorChoice>>,
    disabled: bool,
) -> impl IntoElement {
    Button::new(id)
        .ghost()
        .compact()
        .label(label)
        .disabled(disabled)
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

fn match_choices(
    ix: usize,
    current: Option<UriMatch>,
    cx: &mut Context<Root>,
) -> Rc<Vec<EditorChoice>> {
    let mut choices = Vec::new();
    for value in [
        None,
        Some(UriMatch::Domain),
        Some(UriMatch::Host),
        Some(UriMatch::StartsWith),
        Some(UriMatch::Exact),
        Some(UriMatch::RegularExpression),
        Some(UriMatch::Never),
    ] {
        let active = current == value;
        choices.push(EditorChoice {
            label: match_label(value),
            active,
            on_click: Rc::new(cx.listener(move |this, _, _, cx| {
                this.set_uri_match(ix, value, cx);
            })),
        });
    }
    Rc::new(choices)
}

fn field_kind_choices(
    ix: usize,
    current: LoginFieldKind,
    cx: &mut Context<Root>,
) -> Rc<Vec<EditorChoice>> {
    let mut choices = Vec::new();
    for kind in [
        LoginFieldKind::Text,
        LoginFieldKind::Hidden,
        LoginFieldKind::Boolean,
    ] {
        let active = current == kind;
        choices.push(EditorChoice {
            label: field_kind_label(kind),
            active,
            on_click: Rc::new(cx.listener(move |this, _, window, cx| {
                this.set_field_kind(ix, kind, window, cx);
            })),
        });
    }
    Rc::new(choices)
}

fn match_label(value: Option<UriMatch>) -> String {
    match value {
        None => t!("editor.match_default"),
        Some(UriMatch::Domain) => t!("editor.match_domain"),
        Some(UriMatch::Host) => t!("editor.match_host"),
        Some(UriMatch::StartsWith) => t!("editor.match_starts_with"),
        Some(UriMatch::Exact) => t!("editor.match_exact"),
        Some(UriMatch::RegularExpression) => t!("editor.match_regex"),
        Some(UriMatch::Never) => t!("editor.match_never"),
    }
    .to_string()
}

fn field_kind_label(kind: LoginFieldKind) -> String {
    match kind {
        LoginFieldKind::Text => t!("editor.field_text"),
        LoginFieldKind::Hidden => t!("editor.field_hidden"),
        LoginFieldKind::Boolean => t!("editor.field_boolean"),
    }
    .to_string()
}

fn normalize_boolean(value: &str) -> String {
    if value.trim().is_empty() {
        String::new()
    } else if value.trim().eq_ignore_ascii_case("true") {
        "true".to_string()
    } else {
        "false".to_string()
    }
}
