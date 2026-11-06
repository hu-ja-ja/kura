//! The Sparkle-style update window.
//!
//! The layout follows the neutral controls of `gpui-auto-update` (MIT OR
//! Apache-2.0) but is rebuilt from GPUI Kit components for Kura's theme.

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::*;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::text::TextView;
use gpui_kit::component::*;
use gpui_kit::*;

use kura_dist::{Release, State as UpdateState};
use rust_i18n::t;

use crate::update::UpdateService;

pub struct UpdateWindow {
    service: Entity<UpdateService>,
    focus: FocusHandle,
    _observe: Subscription,
}

impl UpdateWindow {
    pub fn new(
        service: Entity<UpdateService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let observe = cx.observe(&service, |_, _, cx| cx.notify());
        Self {
            service,
            focus,
            _observe: observe,
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.service.update(cx, |service, cx| service.dismiss(cx));
        window.remove_window();
    }

    fn notes(&self, release: &Release, cx: &mut Context<Self>) -> AnyElement {
        if release.notes.is_empty() {
            return div().into_any_element();
        }
        let url = format!(
            "https://github.com/hu-ja-ja/kura/releases/tag/v{}",
            release.version
        );
        v_flex()
            .gap_2()
            .child(
                div()
                    .id("update-notes")
                    .max_h(px(170.))
                    .overflow_y_scrollbar()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .p_3()
                    .child(TextView::markdown(
                        "update-notes-text",
                        release.notes.clone(),
                    )),
            )
            .child(
                Button::new("update-notes-link")
                    .ghost()
                    .compact()
                    .icon(AssetIcon::ExternalLink)
                    .label(t!("update.release_notes"))
                    .accessibility_label(t!("update.release_notes"))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        if let Some(url) = kura_core::openable_url(&url) {
                            cx.open_url(&url);
                        }
                    })),
            )
            .into_any_element()
    }

    fn auto_install_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let checked = self.service.read(cx).settings().auto_install;
        h_flex()
            .items_center()
            .gap_2()
            .child(
                Switch::new("update-auto-install")
                    .checked(checked)
                    .accessibility_label(t!("update.auto_install"))
                    .on_click(cx.listener(|this, requested: &bool, _, cx| {
                        let requested = *requested;
                        this.service
                            .update(cx, |service, cx| service.set_auto_install(requested, cx));
                    })),
            )
            .child(div().text_sm().child(t!("update.auto_install").to_string()))
    }

    fn buttons(&self, state: &UpdateState, cx: &mut Context<Self>) -> AnyElement {
        let mut left = h_flex().items_center().gap_2();
        let mut right = h_flex().items_center().gap_2();

        match state {
            UpdateState::Available(_) => {
                left = left.child(
                    Button::new("update-skip")
                        .ghost()
                        .compact()
                        .label(t!("update.skip"))
                        .accessibility_label(t!("update.skip"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.service.update(cx, |service, cx| service.skip(cx));
                        })),
                );
                right = right.child(
                    Button::new("update-later")
                        .ghost()
                        .compact()
                        .label(t!("update.later"))
                        .accessibility_label(t!("update.later"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.service.update(cx, |service, cx| service.remind(cx));
                        })),
                );
                right = right.child(
                    Button::new("update-install")
                        .primary()
                        .label(t!("update.install"))
                        .accessibility_label(t!("update.install"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.service.update(cx, |service, cx| service.download(cx));
                        })),
                );
            }
            UpdateState::Staged { .. } => {
                right = right.child(
                    Button::new("update-later")
                        .ghost()
                        .compact()
                        .label(t!("update.later"))
                        .accessibility_label(t!("update.later"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.service.update(cx, |service, cx| service.later(cx));
                        })),
                );
                right = right.child(
                    Button::new("update-restart")
                        .primary()
                        .label(t!("update.restart"))
                        .accessibility_label(t!("update.restart"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.service.update(cx, |service, cx| service.restart(cx));
                        })),
                );
            }
            UpdateState::UpToDate | UpdateState::Failed(_) | UpdateState::Disabled => {
                right = right.child(
                    Button::new("update-check")
                        .ghost()
                        .compact()
                        .label(t!("update.check"))
                        .accessibility_label(t!("update.check"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.service.update(cx, |service, cx| service.check_now(cx));
                        })),
                );
            }
            UpdateState::Idle => {
                right = right.child(
                    Button::new("update-check")
                        .primary()
                        .label(t!("update.check"))
                        .accessibility_label(t!("update.check"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.service.update(cx, |service, cx| service.check_now(cx));
                        })),
                );
            }
            _ => {}
        }

        h_flex()
            .w_full()
            .items_center()
            .child(left)
            .child(div().flex_1())
            .child(right)
            .into_any_element()
    }

    fn body(&self, state: &UpdateState, cx: &mut Context<Self>) -> AnyElement {
        let current = self.service.read(cx).current_version().to_owned();
        let feedback = self
            .service
            .read(cx)
            .feedback()
            .map(str::to_owned)
            .filter(|text| !text.is_empty());

        let detail = |text: String, color: Hsla| {
            div()
                .text_sm()
                .text_color(color)
                .child(text)
                .into_any_element()
        };

        match state {
            UpdateState::Idle => v_flex()
                .gap_2()
                .child(div().child(t!("update.idle").to_string()))
                .into_any_element(),
            UpdateState::Checking => h_flex()
                .items_center()
                .gap_2()
                .child(Spinner::new().small())
                .child(div().child(t!("update.checking").to_string()))
                .into_any_element(),
            UpdateState::UpToDate => v_flex()
                .gap_2()
                .child(div().child(t!("update.up_to_date", version = current).to_string()))
                .into_any_element(),
            UpdateState::Available(release) => v_flex()
                .gap_2()
                .child(self.notes(release, cx))
                .child(self.auto_install_row(cx))
                .into_any_element(),
            UpdateState::Downloading {
                release,
                downloaded,
                total,
            } => v_flex()
                .gap_2()
                .child(div().child(t!("update.downloading", version = release.version).to_string()))
                .child(
                    Progress::new("update-progress")
                        .value(progress_value(*downloaded, *total))
                        .accessibility_label(progress_label(*downloaded, *total)),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(progress_label(*downloaded, *total)),
                )
                .into_any_element(),
            UpdateState::Staged { release, .. } => v_flex()
                .gap_2()
                .child(div().child(t!("update.staged", version = release.version).to_string()))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t!("update.staged_hint").to_string()),
                )
                .into_any_element(),
            UpdateState::Failed(error) => v_flex()
                .gap_2()
                .child(div().child(t!("update.failed").to_string()))
                .child(detail(crate::update::error_text(error), cx.theme().danger))
                .into_any_element(),
            UpdateState::Disabled => v_flex()
                .gap_2()
                .child(div().child(t!("update.disabled").to_string()))
                .into_any_element(),
        }
        .pipe(|body| {
            if let Some(feedback) = feedback {
                v_flex()
                    .gap_2()
                    .child(body)
                    .child(
                        div()
                            .id("update-feedback")
                            .role(Role::Alert)
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(feedback),
                    )
                    .into_any_element()
            } else {
                body
            }
        })
    }
}

