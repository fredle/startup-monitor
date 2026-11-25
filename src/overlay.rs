//! The small window by the notification area while a sign-in is watched:
//! how far startup has got, with a way to pause the rest and to open the
//! full window. Drawn with plain Win32 and GDI, not egui: an egui window
//! holds some 180 MB for its graphics context and redraws to animate, while
//! this holds a few hundred kilobytes and redraws only when the collector
//! has news, about once a second.
//!
//! It never takes the keyboard focus, so a click on it does not pull the
//! user out of what they are doing.

use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW,
    CreatePen, CreateSolidBrush, DEFAULT_CHARSET, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE,
    DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, EndPaint, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, FillRect,
    GetStockObject, GetTextExtentPoint32W, HDC, HFONT, HGDIOBJ, InvalidateRect, NULL_PEN, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, PS_SOLID, RoundRect, SRCCOPY, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetMessageW, HTCLIENT, IDC_ARROW, IDC_HAND,
    IsWindowVisible, KillTimer, LoadCursorW, MA_NOACTIVATE, MSG, PostMessageW, RegisterClassW, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER, SetCursor, SetTimer, SetWindowPos, ShowWindow, TranslateMessage,
    WM_APP, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_PAINT, WM_SETCURSOR, WM_TIMER, WNDCLASSW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use eframe::egui;

use crate::history;
use crate::monitor::{Handle, Request};
use crate::theme::{self, Palette, Tone};

const CLASS: PCWSTR = w!("OverlayWindow");
/// The collector has news: redraw.
const WM_REFRESH: u32 = WM_APP + 10;
const HIDE_TIMER: usize = 1;
/// Seconds "Ready" stays up before the overlay hides itself.
const READY_LINGER_MS: u32 = 8_000;
/// Size in points; scaled for the screen.
const WIDTH: i32 = 330;
const HEIGHT: i32 = 186;
const PAD: i32 = 14;

static WINDOW: AtomicIsize = AtomicIsize::new(0);

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Close,
    Open,
    Pause,
    Resume,
    Cost,
}

struct State {
    handle: Handle,
    hits: Vec<(RECT, Hit)>,
    /// The user closed it: it stays closed.
    dismissed: bool,
    /// The hide timer is set for "Ready".
    hiding: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    /// Fonts made so far, by (kind, pixel height): made once, kept.
    static FONTS: RefCell<Vec<((u8, i32), HFONT)>> = const { RefCell::new(Vec::new()) };
}

/// Tells the overlay the collector's state changed. Safe from any thread.
pub fn refresh() {
    let window = WINDOW.load(Ordering::SeqCst);
    if window != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(window as *mut _)), WM_REFRESH, WPARAM(0), LPARAM(0));
        }
    }
}

/// Opens the full window: a separate process, so its memory is given back
/// when it closes. `tab` picks the tab it opens on.
pub fn open_window(tab: Option<&str>) {
    use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
    unsafe {
        // Let the new window come to the front: the click was ours.
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut cmd = std::process::Command::new(exe);
        if let Some(tab) = tab {
            cmd.args(["--tab", tab]);
        }
        let _ = cmd.spawn();
    }
}

/// Runs the overlay on this thread until the process ends. It appears once
/// the startup list has been read.
pub fn run(handle: Handle) -> ! {
    unsafe {
        let Some(window) = create() else {
            // No overlay: the tray icon is still there.
            loop {
                std::thread::park();
            }
        };
        STATE.with_borrow_mut(|s| *s = Some(State { handle, hits: Vec::new(), dismissed: false, hiding: false }));
        WINDOW.store(window.0 as isize, Ordering::SeqCst);
        on_refresh(window);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        loop {
            std::thread::park();
        }
    }
}

unsafe fn create() -> Option<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return None;
        }
        let title = crate::win::wide(crate::brand::NAME);
        let window = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            CLASS,
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            0,
            0,
            WIDTH,
            HEIGHT,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .ok()?;
        let corner = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const _,
            size_of_val(&corner) as u32,
        );
        place(window);
        Some(window)
    }
}

fn scale(window: HWND) -> f32 {
    unsafe { GetDpiForWindow(window).max(96) as f32 / 96.0 }
}

