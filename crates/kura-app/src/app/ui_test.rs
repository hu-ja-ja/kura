//! UI tests: drive the real window in a headless GPUI test app.
//!
//! The backend commands are dropped, so these tests only cover what the view
//! does on its own: labels, dialogs, list filtering and key handling. Network,
//! clipboard, the tray and the OS settings belong to other tests.

use std::sync::Arc;

use gpui_kit::test::TestQueryExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds,
    WindowOptions, px, size,
};
use kura_core::{
    CoreError, ItemKind, TotpCode, TwoFactorChallenge, TwoFactorMethod, VaultItem, VaultItemDetail,
    VaultSnapshot,
};
use rust_i18n::t;
use zeroize::Zeroizing;

use crate::backend::Backend;
use crate::clipboard::Guard as Clipboard;
use crate::settings::Settings;
use crate::update::UpdateService;

use super::{Event, Root};

fn open(
    cx: &mut TestAppContext,
    settings: Settings,
    unlocked: bool,
) -> (AnyWindowHandle, Entity<Root>) {
    open_saved(cx, settings, unlocked, None)
}

fn open_saved(
    cx: &mut TestAppContext,
    settings: Settings,
    unlocked: bool,
    saved_server: Option<String>,
) -> (AnyWindowHandle, Entity<Root>) {
    cx.update(gpui_kit::init);
    cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(900.), px(700.)),
                })),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                let clipboard = cx.new(|_| Clipboard::default());
                let updates = cx.new(|_| UpdateService::detached(kura_dist::State::Idle));
                cx.new(|cx| {
                    let mut root = Root::new(
                        window,
                        cx,
                        Backend::detached(unlocked),
                        settings,
                        clipboard,
                        kura_platform::Platform::detached(),
                        None,
                        saved_server,
                        updates,
                    );
                    // A click must not overwrite the real settings file.
                    root.persist = false;
                    root
                })
            },
        )
        .expect("open the test window")
    })
}

fn settings() -> Settings {
    Settings {
        language: Some("en".to_string()),
        screenshot_protection: false,
        ..Settings::default()
    }
}

/// Deliver a backend event the way the controller does, outside a window update.
fn send(cx: &mut TestAppContext, root: &Entity<Root>, event: Event) {
    cx.update(|cx| {
        root.update(cx, |root, cx| root.handle_event(event, cx));
    });
}

fn login(id: &str, name: &str, favorite: bool) -> VaultItem {
    let username = format!("ada@{id}.example");
    let uri = format!("https://{id}.example");
    let mut search = name.to_lowercase();
    for part in [&username, &uri] {
        search.push('\0');
        search.push_str(&part.to_lowercase());
    }
    VaultItem {
        id: id.to_string(),
        name: name.to_string(),
        subtitle: username.clone(),
        username,
        uris: vec![uri],
        search,
        kind: ItemKind::Login,
        favorite,
        folder_id: None,
    }
}

fn snapshot() -> VaultSnapshot {
    VaultSnapshot {
        items: vec![
            login("github", "GitHub", false),
            login("gitlab", "GitLab", true),
        ],
        ..VaultSnapshot::default()
    }
}

fn detail(id: &str, totp: Option<TotpCode>) -> VaultItemDetail {
    VaultItemDetail {
        id: id.to_string(),
        name: format!("item-{id}"),
        kind: ItemKind::Login,
        favorite: false,
        folder_id: None,
        username: Some("ada".to_string()),
        password: Some(Zeroizing::new("secret".to_string())),
        uris: vec!["https://example.com".to_string()],
        totp,
        notes: None,
        fields: Vec::new(),
        card: None,
        identity: None,
        ssh_key: None,
        attachments: Vec::new(),
    }
}

fn row_of(root: &Entity<Root>, cx: &gpui_kit::App, id: &str) -> Option<usize> {
    root.read(cx).list.read(cx).delegate().row_of(id)
}

