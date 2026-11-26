//! How the app itself starts at sign-in. Standard mode is a per-user Run
//! entry (no admin). Early mode is a logon scheduled task registered once
//! with administrator approval, set to normal priority because tasks default
//! to below-normal.

use crate::win::{self, Hive};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const VALUE: &str = crate::brand::NAME;
pub const TASK_NAME: &str = crate::brand::TASK_NAME;
pub const COLLECT_ARG: &str = "--collect";
pub const EARLY_ARG: &str = "--early";

fn exe() -> String {
    std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn run_command(exe: &str) -> String {
    format!("\"{exe}\" {COLLECT_ARG}")
}

/// Adds or removes the standard-mode Run entry.
pub fn set_standard(on: bool) -> bool {
    if on {
        win::reg_set_text(Hive::User, RUN, VALUE, &run_command(&exe()))
    } else {
        win::reg_get_text(Hive::User, RUN, VALUE).is_none() || win::reg_delete(Hive::User, RUN, VALUE)
    }
}

/// Moves a Run entry filed under a name the app used to have to its current
/// name, keeping the command. Run at every start, before anything reads the
/// entry, so a renamed app neither starts twice nor leaves the old one behind.
pub fn adopt_old_names() {
    for old in crate::brand::OLD_NAMES {
        let Some(command) = win::reg_get_text(Hive::User, RUN, old) else { continue };
        if !standard_is_set() {
            win::reg_set_text(Hive::User, RUN, VALUE, &command);
        }
        win::reg_delete(Hive::User, RUN, old);
    }
}

/// The Run entry's command line as it stands, for diagnostics.
pub fn standard_command() -> Option<String> {
    win::reg_get_text(Hive::User, RUN, VALUE)
}

pub fn standard_is_set() -> bool {
    win::reg_get_text(Hive::User, RUN, VALUE).is_some()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The early-mode task: at this user's sign-in, run the collector at normal
/// priority (4; tasks default to 7). With `highest`, it runs with the
/// highest rights the user has, which for an administrator means elevated;
/// only an administrator can register that. Without, it runs with the
/// user's normal rights, which anyone can register for themselves.
pub fn early_task_xml(exe: &str, user: &str, highest: bool) -> String {
    let level = if highest { "HighestAvailable" } else { "LeastPrivilege" };
    let exe = xml_escape(exe);
    let user = xml_escape(user);
    let name = xml_escape(crate::brand::NAME);
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>{name}: watches startup from the first second of sign-in, then closes.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>{level}</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <ExecutionTimeLimit>PT15M</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>{COLLECT_ARG} {EARLY_ARG}</Arguments>
    </Exec>
  </Actions>
</Task>
"#
    )
}

fn user_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => format!("{d}\\{user}"),
        _ => user,
    }
}

/// Does turning on early mode need an administrator prompt? Only for an
/// administrator's account, whose task runs elevated so the collector sees
/// elevated apps too. Anyone else registers it with their own rights.
pub fn early_needs_admin() -> bool {
    win::user_is_admin()
}

/// Registers the early-mode task: quietly for a standard account, with one
/// administrator prompt for an administrator's (or if a quiet attempt is
/// refused, as some company policies do). Returns a message for the user
/// on failure.
pub fn enable_early() -> Result<(), String> {
    let admin = early_needs_admin();
    let code = if admin { None } else { register_early(false, false)? };
    let code = match code {
        Some(0) => Some(0),
        _ => register_early(true, true)?,
    };
    match code {
        Some(0) => {
            set_standard(false);
            Ok(())
        }
        Some(_) => Err("Windows did not accept the sign-in task. Nothing was changed.".into()),
        None => Err("Administrator approval was not given. Nothing was changed.".into()),
    }
}

/// Writes the task file and runs `schtasks` on it; `elevate` asks for
/// administrator approval. The exit code, or None if approval was refused.
fn register_early(highest: bool, elevate: bool) -> Result<Option<u32>, String> {
    let dir = crate::settings::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("early-task.xml");
    let xml = early_task_xml(&exe(), &user_name(), highest);
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(xml.encode_utf16().flat_map(|c| c.to_le_bytes()));
    std::fs::write(&path, bytes).map_err(|e| format!("Could not write the task file: {e}"))?;
    let args = format!("/Create /TN \"{TASK_NAME}\" /XML \"{}\" /F", path.display());
    let code = win::shell_run("schtasks.exe", &args, elevate, true, true);
    let _ = std::fs::remove_file(&path);
    Ok(code)
}

/// Removes the early-mode task and goes back to the standard Run entry. A
/// task the user registered goes quietly; one made with administrator
/// rights needs one prompt.
pub fn disable_early() -> Result<(), String> {
    let args = format!("/Delete /TN \"{TASK_NAME}\" /F");
    let quiet = win::shell_run("schtasks.exe", &args, false, true, true);
    let result = if quiet == Some(0) { quiet } else { win::shell_run("schtasks.exe", &args, true, true, true) };
    match result {
        Some(_) => {
            set_standard(true);
            Ok(())
        }
        None => Err("Administrator approval was not given, so the sign-in task is still there.".into()),
    }
}

/// Run on uninstall: undoes everything that would outlive the app. Held-back
/// apps start with Windows again (nothing would start them otherwise), and
/// the Run entry and the early task go. Apps the user turned off stay off:
/// that was their choice, and Task Manager can turn them back on.
pub fn remove_all() {
    let settings = crate::settings::load();
    for item in &settings.delayed {
        crate::startup::set_enabled(item, true);
    }
    adopt_old_names();
    set_standard(false);
    if settings.early {
        // A task made with administrator rights refuses a quiet delete; ask
        // once only then.
        let args = format!("/Delete /TN \"{TASK_NAME}\" /F");
        if win::shell_run("schtasks.exe", &args, false, true, true) != Some(0) {
            win::shell_run("schtasks.exe", &args, true, true, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_runs_the_collector_at_normal_priority() {
        let xml = early_task_xml(r"C:\Users\a&b\App\current\app.exe", r"PC\a&b", true);
        assert!(xml.contains("<Priority>4</Priority>"));
        assert!(xml.contains("<LogonTrigger>"));
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        let plain = early_task_xml(r"C:\App\app.exe", r"PC\a", false);
        assert!(plain.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(xml.contains(r"<Command>C:\Users\a&amp;b\App\current\app.exe</Command>"));
        assert!(xml.contains("<Arguments>--collect --early</Arguments>"));
        assert!(xml.contains(r"<UserId>PC\a&amp;b</UserId>"));
    }

    #[test]
    fn run_entry_quotes_the_path() {
        assert_eq!(run_command(r"C:\x y\r.exe"), r#""C:\x y\r.exe" --collect"#);
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Registers the real early task without administrator rights, checks
    /// it, and removes it again without them: run with --ignored on a PC
    /// where early mode is off.
    #[test]
    #[ignore]
    fn live_early_task_without_admin() {
        let query = format!("/Query /TN \"{TASK_NAME}\"");
        assert_ne!(win::shell_run("schtasks.exe", &query, false, true, true), Some(0), "early mode is on here: not touching it");
        assert_eq!(register_early(false, false).unwrap(), Some(0), "quiet registration refused");
        assert_eq!(win::shell_run("schtasks.exe", &query, false, true, true), Some(0), "task not there");
        let delete = format!("/Delete /TN \"{TASK_NAME}\" /F");
        assert_eq!(win::shell_run("schtasks.exe", &delete, false, true, true), Some(0), "quiet delete refused");
        assert_ne!(win::shell_run("schtasks.exe", &query, false, true, true), Some(0), "task still there");
    }
}