impl Render for UpdateWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.service.read(cx).state().clone();
        let current = self.service.read(cx).current_version().to_owned();
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;

        let title = match &state {
            UpdateState::Available(_) => t!("update.title_available").to_string(),
            UpdateState::Staged { .. } => t!("update.title_ready").to_string(),
            UpdateState::Checking => t!("update.title_checking").to_string(),
            _ => t!("update.title").to_string(),
        };
        let subtitle = match &state {
            UpdateState::Available(update) => t!(
                "update.version_change",
                version = update.version,
                current = current
            )
            .to_string(),
            _ => t!("update.current", version = current).to_string(),
        };

        let header = v_flex()
            .gap_1()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::new(AssetIcon::Download).small())
                    .child(div().text_lg().child(title)),
            )
            .child(div().text_sm().text_color(muted).child(subtitle));

        v_flex()
            .size_full()
            .id("update-window")
            .role(Role::Dialog)
            .track_focus(&self.focus)
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .p_5()
            .gap_3()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close(window, cx);
                }
            }))
            .child(header)
            .child(
                v_flex()
                    .min_h_0()
                    .gap_3()
                    .child(self.body(&state, cx))
                    .child(self.buttons(&state, cx)),
            )
            .border_1()
            .border_color(border)
    }
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

fn progress_value(downloaded: u64, total: u64) -> f32 {
    if total == 0 {
        return 0.0;
    }
    (downloaded as f32 / total as f32 * 100.0).min(100.0)
}

