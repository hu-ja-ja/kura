//! The Win32 half of the platform crate.
//!
//! Everything unsafe lives here. The functions exposed to `lib.rs` are safe and
//! return errors instead of panicking.

#![allow(unsafe_code)]
// The whole module is the unsafe boundary; the safe wrappers below are the only
// way the rest of the application reaches it.
#![allow(unsafe_op_in_unsafe_fn)]

use std::mem::size_of;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_CLASS_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND,
    ERROR_SUCCESS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteValueW,
    RegSetValueExW,
};
use windows_sys::Win32::System::RemoteDesktop::{
    NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentProcess};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, FindWindowW, GWLP_USERDATA, GetCursorPos, GetMessageW, GetWindowLongPtrW,
    IDI_APPLICATION, IsIconic, KillTimer, LoadIconW, MF_STRING, MSG, PBT_APMSUSPEND, PostMessageW,
    PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SW_HIDE, SW_RESTORE, SW_SHOW,
    SetForegroundWindow, SetTimer, SetWindowDisplayAffinity, SetWindowLongPtrW, ShowWindowAsync,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WDA_EXCLUDEFROMCAPTURE,
    WDA_NONE, WM_APP, WM_CLOSE, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NULL,
    WM_POWERBROADCAST, WM_RBUTTONUP, WM_TIMER, WM_WTSSESSION_CHANGE, WNDCLASSW, WTS_SESSION_LOCK,
};

use crate::{Event, Labels, StartError, WINDOW_CLASS};

const TRAY_ICON_ID: u32 = 1;
const SHOW_MESSAGE: u32 = WM_APP + 1;
const SHOW_WINDOW_MESSAGE: u32 = WM_APP + 2;
const IDLE_TIMER_ID: usize = 1;
const MENU_SHOW: usize = 1;
const MENU_LOCK: usize = 2;
const MENU_QUIT: usize = 3;
const AUTOSTART_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const AUTOSTART_VALUE: &str = "Kura";

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
        signal_existing();
        return Err(StartError::AlreadyRunning);
    }
    Ok(Instance { handle })
}

/// Ask the first instance to show its window. The first instance may still be
/// starting, so give the window class a moment to appear.
fn signal_existing() {
    let class = wide(WINDOW_CLASS);
    for _ in 0..30 {
        let hwnd = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
        if !hwnd.is_null() {
            unsafe { PostMessageW(hwnd, SHOW_WINDOW_MESSAGE, 0, 0) };
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct State {
    events: UnboundedSender<Event>,
    labels: Arc<Mutex<Labels>>,
    auto_lock: Arc<AtomicU64>,
    icon: isize,
    fired_for_input: u32,
    taskbar_created: u32,
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

    let icon = LoadIconW(std::ptr::null_mut(), IDI_APPLICATION);
    let state = Box::new(State {
        events,
        labels,
        auto_lock,
        icon: icon as isize,
        fired_for_input: 0,
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

unsafe fn show_menu(hwnd: HWND, labels: &Labels, events: &UnboundedSender<Event>) {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    let show = wide(&labels.show);
    let lock = wide(&labels.lock);
    let quit = wide(&labels.quit);
    AppendMenuW(menu, MF_STRING, MENU_SHOW, show.as_ptr());
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
    if timeout == 0 {
        return;
    }
    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return;
    }
    let now = unsafe { GetTickCount64() } as u32;
    if idle_lock_due(info.dwTime, now, timeout, state.fired_for_input) {
        state.fired_for_input = info.dwTime;
        log::info!("locking after {timeout} seconds without input");
        send(&state.events, Event::Lock);
    }
}

/// A pure check so the wrap of the 32-bit tick count stays in one place.
fn idle_lock_due(last_input: u32, now: u32, timeout_seconds: u64, fired_for: u32) -> bool {
    if timeout_seconds == 0 || fired_for == last_input {
        return false;
    }
    let idle_ms = u64::from(now.wrapping_sub(last_input));
    idle_ms >= timeout_seconds.saturating_mul(1000)
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

fn wide(text: &str) -> Vec<u16> {
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
    fn the_idle_timer_fires_once_per_input() {
        assert!(!idle_lock_due(1_000, 30_000, 60, 0));
        assert!(idle_lock_due(1_000, 61_000, 60, 0));
        assert!(!idle_lock_due(1_000, 61_000, 60, 1_000));
    }

    #[test]
    fn a_zero_timeout_never_fires() {
        assert!(!idle_lock_due(1_000, 100_000_000, 0, 0));
    }

    #[test]
    fn a_wrapped_tick_count_still_measures_idle_time() {
        // GetLastInputInfo is 32-bit and wraps about every 49 days.
        assert!(idle_lock_due(u32::MAX - 500, 1_000, 1, 0));
    }
}