#[gpui_kit::test]
fn the_sign_in_button_names_the_server_before_the_password_leaves(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, root) = open(cx, settings(), false);
    send(
        cx,
        &root,
        Event::ConfirmServer {
            server: "https://vault.example.com/bitwarden".to_string(),
        },
    );

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let approving = t!(
            "login.sign_in_to",
            server = "https://vault.example.com/bitwarden"
        )
        .to_string();
        assert_eq!(window.find("sign-in").label(), Some(approving.as_str()));

        // Changing the server withdraws the approval for the old base URL.
        window.click("server-toggle", cx);
        window.click("login-server", cx);
        window.input("https://other.example", cx);
    })
    .unwrap();

    // The change events and the redraw are delivered once the window update ends.
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let submit = t!("login.submit").to_string();
        assert_eq!(window.find("sign-in").label(), Some(submit.as_str()));
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_saved_login_supplies_the_server_when_the_settings_have_none(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, root) = open_saved(
        cx,
        settings(),
        false,
        Some("https://vault.example.com/bitwarden".to_string()),
    );

    cx.update(|cx| {
        let root = root.read(cx);
        assert_eq!(
            root.settings.server_url.as_deref(),
            Some("https://vault.example.com/bitwarden")
        );
        assert_eq!(
            root.sign_in_server.as_deref(),
            Some("https://vault.example.com/bitwarden")
        );
        assert_eq!(
            root.server_url.read(cx).value(),
            "https://vault.example.com/bitwarden"
        );
    });

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("login-server").is_some());

        let expected = t!("login.accessing", server = "vault.example.com").to_string();
        assert_eq!(
            window.find("login-accessing").label(),
            Some(expected.as_str())
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_configured_server_stays_ahead_of_the_saved_login(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let settings = Settings {
        server_url: Some("https://other.example".to_string()),
        ..settings()
    };
    let (_window, root) = open_saved(
        cx,
        settings,
        false,
        Some("https://vault.example.com".to_string()),
    );

    cx.update(|cx| {
        assert_eq!(
            root.read(cx).settings.server_url.as_deref(),
            Some("https://other.example")
        );
    });
}

#[gpui_kit::test]
fn the_login_screen_names_the_default_server_without_a_setting(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, _root) = open(cx, settings(), false);

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let expected = t!("login.accessing", server = "bitwarden.com").to_string();
        let line = window.find("login-accessing");
        assert_eq!(line.label(), Some(expected.as_str()));

        // The line sits at the bottom of the page, outside the login card.
        let submit = window.find("sign-in").bounds();
        assert!(line.bounds().origin.y >= submit.origin.y + submit.size.height);
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_forgotten_server_disappears_from_the_settings_card(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let settings = Settings {
        confirmed_servers: vec!["https://vault.example.com".to_string()],
        ..settings()
    };
    let (window, root) = open(cx, settings, false);
    let id = "forget-https://vault.example.com";

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings", cx);
        assert!(window.try_find(id).is_some());

        window.click(id, cx);
        assert!(window.try_find(id).is_none());

        window.click("settings-close", cx);
    })
    .unwrap();

    cx.update(|cx| {
        assert!(root.read(cx).settings.confirmed_servers.is_empty());
        assert!(!root.read(cx).settings_open);
    });
}

#[gpui_kit::test]
fn the_settings_card_offers_the_log_folder(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, _root) = open(cx, settings(), false);

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings", cx);
        assert!(window.try_find("setting-logs").is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_settings_card_says_when_capture_protection_is_unavailable(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, root) = open(cx, settings(), false);

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings", cx);
        assert!(window.try_find("setting-capture-unavailable").is_none());

        root.update(cx, |root, cx| {
            root.capture_unavailable = true;
            cx.notify();
        });
        window.render_frame(cx);
        assert!(window.try_find("setting-capture-unavailable").is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn closing_to_the_tray_during_a_sign_in_clears_it(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let settings = Settings {
        destroy_on_hide: false,
        ..settings()
    };
    let (window, root) = open(cx, settings, false);

    cx.update_window(window, |_, window, cx| {
        root.update(cx, |root, cx| {
            root.page = super::Page::Login;
            root.busy = true;
            root.password
                .update(cx, |input, cx| input.set_value("secret", window, cx));

            root.close_to_tray(window, cx);

            assert!(!root.busy);
            assert_eq!(root.page, super::Page::Login);
            assert!(root.password.read(cx).value().is_empty());
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn forgetting_the_saved_login_can_reset_the_settings(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let settings = Settings {
        language: Some("en".to_string()),
        screenshot_protection: true,
        auto_lock_minutes: 5,
        ..Settings::default()
    };
    let (window, root) = open(cx, settings, false);

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings", cx);
        window.click("setting-forget-login", cx);
        assert!(window.try_find("forget-login-confirm").is_some());

        window.click("forget-login-reset", cx);
        window.click("forget-login-confirm", cx);
        window.render_frame(cx);
        assert!(window.try_find("forget-login-confirm").is_none());
    })
    .unwrap();

    cx.update(|cx| assert_eq!(root.read(cx).settings, Settings::default()));
    rust_i18n::set_locale("en");
}

#[gpui_kit::test]
fn a_login_that_cannot_be_saved_gets_a_notice(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (_window, root) = open(cx, settings(), true);
    send(cx, &root, Event::SessionNotSaved);

    let expected = t!("error.session_not_saved").to_string();
    cx.update(|cx| {
        assert_eq!(
            root.read(cx).notice.text.as_deref(),
            Some(expected.as_str())
        );
    });
}

#[test]
fn errors_stay_up_longer_than_successes() {
    assert!(super::notice_time(true) > super::notice_time(false));
}

#[gpui_kit::test]
fn an_empty_sign_in_reports_the_missing_input(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, _root) = open(cx, settings(), false);

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("sign-in", cx);

        let problem = t!("error.missing_input").to_string();
        assert_eq!(window.find("problem").label(), Some(problem.as_str()));
    })
    .unwrap();
}

#[gpui_kit::test]
fn escape_closes_the_settings_card(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, root) = open(cx, settings(), false);

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings", cx);
        assert!(window.try_find("settings-close").is_some());

        window.press("escape", cx);
    })
    .unwrap();

    cx.update(|cx| assert!(!root.read(cx).settings_open));
}

