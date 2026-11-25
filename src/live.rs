//! The link between the collector, which watches a sign-in with no window
//! of its own beyond the overlay, and the full window, which is a separate
//! process started only when the user opens it. Keeping the window out of
//! the collector keeps the collector to a few megabytes.
//!
//! The window finds the collector by its tray window's class and talks to
//! it with `WM_COPYDATA`: it asks for a snapshot about once a second, and
//! the collector answers by sending one back to the window's own hidden
//! window; requests (end an app, pause) go the same way. Nothing is written
//! to disk, and the collector does no work for it unless a window asks.

use std::cell::RefCell;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, FindWindowW, HWND_MESSAGE, RegisterClassW, SMTO_ABORTIFHUNG, SendMessageTimeoutW,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_COPYDATA, WNDCLASSW,
};
use windows::core::{PCWSTR, w};

use crate::history::{self, BootRecord};
use crate::monitor::{Handle, Metrics, Request, Shared};
use crate::pause::PauseView;

/// `dwData` of a request from the window.
const KIND_REQUEST: usize = 0x5259_0001;
/// `dwData` of a snapshot sent back.
const KIND_SNAPSHOT: usize = 0x5259_0002;
const TIMEOUT_MS: u32 = 2_000;

/// What the window needs of the collector's state.
#[derive(Serialize, Deserialize, Clone)]
pub struct Snapshot {
    pub record: BootRecord,
    pub metrics: Metrics,
    pub listed: bool,
    pub done: bool,
    pub finished: bool,
    pub note: Option<String>,
    pub pause: Option<PauseView>,
}

impl Snapshot {
    pub fn of(s: &Shared) -> Self {
        Self {
            record: s.record.clone(),
            metrics: s.metrics,
            listed: s.listed,
            done: s.done,
            finished: s.finished,
            note: s.note.clone(),
            pause: s.pause.clone(),
        }
    }
}

pub fn encode(r: &Request) -> String {
    match r {
        Request::End(id) => format!("end\n{id}"),
        Request::EndWhenSeen(id) => format!("endseen\n{id}"),
        Request::Pause => "pause".to_owned(),
        Request::Resume => "resume".to_owned(),
    }
}

pub fn decode(text: &str) -> Option<Request> {
    let (verb, id) = text.split_once('\n').unwrap_or((text, ""));
    match verb {
        "end" if !id.is_empty() => Some(Request::End(id.to_owned())),
        "endseen" if !id.is_empty() => Some(Request::EndWhenSeen(id.to_owned())),
        "pause" => Some(Request::Pause),
        "resume" => Some(Request::Resume),
        _ => None,
    }
}

fn send_copy(to: HWND, from: HWND, kind: usize, bytes: &[u8]) -> bool {
    let data = COPYDATASTRUCT { dwData: kind, cbData: bytes.len() as u32, lpData: bytes.as_ptr() as *mut _ };
    unsafe {
        SendMessageTimeoutW(
            to,
            WM_COPYDATA,
            WPARAM(from.0 as usize),
            LPARAM(&data as *const COPYDATASTRUCT as isize),
            SMTO_ABORTIFHUNG,
            TIMEOUT_MS,
            None,
        )
        .0 != 0
    }
}

unsafe fn copied(lparam: LPARAM) -> (usize, &'static [u8]) {
    unsafe {
        let data = &*(lparam.0 as *const COPYDATASTRUCT);
        let bytes = if data.lpData.is_null() || data.cbData == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize)
        };
        (data.dwData, bytes)
    }
}

// ------------------------------------------------------------ collector

/// The collector's side: a `WM_COPYDATA` that reached its tray window.
/// "snapshot" is answered by sending one to the asking window; anything
/// else is a request for the collector. Returns false if it was not ours.
pub unsafe fn on_copydata(handle: &Handle, own: HWND, wparam: WPARAM, lparam: LPARAM) -> bool {
    let (kind, bytes) = unsafe { copied(lparam) };
    if kind != KIND_REQUEST {
        return false;
    }
    let text = String::from_utf8_lossy(bytes);
    if text == "snapshot" {
        let snapshot = Snapshot::of(&handle.shared.lock().unwrap());
        if let Ok(json) = serde_json::to_vec(&snapshot) {
            send_copy(HWND(wparam.0 as *mut _), own, KIND_SNAPSHOT, &json);
        }
    } else if let Some(r) = decode(&text) {
        handle.send(r);
    }
    true
}

