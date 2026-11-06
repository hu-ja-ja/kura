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
use crate::update::{Request as UpdateRequest, UpdateService};
use crate::update_window::UpdateWindow;

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
        check_updates: t!("tray.check_updates").to_string(),
        lock: t!("tray.lock").to_string(),
        quit: t!("tray.quit").to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn start(
    cx: &mut App,
    backend: Backend,
    mut backend_events: tokio::sync::mpsc::UnboundedReceiver<BackendEvent>,
    platform: kura_platform::Platform,
    mut platform_events: tokio::sync::mpsc::UnboundedReceiver<PlatformEvent>,
    start_in_tray: bool,
    updates: Entity<UpdateService>,
    mut update_requests: tokio::sync::mpsc::UnboundedReceiver<UpdateRequest>,
) {
    let clipboard = cx.new(|_| Clipboard::default());
    let clipboard_on_quit = clipboard.clone();
    let updates_on_quit = updates.clone();
    let quit_subscription = cx.on_app_quit(move |cx| {
        QUITTING.store(true, Ordering::SeqCst);
        clipboard_on_quit.update(cx, |clipboard, cx| clipboard.clear(cx));
        updates_on_quit.update(cx, |updates, _| updates.install_on_quit());
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
        updates,
        window: None,
        update_window: None,
        pending_update_window: false,
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
                Some(request) = update_requests.recv() => {
                    controller.update_request(request, cx).await;
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
    updates: Entity<UpdateService>,
    window: Option<(AnyWindowHandle, WeakEntity<Root>)>,
    update_window: Option<AnyWindowHandle>,
    /// A background discovery waits until the main window is visible.
    pending_update_window: bool,
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
                if self.pending_update_window {
                    self.pending_update_window = false;
                    self.show_update_window(cx).await;
                }
                return;
            }
        }

        self.window = None;
        self.hwnd = None;
        self.open(cx).await;
        if self.pending_update_window {
            self.pending_update_window = false;
            self.show_update_window(cx).await;
        }
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
        let saved = crate::session::load();
        let saved_email = saved.as_ref().map(|saved| saved.email.clone());
        let saved_server = saved.as_ref().and_then(|saved| {
            kura_core::server_url_from_saved(&saved.identity_url, &saved.api_url)
        });
        let backend = self.backend.clone();
        let platform = self.platform.clone();
        let clipboard = self.clipboard.clone();
        let updates = self.updates.clone();

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
                        saved_server,
                        updates,
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

    async fn show_update_window(&mut self, cx: &mut AsyncApp) {
        if let Some(handle) = self.update_window {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
            self.update_window = None;
        }

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

        let updates = self.updates.clone();
        let opened = cx.update(|cx| {
            let options = update_window_options(cx);
            gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| UpdateWindow::new(updates, window, cx))
            })
        });
        match opened {
            Ok((handle, _)) => self.update_window = Some(handle),
            Err(err) => log::error!("could not open the update window: {err}"),
        }
    }

    async fn update_request(&mut self, request: UpdateRequest, cx: &mut AsyncApp) {
        match request {
            UpdateRequest::Open { background } => {
                if background && self.window.is_none() {
                    self.pending_update_window = true;
                    return;
                }
                self.show_update_window(cx).await;
            }
            UpdateRequest::Close => self.close_update_window(cx),
            UpdateRequest::Quit => self.quit(cx),
        }
    }

    fn close_update_window(&mut self, cx: &mut AsyncApp) {
        if let Some(handle) = self.update_window {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }

    fn quit(&mut self, cx: &mut AsyncApp) {
        self.save_window_state(cx);
        self.clipboard
            .update(cx, |clipboard, cx| clipboard.clear(cx));
        cx.update(|cx| cx.quit());
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
            PlatformEvent::CheckForUpdates => {
                self.updates.update(cx, |updates, cx| updates.check_now(cx));
            }
            PlatformEvent::Lock => self.backend.lock(),
            PlatformEvent::Quit => self.quit(cx),
        }
    }

    async fn window_closed(&mut self, window_id: WindowId, cx: &mut AsyncApp) {
        if self.update_window.map(|handle| handle.window_id()) == Some(window_id) {
            self.update_window = None;
            self.detach(cx).await;
            return;
        }

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
        if quitting() || self.headless || self.window.is_some() || self.update_window.is_some() {
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

fn update_window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(640.), px(360.)), cx)),
        window_min_size: Some(size(px(520.), px(320.))),
        titlebar: Some(TitlebarOptions {
            title: Some(t!("update.title").to_string().into()),
            ..Default::default()
        }),
        focus: true,
        show: true,
        ..Default::default()
    }
}

fn window_options(settings: &Settings, cx: &App) -> WindowOptions {
    let saved = match settings.window.zip(settings.window_bounds()) {
        Some((placement, bounds)) => {
            restore_display(bounds, cx).map(|display| (placement, bounds, display))
        }
        None => None,
    };
    let (window_bounds, display_id) = match saved {
        Some((placement, bounds, display)) if placement.maximized => {
            (WindowBounds::Maximized(bounds), Some(display))
        }
        Some((_, bounds, display)) => (WindowBounds::Windowed(bounds), Some(display)),
        None => (
            WindowBounds::centered(size(INITIAL_WIDTH, INITIAL_HEIGHT), cx),
            None,
        ),
    };

    WindowOptions {
        window_bounds: Some(window_bounds),
        display_id,
        window_min_size: Some(size(px(720.), px(480.))),
        titlebar: Some(TitlebarOptions {
            title: Some("Kura".into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// The display holding the window's center. GPUI accepts a saved placement only
/// when the center is on the display the window opens on, and otherwise replaces
/// it with its own 1536x1095 default. Passing this display keeps the saved size.
fn display_under(
    bounds: Bounds<Pixels>,
    mut displays: impl Iterator<Item = (DisplayId, Bounds<Pixels>)>,
) -> Option<DisplayId> {
    let center = bounds.center();
    displays
        .find(|(_, display)| display.contains(&center))
        .map(|(id, _)| id)
}

fn restore_display(bounds: Bounds<Pixels>, cx: &App) -> Option<DisplayId> {
    display_under(
        bounds,
        cx.displays()
            .into_iter()
            .map(|display| (display.id(), display.bounds())),
    )
}

#[cfg(test)]
mod tests {
    use super::{Bounds, DisplayId, Pixels, display_under, point, px, size};

    fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
    }

    #[test]
    fn a_saved_window_restores_on_the_display_holding_its_center() {
        let primary = (DisplayId::new(1), bounds(0., 0., 1920., 1080.));
        let left = (DisplayId::new(2), bounds(-1080., 0., 1080., 1920.));
        let window = bounds(-1040., 100., 900., 700.);

        assert_eq!(
            display_under(window, [primary, left].into_iter()),
            Some(DisplayId::new(2))
        );
    }

    #[test]
    fn bounds_centred_off_every_display_fall_back_to_the_default() {
        let primary = (DisplayId::new(1), bounds(0., 0., 1920., 1080.));
        let window = bounds(-2000., 100., 900., 700.);

        assert_eq!(display_under(window, [primary].into_iter()), None);
    }
}
