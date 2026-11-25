//! What is due to start at sign-in: Run and RunOnce keys, the two Startup
//! folders, logon scheduled tasks and automatic services, minus what Windows
//! has marked disabled. Also the per-user on/off switch Task Manager uses.

use serde::{Deserialize, Serialize};

use crate::win::{self, Hive, RegData};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_ONCE: &str = r"Software\Microsoft\Windows\CurrentVersion\RunOnce";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

/// Where an item comes from, with what is needed to act on it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Run { machine: bool, wow32: bool, once: bool, value: String },
    Folder { common: bool, file: String },
    Task { path: String },
    Service { name: String, delayed: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    App,
    Task,
    Service,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub source: Source,
    /// Command line, or the shortcut target and its arguments.
    #[serde(default)]
    pub command: String,
    /// Lower-case program path (or bare file name) used to spot its process.
    #[serde(default)]
    pub exe: Option<String>,
    /// For services: what it is, in one line.
    #[serde(default)]
    pub about: String,
}

impl Item {
    pub fn kind(&self) -> Kind {
        match self.source {
            Source::Run { .. } | Source::Folder { .. } => Kind::App,
            Source::Task { .. } => Kind::Task,
            Source::Service { .. } => Kind::Service,
        }
    }

    #[cfg(test)]
    pub fn item_kind_is_not_service(&self) -> bool {
        self.kind() != Kind::Service
    }

    /// The app's own sign-in entry: its Run value or its early-start task.
    /// Listed for honesty about its cost, but never counted or acted on.
    pub fn is_self(&self) -> bool {
        match &self.source {
            Source::Run { machine: false, once: false, value, .. } => value == crate::autostart::VALUE,
            Source::Task { path } => path.eq_ignore_ascii_case(crate::autostart::TASK_NAME),
            _ => false,
        }
    }

    /// Per-user entries can be turned off without administrator rights.
    pub fn can_disable(&self) -> bool {
        matches!(
            self.source,
            Source::Run { machine: false, once: false, .. } | Source::Folder { common: false, .. }
        )
    }

    /// Where Windows keeps it, for the details line.
    pub fn origin(&self) -> &'static str {
        match &self.source {
            Source::Run { machine: false, once: false, .. } => "Startup app, this account",
            Source::Run { machine: true, once: false, .. } => "Startup app, all users",
            Source::Run { once: true, .. } => "Runs once at next sign-in",
            Source::Folder { common: false, .. } => "Startup folder, this account",
            Source::Folder { common: true, .. } => "Startup folder, all users",
            Source::Task { .. } => "Scheduled task at sign-in",
            Source::Service { delayed: true, .. } => "Service, delayed start",
            Source::Service { delayed: false, .. } => "Service, automatic",
        }
    }

    /// The StartupApproved key and value name Task Manager uses for this
    /// item, for per-user items.
    fn approved_slot(&self) -> Option<(String, String)> {
        match &self.source {
            Source::Run { machine: false, once: false, value, .. } => Some((format!(r"{APPROVED}\Run"), value.clone())),
            Source::Folder { common: false, file } => {
                let name = std::path::Path::new(file).file_name()?.to_string_lossy().into_owned();
                Some((format!(r"{APPROVED}\StartupFolder"), name))
            }
            _ => None,
        }
    }
}

/// Pulls the program out of a command line: the quoted part, or everything up
/// to `.exe`, or the first word. Lower case, environment variables expanded
/// by `expand`.
pub fn exe_from_command(cmd: &str, expand: impl Fn(&str) -> String) -> Option<String> {
    let cmd = expand(cmd.trim());
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return None;
    }
    let exe = if let Some(rest) = cmd.strip_prefix('"') {
        rest.split('"').next().unwrap_or(rest).to_owned()
    } else {
        let lower = cmd.to_lowercase();
        match lower.find(".exe") {
            Some(at) => cmd[..at + 4].to_owned(),
            None => cmd.split_whitespace().next().unwrap_or(cmd).to_owned(),
        }
    };
    let exe = exe.trim().to_lowercase();
    (!exe.is_empty()).then_some(exe)
}

/// True when a StartupApproved value says "off". Windows stores an even
/// first byte for on and an odd one for off.
pub fn approved_says_disabled(data: &[u8]) -> bool {
    data.first().is_some_and(|b| b & 1 == 1)
}

/// The 12-byte StartupApproved value: on, or off since `filetime`.
pub fn approved_blob(enabled: bool, filetime: u64) -> [u8; 12] {
    let mut b = [0u8; 12];
    if enabled {
        b[0] = 2;
    } else {
        b[0] = 3;
        b[4..].copy_from_slice(&filetime.to_le_bytes());
    }
    b
}

