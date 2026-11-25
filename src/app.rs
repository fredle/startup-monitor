//! The full window, and the entry point for both ways the app runs.
//!
//! Run with `--collect` (at sign-in) it watches with no egui at all: the
//! collector, a tray icon and a small GDI overlay (`overlay.rs`), a few
//! megabytes in all. The full window is this egui app in a process of its
//! own, started only when the user opens it; while a sign-in is being
//! watched it shows it live through `live.rs`, and when it closes its
//! memory goes back to Windows. The footer with the auto-update toggle and
//! version is required on every factory app.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Align, Layout, Rect, RichText, Sense, pos2, vec2};

use crate::history::{self, BootRecord, ItemRecord, Mode, Status};
use crate::monitor::{self, Metrics};
use crate::shots::Shots;
use crate::startup::{Item, Kind, Source};
use crate::theme::{self, Icon, Palette, Tone};
use crate::{autostart, win};

const WIDTH: f32 = 600.0;
const HEIGHT: f32 = 680.0;
/// How many scenes screenshot mode captures (see `show_scene`).
const SHOT_SCENES: usize = 4;

/// The app's own icon for the title bar and taskbar, drawn by
/// `assets/make-icon.ps1`. Without it the window shows egui's logo.
fn window_icon() -> egui::IconData {
    egui::IconData {
        rgba: include_bytes!("../assets/icon-64.rgba").to_vec(),
        width: 64,
        height: 64,
    }
}

pub fn run(args: Vec<String>) -> eframe::Result<()> {
    let shots = args.iter().any(|a| a == "--shots");
    if !shots && args.iter().any(|a| a == autostart::COLLECT_ARG) {
        collect(&args);
        return Ok(());
    }
    // One full window: opening it again brings the open one forward.
    let _window_guard = if shots {
        None
    } else {
        let guard = win::single_instance(&format!(r"Local\{}Window", crate::brand::ID));
        if guard.is_none() {
            win::focus_existing_window();
            return Ok(());
        }
        guard
    };
    let viewport = egui::ViewportBuilder::default()
        .with_title(crate::brand::NAME)
        .with_icon(window_icon())
        .with_inner_size([WIDTH, HEIGHT])
        .with_min_inner_size([520.0, 560.0]);
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(crate::brand::NAME, options, Box::new(move |cc| Ok(Box::new(App::new(&args, &cc.egui_ctx)))))
}

/// Watches this sign-in: the collector, the tray icon and the overlay, with
/// no egui. Never returns: the collector ends the process when done.
fn collect(args: &[String]) {
    let settings = crate::settings::load();
    if !settings.watch {
        return;
    }
    // One collector per session, even if both the Run entry and the early
    // task fire.
    let Some(_guard) = win::single_instance(crate::brand::COLLECTOR_MUTEX) else { return };
    // Anything a pause in an earlier run left frozen carries on.
    crate::pause::release_all();
    win::dpi_aware();
    let mode = if args.iter().any(|a| a == autostart::EARLY_ARG) { Mode::Early } else { Mode::Standard };
    let handle = monitor::spawn(mode, settings.delayed.clone(), || {
        crate::tray::refresh();
        crate::overlay::refresh();
    });
    crate::tray::start(handle.clone());
    crate::overlay::run(handle);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Items,
    Timeline,
    Cost,
}

const TABS: [(Tab, &str); 3] = [(Tab::Items, "Startup items"), (Tab::Timeline, "Timeline"), (Tab::Cost, "What it cost")];

/// What the window shows this frame.
#[derive(Clone)]
struct View {
    record: BootRecord,
    /// Time-to-usable of earlier boots, oldest first.
    totals: Vec<u64>,
    /// Live readings, while the collector runs.
    metrics: Option<Metrics>,
    /// The boot is over and the record is final.
    done: bool,
    /// The startup list has been read.
    listed: bool,
}

enum Action {
    End(Item),
    EndWhenSeen(Item),
    TurnOff(Item),
    Delay(Item),
    Undelay(Item),
    TurnOn(Item),
}

pub struct App {
    settings: crate::settings::Settings,
    version: String,
    shots: Option<Shots>,
    live: Option<monitor::Handle>,
    /// The East Asian fallback fonts are loaded.
    wide_fonts: bool,
    /// Showing the first-launch question about starting earlier.
    ask_early: bool,
    /// Normal launch: the last boot and its predecessors' totals.
    report: Option<(BootRecord, Vec<u64>)>,
    /// Normal launch with no history: what is set to start.
    configured: Arc<Mutex<Option<Vec<Item>>>>,
    /// Screenshot mode's made-up view.
    fake: Option<View>,
    tab: Tab,
    open_row: Option<String>,
    show_services: bool,
    options: bool,
    note: Option<(String, Tone, f64)>,
    /// A "pause the rest" in progress, as of this frame.
    pause: Option<crate::pause::PauseView>,
}

impl App {
    fn new(args: &[String], ctx: &egui::Context) -> Self {
        theme::apply(ctx);
        // A sign-in still being watched: show it live.
        let live = if args.iter().any(|a| a == "--shots") {
            None
        } else {
            let repaint = ctx.clone();
            crate::live::attach(move || repaint.request_repaint())
        };
        // The wide fonts are a big read: not while startup is being timed.
        let timing = live.as_ref().is_some_and(|h| !h.shared.lock().unwrap().done);
        theme::install_fonts(ctx, !timing);
        // For checking both looks without changing Windows' setting.
        if let Some(theme) = crate::shots::theme_from_args(args) {
            ctx.set_theme(theme);
        }
        let shots = Shots::from_args(args);
        let mut app = Self {
            settings: crate::settings::load(),
            version: crate::update::current_version(),
            shots,
            live: None,
            wide_fonts: !timing,
            ask_early: false,
            report: None,
            configured: Arc::new(Mutex::new(None)),
            fake: None,
            tab: if args.windows(2).any(|a| a[0] == "--tab" && a[1] == "cost") { Tab::Cost } else { Tab::Items },
            open_row: None,
            show_services: false,
            options: false,
            note: None,
            pause: None,
        };
        if app.shots.is_some() {
            // Screenshots show the app as a new user first sees it.
            ctx.set_pixels_per_point(1.0);
            app.settings = crate::settings::Settings::default();
            app.version = env!("CARGO_PKG_VERSION").to_owned();
            return app;
        }
        if !app.settings.set_up {
            // First run: watch from the next sign-in, the default.
            if app.settings.watch && !app.settings.early {
                autostart::set_standard(true);
            }
            app.settings.set_up = true;
            app.save();
        } else if app.settings.watch && !app.settings.early && !autostart::standard_is_set() {
            // The Run entry was removed elsewhere: respect that.
            app.settings.watch = false;
            app.save();
        }
        // Asked once, the first time the app is opened by hand: that is before
        // the first watched sign-in, when the answer matters most.
        app.ask_early = live.is_none() && app.settings.watch && !app.settings.early && !app.settings.asked_early;
        if let Some(handle) = live {
            app.live = Some(handle);
        } else {
            let all = history::load();
            if let Some((last, earlier)) = all.split_last() {
                app.report = Some((last.clone(), history::recent_totals(earlier)));
            } else {
                let slot = app.configured.clone();
                let repaint = ctx.clone();
                std::thread::spawn(move || {
                    win::com_init();
                    let mut items = crate::startup::gather();
                    items.retain(|i| i.kind() != Kind::Service);
                    *slot.lock().unwrap() = Some(items);
                    repaint.request_repaint();
                });
            }
        }
        app
    }

