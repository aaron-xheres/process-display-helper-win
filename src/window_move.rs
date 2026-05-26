use crate::monitor::{MonitorInfo, enumerate_monitors};
use anyhow::{Context, Result, anyhow, bail};
use std::mem::size_of;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, SWP_NOACTIVATE,
    SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
};

pub fn move_process_windows_to_monitor(pid: u32, target_monitor_index: u8) -> Result<u32> {
    let monitors = enumerate_monitors()?;
    let target_monitor = monitors
        .iter()
        .find(|monitor| monitor.index == target_monitor_index)
        .ok_or_else(|| anyhow!("target monitor index {} not found", target_monitor_index))?;

    let windows = enumerate_visible_windows_for_pid(pid);
    if windows.is_empty() {
        return Ok(0);
    }

    let mut moved_windows = 0u32;
    for hwnd in windows {
        move_window_to_monitor(hwnd, target_monitor)?;
        moved_windows = moved_windows.saturating_add(1);
    }

    Ok(moved_windows)
}

fn enumerate_visible_windows_for_pid(pid: u32) -> Vec<HWND> {
    let mut context = EnumWindowsContext {
        pid,
        windows: Vec::new(),
    };

    unsafe {
        let _ = EnumWindows(
            Some(collect_visible_windows_for_pid),
            LPARAM((&mut context as *mut EnumWindowsContext) as isize),
        );
    }

    context.windows
}

unsafe extern "system" fn collect_visible_windows_for_pid(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let context = unsafe { &mut *(lparam.0 as *mut EnumWindowsContext) };

    let mut window_pid = 0u32;
    let _ = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut window_pid)) };
    if window_pid != context.pid {
        return BOOL(1);
    }

    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return BOOL(1);
    }

    context.windows.push(hwnd);
    BOOL(1)
}

fn move_window_to_monitor(hwnd: HWND, target_monitor: &MonitorInfo) -> Result<()> {
    let mut window_rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut window_rect) }.context("failed to read window bounds")?;

    let source_monitor_rect = monitor_rect_for_window(hwnd)?;

    let window_width = window_rect.right - window_rect.left;
    let window_height = window_rect.bottom - window_rect.top;

    let target_left = target_monitor.position.0;
    let target_top = target_monitor.position.1;
    let target_right = target_left + i32::from(target_monitor.resolution.0);
    let target_bottom = target_top + i32::from(target_monitor.resolution.1);

    let desired_left = target_left + (window_rect.left - source_monitor_rect.left);
    let desired_top = target_top + (window_rect.top - source_monitor_rect.top);

    let clamped_left = clamp_window_origin(desired_left, window_width, target_left, target_right);
    let clamped_top = clamp_window_origin(desired_top, window_height, target_top, target_bottom);

    unsafe {
        SetWindowPos(
            hwnd,
            HWND::default(),
            clamped_left,
            clamped_top,
            0,
            0,
            SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    }
    .context("failed to move window to target monitor")?;

    Ok(())
}

fn monitor_rect_for_window(hwnd: HWND) -> Result<RECT> {
    let monitor_handle = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor_handle.0.is_null() {
        bail!("failed to resolve source monitor for window");
    }

    let mut monitor_info = MONITORINFO::default();
    monitor_info.cbSize = size_of::<MONITORINFO>() as u32;

    let ok = unsafe { GetMonitorInfoW(monitor_handle, &mut monitor_info as *mut MONITORINFO) };
    if !ok.as_bool() {
        bail!("failed to read source monitor info for window");
    }

    Ok(monitor_info.rcMonitor)
}

fn clamp_window_origin(origin: i32, extent: i32, min_bound: i32, max_bound: i32) -> i32 {
    let available_span = max_bound - min_bound;
    if extent >= available_span {
        return min_bound;
    }

    let max_origin = max_bound - extent;
    origin.clamp(min_bound, max_origin)
}

struct EnumWindowsContext {
    pid: u32,
    windows: Vec<HWND>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_window_origin_clamps_to_bounds() {
        assert_eq!(clamp_window_origin(50, 600, 100, 800), 100);
        assert_eq!(clamp_window_origin(500, 600, 100, 800), 200);
        assert_eq!(clamp_window_origin(150, 600, 100, 800), 150);
    }

    #[test]
    fn clamp_window_origin_pins_when_window_larger_than_target() {
        assert_eq!(clamp_window_origin(300, 900, 100, 800), 100);
    }
}
