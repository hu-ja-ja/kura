//! The detail dialog: the decrypted item and its row actions.

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::*;
use gpui_kit::component::input::Textarea;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use rust_i18n::t;

use super::{Root, problem_text};
use crate::detail::DetailRow;
use crate::list::kind_label;
const MASK: &str = "••••••••••••";
impl Root {
    pub(super) fn render_detail_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let danger = cx.theme().danger;
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let max_height = window.viewport_size().height - px(64.);
        let card_height = (window.viewport_size().height * 0.8).min(max_height);

        let rows = self.detail.rows.clone();
        let data = self.detail.item.as_ref().map(|detail| {
            let folder = detail.folder_id.as_ref().and_then(|id| {
                self.snapshot
                    .folders
                    .iter()
                    .find(|folder| &folder.id == id)
                    .map(|folder| folder.name.clone())
            });
            (detail.name.clone(), detail.kind, detail.favorite, folder)
        });
        let totp_remaining = self
            .detail
            .item
            .as_ref()
            .and_then(|detail| detail.totp.as_ref())
            .map(|totp| totp.remaining);
        let detail_problem = self
            .detail
            .problem
            .as_ref()
            .map(|problem| problem_text(Some(problem)));
        let loading = self.detail.loading;

        let overlay = super::modal_overlay().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| this.close_detail(window, cx)),
        );

        let card = v_flex()
            .id("detail-dialog")
            .w(px(560.))
            .max_w_full()
            .max_h(max_height)
            .role(Role::Dialog)
            .overflow_hidden()
            .bg(cx.theme().background)
            .border_1()
            .border_color(border)
            .rounded_lg()
            .p_5()
            .gap_4()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        let Some((name, kind, favorite, folder)) = data else {
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
                if let Some(button) = retry_button(self.detail.selected_id.clone(), cx) {
                    body = body.child(button);
                }
            }
            return overlay.child(
                card.aria_label(title.clone())
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(div().text_lg().font_weight(FontWeight::BOLD).child(title))
                            .child(close_button(cx)),
                    )
                    .child(body)
                    .focus_trap("detail-dialog-trap", &self.detail.focus),
            );
        };

        let meta = if let Some(folder) = &folder {
            format!("{} · {folder}", kind_label(kind))
        } else {
            kind_label(kind)
        };

        let close = close_button(cx);

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
                                    .child(name.clone()),
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

        let mut card = card.aria_label(name).h(card_height).child(header);

        if let Some(problem) = detail_problem {
            let mut banner = h_flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().text_sm().text_color(danger).child(problem));
            if let Some(button) = retry_button(self.detail.selected_id.clone(), cx) {
                banner = banner.child(button);
            }
            card = card.child(banner);
        }

        let mut children = Vec::new();
        for (ix, row) in rows.iter().enumerate() {
            children.push(
                self.render_detail_row(ix, row, totp_remaining, cx)
                    .into_any_element(),
            );
        }
        card = card.child(
            v_flex()
                .flex_1()
                .min_h_0()
                // The scrollbar overlays the right edge, so rows keep clear of it.
                .pr_4()
                .overflow_y_scrollbar()
                .gap_4()
                .children(children),
        );

        overlay.child(card.focus_trap("detail-dialog-trap", &self.detail.focus))
    }

    fn render_detail_row(
        &mut self,
        ix: usize,
        row: &DetailRow,
        totp_remaining: Option<u32>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let revealed = !row.secret || self.detail.revealed.contains(&row.id);
        let value = if revealed {
            row.value.as_str().to_string()
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
            let row_id = row.id.clone();
            actions = actions.child(
                Button::new(SharedString::from(format!("reveal-{ix}")))
                    .ghost()
                    .icon(icon)
                    .tooltip(tooltip.clone())
                    .accessibility_label(tooltip)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.detail.revealed.remove(&row_id) {
                            this.detail.revealed.insert(row_id.clone());
                        }
                        cx.notify();
                    })),
            );
        }
        if row.copyable {
            let row_id = row.id.clone();
            actions = actions.child(
                Button::new(SharedString::from(format!("copy-{ix}")))
                    .ghost()
                    .icon(IconName::Copy)
                    .tooltip(t!("detail.copy"))
                    .accessibility_label(t!("detail.copy"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.copy_detail_row(&row_id, cx);
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
            Textarea::new(&self.detail.notes)
                .readonly(true)
                .appearance(false)
                .bordered(false)
                .aria_label(t!("detail.notes"))
                .text_sm()
                .w_full()
                .into_any_element()
        } else {
            div()
                .id(SharedString::from(format!("value-{}", row.id)))
                .test_support()
                .text_sm()
                .when(row.large, |this| {
                    this.text_lg().font_weight(FontWeight::MEDIUM)
                })
                .whitespace_normal()
                .child(value)
                .into_any_element()
        };

        let suffix = if row.large {
            totp_remaining.map(|seconds| t!("detail.totp_remaining", seconds = seconds).to_string())
        } else {
            None
        };
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(row.label.clone()),
            )
            .child(
                h_flex()
                    .gap_2()
                    // Wrapped values keep their first line at the top.
                    .items_start()
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
}

/// The close button, shared by the dialog's loading, empty and full states.
fn close_button(cx: &Context<Root>) -> Button {
    Button::new("detail-close")
        .ghost()
        .icon(AssetIcon::X)
        .tooltip(t!("detail.close"))
        .accessibility_label(t!("detail.close"))
        .on_click(cx.listener(|this, _, window, cx| this.close_detail(window, cx)))
}

/// The retry button, shown when a detail request failed.
fn retry_button(id: Option<String>, cx: &Context<Root>) -> Option<Button> {
    id.map(|id| {
        Button::new("retry-detail")
            .secondary()
            .compact()
            .label(t!("common.retry"))
            .on_click(cx.listener(move |this, _, _, _| {
                this.request_detail(id.clone());
            }))
    })
}
