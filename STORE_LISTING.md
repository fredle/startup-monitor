# Windows Startup Monitor - Microsoft Store listing

## Product name

Windows Startup Monitor

## Short description

A live boot stopwatch for your PC: every startup app timed from sign-in, with a report on what slowed it down.

## Description

Windows Startup Monitor times your sign-in. From the moment your desktop appears it counts the programs, scheduled tasks and services that Windows is bringing up, marks each one as waiting, starting or done, and tells you when the machine has settled down enough to use.

During sign-in a compact panel sits in the corner of the screen with a running clock, a progress count ("8 of 13 started"), processor, memory and disk readings, and an estimate of the seconds remaining. The estimate is built only from your own previous sign-ins, so the first one shows a clock with no forecast. Once the PC is quiet the panel records the session and disappears; anything that has not appeared yet is followed from the notification area for up to ten minutes before the app exits.

Every sign-in is kept as a report you can open later:
- Seconds until usable, set against your typical figure over the last ten sign-ins
- A chart of every item from start to settled, with a marker at the point the PC became usable
- A ranking of the programs that took the most processor time and disk activity, and the ones that were slowest to finish

When the report points at a culprit, deal with it from the same row. Programs registered for your account can be switched off at sign-in (the same reversible switch Task Manager uses) or deferred, so Windows Startup Monitor launches them only after everything else has finished. A running program can also be closed straight from the list.

What is checked: Run and RunOnce registry entries, the personal and shared Startup folders, scheduled tasks triggered at logon, and automatic and delayed services. Services are shown for information only, each with a short plain-English description.

Two ways to run. The normal way needs no administrator rights: the app launches alongside your other startup programs and fills in what came before it from process start times and processor use, and it tells you that anything which ran and exited earlier cannot be shown. The optional early mode asks for administrator approval once, registers a sign-in task, and watches from the very first second.

Your timings stay on this PC. No account, no telemetry, no cloud lookups.

## Product features

- Running sign-in clock with a started count and time-remaining estimate
- Processor, memory and disk readings while the PC starts
- Waiting, starting and started status for every startup entry
- Per-sign-in report with your usual time for comparison
- Start-to-settled chart of every startup item
- Rankings by processor time, disk activity and time to settle
- Forecast based only on your own past sign-ins
- Switch a program off at sign-in, reversibly
- Defer a program until the PC is ready
- Close a running startup program from its row
- Covers registry Run entries, Startup folders, logon tasks and automatic services
- Read-only service list with short descriptions
- No administrator rights needed; optional early mode for the full picture
- Follows late starters from the notification area for up to ten minutes
- Exits by itself when startup is finished
- Shows its own startup cost alongside everything else
- Light and dark appearance matching your Windows setting

## Search terms

- boot timer
- startup speed
- login slow
- startup analyzer
- boot report
- startup manager
- sign-in timer

## Category and subcategory

Category: Utilities & tools
Subcategory: System utilities (or "Other" if not offered)

## Privacy

Windows Startup Monitor keeps two files in %LOCALAPPDATA%\StartupMonitor:
- settings.json holds your choices, including the programs you deferred or switched off.
- boots.json holds the timings of your 30 most recent sign-ins: program names, start times, processor time and disk use.

To open at sign-in it adds a "Windows Startup Monitor" value to your account's Run registry key, or, in early mode, a logon scheduled task. Either is removed when you switch the option off, and both are removed on uninstall.

The app connects to the internet for one thing only: if "Automatically update" is on, it checks GitHub Releases about once a day for a newer version and downloads it. It sends no usage data, program lists or personal details anywhere. There is no telemetry and no account.

## Screenshots

1. Watch the clock - a live count of what has started and an estimate of the seconds left.
2. Every item on one chart - start to settled, with the moment the PC became usable.
3. Spot the heavy hitters - programs ranked by processor time and by time to settle.
4. Fix it from the list - defer a program until the PC is ready, or switch it off.
