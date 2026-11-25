//! The collector: watches the startup items once a second from sign-in until
//! the PC is usable, then writes the boot's timings. Items that had not shown
//! up by then are watched on, so apps Windows starts late are still recorded.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::history::{self, BootRecord, ItemRecord, Mode, Status};
use crate::startup::{Item, Kind, Source};
use crate::win::{self, Proc};

/// The PC counts as settled below these for `SETTLE_MS`.
pub const QUIET_CPU: f32 = 20.0;
pub const QUIET_DISK: f64 = 8.0 * 1_048_576.0;
pub const SETTLE_MS: u64 = 5_000;
/// Never call a boot usable before this, nor keep watching past `GIVE_UP_MS`.
const MIN_MS: u64 = 15_000;
const GIVE_UP_MS: u64 = 300_000;
/// A collector started this long after sign-in has nothing to watch.
const LATE_MS: u64 = 600_000;
/// An app counts as busy in a second where it used this much processor time.
const BUSY_CPU_MS: u64 = 20;
/// Once it has settled, it takes this much in a second to call it busy
/// again: a passing blip must not reopen it and push "usable" back.
const REBUSY_CPU_MS: u64 = 250;
/// How often services are read while startup is timed.
const SERVICE_POLL_MS: u64 = 3_000;
/// Once usable, the collector looks this often for apps still to start.
const AFTER_USABLE_TICK_MS: u64 = 2_000;
/// A ready time rebuilt for an app that was already running is a guess, so
/// for this long after watching began any activity still corrects it. After
/// that it stands like a measured one.
const ESTIMATE_GRACE_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Metrics {
    pub cpu: f32,
    pub mem: f32,
    pub disk: f64,
    pub elapsed_ms: u64,
    pub eta_ms: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Shared {
    pub record: BootRecord,
    pub metrics: Metrics,
    /// The startup list has been read.
    pub listed: bool,
    /// Usable: the record is final and saved.
    pub done: bool,
    /// Boots in history before this one.
    pub earlier: Vec<BootRecord>,
    /// The latest result of an action, for the user.
    pub note: Option<String>,
    /// The collector has stopped: nothing is watched any more.
    pub finished: bool,
    /// The window was closed: exit as soon as the collector has finished.
    pub exit_when_done: bool,
    /// Frames the window has drawn, for the diagnostics log.
    pub frames: u64,
    /// What the window did since the last tick, for the diagnostics log.
    pub events: Vec<String>,
    /// A "pause the rest" in progress.
    pub pause: Option<crate::pause::PauseView>,
}

pub enum Request {
    End(String),
    EndWhenSeen(String),
    /// Freeze the startup apps still to settle, for `pause::PAUSE_MS`.
    Pause,
    /// Let every paused app carry on now.
    Resume,
}

#[derive(Clone)]
pub struct Handle {
    pub shared: Arc<Mutex<Shared>>,
    tx: Sender<Request>,
}

impl Handle {
    /// A handle on state kept elsewhere: the full window's copy of a
    /// collector in another process (see `live.rs`).
    pub fn new(shared: Arc<Mutex<Shared>>, tx: Sender<Request>) -> Self {
        Self { shared, tx }
    }

    pub fn send(&self, r: Request) {
        let _ = self.tx.send(r);
    }
}

/// Starts the collector thread. `repaint` is called after every tick.
pub fn spawn(mode: Mode, delayed: Vec<Item>, repaint: impl Fn() + Send + 'static) -> Handle {
    let shared = Arc::new(Mutex::new(Shared {
        record: BootRecord { sign_in: 0, mode, usable_ms: 0, watch_from_ms: 0, watched_ms: 0, items: Vec::new(), paused_ms: None },
        metrics: Metrics::default(),
        listed: false,
        done: false,
        earlier: Vec::new(),
        note: None,
        finished: false,
        // The collector has no window to keep open: it ends when done.
        exit_when_done: true,
        frames: 0,
        events: Vec::new(),
        pause: None,
    }));
    let (tx, rx) = channel();
    let s = shared.clone();
    std::thread::spawn(move || run(mode, delayed, s, rx, repaint));
    Handle { shared, tx }
}

/// A tick's processes, found by file name: each startup item compares
/// itself only with processes of its own file name, not with all several
/// hundred on the PC.
pub struct ProcIndex<'a> {
    by_name: HashMap<&'a str, Vec<&'a Proc>>,
}

impl<'a> ProcIndex<'a> {
    pub fn new(all: &'a [Proc]) -> Self {
        let mut by_name: HashMap<&str, Vec<&Proc>> = HashMap::with_capacity(all.len());
        for p in all {
            by_name.entry(p.name.as_str()).or_default().push(p);
        }
        Self { by_name }
    }