/// A readable name from a registry value name or file name:
/// `com.squirrel.Teams.Teams` becomes `Teams`, `Notes.lnk` becomes `Notes`.
pub fn display_name(raw: &str) -> String {
    let raw = raw.trim();
    let lower = raw.to_lowercase();
    let raw = if lower.ends_with(".lnk") || lower.ends_with(".url") || lower.ends_with(".exe") {
        &raw[..raw.len() - 4]
    } else {
        raw
    };
    if raw.contains('.') && !raw.contains(' ') {
        // `com.glean.desktop`: the last part is often a word that says
        // nothing about the app, so the one before it names it.
        let parts: Vec<&str> = raw.split('.').filter(|s| !s.is_empty()).collect();
        if let Some(last) = parts.iter().rev().find(|s| !is_generic(s)).or(parts.last()) {
            return capitalise(last);
        }
    }
    raw.to_owned()
}

/// Words installers use for the part of an app that starts with Windows,
/// which on their own do not say which app it is.
fn is_generic(word: &str) -> bool {
    const WORDS: [&str; 14] = [
        "desktop", "app", "application", "client", "launcher", "main", "startup", "autostart", "agent", "helper", "tray",
        "service", "background", "electron",
    ];
    WORDS.contains(&word.to_lowercase().as_str())
}

