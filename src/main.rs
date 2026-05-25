#![cfg_attr(not(test), windows_subsystem = "windows")]

mod config;
mod logger;
mod monitor;
mod process_monitor;
mod tray;
mod watcher;

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::mpsc;
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
use windows::core::PCWSTR;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        show_error_dialog(&format!("{error:#}"));
    }
}

fn run() -> Result<()> {
    let exe_dir = executable_dir()?;
    logger::init_logger(&exe_dir)?;

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        path = %exe_dir.display(),
        "starting process-display-helper"
    );

    let config = config::load_config(&exe_dir)?;
    tracing::info!(watch_entries = config.watch.len(), "config loaded");

    let (tx, rx) = mpsc::channel();
    let etw_handle = process_monitor::spawn_etw_listener(tx)?;
    let mut state = watcher::WatchState::default();

    let loop_result = tray::run_message_loop(&exe_dir, &rx, &config, &mut state);
    etw_handle.stop();

    loop_result
}

fn executable_dir() -> Result<PathBuf> {
    let exe_path = std::env::current_exe().context("failed to resolve current executable path")?;
    exe_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("failed to resolve executable directory"))
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
