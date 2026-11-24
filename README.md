# Windows Startup Monitor

A live boot stopwatch for your PC: every startup app timed from sign-in, with
a report on what slowed it down.

After you sign in, Windows spends a minute or more launching startup apps and
services and shows you nothing about it. Windows Startup Monitor opens a small window at
sign-in with a count ("9 of 15 started"), CPU, memory and disk activity, and
the time left. Once the PC has settled it saves the boot's timings and closes.

## What it does

- Lists everything due to start at sign-in: the Run and RunOnce registry keys
  (this account and all users, 32 and 64-bit), both Startup folders, scheduled
  tasks with a sign-in trigger, and automatic services. Entries Windows has
  marked as off are left out.
- Shows each item as waiting, starting or started. Services report their own
  state. For apps, "waiting" means configured but no process seen yet.
- Calls the PC usable when every expected item has started (or stopped being
  waited for) and CPU and disk have stayed quiet for five seconds.
- After every boot: time to usable, a timeline of what started when, the apps
  that used the most processor time and took longest to settle, and a
  comparison with your last ten boots.
- Learns an estimate of the time left from earlier boots. The first watched
  boot shows no estimate rather than a guess.
- Per app: end it now, turn it off at sign-in, or hold it back until the PC is
  usable. Turning an app off uses the same switch as Task Manager's Startup
  apps page, so you can turn it back on there or in Windows Startup Monitor. Only entries
  for your own account can be turned off or held back. Services are shown
  read-only.

### Two ways to start

- **Standard** (default, no administrator rights): Windows Startup Monitor starts with your
  other startup apps and rebuilds what happened before it opened from each
  process's start time and processor time. Programs that started and closed
  before it opened are missing, and the timeline says so.
- **Start earlier** (Options): registers a sign-in scheduled task, which asks
  for administrator approval once. It runs at normal priority (tasks default
  to below normal) and raises itself to high priority, so the view starts in
  the first second of sign-in. In this mode you can also end an app as soon as
  it starts. Turning it off removes the task, with one more approval.

Keyboard: Ctrl+1, Ctrl+2 and Ctrl+3 switch between Startup items, Timeline and
What it cost. Esc closes Options or an open row.

## Install

- **Microsoft Store**: search for "Windows Startup Monitor".
- **GitHub Releases**: download the `Setup.exe` from the
  [releases page](https://github.com/fredle/startup-monitor/releases). It installs
  for your account only and needs no administrator rights.

Open Windows Startup Monitor once after installing. From then on it opens by itself at each
sign-in. To stop that, turn off "Watch every sign-in" in Options.

## Where data lives

Everything stays in `%LOCALAPPDATA%\StartupMonitor`:

- `settings.json`: your options, and the apps you held back or turned off.
- `boots.json`: timings of your last 30 boots.

Windows Startup Monitor also writes these, and removes them when you turn the matching
option off:

- A `Windows Startup Monitor` value under
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (standard mode).
- A `\StartupMonitor\Early start` scheduled task (Start earlier).
- `StartupApproved` values under
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer`, for apps you turn
  off or hold back.

## Privacy

- No telemetry, no accounts, no analytics.
- Process, service and startup information is read on your PC with Windows
  APIs and never leaves it.
- The only network use is the update check: about once a day, if "Automatically
  update" is on, Windows Startup Monitor asks GitHub Releases for a newer version and
  downloads it.

## Building

```
cargo build --release
cargo test --release
cargo run --release -- --shots target/shots            # screenshots, light
cargo run --release -- --shots target/shots --theme dark
```