    /// Saves settings, except in screenshot mode.
    fn save(&self) {
        if self.shots.is_none() {
            crate::settings::save(&self.settings);
        }
    }

    fn view(&self) -> Option<View> {
        if let Some(v) = &self.fake {
            return Some(v.clone());
        }
        if let Some(h) = &self.live {
            let s = h.shared.lock().unwrap();
            return Some(View {
                record: s.record.clone(),
                totals: history::recent_totals(&s.earlier),
                metrics: Some(s.metrics),
                done: s.done,
                listed: s.listed,
            });
        }
        self.report.as_ref().map(|(record, totals)| View {
            record: record.clone(),
            totals: totals.clone(),
            metrics: None,
            done: true,
            listed: true,
        })
    }

    fn say(&mut self, ctx: &egui::Context, text: impl Into<String>, tone: Tone) {
        self.note = Some((text.into(), tone, ctx.input(|i| i.time)));
    }

    /// A line for the collector's diagnostics log.
    fn log_event(&self, text: &str) {
        if let Some(h) = &self.live {
            h.shared.lock().unwrap().events.push(text.to_owned());
        }
    }

    /// The full window lists item names, which may need the East Asian
    /// fonts. They are some 50 MB to read, enough to count as a busy disk and
    /// push "usable" back, so they wait until the timing is over.
    fn load_wide_fonts_when_free(&mut self, ctx: &egui::Context) {
        if self.wide_fonts {
            return;
        }
        if self.live.as_ref().is_some_and(|h| !h.shared.lock().unwrap().done) {
            return;
        }
        self.wide_fonts = true;
        theme::install_fonts(ctx, true);
        self.log_event("wide fonts loaded");
    }

    // ------------------------------------------------------------ scenes

    /// Puts the app into made-up state number `scene` (0-based) for a Store
    /// screenshot. Fill the window with believable content; start nothing
    /// real.
    fn show_scene(&mut self, scene: usize) {
        let totals = vec![44_000, 41_000, 47_000, 39_000, 52_000, 43_000, 45_000, 40_000, 46_000, 42_000];
        self.options = false;
        self.open_row = None;
        self.show_services = false;
        self.settings.delayed.clear();
        self.note = None;
        match scene {
            0 => {
                let record = demo_record(Some(23_000));
                self.fake = Some(View {
                    record,
                    totals,
                    metrics: Some(Metrics { cpu: 71.0, mem: 46.0, disk: 38.0 * 1_048_576.0, elapsed_ms: 23_000, eta_ms: Some(21_000) }),
                    done: false,
                    listed: true,
                });
                self.tab = Tab::Items;
            }
            1 => {
                self.fake = Some(View { record: demo_record(None), totals, metrics: None, done: true, listed: true });
                self.tab = Tab::Timeline;
            }
            2 => {
                self.fake = Some(View { record: demo_record(None), totals, metrics: None, done: true, listed: true });
                self.tab = Tab::Cost;
            }
            _ => {
                let record = demo_record(None);
                if let Some(r) = record.items.iter().find(|r| r.item.name == "Spotify") {
                    self.settings.delayed.push(r.item.clone());
                }
                self.open_row = record.items.iter().find(|r| r.item.name == "Teams").map(|r| r.item.id.clone());
                self.fake = Some(View { record, totals, metrics: None, done: true, listed: true });
                self.tab = Tab::Items;
            }
        }
    }

    // ------------------------------------------------------------ actions

    fn act(&mut self, ctx: &egui::Context, action: Action) {
        if self.shots.is_some() {
            return;
        }
        let live_running = self.live.as_ref().is_some_and(|h| !h.shared.lock().unwrap().done);
        match action {
            Action::End(item) => {
                if let (true, Some(h)) = (live_running, &self.live) {
                    h.send(monitor::Request::End(item.id.clone()));
                } else {
                    let (found, ended) = monitor::end_item(&item);
                    let (text, tone) = if ended > 0 {
                        (format!("Ended {}.", item.name), Tone::Success)
                    } else if found > 0 {
                        (format!("Windows would not end {}. It may be running as administrator.", item.name), Tone::Error)
                    } else {
                        (format!("{} is not running.", item.name), Tone::Neutral)
                    };
                    self.say(ctx, text, tone);
                }
            }
            Action::EndWhenSeen(item) => {
                if let Some(h) = &self.live {
                    h.send(monitor::Request::EndWhenSeen(item.id.clone()));
                }
            }
            Action::TurnOff(item) => {
                if crate::startup::set_enabled(&item, false) {
                    self.say(ctx, format!("{} will not start at your next sign-in.", item.name), Tone::Success);
                    self.settings.turned_off.retain(|i| i.id != item.id);
                    self.settings.turned_off.push(item);
                    self.save();
                } else {
                    self.say(ctx, format!("Could not turn off {}. Windows refused the change.", item.name), Tone::Error);
                }
            }
            Action::Delay(item) => {
                if crate::startup::set_enabled(&item, false) {
                    self.say(ctx, format!("From your next sign-in, {} starts once the PC is ready.", item.name), Tone::Success);
                    self.settings.delayed.retain(|i| i.id != item.id);
                    self.settings.delayed.push(item);
                    self.save();
                } else {
                    self.say(ctx, format!("Could not hold back {}. Windows refused the change.", item.name), Tone::Error);
                }
            }
            Action::Undelay(item) => {
                crate::startup::set_enabled(&item, true);
                self.settings.delayed.retain(|i| i.id != item.id);
                self.save();
                self.say(ctx, format!("{} starts with Windows again.", item.name), Tone::Success);
            }
            Action::TurnOn(item) => {
                crate::startup::set_enabled(&item, true);
                self.settings.turned_off.retain(|i| i.id != item.id);
                self.save();
                self.say(ctx, format!("{} starts with Windows again.", item.name), Tone::Success);
            }
        }
    }

    fn set_watch(&mut self, ctx: &egui::Context, on: bool) {
        if self.shots.is_some() {
            self.settings.watch = on;
            return;
        }
        if on {
            if !self.settings.early {
                autostart::set_standard(true);
            }
            self.settings.watch = true;
            self.say(ctx, format!("{} will watch your next sign-in.", crate::brand::NAME), Tone::Success);
        } else {
            if self.settings.early {
                match autostart::disable_early() {
                    Ok(()) => self.settings.early = false,
                    Err(e) => {
                        self.say(ctx, e, Tone::Error);
                        return;
                    }
                }
            }
            autostart::set_standard(false);
            self.settings.watch = false;
            // Held-back items rely on the app to start them.
            let n = self.settings.delayed.len();
            for item in std::mem::take(&mut self.settings.delayed) {
                crate::startup::set_enabled(&item, true);
            }
            let text = if n > 0 {
                format!("Stopped watching. {n} held-back items start with Windows again.")
            } else {
                "Stopped watching. Nothing runs at sign-in now.".to_owned()
            };
            self.say(ctx, text, Tone::Neutral);
        }
        self.save();
    }

