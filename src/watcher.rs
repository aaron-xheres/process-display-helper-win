use crate::config::{Config, WatchEntry};
use crate::monitor::{
    DisplaySnapshot, current_primary_snapshot, restore_display_snapshot, set_monitor_mode,
    set_primary_monitor,
};
use crate::process_monitor::ProcessEvent;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::time::Instant;

#[derive(Debug)]
struct ActiveWatch {
    pids: Vec<u32>,
    first_activated_at: Instant,
}

#[derive(Debug, Default)]
pub struct WatchState {
    active: HashMap<String, ActiveWatch>,
    current_winner: Option<String>,
    baseline: Option<DisplaySnapshot>,
}

pub fn handle_process_event(
    event: ProcessEvent,
    config: &Config,
    state: &mut WatchState,
) -> Result<()> {
    match event {
        ProcessEvent::Started { name, pid } => handle_started(&name, pid, config, state),
        ProcessEvent::Exited { pid } => handle_exited(pid, config, state),
    }
}

pub fn reload_config(config: &Config, state: &mut WatchState) -> Result<()> {
    let previous_winner = state.current_winner.clone();
    let previous_active_count = state.active.len();

    state
        .active
        .retain(|process_name, _| find_watch_entry(config, process_name).is_some());

    let removed = previous_active_count.saturating_sub(state.active.len());
    if removed > 0 {
        tracing::info!(
            removed_entries = removed,
            "dropped active watches missing in reloaded config"
        );
    }

    let next_winner = find_winner_key(config, state);
    if let Some(winner_key) = next_winner.as_ref() {
        let Some(watch_entry) = find_watch_entry(config, winner_key) else {
            tracing::error!(process = %winner_key, "winner watch entry missing from reloaded config");
            return Ok(());
        };

        let primary_monitor = effective_primary_monitor(watch_entry);

        if let Err(error) = apply_watch_display_plan(watch_entry) {
            tracing::error!(
                process = %winner_key,
                primary_monitor = ?primary_monitor,
                display_count = watch_entry.display.len(),
                error = %error,
                "failed to apply winner display after config reload"
            );
            return Ok(());
        }

        tracing::info!(
            process = %winner_key,
            priority = watch_entry.priority,
            primary_monitor = ?primary_monitor,
            display_count = watch_entry.display.len(),
            "applied winner display configuration after config reload"
        );

        state.current_winner = next_winner;
        return Ok(());
    }

    if previous_winner.is_some() {
        if let Some(snapshot) = state.baseline.as_ref() {
            tracing::info!(
                restore_reason = "config-reload",
                target_monitor = snapshot.primary,
                primary_device = %snapshot.primary_device_name,
                width = snapshot.resolution.0,
                height = snapshot.resolution.1,
                refresh_hz = snapshot.refresh_rate,
                flip_orientation = snapshot.flip_orientation,
                display_orientation = snapshot.display_orientation.0,
                saved_monitor_modes = snapshot.monitor_modes.len(),
                "restoring baseline display snapshot"
            );
            if let Err(error) = restore_display_snapshot(snapshot) {
                tracing::error!(error = %error, "failed to restore baseline display after config reload");
            }
        }
    }

    state.current_winner = None;
    state.baseline = None;
    Ok(())
}

fn handle_started(name: &str, pid: u32, config: &Config, state: &mut WatchState) -> Result<()> {
    let key = normalize_process_name(name);
    if find_watch_entry(config, &key).is_none() {
        return Ok(());
    }

    let was_empty = state.active.is_empty();
    let mut inserted_new = false;

    match state.active.get_mut(&key) {
        Some(active) => {
            if !active.pids.contains(&pid) {
                active.pids.push(pid);
                tracing::debug!(process = %key, pid, "additional process instance detected");
            }
        }
        None => {
            state.active.insert(
                key.clone(),
                ActiveWatch {
                    pids: vec![pid],
                    first_activated_at: Instant::now(),
                },
            );
            inserted_new = true;
            tracing::info!(process = %key, pid, "watched process activated");
        }
    }

    if !inserted_new {
        return Ok(());
    }

    if was_empty && state.baseline.is_none() {
        let snapshot = current_primary_snapshot()?;
        tracing::info!(
            target_monitor = snapshot.primary,
            primary_device = %snapshot.primary_device_name,
            width = snapshot.resolution.0,
            height = snapshot.resolution.1,
            refresh_hz = snapshot.refresh_rate,
            flip_orientation = snapshot.flip_orientation,
            display_orientation = snapshot.display_orientation.0,
            saved_monitor_modes = snapshot.monitor_modes.len(),
            "captured baseline display snapshot"
        );
        state.baseline = Some(snapshot);
    }

    apply_winner_if_changed(config, state)
}

