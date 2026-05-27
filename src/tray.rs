use crate::config::Config;
use crate::process_monitor::ProcessEvent;
use crate::watcher::{WatchState, handle_process_event, reload_config as reload_watch_state};
use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread;
use std::time::Duration;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MSG, MessageBoxW, PM_REMOVE,
    PeekMessageW, TranslateMessage,
};
use windows::core::PCWSTR;

const STARTUP_TASK_NAME: &str = "ProcessDisplayHelperWin";
const LEGACY_RUN_REGISTRY_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const LEGACY_RUN_REGISTRY_VALUE_NAME: &str = "ProcessDisplayHelperWin";

pub fn run_message_loop(
    exe_dir: &Path,
    rx: &Receiver<ProcessEvent>,
    config: &Config,
    state: &mut WatchState,
) -> Result<()> {
    let mut active_config = config.clone();
    let tray_menu = Menu::new();
    let open_config_item = MenuItem::new("Open Config Folder", true, None);
    let reload_config_item = MenuItem::new("Reload Config", true, None);
    let check_config_item = MenuItem::new("Check Config", true, None);
    let run_on_startup_item =
        CheckMenuItem::new("Run on Startup", true, active_config.run_on_startup, None);
    let exit_item = MenuItem::new("Exit", true, None);

    tray_menu
        .append(&open_config_item)
        .context("failed to append Open Config Folder menu item")?;
    tray_menu
        .append(&reload_config_item)
        .context("failed to append Reload Config menu item")?;
    tray_menu
        .append(&check_config_item)
        .context("failed to append Check Config menu item")?;
    tray_menu
        .append(&PredefinedMenuItem::separator())
        .context("failed to append menu separator")?;
    tray_menu
        .append(&run_on_startup_item)
        .context("failed to append Run on Startup menu item")?;
    tray_menu
        .append(&PredefinedMenuItem::separator())
        .context("failed to append menu separator")?;
    tray_menu
        .append(&exit_item)
        .context("failed to append Exit menu item")?;

    let open_config_id = open_config_item.id().clone();
    let reload_config_id = reload_config_item.id().clone();
    let check_config_id = check_config_item.id().clone();
    let run_on_startup_id = run_on_startup_item.id().clone();
    let exit_id = exit_item.id().clone();

    let icon =
        Icon::from_rgba(vec![0x00, 0x95, 0xE8, 0xFF], 1, 1).context("failed to build tray icon")?;

    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("Process Display Helper")
        .with_icon(icon)
        .build()
        .context("failed to initialize tray icon")?;

    if let Err(error) = apply_run_on_startup_setting(active_config.run_on_startup) {
        tracing::error!(error = %error, enabled = active_config.run_on_startup, "failed to apply startup setting from config");
        show_error_dialog(&format!(
            "Failed to apply Run on Startup setting from config:\n{error:#}"
        ));
    }

    run_on_startup_item.set_checked(active_config.run_on_startup);

    tracing::info!("tray initialized");

    loop {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == open_config_id {
                open_config_folder(exe_dir);
            } else if event.id == reload_config_id {
                reload_config_from_disk(exe_dir, &mut active_config, state, &run_on_startup_item);
            } else if event.id == check_config_id {
                check_config_from_disk(exe_dir);
            } else if event.id == run_on_startup_id {
                toggle_run_on_startup(exe_dir, &mut active_config, &run_on_startup_item);
            } else if event.id == exit_id {
                tracing::info!("exit requested from tray");
                return Ok(());
            }
        }

        while let Ok(_event) = TrayIconEvent::receiver().try_recv() {}

        loop {
            match rx.try_recv() {
                Ok(event) => {
                    if let Err(error) = handle_process_event(event, &active_config, state) {
                        tracing::error!(error = %error, "failed to handle process event");
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    tracing::warn!("process event channel disconnected");
                    return Ok(());
                }
            }
        }

        pump_windows_messages();
        thread::sleep(Duration::from_millis(25));
    }
}

fn open_config_folder(exe_dir: &Path) {
    if let Err(error) = Command::new("explorer").arg(exe_dir).spawn() {
        tracing::error!(
            error = %error,
            path = %exe_dir.display(),
            "failed to open config folder"
        );
    }
}

fn reload_config_from_disk(
    exe_dir: &Path,
    config: &mut Config,
    state: &mut WatchState,
    run_on_startup_item: &CheckMenuItem,
) {
    match crate::config::load_config(exe_dir) {
        Ok(reloaded) => {
            *config = reloaded;
            tracing::info!(watch_entries = config.watch.len(), "configuration reloaded");

            if let Err(error) = reload_watch_state(config, state) {
                tracing::error!(error = %error, "failed to apply reloaded configuration");
                show_error_dialog(&format!(
                    "Configuration was reloaded but failed to apply runtime state:\n{error:#}"
                ));
                return;
            }

            if let Err(error) = apply_run_on_startup_setting(config.run_on_startup) {
                tracing::error!(error = %error, enabled = config.run_on_startup, "failed to apply startup setting after config reload");
                show_error_dialog(&format!(
                    "Configuration was reloaded, but Run on Startup failed to apply:\n{error:#}"
                ));
                return;
            }

            run_on_startup_item.set_checked(config.run_on_startup);

            show_info_dialog(&format!(
                "Configuration reloaded successfully.\nWatch entries: {}",
                config.watch.len()
            ));
        }
        Err(error) => {
            tracing::error!(error = %error, "failed to reload configuration from disk");
            show_error_dialog(&format!("Failed to reload configuration:\n{error:#}"));
        }
    }
}

