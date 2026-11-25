//! "Pause the rest": freezes this account's startup apps for a while so the
//! user can get on with work, then lets them carry on.
//!
//! Only apps and sign-in tasks running as the user are paused, never
//! services, programs in the Windows folder or security software: freezing
//! those can trip tamper protection or cut the network. An app the user
//! brings to the front, or starts again, carries on at once. Every frozen
//! process is written to a file as it is frozen, so quitting, or a crash,
//! followed by the next run, never leaves one frozen.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::startup::{Item, Kind};

/// How long a pause lasts unless the user ends it sooner.
pub const PAUSE_MS: u64 = 10 * 60_000;

/// Folders of security, VPN and device-management software. Freezing these
/// is at best refused and at worst raised with IT as tampering.
const PROTECTED: [&str; 24] = [
    "\\windows\\",
    "\\windows defender",
    "\\microsoft security client",
    "\\palo alto networks\\",
    "\\crowdstrike\\",
    "\\sentinelone\\",
    "\\cylance\\",
    "\\sophos\\",
    "\\mcafee\\",
    "\\symantec",
    "\\norton",
    "\\trend micro\\",
    "\\eset\\",
    "\\bitdefender\\",
    "\\kaspersky",
    "\\malwarebytes\\",
    "\\zscaler\\",
    "\\netskope\\",
    "\\carbon black",
    "\\tanium\\",
    "\\cisco\\",
    "\\fortinet\\",
    "\\microsoft intune management extension\\",
    "\\avast",
];

pub fn protected(exe: &str) -> bool {
    let exe = exe.to_lowercase();
    PROTECTED.iter().any(|p| exe.contains(p))
}

/// Can this startup item be paused at all?
pub fn can_pause(item: &Item) -> bool {
    !item.is_self()
        && !item.runs_as_system()
        && matches!(item.kind(), Kind::App | Kind::Task)
        && item.exe.as_deref().is_some_and(|e| e.contains('\\') && !protected(e))
}

fn path() -> std::path::PathBuf {
    crate::settings::data_dir().join("paused.json")
}

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    /// (pid, creation time) of every process frozen and not yet let go.
    procs: Vec<(u32, u64)>,
}

/// Writes the frozen processes, or removes the file when there are none.
pub fn save(procs: &HashSet<(u32, u64)>) {
    if procs.is_empty() {
        let _ = std::fs::remove_file(path());
        return;
    }
    let _ = std::fs::create_dir_all(crate::settings::data_dir());
    let saved = Saved { procs: procs.iter().copied().collect() };
    if let Ok(json) = serde_json::to_string(&saved) {
        let _ = std::fs::write(path(), json);
    }
}

/// Lets go of every process a pause left frozen: on quitting, and at the
/// start of each run in case the last one ended without doing so. A process
/// is resumed only if it is the very one frozen (same id and start time).
pub fn release_all() {
    let Some(saved) = std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str::<Saved>(&s).ok()) else {
        return;
    };
    let wanted: HashSet<(u32, u64)> = saved.procs.into_iter().collect();
    for p in crate::win::ProcessLister::default().list() {
        if wanted.contains(&(p.pid, p.created)) {
            crate::win::resume_process(p.pid);
        }
    }
    let _ = std::fs::remove_file(path());
}

/// Does the user want a paused item back? Yes when one of its frozen windows
/// is in front, or when a new process of it starts after it was frozen:
/// someone opened it again.
pub fn wants_back(frozen: &HashSet<(u32, u64)>, frozen_at: u64, running: &[(u32, u64)], foreground: Option<u32>) -> bool {
    if frozen.is_empty() {
        return false;
    }
    foreground.is_some_and(|f| frozen.iter().any(|k| k.0 == f))
        || running.iter().any(|k| !frozen.contains(k) && k.1 > frozen_at)
}

/// What the window shows of a pause in progress.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PauseView {
    /// ms after sign-in when it ends by itself.
    pub until_ms: u64,
    /// Ids of the items frozen now.
    pub items: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::startup::Source;

    fn app(exe: &str) -> Item {
        Item {
            id: "x".into(),
            name: "X".into(),
            exe: Some(exe.into()),
            command: String::new(),
            source: Source::Run { machine: false, wow32: false, once: false, value: "X".into() },
            about: String::new(),
            as_system: false,
        }
    }

    #[test]
    fn an_app_comes_back_when_brought_to_the_front_or_opened_again() {
        let frozen: HashSet<(u32, u64)> = [(10, 100), (11, 105)].into_iter().collect();
        // Still frozen, nothing else going on.
        assert!(!wants_back(&frozen, 200, &[(10, 100), (11, 105)], Some(99)));
        // Its window is in front.
        assert!(wants_back(&frozen, 200, &[(10, 100)], Some(11)));
        // A new process of it started after the freeze.
        assert!(wants_back(&frozen, 200, &[(10, 100), (12, 250)], None));
        // One that started before the freeze is just not frozen yet.
        assert!(!wants_back(&frozen, 200, &[(10, 100), (12, 150)], None));
        // Nothing frozen: nothing to bring back.
        assert!(!wants_back(&HashSet::new(), 200, &[(12, 250)], Some(12)));
    }

    /// Freezes a busy child process and lets it go: run with --ignored.
    #[test]
    #[ignore]
    fn live_freeze_and_resume() {
        let mut child = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", "while ($true) { $i++ }"])
            .spawn()
            .unwrap();
        let pid = child.id();
        let cpu = || crate::win::ProcessLister::default().list().into_iter().find(|p| p.pid == pid).map(|p| p.cpu).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1500));
        assert!(crate::win::suspend_process(pid));
        std::thread::sleep(std::time::Duration::from_millis(200));
        let a = cpu();
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let frozen = cpu() - a;
        assert!(crate::win::resume_process(pid));
        let b = cpu();
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let running = cpu() - b;
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(frozen, 0, "used processor time while frozen");
        assert!(running > 2_000_000, "did not carry on: {running}");
    }

    #[test]
    fn security_and_windows_programs_are_never_paused() {
        assert!(can_pause(&app(r"c:\users\me\appdata\local\programs\@gleandesktop\glean.exe")));
        assert!(can_pause(&app(r"c:\program files\google\chrome\application\chrome.exe")));
        assert!(!can_pause(&app(r"c:\program files\palo alto networks\traps\cytray.exe")));
        assert!(!can_pause(&app(r"c:\program files\palo alto networks\globalprotect\pangpa.exe")));
        assert!(!can_pause(&app(r"c:\windows\system32\securityhealthsystray.exe")));
        assert!(!can_pause(&app("chrome.exe")));
    }
}
