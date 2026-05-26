# process-display-helper-win

Windows tray helper that watches processes and applies monitor layout rules.

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
