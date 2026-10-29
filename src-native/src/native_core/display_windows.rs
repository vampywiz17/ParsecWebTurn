//! Monitor configuration queries, only on panel open/move/display change.
//! No display changes, polling, graphics device or frame work.
use super::display_timing::Display;
use std::{mem::size_of, ptr::null_mut};
use windows_sys::Win32::{
    Devices::Display::*,
    Foundation::{ERROR_INSUFFICIENT_BUFFER, HWND},
    Graphics::Gdi::*,
};

/// Match the window's actual monitor GDI source name. On ambiguous clone paths
/// with different refresh rates, report unknown rather than picking one.
unsafe fn refresh(device: &[u16; 32]) -> Option<f64> {
    for _ in 0..3 {
        let (mut path_count, mut mode_count) = (0, 0);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count) != 0
            || !(1..=256).contains(&path_count)
            || mode_count > 1024
        {
            return None;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count.max(1) as usize];
        let status = QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            null_mut(),
        );
        if status == ERROR_INSUFFICIENT_BUFFER {
            continue;
        }
        if status != 0 {
            return None;
        }
        let mut found: Option<f64> = None;
        for path in paths.iter().take(path_count as usize) {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                    adapterId: path.sourceInfo.adapterId,
                    id: path.sourceInfo.id,
                },
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0
                || source.viewGdiDeviceName != *device
            {
                continue;
            }
            let ratio = path.targetInfo.refreshRate;
            if ratio.Denominator == 0 || ratio.Numerator == 0 {
                return None;
            }
            let hz = f64::from(ratio.Numerator) / f64::from(ratio.Denominator);
            if !(1.0..=1000.0).contains(&hz)
                || found.is_some_and(|previous| (previous - hz).abs() > 0.001)
            {
                return None;
            }
            found = Some(hz);
        }
        return found;
    }
    None
}
pub unsafe fn query(hwnd: HWND) -> Display {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(
        MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
        &mut info.monitorInfo,
    ) == 0
    {
        return Display::default();
    }
    let mut result = Display {
        hz: refresh(&info.szDevice),
        ..Default::default()
    };
    let mut current = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    if EnumDisplaySettingsW(info.szDevice.as_ptr(), ENUM_CURRENT_SETTINGS, &mut current) == 0 {
        return result;
    }
    for index in 0..4096 {
        let mut mode = DEVMODEW {
            dmSize: size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        if EnumDisplaySettingsW(info.szDevice.as_ptr(), index, &mut mode) == 0 {
            break;
        }
        if mode.dmPelsWidth == current.dmPelsWidth
            && mode.dmPelsHeight == current.dmPelsHeight
            && mode.dmBitsPerPel == current.dmBitsPerPel
            && mode.Anonymous2.dmDisplayFlags & DM_INTERLACED == 0
            && (2..=1000).contains(&mode.dmDisplayFrequency)
        {
            result.supported_hz.push(mode.dmDisplayFrequency);
        }
    }
    result.supported_hz.sort_unstable();
    result.supported_hz.dedup();
    result
}