    fn set_early(&mut self, ctx: &egui::Context, on: bool) {
        if self.shots.is_some() {
            self.settings.early = on;
            return;
        }
        let result = if on { autostart::enable_early() } else { autostart::disable_early() };
        match result {
            Ok(()) => {
                self.settings.early = on;
                if on {
                    self.settings.watch = true;
                }
                self.save();
                let text = if on {
                    format!("From your next sign-in, {} watches from the first second.", crate::brand::NAME)
                } else {
                    format!("{} starts with your other apps again.", crate::brand::NAME)
                };
                self.say(ctx, text, Tone::Success);
            }
            Err(e) => self.say(ctx, e, Tone::Error),
        }
    }

    /// "Pause the rest", or how a pause in progress stands with a way to end it.
    fn pause_row(&mut self, ui: &mut egui::Ui, view: &View, p: Palette) {
        let now_ms = view.metrics.map(|m| m.elapsed_ms).unwrap_or(0);
        match self.pause.clone() {
            Some(pause) => {
                ui.horizontal(|ui| {
                    let left = history::fmt_secs(pause.until_ms.saturating_sub(now_ms).div_ceil(1000) * 1000);
                    let what = match pause.items.len() {
                        0 => "Pausing startup apps".to_owned(),
                        1 => "1 app paused".to_owned(),
                        n => format!("{n} apps paused"),
                    };
                    ui.label(RichText::new(what).font(theme::strong()).color(p.text));
                    ui.label(RichText::new(format!("{left} left")).color(p.muted));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(theme::secondary_button(p, "Resume now"))
                            .on_hover_text("Let every paused app carry on starting.")
                            .clicked()
                        {
                            self.send_pause(false);
                        }
                    });
                });
            }
            None if !view.done => {
                ui.horizontal(|ui| {
                    if ui
                        .add(theme::secondary_button(p, "Pause the rest"))
                        .on_hover_text(format!(
                            "Freezes your startup apps for {} minutes so you can get on with work.                              Open one and it carries on at once. Windows and security software are never paused.",
                            crate::pause::PAUSE_MS / 60_000
                        ))
                        .clicked()
                    {
                        self.send_pause(true);
                    }
                    ui.label(RichText::new("Enough already? Get to work first.").font(theme::small()).color(p.muted));
                });
            }
            None => {}
        }
    }

    fn send_pause(&mut self, pause: bool) {
        if let Some(h) = &self.live {
            h.send(if pause { monitor::Request::Pause } else { monitor::Request::Resume });
            h.shared.lock().unwrap().events.push(if pause { "pause pressed" } else { "resume pressed" }.to_owned());
        }
    }

    // ------------------------------------------------------------ full window

    fn main_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let p = theme::palette(&ctx);
        self.keys(&ctx);
        if let Some(h) = &self.live {
            if let Some(n) = h.shared.lock().unwrap().note.take() {
                self.note = Some((n, Tone::Neutral, ctx.input(|i| i.time)));
            }
        }
        let view = self.view();
        match &view {
            Some(v) => self.summary_card(ui, v, p),
            None => self.empty_card(ui, p),
        }
        ui.add_space(4.0);

        if self.options {
            self.options_card(ui, p);
            self.note_line(ui, p);
            return;
        }
        if self.ask_early {
            self.early_card(ui, p);
            self.note_line(ui, p);
            return;
        }
        let Some(view) = view else {
            self.configured_card(ui, p);
            self.note_line(ui, p);
            return;
        };

        let labels: Vec<&str> = TABS.iter().map(|t| t.1).collect();
        let selected = TABS.iter().position(|t| t.0 == self.tab).unwrap_or(0);
        if let Some(i) = theme::segmented(ui, egui::Id::new("tabs"), &labels, selected, p) {
            self.tab = TABS[i].0;
        }
        ui.add_space(4.0);
        let note_h = if self.note.is_some() { 36.0 } else { 0.0 };
        let height = (ui.available_height() - note_h - 26.0).max(120.0);
        theme::card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(height)
                .show(ui, |ui| match self.tab {
                    Tab::Items => self.items_tab(ui, &view, p),
                    Tab::Timeline => self.timeline_tab(ui, &view, p),
                    Tab::Cost => self.cost_tab(ui, &view, p),
                });
        });
        self.note_line(ui, p);
    }

    fn keys(&mut self, ctx: &egui::Context) {
        let (escape, tab) = ctx.input(|i| {
            let tab = if i.modifiers.ctrl {
                [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3]
                    .iter()
                    .position(|k| i.key_pressed(*k))
            } else {
                None
            };
            (i.key_pressed(egui::Key::Escape), tab)
        });
        if escape {
            if self.options {
                self.options = false;
            } else {
                self.open_row = None;
            }
        }
        if let Some(t) = tab {
            self.tab = TABS[t].0;
            self.options = false;
        }
    }

    fn note_line(&mut self, ui: &mut egui::Ui, p: Palette) {
        let now = ui.input(|i| i.time);
        if let Some((text, tone, at)) = self.note.clone() {
            if now - at > 6.0 {
                self.note = None;
                return;
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
            ui.add_space(4.0);
            let icon = if tone == Tone::Error { Icon::Info } else { Icon::Check };
            let (fg, _) = theme::tone_colors(p, tone);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(18.0, 20.0), Sense::hover());
                theme::paint_icon(ui.painter(), r.center(), icon, fg);
                ui.label(RichText::new(text).color(if tone == Tone::Neutral { p.text } else { fg }));
            });
        }
    }

    fn header_options(&mut self, ui: &mut egui::Ui, p: Palette) {
        let r = theme::icon_button(ui, Icon::Options, p).on_hover_text("Options");
        if r.clicked() {
            self.options = !self.options;
        }
    }

    fn summary_card(&mut self, ui: &mut egui::Ui, view: &View, p: Palette) {
        let live = !view.done;
        theme::card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let label = if live { "Starting up for" } else { "Usable after" };
                    theme::section_label(ui, label, p);
                    let number = if live {
                        fmt_clock(view.metrics.map(|m| m.elapsed_ms).unwrap_or(0))
                    } else {
                        history::fmt_secs(view.record.usable_ms)
                    };
                    ui.label(RichText::new(number).font(theme::hero()).color(p.text));
                });
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    self.header_options(ui, p);
                    if live {
                        theme::live_pill(ui, "Starting up", Tone::Accent, true, p);
                    } else if self.live.is_some() || self.fake.is_some() {
                        theme::live_pill(ui, "Ready", Tone::Success, false, p);
                    } else {
                        theme::tone_pill(ui, "Last sign-in", Tone::Neutral, p);
                    }
                });
            });

            if live {
                let m = view.metrics.unwrap_or_default();
                ui.add_space(6.0);
                split_progress(ui, view, 92.0, 8.0, p);
                ui.add_space(2.0);
                ui.label(RichText::new(eta_text(m.eta_ms)).color(p.muted));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let gap = 24.0;
                    let w = (ui.available_width() - gap * 2.0) / 3.0;
                    ui.spacing_mut().item_spacing.x = gap;
                    theme::meter(ui, "CPU", &format!("{:.0}%", m.cpu), m.cpu / 100.0, w, p);
                    theme::meter(ui, "Memory", &format!("{:.0}%", m.mem), m.mem / 100.0, w, p);
                    let disk_mb = m.disk / 1_048_576.0;
                    theme::meter(ui, "Disk", &format!("{}/s", history::fmt_bytes(m.disk as u64)), (disk_mb / 100.0) as f32, w, p);
                });
                ui.add_space(8.0);
                self.pause_row(ui, view, p);
            } else {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        match comparison(view.record.usable_ms, &view.totals) {
                            Some((text, tone)) => {
                                let (fg, _) = theme::tone_colors(p, tone);
                                ui.label(RichText::new(text).font(theme::strong()).color(if tone == Tone::Neutral { p.text } else { fg }));
                            }
                            None => {
                                ui.label(RichText::new("First watched sign-in").font(theme::strong()).color(p.text));
                            }
                        }
                        ui.label(RichText::new(when_line(&view.record)).font(theme::small()).color(p.muted));
                        if let Some(at) = view.record.paused_ms {
                            ui.label(
                                RichText::new(format!(
                                    "You paused startup at {}, so this sign-in is left out of your usual time.",
                                    fmt_clock(at)
                                ))
                                .font(theme::small())
                                .color(p.muted),
                            );
                        }
                    });
                    if !view.totals.is_empty() {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let mut values = view.totals.clone();
                            values.push(view.record.usable_ms);
                            theme::boot_bars(ui, &values, history::median(&view.totals), vec2(150.0, 40.0), p)
                                .on_hover_text("Your last sign-ins. The dashed line is your usual time.");
                        });
                    }
                });
                if self.pause.is_some() {
                    ui.add_space(6.0);
                    self.pause_row(ui, view, p);
                }
            }
        });
    }

    fn empty_card(&mut self, ui: &mut egui::Ui, p: Palette) {
        let ctx = ui.ctx().clone();
        theme::card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(132.0, 112.0), Sense::hover());
                theme::paint_startup_motif(ui.painter(), r.center(), p);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("No sign-ins timed yet").font(theme::title()).color(p.text));
                        ui.with_layout(Layout::right_to_left(Align::Min), |ui| self.header_options(ui, p));
                    });
                    ui.label(
                        RichText::new(
                            "Next time you sign in, a small window shows what has started, what is still to come \
                             and how long until the PC is usable. It closes itself when the PC is ready.",
                        )
                        .color(p.muted),
                    );
                    ui.add_space(6.0);
                    if self.settings.watch {
                        theme::tone_pill(ui, "Watching from your next sign-in", Tone::Success, p);
                    } else if ui.add(theme::primary_button(p, "Watch my next sign-in", true, 220.0)).clicked() {
                        self.set_watch(&ctx, true);
                    }
                });
            });
        });
    }

    fn configured_card(&mut self, ui: &mut egui::Ui, p: Palette) {
        let items = self.configured.lock().unwrap().clone();
        let height = (ui.available_height() - 40.0).max(120.0);
        theme::card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            match items {
                None => {
                    theme::section_label(ui, "Set to start at sign-in", p);
                    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
                    theme::progress_track(ui.painter(), bar, None, ui.input(|i| i.time), p);
                    ui.ctx().request_repaint();
                }
                Some(items) => {
                    theme::section_label(ui, &format!("Set to start at sign-in · {}", items.len()), p);
                    egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(height).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for item in items {
                            let rec = ItemRecord::new(item);
                            self.item_row(ui, &rec, None, p);
                        }
                        self.held_sections(ui, p);
                    });
                }
            }
        });
    }

    /// The first-launch question: watch from the first second of sign-in?
    fn early_card(&mut self, ui: &mut egui::Ui, p: Palette) {
        let ctx = ui.ctx().clone();
        theme::card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section_label(ui, "Before your next sign-in", p);
            ui.add_space(2.0);
            ui.label(RichText::new("Watch from the first second?").font(theme::title()).color(p.text));
            ui.add_space(4.0);
            for text in [
                format!("{} is set to open with your other startup apps, so it cannot watch anything that starts before it does. It works out those timings afterwards.", crate::brand::NAME),
                "Start earlier opens it the moment you sign in. You see everything as it happens, and you can stop an app before it opens.".to_owned(),
                if autostart::early_needs_admin() {
                    "Windows will ask for administrator approval once. You can change this at any time in Options.".to_owned()
                } else {
                    "No administrator approval is needed. You can change this at any time in Options.".to_owned()
                },
            ] {
                ui.add(egui::Label::new(RichText::new(text).font(theme::body()).color(p.muted)).wrap());
                ui.add_space(4.0);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add(theme::primary_button(p, "Start earlier", true, 150.0)).clicked() {
                    self.set_early(&ctx, true);
                    // Approval refused: leave the question up with the reason.
                    if self.settings.early {
                        self.answer_early();
                    }
                }
                if ui.add(theme::secondary_button(p, "Not now")).clicked() {
                    self.answer_early();
                }
            });
        });
    }

    fn answer_early(&mut self) {
        self.ask_early = false;
        self.settings.asked_early = true;
        self.save();
    }

    fn options_card(&mut self, ui: &mut egui::Ui, p: Palette) {
        let ctx = ui.ctx().clone();
        theme::card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section_label(ui, "Options", p);
            ui.add_space(2.0);
            let mut watch = self.settings.watch;
            if theme::checkbox(ui, &mut watch, "Watch every sign-in").changed() {
                self.set_watch(&ctx, watch);
            }
            let mut early = self.settings.early;
            if theme::checkbox(ui, &mut early, "Start earlier").changed() {
                self.set_early(&ctx, early);
            }
            // Indented under the checkbox label. A frame, not a horizontal
            // row: a row never wraps, and the text ran out of the card.
            egui::Frame::new().inner_margin(egui::Margin { left: 26, right: 0, top: 0, bottom: 0 }).show(ui, |ui| {
                ui.add(egui::Label::new(
                    RichText::new(if autostart::early_needs_admin() {
                        "Watches from the first second of sign-in and can stop apps before they open. \
                         Asks for administrator approval once, and again to turn it off."
                    } else {
                        "Watches from the first second of sign-in and can stop apps before they open."
                    })
                    .font(theme::small())
                    .color(p.muted),
                ).wrap());
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(theme::secondary_button(p, "Done")).clicked() {
                    self.options = false;
                }
                if ui
                    .add(theme::secondary_button(p, "Copy diagnostics"))
                    .on_hover_text("Copies a text report for troubleshooting: settings, timings and your startup items. Nothing is sent anywhere.")
                    .clicked()
                {
                    let live = self.live.as_ref().map(|h| h.shared.lock().unwrap().record.clone());
                    let text = crate::diag::report(&self.version, &self.settings, live.as_ref());
                    let lines = text.lines().count();
                    ctx.copy_text(text);
                    self.say(&ctx, format!("Diagnostics copied ({lines} lines). Paste them where you need them."), Tone::Success);
                }
            });
        });
    }

    // ------------------------------------------------------------ tabs

    fn items_tab(&mut self, ui: &mut egui::Ui, view: &View, p: Palette) {
        if !view.listed {
            theme::section_label(ui, "Reading the startup list", p);
            return;
        }
        let mut apps: Vec<&ItemRecord> = view
            .record
            .items
            .iter()
            .filter(|r| r.item.kind() != Kind::Service && r.status != Status::Delayed)
            .collect();
        apps.sort_by_key(|r| (r.start_ms.is_none(), r.start_ms, r.item.name.to_lowercase()));
        theme::section_label(ui, &format!("Apps and tasks · {}", apps.len()), p);
        ui.spacing_mut().item_spacing.y = 2.0;
        let mode = Some((view.record.mode, view.record.watch_from_ms, !view.done));
        for r in apps {
            self.item_row(ui, r, mode, p);
        }

        let services: Vec<&ItemRecord> = view.record.items.iter().filter(|r| r.item.kind() == Kind::Service).collect();
        if !services.is_empty() {
            ui.add_space(8.0);
            let before = services.iter().filter(|r| r.before_sign_in).count();
            let label = format!("Windows services · {}, {} running before you signed in", services.len(), before);
            if disclosure(ui, &label, self.show_services, p).clicked() {
                self.show_services = !self.show_services;
            }
            if self.show_services {
                let mut services = services;
                services.sort_by_key(|r| (r.before_sign_in, r.start_ms.is_none(), r.start_ms));
                for r in services {
                    self.item_row(ui, r, mode, p);
                }
            }
        }
        self.held_sections(ui, p);
    }

    /// Items the app holds back or has turned off, with the way back.
    fn held_sections(&mut self, ui: &mut egui::Ui, p: Palette) {
        let ctx = ui.ctx().clone();
        for (title, list, held) in [
            ("Held back until the PC is ready".to_owned(), self.settings.delayed.clone(), true),
            (format!("Turned off by {}", crate::brand::NAME), self.settings.turned_off.clone(), false),
        ] {
            if list.is_empty() {
                continue;
            }
            ui.add_space(10.0);
            theme::section_label(ui, &format!("{title} · {}", list.len()), p);
            for item in list {
                let w = ui.available_width();
                let (rect, _) = ui.allocate_exact_size(vec2(w, theme::ROW_HEIGHT), Sense::hover());
                let tone = if held { Tone::Accent } else { Tone::Neutral };
                theme::icon_tile(ui.painter(), pos2(rect.left() + 19.0, rect.center().y), Icon::App, tone, p);
                let text_w = w - 48.0 - 170.0;
                let name = theme::truncated(ui.painter(), &item.name, theme::strong(), p.text, text_w);
                ui.painter().galley(pos2(rect.left() + 46.0, rect.top() + 7.0), name, p.text);
                let sub = if held { "Starts once the PC is usable" } else { "Does not start at sign-in" };
                let sub = theme::truncated(ui.painter(), sub, theme::small(), p.muted, text_w);
                ui.painter().galley(pos2(rect.left() + 46.0, rect.top() + 27.0), sub, p.muted);
                let button = Rect::from_min_size(pos2(rect.right() - 168.0, rect.center().y - 16.0), vec2(164.0, 32.0));
                let label = if held { "Start at sign-in" } else { "Turn back on" };
                if ui.put(button, theme::secondary_button(p, label)).clicked() {
                    let a = if held { Action::Undelay(item.clone()) } else { Action::TurnOn(item.clone()) };
                    self.act(&ctx, a);
                }
            }
        }
    }

    /// One row of the startup list. `mode` is (mode, watch start, live) for
    /// a watched boot, `None` for the plain configured list.
    fn item_row(&mut self, ui: &mut egui::Ui, r: &ItemRecord, mode: Option<(Mode, u64, bool)>, p: Palette) {
        let ctx = ui.ctx().clone();
        let w = ui.available_width();
        // Painted last, behind the row and its open actions together.
        let background = ui.painter().add(egui::Shape::Noop);
        let (rect, response) = ui.allocate_exact_size(vec2(w, theme::ROW_HEIGHT), Sense::click());
        let open = self.open_row.as_deref() == Some(r.item.id.as_str());
        if response.hovered() && !open {
            ui.painter().set(
                background,
                egui::Shape::rect_filled(rect.expand2(vec2(6.0, 0.0)), theme::CONTROL_RADIUS, p.hover),
            );
        }
        let paused = self.pause.as_ref().is_some_and(|x| x.items.contains(&r.item.id));
        let (label, tone) = if paused { ("Paused", Tone::Accent) } else { status_label(r) };
        let icon = match r.item.kind() {
            Kind::App => Icon::App,
            Kind::Task => Icon::Clock,
            Kind::Service => Icon::Service,
        };
        let tile_tone = match (mode, r.status) {
            (None, _) => Tone::Neutral,
            (_, Status::Starting) => Tone::Accent,
            (_, Status::Started) => Tone::Success,
            (_, Status::NotSeen) => Tone::Warn,
            _ => Tone::Neutral,
        };
        theme::icon_tile(ui.painter(), pos2(rect.left() + 19.0, rect.center().y), icon, tile_tone, p);

        // Status pill, right-aligned.
        let pill_w = if mode.is_some() {
            theme::paint_tone_pill(ui.painter(), rect.right_center(), label, tone, p)
        } else {
            0.0
        };
        let text_w = (w - 46.0 - pill_w - 16.0).max(40.0);
        let own = r.item.is_self();
        // Services sit under their own heading: only tasks need the tag.
        let system = !own && r.item.kind() != Kind::Service && r.item.runs_as_system();
        let tag = if own { "This app" } else { "System" };
        let tag_w = if own || system { 70.0 } else { 0.0 };
        let name = theme::truncated(ui.painter(), &r.item.name, theme::strong(), p.text, text_w - tag_w);
        let name_w = name.size().x;
        ui.painter().galley(pos2(rect.left() + 46.0, rect.top() + 6.0), name, p.text);
        if own || system {
            // "This app", so it is not mistaken for someone else's startup
            // item; "System" for what runs as Windows or administrator.
            ui.painter().text(
                pos2(rect.left() + 46.0 + name_w + 8.0, rect.top() + 8.0),
                egui::Align2::LEFT_TOP,
                tag,
                theme::small(),
                if own { p.accent_text } else { p.muted },
            );
        }
        let sub = match mode {
            Some((m, from, live)) => describe(r, m, from, live),
            None => r.item.origin().to_owned(),
        };
        let sub = theme::truncated(ui.painter(), &sub, theme::small(), p.muted, text_w);
        ui.painter().galley(pos2(rect.left() + 46.0, rect.top() + 27.0), sub, p.muted);
        if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.open_row = if open { None } else { Some(r.item.id.clone()) };
        }
        if !open {
            return;
        }

        // Actions for the open row.
        let live = mode.is_some_and(|m| m.2);
        let early = mode.is_some_and(|m| m.0 == Mode::Early);
        let actions = egui::Frame::new()
            .inner_margin(egui::Margin { left: 46, right: 0, top: 0, bottom: 10 })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let detail = if r.item.kind() == Kind::Service {
                    if r.item.about.is_empty() { r.item.origin().to_owned() } else { r.item.about.clone() }
                } else {
                    r.item.command.clone()
                };
                if !detail.is_empty() {
                    ui.label(RichText::new(detail).font(theme::small()).color(p.text));
                }
                if r.item.kind() == Kind::Service {
                    ui.label(RichText::new("Services are shown read-only.").font(theme::small()).color(p.muted));
                    return;
                }
                if own {
                    // Ending, holding back or turning off its own entry
                    // would break it; the option does the job properly.
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "This is {} itself. To stop it opening at sign-in, turn off \
                                 \"Watch every sign-in\" in Options.",
                                crate::brand::NAME
                            ))
                            .font(theme::small())
                            .color(p.muted),
                        )
                        .wrap(),
                    );
                    ui.add_space(2.0);
                    if ui.add(theme::secondary_button(p, "Open Options")).clicked() {
                        self.options = true;
                    }
                    return;
                }
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    let running = matches!(r.status, Status::Starting | Status::Started) || !live;
                    if r.item.exe.is_some() && running && ui.add(theme::secondary_button(p, "End it now")).clicked() {
                        self.act(&ctx, Action::End(r.item.clone()));
                    }
                    if live && early && r.status == Status::Waiting && r.item.exe.is_some()
                        && ui.add(theme::secondary_button(p, "End it when it starts")).clicked()
                    {
                        self.act(&ctx, Action::EndWhenSeen(r.item.clone()));
                    }
                    if r.item.can_disable() {
                        if ui.add(theme::secondary_button(p, "Start it once the PC is ready")).clicked() {
                            self.act(&ctx, Action::Delay(r.item.clone()));
                        }
                        if ui.add(theme::secondary_button(p, "Turn off at sign-in")).clicked() {
                            self.act(&ctx, Action::TurnOff(r.item.clone()));
                        }
                    }
                });
                if !r.item.can_disable() {
                    let why = match r.item.source {
                        Source::Task { .. } => "Scheduled tasks can only be ended here.",
                        _ => "This entry is set for all users, so it can only be ended here.",
                    };
                    ui.label(RichText::new(why).font(theme::small()).color(p.muted));
                }
            });
        let whole = rect.union(actions.response.rect).expand2(vec2(6.0, 0.0));
        ui.painter().set(background, egui::Shape::rect_filled(whole, theme::CONTROL_RADIUS, p.hover));
        ui.add_space(4.0);
    }

    fn timeline_tab(&mut self, ui: &mut egui::Ui, view: &View, p: Palette) {
        let rec = &view.record;
        if rec.mode == Mode::Standard && rec.watch_from_ms > 2_000 {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(18.0, 20.0), Sense::hover());
                theme::paint_icon(ui.painter(), r.center(), Icon::Info, p.muted);
                ui.label(
                    RichText::new(format!(
                        "{} opened {} after sign-in. Programs that ran and closed before then are missing.",
                        crate::brand::NAME,
                        history::fmt_secs(rec.watch_from_ms)
                    ))
                    .font(theme::small())
                    .color(p.muted),
                );
            });
            ui.horizontal(|ui| {
                ui.add_space(26.0);
                if theme::text_link(ui, "Start earlier to see them", p).clicked() {
                    self.options = true;
                }
            });
            ui.add_space(8.0);
        }
        let now = view.metrics.filter(|_| !view.done).map(|m| m.elapsed_ms);
        let mut spans: Vec<theme::Span> = rec
            .items
            .iter()
            .filter(|r| r.item.kind() != Kind::Service || (r.start_ms.is_some() && !r.before_sign_in))
            .filter_map(|r| {
                let start = r.start_ms?;
                let end = match r.status {
                    Status::Starting => now.unwrap_or(start),
                    _ => r.ready_ms.unwrap_or(start),
                };
                let tone = match r.status {
                    Status::Starting => Tone::Warn,
                    Status::Ended => Tone::Neutral,
                    _ => Tone::Accent,
                };
                Some(theme::Span { label: r.item.name.clone(), start_ms: start, end_ms: end.max(start), tone, estimated: r.estimated })
            })
            .collect();
        spans.sort_by_key(|s| s.start_ms);
        if spans.is_empty() {
            theme::section_label(ui, "Nothing has started yet", p);
            return;
        }
        let usable = view.done.then_some(rec.usable_ms);
        theme::timeline(ui, &spans, usable, now, p);
        ui.add_space(8.0);
        let legend = if view.done {
            "Bars run from start to settled. Paler bars were worked out afterwards. The dashed line is when the PC became usable."
        } else {
            "Bars run from start to settled. Orange bars are still busy."
        };
        ui.label(RichText::new(legend).font(theme::small()).color(p.muted));
    }

    fn cost_tab(&mut self, ui: &mut egui::Ui, view: &View, p: Palette) {
        let rec = &view.record;
        let top = history::costliest(rec, 4);
        if top.is_empty() {
            theme::section_label(ui, "Nothing measured yet", p);
            return;
        }
        ui.spacing_mut().item_spacing.y = 4.0;
        theme::section_label(ui, "Most processor time", p);
        let max = top.first().map(|r| r.cpu_ms).unwrap_or(1).max(1);
        for r in &top {
            cost_row(
                ui,
                &format!("{} cost you {}", r.item.name, history::fmt_cpu(r.cpu_ms)),
                &format!("{} read and written", history::fmt_bytes(r.io_bytes)),
                r.cpu_ms as f32 / max as f32,
                Tone::Accent,
                p,
            );
        }

        let mut slow: Vec<(&ItemRecord, u64)> = rec
            .items
            .iter()
            // The app is busy for as long as it watches: not a slow starter.
            .filter(|r| r.item.kind() != Kind::Service && !r.item.is_self())
            .filter_map(|r| Some((r, r.ready_ms?.saturating_sub(r.start_ms?))))
            .filter(|(_, d)| *d > 0)
            .collect();
        slow.sort_by(|a, b| b.1.cmp(&a.1));
        slow.truncate(3);
        if !slow.is_empty() {
            ui.add_space(4.0);
            theme::section_label(ui, "Slowest to settle", p);
            let max = slow[0].1.max(1);
            for (r, d) in slow {
                let est = if r.estimated { ", worked out afterwards" } else { "" };
                cost_row(
                    ui,
                    &format!("{} took {}", r.item.name, history::fmt_secs(d)),
                    &format!("from {} to {} after sign-in{est}", history::fmt_secs(r.start_ms.unwrap_or(0)), history::fmt_secs(r.ready_ms.unwrap_or(0))),
                    d as f32 / max as f32,
                    Tone::Warn,
                    p,
                );
            }
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui.ctx());
        ui.horizontal(|ui| {
            let mut auto_update = self.settings.auto_update;
            let label = egui::RichText::new("Automatically update").font(theme::small()).color(p.muted);
            if theme::checkbox(ui, &mut auto_update, label)
                .on_hover_text("Checks for new versions about once a day and installs them in the background.")
                .changed()
            {
                self.settings.auto_update = auto_update;
                self.save();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("v{}", self.version))
                        .font(theme::small())
                        .color(p.muted),
                );
            });
        });
    }
}

