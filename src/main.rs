//! Windows Startup Monitor - A live boot stopwatch for your PC: every startup app timed from sign-in, with a report on what slowed it down.
//!
//!   startup_monitor.exe                 open the app
//!   startup_monitor.exe --auto-update   internal: background update check/download
//!   startup_monitor.exe --shots <dir>   write screenshots of made-up scenes, then exit
//!                                 (add --theme dark for the dark look)

#![windows_subsystem = "windows"]

mod app;
mod autostart;
mod diag;
mod history;
mod monitor;
mod settings;
mod startup;
mod win;
mod shots;
mod theme;
mod tray;
mod update;

fn main() {
    // First thing, always: Velopack may need to run an install/uninstall/
    // update hook and exit before any of our own argument handling runs.
    update::run_app_hooks();

    let args: Vec<String> = std::env::args().skip(1).collect();

    // The detached child spawned below; must not spawn another.
    if args.first().map(String::as_str) == Some("--auto-update") {
        update::run_background_update();
        return;
    }
    if !args.iter().any(|a| a == "--shots") {
        update::maybe_spawn_background_update();
    }

    if let Err(e) = app::run(args) {
        eprintln!("{e}");
    }
}