/// `glean` becomes `Glean`; names with capitals of their own are kept.
fn capitalise(word: &str) -> String {
    if word.chars().any(|c| c.is_uppercase()) {
        return word.to_owned();
    }
    let mut c = word.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Drops the machine-made tails Windows and installers add to names: a user
/// SID (`Task-S-1-5-21-...`) or a long hex id (`AutoLaunch_8392B9AF...`).
pub fn clean_name(raw: &str) -> String {
    let mut s = raw.trim().to_owned();
    if let Some(at) = s.find("-S-1-5-") {
        s.truncate(at);
    }
    if let Some((head, tail)) = s.rsplit_once(['_', '-']) {
        if tail.len() >= 8 && tail.chars().all(|c| c.is_ascii_hexdigit()) {
            s = head.to_owned();
        }
    }
    s.trim().to_owned()
}

/// The name to show: the cleaned entry name, or the program's own
/// description when the entry name reads like an identifier.
pub fn pick_name(raw: &str, description: Option<String>) -> String {
    let name = clean_name(&display_name(raw));
    let machine_like = !name.contains(' ') && (name.len() >= 12 || name.contains('_') || is_generic(&name));
    match description {
        Some(d) if machine_like || name.is_empty() => d,
        _ => name,
    }
}

/// The first sentence of a service description, for the "what this is" line.
pub fn first_sentence(text: &str) -> String {
    let text = text.trim();
    let cut = text
        .char_indices()
        .find(|&(i, c)| c == '.' && text[i + 1..].starts_with(|n: char| n.is_whitespace()))
        .map(|(i, _)| i + 1)
        .unwrap_or(text.len());
    let s = &text[..cut];
    if s.chars().count() > 140 {
        let cut: String = s.chars().take(137).collect();
        format!("{}...", cut.trim_end())
    } else {
        s.to_owned()
    }
}

fn disabled_names(hive: Hive, sub: &str) -> Vec<String> {
    win::reg_values(hive, &format!(r"{APPROVED}\{sub}"), false)
        .into_iter()
        .filter_map(|(name, data)| match data {
            RegData::Binary(b) if approved_says_disabled(&b) => Some(name.to_lowercase()),
            _ => None,
        })
        .collect()
}

/// Everything due to start at sign-in, minus what Windows has marked off.
/// Needs `win::com_init` on the calling thread.
pub fn gather() -> Vec<Item> {
    let mut items = Vec::new();
    let off_user_run = disabled_names(Hive::User, "Run");
    let off_user_folder = disabled_names(Hive::User, "StartupFolder");
    let mut off_machine_run = disabled_names(Hive::Machine, "Run");
    off_machine_run.extend(disabled_names(Hive::Machine, "Run32"));
    off_machine_run.extend(off_user_run.iter().cloned());
    let off_machine_folder = disabled_names(Hive::Machine, "StartupFolder");

    let keys: [(Hive, &str, bool, bool); 6] = [
        (Hive::User, RUN, false, false),
        (Hive::User, RUN_ONCE, false, true),
        (Hive::Machine, RUN, false, false),
        (Hive::Machine, RUN, true, false),
        (Hive::Machine, RUN_ONCE, false, true),
        (Hive::Machine, RUN_ONCE, true, true),
    ];
    for (hive, path, wow32, once) in keys {
        let machine = hive == Hive::Machine;
        let off = if machine { &off_machine_run } else { &off_user_run };
        for (value, data) in win::reg_values(hive, path, wow32) {
            let RegData::Text(command) = data else { continue };
            if command.trim().is_empty() || (!once && off.contains(&value.to_lowercase())) {
                continue;
            }
            let id = format!(
                "{}:{}{}:{}",
                if once { "runonce" } else { "run" },
                if machine { "hklm" } else { "hkcu" },
                if wow32 { "32" } else { "" },
                value.to_lowercase()
            );
            if items.iter().any(|i: &Item| i.id == id) {
                continue;
            }
            let exe = exe_from_command(&command, win::expand_env);
            items.push(Item {
                id,
                name: pick_name(&value, exe.as_deref().and_then(win::file_description)),
                exe,
                source: Source::Run { machine, wow32, once, value },
                command,
                about: String::new(),
            });
        }
    }

    for common in [false, true] {
        let Some(dir) = win::startup_folder(common) else { continue };
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let off = if common { &off_machine_folder } else { &off_user_folder };
        for entry in entries.flatten() {
            let path = entry.path();
            let file = entry.file_name().to_string_lossy().into_owned();
            if file.eq_ignore_ascii_case("desktop.ini") || !path.is_file() || off.contains(&file.to_lowercase()) {
                continue;
            }
            let (command, exe) = if file.to_lowercase().ends_with(".lnk") {
                match win::resolve_shortcut(&path) {
                    Some((target, args)) => {
                        let exe = (!target.is_empty()).then(|| target.to_lowercase());
                        (format!("\"{target}\" {args}").trim().to_owned(), exe)
                    }
                    None => (path.to_string_lossy().into_owned(), None),
                }
            } else {
                let p = path.to_string_lossy().into_owned();
                let exe = p.to_lowercase().ends_with(".exe").then(|| p.to_lowercase());
                (p, exe)
            };
            items.push(Item {
                id: format!("folder:{}:{}", if common { "all" } else { "me" }, file.to_lowercase()),
                name: pick_name(&file, exe.as_deref().and_then(win::file_description)),
                source: Source::Folder { common, file: path.to_string_lossy().into_owned() },
                command,
                exe,
                about: String::new(),
            });
        }
    }

    for task in win::logon_tasks() {
        let exe = exe_from_command(&task.command, win::expand_env);
        let name = if task.path.eq_ignore_ascii_case(crate::autostart::TASK_NAME) {
            crate::brand::NAME.to_owned()
        } else {
            pick_name(&task.name, exe.as_deref().and_then(win::file_description))
        };
        items.push(Item {
            id: format!("task:{}", task.path.to_lowercase()),
            name,
            exe,
            source: Source::Task { path: task.path.clone() },
            command: task.command.clone(),
            about: String::new(),
        });
    }

    for svc in win::auto_services() {
        items.push(Item {
            id: format!("svc:{}", svc.name),
            name: svc.display.clone(),
            exe: None,
            source: Source::Service { name: svc.name.clone(), delayed: svc.delayed },
            command: svc.binary.clone(),
            about: first_sentence(&svc.description),
        });
    }
    items
}

/// Turns a per-user item off or back on with the same switch Task Manager
/// uses. False if the item is not per-user or the write failed.
pub fn set_enabled(item: &Item, enabled: bool) -> bool {
    let Some((key, name)) = item.approved_slot() else { return false };
    win::reg_set_binary(Hive::User, &key, &name, &approved_blob(enabled, win::now_ft()))
}

/// Starts a delayed item the way Windows would have: the command line for a
/// Run entry, the file itself for a Startup folder entry.
pub fn launch(item: &Item) -> bool {
    match &item.source {
        Source::Folder { file, .. } => win::shell_run(file, "", false, false, false).is_some(),
        _ => {
            let cmd = win::expand_env(item.command.trim());
            let (file, args) = split_command(&cmd);
            win::shell_run(&file, &args, false, false, false).is_some()
        }
    }
}

/// Splits a command line into program and arguments.
pub fn split_command(cmd: &str) -> (String, String) {
    let cmd = cmd.trim();
    if let Some(rest) = cmd.strip_prefix('"') {
        let mut parts = rest.splitn(2, '"');
        let file = parts.next().unwrap_or("").to_owned();
        let args = parts.next().unwrap_or("").trim().to_owned();
        return (file, args);
    }
    let lower = cmd.to_lowercase();
    if let Some(at) = lower.find(".exe") {
        return (cmd[..at + 4].to_owned(), cmd[at + 4..].trim().to_owned());
    }
    match cmd.split_once(' ') {
        Some((f, a)) => (f.to_owned(), a.trim().to_owned()),
        None => (cmd.to_owned(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(s: &str) -> String {
        s.replace("%ProgramFiles%", r"C:\Program Files")
    }

    #[test]
    fn finds_the_program_in_command_lines() {
        assert_eq!(
            exe_from_command(r#""C:\Program Files\App\app.exe" --minimized"#, no_env).as_deref(),
            Some(r"c:\program files\app\app.exe")
        );
        assert_eq!(
            exe_from_command(r"C:\Program Files\Tool\tool.exe /background", no_env).as_deref(),
            Some(r"c:\program files\tool\tool.exe")
        );
        assert_eq!(
            exe_from_command(r"%ProgramFiles%\X\x.EXE -s", no_env).as_deref(),
            Some(r"c:\program files\x\x.exe")
        );
        assert_eq!(exe_from_command("rundll32 shell32.dll,Foo", no_env).as_deref(), Some("rundll32"));
        assert_eq!(exe_from_command("   ", no_env), None);
    }

    #[test]
    fn splits_program_from_arguments() {
        assert_eq!(
            split_command(r#""C:\a b\c.exe" -x -y"#),
            (r"C:\a b\c.exe".to_owned(), "-x -y".to_owned())
        );
        assert_eq!(split_command(r"C:\a b\c.exe -x"), (r"C:\a b\c.exe".to_owned(), "-x".to_owned()));
        assert_eq!(split_command("notepad"), ("notepad".to_owned(), String::new()));
    }

    #[test]
    fn reads_and_writes_the_task_manager_switch() {
        assert!(!approved_says_disabled(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!approved_says_disabled(&[6, 0, 0, 0]));
        assert!(approved_says_disabled(&[3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]));
        assert!(!approved_says_disabled(&[]));
        let off = approved_blob(false, 0x0102_0304_0506_0708);
        assert!(approved_says_disabled(&off));
        assert_eq!(&off[4..], &[8, 7, 6, 5, 4, 3, 2, 1]);
        assert!(!approved_says_disabled(&approved_blob(true, 99)));
        assert_eq!(approved_blob(true, 99), [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn makes_readable_names() {
        assert_eq!(display_name("com.squirrel.Teams.Teams"), "Teams");
        assert_eq!(display_name("Send to OneNote.lnk"), "Send to OneNote");
        assert_eq!(display_name("OneDrive"), "OneDrive");
        assert_eq!(display_name("Steam Client 1.2"), "Steam Client 1.2");
    }

    #[test]
    fn cleans_machine_made_names() {
        assert_eq!(clean_name("OneDrive Startup Task-S-1-5-21-385602-1001"), "OneDrive Startup Task");
        assert_eq!(clean_name("MicrosoftEdgeAutoLaunch_8392B9AFEC7D06D7882DCFDFCDBDD4A0"), "MicrosoftEdgeAutoLaunch");
        assert_eq!(clean_name("Steam"), "Steam");
        assert_eq!(clean_name("Update-cafe"), "Update-cafe");
        assert_eq!(
            pick_name("MicrosoftEdgeAutoLaunch_8392B9AFEC7D06D7", Some("Microsoft Edge".into())),
            "Microsoft Edge"
        );
        assert_eq!(pick_name("SecurityHealth", Some("Windows Security notification icon".into())), "Windows Security notification icon");
        assert_eq!(pick_name("OneDrive", Some("Microsoft OneDrive".into())), "OneDrive");
        assert_eq!(pick_name("Docker Desktop", None), "Docker Desktop");
        assert_eq!(display_name("com.glean.desktop"), "Glean");
        assert_eq!(pick_name("com.glean.desktop", Some("Glean".into())), "Glean");
        assert_eq!(pick_name("desktop", Some("Glean".into())), "Glean");
        assert_eq!(display_name("com.squirrel.Teams.Teams"), "Teams");
    }

    #[test]
    fn keeps_the_first_sentence() {
        assert_eq!(first_sentence("Manages audio. Stops if you stop it."), "Manages audio.");
        assert_eq!(first_sentence("Version 1.2 of the thing"), "Version 1.2 of the thing");
        assert!(first_sentence(&"a".repeat(300)).ends_with("..."));
    }

    #[test]
    fn only_per_user_entries_can_be_turned_off() {
        let mut item = Item {
            id: "x".into(),
            name: "X".into(),
            source: Source::Run { machine: false, wow32: false, once: false, value: "X".into() },
            command: String::new(),
            exe: None,
            about: String::new(),
        };
        assert!(item.can_disable());
        item.source = Source::Run { machine: true, wow32: false, once: false, value: "X".into() };
        assert!(!item.can_disable());
        item.source = Source::Service { name: "x".into(), delayed: false };
        assert!(!item.can_disable());
        item.source = Source::Folder { common: false, file: r"C:\x\Y.lnk".into() };
        assert!(item.can_disable());
        assert_eq!(item.approved_slot().unwrap().1, "Y.lnk");
    }
}
