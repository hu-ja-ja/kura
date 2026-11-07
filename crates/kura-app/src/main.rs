//! Kura: a native Bitwarden client for Windows.

// A console subsystem binary makes Windows open a console window beside the
// app. Debug builds keep it so `cargo run` still shows panics and build output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::borrow::Cow;

use gpui_kit::assets::AllAssets;
use gpui_kit::component::Theme;
use gpui_kit::component::scroll::ScrollbarMode;
use gpui_kit::*;

mod app;
mod backend;
mod clipboard;
mod controller;
mod detail;
mod json_file;
mod licenses;
mod list;
mod logging;
mod session;
mod settings;
#[cfg(test)]
mod test_lock;
mod update;
mod update_window;

rust_i18n::i18n!("locales", fallback = "en");

// rust-i18n reads the locale file while expanding the macro. Keeping it as a
// compile-time input makes cargo rebuild the binary when the text changes.
const _: &str = include_str!("../locales/app.yml");

const FONT_REGULAR: &[u8] = include_bytes!("../assets/fonts/IBMPlexSansJP-Regular.ttf");
const FONT_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/IBMPlexSansJP-SemiBold.ttf");

fn main() {
    logging::init();
    match logging::folder() {
        Some(folder) => log::info!(
            "Kura {} starting; logs: {}",
            env!("CARGO_PKG_VERSION"),
            logging::display(&folder)
        ),
        None => log::info!(
            "Kura {} starting; logs are unavailable",
            env!("CARGO_PKG_VERSION")
        ),
    }
    // Before a vault can be opened: a dump of this process would hold the keys.
    kura_platform::disable_dumps();

    #[cfg(debug_assertions)]
    let demo = app::demo::install();
    #[cfg(debug_assertions)]
    let demo_mode = demo.is_some();
    #[cfg(not(debug_assertions))]
    let demo_mode = false;

    #[cfg(debug_assertions)]
    let settings = match demo {
        Some(options) => app::demo::settings(options),
        None => settings::Settings::load(),
    };
    #[cfg(not(debug_assertions))]
    let settings = settings::Settings::load();
    let locale = settings.locale();
    rust_i18n::set_locale(&locale);
    log::info!("locale: {locale}");

    // A demo must not open the real saved login or write to the real profile.
    let saved_session = if demo_mode { None } else { session::load() };
    if saved_session.is_some() {
        log::info!("a saved login was found");
    }
    let start_in_tray = settings.start_in_tray;

    let (platform_events, platform_event_rx) = tokio::sync::mpsc::unbounded_channel();
    let (platform, instance) = if demo_mode {
        // No tray in a demo run. Hold the sender so the controller keeps
        // running after it opens the window.
        std::mem::forget(platform_events);
        (kura_platform::Platform::detached(), None)
    } else {
        match kura_platform::start(
            platform_events,
            controller::labels(),
            settings.auto_lock().as_secs(),
        ) {
            Ok((platform, instance)) => (platform, Some(instance)),
            Err(kura_platform::StartError::AlreadyRunning) => {
                log::info!("another Kura is already running");
                return;
            }
            Err(err) => {
                log::error!("could not start the tray: {err}");
                kura_platform::show_error(&format!("Kura could not start: {err}"));
                std::process::exit(1);
            }
        }
    };
    // The named mutex is released when this drops, so keep it for the whole run.
    let _instance = instance;
    // Set before any window exists: start_in_tray shows the tray menu first.
    platform.set_appearance(settings.theme.into());

    // The SDK talks to the network, so it needs a runtime of its own. The
    // platform drops the launch closure as soon as it has run, so the runtime
    // has to live longer than that.
    let runtime: &'static tokio::runtime::Runtime = Box::leak(Box::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to start the tokio runtime"),
    ));

    let (backend, backend_events) = if demo_mode {
        backend::Backend::detached(true)
    } else {
        backend::Backend::spawn(
            runtime.handle(),
            saved_session,
            settings.confirmed_servers.clone(),
        )
    };

    let (update_requests, update_request_rx) = tokio::sync::mpsc::unbounded_channel();

    let app = gpui_kit::application()
        .with_assets(AllAssets)
        .with_quit_mode(QuitMode::Explicit);

    app.run(move |cx| {
        gpui_kit::init(cx);
        // A scrollbar that only shows while scrolling is easy to miss; keep it
        // on wherever the content overflows.
        Theme::set_scrollbar_mode(ScrollbarMode::Always, cx);
        cx.text_system()
            .add_fonts(vec![
                Cow::Borrowed(FONT_REGULAR),
                Cow::Borrowed(FONT_SEMIBOLD),
            ])
            .expect("failed to load the bundled fonts");
        Theme::global_mut(cx).font_family = "IBM Plex Sans JP".into();

        let updates = cx.new(|cx| update::UpdateService::new(update_requests, cx));

        controller::start(
            cx,
            backend,
            backend_events,
            platform,
            platform_event_rx,
            start_in_tray,
            updates,
            update_request_rx,
        );
    });
}
