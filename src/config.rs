use anyhow::{Context, Result};
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
# target_monitor = 2
# restore_on_exit = true
# priority = 10
# # Optional mode override (must be supported by target monitor):
# # resolution = [2560, 1440]
# # refresh_rate = 165
# # flip_orientation = true
#
# [[watch]]
# process_name = "obs64.exe"
# target_monitor = 1
# restore_on_exit = false
# priority = 5
"#;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub watch: Vec<WatchEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WatchEntry {
    pub process_name: String,
    pub target_monitor: u8,
    #[serde(default)]
    pub restore_on_exit: bool,
    #[serde(default)]
    pub priority: u8,
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
    if raw.trim().is_empty() {
        tracing::warn!(path = %config_path.display(), "config file is empty; using no watch entries");
        return Ok(Config::default());
    }

    let parsed: Config = toml::from_str(&raw)
        .with_context(|| format!("failed to parse config file at {}", config_path.display()))?;

    Ok(parsed)
}

pub fn config_path(exe_dir: &Path) -> PathBuf {
    exe_dir.join("config.toml")
}
