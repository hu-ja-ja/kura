//! Tests for the update service: settings, the notification decisions, and
//! how a check result reaches the window. The engine's own fetch, download and
//! verification are covered by `kura-dist`.

use gpui_kit::{AppContext as _, Entity, TestAppContext};
use kura_dist::{Error, Release, State, Version};
use tokio::sync::mpsc::UnboundedReceiver;

use super::{Request, UpdateService, UpdateSettings, error_text, now_ms};

fn release(version: &str) -> Release {
    Release::new(
        Version::parse(version).expect("a version"),
        format!("https://example.invalid/Kura-{version}-setup.exe"),
        1024,
        "a".repeat(64),
        String::new(),
    )
}

/// A service with no engine and a receiver the test can inspect.
fn service(cx: &mut TestAppContext) -> (Entity<UpdateService>, UnboundedReceiver<Request>) {
    let (requests, receiver) = tokio::sync::mpsc::unbounded_channel();
    let service = UpdateService {
        updater: None,
        state: State::Idle,
        settings: UpdateSettings::default(),
        feedback: None,
        installing: false,
        persist: false,
        requests,
    };
    (cx.update(|cx| cx.new(|_| service)), receiver)
}

#[test]
fn settings_default_to_automatic_checks_without_automatic_install() {
    let settings = UpdateSettings::default();
    assert!(settings.auto_check);
    assert!(!settings.auto_install);
}

#[test]
fn every_engine_error_has_a_localized_text() {
    let _serial = crate::test_lock::serial();
    let cases = [
        (Error::Manifest(String::new()), "update.error_manifest"),
        (Error::Signature(String::new()), "update.error_verify"),
        (Error::Download(String::new()), "update.error_download"),
        (Error::Install(String::new()), "update.error_installer"),
        (
            Error::Configuration(String::new()),
            "update.error_configuration",
        ),
    ];

    for (error, key) in cases {
        assert_eq!(error_text(&error), rust_i18n::t!(key).to_string());
    }
}

#[test]
fn settings_round_trip_and_replace_atomically() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("update.json");
    let settings = UpdateSettings {
        auto_install: true,
        skipped_version: Some("1.2.3".to_owned()),
        ..UpdateSettings::default()
    };
    settings.save_to(&path).expect("save the update settings");
    assert_eq!(UpdateSettings::load_from(&path), settings);
    assert!(!path.with_extension("json.tmp").exists());
}

#[test]
fn a_corrupt_settings_file_falls_back_to_defaults() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("update.json");
    std::fs::write(&path, "{").expect("write a corrupt file");
    assert_eq!(UpdateSettings::load_from(&path), UpdateSettings::default());
}

#[test]
fn skip_and_remind_suppress_only_the_same_build() {
    let mut service = UpdateService::detached(State::Idle);
    service.settings.skipped_version = Some("1.2.3".to_owned());
    assert!(!service.should_notify(&release("1.2.3")));
    assert!(service.should_notify(&release("1.2.4")));

    service.settings.skipped_version = None;
    service.settings.remind_after_unix_ms = Some(now_ms() + 60_000);
    assert!(!service.should_notify(&release("1.2.4")));
}

#[gpui_kit::test]
fn a_background_check_opens_the_window_only_when_an_update_appears(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (service, mut requests) = service(cx);

    cx.update(|cx| {
        service.update(cx, |service, cx| {
            service.after_check(Ok(Some(release("0.2.0"))), false, cx);
        })
    });

    assert_eq!(requests.try_recv(), Ok(Request::Open { background: true }));
    cx.update(|cx| assert!(matches!(service.read(cx).state(), State::Available(_))));
}

#[gpui_kit::test]
fn an_up_to_date_background_check_stays_silent(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (service, mut requests) = service(cx);

    cx.update(|cx| service.update(cx, |service, cx| service.after_check(Ok(None), false, cx)));

    assert!(requests.try_recv().is_err());
    cx.update(|cx| assert!(matches!(service.read(cx).state(), State::UpToDate)));
}

#[gpui_kit::test]
fn a_failed_manual_check_reports_the_error_and_opens_the_window(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (service, mut requests) = service(cx);
    let error = Error::Manifest("the network is down".to_owned());

    cx.update(|cx| {
        service.update(cx, |service, cx| {
            service.after_check(Err(error), true, cx);
        })
    });

    assert_eq!(requests.try_recv(), Ok(Request::Open { background: false }));
    cx.update(|cx| {
        let expected = rust_i18n::t!("update.error_manifest").to_string();
        assert_eq!(service.read(cx).feedback(), Some(expected.as_str()));
    });
}

#[gpui_kit::test]
fn a_failed_background_check_stays_silent(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (service, mut requests) = service(cx);

    cx.update(|cx| {
        service.update(cx, |service, cx| {
            service.after_check(Err(Error::Download("no bytes".to_owned())), false, cx);
        })
    });

    assert!(requests.try_recv().is_err());
    cx.update(|cx| assert_eq!(service.read(cx).feedback(), None));
}

#[gpui_kit::test]
fn a_check_is_skipped_while_an_update_is_running(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (service, mut requests) = service(cx);
    cx.update(|cx| {
        service.update(cx, |service, cx| {
            service.state = State::Downloading {
                release: release("0.2.0"),
                downloaded: 0,
                total: 1024,
            };
            service.check(true, cx);
        })
    });

    assert_eq!(requests.try_recv(), Ok(Request::Open { background: false }));
    assert!(requests.try_recv().is_err());
    cx.update(|cx| {
        assert!(matches!(
            service.read(cx).state(),
            State::Downloading { .. }
        ));
        assert_eq!(service.read(cx).settings().last_check_unix_ms, None);
    });
}

#[gpui_kit::test]
fn a_check_without_an_engine_explains_and_opens_the_window(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (service, mut requests) = service(cx);

    cx.update(|cx| service.update(cx, |service, cx| service.check_now(cx)));

    assert_eq!(requests.try_recv(), Ok(Request::Open { background: false }));
    cx.update(|cx| {
        let expected = rust_i18n::t!("update.cannot_check").to_string();
        assert_eq!(service.read(cx).feedback(), Some(expected.as_str()));
    });
}

#[gpui_kit::test]
fn skipping_an_update_closes_the_window_and_remembers_the_build(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (service, mut requests) = service(cx);

    cx.update(|cx| {
        service.update(cx, |service, cx| {
            service.state = State::Available(release("0.2.0"));
            service.skip(cx);
        })
    });

    assert_eq!(requests.try_recv(), Ok(Request::Close));
    cx.update(|cx| {
        assert_eq!(
            service.read(cx).settings().skipped_version.as_deref(),
            Some("0.2.0")
        );
    });
}

#[gpui_kit::test]
fn automatic_install_downloads_instead_of_opening_the_window(cx: &mut TestAppContext) {
    let _serial = crate::test_lock::serial();
    let (service, mut requests) = service(cx);

    cx.update(|cx| service.update(cx, |service, cx| service.set_auto_install(true, cx)));
    cx.update(|cx| {
        service.update(cx, |service, cx| {
            service.after_check(Ok(Some(release("0.2.0"))), false, cx);
        })
    });

    // The test service has no engine, so reaching the download is observable
    // only as its failure.
    assert!(requests.try_recv().is_err());
    cx.update(|cx| {
        let expected = rust_i18n::t!("update.unavailable").to_string();
        assert_eq!(service.read(cx).feedback(), Some(expected.as_str()));
    });
}
