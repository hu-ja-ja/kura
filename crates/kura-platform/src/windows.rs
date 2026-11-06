//! The Win32 half of the platform crate.
//!
//! Everything unsafe lives here. The functions exposed to `lib.rs` are safe and
//! return errors instead of panicking.

#![allow(unsafe_code)]
// The whole module is the unsafe boundary; the safe wrappers below are the only
// way the rest of the application reaches it.
#![allow(unsafe_op_in_unsafe_fn)]

use std::mem::size_of;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_CLASS_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND,
    ERROR_SUCCESS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteValueW,
    RegSetValueExW,
};
use windows_sys::Win32::System::RemoteDesktop::{
    NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, GetCurrentProcessId};
use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, FindWindowW, GWLP_USERDATA, GetCursorPos, GetForegroundWindow, GetMessageW,
    GetSystemMetrics, GetWindowLongPtrW, GetWindowThreadProcessId, ICON_BIG, ICON_SMALL,
    IMAGE_ICON, IsIconic, KillTimer, LR_DEFAULTCOLOR, LR_SHARED, LoadImageW, MB_ICONERROR, MB_OK,
    MF_STRING, MSG, MessageBoxW, PBT_APMSUSPEND, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SM_CXICON, SM_CXSMICON, SPI_GETHIGHCONTRAST, SW_HIDE, SW_RESTORE,
    SW_SHOW, SendMessageW, SetForegroundWindow, SetTimer, SetWindowDisplayAffinity,
    SetWindowLongPtrW, ShowWindowAsync, SystemParametersInfoW, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenu, TranslateMessage, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WM_APP, WM_CLOSE,
    WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NULL, WM_POWERBROADCAST, WM_RBUTTONUP,
    WM_SETICON, WM_TIMER, WM_WTSSESSION_CHANGE, WNDCLASSW, WTS_SESSION_LOCK,
};

use crate::{Appearance, Event, Labels, StartError, WINDOW_CLASS};

const TRAY_ICON_ID: u32 = 1;
/// The kura.ico resource compiled into the executable by kura-app's build script.
const ICON_RESOURCE: u32 = 1;
const SHOW_MESSAGE: u32 = WM_APP + 1;
const SHOW_WINDOW_MESSAGE: u32 = WM_APP + 2;
const IDLE_TIMER_ID: usize = 1;
const MENU_SHOW: usize = 1;
const MENU_CHECK_UPDATES: usize = 2;
const MENU_LOCK: usize = 3;
const MENU_QUIT: usize = 4;
/// The uxtheme app mode for a right-click menu. The values are undocumented.
const APP_MODE_ALLOW_DARK: i32 = 1;
const APP_MODE_FORCE_DARK: i32 = 2;
const APP_MODE_FORCE_LIGHT: i32 = 3;
const AUTOSTART_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const AUTOSTART_VALUE: &str = "Kura";

/// The tray menu reads this on every right click, so there is no window to
/// notify when the theme changes.
static APPEARANCE: AtomicU8 = AtomicU8::new(Appearance::System as u8);

/// The named mutex handle. Kept open until the process exits.
pub struct Instance {
    handle: HANDLE,
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}

pub fn acquire_instance() -> Result<Instance, StartError> {
    let name = wide("Local\\Kura.SingleInstance");
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(StartError::Failed(last_error("CreateMutexW")));
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { CloseHandle(handle) };
        if !signal_existing() {
            show_error(
                "Kura is already running, but its window could not be shown. Check the tray.",
            );
        }
        return Err(StartError::AlreadyRunning);
    }
    Ok(Instance { handle })
}

