//! The notification-area icon shown while the collector is watching a
//! sign-in. Its tooltip gives the progress, a click opens the full window,
//! and the menu has Open and Quit. It is how the user gets back to Windows Startup Monitor
//! after the overlay has hidden itself.
//!
//! It runs on its own thread with its own hidden window and message loop,
//! because eframe stops calling the app while the main window is hidden.
//! Plain Win32 through the `windows` crate.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFY_ICON_MESSAGE, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIcon, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    GetCursorPos, GetMessageW, GetSystemMetrics, HICON, IMAGE_ICON, IsIconic, LR_DEFAULTCOLOR, LoadImageW, MF_STRING,
    MSG, PostMessageW, RegisterClassW, RegisterWindowMessageW, SM_CXSMICON, SM_CYSMICON, SW_RESTORE, SW_SHOW,
    SetForegroundWindow, ShowWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CONTEXTMENU, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP,
    WNDCLASSW,
};
use windows::core::{PCWSTR, w};

use crate::history;
use crate::monitor::Shared;

const CLASS: PCWSTR = w!("StartupMonitorTrayWindow");
/// From the tray icon; `lparam` is the mouse message.
const WM_TRAY: u32 = WM_APP + 1;
/// The collector's state changed: bring the tooltip up to date.
const WM_REFRESH: u32 = WM_APP + 2;
const ICON_ID: u32 = 1;
const MENU_OPEN: usize = 1;
const MENU_QUIT: usize = 2;

/// The tray thread's hidden window, 0 while there is none.
static WINDOW: AtomicIsize = AtomicIsize::new(0);
/// The user asked for the window from the tray; the app picks this up.
static OPEN: AtomicBool = AtomicBool::new(false);

struct State {
    shared: Arc<Mutex<Shared>>,
    /// The main window, as a number so it can cross threads.
    main_window: isize,
    ctx: egui::Context,
    icon: HICON,
    /// The tooltip now shown, to call Windows only on a change.
    tip: String,
    taskbar_created: u32,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn hwnd(value: isize) -> HWND {
    HWND(value as *mut core::ffi::c_void)
}

/// The tooltip: where the sign-in has got to.
pub fn tip(listed: bool, done: bool, finished: bool, progress: (usize, usize), usable_ms: u64) -> String {
    if !listed {
        "Windows Startup Monitor: reading the startup list".to_owned()
    } else if !done {
        format!("Windows Startup Monitor: {} of {} started", progress.0, progress.1)
    } else if !finished {
        format!("Windows Startup Monitor: usable after {}. Still watching for late apps", history::fmt_secs(usable_ms))
    } else {
        format!("Windows Startup Monitor: usable after {}", history::fmt_secs(usable_ms))
    }
}

/// Puts the icon in the notification area. Does nothing if Windows will not
/// create it: the app then works as before, without a way back once hidden.
pub fn start(shared: Arc<Mutex<Shared>>, main_window: isize, ctx: egui::Context) {
    std::thread::spawn(move || unsafe {
        let Some(window) = create_window() else { return };
        STATE.with_borrow_mut(|state| {
            *state = Some(State {
                shared,
                main_window,
                ctx,
                icon: load_icon(),
                tip: String::new(),
                taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            });
        });
        WINDOW.store(window.0 as isize, Ordering::SeqCst);
        update(window, true);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    });
}

/// Tells the tray the collector's state changed. Safe from any thread.
pub fn refresh() {
    let window = WINDOW.load(Ordering::SeqCst);
    if window != 0 {
        unsafe {
            let _ = PostMessageW(Some(hwnd(window)), WM_REFRESH, WPARAM(0), LPARAM(0));
        }
    }
}

/// Takes the icon away. Call before the process ends, or Windows leaves a
/// dead icon behind until the mouse passes over it. Safe from any thread.
pub fn remove() {
    let window = WINDOW.swap(0, Ordering::SeqCst);
    if window != 0 {
        send(NIM_DELETE, &icon_data(hwnd(window)));
    }
}

/// True once after the user asked for the window from the tray.
pub fn take_open_request() -> bool {
    OPEN.swap(false, Ordering::SeqCst)
}

unsafe fn create_window() -> Option<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return None;
        }
        // Never shown. Not a message-only window: those do not get the
        // "TaskbarCreated" broadcast.
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("Windows Startup Monitor"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .ok()
    }
}

/// The app's own icon at tray size: from the exe's resources, or drawn from
/// the window icon if that is missing.
fn load_icon() -> HICON {
    unsafe {
        let (w, h) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        if let Ok(instance) = GetModuleHandleW(None) {
            // winresource embeds assets/app.ico as resource 1.
            let id = PCWSTR(1 as *const u16);
            if let Ok(handle) = LoadImageW(Some(instance.into()), id, IMAGE_ICON, w, h, LR_DEFAULTCOLOR) {
                return HICON(handle.0);
            }
        }
        let mut pixels = include_bytes!("../assets/icon-64.rgba").to_vec();
        // Windows wants blue first.
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        // With 32-bit colour the alpha channel decides transparency; the
        // mask is still required.
        let mask = vec![0u8; 64 * 64 / 8];
        CreateIcon(None, 64, 64, 1, 32, mask.as_ptr(), pixels.as_ptr()).unwrap_or_default()
    }
}

