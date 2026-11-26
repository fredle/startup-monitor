//! What makes this app Startup Monitor rather than its twin, Ready
//! Yet. The twins share every source file except this one and `theme.rs`
//! (the skin). A change to any other file must be made in both:
//! app-factory's `tools/sync-twins.ps1` copies and checks them.
//!
//! Never change the ids once released: installs, the sign-in entry and the
//! saved history depend on them.

use windows::core::{PCWSTR, w};

/// The name people see: window title, tray, messages.
pub const NAME: &str = "Startup Monitor";
/// Names this app has had. The sign-in entry was filed under them, so the
/// app moves it to the current name when it starts.
pub const OLD_NAMES: &[&str] = &["Windows Startup Monitor"];
/// Folder under %LOCALAPPDATA% for settings and history.
pub const ID: &str = "StartupMonitor";
/// The early-start scheduled task.
pub const TASK_NAME: &str = r"\StartupMonitor\Early start";
/// One collector per session.
pub const COLLECTOR_MUTEX: &str = r"Local\StartupMonitorCollector";
/// The tray icon's hidden window class.
pub const TRAY_CLASS: PCWSTR = w!("StartupMonitorTrayWindow");