/// A line of the cost list: what it cost on the left, a bar on the right,
/// the detail under it.
fn cost_row(ui: &mut egui::Ui, title: &str, sub: &str, fraction: f32, tone: Tone, p: Palette) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 42.0), Sense::hover());
    let bar_w = (w * 0.32).min(180.0);
    let text_w = w - bar_w - 16.0;
    let painter = ui.painter();
    let t = theme::truncated(painter, title, theme::strong(), p.text, text_w);
    painter.galley(pos2(rect.left(), rect.top() + 2.0), t, p.text);
    let s = theme::truncated(painter, sub, theme::small(), p.muted, text_w);
    painter.galley(pos2(rect.left(), rect.top() + 22.0), s, p.muted);
    let bar = Rect::from_min_size(pos2(rect.right() - bar_w, rect.top() + 10.0), vec2(bar_w, 6.0));
    theme::paint_share_bar(painter, bar, fraction, tone, p);
}

/// A clickable section heading with a chevron.
fn disclosure(ui: &mut egui::Ui, text: &str, open: bool, p: Palette) -> egui::Response {
    let w = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(w, 26.0), Sense::click());
    let icon = if open { Icon::ChevronDown } else { Icon::ChevronRight };
    let color = if response.hovered() { p.text } else { p.muted };
    theme::paint_icon(ui.painter(), pos2(rect.left() + 6.0, rect.center().y), icon, color);
    let g = theme::truncated(ui.painter(), text, egui::FontId::new(theme::SMALL_SIZE, theme::semibold()), color, w - 24.0);
    ui.painter().galley(pos2(rect.left() + 18.0, rect.center().y - g.size().y / 2.0), g, color);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One progress row each for what runs as the user and what runs as