// ------------------------------------------------------------ window

thread_local! {
    static INBOX: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

unsafe extern "system" fn link_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_COPYDATA {
        let (kind, bytes) = unsafe { copied(lparam) };
        if kind == KIND_SNAPSHOT {
            INBOX.with_borrow_mut(|inbox| *inbox = Some(bytes.to_vec()));
            return LRESULT(1);
        }
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

fn collector() -> Option<HWND> {
    unsafe { FindWindowW(crate::brand::TRAY_CLASS, PCWSTR::null()).ok().filter(|h| !h.is_invalid()) }
}

fn link_window() -> Option<HWND> {
    const CLASS: PCWSTR = w!("LiveLinkWindow");
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSW { lpfnWndProc: Some(link_proc), hInstance: instance.into(), lpszClassName: CLASS, ..Default::default() };
        RegisterClassW(&class);
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            PCWSTR::null(),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )
        .ok()
    }
}

fn fetch(collector: HWND, own: HWND) -> Option<Snapshot> {
    INBOX.with_borrow_mut(|inbox| *inbox = None);
    if !send_copy(collector, own, KIND_REQUEST, b"snapshot") {
        return None;
    }
    let bytes = INBOX.with_borrow_mut(|inbox| inbox.take())?;
    serde_json::from_slice(&bytes).ok()
}

fn apply(shared: &Mutex<Shared>, snap: Snapshot, last_note: &mut Option<String>) {
    let mut s = shared.lock().unwrap();
    s.record = snap.record;
    s.metrics = snap.metrics;
    s.listed = snap.listed;
    s.done = snap.done;
    s.finished = snap.finished;
    s.pause = snap.pause;
    if snap.note.is_some() && snap.note != *last_note {
        s.note = snap.note.clone();
        *last_note = snap.note;
    }
}

/// The window's side: connects to a collector watching this sign-in, if
/// one is running, and keeps a copy of its state up to date about once a
/// second. Requests sent through the handle go to the collector.
pub fn attach(repaint: impl Fn() + Send + 'static) -> Option<Handle> {
    collector()?;
    let (ready_tx, ready_rx) = channel::<Option<Handle>>();
    std::thread::spawn(move || {
        let Some(own) = link_window() else {
            let _ = ready_tx.send(None);
            return;
        };
        let Some(first) = collector().and_then(|c| fetch(c, own)) else {
            let _ = ready_tx.send(None);
            return;
        };
        let sign_in = first.record.sign_in;
        let mut last_note = first.note.clone();
        let shared = Arc::new(Mutex::new(Shared {
            record: first.record,
            metrics: first.metrics,
            listed: first.listed,
            done: first.done,
            earlier: history::load().into_iter().filter(|b| b.sign_in != sign_in).collect(),
            note: None,
            finished: first.finished,
            exit_when_done: false,
            frames: 0,
            events: Vec::new(),
            pause: first.pause,
        }));
        let (tx, rx) = channel();
        let _ = ready_tx.send(Some(Handle::new(shared.clone(), tx)));
        loop {
            let request = match rx.recv_timeout(Duration::from_millis(1000)) {
                Ok(r) => Some(r),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            let Some(c) = collector() else {
                // The collector has finished (or was quit): what was last
                // seen stands.
                shared.lock().unwrap().finished = true;
                repaint();
                return;
            };
            if let Some(r) = request {
                send_copy(c, own, KIND_REQUEST, encode(&r).as_bytes());
            }
            if let Some(snap) = fetch(c, own) {
                apply(&shared, snap, &mut last_note);
                repaint();
            }
        }
    });
    ready_rx.recv_timeout(Duration::from_secs(3)).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_survive_the_trip() {
        for r in [Request::End("run:hkcu:teams".into()), Request::EndWhenSeen("task:\\x".into()), Request::Pause, Request::Resume] {
            let back = decode(&encode(&r)).unwrap();
            assert_eq!(encode(&back), encode(&r));
        }
        assert!(decode("end\n").is_none() && decode("nonsense").is_none());
    }
}
