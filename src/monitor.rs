use anyhow::{Context, Result, anyhow, bail};
use std::collections::HashMap;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
    QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    CDS_NORESET, CDS_SET_PRIMARY, CDS_UPDATEREGISTRY, ChangeDisplaySettingsExW,
    DEVMODE_DISPLAY_ORIENTATION, DEVMODEW, DISP_CHANGE, DISP_CHANGE_SUCCESSFUL,
    DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICE_PRIMARY_DEVICE, DISPLAY_DEVICEW,
    DM_DISPLAYFREQUENCY, DM_DISPLAYORIENTATION, DM_PELSHEIGHT, DM_PELSWIDTH, DM_POSITION, DMDO_90,
    DMDO_180, DMDO_270, DMDO_DEFAULT, ENUM_CURRENT_SETTINGS, EnumDisplayDevicesW,
    EnumDisplaySettingsW,
};
use windows::core::PCWSTR;

#[derive(Debug, Clone)]
pub struct DisplaySnapshot {
    pub primary: u8,
    pub primary_device_name: String,
    pub resolution: (u16, u16),
    pub refresh_rate: u16,
    pub flip_orientation: bool,
    pub display_orientation: DEVMODE_DISPLAY_ORIENTATION,
    pub monitor_modes: Vec<DisplayModeSnapshot>,
}

#[derive(Debug, Clone)]
pub struct DisplayModeSnapshot {
    pub device_name: String,
    pub resolution: (u16, u16),
    pub refresh_rate: u16,
    pub display_orientation: DEVMODE_DISPLAY_ORIENTATION,
}

#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub device_name: String,
    pub index: u8,
    pub position: (i32, i32),
    pub resolution: (u16, u16),
    pub refresh_rate: u16,
    pub display_orientation: DEVMODE_DISPLAY_ORIENTATION,
    pub is_primary: bool,
}

pub fn enumerate_monitors() -> Result<Vec<MonitorInfo>> {
    let mut result = Vec::new();
    let mut device_num: u32 = 0;

    loop {
        let mut display_device = DISPLAY_DEVICEW::default();
        display_device.cb = size_of::<DISPLAY_DEVICEW>() as u32;

        let ok = unsafe { EnumDisplayDevicesW(PCWSTR::null(), device_num, &mut display_device, 0) };
        if !ok.as_bool() {
            break;
        }

        device_num = device_num.saturating_add(1);

        let attached = (display_device.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP) != 0;
        if !attached {
            continue;
        }

        let mut mode = DEVMODEW::default();
        mode.dmSize = size_of::<DEVMODEW>() as u16;

        let current_ok = unsafe {
            EnumDisplaySettingsW(
                PCWSTR(display_device.DeviceName.as_ptr()),
                ENUM_CURRENT_SETTINGS,
                &mut mode,
            )
        };
        if !current_ok.as_bool() {
            continue;
        }

        let (position, display_orientation) = unsafe {
            (
                (
                    mode.Anonymous1.Anonymous2.dmPosition.x,
                    mode.Anonymous1.Anonymous2.dmPosition.y,
                ),
                mode.Anonymous1.Anonymous2.dmDisplayOrientation,
            )
        };

        let refresh = if mode.dmDisplayFrequency == 0 {
            60
        } else {
            mode.dmDisplayFrequency as u16
        };

        let device_name = wide_to_string(&display_device.DeviceName);
        let is_primary = (display_device.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE) != 0;

        result.push(MonitorInfo {
            device_name,
            index: 0,
            position,
            resolution: (mode.dmPelsWidth as u16, mode.dmPelsHeight as u16),
            refresh_rate: refresh,
            display_orientation,
            is_primary,
        });
    }

    if result.is_empty() {
        return Ok(result);
    }

    let display_label_indices =
        load_display_label_indices().context("failed to load monitor labels from DisplayConfig")?;

    for monitor in &mut result {
        monitor.index = display_label_indices
            .get(&monitor.device_name)
            .copied()
            .ok_or_else(|| {
                let mut known_labels: Vec<_> = display_label_indices.keys().cloned().collect();
                known_labels.sort();
                anyhow!(
                    "DisplayConfig did not return a label for active monitor '{}' (known labels: {:?})",
                    monitor.device_name,
                    known_labels
                )
            })?;
    }

    Ok(result)
}