fn handle_exited(pid: u32, config: &Config, state: &mut WatchState) -> Result<()> {
    let key = state
        .active
        .iter()
        .find(|(_, active)| active.pids.contains(&pid))
        .map(|(name, _)| name.clone());

    let Some(key) = key else {
        return Ok(());
    };

    if let Some(active) = state.active.get_mut(&key) {
        active.pids.retain(|entry_pid| *entry_pid != pid);
        if !active.pids.is_empty() {
            tracing::debug!(process = %key, pid, remaining = active.pids.len(), "process instance exited");
            return Ok(());
        }
    }

    let should_restore = find_watch_entry(config, &key)
        .map(|entry| entry.restore_on_exit)
        .unwrap_or(false);

    state.active.remove(&key);
    tracing::info!(process = %key, pid, "final process instance exited");

    if state.active.is_empty() {
        if should_restore {
            if let Some(snapshot) = state.baseline.as_ref() {
                tracing::info!(
                    restore_reason = "last-process-exit",
                    target_monitor = snapshot.primary,
                    primary_device = %snapshot.primary_device_name,
                    width = snapshot.resolution.0,
                    height = snapshot.resolution.1,
                    refresh_hz = snapshot.refresh_rate,
                    flip_orientation = snapshot.flip_orientation,
                    display_orientation = snapshot.display_orientation.0,
                    saved_monitor_modes = snapshot.monitor_modes.len(),
                    "restoring baseline display snapshot"
                );
                if let Err(error) = restore_display_snapshot(snapshot) {
                    tracing::error!(error = %error, "failed to restore baseline display state");
                }
            }
        }

        state.current_winner = None;
        state.baseline = None;
        return Ok(());
    }

    apply_winner_if_changed(config, state)
}

fn apply_winner_if_changed(config: &Config, state: &mut WatchState) -> Result<()> {
    let next_winner = find_winner_key(config, state);
    if next_winner == state.current_winner {
        return Ok(());
    }

    if let Some(winner_key) = next_winner.as_ref() {
        let Some(watch_entry) = find_watch_entry(config, winner_key) else {
            tracing::error!(process = %winner_key, "winner watch entry missing from config");
            return Ok(());
        };

        let primary_monitor = effective_primary_monitor(watch_entry);

        if let Err(error) = apply_watch_display_plan(watch_entry) {
            tracing::error!(
                process = %winner_key,
                primary_monitor = ?primary_monitor,
                display_count = watch_entry.display.len(),
                error = %error,
                "failed to apply winner display configuration; skipping switch"
            );
            return Ok(());
        }

        tracing::info!(
            process = %winner_key,
            priority = watch_entry.priority,
            primary_monitor = ?primary_monitor,
            display_count = watch_entry.display.len(),
            "display control moved to winner"
        );
    }

    state.current_winner = next_winner;
    Ok(())
}

fn effective_primary_monitor(watch_entry: &WatchEntry) -> Option<u8> {
    watch_entry
        .display
        .iter()
        .rev()
        .find(|display| display.set_primary)
        .map(|display| display.monitor)
}

