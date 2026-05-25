use crate::config::Config;
use crate::process_monitor::ProcessEvent;
use crate::watcher::{WatchState, handle_process_event, reload_config as reload_watch_state};
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread;
use std::time::Duration;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
};

pub fn run_message_loop(
    exe_dir: &Path,
    rx: &Receiver<ProcessEvent>,
    config: &Config,
    state: &mut WatchState,
) -> Result<()> {
    let tray_menu = Menu::new();
    let open_config_item = MenuItem::new("Open Config Folder", true, None);
    let reload_config_item = MenuItem::new("Reload Config", true, None);
    let exit_item = MenuItem::new("Exit", true, None);
    let mut active_config = config.clone();

    tray_menu
        .append(&open_config_item)
        .context("failed to append Open Config Folder menu item")?;
    tray_menu
        .append(&reload_config_item)
        .context("failed to append Reload Config menu item")?;
    tray_menu
        .append(&PredefinedMenuItem::separator())
        .context("failed to append menu separator")?;
    tray_menu
        .append(&exit_item)
        .context("failed to append Exit menu item")?;

    let open_config_id = open_config_item.id().clone();
    let reload_config_id = reload_config_item.id().clone();
    let exit_id = exit_item.id().clone();

    let icon =
        Icon::from_rgba(vec![0x00, 0x95, 0xE8, 0xFF], 1, 1).context("failed to build tray icon")?;

    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("Process Display Helper")
        .with_icon(icon)
        .build()
        .context("failed to initialize tray icon")?;

    tracing::info!("tray initialized");

    loop {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == open_config_id {
                open_config_folder(exe_dir);
            } else if event.id == reload_config_id {
                reload_config_from_disk(exe_dir, &mut active_config, state);
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

fn reload_config_from_disk(exe_dir: &Path, config: &mut Config, state: &mut WatchState) {
    match crate::config::load_config(exe_dir) {
        Ok(reloaded) => {
            *config = reloaded;
            tracing::info!(watch_entries = config.watch.len(), "configuration reloaded");

            if let Err(error) = reload_watch_state(config, state) {
                tracing::error!(error = %error, "failed to apply reloaded configuration");
            }
        }
        Err(error) => {
            tracing::error!(error = %error, "failed to reload configuration from disk");
        }
    }
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