/// Windows or administrator: a label `label_w` wide, a bar, and the count.
fn split_progress(ui: &mut egui::Ui, view: &View, label_w: f32, bar_h: f32, p: Palette) {
    if !view.listed {
        ui.label(RichText::new("Reading the startup list").font(theme::strong()).color(p.text));
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), bar_h), Sense::hover());
        theme::progress_track(ui.painter(), bar, None, ui.input(|i| i.time), p);
        return;
    }
    let (yours, system) = view.record.progress_split();
    let rows = [
        ("Your apps", yours, "Startup apps and tasks that run as you: the ones you can pause or turn off."),
        ("System", system, "Services, and tasks that run as Windows or with administrator rights."),
    ];
    for (label, (done, total), about) in rows {
        let count = format!("{done} of {total}");
        ui.horizontal(|ui| {
            let (r, resp) = ui.allocate_exact_size(vec2(label_w, 20.0), Sense::hover());
            ui.painter().text(r.left_center(), egui::Align2::LEFT_CENTER, label, theme::strong(), p.text);
            resp.on_hover_text(about);
            let count_w = 64.0;
            let bar_w = (ui.available_width() - count_w).max(40.0);
            let (row, _) = ui.allocate_exact_size(vec2(bar_w, 20.0), Sense::hover());
            let bar = egui::Rect::from_center_size(row.center(), vec2(bar_w, bar_h));
            // Nothing of a kind to wait for: an empty track, not a full bar.
            let fraction = if total > 0 { Some(done as f32 / total as f32) } else { Some(0.0) };
            theme::progress_track(ui.painter(), bar, fraction, ui.input(|i| i.time), p);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(count).color(p.muted));
            });
        });
    }
}

