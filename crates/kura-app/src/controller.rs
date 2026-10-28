//! The part of the application that outlives the vault window.
//!
//! It owns the backend event stream, the tray actions and the window
//! lifecycle. The window itself can be destroyed while the app stays unlocked
//! or locked in the tray; the controller opens a new one when asked.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::*;
use kura_platform::Event as PlatformEvent;
use rust_i18n::t;

use crate::app::Root;
use crate::backend::{Backend, Event as BackendEvent};
use crate::clipboard::Guard as Clipboard;
use crate::settings::{self, Settings};

const INITIAL_WIDTH: Pixels = px(900.);
const INITIAL_HEIGHT: Pixels = px(700.);

static QUITTING: AtomicBool = AtomicBool::new(false);

/// Whether the application is on its way out. The close handler must not turn
/// a shutdown into a trip to the tray.
pub fn quitting() -> bool {
    QUITTING.load(Ordering::SeqCst)
}

/// Localized tray menu labels for the active locale.
pub fn labels() -> kura_platform::Labels {
    kura_platform::Labels {
        show: t!("tray.show").to_string(),
        lock: t!("tray.lock").to_string(),
        quit: t!("tray.quit").to_string(),
    }
}

pub fn start(
    cx: &mut App,
    backend: Backend,
    mut backend_events: tokio::sync::mpsc::UnboundedReceiver<BackendEvent>,
    platform: kura_platform::Platform,
    mut platform_events: tokio::sync::mpsc::UnboundedReceiver<PlatformEvent>,
    start_in_tray: bool,
) {
    let clipboard = cx.new(|_| Clipboard::default());
    let clipboard_on_quit = clipboard.clone();
    let quit_subscription = cx.on_app_quit(move |cx| {
        QUITTING.store(true, Ordering::SeqCst);
        clipboard_on_quit.update(cx, |clipboard, cx| clipboard.clear(cx));
        async {}
    });

    let (window_tx, mut window_rx) = tokio::sync::mpsc::unbounded_channel();
    let closed_subscription = cx.on_window_closed(move |_, window_id| {
        let _ = window_tx.send(window_id);
    });

    let mut controller = Controller {
        backend,
        platform,
        clipboard,
        window: None,
        hwnd: None,
        headless: false,
        _quit: quit_subscription,
        _closed: closed_subscription,
    };

    cx.spawn(async move |cx| {
        if start_in_tray {
            controller.detach(cx).await;
        } else {
            controller.show(cx).await;
        }
        loop {
            tokio::select! {
                event = backend_events.recv() => {
                    let Some(event) = event else { break };
                    controller.backend_event(event, cx);
                }
                event = platform_events.recv() => {
                    let Some(event) = event else { break };
                    controller.platform_event(event, cx).await;
                }
                Some(window_id) = window_rx.recv() => {
                    controller.window_closed(window_id, cx).await;
                }
            }
        }
        cx.update(|cx| cx.quit());
    })
    .detach();
}

struct Controller {
    backend: Backend,
    platform: kura_platform::Platform,
    clipboard: Entity<Clipboard>,
    window: Option<(AnyWindowHandle, WeakEntity<Root>)>,
    hwnd: Option<isize>,
    /// The GPU is released while there is no window.
    headless: bool,
    _quit: Subscription,
    _closed: Subscription,
}

impl Controller {
    async fn show(&mut self, cx: &mut AsyncApp) {
        if let Some((handle, root)) = self.window.clone()
            && root.upgrade().is_some()
        {
            if let Some(hwnd) = self.hwnd {
                kura_platform::show_window(hwnd);
            }
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                if let Some(root) = root.upgrade() {
                    root.update(cx, |root, cx| root.on_shown(cx));
                }
                return;
            }
        }

