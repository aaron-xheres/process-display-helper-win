use anyhow::{Context, Result, bail};
use serde::Deserialize;
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

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub watch: Vec<WatchEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WatchEntry {
    pub process_name: String,
    #[serde(default)]
    pub restore_on_exit: bool,
    #[serde(default)]
    pub priority: u8,
    #[serde(default)]
    pub display: Vec<DisplayEntry>,
}

#[derive(Debug, Clone, Deserialize)]
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
    }
}