    /// Processes that may belong to `exe`: those with its file name.
    /// `matches` decides.
    pub fn named(&self, exe: &str) -> impl Iterator<Item = &'a Proc> + '_ {
        let file = exe.rsplit('\\').next().unwrap_or(exe);
        let with_exe = (!file.ends_with(".exe")).then(|| format!("{file}.exe"));
        let a = self.by_name.get(file).into_iter().flatten();
        let b = with_exe.and_then(|f| self.by_name.get(f.as_str())).into_iter().flatten();
        a.chain(b).copied()
    }
}

/// Does this process belong to the item?
pub fn matches(exe: &str, p: &Proc) -> bool {
    // Store app aliases under WindowsApps run a program from elsewhere with
    // the same file name.
    if exe.contains("\\windowsapps\\") && !exe.contains("\\program files\\windowsapps\\") {
        return exe.rsplit('\\').next() == Some(p.name.as_str());
    }
    if exe.contains('\\') {
        match &p.path {
            Some(path) => path == exe,
            None => exe.rsplit('\\').next() == Some(p.name.as_str()),
        }
    } else {
        let bare = exe.trim_end_matches(".exe");
        p.name.trim_end_matches(".exe") == bare
    }
}

/// Status of an app or task from what has been seen of its processes.
pub fn app_status(seen: bool, alive: bool, since_start_ms: u64, since_busy_ms: Option<u64>, waited_out: bool) -> Status {
    if alive {
        let busy = since_busy_ms.is_some_and(|b| b < 3_000);
        if busy && since_start_ms < 120_000 { Status::Starting } else { Status::Started }
    } else if seen {
        Status::Exited
    } else if waited_out {
        Status::NotSeen
    } else {
        Status::Waiting
    }
}

/// Processor time in one second that marks an app as busy. One that has
/// settled takes a real burst to reopen; a guessed ready time gives way to
/// any activity, but only while watching has just begun.
pub fn busy_threshold(ready: bool, estimated: bool, watched_ms: u64) -> u64 {
    let settled = ready && (!estimated || watched_ms >= ESTIMATE_GRACE_MS);
    if settled { REBUSY_CPU_MS } else { BUSY_CPU_MS }
}

/// When the PC became usable, if it has: everything counted is done and the
/// machine has been quiet since `quiet_since` for long enough.
pub fn usable_at(all_done: bool, quiet_since: Option<u64>, now_ms: u64) -> Option<u64> {
    if now_ms >= GIVE_UP_MS {
        return Some(now_ms);
    }
    let since = quiet_since?;
    (all_done && now_ms >= MIN_MS && now_ms.saturating_sub(since) >= SETTLE_MS).then_some(since.max(MIN_MS - SETTLE_MS))
}

/// Sign-in time in FILETIME ticks: when this session's shell host started.
fn sign_in_time(procs: &[Proc], session: u32) -> u64 {
    procs
        .iter()
        .filter(|p| p.session == session && (p.name == "sihost.exe" || p.name == "explorer.exe" || p.name == "userinit.exe"))
        .map(|p| p.created)
        .min()
        .unwrap_or_else(win::now_ft)
}

/// Ends every process of the item in this session. Returns how many.
pub fn end_item(item: &Item) -> (usize, usize) {
    let Some(exe) = &item.exe else { return (0, 0) };
    let session = win::current_session();
    let me = std::process::id();
    let mut lister = win::ProcessLister::default();
    let mut found = 0;
    let mut ended = 0;
    for p in lister.list() {
        if p.session == session && p.pid != me && matches(exe, &p) {
            found += 1;
            if win::end_process(p.pid) {
                ended += 1;
            }
        }
    }
    (found, ended)
}

struct Track {
    rec: ItemRecord,
    /// Live processes: (pid, created) -> (cpu ticks, io bytes) at first and last sight.
    procs: HashMap<(u32, u64), (u64, u64)>,
    gone_cpu: u64,
    gone_io: u64,
    last_cpu: u64,
    busy_at: Option<u64>,
    seen: bool,
    limit_ms: u64,
    end_when_seen: bool,
    /// Services: seen not running during this watch.
    seen_down: bool,
    /// Had not shown up when the PC became usable: still watched after it.
    late: bool,
    /// Never started in recent boots watched to the end: not waited for.
    never_starts: bool,
    /// Processes frozen by a pause, as (pid, creation time).
    frozen: HashSet<(u32, u64)>,
    /// When it was last frozen, FILETIME ticks.
    frozen_at: u64,
    /// The user wanted it back during this pause: left alone until it ends.
    let_through: bool,
}

/// Still to show up when the PC becomes usable?
fn is_missing(rec: &ItemRecord) -> bool {
    match rec.item.kind() {
        // Only apps are watched on after "usable". Tasks and delayed
        // services that have not run by then mostly never do, and waiting
        // for them kept the app in memory for ten minutes.
        Kind::App => matches!(rec.status, Status::Waiting | Status::NotSeen),
        Kind::Task | Kind::Service => false,
    }
}