pub fn log_monitor_inventory() -> Result<()> {
    let monitors = enumerate_monitors()?;
    for monitor in monitors {
        tracing::info!(
            monitor = monitor.index,
            device = %monitor.device_name,
            is_primary = monitor.is_primary,
            position_x = monitor.position.0,
            position_y = monitor.position.1,
            width = monitor.resolution.0,
            height = monitor.resolution.1,
            refresh_hz = monitor.refresh_rate,
            orientation = monitor.display_orientation.0,
            "monitor inventory"
        );
    }

    Ok(())
}

pub fn current_primary_snapshot() -> Result<DisplaySnapshot> {
    let monitors = enumerate_monitors()?;
    let primary = monitors
        .iter()
        .find(|monitor| monitor.is_primary)
        .ok_or_else(|| anyhow!("no primary monitor found"))?;

    let normalized_resolution =
        normalize_resolution_for_orientation(primary.resolution, primary.display_orientation);

    let monitor_modes = monitors
        .iter()
        .map(|monitor| DisplayModeSnapshot {
            device_name: monitor.device_name.clone(),
            resolution: normalize_resolution_for_orientation(
                monitor.resolution,
                monitor.display_orientation,
            ),
            refresh_rate: monitor.refresh_rate,
            display_orientation: monitor.display_orientation,
        })
        .collect();

    Ok(DisplaySnapshot {
        primary: primary.index,
        primary_device_name: primary.device_name.clone(),
        resolution: normalized_resolution,
        refresh_rate: primary.refresh_rate,
        flip_orientation: is_flipped_orientation(primary.display_orientation),
        display_orientation: primary.display_orientation,
        monitor_modes,
    })
}

pub fn restore_display_snapshot(snapshot: &DisplaySnapshot) -> Result<()> {
    let monitors = enumerate_monitors()?;
    let monitor_indices_by_name: HashMap<&str, u8> = monitors
        .iter()
        .map(|monitor| (monitor.device_name.as_str(), monitor.index))
        .collect();

    let primary_index = monitor_indices_by_name
        .get(snapshot.primary_device_name.as_str())
        .copied()
        .unwrap_or(snapshot.primary);

    set_primary_monitor_with_orientation(
        primary_index,
        Some([snapshot.resolution.0, snapshot.resolution.1]),
        Some(snapshot.refresh_rate),
        snapshot.flip_orientation,
        Some(snapshot.display_orientation),
    )
    .with_context(|| {
        format!(
            "failed restoring primary monitor {}",
            snapshot.primary_device_name
        )
    })?;

    for mode in &snapshot.monitor_modes {
        if mode.device_name == snapshot.primary_device_name {
            continue;
        }

        if !monitor_indices_by_name.contains_key(mode.device_name.as_str()) {
            tracing::warn!(
                device = %mode.device_name,
                "skipping baseline monitor restore because device is no longer present"
            );
            continue;
        }

        apply_mode_only(
            &mode.device_name,
            mode.resolution.0,
            mode.resolution.1,
            mode.refresh_rate,
            mode.display_orientation,
        )
        .with_context(|| format!("failed restoring monitor mode for {}", mode.device_name))?;
    }

    Ok(())
}

pub fn set_primary_monitor(
    target_index: u8,
    resolution: Option<[u16; 2]>,
    refresh_rate: Option<u16>,
    flip_orientation: bool,
) -> Result<()> {
    set_primary_monitor_with_orientation(
        target_index,
        resolution,
        refresh_rate,
        flip_orientation,
        None,
    )
}

pub fn set_monitor_mode(
    target_index: u8,
    resolution: Option<[u16; 2]>,
    refresh_rate: Option<u16>,
    flip_orientation: bool,
) -> Result<()> {
    let monitors = enumerate_monitors()?;
    let target = monitors
        .iter()
        .find(|monitor| monitor.index == target_index)
        .ok_or_else(|| anyhow!("target monitor index {} not found", target_index))?;

    let desired_width = resolution
        .map(|value| value[0])
        .unwrap_or(target.resolution.0);
    let desired_height = resolution
        .map(|value| value[1])
        .unwrap_or(target.resolution.1);
    let desired_refresh = refresh_rate.unwrap_or(target.refresh_rate);
    let desired_orientation =
        desired_display_orientation(desired_width, desired_height, flip_orientation);

    let mode_change_requested = desired_width != target.resolution.0
        || desired_height != target.resolution.1
        || desired_refresh != target.refresh_rate
        || desired_orientation != target.display_orientation;

    if !mode_change_requested {
        tracing::info!(
            target_monitor = target.index,
            "target monitor mode already matches requested values; skipping mode update"
        );
        return Ok(());
    }

    apply_mode_only(
        &target.device_name,
        desired_width,
        desired_height,
        desired_refresh,
        desired_orientation,
    )?;

    tracing::info!(
        target_monitor = target.index,
        width = desired_width,
        height = desired_height,
        refresh_hz = desired_refresh,
        flip_orientation,
        desired_orientation = desired_orientation.0,
        "monitor mode updated"
    );

    Ok(())
}

