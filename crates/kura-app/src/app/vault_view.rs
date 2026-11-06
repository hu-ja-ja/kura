//! The vault page: header, search, filters and the item list.

use std::rc::Rc;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::*;
use gpui_kit::component::input::Input;
use gpui_kit::component::list::List;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use kura_core::{ItemKind, VaultFilter};
use rust_i18n::t;

use super::{ClickHandler, Page, Root, problem_text};
use crate::list::kind_label;
const TYPE_FILTERS: &[ItemKind] = &[
    ItemKind::Login,
    ItemKind::Card,
    ItemKind::Identity,
    ItemKind::SecureNote,
    ItemKind::SshKey,
    ItemKind::Other,
];
/// One folder in the filter menu. Built before the menu so the entries can
/// borrow the root's listeners.
struct FolderChoice {
    label: String,
    checked: bool,
    on_click: ClickHandler,
}
impl Root {
    pub(super) fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
    fn render_search(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_query = !self.search.read(cx).value().is_empty();
        h_flex()
            .items_center()
            .gap_3()
            .px_4()
            .py_2()
            .child(
                Input::new(&self.search)
                    .id("vault-search")
                    .aria_label(t!("vault.search"))
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
                                    // `set_value` does not emit a change event.
                                    this.list.update(cx, |list, cx| {
                                        list.set_query("", window, cx);
                                    });
                                    this.focus_search(window, cx);
                                    cx.notify();
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

    pub(super) fn render_vault(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