/// The final word on an item nobody is waiting for any longer.
fn settle(rec: &mut ItemRecord, now_ms: u64) {
    match rec.status {
        Status::Waiting => rec.status = Status::NotSeen,
        Status::Starting => {
            rec.status = Status::Started;
            rec.ready_ms.get_or_insert(now_ms);
        }
        _ => {}
    }
}

/// The record's items as they stand: everything not still watched is settled.
fn snapshot(tracks: &[Track], now_ms: u64, last: bool) -> Vec<ItemRecord> {
    tracks
        .iter()
        .map(|t| {
            let mut r = t.rec.clone();
            if last || r.status == Status::Waiting {
                settle(&mut r, now_ms);
            }
            r
        })
        .collect()
}

fn ms_after(t: u64, sign_in: u64) -> u64 {
    t.saturating_sub(sign_in) / 10_000
}

fn run(mode: Mode, delayed: Vec<Item>, shared: Arc<Mutex<Shared>>, rx: Receiver<Request>, repaint: impl Fn()) {
    win::com_init();
    if mode == Mode::Early {
        win::raise_own_priority();
    }
    let session = win::current_session();
    let mut lister = win::ProcessLister::default();
    let mut counters = win::Counters::open();
    let procs = lister.list();
    let sign_in = sign_in_time(&procs, session);
    let watch_from_ms = ms_after(win::now_ft(), sign_in);
    if watch_from_ms > LATE_MS {
        // Started by hand long after sign-in: there is no startup to time,
        // and a record now would spoil the averages.
        crate::tray::remove();
        std::process::exit(0);
    }
    let earlier = history::load();
    let totals = history::recent_totals(&earlier);

    let mut tracks: Vec<Track> = gather_tracks(&earlier, &delayed);
    let mut log = crate::diag::Log::default();
    let me = std::process::id();
    log.add(
        watch_from_ms,
        format!(
            "start {} mode={mode:?} sign_in={} session={session} processes={} earlier_boots={} tracks={} held_back={}",
            env!("CARGO_PKG_VERSION"),
            win::ft_to_unix(sign_in),
            procs.len(),
            earlier.len(),
            tracks.len(),
            delayed.len(),
        ),
    );
    for t in tracks.iter().filter(|t| t.rec.item.kind() != Kind::Service) {
        log.add(
            watch_from_ms,
            format!(
                "track {} | {} | wait_limit={}ms never_starts={} self={} | exe={}",
                t.rec.item.name,
                t.rec.item.origin(),
                t.limit_ms,
                t.never_starts,
                t.rec.item.is_self(),
                t.rec.item.exe.as_deref().unwrap_or("-"),
            ),
        );
    }
    {
        let mut s = shared.lock().unwrap();
        s.record.sign_in = win::ft_to_unix(sign_in);
        s.record.watch_from_ms = watch_from_ms;
        s.earlier = earlier.clone();
    }

    let task_runs = win::TaskRuns::open();
    let mut tasks: Vec<win::LogonTask> = Vec::new();
    let mut last_task_poll: Option<Instant> = None;
    let mut services: Vec<win::ServiceStatus> = Vec::new();
    let mut last_service_poll: Option<Instant> = None;
    let mut quiet_since: Option<u64> = None;
    let mut usable_ms: Option<u64> = None;
    let mut late_started = 0;
    let mut first = true;
    let mut pause_until: Option<u64> = None;
    let mut procs = procs;

    loop {
        let tick = Instant::now();
        let now = win::now_ft();
        let now_ms = ms_after(now, sign_in);
        // Task Scheduler is asked only while a sign-in task has not been
        // seen: it is the one way to catch a task that ran and closed
        // between two ticks, and it costs more than the rest of a tick.
        // Only the tasks not yet seen are asked about, each by its path.
        let unseen: Vec<&str> = if usable_ms.is_none() {
            tracks
                .iter()
                .filter(|t| !t.seen)
                .filter_map(|t| match &t.rec.item.source {
                    Source::Task { path } => Some(path.as_str()),
                    _ => None,
                })
                .collect()
        } else {
            Vec::new()
        };
        if !unseen.is_empty() && last_task_poll.is_none_or(|at| at.elapsed() > Duration::from_secs(5)) {
            tasks = task_runs.last_runs(&unseen);
            last_task_poll = Some(Instant::now());
        }
        // Services every few seconds, and not at all once usable: their
        // start times come from Windows, so a slower look loses nothing, and
        // reading them all costs as much as the rest of a tick.
        if usable_ms.is_none() && last_service_poll.is_none_or(|at| at.elapsed() >= Duration::from_millis(SERVICE_POLL_MS)) {
            services = win::service_statuses();
            last_service_poll = Some(Instant::now());
        }
        let (cpu, disk) = counters.as_mut().map(|c| c.sample()).unwrap_or((0.0, 0.0));
        let mem = win::memory_load();

        while let Ok(req) = rx.try_recv() {
            let (id, now_or_later) = match &req {
                Request::End(id) => (id.clone(), true),
                Request::EndWhenSeen(id) => (id.clone(), false),
                Request::Pause => {
                    if pause_until.is_none() {
                        pause_until = Some(now_ms + crate::pause::PAUSE_MS);
                        shared.lock().unwrap().record.paused_ms.get_or_insert(now_ms);
                        log.add(now_ms, "pause the rest");
                    }
                    continue;
                }
                Request::Resume => {
                    if pause_until.take().is_some() {
                        let n = resume_all(&mut tracks);
                        log.add(now_ms, format!("resume now: {n} processes"));
                        let mut s = shared.lock().unwrap();
                        s.pause = None;
                        s.note = Some("Everything carries on starting.".to_owned());
                    }
                    continue;
                }
            };
            let Some(t) = tracks.iter_mut().find(|t| t.rec.item.id == id) else { continue };
            let note = if now_or_later {
                let (found, ended) = end_item(&t.rec.item);
                if ended > 0 {
                    t.rec.status = Status::Ended;
                    format!("Ended {}.", t.rec.item.name)
                } else if found > 0 {
                    format!("Windows would not end {}. It may be running as administrator.", t.rec.item.name)
                } else {
                    format!("{} is not running.", t.rec.item.name)
                }
            } else {
                t.end_when_seen = true;
                format!("{} will be ended as soon as it starts.", t.rec.item.name)
            };
            shared.lock().unwrap().note = Some(note);
        }

        if let Some(until) = pause_until {
            if now_ms >= until {
                pause_until = None;
                let n = resume_all(&mut tracks);
                log.add(now_ms, format!("pause over: {n} processes"));
                shared.lock().unwrap().note = Some("The pause is over: everything carries on starting.".to_owned());
            } else {
                let (back, refused) = pause_tick(&mut tracks, &ProcIndex::new(&procs), session, sign_in, me);
                for name in &back {
                    log.add(now_ms, format!("let through {name}"));
                }
                if let Some(name) = back.last() {
                    shared.lock().unwrap().note = Some(format!("{name} carries on: you opened it."));
                }
                if refused > 0 {
                    log.add(now_ms, format!("pause refused for {refused} processes"));
                }
            }
            let view = pause_until.map(|until_ms| crate::pause::PauseView {
                until_ms,
                items: tracks.iter().filter(|t| !t.frozen.is_empty()).map(|t| t.rec.item.id.clone()).collect(),
            });
            shared.lock().unwrap().pause = view;
        }

        let index = ProcIndex::new(&procs);
        for t in tracks.iter_mut() {
            // Once usable, the rest keep the figures they had at that moment.
            if usable_ms.is_none() || t.late {
                update_track(t, &index, &services, &tasks, session, sign_in, now_ms, first, watch_from_ms);
            }
        }
        if first {
            // The startup list, history and first look are done: what they
            // touched is not needed again.
            win::trim_memory();
        }
        first = false;
        // Before the log line, so it carries this tick's verdict.
        if usable_ms.is_none() {
            let quiet = cpu < QUIET_CPU && disk < QUIET_DISK;
            quiet_since = if quiet { quiet_since.or(Some(now_ms)) } else { None };
        }
        let (frames, events) = {
            let mut s = shared.lock().unwrap();
            (s.frames, std::mem::take(&mut s.events))
        };
        for e in events {
            log.add(now_ms, format!("ui    {e}"));
        }
        log.observe(
            &crate::diag::Tick {
                now_ms,
                cpu,
                disk,
                mem,
                quiet_since,
                usable: usable_ms.is_some(),
                work_ms: tick.elapsed().as_millis() as u64,
                own_cpu_ms: procs.iter().find(|p| p.pid == me).map(|p| p.cpu / 10_000).unwrap_or(0),
                processes: procs.len(),
                frames,
            },
            tracks.iter().map(|t| &t.rec),
        );

        if usable_ms.is_some() {
            // Usable, and the record is saved. Keep watching only for the
            // items that had not shown up, and save again as each one does.
            let started = tracks.iter().filter(|t| t.late && t.rec.start_ms.is_some()).count();
            let pending = tracks.iter().any(|t| t.late && (is_missing(&t.rec) || t.rec.status == Status::Starting));
            // A pause must run out (and everything carry on) before the end.
            let finished = (!pending || now_ms >= history::WATCH_END_MS) && pause_until.is_none();
            let exit = {
                let mut s = shared.lock().unwrap();
                s.metrics = Metrics { cpu, mem, disk, elapsed_ms: now_ms, eta_ms: None };
                s.record.items = snapshot(&tracks, now_ms, finished);
                s.record.watched_ms = now_ms;
                if finished || started != late_started {
                    history::save(&history::push(earlier.clone(), s.record.clone()));
                }
                s.finished = finished;
                s.exit_when_done
            };
            late_started = started;
            repaint();
            if finished {
                let still: Vec<&str> = tracks.iter().filter(|t| t.late && is_missing(&t.rec)).map(|t| t.rec.item.name.as_str()).collect();
                log.add(now_ms, format!("finished: pending={pending} late_started={started} never_seen={still:?} exit={exit}"));
                log.save();
                if exit {
                    crate::tray::remove();
                    std::process::exit(0);
                }
                return;
            }
            // Only late apps are left to see start, and their start time
            // comes from Windows: a slower look is just as exact.
            std::thread::sleep(Duration::from_millis(AFTER_USABLE_TICK_MS).saturating_sub(tick.elapsed()));
            procs = lister.list();
            continue;
        }

        let all_done = tracks.iter().filter(|t| t.rec.counts()).all(|t| t.rec.status.is_done());
        let waiting: Vec<u64> = tracks
            .iter()
            .filter(|t| t.rec.counts() && !t.rec.status.is_done())
            .filter_map(|t| history::typical_start(&earlier, &t.rec.item.id))
            .collect();
        let usable = usable_at(all_done, quiet_since, now_ms);

        {
            let mut s = shared.lock().unwrap();
            s.listed = true;
            s.metrics = Metrics {
                cpu,
                mem,
                disk,
                elapsed_ms: now_ms,
                eta_ms: history::eta_ms(now_ms, &totals, &waiting),
            };
            if let Some(at) = usable {
                for t in tracks.iter_mut() {
                    t.late = is_missing(&t.rec) && !t.never_starts;
                    if !t.late {
                        settle(&mut t.rec, now_ms);
                    }
                }
                let late: Vec<&str> = tracks.iter().filter(|t| t.late).map(|t| t.rec.item.name.as_str()).collect();
                log.add(
                    now_ms,
                    format!("usable at {at}ms: all_done={all_done} quiet_since={quiet_since:?} cpu {cpu:.0}% | still watching for {late:?}"),
                );
                log.save();
                s.record.items = snapshot(&tracks, now_ms, false);
                s.record.usable_ms = at;
                s.record.watched_ms = now_ms;
                let all = history::push(earlier.clone(), s.record.clone());
                history::save(&all);
                s.done = true;
                // The busy part is over: give back what it used.
                win::trim_memory();
            } else {
                s.record.items = tracks.iter().map(|t| t.rec.clone()).collect();
            }
        }
        repaint();
        if usable.is_some() {
            for item in &delayed {
                crate::startup::launch(item);
            }
            usable_ms = usable;
        }
        std::thread::sleep(Duration::from_millis(1000).saturating_sub(tick.elapsed()));
        procs = lister.list();
    }
}