fn set_primary_monitor_with_orientation(
    target_index: u8,
    resolution: Option<[u16; 2]>,
    refresh_rate: Option<u16>,
    flip_orientation: bool,
    orientation_override: Option<DEVMODE_DISPLAY_ORIENTATION>,
) -> Result<()> {
    let monitors = enumerate_monitors()?;
    let target = monitors
        .iter()
        .find(|monitor| monitor.index == target_index)
        .ok_or_else(|| anyhow!("target monitor index {} not found", target_index))?;

    let desired_width = resolution.map(|v| v[0]).unwrap_or(target.resolution.0);
    let desired_height = resolution.map(|v| v[1]).unwrap_or(target.resolution.1);
    let desired_refresh = refresh_rate.unwrap_or(target.refresh_rate);
    let desired_orientation = orientation_override.unwrap_or_else(|| {
        desired_display_orientation(desired_width, desired_height, flip_orientation)
    });

    let mode_change_requested = desired_width != target.resolution.0
        || desired_height != target.resolution.1
        || desired_refresh != target.refresh_rate
        || desired_orientation != target.display_orientation;

    if target.is_primary && !mode_change_requested {
        tracing::info!(
            target_monitor = target.index,
            "target monitor is already primary; skipping switch"
        );
        return Ok(());
    }

    if target.is_primary {
        apply_mode_only(
            &target.device_name,
            desired_width,
            desired_height,
            desired_refresh,
            desired_orientation,
        )?;
        tracing::info!(
            target_monitor = target.index,
            width = desired_width,
            height = desired_height,
            refresh_hz = desired_refresh,
            flip_orientation,
            desired_orientation = desired_orientation.0,
            "target monitor already primary; applied mode change only"
        );
        return Ok(());
    }

    let offset_x = target.position.0;
    let offset_y = target.position.1;

    let target_device_name_wide = to_wide_null_terminated(&target.device_name);
    let mut target_mode = query_current_mode(PCWSTR(target_device_name_wide.as_ptr()))?;
    target_mode.Anonymous1.Anonymous2.dmPosition.x = 0;
    target_mode.Anonymous1.Anonymous2.dmPosition.y = 0;
    target_mode.dmFields |= DM_POSITION;

    let target_status = unsafe {
        ChangeDisplaySettingsExW(
            PCWSTR(target_device_name_wide.as_ptr()),
            Some(std::ptr::from_ref(&target_mode)),
            HWND(std::ptr::null_mut()),
            CDS_UPDATEREGISTRY | CDS_NORESET | CDS_SET_PRIMARY,
            None,
        )
    };
    ensure_display_change_success(target_status).with_context(|| {
        format!(
            "failed applying primary display change for {}",
            target.device_name
        )
    })?;

    for monitor in monitors
        .iter()
        .filter(|monitor| monitor.index != target.index)
    {
        let device_name_wide = to_wide_null_terminated(&monitor.device_name);
        let mut mode = query_current_mode(PCWSTR(device_name_wide.as_ptr()))?;

        let new_x = monitor.position.0 - offset_x;
        let new_y = monitor.position.1 - offset_y;

        mode.Anonymous1.Anonymous2.dmPosition.x = new_x;
        mode.Anonymous1.Anonymous2.dmPosition.y = new_y;
        mode.dmFields |= DM_POSITION;

        let status = unsafe {
            ChangeDisplaySettingsExW(
                PCWSTR(device_name_wide.as_ptr()),
                Some(std::ptr::from_ref(&mode)),
                HWND(std::ptr::null_mut()),
                CDS_UPDATEREGISTRY | CDS_NORESET,
                None,
            )
        };
        ensure_display_change_success(status).with_context(|| {
            format!(
                "failed applying display changes for {}",
                monitor.device_name
            )
        })?;
    }

    let commit_status = unsafe {
        ChangeDisplaySettingsExW(
            PCWSTR::null(),
            None,
            HWND(std::ptr::null_mut()),
            Default::default(),
            None,
        )
    };
    ensure_display_change_success(commit_status).context("failed to commit display changes")?;

    if mode_change_requested {
        apply_mode_only(
            &target.device_name,
            desired_width,
            desired_height,
            desired_refresh,
            desired_orientation,
        )
        .context("failed to apply target monitor mode after primary switch")?;
    }

    tracing::info!(
        target_monitor = target.index,
        width = desired_width,
        height = desired_height,
        refresh_hz = desired_refresh,
        flip_orientation,
        desired_orientation = desired_orientation.0,
        "primary monitor switched"
    );

    Ok(())
}