fn check_config_from_disk(exe_dir: &Path) {
    match crate::config::check_config_file(exe_dir) {
        Ok(validated) => {
            tracing::info!(
                watch_entries = validated.watch.len(),
                "configuration check succeeded"
            );
            show_info_dialog(&format!(
                "Configuration is valid.\nWatch entries: {}",
                validated.watch.len()
            ));
        }
        Err(error) => {
            tracing::error!(error = %error, "configuration check failed");
            show_error_dialog(&format!("Configuration is invalid:\n{error:#}"));
        }
    }
}

fn toggle_run_on_startup(exe_dir: &Path, config: &mut Config, run_on_startup_item: &CheckMenuItem) {
    let previous = config.run_on_startup;
    let target = !previous;

    if let Err(error) = apply_run_on_startup_setting(target) {
        tracing::error!(error = %error, enabled = target, "failed to apply run on startup setting");
        run_on_startup_item.set_checked(previous);
        show_error_dialog(&format!("Failed to update Run on Startup:\n{error:#}"));
        return;
    }

    match crate::config::set_run_on_startup(exe_dir, target) {
        Ok(updated) => {
            *config = updated;
            run_on_startup_item.set_checked(config.run_on_startup);
            tracing::info!(
                enabled = config.run_on_startup,
                "updated run on startup setting"
            );
        }
        Err(error) => {
            tracing::error!(error = %error, enabled = target, "failed to persist run on startup setting");
            if let Err(rollback_error) = apply_run_on_startup_setting(previous) {
                tracing::error!(error = %rollback_error, enabled = previous, "failed to roll back run on startup state after config write failure");
            }
            run_on_startup_item.set_checked(previous);
            show_error_dialog(&format!(
                "Failed to persist Run on Startup setting:\n{error:#}"
            ));
        }
    }
}

fn apply_run_on_startup_setting(enabled: bool) -> Result<()> {
    if enabled {
        add_run_on_startup_task()?;

        if let Err(error) = remove_legacy_run_on_startup_registry_value() {
            tracing::warn!(error = %error, "failed to remove legacy Run registry startup value");
        }

        Ok(())
    } else {
        remove_run_on_startup_task()?;

        if let Err(error) = remove_legacy_run_on_startup_registry_value() {
            tracing::warn!(error = %error, "failed to remove legacy Run registry startup value");
        }

        Ok(())
    }
}

fn add_run_on_startup_task() -> Result<()> {
    let command_value = startup_task_command_value()?;
    run_schtasks_command(
        &[
            "/Create",
            "/TN",
            STARTUP_TASK_NAME,
            "/SC",
            "ONLOGON",
            "/RL",
            "HIGHEST",
            "/TR",
            &command_value,
            "/F",
        ],
        "adding Run on Startup scheduled task",
    )
}

fn remove_run_on_startup_task() -> Result<()> {
    let query_output = Command::new("schtasks")
        .args(["/Query", "/TN", STARTUP_TASK_NAME])
        .output()
        .context("failed to query Run on Startup scheduled task")?;

    if !query_output.status.success() {
        return Ok(());
    }

    run_schtasks_command(
        &["/Delete", "/TN", STARTUP_TASK_NAME, "/F"],
        "removing Run on Startup scheduled task",
    )
}

fn remove_legacy_run_on_startup_registry_value() -> Result<()> {
    let query_output = Command::new("reg")
        .args([
            "query",
            LEGACY_RUN_REGISTRY_KEY,
            "/v",
            LEGACY_RUN_REGISTRY_VALUE_NAME,
        ])
        .output()
        .context("failed to query legacy Run on Startup registry value")?;

    if !query_output.status.success() {
        return Ok(());
    }

    run_reg_command(
        &[
            "delete",
            LEGACY_RUN_REGISTRY_KEY,
            "/v",
            LEGACY_RUN_REGISTRY_VALUE_NAME,
            "/f",
        ],
        "removing legacy Run on Startup registry value",
    )
}

fn run_schtasks_command(args: &[&str], action: &str) -> Result<()> {
    let output = Command::new("schtasks")
        .args(args)
        .output()
        .with_context(|| format!("failed to execute schtasks command while {action}"))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let details = if stderr.is_empty() { stdout } else { stderr };

    bail!("schtasks command failed while {action}: {details}")
}

fn run_reg_command(args: &[&str], action: &str) -> Result<()> {
    let output = Command::new("reg")
        .args(args)
        .output()
        .with_context(|| format!("failed to execute reg command while {action}"))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let details = if stderr.is_empty() { stdout } else { stderr };

    bail!("reg command failed while {action}: {details}")
}

fn startup_task_command_value() -> Result<String> {
    let exe_path = std::env::current_exe().context("failed to resolve executable path")?;
    Ok(format!("\"{}\"", exe_path.display()))
}

fn show_info_dialog(message: &str) {
    let title = wide("Process Display Helper");
    let body = wide(message);

    unsafe {
        MessageBoxW(
            None,
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

fn show_error_dialog(message: &str) {
    let title = wide("Process Display Helper");
    let body = wide(message);

    unsafe {
        MessageBoxW(
            None,
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn pump_windows_messages() {
    unsafe {
        let mut message = MSG::default();
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
