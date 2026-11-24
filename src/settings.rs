//! Small persisted preferences in %LOCALAPPDATA%\StartupMonitor\settings.json.
//! Add app-specific fields here, always with `#[serde(default)]` so older
//! settings files keep loading.

use serde::{Deserialize, Serialize};

use crate::startup::Item;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Settings {
    /// Check for updates in the background and install them silently.
    #[serde(default = "default_true")]
    pub auto_update: bool,
    /// Unix seconds of the last background update check, to throttle to once
    /// a day.
    #[serde(default)]
    pub last_update_check: u64,
    /// Watch every sign-in (standard mode Run entry or early-mode task).
    #[serde(default = "default_true")]
    pub watch: bool,
    /// Early mode: the logon task is registered.
    #[serde(default)]
    pub early: bool,
    /// Set once the Run entry has been written on first launch.
    #[serde(default)]
    pub set_up: bool,
    /// The first-launch question about starting earlier has been answered.
    #[serde(default)]
    pub asked_early: bool,
    /// Items Windows Startup Monitor holds back and starts once the PC is usable.
    #[serde(default)]
    pub delayed: Vec<Item>,
    /// Items turned off from Windows Startup Monitor, so they can be turned back on.
    #[serde(default)]
    pub turned_off: Vec<Item>,
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_update: true,
            last_update_check: 0,
            watch: true,
            early: false,
            set_up: false,
            asked_early: false,
            delayed: Vec::new(),
            turned_off: Vec::new(),
        }
    }
}

pub fn data_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into()))
        .join("StartupMonitor")
}

fn settings_path() -> std::path::PathBuf {
    data_dir().join("settings.json")
}

pub fn load() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(settings: &Settings) {
    let _ = std::fs::create_dir_all(data_dir());
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(settings_path(), json);
    }
}