fn apply_mode_only(
    device_name: &str,
    width: u16,
    height: u16,
    refresh_rate: u16,
    desired_orientation: DEVMODE_DISPLAY_ORIENTATION,
) -> Result<()> {
    let device_name_wide = to_wide_null_terminated(device_name);
    let mut attempts = vec![(width, height)];
    if width != height {
        attempts.push((height, width));
    }

    let mut errors: Vec<String> = Vec::new();

    for (candidate_width, candidate_height) in attempts.iter().copied() {
        match apply_mode_single_pass(
            PCWSTR(device_name_wide.as_ptr()),
            desired_orientation,
            candidate_width,
            candidate_height,
            refresh_rate,
        ) {
            Ok(()) => return Ok(()),
            Err(error) => errors.push(format!(
                "single-pass {}x{}@{}Hz failed: {}",
                candidate_width, candidate_height, refresh_rate, error
            )),
        }
    }

    for (candidate_width, candidate_height) in attempts {
        match apply_mode_orientation_first(
            PCWSTR(device_name_wide.as_ptr()),
            desired_orientation,
            candidate_width,
            candidate_height,
            refresh_rate,
        ) {
            Ok(()) => return Ok(()),
            Err(error) => errors.push(format!(
                "orientation-first {}x{}@{}Hz failed: {}",
                candidate_width, candidate_height, refresh_rate, error
            )),
        }
    }

    bail!(
        "failed to apply monitor mode after fallback attempts: {}",
        errors.join(" | ")
    )
}

fn query_current_mode(device_name: PCWSTR) -> Result<DEVMODEW> {
    let mut mode = DEVMODEW::default();
    mode.dmSize = size_of::<DEVMODEW>() as u16;

    let ok = unsafe { EnumDisplaySettingsW(device_name, ENUM_CURRENT_SETTINGS, &mut mode) };
    if !ok.as_bool() {
        bail!("failed to query display settings");
    }

    Ok(mode)
}

fn apply_mode_single_pass(
    device_name: PCWSTR,
    desired_orientation: DEVMODE_DISPLAY_ORIENTATION,
    width: u16,
    height: u16,
    refresh_rate: u16,
) -> Result<()> {
    let mut mode = query_current_mode(device_name)?;
    mode.Anonymous1.Anonymous2.dmDisplayOrientation = desired_orientation;
    mode.dmPelsWidth = width as u32;
    mode.dmPelsHeight = height as u32;
    mode.dmDisplayFrequency = refresh_rate as u32;
    mode.dmFields |= DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY | DM_DISPLAYORIENTATION;

    let status = unsafe {
        ChangeDisplaySettingsExW(
            device_name,
            Some(std::ptr::from_ref(&mode)),
            HWND(std::ptr::null_mut()),
            CDS_UPDATEREGISTRY,
            None,
        )
    };
    ensure_display_change_success(status)
}

fn apply_mode_orientation_first(
    device_name: PCWSTR,
    desired_orientation: DEVMODE_DISPLAY_ORIENTATION,
    width: u16,
    height: u16,
    refresh_rate: u16,
) -> Result<()> {
    let mut orientation_mode = query_current_mode(device_name)?;
    let current_orientation =
        unsafe { orientation_mode.Anonymous1.Anonymous2.dmDisplayOrientation };

    if current_orientation != desired_orientation {
        let (orientation_width, orientation_height) = mode_dimensions_for_orientation_transition(
            orientation_mode.dmPelsWidth as u16,
            orientation_mode.dmPelsHeight as u16,
            current_orientation,
            desired_orientation,
        );

        orientation_mode.Anonymous1.Anonymous2.dmDisplayOrientation = desired_orientation;
        orientation_mode.dmPelsWidth = orientation_width as u32;
        orientation_mode.dmPelsHeight = orientation_height as u32;
        orientation_mode.dmFields |= DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYORIENTATION;

        let orientation_status = unsafe {
            ChangeDisplaySettingsExW(
                device_name,
                Some(std::ptr::from_ref(&orientation_mode)),
                HWND(std::ptr::null_mut()),
                CDS_UPDATEREGISTRY,
                None,
            )
        };
        ensure_display_change_success(orientation_status).context("failed orientation step")?;
    }

    apply_mode_single_pass(
        device_name,
        desired_orientation,
        width,
        height,
        refresh_rate,
    )
    .context("failed final mode step")
}