/// One second of a pause: freezes the pausable items' processes, and lets an
/// item go when the user wants it back. Returns the names let go and how
/// many processes Windows would not freeze.
fn pause_tick(tracks: &mut [Track], procs: &ProcIndex, session: u32, sign_in: u64, me: u32) -> (Vec<String>, usize) {
    let foreground = win::foreground_pid();
    let mut back = Vec::new();
    let mut refused = 0;
    let mut changed = false;
    for t in tracks.iter_mut().filter(|t| !t.let_through && crate::pause::can_pause(&t.rec.item)) {
        let Some(exe) = t.rec.item.exe.clone() else { continue };
        let running: Vec<(u32, u64)> = procs
            .named(&exe)
            .filter(|p| p.session == session && p.created >= sign_in && p.pid != me && matches(&exe, p))
            .map(|p| (p.pid, p.created))
            .collect();
        if crate::pause::wants_back(&t.frozen, t.frozen_at, &running, foreground) {
            for &(pid, _) in &t.frozen {
                win::resume_process(pid);
            }
            t.frozen.clear();
            t.let_through = true;
            back.push(t.rec.item.name.clone());
            changed = true;
            continue;
        }
        let mut froze = false;
        for k in running {
            if t.frozen.contains(&k) {
                continue;
            }
            if win::suspend_process(k.0) {
                t.frozen.insert(k);
                froze = true;
            } else {
                refused += 1;
            }
        }
        if froze {
            t.frozen_at = win::now_ft();
            changed = true;
        }
    }
    if changed {
        save_frozen(tracks);
    }
    (back, refused)
}

