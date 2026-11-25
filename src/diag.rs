//! Diagnostics for developing the app: a timed log of what the collector
//! saw and decided during a sign-in, and a plain-text report the user can
//! copy from Options. Nothing here is sent anywhere; the report only leaves
//! the PC if the user pastes it somewhere.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::history::{self, BootRecord, ItemRecord, Status};
use crate::settings::Settings;
use crate::startup::Item;
use crate::win;

/// Lines kept from one watch. A sign-in logs a few hundred.
const MAX_LINES: usize = 3_000;
/// A line of readings every this many ticks, besides every change.
const SAMPLE_EVERY: u64 = 10;

fn log_path() -> std::path::PathBuf {
    crate::settings::data_dir().join("last-watch.log")
}

/// The collector's log for one sign-in, written to `last-watch.log`.
#[derive(Default)]
pub struct Log {
    lines: Vec<String>,
    /// What each item looked like at the last tick: status and start time.
    seen: HashMap<String, (Status, Option<u64>)>,
    quiet: Option<bool>,
    ticks: u64,
    worst_tick_ms: u64,
}

/// One tick's readings, for `Log::observe`.
pub struct Tick {
    pub now_ms: u64,
    pub cpu: f32,
    pub disk: f64,
    pub mem: f32,
    /// Since when the PC has been quiet, ms after sign-in.
    pub quiet_since: Option<u64>,
    pub usable: bool,
    /// How long this tick's own work took.
    pub work_ms: u64,
    /// The app's own processor time so far, ms.
    pub own_cpu_ms: u64,
    pub processes: usize,
    /// Frames the window has drawn so far.
    pub frames: u64,
}

impl Log {
    pub fn add(&mut self, now_ms: u64, text: impl AsRef<str>) {
        if self.lines.len() < MAX_LINES {
            self.lines.push(format!("{:>8.1}s  {}", now_ms as f64 / 1000.0, text.as_ref()));
        }
    }

    /// Logs every item whose status or start time changed since the last
    /// tick, quiet/busy flips, and a line of readings now and then.
    pub fn observe<'a>(&mut self, t: &Tick, records: impl Iterator<Item = &'a ItemRecord>) {
        let (mut done, mut total) = (0, 0);
        let mut changes = Vec::new();
        for r in records {
            if r.counts() {
                total += 1;
                done += r.status.is_done() as usize;
            }
            let now = (r.status, r.start_ms);
            let before = self.seen.insert(r.item.id.clone(), now);
            // Services that were up before sign-in are not news.
            if before.is_none() && r.before_sign_in {
                continue;
            }
            if before != Some(now) {
                let from = before.map(|b| format!("{:?}", b.0)).unwrap_or_else(|| "-".into());
                changes.push(format!(
                    "item  {from} -> {:?}  {}  start={} ready={} cpu={}ms io={} est={} counts={}",
                    r.status,
                    r.item.name,
                    opt(r.start_ms),
                    opt(r.ready_ms),
                    r.cpu_ms,
                    r.io_bytes,
                    r.estimated,
                    r.counts(),
                ));
            }
        }
        for c in changes {
            self.add(t.now_ms, c);
        }
        let quiet = t.quiet_since.is_some();
        if self.quiet != Some(quiet) {
            self.quiet = Some(quiet);
            self.add(t.now_ms, format!("pc    {} (cpu {:.0}% disk {:.1} MB/s)", if quiet { "quiet" } else { "busy" }, t.cpu, mb(t.disk)));
        }
        self.worst_tick_ms = self.worst_tick_ms.max(t.work_ms);
        if self.ticks % SAMPLE_EVERY == 0 {
            self.add(
                t.now_ms,
                format!(
                    "tick  #{} cpu {:.0}% disk {:.1} MB/s mem {:.0}% | {done}/{total} done | quiet_since={} usable={} | {} processes | tick work {} ms (worst {}) | own cpu {} ms | frames {}",
                    self.ticks,
                    t.cpu,
                    mb(t.disk),
                    t.mem,
                    opt(t.quiet_since),
                    t.usable,
                    t.processes,
                    t.work_ms,
                    self.worst_tick_ms,
                    t.own_cpu_ms,
                    t.frames,
                ),
            );
        }
        self.ticks += 1;
        if self.ticks % 30 == 0 {
            self.save();
        }
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(crate::settings::data_dir());
        let _ = std::fs::write(log_path(), self.lines.join("\r\n"));
    }
}

