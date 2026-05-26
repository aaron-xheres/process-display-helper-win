# process-display-helper-win

Windows tray helper that watches processes and applies monitor layout rules.

## Requirements

Windows, x86-64

## What it does

- Runs silently in the system tray (single instance enforced)
- Monitors running processes by name
- When a watched process starts, applies a configured display layout: set primary monitor, change resolution, refresh rate, and/or flip orientation
- Moves the process's windows to the target monitor
- When the process exits, optionally restores the previous display state
- Supports multiple watch rules with priority-based conflict resolution (highest-priority active process wins)
- Hot-reloads `config.toml` without restarting
- Auto-generates a config template on first run

## Config location

Config is loaded from config.toml in the same folder as the executable (for local dev this is usually target/debug/config.toml). If the file is missing, a template is created automatically.

## Config reference

Each process rule uses [[watch]]:

- process_name (required)
- restore_on_exit (bool)
- priority (higher wins)

Each rule must include one or more [[watch.display]] entries:

- monitor (required)
- set_primary (bool)
- move_to_monitor (bool)
- resolution = [w, h]
- refresh_rate (Hz)
- flip_orientation (bool)

For schema and validation details, see src/config.rs.