/// Copies `text` into a fixed, NUL-terminated UTF-16 field, cutting it at a
/// character boundary if it is too long.
fn copy_text(field: &mut [u16], text: &str) {
    let mut at = 0;
    for c in text.chars() {
        let mut units = [0u16; 2];
        let units = c.encode_utf16(&mut units);
        if at + units.len() >= field.len() {
            break;
        }
        field[at..at + units.len()].copy_from_slice(units);
        at += units.len();
    }
    field[at] = 0;
}

fn icon_data(window: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: window, uID: ICON_ID, ..Default::default() }
}

fn send(message: NOTIFY_ICON_MESSAGE, data: &NOTIFYICONDATAW) {
    unsafe {
        let _ = Shell_NotifyIconW(message, data);
    }
}

/// Brings the tooltip in line with the collector. `add` puts the icon in
/// the tray (at start, and when Explorer restarts).
fn update(window: HWND, add: bool) {
    // The state is only borrowed for a moment at a time: Windows may call
    // `window_proc` again while a tray call below is waiting for Explorer.
    let Some((shared, icon, shown)) =
        STATE.with_borrow(|state| state.as_ref().map(|s| (s.shared.clone(), s.icon, s.tip.clone())))
    else {
        return;
    };
    let text = {
        let s = shared.lock().unwrap();
        tip(s.listed, s.done, s.finished, s.record.progress(), s.record.usable_ms)
    };
    if !add && text == shown {
        return;
    }
    let mut data = icon_data(window);
    data.uFlags = NIF_MESSAGE | NIF_TIP | NIF_ICON;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = icon;
    copy_text(&mut data.szTip, &text);
    send(if add { NIM_ADD } else { NIM_MODIFY }, &data);
    STATE.with_borrow_mut(|state| {
        if let Some(s) = state.as_mut() {
            s.tip = text;
        }
    });
}

/// Shows the main window and asks the app to open the full view in it.
fn open(window: HWND) {
    let Some((main_window, ctx)) = STATE.with_borrow(|state| state.as_ref().map(|s| (s.main_window, s.ctx.clone())))
    else {
        return;
    };
    let _ = window;
    OPEN.store(true, Ordering::SeqCst);
    unsafe {
        let main = hwnd(main_window);
        let _ = ShowWindow(main, if IsIconic(main).as_bool() { SW_RESTORE } else { SW_SHOW });
        let _ = SetForegroundWindow(main);
    }
    ctx.request_repaint();
}

fn menu(window: HWND) {
    unsafe {
        let Ok(popup) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(popup, MF_STRING, MENU_OPEN, w!("Open Windows Startup Monitor"));
        let _ = AppendMenuW(popup, MF_STRING, MENU_QUIT, w!("Quit"));
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        // Without this the menu does not close when the user clicks away.
        let _ = SetForegroundWindow(window);
        let chosen =
            TrackPopupMenu(popup, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, cursor.x, cursor.y, None, window, None);
        let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(popup);
        match chosen.0 as usize {
            MENU_OPEN => open(window),
            MENU_QUIT => {
                // The record is saved as it changes; a sign-in that is not
                // usable yet is simply not recorded.
                remove();
                std::process::exit(0);
            }
            _ => {}
        }
    }
}

unsafe extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_TRAY => {
            match lparam.0 as u32 & 0xffff {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => open(window),
                WM_RBUTTONUP | WM_CONTEXTMENU => menu(window),
                _ => {}
            }
            LRESULT(0)
        }
        WM_REFRESH => {
            update(window, false);
            LRESULT(0)
        }
        _ => {
            let taskbar_created = STATE.with_borrow(|state| state.as_ref().map(|s| s.taskbar_created)).unwrap_or(0);
            if taskbar_created != 0 && message == taskbar_created {
                // Explorer restarted and forgot every tray icon.
                update(window, true);
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_follows_the_sign_in() {
        assert_eq!(tip(false, false, false, (0, 0), 0), "Windows Startup Monitor: reading the startup list");
        assert_eq!(tip(true, false, false, (9, 15), 0), "Windows Startup Monitor: 9 of 15 started");
        assert_eq!(tip(true, true, false, (15, 15), 48_000), "Windows Startup Monitor: usable after 48 s. Still watching for late apps");
        assert_eq!(tip(true, true, true, (15, 15), 48_000), "Windows Startup Monitor: usable after 48 s");
    }

    #[test]
    fn text_is_cut_to_the_field_and_terminated() {
        let mut field = [0xffffu16; 6];
        copy_text(&mut field, "ab");
        assert_eq!(&field[..3], [b'a' as u16, b'b' as u16, 0]);
        copy_text(&mut field, "abcdefgh");
        assert_eq!(String::from_utf16_lossy(&field[..5]), "abcde");
        assert_eq!(field[5], 0);
    }
}