#[gpui_kit::test]
fn ctrl_l_returns_to_the_login_screen_and_drops_the_vault(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, root) = open(cx, settings(), true);
    send(cx, &root, Event::Snapshot(Arc::new(snapshot())));

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("vault-search").is_some());

        window.press("ctrl-l", cx);
        window.render_frame(cx);

        assert!(window.try_find("sign-in").is_some());
        assert!(window.try_find("vault-search").is_none());
    })
    .unwrap();

    cx.update(|cx| assert!(root.read(cx).snapshot.items.is_empty()));
}

#[gpui_kit::test]
fn a_long_memo_grows_instead_of_scrolling_at_a_fixed_height(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, root) = open(cx, settings(), true);
    cx.update_window(window, |_, window, cx| {
        root.update(cx, |root, cx| root.open_item("a".to_string(), window, cx));
    })
    .unwrap();

    let mut item = detail("a", None);
    item.notes = Some(Zeroizing::new("memo line\n".repeat(20)));
    send(cx, &root, Event::Detail(Box::new(item)));

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        let notes = window.find_by_label(t!("detail.notes").as_ref());
        assert!(notes.bounds().size.height > px(140.));
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_long_default_value_wraps_instead_of_being_cut(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, root) = open(cx, settings(), true);
    cx.update_window(window, |_, window, cx| {
        root.update(cx, |root, cx| root.open_item("a".to_string(), window, cx));
    })
    .unwrap();

    let mut item = detail("a", None);
    item.identity = Some(kura_core::IdentityDetail {
        address: Some("1-2-3 Chiyoda, Chiyoda-ku, Tokyo, Japan, 100-0001".repeat(4)),
        ..Default::default()
    });
    send(cx, &root, Event::Detail(Box::new(item)));

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let value = window.find("value-identity-address");
        assert!(value.bounds().size.height > px(30.));
    })
    .unwrap();
}

#[gpui_kit::test]
fn switching_items_drops_the_previous_detail_rows(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, root) = open(cx, settings(), true);
    cx.update_window(window, |_, window, cx| {
        root.update(cx, |root, cx| root.open_item("a".to_string(), window, cx));
    })
    .unwrap();
    send(cx, &root, Event::Detail(Box::new(detail("a", None))));
    cx.update(|cx| assert!(!root.read(cx).detail.rows.is_empty()));

    cx.update_window(window, |_, window, cx| {
        root.update(cx, |root, cx| root.open_item("b".to_string(), window, cx));
    })
    .unwrap();
    send(
        cx,
        &root,
        Event::DetailFailed {
            id: "b".to_string(),
            err: CoreError::NotFound,
        },
    );

    cx.update(|cx| assert!(root.read(cx).detail.rows.is_empty()));
}

#[gpui_kit::test]
fn a_totp_update_rebuilds_the_detail_rows(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, root) = open(cx, settings(), true);
    cx.update_window(window, |_, window, cx| {
        root.update(cx, |root, cx| root.open_item("a".to_string(), window, cx));
    })
    .unwrap();
    send(
        cx,
        &root,
        Event::Detail(Box::new(detail(
            "a",
            Some(TotpCode {
                code: "111111".to_string(),
                remaining: 30,
            }),
        ))),
    );
    send(
        cx,
        &root,
        Event::Totp {
            id: "a".to_string(),
            totp: TotpCode {
                code: "222222".to_string(),
                remaining: 30,
            },
        },
    );

    cx.update(|cx| {
        let root = root.read(cx);
        let row = root
            .detail
            .rows
            .iter()
            .find(|row| row.large)
            .expect("the TOTP row");
        assert_eq!(row.value.as_str(), "222 222");
        assert_eq!(
            row.copy_value.as_ref().map(|value| value.as_str()),
            Some("222222")
        );
    });
}

