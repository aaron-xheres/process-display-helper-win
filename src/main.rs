#![cfg_attr(not(test), windows_subsystem = "windows")]

mod config;
mod logger;
mod monitor;
mod process_monitor;
mod tray;
mod watcher;
mod window_move;

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::mpsc;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW,
};
use windows::core::PCWSTR;

const SINGLE_INSTANCE_MUTEX_NAME: &str = "Local\\xhrs-process-display-helper";

struct SingleInstanceGuard {
    handle: HANDLE,
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        show_error_dialog(&format!("{error:#}"));
    }
}

fn run() -> Result<()> {
    let exe_dir = executable_dir()?;

    let _single_instance_guard = match acquire_single_instance_guard()? {
        Some(guard) => guard,
        None => {
            if logger::init_logger(&exe_dir).is_ok() {
                tracing::info!(
                    mutex_name = SINGLE_INSTANCE_MUTEX_NAME,
                    "another instance is already running; exiting"
                );
            } else {
                eprintln!("another instance is already running; exiting");
            }

            show_info_dialog("Process Display Helper is already running.");
            return Ok(());
        }
    };

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

fn acquire_single_instance_guard() -> Result<Option<SingleInstanceGuard>> {
    let mutex_name = wide(SINGLE_INSTANCE_MUTEX_NAME);
    let handle = unsafe { CreateMutexW(None, false, PCWSTR(mutex_name.as_ptr())) }
        .context("failed to create single-instance mutex")?;

    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            let _ = CloseHandle(handle);
        }
        return Ok(None);
    }

    Ok(Some(SingleInstanceGuard { handle }))
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

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