/// Ask the first instance to show its window. The first instance may still be
/// starting, so give the window class a moment to appear.
fn signal_existing() -> bool {
    let class = wide(WINDOW_CLASS);
    for _ in 0..30 {
        let hwnd = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
        if !hwnd.is_null() {
            unsafe { PostMessageW(hwnd, SHOW_WINDOW_MESSAGE, 0, 0) };
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Show a blocking error box for a failure that happens before any window
/// exists. The caller is about to exit.
pub fn show_error(message: &str) {
    let text = wide(message);
    let title = wide("Kura");
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

struct State {
    events: UnboundedSender<Event>,
    labels: Arc<Mutex<Labels>>,
    auto_lock: Arc<AtomicU64>,
    icon: isize,
    /// The `GetLastInputInfo` value already seen, so one input is counted once.
    last_input: u32,
    idle: IdleTimer,
    taskbar_created: u32,
}

/// The automatic lock, as a decision that needs no Win32 to test.
struct IdleTimer {
    /// The tick of the last input that went to a Kura window.
    last_touch: u64,
    /// Whether this stretch of idle already locked the vault.
    fired: bool,
}

impl IdleTimer {
    /// Count an interaction with Kura: hold the vault open and rearm the timer.
    fn touch(&mut self, now: u64) {
        self.last_touch = now;
        self.fired = false;
    }

    /// Take one poll and say whether the vault has to be locked now.
    fn poll(&mut self, now: u64, timeout_seconds: u64, new_input: bool, ours: bool) -> bool {
        // The clock follows use of Kura, like Bitwarden's vault timeout, not
        // use of the machine: work in another app must not keep it open.
        if new_input && ours {
            self.touch(now);
        }
        if self.fired
            || timeout_seconds == 0
            || now.saturating_sub(self.last_touch) < timeout_seconds.saturating_mul(1000)
        {
            return false;
        }
        self.fired = true;
        true
    }
}

pub fn run(
    events: UnboundedSender<Event>,
    labels: Arc<Mutex<Labels>>,
    auto_lock: Arc<AtomicU64>,
    ready: Sender<Result<(), String>>,
) {
    match unsafe { create(events, labels, auto_lock) } {
        Ok(()) => {
            let _ = ready.send(Ok(()));
            unsafe { message_loop() };
        }
        Err(message) => {
            let _ = ready.send(Err(message));
        }
    }
}

unsafe fn create(
    events: UnboundedSender<Event>,
    labels: Arc<Mutex<Labels>>,
    auto_lock: Arc<AtomicU64>,
) -> Result<(), String> {
    let instance = GetModuleHandleW(std::ptr::null());
    let class = wide(WINDOW_CLASS);
    let window_class = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        lpszClassName: class.as_ptr(),
        ..Default::default()
    };
    let atom = RegisterClassW(&window_class);
    if atom == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
        return Err(last_error("RegisterClassW"));
    }

    let title = wide("Kura");
    let hwnd = CreateWindowExW(
        0,
        class.as_ptr(),
        title.as_ptr(),
        0,
        0,
        0,
        0,
        0,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        instance,
        std::ptr::null(),
    );
    if hwnd.is_null() {
        return Err(last_error("CreateWindowExW"));
    }

    let size = GetSystemMetrics(SM_CXSMICON);
    let icon = load_icon(size);
    let state = Box::new(State {
        events,
        labels,
        auto_lock,
        icon: icon as isize,
        last_input: 0,
        // Nothing is held open before the user first touches Kura.
        idle: IdleTimer {
            last_touch: 0,
            fired: true,
        },
        taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
    });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);

    if icon.is_null() {
        log::warn!("the tray icon could not be loaded");
    } else {
        add_icon(hwnd, icon as isize);
    }

    SetTimer(hwnd, IDLE_TIMER_ID, 1000, None);
    if WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) == 0 {
        log::warn!("session change notifications are unavailable");
    }

    Ok(())
}

unsafe fn message_loop() {
    let mut message = MSG::default();
    while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
        TranslateMessage(&message);
        DispatchMessageW(&message);
    }
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_DESTROY {
        cleanup(hwnd);
        PostQuitMessage(0);
        return 0;
    }
    if message == WM_CLOSE {
        DestroyWindow(hwnd);
        return 0;
    }

    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }

    if message == SHOW_MESSAGE {
        let (events, mouse) = {
            let state = &*pointer;
            (state.events.clone(), lparam as u32)
        };
        // Using the tray is using Kura, so it holds the vault open like a click
        // in the window does.
        if matches!(mouse, WM_LBUTTONUP | WM_LBUTTONDBLCLK | WM_RBUTTONUP) {
            (*pointer).idle.touch(GetTickCount64());
        }
        match mouse {
            WM_LBUTTONUP | WM_LBUTTONDBLCLK => send(&events, Event::Show),
            WM_RBUTTONUP => {
                let labels = {
                    let state = &*pointer;
                    state
                        .labels
                        .lock()
                        .map(|labels| labels.clone())
                        .unwrap_or_default()
                };
                show_menu(hwnd, &labels, &events);
            }
            _ => {}
        }
    } else if message == SHOW_WINDOW_MESSAGE {
        (*pointer).idle.touch(GetTickCount64());
        send(&(*pointer).events, Event::Show);
    } else if message == WM_TIMER && wparam == IDLE_TIMER_ID {
        check_idle(&mut *pointer);
    } else if (message == WM_WTSSESSION_CHANGE && wparam as u32 == WTS_SESSION_LOCK)
        || (message == WM_POWERBROADCAST && wparam as u32 == PBT_APMSUSPEND)
    {
        log::info!("the Windows session is locking");
        send(&(*pointer).events, Event::Lock);
    } else if (*pointer).taskbar_created != 0 && message == (*pointer).taskbar_created {
        let icon = (*pointer).icon;
        if icon != 0 {
            add_icon(hwnd, icon as _);
        }
    }

    DefWindowProcW(hwnd, message, wparam, lparam)
}

