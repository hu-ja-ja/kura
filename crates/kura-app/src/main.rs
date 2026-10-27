//! Kura: a native Bitwarden client for Windows.

use std::borrow::Cow;

use gpui_kit::assets::AllAssets;
use gpui_kit::component::Theme;
use gpui_kit::*;

mod app;
mod backend;
mod clipboard;
mod controller;
mod detail;
mod list;
mod logging;
mod session;
mod settings;

rust_i18n::i18n!("locales", fallback = "en");

// rust-i18n reads the locale file while expanding the macro. Keeping it as a
// compile-time input makes cargo rebuild the binary when the text changes.
const _: &str = include_str!("../locales/app.yml");

const FONT_REGULAR: &[u8] = include_bytes!("../assets/fonts/IBMPlexSansJP-Regular.ttf");
const FONT_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/IBMPlexSansJP-SemiBold.ttf");

fn main() {
    logging::init();

    let settings = settings::Settings::load();
    let locale = settings.locale();
    rust_i18n::set_locale(&locale);

    let saved_session = session::load();
    let start_in_tray = settings.start_in_tray;

    let (platform_events, platform_event_rx) = tokio::sync::mpsc::unbounded_channel();
    let (platform, instance) = match kura_platform::start(
        platform_events,
        controller::labels(),
        settings.auto_lock().as_secs(),
    ) {
        Ok(started) => started,
        Err(kura_platform::StartError::AlreadyRunning) => return,
        Err(err) => {
            log::error!("could not start the tray: {err}");
            std::process::exit(1);
        }
    };
    // The named mutex is released when this drops, so keep it for the whole run.
    let _instance = instance;

    // The SDK talks to the network, so it needs a runtime of its own. The
    // platform drops the launch closure as soon as it has run, so the runtime
    // has to live longer than that.
    let runtime: &'static tokio::runtime::Runtime = Box::leak(Box::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to start the tokio runtime"),
    ));

    let (backend, backend_events) = backend::Backend::spawn(runtime.handle(), saved_session);

    let app = gpui_kit::application()
        .with_assets(AllAssets)
        .with_quit_mode(QuitMode::Explicit);

    app.run(move |cx| {
        gpui_kit::init(cx);
        cx.text_system()
            .add_fonts(vec![
                Cow::Borrowed(FONT_REGULAR),
                Cow::Borrowed(FONT_SEMIBOLD),
            ])
            .expect("failed to load the bundled fonts");
        Theme::global_mut(cx).font_family = "IBM Plex Sans JP".into();

        controller::start(
            cx,
            backend,
            backend_events,
            platform,
            platform_event_rx,
            start_in_tray,
        );
    });
}
