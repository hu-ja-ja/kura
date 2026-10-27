//! The vault list.
//!
//! The list component does the virtual scrolling; the delegate only says how
//! many rows there are and how a row looks.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::list::{ListDelegate, ListItem, ListState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use kura_core::{ItemKind, VaultFilter, VaultItem, VaultSnapshot, filter_indices};
use rust_i18n::t;

type OpenHandler = Box<dyn Fn(&str, &mut Window, &mut App) + 'static>;
type CopyHandler = Rc<dyn Fn(&str, &mut Window, &mut App) + 'static>;

pub struct VaultDelegate {
    snapshot: Arc<VaultSnapshot>,
    /// Indices into the snapshot, in the order they are shown.
    visible: Vec<usize>,
    query: String,
    filter: VaultFilter,
    loading: bool,
    selected: Option<usize>,
    on_open: Option<OpenHandler>,
    on_copy_text: Option<CopyHandler>,
    on_copy_password: Option<CopyHandler>,
}

impl VaultDelegate {
    pub fn new(snapshot: Arc<VaultSnapshot>) -> Self {
        let visible = (0..snapshot.items.len()).collect();
        Self {
            snapshot,
            visible,
            query: String::new(),
            filter: VaultFilter::All,
            loading: false,
            selected: None,
            on_open: None,
            on_copy_text: None,
            on_copy_password: None,
        }
    }

    pub fn set_on_open(&mut self, on_open: impl Fn(&str, &mut Window, &mut App) + 'static) {
        self.on_open = Some(Box::new(on_open));
    }

    pub fn set_on_copy_text(&mut self, on_copy: impl Fn(&str, &mut Window, &mut App) + 'static) {
        self.on_copy_text = Some(Rc::new(on_copy));
    }

    pub fn set_on_copy_password(
        &mut self,
        on_copy: impl Fn(&str, &mut Window, &mut App) + 'static,
    ) {
        self.on_copy_password = Some(Rc::new(on_copy));
    }

    /// The id of the row the list currently highlights.
    pub fn selected_item_id(&self) -> Option<String> {
        self.selected
            .and_then(|row| self.visible.get(row))
            .and_then(|ix| self.snapshot.items.get(*ix))
            .map(|item| item.id.clone())
    }

    /// The visible row for an item, after a filter or sync changed the order.
    pub fn row_of(&self, id: &str) -> Option<usize> {
        self.visible.iter().position(|ix| {
            self.snapshot
                .items
                .get(*ix)
                .is_some_and(|item| item.id == id)
        })
    }

    pub fn set_snapshot(&mut self, snapshot: Arc<VaultSnapshot>) {
        self.snapshot = snapshot;
        self.rebuild();
    }

    pub fn set_filter(&mut self, filter: VaultFilter) {
        self.filter = filter;
        self.rebuild();
    }

    pub fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
    }

    fn rebuild(&mut self) {
        self.visible = filter_indices(&self.snapshot.items, &self.query, &self.filter);
    }
}

impl ListDelegate for VaultDelegate {
    type Item = ListItem;

    fn items_count(&self, _section: usize, _cx: &App) -> usize {
        self.visible.len()
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) {
        self.selected = ix.map(|ix| ix.row);
    }

    fn confirm(
        &mut self,
        _secondary: bool,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        let Some(row) = self.selected else {
            return;
        };
        let Some(item) = self
            .visible
            .get(row)
            .and_then(|ix| self.snapshot.items.get(*ix))
        else {
            return;
        };
        if let Some(on_open) = &self.on_open {
            on_open(&item.id, window, cx);
        }
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let item = self.snapshot.items.get(*self.visible.get(ix.row)?)?;
        Some(row(
            item,
            ix,
            self.on_copy_text.clone(),
            self.on_copy_password.clone(),
            cx,
        ))
    }

    fn perform_search(
        &mut self,
        query: &str,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.query = query.to_string();
        self.rebuild();
        cx.notify();
        Task::ready(())
    }

    fn loading(&self, _cx: &App) -> bool {
        self.loading
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        let (message, hint) = if self.snapshot.items.is_empty() {
            (t!("vault.empty"), t!("vault.empty_hint"))
        } else {
            (t!("vault.no_results"), t!("vault.no_results_hint"))
        };

        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                Icon::new(IconName::Inbox)
                    .large()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(div().text_sm().child(message))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(hint),
            )
    }
}

fn row(
    item: &VaultItem,
    ix: IndexPath,
    on_copy_text: Option<CopyHandler>,
    on_copy_password: Option<CopyHandler>,
    cx: &mut Context<ListState<VaultDelegate>>,
) -> ListItem {
    let muted = cx.theme().muted_foreground;
    let item_id = item.id.clone();
    let username = item.username.clone();
    let is_login = item.kind == ItemKind::Login;
    let has_username = !username.is_empty();
    let has_actions = has_username || (is_login && on_copy_password.is_some());

    let row = h_flex()
        .id(SharedString::from(format!("row-menu-{}", item.id)))
        .w_full()
        .h_full()
        .items_center()
        .gap_3()
        .border_b_1()
        .border_color(cx.theme().muted_foreground.opacity(0.25))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(item.name.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(item.subtitle.clone()),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child(kind_label(item.kind)),
        )
        .when(item.favorite, |this| {
            this.child(
                Icon::new(IconName::StarFill)
                    .with_size(gpui_kit::component::Size::Medium)
                    .text_color(cx.theme().warning),
            )
        });

    let row: AnyElement = if has_actions {
        row.context_menu(move |menu, _, _| {
            let mut menu = menu.min_w(px(190.));
            if let Some(handler) = on_copy_text.as_ref() {
                let handler = handler.clone();
                let value = username.clone();
                menu = menu.item(
                    PopupMenuItem::new(t!("vault.copy_username"))
                        .disabled(!has_username)
                        .on_click(move |_, window, cx| {
                            if has_username {
                                handler(&value, window, cx);
                            }
                        }),
                );
            }
            if is_login && let Some(handler) = on_copy_password.as_ref() {
                let handler = handler.clone();
                let id = item_id.clone();
                menu = menu.item(
                    PopupMenuItem::new(t!("vault.copy_password"))
                        .on_click(move |_, window, cx| handler(&id, window, cx)),
                );
            }
            menu
        })
        .into_any_element()
    } else {
        row.into_any_element()
    };

    ListItem::new(ix).h(px(44.)).child(row)
}

pub(crate) fn kind_label(kind: ItemKind) -> String {
    match kind {
        ItemKind::Login => t!("item_kind.login").to_string(),
        ItemKind::Card => t!("item_kind.card").to_string(),
        ItemKind::Identity => t!("item_kind.identity").to_string(),
        ItemKind::SecureNote => t!("item_kind.secure_note").to_string(),
        ItemKind::SshKey => t!("item_kind.ssh_key").to_string(),
        ItemKind::Other => t!("item_kind.other").to_string(),
    }
}