unsafe fn cleanup(hwnd: HWND) {
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if pointer.is_null() {
        return;
    }
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
    let state = Box::from_raw(pointer);
    if state.icon != 0 {
        let data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ICON_ID,
            ..Default::default()
        };
        Shell_NotifyIconW(NIM_DELETE, &data);
    }
    WTSUnRegisterSessionNotification(hwnd);
    KillTimer(hwnd, IDLE_TIMER_ID);
}

unsafe fn add_icon(hwnd: HWND, icon: isize) {
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: SHOW_MESSAGE,
        hIcon: icon as _,
        ..Default::default()
    };
    set_wide(&mut data.szTip, "Kura");
    if Shell_NotifyIconW(NIM_ADD, &data) == 0 {
        log::warn!("the tray icon could not be added");
    }
}

/// Load the icon from the executable's resources. `LR_SHARED` keeps Windows from
/// handing out a new copy every time the window is recreated; the handle is not
/// ours to destroy.
unsafe fn load_icon(size: i32) -> HANDLE {
    let instance = GetModuleHandleW(std::ptr::null());
    LoadImageW(
        instance,
        ICON_RESOURCE as usize as *const u16,
        IMAGE_ICON,
        size,
        size,
        LR_DEFAULTCOLOR | LR_SHARED,
    )
}

/// Give a window the icon the taskbar and Alt+Tab show.
pub fn set_window_icon(hwnd: isize) {
    unsafe {
        for (slot, size) in [
            (ICON_BIG, GetSystemMetrics(SM_CXICON)),
            (ICON_SMALL, GetSystemMetrics(SM_CXSMICON)),
        ] {
            let icon = load_icon(size);
            if icon.is_null() {
                log::warn!("the window icon could not be loaded");
                continue;
            }
            SendMessageW(hwnd as HWND, WM_SETICON, slot as WPARAM, icon as LPARAM);
        }
    }
}

/// The uxtheme entry points that let a classic Win32 menu draw dark. Windows
/// exports them by ordinal only, and not before Windows 10 1903.
struct MenuTheme {
    set_mode: unsafe extern "system" fn(i32) -> i32,
    /// Only re-themes menus that are already open, so it is optional.
    flush: Option<unsafe extern "system" fn()>,
}

fn menu_theme() -> Option<&'static MenuTheme> {
    static THEME: OnceLock<Option<MenuTheme>> = OnceLock::new();
    THEME
        .get_or_init(|| {
            let theme = unsafe { load_menu_theme() };
            if theme.is_none() {
                log::warn!("the tray menu cannot follow the theme on this Windows version");
            }
            theme
        })
        .as_ref()
}

unsafe fn load_menu_theme() -> Option<MenuTheme> {
    // The system copy, never one dropped next to kura.exe.
    let module = LoadLibraryExW(
        wide("uxtheme.dll").as_ptr(),
        std::ptr::null_mut(),
        LOAD_LIBRARY_SEARCH_SYSTEM32,
    );
    if module.is_null() {
        return None;
    }
    let set_mode = GetProcAddress(module, 135_usize as *const u8)?;
    Some(MenuTheme {
        // The undocumented signatures, from the dark mode reverse engineering.
        set_mode: std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            unsafe extern "system" fn(i32) -> i32,
        >(set_mode),
        flush:
            GetProcAddress(module, 136_usize as *const u8).map(|flush| {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "system" fn(),
                >(flush)
            }),
    })
}

pub fn set_appearance(appearance: Appearance) {
    APPEARANCE.store(appearance as u8, Ordering::Relaxed);
}