fn progress_label(downloaded: u64, total: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    format!(
        "{:.1} / {:.1} MB",
        downloaded as f64 / MIB,
        total as f64 / MIB
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::update::UpdateService;
    use crate::update_window::UpdateWindow;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds,
        WindowOptions, px, size,
    };
    use kura_dist::{Error, Release, State as UpdateState, Version};

    fn open(
        cx: &mut TestAppContext,
        state: UpdateState,
    ) -> (AnyWindowHandle, Entity<UpdateWindow>, Entity<UpdateService>) {
        rust_i18n::set_locale("en");
        cx.update(gpui_kit::init);
        let service = cx.update(|cx| cx.new(|_| UpdateService::detached(state)));
        let service_for_window = service.clone();
        let window = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::default(),
                        size: size(px(540.), px(560.)),
                    })),
                    ..Default::default()
                },
                cx,
                move |window, cx| cx.new(|cx| UpdateWindow::new(service_for_window, window, cx)),
            )
            .expect("open the update window")
        });
        (window.0, window.1, service)
    }

    fn release(version: &str, notes: &str) -> Release {
        Release::new(
            Version::parse(version).expect("a version"),
            format!("https://example.invalid/Kura-{version}-setup.exe"),
            1024,
            "a".repeat(64),
            notes.to_owned(),
        )
    }

    fn available() -> UpdateState {
        UpdateState::Available(release("0.2.0", "## Changes\n\n- Faster search"))
    }

    #[gpui_kit::test]
    fn an_available_update_shows_release_notes_and_actions(cx: &mut TestAppContext) {
        let (window, _, _) = open(cx, available());
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("update-install").is_some());
            assert!(window.try_find("update-skip").is_some());
            assert!(window.try_find("update-later").is_some());
            assert!(window.try_find("update-auto-install").is_some());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn a_staged_update_offers_restart_and_later(cx: &mut TestAppContext) {
        let state = UpdateState::Staged {
            release: release("0.2.0", ""),
            artifact: PathBuf::from("Kura-0.2.0-setup.exe"),
        };
        let (window, _, _) = open(cx, state);
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("update-restart").is_some());
            assert!(window.try_find("update-later").is_some());
            assert!(window.try_find("update-install").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn downloading_shows_a_progress_label_and_no_install_button(cx: &mut TestAppContext) {
        let (window, _, _) = open(
            cx,
            UpdateState::Downloading {
                release: release("0.2.0", ""),
                downloaded: 5 * 1024 * 1024,
                total: 10 * 1024 * 1024,
            },
        );
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("update-progress").is_some());
            assert!(window.try_find("update-install").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn a_failed_update_offers_a_retry(cx: &mut TestAppContext) {
        let error = Error::Download("The download failed.".to_owned());
        let (window, _, _) = open(cx, UpdateState::Failed(error));
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("update-check").is_some());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn the_automatic_install_switch_updates_the_service(cx: &mut TestAppContext) {
        let (window, _, service) = open(cx, available());
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.click("update-auto-install", cx);
        })
        .unwrap();
        cx.update(|cx| assert!(service.read(cx).settings().auto_install));
    }

    #[gpui_kit::test]
    fn skipping_remembers_the_exact_update_build(cx: &mut TestAppContext) {
        let (window, _, service) = open(cx, available());
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.click("update-skip", cx);
        })
        .unwrap();
        cx.update(|cx| {
            assert_eq!(
                service.read(cx).settings().skipped_version.as_deref(),
                Some("0.2.0")
            );
        });
    }
}
