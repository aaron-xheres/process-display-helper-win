use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const CONFIG_TEMPLATE: &str = r#"# process-display-helper configuration
#
# Add one [[watch]] section per process to watch.
# Higher priority values take precedence.
# If two watched processes have the same priority, the newest one wins.
#
# [[watch]]
# process_name = "game.exe"
# restore_on_exit = true
# priority = 10
#
# [[watch.display]]
# monitor = 1
# set_primary = false
# move_to_monitor = false
# resolution = [1920, 1080]
# refresh_rate = 60
# flip_orientation = false
#
# [[watch.display]]
# monitor = 2
# set_primary = true
# move_to_monitor = true
# resolution = [1080, 1920]
# refresh_rate = 165
# flip_orientation = false
"#;

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Config {
    #[serde(default)]
    pub run_on_startup: bool,
    #[serde(default)]
    pub watch: Vec<WatchEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WatchEntry {
    pub process_name: String,
    #[serde(default)]
    pub restore_on_exit: bool,
    #[serde(default)]
    pub priority: u8,
    #[serde(default)]
    pub display: Vec<DisplayEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DisplayEntry {
    pub monitor: u8,
    #[serde(default)]
    pub set_primary: bool,
    #[serde(default)]
    pub move_to_monitor: bool,
    pub resolution: Option<[u16; 2]>,
    pub refresh_rate: Option<u16>,
    #[serde(default)]
    pub flip_orientation: bool,
}

pub fn load_config(exe_dir: &Path) -> Result<Config> {
    let config_path = config_path(exe_dir);
    if !config_path.exists() {
        fs::write(&config_path, CONFIG_TEMPLATE).with_context(|| {
            format!(
                "failed to create config template at {}",
                config_path.display()
            )
        })?;
        tracing::info!(path = %config_path.display(), "created config template");
        return Ok(Config::default());
    }

    let raw = fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config file at {}", config_path.display()))?;
    parse_config_contents(&raw, &config_path)
}

pub fn check_config_file(exe_dir: &Path) -> Result<Config> {
    let config_path = config_path(exe_dir);
    let raw = fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config file at {}", config_path.display()))?;
    parse_config_contents(&raw, &config_path)
}

pub fn set_run_on_startup(exe_dir: &Path, enabled: bool) -> Result<Config> {
    load_config(exe_dir)?;
    let config_path = config_path(exe_dir);
    let raw = fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config file at {}", config_path.display()))?;

    let updated_raw = rewrite_run_on_startup_entry(&raw, enabled);
    let updated = parse_config_contents(&updated_raw, &config_path).with_context(|| {
        format!(
            "failed to validate updated config file at {}",
            config_path.display()
        )
    })?;

    fs::write(&config_path, updated_raw)
        .with_context(|| format!("failed to write config file at {}", config_path.display()))?;

    Ok(updated)
}

fn rewrite_run_on_startup_entry(raw: &str, enabled: bool) -> String {
    let newline = newline_for(raw);
    let mut lines: Vec<String> = raw.lines().map(ToOwned::to_owned).collect();

    lines.retain(|line| !is_run_on_startup_assignment(line));

    while !lines.is_empty() && lines[0].trim().is_empty() {
        lines.remove(0);
    }

    let top_comment_line_count = lines
        .iter()
        .take_while(|line| line.trim_start().starts_with('#'))
        .count();

    let insert_idx = if top_comment_line_count > 0 {
        let idx = top_comment_line_count;
        while idx < lines.len() && lines[idx].trim().is_empty() {
            lines.remove(idx);
        }

        lines.insert(idx, String::new());
        idx + 1
    } else {
        0
    };

    lines.insert(insert_idx, format!("run_on_startup = {enabled}"));

    let after_run_idx = insert_idx + 1;
    while after_run_idx < lines.len() && lines[after_run_idx].trim().is_empty() {
        lines.remove(after_run_idx);
    }

    if after_run_idx < lines.len() {
        lines.insert(after_run_idx, String::new());
    }

    let mut rewritten = lines.join(newline);
    if raw.ends_with('\n') {
        rewritten.push_str(newline);
    }

    rewritten
}

fn is_run_on_startup_assignment(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix("run_on_startup") else {
        return false;
    };

    rest.trim_start().starts_with('=')
}

fn newline_for(raw: &str) -> &'static str {
    if raw.contains("\r\n") { "\r\n" } else { "\n" }
}

fn parse_config_contents(raw: &str, config_path: &Path) -> Result<Config> {
    if raw.trim().is_empty() {
        tracing::warn!(path = %config_path.display(), "config file is empty; using no watch entries");
        return Ok(Config::default());
    }

    let parsed: Config = toml::from_str(raw)
        .with_context(|| format!("failed to parse config file at {}", config_path.display()))?;

    validate_config(&parsed)?;

    Ok(parsed)
}

fn validate_config(config: &Config) -> Result<()> {
    for watch in &config.watch {
        if watch.display.is_empty() {
            bail!(
                "watch entry '{}' must define at least one [[watch.display]] section",
                watch.process_name
            );
        }
    }

    Ok(())
}

pub fn config_path(exe_dir: &Path) -> PathBuf {
    exe_dir.join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn watch_display_is_required() {
        let raw = r#"
[[watch]]
process_name = "game.exe"
"#;

        let parsed: Config = toml::from_str(raw).expect("config should deserialize for validation");
        let error = validate_config(&parsed).expect_err("validation should reject missing display");

        assert!(
            error
                .to_string()
                .contains("must define at least one [[watch.display]] section")
        );
    }

    #[test]
    fn display_move_to_monitor_defaults_false() {
        let raw = r#"
[[watch]]
process_name = "game.exe"

[[watch.display]]
monitor = 1
set_primary = true
"#;

        let parsed: Config = toml::from_str(raw).expect("config should deserialize");
        assert!(!parsed.watch[0].display[0].move_to_monitor);
    }

    #[test]
    fn run_on_startup_defaults_false_when_missing() {
        let raw = r#"
[[watch]]
process_name = "game.exe"

[[watch.display]]
monitor = 1
"#;

        let parsed: Config = toml::from_str(raw).expect("config should deserialize");
        assert!(!parsed.run_on_startup);
    }

    #[test]
    fn run_on_startup_can_be_enabled() {
        let raw = r#"
run_on_startup = true
"#;

        let parsed: Config = toml::from_str(raw).expect("config should deserialize");
        assert!(parsed.run_on_startup);
    }

    #[test]
    fn rewrite_run_on_startup_places_entry_after_top_comments() {
        let raw = r#"# Test comment
# Additional context

[[watch]]
process_name = "game.exe"

[[watch.display]]
monitor = 1
"#;

        let rewritten = rewrite_run_on_startup_entry(raw, true);
        let expected = r#"# Test comment
# Additional context

run_on_startup = true

[[watch]]
process_name = "game.exe"

[[watch.display]]
monitor = 1
"#;

        assert_eq!(rewritten, expected);
    }

    #[test]
    fn rewrite_run_on_startup_removes_existing_entry_and_repositions() {
        let raw = r#"# Test comment

[[watch]]
process_name = "game.exe"

[[watch.display]]
monitor = 1

run_on_startup = false
"#;

        let rewritten = rewrite_run_on_startup_entry(raw, true);

        assert_eq!(rewritten.matches("run_on_startup = true").count(), 1);
        assert!(!rewritten.contains("run_on_startup = false"));
        assert!(rewritten.starts_with("# Test comment\n\nrun_on_startup = true\n\n"));
    }

    #[test]
    fn legacy_monitor_device_field_is_ignored() {
        let raw = r#"
[[watch]]
process_name = "game.exe"

[[watch.display]]
monitor = 1
monitor_device = "\\\\.\\DISPLAY2"
set_primary = true
"#;

        let parsed: Config = toml::from_str(raw).expect("config should deserialize");
        assert_eq!(parsed.watch[0].display[0].monitor, 1);
        assert!(parsed.watch[0].display[0].set_primary);
    }

    #[test]
    fn empty_config_is_considered_valid() {
        let parsed =
            parse_config_contents("\n\n", Path::new("config.toml")).expect("config should parse");

        assert!(parsed.watch.is_empty());
        assert!(!parsed.run_on_startup);
    }
}
