//! Kura: a native Bitwarden client for Windows.

use gpui_kit::assets::Assets;
use gpui_kit::*;

mod app;
mod backend;
mod list;
mod settings;

rust_i18n::i18n!("locales", fallback = "en");

fn main() {
    // The SDK talks to the network, so it needs a runtime of its own. The
    // platform drops the launch closure as soon as it has run, so the runtime
    // has to live longer than that.
    let runtime: &'static tokio::runtime::Runtime = Box::leak(Box::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to start the tokio runtime"),
    ));

    let app = gpui_kit::application().with_assets(Assets);

    app.run(move |cx| {
        gpui_kit::init(cx);

        let (backend, events) = backend::Backend::spawn(runtime.handle());
        let settings = settings::Settings::load();

        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(900.), px(700.)), cx)),
            titlebar: Some(TitlebarOptions {
                title: Some("Kura".into()),
                ..Default::default()
            }),
            ..Default::default()
        };

        gpui_kit::open_window(window_options, cx, |window, cx| {
            cx.new(|cx| app::Root::new(window, cx, backend, events, settings))
        })
        .expect("failed to open window");
    });
}