fn opt(v: Option<u64>) -> String {
    v.map(|v| format!("{v}ms")).unwrap_or_else(|| "-".into())
}

fn mb(bytes: f64) -> f64 {
    bytes / 1_048_576.0
}

fn when(unix: u64) -> String {
    match win::local_time(unix) {
        Some((y, mo, d, _, h, mi)) => format!("{y}-{mo:02}-{d:02} {h:02}:{mi:02}"),
        None => unix.to_string(),
    }
}

fn item_line(i: &Item) -> String {
    format!(
        "{} | {} | id={} | self={} | exe={} | cmd={}",
        i.name,
        i.origin(),
        i.id,
        i.is_self(),
        i.exe.as_deref().unwrap_or("-"),
        i.command
    )
}

fn record_block(out: &mut String, b: &BootRecord) {
    let _ = writeln!(
        out,
        "sign-in {} ({}) mode={:?} usable={}ms watch_from={}ms watched={}ms items={} progress={:?}",
        when(b.sign_in),
        b.sign_in,
        b.mode,
        b.usable_ms,
        b.watch_from_ms,
        b.watched_ms,
        b.items.len(),
        b.progress(),
    );
    let before = b.items.iter().filter(|r| r.before_sign_in).count();
    let mut items: Vec<&ItemRecord> = b.items.iter().filter(|r| !r.before_sign_in).collect();
    items.sort_by_key(|r| (r.start_ms.is_none(), r.start_ms));
    let _ = writeln!(out, "  ({before} services already running before sign-in are left out)");
    for r in items {
        let _ = writeln!(
            out,
            "  {:?} | {} | {} | start={} ready={} est={} before_sign_in={} counts={} system={} | cpu={}ms io={} | id={}",
            r.status,
            r.item.name,
            r.item.origin(),
            opt(r.start_ms),
            opt(r.ready_ms),
            r.estimated,
            r.before_sign_in,
            r.counts(),
            r.item.runs_as_system(),
            r.cpu_ms,
            r.io_bytes,
            r.item.id,
        );
    }
}