fn save_frozen(tracks: &[Track]) {
    let all: HashSet<(u32, u64)> = tracks.iter().flat_map(|t| t.frozen.iter().copied()).collect();
    crate::pause::save(&all);
}

/// Ends a pause: every frozen process carries on. Returns how many.
fn resume_all(tracks: &mut [Track]) -> usize {
    let mut n = 0;
    for t in tracks.iter_mut() {
        for &(pid, _) in &t.frozen {
            n += win::resume_process(pid) as usize;
        }
        t.frozen.clear();
        t.let_through = false;
    }
    save_frozen(tracks);
    n
}

#[allow(clippy::too_many_arguments)]
fn update_track(
    t: &mut Track,
    procs: &ProcIndex,
    services: &[win::ServiceStatus],
    tasks: &[win::LogonTask],
    session: u32,
    sign_in: u64,
    now_ms: u64,
    first: bool,
    watch_from_ms: u64,
) {
    if matches!(t.rec.status, Status::Delayed | Status::Ended) && !t.end_when_seen {
        return;
    }
    match &t.rec.item.source {
        Source::Service { name, delayed } => {
            let Some(s) = services.iter().find(|s| &s.name == name) else {
                t.rec.status = Status::Exited;
                return;
            };
            match s.state {
                win::ServiceState::Running => {
                    if t.rec.status != Status::Started {
                        // Service hosts run in session 0, outside the list.
                        let host = win::process_created(s.pid);
                        if t.seen_down {
                            t.rec.start_ms = Some(now_ms);
                        } else if let Some(c) = host.filter(|&c| c >= sign_in) {
                            t.rec.start_ms = Some(ms_after(c, sign_in));
                        } else {
                            t.rec.before_sign_in = true;
                        }
                        t.rec.ready_ms = t.rec.start_ms;
                    }
                    t.rec.status = Status::Started;
                }
                win::ServiceState::Starting => {
                    t.seen_down = true;
                    t.rec.status = Status::Starting;
                }
                win::ServiceState::Stopped => {
                    t.seen_down = true;
                    t.rec.status = if *delayed && now_ms < 240_000 { Status::Waiting } else { Status::Exited };
                }
            }
        }
        _ => {
            let Some(exe) = t.rec.item.exe.clone() else {
                if now_ms >= t.limit_ms {
                    t.rec.status = Status::NotSeen;
                }
                return;
            };
            let me = std::process::id();
            let mut current: HashMap<(u32, u64), (u64, u64)> = HashMap::new();
            for p in procs.named(&exe) {
                if p.session != session || p.created < sign_in || !matches(&exe, p) {
                    continue;
                }
                if t.end_when_seen && p.pid != me {
                    win::end_process(p.pid);
                    t.rec.status = Status::Ended;
                    continue;
                }
                current.insert((p.pid, p.created), (p.cpu, p.io));
                let start = ms_after(p.created, sign_in);
                if t.rec.start_ms.is_none_or(|s| start < s) {
                    t.rec.start_ms = Some(start);
                }
            }
            if t.rec.status == Status::Ended {
                return;
            }
            let gone: HashSet<_> = t.procs.keys().filter(|k| !current.contains_key(k)).copied().collect();
            for k in gone {
                let (c, i) = t.procs.remove(&k).unwrap_or_default();
                t.gone_cpu += c;
                t.gone_io += i;
            }
            t.procs.extend(current.iter().map(|(k, v)| (*k, *v)));
            let cpu_ticks = t.gone_cpu + t.procs.values().map(|v| v.0).sum::<u64>();
            let io = t.gone_io + t.procs.values().map(|v| v.1).sum::<u64>();
            t.rec.cpu_ms = cpu_ticks / 10_000;
            t.rec.io_bytes = io;
            let alive = !current.is_empty();

            if alive && !t.seen {
                t.seen = true;
                if first && watch_from_ms > 2_000 {
                    // Already running when the app started: rebuild from
                    // its start time and processor time.
                    t.rec.estimated = true;
                    t.rec.ready_ms = t.rec.start_ms.map(|s| (s + t.rec.cpu_ms).min(watch_from_ms));
                } else {
                    t.busy_at = Some(now_ms);
                }
            } else {
                let used = cpu_ticks.saturating_sub(t.last_cpu) / 10_000;
                if used >= busy_threshold(t.rec.ready_ms.is_some(), t.rec.estimated, now_ms.saturating_sub(watch_from_ms)) {
                    t.busy_at = Some(now_ms);
                    t.rec.estimated = false;
                    t.rec.ready_ms = None;
                }
            }
            t.last_cpu = cpu_ticks;
            if alive && t.rec.item.is_self() {
                // The app is busy for as long as it watches: there is no
                // settling to time, only when it started and what it cost.
                t.rec.status = Status::Started;
                t.rec.estimated = false;
                t.rec.ready_ms = t.rec.start_ms;
                return;
            }

            if !t.seen {
                if let Source::Task { path } = &t.rec.item.source {
                    let ran = tasks.iter().find(|k| &k.path == path).map(|k| k.last_run).unwrap_or(0);
                    let sign_in_unix = win::ft_to_unix(sign_in);
                    if ran >= sign_in_unix && ran > 0 {
                        t.seen = true;
                        t.rec.start_ms = Some((ran - sign_in_unix) * 1000);
                    }
                }
            }

            let start = t.rec.start_ms.unwrap_or(now_ms);
            let status = app_status(
                t.seen,
                alive,
                now_ms.saturating_sub(start),
                t.busy_at.map(|b| now_ms.saturating_sub(b)),
                now_ms >= t.limit_ms,
            );
            if status == Status::Started && t.rec.ready_ms.is_none() {
                t.rec.ready_ms = Some(t.busy_at.unwrap_or(start).max(start));
            }
            if status == Status::Exited && t.rec.ready_ms.is_none() {
                t.rec.ready_ms = Some(t.busy_at.unwrap_or(start).max(start));
            }
            t.rec.status = status;
        }
    }
}