/// Pill text and tone for a status.
fn status_label(r: &ItemRecord) -> (&'static str, Tone) {
    match r.status {
        Status::Waiting => ("Waiting", Tone::Neutral),
        Status::Starting => ("Starting", Tone::Accent),
        Status::Started if r.before_sign_in => ("Running", Tone::Neutral),
        Status::Started => ("Started", Tone::Success),
        Status::Exited if r.item.kind() == Kind::Service => ("Not running", Tone::Neutral),
        Status::Exited => ("Ran and closed", Tone::Neutral),
        Status::NotSeen => ("Not seen", Tone::Warn),
        Status::Ended => ("Ended", Tone::Error),
        Status::Delayed => ("Held back", Tone::Accent),
    }
}

/// The detail line under an item's name.
fn describe(r: &ItemRecord, mode: Mode, watch_from_ms: u64, live: bool) -> String {
    let at = |ms: Option<u64>| history::fmt_secs(ms.unwrap_or(0));
    let cost = if r.cpu_ms > 0 { format!(" · {} CPU", history::fmt_cpu(r.cpu_ms)) } else { String::new() };
    match r.status {
        Status::Waiting if r.item.kind() == Kind::Service => "Delayed start, not running yet".to_owned(),
        Status::Waiting => "Waiting · configured, no process seen yet".to_owned(),
        Status::Starting => format!("Started at {} · still busy{cost}", at(r.start_ms)),
        Status::Started if r.before_sign_in => "Running before you signed in".to_owned(),
        Status::Started => match (r.ready_ms, r.estimated) {
            (Some(ready), false) if ready > r.start_ms.unwrap_or(0) => {
                format!("Started at {} · settled at {}{cost}", at(r.start_ms), history::fmt_secs(ready))
            }
            _ => format!("Started at {}{cost}", at(r.start_ms)),
        },
        Status::Exited if r.item.kind() == Kind::Service => "Set to start automatically, not running".to_owned(),
        Status::Exited => format!("Ran at {} and closed{cost}", at(r.start_ms)),
        Status::NotSeen if mode == Mode::Standard && watch_from_ms > 2_000 => {
            format!("Not seen. It may have run and closed before {} opened", crate::brand::NAME)
        }
        Status::NotSeen if live => "Not seen yet, no longer waited for".to_owned(),
        Status::NotSeen => "Did not start before the PC was ready".to_owned(),
        Status::Ended => format!("Ended from {}", crate::brand::NAME),
        Status::Delayed => "Held back until the PC is ready".to_owned(),
    }
}