fn apply_watch_display_plan(watch_entry: &WatchEntry) -> Result<()> {
    let effective_primary = watch_entry
        .display
        .iter()
        .enumerate()
        .rev()
        .find(|(_, display)| display.set_primary);
    let effective_primary_monitor = effective_primary.map(|(_, display)| display.monitor);

    for display in &watch_entry.display {
        if Some(display.monitor) == effective_primary_monitor {
            continue;
        }

        set_monitor_mode(
            display.monitor,
            display.resolution,
            display.refresh_rate,
            display.flip_orientation,
        )
        .with_context(|| {
            format!(
                "failed applying monitor {} mode for process {}",
                display.monitor, watch_entry.process_name
            )
        })?;
    }

    if let Some((_, display)) = effective_primary {
        set_primary_monitor(
            display.monitor,
            display.resolution,
            display.refresh_rate,
            display.flip_orientation,
        )
        .with_context(|| {
            format!(
                "failed applying primary monitor {} for process {}",
                display.monitor, watch_entry.process_name
            )
        })?;
    }

    Ok(())
}

fn find_winner_key(config: &Config, state: &WatchState) -> Option<String> {
    state
        .active
        .iter()
        .filter_map(|(key, active)| {
            let watch = find_watch_entry(config, key)?;
            Some((key.clone(), watch.priority, active.first_activated_at))
        })
        .max_by(|left, right| left.1.cmp(&right.1).then(left.2.cmp(&right.2)))
        .map(|entry| entry.0)
}

fn find_watch_entry<'a>(config: &'a Config, normalized_name: &str) -> Option<&'a WatchEntry> {
    config
        .watch
        .iter()
        .find(|entry| normalize_process_name(&entry.process_name) == normalized_name)
}

fn normalize_process_name(name: &str) -> String {
    let basename = std::path::Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(name);

    basename.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DisplayEntry;
    use std::time::Duration;

    fn display_entry(monitor: u8, set_primary: bool) -> DisplayEntry {
        DisplayEntry {
            monitor,
            set_primary,
            resolution: None,
            refresh_rate: None,
            flip_orientation: false,
        }
    }

    fn watch_entry(process_name: &str, priority: u8) -> WatchEntry {
        WatchEntry {
            process_name: process_name.to_string(),
            restore_on_exit: true,
            priority,
            display: vec![display_entry(1, true)],
        }
    }

    #[test]
    fn effective_primary_monitor_uses_last_set_primary_entry() {
        let entry = WatchEntry {
            process_name: "test.exe".to_string(),
            restore_on_exit: true,
            priority: 0,
            display: vec![
                display_entry(1, true),
                display_entry(2, false),
                display_entry(3, true),
            ],
        };

        assert_eq!(effective_primary_monitor(&entry), Some(3));
    }

    #[test]
    fn normalize_process_name_extracts_filename_and_lowercases() {
        let normalized = normalize_process_name(r"C:\Program Files\App\ALACRITTY.EXE");
        assert_eq!(normalized, "alacritty.exe");
    }

    #[test]
    fn find_watch_entry_matches_case_insensitively() {
        let config = Config {
            watch: vec![watch_entry("ALACRITTY.EXE", 0)],
        };

        let matched = find_watch_entry(&config, "alacritty.exe");
        assert!(matched.is_some());
    }

    #[test]
    fn winner_prefers_higher_priority() {
        let config = Config {
            watch: vec![watch_entry("a.exe", 1), watch_entry("b.exe", 5)],
        };

        let mut state = WatchState::default();
        let now = Instant::now();
        state.active.insert(
            "a.exe".to_string(),
            ActiveWatch {
                pids: vec![1],
                first_activated_at: now,
            },
        );
        state.active.insert(
            "b.exe".to_string(),
            ActiveWatch {
                pids: vec![2],
                first_activated_at: now,
            },
        );

        let winner = find_winner_key(&config, &state);
        assert_eq!(winner.as_deref(), Some("b.exe"));
    }

    #[test]
    fn winner_uses_newest_activation_for_priority_ties() {
        let config = Config {
            watch: vec![watch_entry("a.exe", 3), watch_entry("b.exe", 3)],
        };

        let mut state = WatchState::default();
        let now = Instant::now();
        state.active.insert(
            "a.exe".to_string(),
            ActiveWatch {
                pids: vec![1],
                first_activated_at: now,
            },
        );
        state.active.insert(
            "b.exe".to_string(),
            ActiveWatch {
                pids: vec![2],
                first_activated_at: now + Duration::from_millis(1),
            },
        );

        let winner = find_winner_key(&config, &state);
        assert_eq!(winner.as_deref(), Some("b.exe"));
    }
}
