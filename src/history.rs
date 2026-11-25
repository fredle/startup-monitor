//! One record per boot, saved to `boots.json` once the PC is usable, and the
//! sums made from them: the ETA, the comparison with recent boots and what
//! cost the most.

use serde::{Deserialize, Serialize};

use crate::startup::{Item, Kind};

/// How many boots the comparison uses.
pub const COMPARE: usize = 10;
/// How many boots are kept on disk.
const KEEP: usize = 30;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Standard,
    Early,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Configured, nothing seen yet.
    Waiting,
    /// Running and still busy getting going.
    Starting,
    /// Running and settled.
    Started,
    /// Ran and closed again (or, for a service, is not running).
    Exited,
    /// Never seen before the PC was ready.
    NotSeen,
    /// Ended by the user from the app.
    Ended,
    /// Held back by the app until the PC was ready.
    Delayed,
}

impl Status {
    /// Finished one way or another: it no longer holds up "usable".
    pub fn is_done(self) -> bool {
        !matches!(self, Status::Waiting | Status::Starting)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ItemRecord {
    pub item: Item,
    pub status: Status,
    /// When it started, ms after sign-in.
    pub start_ms: Option<u64>,
    /// When it stopped being busy, ms after sign-in.
    pub ready_ms: Option<u64>,
    /// True when `ready_ms` was worked out afterwards rather than watched.
    #[serde(default)]
    pub estimated: bool,
    /// Was already running when the user signed in (services).
    #[serde(default)]
    pub before_sign_in: bool,
    /// Processor time used during startup, ms.
    pub cpu_ms: u64,
    /// Bytes read and written during startup.
    pub io_bytes: u64,
}

impl ItemRecord {
    pub fn new(item: Item) -> Self {
        Self {
            item,
            status: Status::Waiting,
            start_ms: None,
            ready_ms: None,
            estimated: false,
            before_sign_in: false,
            cpu_ms: 0,
            io_bytes: 0,
        }
    }

    /// Counted in "12 of 31": every app and task, and services that had not
    /// already started before sign-in. Delayed services come after "usable"
    /// by design and are left out.
    pub fn counts(&self) -> bool {
        // The app itself has started by definition: not part of the count.
        if self.item.is_self() {
            return false;
        }
        match self.item.kind() {
            Kind::Service => {
                !self.before_sign_in
                    && !matches!(self.item.source, crate::startup::Source::Service { delayed: true, .. })
                    && matches!(self.status, Status::Starting | Status::Started)
            }
            // Many sign-in tasks never run (updaters with other conditions,
            // maintenance jobs), so a task counts only once it has started:
            // waiting on them added a minute to "usable" for nothing.
            Kind::Task => !matches!(self.status, Status::Waiting | Status::NotSeen),
            Kind::App => true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BootRecord {
    /// Sign-in time, Unix seconds.
    pub sign_in: u64,
    pub mode: Mode,
    /// ms from sign-in until usable.
    pub usable_ms: u64,
    /// ms from sign-in until the app itself was running.
    pub watch_from_ms: u64,
    /// ms from sign-in until the app stopped watching. Items still missing
    /// when the PC became usable are watched on until `WATCH_END_MS`.
    #[serde(default)]
    pub watched_ms: u64,
    pub items: Vec<ItemRecord>,
}

impl BootRecord {
    /// (done, total) for the progress count.
    pub fn progress(&self) -> (usize, usize) {
        let counted: Vec<_> = self.items.iter().filter(|r| r.counts()).collect();
        (counted.iter().filter(|r| r.status.is_done()).count(), counted.len())
    }
}

fn path() -> std::path::PathBuf {
    crate::settings::data_dir().join("boots.json")
}

pub fn load() -> Vec<BootRecord> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Adds a boot, newest last, keeping the most recent few dozen.
pub fn push(mut all: Vec<BootRecord>, record: BootRecord) -> Vec<BootRecord> {
    all.retain(|b| b.sign_in != record.sign_in);
    all.push(record);
    all.sort_by_key(|b| b.sign_in);
    let extra = all.len().saturating_sub(KEEP);
    all.drain(..extra);
    all
}

pub fn save(all: &[BootRecord]) {
    let _ = std::fs::create_dir_all(crate::settings::data_dir());
    if let Ok(json) = serde_json::to_string(all) {
        let _ = std::fs::write(path(), json);
    }
}

pub fn median(values: &[u64]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_unstable();
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2 })
}

/// Time-to-usable of the last `COMPARE` boots, oldest first.
pub fn recent_totals(history: &[BootRecord]) -> Vec<u64> {
    let skip = history.len().saturating_sub(COMPARE);
    history[skip..].iter().map(|b| b.usable_ms).collect()
}

/// When this item usually starts, from past boots.
pub fn typical_start(history: &[BootRecord], id: &str) -> Option<u64> {
    let skip = history.len().saturating_sub(COMPARE);
    let starts: Vec<u64> = history[skip..]
        .iter()
        .filter_map(|b| b.items.iter().find(|r| r.item.id == id))
        .filter_map(|r| r.start_ms)
        .collect();
    median(&starts)
}

/// Items that have not shown up when the PC becomes usable are watched until
/// this long after sign-in, because Windows starts some apps minutes late.
pub const WATCH_END_MS: u64 = 600_000;

/// True when recent boots watched for the item to the very end and it never
/// started: not worth staying open for again. Boots that stopped watching
/// sooner say nothing either way, so an item is re-checked as they age out.
pub fn never_starts(history: &[BootRecord], id: &str) -> bool {
    let skip = history.len().saturating_sub(COMPARE);
    let full: Vec<&ItemRecord> = history[skip..]
        .iter()
        .filter(|b| b.watched_ms >= WATCH_END_MS)
        .filter_map(|b| b.items.iter().find(|r| r.item.id == id))
        .collect();
    full.len() >= 2 && full.iter().all(|r| r.start_ms.is_none())
}

/// How long to wait for an item that has not shown up before calling it
/// not seen: a bit past when it usually starts, or a minute without history.
pub fn wait_limit_ms(typical: Option<u64>, seen_before: bool) -> u64 {
    match typical {
        Some(t) => (t + 20_000).max(30_000),
        None if seen_before => 30_000,
        None => 60_000,
    }
}

/// Time left until usable, learned from past boots. `None` on the first
/// boot: no guess. `waiting` holds the usual start time of each item still
/// to come.
pub fn eta_ms(elapsed_ms: u64, totals: &[u64], waiting: &[u64]) -> Option<u64> {
    let usual = median(totals)?;
    let last_item = waiting.iter().copied().max().map(|t| t + 5_000).unwrap_or(0);
    let target = usual.max(last_item);
    Some(target.saturating_sub(elapsed_ms))
}

/// This boot against the usual one: positive means slower, in ms.
pub fn versus_usual(current_ms: u64, earlier: &[u64]) -> Option<i64> {
    median(earlier).map(|m| current_ms as i64 - m as i64)
}

/// The items that cost the most processor time, largest first.
pub fn costliest(record: &BootRecord, n: usize) -> Vec<&ItemRecord> {
    let mut v: Vec<&ItemRecord> = record
        .items
        .iter()
        .filter(|r| r.item.kind() != Kind::Service && (r.cpu_ms > 0 || r.io_bytes > 0))
        .collect();
    v.sort_by(|a, b| b.cpu_ms.cmp(&a.cpu_ms).then(b.io_bytes.cmp(&a.io_bytes)));
    v.truncate(n);
    v
}

/// "48 s", "1 min 05 s".
pub fn fmt_secs(ms: u64) -> String {
    let s = (ms + 500) / 1000;
    if s < 60 { format!("{s} s") } else { format!("{} min {:02} s", s / 60, s % 60) }
}

/// "11.2 s" below a minute, whole seconds above.
pub fn fmt_cpu(ms: u64) -> String {
    if ms < 10_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        fmt_secs(ms)
    }
}

pub fn fmt_bytes(b: u64) -> String {
    let mb = b as f64 / 1_048_576.0;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else if mb >= 10.0 {
        format!("{mb:.0} MB")
    } else {
        format!("{mb:.1} MB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::startup::Source;

    fn item(id: &str, source: Source) -> Item {
        Item { id: id.into(), name: id.into(), source, command: String::new(), exe: None, about: String::new() }
    }

    fn app(id: &str) -> Item {
        item(id, Source::Run { machine: false, wow32: false, once: false, value: id.into() })
    }

    fn boot(sign_in: u64, usable: u64, starts: &[(&str, u64)]) -> BootRecord {
        BootRecord {
            sign_in,
            mode: Mode::Standard,
            usable_ms: usable,
            watch_from_ms: 0,
            watched_ms: usable,
            items: starts
                .iter()
                .map(|(id, s)| ItemRecord { start_ms: Some(*s), status: Status::Started, ..ItemRecord::new(app(id)) })
                .collect(),
        }
    }

    #[test]
    fn an_item_is_given_up_on_only_after_two_full_watches() {
        let missing = |watched| {
            let mut b = boot(1, 40_000, &[("a", 1_000)]);
            b.watched_ms = watched;
            b.items.push(ItemRecord { status: Status::NotSeen, ..ItemRecord::new(app("late")) });
            b
        };
        // Stopped watching early: says nothing about a late starter.
        assert!(!never_starts(&[missing(40_000), missing(40_000), missing(40_000)], "late"));
        assert!(!never_starts(&[missing(WATCH_END_MS)], "late"));
        assert!(never_starts(&[missing(WATCH_END_MS), missing(WATCH_END_MS)], "late"));
        // It did start once in a full watch.
        let mut started = missing(WATCH_END_MS);
        started.items[1].start_ms = Some(150_000);
        assert!(!never_starts(&[missing(WATCH_END_MS), started, missing(WATCH_END_MS)], "late"));
        assert!(!never_starts(&[missing(WATCH_END_MS), missing(WATCH_END_MS)], "a"));
    }

    #[test]
    fn the_app_itself_is_listed_but_not_counted() {
        let own_run = item("me", Source::Run { machine: false, wow32: false, once: false, value: crate::brand::NAME.into() });
        let own_task = item("t", Source::Task { path: crate::brand::TASK_NAME.to_lowercase() });
        let other = item("o", Source::Run { machine: true, wow32: false, once: false, value: crate::brand::NAME.into() });
        assert!(own_run.is_self() && own_task.is_self());
        assert!(!other.is_self() && !app("teams").is_self());
        let mut b = boot(1, 0, &[("a", 0)]);
        b.items.push(ItemRecord { status: Status::Started, ..ItemRecord::new(own_run) });
        assert_eq!(b.progress(), (1, 1));
    }

    #[test]
    fn a_task_counts_only_once_it_has_started() {
        let task = |status| ItemRecord { status, ..ItemRecord::new(item("t", Source::Task { path: r"\\Vendor\\Updater".into() })) };
        assert!(!task(Status::Waiting).counts() && !task(Status::NotSeen).counts());
        assert!(task(Status::Starting).counts() && task(Status::Started).counts() && task(Status::Exited).counts());
        let mut b = boot(1, 0, &[("a", 0)]);
        b.items.push(task(Status::Waiting));
        assert_eq!(b.progress(), (1, 1));
    }

    #[test]
    fn median_handles_odd_even_and_empty() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[5, 1, 3]), Some(3));
        assert_eq!(median(&[4, 1, 3, 2]), Some(2));
    }

    #[test]
    fn no_estimate_without_history() {
        assert_eq!(eta_ms(5_000, &[], &[]), None);
    }

    #[test]
    fn estimate_counts_down_from_the_usual_boot() {
        assert_eq!(eta_ms(10_000, &[40_000, 50_000, 45_000], &[]), Some(35_000));
        assert_eq!(eta_ms(60_000, &[40_000], &[]), Some(0));
        // A late item pushes the estimate out.
        assert_eq!(eta_ms(10_000, &[40_000], &[50_000]), Some(45_000));
    }

    #[test]
    fn compares_against_the_usual_boot() {
        assert_eq!(versus_usual(50_000, &[40_000, 44_000, 42_000]), Some(8_000));
        assert_eq!(versus_usual(30_000, &[40_000]), Some(-10_000));
        assert_eq!(versus_usual(30_000, &[]), None);
    }

    #[test]
    fn keeps_boots_in_order_and_trims_old_ones() {
        let mut all = Vec::new();
        for i in (0..40).rev() {
            all = push(all, boot(i, 1000, &[]));
        }
        assert_eq!(all.len(), KEEP);
        assert_eq!(all.first().unwrap().sign_in, 10);
        assert_eq!(all.last().unwrap().sign_in, 39);
        // Same sign-in replaces, not duplicates.
        let all = push(all, boot(39, 2000, &[]));
        assert_eq!(all.len(), KEEP);
        assert_eq!(all.last().unwrap().usable_ms, 2000);
        assert_eq!(recent_totals(&all).len(), COMPARE);
    }

    #[test]
    fn learns_typical_start_times() {
        let h = vec![boot(1, 0, &[("a", 4_000)]), boot(2, 0, &[("a", 6_000)]), boot(3, 0, &[("b", 1_000)])];
        assert_eq!(typical_start(&h, "a"), Some(5_000));
        assert_eq!(typical_start(&h, "c"), None);
        assert_eq!(wait_limit_ms(Some(5_000), true), 30_000);
        assert_eq!(wait_limit_ms(Some(40_000), true), 60_000);
        assert_eq!(wait_limit_ms(None, false), 60_000);
    }

    #[test]
    fn ranks_by_processor_time_and_skips_services() {
        let mut b = boot(1, 0, &[("a", 0), ("b", 0), ("c", 0)]);
        b.items[0].cpu_ms = 100;
        b.items[1].cpu_ms = 900;
        b.items[2].cpu_ms = 0;
        let mut svc = ItemRecord::new(item("s", Source::Service { name: "s".into(), delayed: false }));
        svc.cpu_ms = 5_000;
        b.items.push(svc);
        let top: Vec<&str> = costliest(&b, 5).iter().map(|r| r.item.id.as_str()).collect();
        assert_eq!(top, vec!["b", "a"]);
    }

    #[test]
    fn progress_counts_apps_and_services_started_after_sign_in() {
        let mut b = boot(1, 0, &[("a", 0)]);
        b.items.push(ItemRecord::new(app("w")));
        let mut early_svc = ItemRecord::new(item("s1", Source::Service { name: "s1".into(), delayed: false }));
        early_svc.status = Status::Started;
        early_svc.before_sign_in = true;
        let mut late_svc = ItemRecord::new(item("s2", Source::Service { name: "s2".into(), delayed: false }));
        late_svc.status = Status::Starting;
        b.items.push(early_svc);
        b.items.push(late_svc);
        assert_eq!(b.progress(), (1, 3));
    }

    #[test]
    fn formats_times_and_sizes() {
        assert_eq!(fmt_secs(48_400), "48 s");
        assert_eq!(fmt_secs(65_000), "1 min 05 s");
        assert_eq!(fmt_cpu(11_240), "11 s");
        assert_eq!(fmt_cpu(2_340), "2.3 s");
        assert_eq!(fmt_bytes(5 * 1_048_576), "5.0 MB");
        assert_eq!(fmt_bytes(240 * 1_048_576), "240 MB");
        assert_eq!(fmt_bytes(3 * 1_073_741_824), "3.0 GB");
    }
}