fn ensure_display_change_success(status: DISP_CHANGE) -> Result<()> {
    if status == DISP_CHANGE_SUCCESSFUL {
        return Ok(());
    }

    bail!("display API returned status code {}", status.0)
}

fn desired_display_orientation(
    width: u16,
    height: u16,
    flip_orientation: bool,
) -> DEVMODE_DISPLAY_ORIENTATION {
    if is_portrait_resolution(width, height) {
        if flip_orientation { DMDO_270 } else { DMDO_90 }
    } else if flip_orientation {
        DMDO_180
    } else {
        DMDO_DEFAULT
    }
}

fn mode_dimensions_for_orientation_transition(
    width: u16,
    height: u16,
    current_orientation: DEVMODE_DISPLAY_ORIENTATION,
    desired_orientation: DEVMODE_DISPLAY_ORIENTATION,
) -> (u16, u16) {
    if is_portrait_orientation(current_orientation) != is_portrait_orientation(desired_orientation)
    {
        (height, width)
    } else {
        (width, height)
    }
}

fn is_portrait_resolution(width: u16, height: u16) -> bool {
    height > width
}

fn normalize_resolution_for_orientation(
    resolution: (u16, u16),
    orientation: DEVMODE_DISPLAY_ORIENTATION,
) -> (u16, u16) {
    let (width, height) = resolution;
    if is_portrait_orientation(orientation) {
        if width > height {
            (height, width)
        } else {
            (width, height)
        }
    } else if height > width {
        (height, width)
    } else {
        (width, height)
    }
}

fn is_portrait_orientation(orientation: DEVMODE_DISPLAY_ORIENTATION) -> bool {
    orientation == DMDO_90 || orientation == DMDO_270
}

fn is_flipped_orientation(orientation: DEVMODE_DISPLAY_ORIENTATION) -> bool {
    orientation == DMDO_180 || orientation == DMDO_270
}

fn wide_to_string(wide: &[u16]) -> String {
    let len = wide.iter().position(|ch| *ch == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..len])
}

fn load_display_label_indices() -> Result<HashMap<String, u8>> {
    const MAX_QUERY_RETRIES: u8 = 3;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

    let mut attempt: u8 = 0;
    loop {
        attempt = attempt.saturating_add(1);

        let mut path_count = 0u32;
        let mut mode_count = 0u32;

        let buffer_status = unsafe {
            GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
        };
        if buffer_status.0 != 0 {
            bail!(
                "GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS) failed with status {} ({})",
                buffer_status.0,
                win32_status_message(buffer_status.0 as i32)
            );
        }

        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];

        let query_status = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                None,
            )
        };

        if query_status.0 == ERROR_INSUFFICIENT_BUFFER && attempt < MAX_QUERY_RETRIES {
            tracing::debug!(
                attempt,
                status = query_status.0,
                status_text = %win32_status_message(query_status.0 as i32),
                requested_path_count = paths.len(),
                requested_mode_count = modes.len(),
                "QueryDisplayConfig reported insufficient buffer; retrying"
            );
            continue;
        }

        if query_status.0 != 0 {
            bail!(
                "QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS) failed with status {} ({})",
                query_status.0,
                win32_status_message(query_status.0 as i32)
            );
        }

        paths.truncate(path_count as usize);
        if paths.is_empty() {
            bail!("QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS) returned zero active display paths");
        }

        let mut mapping = HashMap::new();
        let mut source_name_failures = 0u32;
        let mut ordinal_overflow = 0u32;

        for path in paths {
            let mut source_name = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            source_name.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            source_name.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            source_name.header.adapterId = path.sourceInfo.adapterId;
            source_name.header.id = path.sourceInfo.id;

            let status = unsafe { DisplayConfigGetDeviceInfo(&mut source_name.header) };
            if status != 0 {
                source_name_failures = source_name_failures.saturating_add(1);
                tracing::warn!(
                    status,
                    status_text = %win32_status_message(status),
                    adapter_luid_high = path.sourceInfo.adapterId.HighPart,
                    adapter_luid_low = path.sourceInfo.adapterId.LowPart,
                    source_id = path.sourceInfo.id,
                    target_id = path.targetInfo.id,
                    "DisplayConfigGetDeviceInfo(GET_SOURCE_NAME) failed for active path"
                );
                continue;
            }

            let gdi_name = wide_to_string(&source_name.viewGdiDeviceName);
            if mapping.contains_key(&gdi_name) {
                continue;
            }

            let Some(monitor_index) = u8::try_from(mapping.len().saturating_add(1)).ok() else {
                ordinal_overflow = ordinal_overflow.saturating_add(1);
                tracing::warn!(
                    gdi_name = %gdi_name,
                    source_id = path.sourceInfo.id,
                    adapter_luid_high = path.sourceInfo.adapterId.HighPart,
                    adapter_luid_low = path.sourceInfo.adapterId.LowPart,
                    target_id = path.targetInfo.id,
                    "DisplayConfig path ordinal exceeded supported monitor index range"
                );
                continue;
            };

            tracing::debug!(
                gdi_name = %gdi_name,
                source_id = path.sourceInfo.id,
                target_id = path.targetInfo.id,
                monitor = monitor_index,
                "mapped monitor label from DisplayConfig path order"
            );
            mapping.insert(gdi_name, monitor_index);
        }

        if mapping.is_empty() {
            bail!(
                "DisplayConfig returned {} active path(s) but no usable source labels (device_info_failures={}, ordinal_overflow={})",
                path_count,
                source_name_failures,
                ordinal_overflow
            );
        }

        return Ok(mapping);
    }
}