/// Without the uxtheme entry points the menu keeps the light system look.
unsafe fn apply_menu_theme() {
    let Some(theme) = menu_theme() else {
        return;
    };
    (theme.set_mode)(menu_mode(
        APPEARANCE.load(Ordering::Relaxed),
        high_contrast(),
    ));
    if let Some(flush) = theme.flush {
        // Menus cache their frame until the next flush.
        flush();
    }
}

/// High contrast colours must win over the app theme.
fn menu_mode(appearance: u8, high_contrast: bool) -> i32 {
    if high_contrast {
        return APP_MODE_ALLOW_DARK;
    }
    // The values are the discriminants of `Appearance`.
    match appearance {
        1 => APP_MODE_FORCE_LIGHT,
        2 => APP_MODE_FORCE_DARK,
        _ => APP_MODE_ALLOW_DARK,
    }
}

fn high_contrast() -> bool {
    let mut info = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            info.cbSize,
            (&mut info as *mut HIGHCONTRASTW).cast(),
            0,
        )
    };
    ok != 0 && info.dwFlags & HCF_HIGHCONTRASTON != 0
}

unsafe fn show_menu(hwnd: HWND, labels: &Labels, events: &UnboundedSender<Event>) {
    apply_menu_theme();
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    let show = wide(&labels.show);
    let check_updates = wide(&labels.check_updates);
    let lock = wide(&labels.lock);
    let quit = wide(&labels.quit);
    AppendMenuW(menu, MF_STRING, MENU_SHOW, show.as_ptr());
    AppendMenuW(menu, MF_STRING, MENU_CHECK_UPDATES, check_updates.as_ptr());
    AppendMenuW(menu, MF_STRING, MENU_LOCK, lock.as_ptr());
    AppendMenuW(menu, MF_STRING, MENU_QUIT, quit.as_ptr());

    let mut point = POINT::default();
    if GetCursorPos(&mut point) == 0 {
        DestroyMenu(menu);
        return;
    }
    SetForegroundWindow(hwnd);
    let command = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON,
        point.x,
        point.y,
        0,
        hwnd,
        std::ptr::null(),
    );
    DestroyMenu(menu);
    PostMessageW(hwnd, WM_NULL, 0, 0);

    match command as usize {
        MENU_SHOW => send(events, Event::Show),
        MENU_CHECK_UPDATES => send(events, Event::CheckForUpdates),
        MENU_LOCK => send(events, Event::Lock),
        MENU_QUIT => send(events, Event::Quit),
        _ => {}
    }
}

fn send(events: &UnboundedSender<Event>, event: Event) {
    let _ = events.send(event);
}

fn check_idle(state: &mut State) {
    let timeout = state.auto_lock.load(Ordering::Relaxed);
    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return;
    }
    let now = unsafe { GetTickCount64() };
    let new_input = info.dwTime != state.last_input;
    state.last_input = info.dwTime;
    if state
        .idle
        .poll(now, timeout, new_input, foreground_is_ours())
    {
        log::info!("locking after {timeout} seconds without input to Kura");
        send(&state.events, Event::Lock);
    }
}

/// Whether the input behind the current `GetLastInputInfo` value landed on a
/// Kura window. The tray message loop and every app window share this process.
fn foreground_is_ours() -> bool {
    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return false;
    }
    let mut process = 0;
    unsafe { GetWindowThreadProcessId(window, &mut process) };
    process == unsafe { GetCurrentProcessId() }
}

pub fn hide_window(hwnd: isize) {
    unsafe { ShowWindowAsync(hwnd as HWND, SW_HIDE) };
}

/// Drop as many pages as Windows will let go while the app sits in the tray.
/// They are faulted back when a window is opened again.
pub fn trim_working_set() {
    unsafe {
        EmptyWorkingSet(GetCurrentProcess());
    }
}

pub fn show_window(hwnd: isize) {
    let hwnd = hwnd as HWND;
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindowAsync(hwnd, SW_RESTORE);
        } else {
            // SW_RESTORE would also undo a maximized state.
            ShowWindowAsync(hwnd, SW_SHOW);
        }
        SetForegroundWindow(hwnd);
    }
}