/// The whole report, as text for the clipboard. `live` is the record being
/// watched right now, if the collector is running in this process.
pub fn report(version: &str, settings: &Settings, live: Option<&BootRecord>) -> String {
    let mut out = String::new();
    let now = win::ft_to_unix(win::now_ft());
    let _ = writeln!(out, "{} diagnostics", crate::brand::NAME);
    let _ = writeln!(out, "=====================");
    let _ = writeln!(out, "version: {version}");
    let _ = writeln!(out, "generated: {} ({now})", when(now));
    let _ = writeln!(out, "exe: {}", std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default());
    let _ = writeln!(out, "args: {:?}", std::env::args().skip(1).collect::<Vec<_>>());
    let _ = writeln!(out, "windows: {}", win::windows_version());
    let _ = writeln!(out, "logical processors: {}", std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0));
    let _ = writeln!(out, "memory in use: {:.0}%", win::memory_load());
    let _ = writeln!(out, "session: {}", win::current_session());

    let _ = writeln!(out, "\nThis process");
    let _ = writeln!(out, "------------");
    let me = std::process::id();
    let procs = win::ProcessLister::default().list();
    if let Some(p) = procs.iter().find(|p| p.pid == me) {
        let _ = writeln!(out, "pid {me}: cpu {} ms, io {} bytes, running {} s", p.cpu / 10_000, p.io, win::now_ft().saturating_sub(p.created) / 10_000_000);
    }
    let (working, private) = win::own_memory();
    let _ = writeln!(out, "memory: working set {:.1} MB, private {:.1} MB", mb(working as f64), mb(private as f64));
    let _ = writeln!(out, "processes on this PC: {}", procs.len());

    let _ = writeln!(out, "\nSettings");
    let _ = writeln!(out, "--------");
    let _ = writeln!(out, "watch={} early={} set_up={} auto_update={} last_update_check={}", settings.watch, settings.early, settings.set_up, settings.auto_update, settings.last_update_check);
    let _ = writeln!(out, "run entry: {}", crate::autostart::standard_command().unwrap_or_else(|| "(none)".into()));
    let _ = writeln!(out, "held back: {}", settings.delayed.len());
    for i in &settings.delayed {
        let _ = writeln!(out, "  {}", item_line(i));
    }
    let _ = writeln!(out, "turned off: {}", settings.turned_off.len());
    for i in &settings.turned_off {
        let _ = writeln!(out, "  {}", item_line(i));
    }

    if let Some(b) = live {
        let _ = writeln!(out, "\nBeing watched now");
        let _ = writeln!(out, "-----------------");
        record_block(&mut out, b);
    }

    let all = history::load();
    let _ = writeln!(out, "\nHistory ({} sign-ins, newest first)", all.len());
    let _ = writeln!(out, "----------------------------------");
    for b in all.iter().rev().take(10) {
        if live.is_some_and(|l| l.sign_in == b.sign_in) {
            let _ = writeln!(out, "sign-in {} ({}): the one being watched now, as last saved: usable={}ms watched={}ms", when(b.sign_in), b.sign_in, b.usable_ms, b.watched_ms);
            continue;
        }
        record_block(&mut out, b);
    }

    let _ = writeln!(out, "\nStartup list now");
    let _ = writeln!(out, "----------------");
    win::com_init();
    let items = crate::startup::gather();
    let services = items.iter().filter(|i| i.kind() == crate::startup::Kind::Service).count();
    for i in items.iter().filter(|i| i.kind() != crate::startup::Kind::Service) {
        let wait = history::wait_limit_ms(
            history::typical_start(&all, &i.id),
            all.iter().any(|b| b.items.iter().any(|r| r.item.id == i.id)),
        );
        let _ = writeln!(out, "  {} | wait_limit={wait}ms never_starts={}", item_line(i), history::never_starts(&all, &i.id));
    }
    let _ = writeln!(out, "  (+ {services} automatic services)");

    let _ = writeln!(out, "\nCollector log of the last watched sign-in");
    let _ = writeln!(out, "-----------------------------------------");
    match std::fs::read_to_string(log_path()) {
        Ok(text) if !text.is_empty() => out.push_str(&text),
        _ => out.push_str("(none yet: written at the next watched sign-in)"),
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::startup::Source;

    fn rec(id: &str, status: Status, start: Option<u64>) -> ItemRecord {
        let item = Item {
            id: id.into(),
            name: id.into(),
            source: Source::Run { machine: false, wow32: false, once: false, value: id.into() },
            command: String::new(),
            exe: None,
            about: String::new(),
            as_system: false,
        };
        ItemRecord { status, start_ms: start, ..ItemRecord::new(item) }
    }

    fn tick(now_ms: u64, quiet: bool) -> Tick {
        Tick { now_ms, cpu: 10.0, disk: 0.0, mem: 40.0, quiet_since: quiet.then_some(now_ms), usable: false, work_ms: 3, own_cpu_ms: 0, processes: 1, frames: 0 }
    }

    /// Prints a real report: `cargo test --release -- --ignored live_report --nocapture`.
    #[test]
    #[ignore]
    fn live_report() {
        println!("{}", report("test", &crate::settings::load(), None));
    }

    #[test]
    fn logs_changes_once_and_readings_now_and_then() {
        let mut log = Log::default();
        let a = [rec("a", Status::Waiting, None)];
        log.observe(&tick(1_000, false), a.iter());
        // First sight of the item, the busy line and the first readings line.
        assert_eq!(log.lines.len(), 3);
        log.observe(&tick(2_000, false), a.iter());
        assert_eq!(log.lines.len(), 3, "nothing changed");
        let b = [rec("a", Status::Started, Some(1_500))];
        log.observe(&tick(3_000, true), b.iter());
        assert_eq!(log.lines.len(), 5);
        assert!(log.lines[3].contains("Waiting -> Started") && log.lines[3].contains("start=1500ms"));
        assert!(log.lines[4].contains("quiet"));
    }
}