/// "6 s slower than your usual 42 s", with a tone.
pub(crate) fn comparison(current_ms: u64, totals: &[u64]) -> Option<(String, Tone)> {
    let diff = history::versus_usual(current_ms, totals)?;
    let usual = history::fmt_secs(history::median(totals)?);
    let d = history::fmt_secs(diff.unsigned_abs());
    Some(if diff.unsigned_abs() < 1_500 {
        (format!("About your usual {usual}"), Tone::Neutral)
    } else if diff > 0 {
        (format!("{d} slower than your usual {usual}"), Tone::Warn)
    } else {
        (format!("{d} faster than your usual {usual}"), Tone::Success)
    })
}

pub(crate) fn eta_text(eta: Option<u64>) -> String {
    match eta {
        None => "No estimate on the first watched sign-in".to_owned(),
        Some(ms) if ms < 3_000 => "Almost ready".to_owned(),
        Some(ms) => format!("About {} left", history::fmt_secs(ms)),
    }
}

/// "0:32", "1:05".
pub(crate) fn fmt_clock(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

fn fmt_when(parts: (u16, u16, u16, u16, u16, u16)) -> String {
    let (_, month, day, dow, hour, minute) = parts;
    format!(
        "{} {} {} at {:02}:{:02}",
        DAYS[(dow as usize) % 7],
        day,
        MONTHS[(month.clamp(1, 12) - 1) as usize],
        hour,
        minute
    )
}

fn when_line(r: &BootRecord) -> String {
    let n = r.items.iter().filter(|i| i.item.kind() != Kind::Service && i.status != Status::Delayed).count();
    let when = win::local_time(r.sign_in).map(|t| format!("Signed in {} · ", fmt_when(t))).unwrap_or_default();
    format!("{when}{n} apps and tasks")
}

/// A made-up but believable boot for the Store screenshots. With `at`, the
/// boot is still going at that many ms after sign-in.
fn demo_record(at: Option<u64>) -> BootRecord {
    // name, kind, start s, busy for s, cpu ms, io MB
    let rows: [(&str, u8, f32, f32, u64, u64); 14] = [
        ("Security notification icon", 0, 1.4, 0.8, 310, 4),
        (crate::brand::NAME, 0, 2.1, 0.6, 180, 3),
        ("OneDrive", 0, 3.0, 9.5, 4_200, 96),
        ("Realtek Audio Console", 0, 3.6, 2.2, 640, 18),
        ("Teams", 0, 5.2, 24.8, 11_240, 310),
        ("Logitech G HUB", 0, 6.0, 11.0, 5_300, 140),
        ("Dropbox", 0, 8.4, 14.0, 3_900, 205),
        ("Spotify", 0, 10.5, 9.0, 2_700, 88),
        ("Steam", 0, 12.8, 17.5, 6_100, 260),
        ("Adobe Acrobat Update", 1, 14.0, 3.0, 900, 22),
        ("Discord", 0, 19.0, 12.0, 4_800, 150),
        ("Docker Desktop", 0, 24.0, 15.0, 7_400, 330),
        ("Greenshot", 0, 31.0, 2.0, 520, 9),
        ("NVIDIA Broadcast", 1, 33.5, 4.0, 1_300, 40),
    ];
    let mut items = Vec::new();
    for (name, kind, start, busy, cpu, io) in rows {
        let id = format!("demo:{}", name.to_lowercase());
        let source = if kind == 1 {
            Source::Task { path: format!("\\{name}") }
        } else {
            Source::Run { machine: false, wow32: false, once: false, value: name.into() }
        };
        let exe = format!(r"c:\program files\{0}\{0}.exe", name.to_lowercase().replace(' ', ""));
        let item = Item {
            id,
            name: name.into(),
            source,
            command: format!("\"C:\\Program Files\\{0}\\{0}.exe\" --background", name.replace(' ', "")),
            exe: Some(exe),
            about: String::new(),
            as_system: kind == 1,
        };
        let start_ms = (start * 1000.0) as u64;
        let ready_ms = start_ms + (busy * 1000.0) as u64;
        let mut r = ItemRecord::new(item);
        match at {
            Some(now) if start_ms > now => {}
            Some(now) if ready_ms > now => {
                r.status = Status::Starting;
                r.start_ms = Some(start_ms);
                r.cpu_ms = cpu * (now - start_ms) / (ready_ms - start_ms).max(1);
                r.io_bytes = io * 1_048_576 * (now - start_ms) / (ready_ms - start_ms).max(1);
            }
            _ => {
                r.status = Status::Started;
                r.start_ms = Some(start_ms);
                r.ready_ms = Some(ready_ms);
                r.cpu_ms = cpu;
                r.io_bytes = io * 1_048_576;
            }
        }
        items.push(r);
    }
    for (i, name) in ["Windows Audio", "Print Spooler", "Windows Search", "Cryptographic Services", "DHCP Client", "Windows Update"]
        .iter()
        .enumerate()
    {
        let mut r = ItemRecord::new(Item {
            id: format!("svc:demo{i}"),
            name: (*name).into(),
            source: Source::Service { name: format!("demo{i}"), delayed: i == 5 },
            command: String::new(),
            exe: None,
            about: String::new(),
            as_system: false,
        });
        r.status = Status::Started;
        r.before_sign_in = i < 4;
        if !r.before_sign_in {
            r.start_ms = Some(4_000 + i as u64 * 2_000);
            r.ready_ms = r.start_ms;
        }
        items.push(r);
    }
    BootRecord {
        // A Tuesday morning.
        sign_in: 1_795_507_920,
        mode: Mode::Standard,
        usable_ms: 49_000,
        watch_from_ms: 1_900,
        watched_ms: 49_000,
        items,
        paused_ms: None,
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui_root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui_root.ctx().clone();
        if let Some(scene) = self.shots.as_mut().and_then(|shots| shots.drive(&ctx, SHOT_SCENES)) {
            self.show_scene(scene);
        }
        if let Some(h) = &self.live {
            let mut s = h.shared.lock().unwrap();
            s.frames += 1;
            self.pause = s.pause.clone();
        }
        self.load_wide_fonts_when_free(&ctx);
        let p = theme::palette(&ctx);
        theme::window_mark(ui_root, &p);
        egui::Panel::bottom("footer")
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(16, 8)))
            .show(ui_root, |ui| self.footer(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::same(16)))
            .show(ui_root, |ui| self.main_ui(ui));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_and_dates_read_naturally() {
        assert_eq!(fmt_clock(32_400), "0:32");
        assert_eq!(fmt_clock(65_000), "1:05");
        assert_eq!(fmt_when((2026, 11, 24, 2, 8, 5)), "Tue 24 Nov at 08:05");
    }

    #[test]
    fn eta_never_guesses_on_the_first_boot() {
        assert_eq!(eta_text(None), "No estimate on the first watched sign-in");
        assert_eq!(eta_text(Some(1_000)), "Almost ready");
        assert_eq!(eta_text(Some(21_000)), "About 21 s left");
    }

    #[test]
    fn comparison_says_faster_or_slower() {
        let t = [40_000, 42_000, 44_000];
        assert_eq!(comparison(50_000, &t), Some(("8 s slower than your usual 42 s".to_owned(), Tone::Warn)));
        assert_eq!(comparison(30_000, &t), Some(("12 s faster than your usual 42 s".to_owned(), Tone::Success)));
        assert_eq!(comparison(42_500, &t).map(|c| c.1), Some(Tone::Neutral));
        assert_eq!(comparison(42_500, &[]), None);
    }

    #[test]
    fn standard_mode_explains_what_it_could_not_see() {
        let mut r = demo_record(None).items.remove(0);
        r.status = Status::NotSeen;
        assert!(describe(&r, Mode::Standard, 14_000, false).contains(&format!("before {} opened", crate::brand::NAME)));
        assert_eq!(describe(&r, Mode::Early, 0, false), "Did not start before the PC was ready");
        r.status = Status::Waiting;
        assert!(describe(&r, Mode::Standard, 0, true).contains("no process seen yet"));
    }

    #[test]
    fn demo_boot_is_part_way_when_asked() {
        let live = demo_record(Some(23_000));
        let (done, total) = live.progress();
        assert!(done > 0 && done < total, "{done} of {total}");
        assert!(live.items.iter().any(|r| r.status == Status::Waiting));
        assert!(live.items.iter().any(|r| r.status == Status::Starting));
    }
}