pub fn set_capture_protection(hwnd: isize, enabled: bool) -> Result<(), String> {
    let affinity = if enabled {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    if unsafe { SetWindowDisplayAffinity(hwnd as HWND, affinity) } == 0 {
        return Err(last_error("SetWindowDisplayAffinity"));
    }
    Ok(())
}

pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let mut key: HKEY = std::ptr::null_mut();
    let mut disposition = 0;
    let result = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(AUTOSTART_KEY).as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            &mut disposition,
        )
    };
    if result != ERROR_SUCCESS {
        return Err(format!(
            "opening the startup key failed with error {result}"
        ));
    }

    let value = wide(AUTOSTART_VALUE);
    let result = if enabled {
        match autostart_command() {
            Ok(command) => {
                let command = wide(&command);
                unsafe {
                    RegSetValueExW(
                        key,
                        value.as_ptr(),
                        0,
                        REG_SZ,
                        command.as_ptr() as *const u8,
                        (command.len() * size_of::<u16>()) as u32,
                    )
                }
            }
            Err(message) => {
                unsafe { RegCloseKey(key) };
                return Err(message);
            }
        }
    } else {
        unsafe { RegDeleteValueW(key, value.as_ptr()) }
    };
    unsafe { RegCloseKey(key) };

    if result != ERROR_SUCCESS && !(!enabled && result == ERROR_FILE_NOT_FOUND) {
        return Err(format!(
            "writing the startup entry failed with error {result}"
        ));
    }
    Ok(())
}

fn autostart_command() -> Result<String, String> {
    let path = std::env::current_exe().map_err(|err| err.to_string())?;
    Ok(format!("\"{}\"", path.display()))
}

pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn set_wide(buffer: &mut [u16], text: &str) {
    for (slot, unit) in buffer.iter_mut().zip(text.encode_utf16()) {
        *slot = unit;
    }
    if let Some(last) = buffer.last_mut() {
        *last = 0;
    }
}

fn last_error(what: &str) -> String {
    let code = unsafe { GetLastError() };
    format!("{what} failed with Windows error {code}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_in_kura_holds_the_vault_open() {
        let mut timer = arming_at(1_000);
        assert!(!timer.poll(10_000, 60, true, true));
        assert!(!timer.poll(61_000, 60, false, true));
    }

    #[test]
    fn input_in_another_app_does_not_hold_the_vault_open() {
        let mut timer = arming_at(1_000);
        assert!(!timer.poll(10_000, 60, true, false));
        assert!(timer.poll(61_000, 60, false, false));
    }

    #[test]
    fn the_lock_fires_once_per_stretch_of_idle() {
        let mut timer = arming_at(1_000);
        assert!(timer.poll(61_000, 60, false, false));
        assert!(!timer.poll(600_000, 60, false, false));
    }

    #[test]
    fn a_touch_after_a_lock_rearms_the_timer() {
        let mut timer = arming_at(1_000);
        assert!(timer.poll(61_000, 60, false, false));
        timer.touch(70_000);
        assert!(!timer.poll(129_999, 60, false, false));
        assert!(timer.poll(130_000, 60, false, false));
    }

    #[test]
    fn a_zero_timeout_never_fires() {
        assert!(!arming_at(0).poll(100_000_000, 0, false, false));
    }

    #[test]
    fn a_tick_count_that_goes_backwards_never_fires() {
        assert!(!arming_at(90_000).poll(30_000, 60, false, false));
    }

    /// A timer that has seen one touch, as the tray leaves it.
    fn arming_at(last_touch: u64) -> IdleTimer {
        IdleTimer {
            last_touch,
            fired: false,
        }
    }

    #[test]
    fn the_menu_mode_matches_the_theme() {
        assert_eq!(
            menu_mode(Appearance::System as u8, false),
            APP_MODE_ALLOW_DARK
        );
        assert_eq!(
            menu_mode(Appearance::Light as u8, false),
            APP_MODE_FORCE_LIGHT
        );
        assert_eq!(
            menu_mode(Appearance::Dark as u8, false),
            APP_MODE_FORCE_DARK
        );
        // A value this build does not know falls back to the system setting.
        assert_eq!(menu_mode(9, false), APP_MODE_ALLOW_DARK);
    }

    #[test]
    fn high_contrast_wins_over_the_theme() {
        assert_eq!(menu_mode(Appearance::Dark as u8, true), APP_MODE_ALLOW_DARK);
        assert_eq!(
            menu_mode(Appearance::Light as u8, true),
            APP_MODE_ALLOW_DARK
        );
    }
}