        self.window = None;
        self.hwnd = None;
        self.open(cx).await;
    }

    async fn open(&mut self, cx: &mut AsyncApp) {
        if self.headless {
            let reattach = cx.update(|cx| {
                cx.request_windowing(WindowingRequest::Windowed(GraphicalEnvironment::detect()))
            });
            match reattach.await {
                Ok(()) => self.headless = false,
                Err(err) => {
                    log::error!("could not restore the display: {err}");
                    return;
                }
            }
        }

        let settings = Settings::load();
        let saved_email = crate::session::load().map(|saved| saved.email);
        let backend = self.backend.clone();
        let platform = self.platform.clone();
        let clipboard = self.clipboard.clone();

        let opened = cx.update(|cx| {
            let options = window_options(&settings, cx);
            gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| {
                    Root::new(
                        window,
                        cx,
                        backend,
                        settings,
                        clipboard,
                        platform,
                        saved_email,
                    )
                })
            })
        });

        let (handle, root) = match opened {
            Ok(opened) => opened,
            Err(err) => {
                log::error!("could not open the vault window: {err}");
                return;
            }
        };
        self.hwnd = handle
            .update(cx, |_, window, _| window_hwnd(window))
            .ok()
            .flatten();
        if let Some(hwnd) = self.hwnd {
            kura_platform::set_window_icon(hwnd);
        }
        self.window = Some((handle, root.downgrade()));
    }

    fn backend_event(&mut self, event: BackendEvent, cx: &mut AsyncApp) {
        if matches!(event, BackendEvent::Locked) {
            self.clipboard
                .update(cx, |clipboard, cx| clipboard.clear(cx));
        }

        let Some((_, root)) = self.window.clone() else {
            return;
        };
        let Some(root) = root.upgrade() else {
            self.window = None;
            self.hwnd = None;
            return;
        };
        root.update(cx, |root, cx| root.handle_event(event, cx));
    }

    async fn platform_event(&mut self, event: PlatformEvent, cx: &mut AsyncApp) {
        match event {
            PlatformEvent::Show => self.show(cx).await,
            PlatformEvent::Lock => self.backend.lock(),
            PlatformEvent::Quit => {
                self.save_window_state(cx);
                self.clipboard
                    .update(cx, |clipboard, cx| clipboard.clear(cx));
                cx.update(|cx| cx.quit());
            }
        }
    }

    async fn window_closed(&mut self, window_id: WindowId, cx: &mut AsyncApp) {
        let current = self.window.as_ref().map(|(handle, _)| handle.window_id());
        if current != Some(window_id) {
            return;
        }
        self.window = None;
        self.hwnd = None;
        self.detach(cx).await;
    }

    /// Release the DirectX devices while there is no window. The window being
    /// gone is not enough on its own: the renderer keeps a lot of memory.
    async fn detach(&mut self, cx: &mut AsyncApp) {
        if quitting() || self.headless || self.window.is_some() {
            return;
        }
        let request = cx.update(|cx| cx.request_windowing(WindowingRequest::Headless));
        match request.await {
            Ok(()) => {
                self.headless = true;
                log::info!("released the display while in the tray");
                // Let the renderer drop its resources before asking Windows to
                // reclaim the pages.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                kura_platform::trim_working_set();
            }
            Err(err) => log::warn!("could not release the display: {err}"),
        }
    }

    fn save_window_state(&mut self, cx: &mut AsyncApp) {
        let Some((handle, _)) = self.window.clone() else {
            return;
        };
        let _ = handle.update(cx, |_, window, _| {
            let mut settings = Settings::load();
            settings.window = Some(settings::placement(window));
            if let Err(err) = settings.save() {
                log::warn!("could not save the window placement: {err}");
            }
        });
    }
}

/// The HWND behind a GPUI window, for the few Win32 calls GPUI does not wrap.
pub(crate) fn window_hwnd(window: &Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

fn window_options(settings: &Settings, cx: &App) -> WindowOptions {
    let saved = settings
        .window
        .zip(settings.window_bounds())
        .filter(|(_, bounds)| is_on_screen(*bounds, cx));
    let window_bounds = match saved {
        Some((placement, bounds)) if placement.maximized => WindowBounds::Maximized(bounds),
        Some((_, bounds)) => WindowBounds::Windowed(bounds),
        None => WindowBounds::centered(size(INITIAL_WIDTH, INITIAL_HEIGHT), cx),
    };

    WindowOptions {
        window_bounds: Some(window_bounds),
        window_min_size: Some(size(px(720.), px(480.))),
        titlebar: Some(TitlebarOptions {
            title: Some("Kura".into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn is_on_screen(bounds: Bounds<Pixels>, cx: &App) -> bool {
    let a = (
        f32::from(bounds.origin.x),
        f32::from(bounds.origin.y),
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
    );
    cx.displays().iter().any(|display| {
        let bounds = display.bounds();
        let b = (
            f32::from(bounds.origin.x),
            f32::from(bounds.origin.y),
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
        );
        rects_intersect(a, b)
    })
}

fn rects_intersect(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    let (ax, ay, aw, ah) = a;
    let (bx, by, bw, bh) = b;
    ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah
}

#[cfg(test)]
mod tests {
    use super::rects_intersect;

    #[test]
    fn overlapping_rects_count_as_visible() {
        assert!(rects_intersect(
            (0., 0., 100., 100.),
            (90., 90., 100., 100.)
        ));
    }

    #[test]
    fn a_rect_off_every_display_is_not_visible() {
        assert!(!rects_intersect(
            (-5000., -5000., 900., 700.),
            (0., 0., 1920., 1080.)
        ));
    }

    #[test]
    fn a_touching_edge_does_not_overlap() {
        assert!(!rects_intersect(
            (0., 0., 100., 100.),
            (100., 0., 100., 100.)
        ));
    }
}
