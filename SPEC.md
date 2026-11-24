# Windows Startup Monitor

Watch your PC start up: what has launched, what is still to come, and how long until it is usable.

## Problem

After sign-in a PC spends a minute or more launching startup apps and services, and Windows shows nothing about it. Task Manager's Startup tab and Autoruns list what is configured, not what has started, what is still to come or when the machine will settle. People wait, guess, and do not know which program is costing them the time. Soluto showed this live around 2010 and was popular; nothing current does.

## Features (1.0)

- Builds the list of everything due to start at sign-in: Run and RunOnce registry keys (per-user and machine, 32 and 64-bit), both Startup folders, scheduled tasks with a logon trigger, and automatic and delayed-automatic services, leaving out entries Windows has marked disabled
- Live status for each item: started, starting or waiting. Services report their own state; for apps, 'waiting' means configured with no process seen yet, and the UI says so
- One app, two modes. Standard mode (default, no admin) starts with the other startup apps and reconstructs the boot so far from each running process's and service's start time, CPU time and disk I/O, then watches the rest live. Early mode is an opt-in 'Start earlier' setting that asks for one admin prompt and gives the live view from the first second of sign-in. Switching it off removes the task
- Post-mortem after every boot in both modes: time to usable, a timeline of what started when, the slowest and most expensive items ('Teams cost you 11 s'), and a comparison with the last ten boots
- Standard mode says what it cannot see: programs that started and exited before it launched are missing from the timeline
- Small always-on-top overlay shown at sign-in with a progress count ('12 of 31'), CPU, memory and disk activity, and the time left until the PC is usable
- 'Usable' is defined as every expected item started or timed out and CPU and disk settled below a threshold for a few seconds
- ETA learned from history: per-item start times and total time-to-usable are saved each boot. The first boot shows no ETA and says why, never a guess
- Per-item actions: end it now, disable it for future boots, or delay it until the PC is usable. Disabling a per-user entry uses the same reversible switch as Task Manager
- Services are listed read-only in 1.0, each with a plain 'what this is' line
- Early mode registers a logon scheduled task with normal task priority set explicitly, since tasks default to below-normal, and the process raises itself to high priority. Ending an item before it launches is only possible in this mode
- Lightweight by design: direct Win32 process and service queries polled about once a second, no WMI, PowerShell or webview. The collector starts headless and the full window opens only on click
- Exits once the PC is usable, after writing the boot's timings, so it costs nothing for the rest of the session
- Lists itself among the startup items with its own measured cost
- Targets: under 10 MB of memory and under 1% CPU while watching, overlay visible within a second of the desktop appearing

## Non-goals

- No stopping, disabling or delaying of services or machine-wide entries in 1.0
- No deleting of startup entries or files
- No alerts when a program adds itself to startup, which Startup Watch does
- No measurement of the time before sign-in (firmware, kernel and driver boot)
- No general process manager or all-day resource monitor
- No malware verdicts or cloud lookups

## Evidence of demand