impl Track {
    fn new(rec: ItemRecord, limit_ms: u64) -> Self {
        Self {
            rec,
            procs: HashMap::new(),
            gone_cpu: 0,
            gone_io: 0,
            last_cpu: 0,
            busy_at: None,
            seen: false,
            limit_ms,
            end_when_seen: false,
            seen_down: false,
            late: false,
            never_starts: false,
            frozen: HashSet::new(),
            frozen_at: 0,
            let_through: false,
        }
    }
}

/// The startup list, ready to track, with the app's held-back items.
fn gather_tracks(earlier: &[BootRecord], delayed: &[Item]) -> Vec<Track> {
    let mut items = crate::startup::gather();
    items.retain(|i| !delayed.iter().any(|d| d.id == i.id));
    let mut tracks: Vec<Track> = items
        .into_iter()
        .map(|item| {
            let typical = history::typical_start(earlier, &item.id);
            let seen_before = earlier.iter().any(|b| b.items.iter().any(|r| r.item.id == item.id));
            let never_starts = history::never_starts(earlier, &item.id);
            let mut t = Track::new(ItemRecord::new(item), history::wait_limit_ms(typical, seen_before));
            t.never_starts = never_starts;
            t
        })
        .collect();
    for d in delayed {
        let mut rec = ItemRecord::new(d.clone());
        rec.status = Status::Delayed;
        tracks.push(Track::new(rec, 0));
    }
    tracks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(name: &str, path: Option<&str>) -> Proc {
        Proc { pid: 10, name: name.into(), path: path.map(Into::into), session: 1, created: 0, cpu: 0, io: 0 }
    }

    #[test]
    fn the_index_finds_what_matching_would() {
        let all = vec![
            proc("teams.exe", Some(r"c:\apps\teams\teams.exe")),
            proc("ms-teams.exe", Some(r"c:\program files\windowsapps\msteams_1\ms-teams.exe")),
            proc("chrome.exe", Some(r"c:\program files\google\chrome\application\chrome.exe")),
            proc("chrome.exe", None),
        ];
        let index = ProcIndex::new(&all);
        let exes = [
            r"c:\apps\teams\teams.exe",
            "teams",
            "teams.exe",
            r"c:\users\me\appdata\local\microsoft\windowsapps\msteams_8wekyb3d8bbwe\ms-teams.exe",
            r"c:\program files\google\chrome\application\chrome.exe",
            r"c:\nowhere\nothing.exe",
        ];
        for exe in exes {
            let by_index = index.named(exe).filter(|p| matches(exe, p)).count();
            let by_scan = all.iter().filter(|p| matches(exe, p)).count();
            assert_eq!(by_index, by_scan, "{exe}");
        }
    }

    #[test]
    fn matches_processes_by_path_or_name() {
        let p = proc("teams.exe", Some(r"c:\apps\teams\teams.exe"));
        assert!(matches(r"c:\apps\teams\teams.exe", &p));
        assert!(!matches(r"c:\other\teams.exe", &p));
        assert!(matches("teams.exe", &p));
        assert!(matches("teams", &p));
        let hidden = proc("teams.exe", None);
        assert!(matches(r"c:\apps\teams\teams.exe", &hidden));
        let py = proc("pythonw.exe", Some(r"c:\program files\windowsapps\python.3.12\pythonw.exe"));
        assert!(matches(r"c:\users\a\appdata\local\microsoft\windowsapps\pythonw.exe", &py));
    }

    #[test]
    fn app_status_follows_what_was_seen() {
        assert_eq!(app_status(false, false, 0, None, false), Status::Waiting);
        assert_eq!(app_status(false, false, 0, None, true), Status::NotSeen);
        assert_eq!(app_status(true, true, 2_000, Some(500), false), Status::Starting);
        assert_eq!(app_status(true, true, 9_000, Some(5_000), false), Status::Started);
        assert_eq!(app_status(true, true, 200_000, Some(0), false), Status::Started);
        assert_eq!(app_status(true, false, 9_000, None, false), Status::Exited);
    }

    /// Reads this machine's real startup list and processes. Read-only.
    /// `cargo test --release -- --ignored --nocapture live_smoke`
    /// What one second of watching costs: run with
    /// `cargo test --release -- --ignored live_cost --nocapture`.
    #[test]
    #[ignore]
    fn live_cost() {
        win::com_init();
        let mut lister = win::ProcessLister::default();
        let me = std::process::id();
        let mut own_cpu = |l: &mut win::ProcessLister| l.list().iter().find(|p| p.pid == me).map(|p| p.cpu / 10_000).unwrap_or(0);
        let mut time = |name: &str, n: u32, f: &mut dyn FnMut()| {
            let c0 = own_cpu(&mut lister);
            let t0 = Instant::now();
            for _ in 0..n {
                f();
            }
            let wall = t0.elapsed().as_millis() as u64 / n as u64;
            let cpu = (own_cpu(&mut lister) - c0) / n as u64;
            println!("{name:<22} wall {wall:>5} ms  own cpu {cpu:>5} ms  (each of {n})");
        };
        time("counters open (cold)", 1, &mut || drop(win::Counters::open()));
        time("logon_tasks", 5, &mut || drop(win::logon_tasks()));
        let runs = win::TaskRuns::open();
        let all = win::logon_tasks();
        let paths: Vec<&str> = all.iter().map(|t| t.path.as_str()).collect();
        time("task last runs", 5, &mut || drop(runs.last_runs(&paths)));
        assert_eq!(runs.last_runs(&paths).iter().map(|t| t.last_run).collect::<Vec<_>>(), all.iter().map(|t| t.last_run).collect::<Vec<_>>());
        time("service_statuses", 20, &mut || drop(win::service_statuses()));
        let mut l2 = win::ProcessLister::default();
        time("process list", 20, &mut || drop(l2.list()));
        time("startup gather", 3, &mut || drop(crate::startup::gather()));
        let mut c = win::Counters::open().expect("counters");
        time("counters sample", 20, &mut || drop(c.sample()));
    }

    #[test]
    #[ignore]
    fn live_smoke() {
        win::com_init();
        let items = crate::startup::gather();
        let mut lister = win::ProcessLister::default();
        let procs = lister.list();
        let session = win::current_session();
        let sign_in = sign_in_time(&procs, session);
        println!("sign-in {} s ago", (win::now_ft() - sign_in) / 10_000_000);
        println!("{} processes, {} with paths", procs.len(), procs.iter().filter(|p| p.path.is_some()).count());
        for i in items.iter().filter(|i| i.item_kind_is_not_service()) {
            let running = i.exe.as_ref().is_some_and(|e| procs.iter().any(|p| matches(e, p)));
            println!("{:<40} {:<28} running={running} exe={:?}", i.name, i.origin(), i.exe);
        }
        let services = items.iter().filter(|i| !i.item_kind_is_not_service()).count();
        let statuses = win::service_statuses();
        println!("{services} auto services; {} services known", statuses.len());
        if let Some(s) = items.iter().find(|i| !i.item_kind_is_not_service()) {
            println!("e.g. {}: {}", s.name, s.about);
        }
        let mut c = win::Counters::open().expect("counters");
        std::thread::sleep(Duration::from_millis(1000));
        println!("cpu/disk {:?}, mem {}", c.sample(), win::memory_load());
        println!("work area {:?}", win::work_area_points());
        assert!(!procs.is_empty());
    }

    #[test]
    fn a_blip_does_not_reopen_an_app_with_a_guessed_ready_time() {
        // Still starting, or a guess made a moment ago: any activity counts.
        assert_eq!(busy_threshold(false, false, 60_000), BUSY_CPU_MS);
        assert_eq!(busy_threshold(true, true, 3_000), BUSY_CPU_MS);
        // Settled, measured or guessed: only a real burst reopens it.
        assert_eq!(busy_threshold(true, false, 0), REBUSY_CPU_MS);
        assert_eq!(busy_threshold(true, true, 16_000), REBUSY_CPU_MS);
    }

    #[test]
    fn usable_needs_everything_done_and_a_quiet_spell() {
        assert_eq!(usable_at(true, Some(20_000), 26_000), Some(20_000));
        assert_eq!(usable_at(true, Some(20_000), 23_000), None);
        assert_eq!(usable_at(false, Some(20_000), 40_000), None);
        assert_eq!(usable_at(true, None, 40_000), None);
        // Never before the minimum, and always by the cut-off.
        assert_eq!(usable_at(true, Some(0), 16_000), Some(10_000));
        assert_eq!(usable_at(false, None, GIVE_UP_MS), Some(GIVE_UP_MS));
    }
}