#[gpui_kit::test]
fn a_lock_event_after_a_login_returns_to_the_login_page(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (_window, root) = open(cx, settings(), false);

    send(cx, &root, Event::LoggedIn);
    cx.update(|cx| assert_eq!(root.read(cx).page, super::Page::Vault));

    send(cx, &root, Event::Locked);
    cx.update(|cx| {
        assert_eq!(root.read(cx).page, super::Page::Login);
        assert!(root.read(cx).snapshot.items.is_empty());
    });
}

#[gpui_kit::test]
fn closing_the_forget_dialog_restores_the_settings_focus(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, root) = open(cx, settings(), false);

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings", cx);
        window.click("setting-forget-login", cx);
        window.press("escape", cx);
        assert!(root.read(cx).settings_focus.is_focused(window));
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_snapshot_fills_the_list_and_the_search_narrows_it(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, root) = open(cx, settings(), true);
    send(cx, &root, Event::Snapshot(Arc::new(snapshot())));

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(row_of(&root, cx, "github"), Some(0));
        assert_eq!(row_of(&root, cx, "gitlab"), Some(1));

        window.click("vault-search", cx);
        window.input("gitlab", cx);
    })
    .unwrap();

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(row_of(&root, cx, "github"), None);
        assert_eq!(row_of(&root, cx, "gitlab"), Some(0));
        assert!(window.try_find("clear-search").is_some());

        // Clearing the field resets the list, not only the text.
        window.click("clear-search", cx);
        window.render_frame(cx);
        assert_eq!(row_of(&root, cx, "github"), Some(0));
        assert!(window.try_find("clear-search").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn escape_in_the_search_clears_the_query_and_the_filter(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, root) = open(cx, settings(), true);
    send(cx, &root, Event::Snapshot(Arc::new(snapshot())));

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("vault-search", cx);
        window.input("gitlab", cx);
    })
    .unwrap();

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(row_of(&root, cx, "github"), None);

        window.press("escape", cx);
        window.render_frame(cx);

        assert_eq!(row_of(&root, cx, "github"), Some(0));
        assert!(window.try_find("clear-search").is_none());
    })
    .unwrap();

    cx.update(|cx| assert!(root.read(cx).search.read(cx).value().is_empty()));
}

#[gpui_kit::test]
fn ctrl_f_focuses_the_search(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, root) = open(cx, settings(), true);
    send(cx, &root, Event::Snapshot(Arc::new(snapshot())));

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);

        // Typing "down" in the search hands the focus to the list.
        window.press("down", cx);
        window.render_frame(cx);
        assert_eq!(window.find("vault-search").focused(), Some(false));

        window.press("ctrl-f", cx);
        assert_eq!(window.find("vault-search").focused(), Some(true));
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_two_factor_challenge_opens_the_otp_page_and_back_returns(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, root) = open(cx, settings(), false);
    send(
        cx,
        &root,
        Event::TwoFactorRequired(TwoFactorChallenge {
            methods: vec![
                TwoFactorMethod::Authenticator,
                TwoFactorMethod::Email {
                    address: "ada@example.com".to_string(),
                },
            ],
        }),
    );

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("verify").is_some());
        assert!(window.try_find("method-authenticator").is_some());

        window.click("method-email", cx);
        window.render_frame(cx);
        assert!(window.try_find("send-code").is_some());

        window.click("back", cx);
        window.render_frame(cx);
        assert!(window.try_find("sign-in").is_some());
    })
    .unwrap();

    cx.update(|cx| assert!(root.read(cx).challenge.is_none()));
}

#[gpui_kit::test]
fn a_failed_sync_offers_a_retry(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (window, root) = open(cx, settings(), true);
    send(cx, &root, Event::Snapshot(Arc::new(snapshot())));
    send(cx, &root, Event::Failed(CoreError::Network));

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("retry-sync").is_some());

        window.click("retry-sync", cx);
    })
    .unwrap();

    cx.update(|cx| assert!(root.read(cx).busy));
}
