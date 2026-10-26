//! The vault list.
//!
//! The list component does the virtual scrolling; the delegate only says how
//! many rows there are and how a row looks.

use std::sync::Arc;

use gpui_kit::component::list::{ListDelegate, ListItem, ListState};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use kura_core::{ItemKind, VaultFilter, VaultItem, VaultSnapshot, filter_indices};
use rust_i18n::t;

pub struct VaultDelegate {
    snapshot: Arc<VaultSnapshot>,
    /// Indices into the snapshot, in the order they are shown.
    visible: Vec<usize>,
    query: String,
    filter: VaultFilter,
    loading: bool,
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
        }
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
        _ix: Option<IndexPath>,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) {
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let item = self.snapshot.items.get(*self.visible.get(ix.row)?)?;
        Some(row(item, ix, cx))
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

fn row(item: &VaultItem, ix: IndexPath, cx: &mut Context<ListState<VaultDelegate>>) -> ListItem {
    ListItem::new(ix).h(px(44.)).child(
        h_flex()
            .w_full()
            .items_center()
            .gap_3()
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
                            .text_color(cx.theme().muted_foreground)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(item.subtitle.clone()),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(kind_label(item.kind)),
            )
            .when(item.favorite, |this| {
                this.child(
                    Icon::new(IconName::StarFill)
                        .xsmall()
                        .text_color(cx.theme().warning),
                )
            }),
    )
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