/// Bottom right of the work area, clear of the taskbar.
fn place(window: HWND) {
    let s = scale(window);
    let (w, h) = ((WIDTH as f32 * s) as i32, (HEIGHT as f32 * s) as i32);
    let (right, bottom) = crate::win::work_area_points()
        .map(|(_, _, r, b)| {
            let sys = crate::win::system_scale();
            ((r * sys) as i32, (b * sys) as i32)
        })
        .unwrap_or((1280, 720));
    let gap = (16.0 * s) as i32;
    unsafe {
        let _ = SetWindowPos(window, None, right - w - gap, bottom - h - gap, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

fn on_refresh(window: HWND) {
    let Some((listed, done, paused, dismissed, hiding)) = STATE.with_borrow(|s| {
        s.as_ref().map(|st| {
            let sh = st.handle.shared.lock().unwrap();
            (sh.listed, sh.done, sh.pause.is_some(), st.dismissed, st.hiding)
        })
    }) else {
        return;
    };
    unsafe {
        if listed && !dismissed && !IsWindowVisible(window).as_bool() && (!done || paused) {
            let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
        }
        if done && !paused && !hiding {
            SetTimer(Some(window), HIDE_TIMER, READY_LINGER_MS, None);
            STATE.with_borrow_mut(|s| {
                if let Some(s) = s.as_mut() {
                    s.hiding = true;
                }
            });
        }
        if paused && hiding {
            let _ = KillTimer(Some(window), HIDE_TIMER);
            STATE.with_borrow_mut(|s| {
                if let Some(s) = s.as_mut() {
                    s.hiding = false;
                }
            });
        }
        let _ = InvalidateRect(Some(window), None, false);
    }
}

fn hide(window: HWND) {
    unsafe {
        let _ = ShowWindow(window, SW_HIDE);
    }
}

fn hit_at(lparam: LPARAM) -> Option<Hit> {
    let (x, y) = ((lparam.0 & 0xffff) as i16 as i32, ((lparam.0 >> 16) & 0xffff) as i16 as i32);
    STATE.with_borrow(|s| {
        s.as_ref().and_then(|s| {
            s.hits.iter().find(|(r, _)| x >= r.left && x < r.right && y >= r.top && y < r.bottom).map(|(_, h)| *h)
        })
    })
}

fn click(window: HWND, hit: Option<Hit>) {
    let send = |r: Request| STATE.with_borrow(|s| s.as_ref().map(|s| s.handle.send(r)));
    match hit {
        Some(Hit::Close) => {
            STATE.with_borrow_mut(|s| {
                if let Some(s) = s.as_mut() {
                    s.dismissed = true;
                }
            });
            hide(window);
        }
        Some(Hit::Pause) => {
            send(Request::Pause);
        }
        Some(Hit::Resume) => {
            send(Request::Resume);
        }
        Some(Hit::Cost) => open_window(Some("cost")),
        Some(Hit::Open) | None => open_window(None),
    }
}

unsafe extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_PAINT => {
                paint(window);
                LRESULT(0)
            }
            WM_REFRESH => {
                on_refresh(window);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == HIDE_TIMER => {
                let _ = KillTimer(Some(window), HIDE_TIMER);
                let paused = STATE.with_borrow(|s| s.as_ref().is_some_and(|s| s.handle.shared.lock().unwrap().pause.is_some()));
                if !paused {
                    hide(window);
                }
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_LBUTTONUP => {
                click(window, hit_at(lparam));
                LRESULT(0)
            }
            WM_SETCURSOR if (lparam.0 & 0xffff) as u32 == HTCLIENT => {
                // A hand over the controls; the rest opens the window too,
                // but reads as a surface.
                let mut point = windows::Win32::Foundation::POINT::default();
                let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point);
                let _ = windows::Win32::Graphics::Gdi::ScreenToClient(window, &mut point);
                let over = hit_at(LPARAM(((point.y as isize & 0xffff) << 16) | (point.x as isize & 0xffff)));
                let cursor = if over.is_some() { IDC_HAND } else { IDC_ARROW };
                SetCursor(LoadCursorW(None, cursor).ok());
                LRESULT(1)
            }
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }
}

// ------------------------------------------------------------ drawing

/// What the overlay shows, copied out so the lock is held only briefly.
struct OverlayView {
    done: bool,
    usable_ms: u64,
    totals: Vec<u64>,
    metrics: crate::monitor::Metrics,
    yours: (usize, usize),
    system: (usize, usize),
    pause: Option<crate::pause::PauseView>,
}

impl OverlayView {
    fn of(s: &crate::monitor::Shared) -> Self {
        let (yours, system) = s.record.progress_split();
        Self {
            done: s.done,
            usable_ms: s.record.usable_ms,
            totals: history::recent_totals(&s.earlier),
            metrics: s.metrics,
            yours,
            system,
            pause: s.pause.clone(),
        }
    }
}

fn rgb(c: egui::Color32) -> COLORREF {
    COLORREF(c.r() as u32 | (c.g() as u32) << 8 | (c.b() as u32) << 16)
}

struct Painter {
    dc: HDC,
    s: f32,
}

#[derive(Clone, Copy)]
enum Font {
    Body,
    Small,
    Strong,
    Title,
    Icons,
}

impl Painter {
    fn px(&self, v: i32) -> i32 {
        (v as f32 * self.s).round() as i32
    }

    fn font(&mut self, f: Font) -> HFONT {
        let key = (f as u8, self.px(match f {
            Font::Body | Font::Strong => 13,
            Font::Small => 12,
            Font::Title => 19,
            Font::Icons => 11,
        }));
        if let Some(font) = FONTS.with_borrow(|fonts| fonts.iter().find(|(k, _)| *k == key).map(|(_, f)| *f)) {
            return font;
        }
        let (size, weight, face) = match f {
            Font::Body => (13, FW_NORMAL, w!("Segoe UI")),
            Font::Small => (12, FW_NORMAL, w!("Segoe UI")),
            Font::Strong => (13, FW_SEMIBOLD, w!("Segoe UI")),
            Font::Title => (19, FW_SEMIBOLD, w!("Segoe UI")),
            Font::Icons => (11, FW_NORMAL, w!("Segoe MDL2 Assets")),
        };
        let font = unsafe {
            CreateFontW(
                -self.px(size),
                0,
                0,
                0,
                weight.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                FF_DONTCARE.0 as u32,
                face,
            )
        };
        FONTS.with_borrow_mut(|fonts| fonts.push((key, font)));
        font
    }

    fn rect_fill(&self, r: RECT, c: egui::Color32, radius: i32) {
        unsafe {
            let brush = CreateSolidBrush(rgb(c));
            if radius == 0 {
                FillRect(self.dc, &r, brush);
            } else {
                let old_pen = SelectObject(self.dc, GetStockObject(NULL_PEN));
                let old_brush = SelectObject(self.dc, HGDIOBJ(brush.0));
                let d = self.px(radius) * 2;
                let _ = RoundRect(self.dc, r.left, r.top, r.right + 1, r.bottom + 1, d, d);
                SelectObject(self.dc, old_brush);
                SelectObject(self.dc, old_pen);
            }
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }
    }

    fn rect_outline(&self, r: RECT, fill: egui::Color32, line: egui::Color32, radius: i32) {
        unsafe {
            let brush = CreateSolidBrush(rgb(fill));
            let pen = CreatePen(PS_SOLID, self.px(1).max(1), rgb(line));
            let old_pen = SelectObject(self.dc, HGDIOBJ(pen.0));
            let old_brush = SelectObject(self.dc, HGDIOBJ(brush.0));
            let d = self.px(radius) * 2;
            let _ = RoundRect(self.dc, r.left, r.top, r.right, r.bottom, d, d);
            SelectObject(self.dc, old_brush);
            SelectObject(self.dc, old_pen);
            let _ = DeleteObject(HGDIOBJ(brush.0));
            let _ = DeleteObject(HGDIOBJ(pen.0));
        }
    }

    fn text_width(&mut self, text: &str, f: Font) -> i32 {
        let font = self.font(f);
        let w: Vec<u16> = text.encode_utf16().collect();
        let mut size = windows::Win32::Foundation::SIZE::default();
        unsafe {
            let old = SelectObject(self.dc, HGDIOBJ(font.0));
            let _ = GetTextExtentPoint32W(self.dc, &w, &mut size);
            SelectObject(self.dc, old);
        }
        size.cx
    }

    /// Draws `text` in `r`, vertically centred; right-aligned for `right`.
    fn text(&mut self, text: &str, mut r: RECT, f: Font, c: egui::Color32, right: bool) {
        let font = self.font(f);
        let mut w: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let old = SelectObject(self.dc, HGDIOBJ(font.0));
            SetTextColor(self.dc, rgb(c));
            let align = if right { DT_RIGHT } else { DT_LEFT };
            DrawTextW(self.dc, &mut w, &mut r, align | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
            SelectObject(self.dc, old);
        }
    }

    fn rect(&self, x: i32, y: i32, w: i32, h: i32) -> RECT {
        RECT { left: self.px(x), top: self.px(y), right: self.px(x + w), bottom: self.px(y + h) }
    }

    /// A pill sized to its text; returns its width in points.
    fn pill(&mut self, x: i32, y: i32, text: &str, fg: egui::Color32, bg: egui::Color32) -> i32 {
        let tw = (self.text_width(text, Font::Strong) as f32 / self.s).ceil() as i32;
        let w = tw + 20;
        self.rect_fill(self.rect(x, y, w, 22), bg, 11);
        self.text(text, self.rect(x + 10, y, tw + 2, 22), Font::Strong, fg, false);
        w
    }

    /// A quiet bordered button; returns its rectangle in pixels.
    fn button(&mut self, x: i32, y: i32, text: &str, p: &Palette, right: bool) -> RECT {
        let tw = (self.text_width(text, Font::Body) as f32 / self.s).ceil() as i32;
        let w = tw + 24;
        let x = if right { x - w } else { x };
        let r = self.rect(x, y, w, 28);
        self.rect_outline(r, p.control, p.border, theme::CONTROL_RADIUS as i32);
        self.text(text, self.rect(x + 12, y, tw + 2, 28), Font::Body, p.text, false);
        r
    }

    fn bar(&self, r: RECT, fraction: f32, p: &Palette) {
        self.rect_fill(r, p.track, 3);
        let w = ((r.right - r.left) as f32 * fraction.clamp(0.0, 1.0)).round() as i32;
        if w > 0 {
            self.rect_fill(RECT { right: r.left + w.max(self.px(6)), ..r }, p.accent, 3);
        }
    }
}

unsafe fn paint(window: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let screen = BeginPaint(window, &mut ps);
        let mut client = RECT::default();
        let _ = GetClientRect(window, &mut client);
        let (w, h) = (client.right, client.bottom);
        // Drawn off screen and copied in one go: no flicker.
        let dc = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        let old_bitmap = SelectObject(dc, HGDIOBJ(bitmap.0));
        SetBkMode(dc, TRANSPARENT);
        let mut painter = Painter { dc, s: scale(window) };
        let hits = STATE.with_borrow(|s| s.as_ref().map(|st| draw(&mut painter, &st.handle, w))).unwrap_or_default();
        STATE.with_borrow_mut(|s| {
            if let Some(s) = s.as_mut() {
                s.hits = hits;
            }
        });
        let _ = BitBlt(screen, 0, 0, w, h, Some(dc), 0, 0, SRCCOPY);
        SelectObject(dc, old_bitmap);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(dc);
        let _ = EndPaint(window, &ps);
    }
}

fn draw(g: &mut Painter, handle: &Handle, width_px: i32) -> Vec<(RECT, Hit)> {
    let p = theme::palette_for(crate::win::apps_use_dark());
    let s = OverlayView::of(&handle.shared.lock().unwrap());
    let width = (width_px as f32 / g.s).round() as i32;
    let mut hits = Vec::new();

    g.rect_fill(RECT { left: 0, top: 0, right: width_px, bottom: g.px(HEIGHT) + 2 }, p.surface, 0);
    if theme::OVERLAY_RULE {
        g.rect_fill(g.rect(0, 0, 4, HEIGHT), p.accent, 0);
    }

    // Header: state, clock, and the two window buttons.
    let y = 12;
    if s.done {
        g.pill(PAD, y, "Ready", p.success, p.success_soft);
    } else {
        let pw = g.pill(PAD, y, "Starting up", p.accent_text, p.accent_soft);
        g.text(&crate::app::fmt_clock(s.metrics.elapsed_ms), g.rect(PAD + pw + 8, y, 60, 22), Font::Strong, p.text, false);
    }
    let close = g.rect(width - PAD - 22, y, 22, 22);
    g.text("\u{E8BB}", close, Font::Icons, p.muted, true);
    hits.push((close, Hit::Close));
    let open = g.rect(width - PAD - 52, y, 22, 22);
    g.text("\u{E8A7}", open, Font::Icons, p.muted, true);
    hits.push((open, Hit::Open));

    let mut y = 46;
    if s.done {
        g.text(&format!("Usable after {}", history::fmt_secs(s.usable_ms)), g.rect(PAD, y, width - 2 * PAD, 26), Font::Title, p.text, false);
        y += 28;
        let (line, tone) = crate::app::comparison(s.usable_ms, &s.totals).unwrap_or(("First watched sign-in.".to_owned(), Tone::Neutral));
        let colour = if tone == Tone::Neutral { p.muted } else { theme::tone_colors(p, tone).0 };
        g.text(&line, g.rect(PAD, y, width - 2 * PAD, 18), Font::Small, colour, false);
        y += 22;
        let link = "See what took the time";
        let lw = (g.text_width(link, Font::Body) as f32 / g.s).ceil() as i32;
        let r = g.rect(PAD, y, lw + 2, 20);
        g.text(link, r, Font::Body, p.accent_text, false);
        hits.push((r, Hit::Cost));
    } else {
        // One bar for what runs as the user, one for what runs as Windows.
        let rows = [("Your apps", s.yours), ("System", s.system)];
        for (label, (done, total)) in rows {
            g.text(label, g.rect(PAD, y, 80, 20), Font::Strong, p.text, false);
            let bar_left = PAD + 80;
            let bar_right = width - PAD - 58;
            // Nothing of a kind to wait for: an empty track, not a full bar.
            let fraction = if total > 0 { done as f32 / total as f32 } else { 0.0 };
            g.bar(g.rect(bar_left, y + 7, bar_right - bar_left, 6), fraction, &p);
            g.text(&format!("{done} of {total}"), g.rect(bar_right, y, width - PAD - bar_right, 20), Font::Small, p.muted, true);
            y += 22;
        }
        y += 2;
        g.text(&crate::app::eta_text(s.metrics.eta_ms), g.rect(PAD, y, width - 2 * PAD, 18), Font::Small, p.muted, false);
        y += 18;
        let m = s.metrics;
        let line = format!("CPU {:.0}%   Memory {:.0}%   Disk {}/s", m.cpu, m.mem, history::fmt_bytes(m.disk as u64));
        g.text(&line, g.rect(PAD, y, width - 2 * PAD, 18), Font::Small, p.muted, false);
    }

    // The pause row, at the bottom.
    let y = HEIGHT - PAD - 28;
    match &s.pause {
        Some(pause) => {
            let left = pause.until_ms.saturating_sub(s.metrics.elapsed_ms).div_ceil(1000) * 1000;
            let what = match pause.items.len() {
                0 => "Pausing startup apps".to_owned(),
                1 => "1 app paused".to_owned(),
                n => format!("{n} apps paused"),
            };
            let r = g.button(width - PAD, y, "Resume now", &p, true);
            hits.push((r, Hit::Resume));
            let text_w = ((r.left as f32 / g.s) as i32) - PAD - 8;
            g.text(&format!("{what} · {} left", history::fmt_secs(left)), g.rect(PAD, y, text_w, 28), Font::Small, p.text, false);
        }
        None if !s.done => {
            let r = g.button(PAD, y, "Pause the rest", &p, false);
            hits.push((r, Hit::Pause));
            let x = (r.right as f32 / g.s) as i32 + 10;
            g.text("Enough already? Get to work first.", g.rect(x, y, width - PAD - x, 28), Font::Small, p.muted, false);
        }
        None => {}
    }
    hits
}