fn win32_status_message(status: i32) -> String {
    std::io::Error::from_raw_os_error(status).to_string()
}

fn to_wide_null_terminated(value: &str) -> Vec<u16> {
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientation_defaults_to_landscape_for_wide_resolution() {
        assert_eq!(desired_display_orientation(1920, 1080, false), DMDO_DEFAULT);
    }

    #[test]
    fn orientation_uses_portrait_for_tall_resolution() {
        assert_eq!(desired_display_orientation(1080, 1920, false), DMDO_90);
    }

    #[test]
    fn orientation_uses_flipped_variant_when_requested() {
        assert_eq!(desired_display_orientation(1920, 1080, true), DMDO_180);
        assert_eq!(desired_display_orientation(1080, 1920, true), DMDO_270);
    }

    #[test]
    fn flipped_orientation_detection_matches_expected_values() {
        assert!(!is_flipped_orientation(DMDO_DEFAULT));
        assert!(!is_flipped_orientation(DMDO_90));
        assert!(is_flipped_orientation(DMDO_180));
        assert!(is_flipped_orientation(DMDO_270));
    }

    #[test]
    fn mode_dimensions_swap_for_portrait_landscape_transition() {
        assert_eq!(
            mode_dimensions_for_orientation_transition(1920, 1080, DMDO_90, DMDO_DEFAULT),
            (1080, 1920)
        );
        assert_eq!(
            mode_dimensions_for_orientation_transition(1080, 1920, DMDO_DEFAULT, DMDO_90),
            (1920, 1080)
        );
    }

    #[test]
    fn mode_dimensions_stay_same_when_orientation_class_unchanged() {
        assert_eq!(
            mode_dimensions_for_orientation_transition(1920, 1080, DMDO_DEFAULT, DMDO_180),
            (1920, 1080)
        );
        assert_eq!(
            mode_dimensions_for_orientation_transition(1080, 1920, DMDO_90, DMDO_270),
            (1080, 1920)
        );
    }

    #[test]
    fn normalize_resolution_matches_portrait_orientation() {
        assert_eq!(
            normalize_resolution_for_orientation((1920, 1080), DMDO_90),
            (1080, 1920)
        );
        assert_eq!(
            normalize_resolution_for_orientation((1080, 1920), DMDO_90),
            (1080, 1920)
        );
    }

    #[test]
    fn normalize_resolution_matches_landscape_orientation() {
        assert_eq!(
            normalize_resolution_for_orientation((1080, 1920), DMDO_DEFAULT),
            (1920, 1080)
        );
        assert_eq!(
            normalize_resolution_for_orientation((1920, 1080), DMDO_DEFAULT),
            (1920, 1080)
        );
    }
}
