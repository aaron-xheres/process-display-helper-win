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
# resolution = [1920, 1080]
# refresh_rate = 60
# flip_orientation = false
#
# [[watch.display]]
# monitor = 2
# set_primary = true
# resolution = [1080, 1920]
# refresh_rate = 165
# flip_orientation = false
#
# [[watch]]
# process_name = "obs64.exe"
# restore_on_exit = false
# priority = 5
#
# [[watch.display]]
# monitor = 1
# set_primary = true
# resolution = [1920, 1080]
# refresh_rate = 120
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
    pub display: Vec<DisplayEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DisplayEntry {
    pub monitor: u8,
    #[serde(default)]
    pub set_primary: bool,
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

    for watch in &parsed.watch {
        if watch.display.is_empty() {
            bail!(
                "watch entry '{}' must define at least one [[watch.display]] section",
                watch.process_name
            );
        }
    }

    Ok(parsed)
}

pub fn config_path(exe_dir: &Path) -> PathBuf {
    exe_dir.join("config.toml")
}
