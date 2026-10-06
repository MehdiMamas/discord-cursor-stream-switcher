//! Lists the active monitors and finds the virtual stream display.

use crate::geometry::Rect;
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
    QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
use windows::core::BOOL;

/// EDID manufacturer + product code of the VirtualDrivers Virtual Display Driver ("MTT", 0x1337).
/// It shows up in the monitor device path, e.g. `\\?\DISPLAY#MTT1337#...`.
pub const VDD_EDID_ID: &str = "MTT1337";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    /// GDI device name such as `\\.\DISPLAY1`. Matches DXGI_OUTPUT_DESC::DeviceName.
    pub gdi_name: String,
    /// Position on the virtual desktop, in physical pixels.
    pub rect: Rect,
    /// Friendly name from the EDID, such as "DELL U2720Q".
    pub name: String,
    /// Stable ID (monitor device path) used to remember hidden monitors across reboots.
    pub id: String,
    /// True for the virtual display the stream is drawn on.
    pub is_stream: bool,
    pub primary: bool,
}

impl Monitor {
    pub fn label(&self) -> String {
        let short = self.gdi_name.trim_start_matches(r"\\.\");
        format!("{} ({short}, {}x{})", self.name, self.rect.width(), self.rect.height())
    }
}

pub fn wide_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Lists active monitors. `stream_display` is the config value: "auto" picks the Virtual
/// Display Driver screen; anything else is matched against the GDI name.
pub fn enumerate(stream_display: &str) -> Vec<Monitor> {
    let targets = display_targets();
    let mut monitors = Vec::new();
    for (hmon, rect, primary, gdi_name) in gdi_monitors() {
        let _ = hmon;
        let target = targets.iter().find(|t| t.0.eq_ignore_ascii_case(&gdi_name));
        let (name, id) = match target {
            Some((_, name, path)) => (
                if name.is_empty() { "Display".to_owned() } else { name.clone() },
                if path.is_empty() { gdi_name.clone() } else { path.clone() },
            ),
            None => ("Display".to_owned(), gdi_name.clone()),
        };
        let is_stream = if stream_display.eq_ignore_ascii_case("auto") {
            id.to_ascii_uppercase().contains(VDD_EDID_ID)
        } else {
            gdi_name.eq_ignore_ascii_case(stream_display.trim())
        };
        monitors.push(Monitor { gdi_name, rect, name, id, is_stream, primary });
    }
    // Only one stream display: the first match wins.
    let mut seen = false;
    for m in &mut monitors {
        if m.is_stream {
            m.is_stream = !seen;
            seen = true;
        }
    }
    monitors
}

fn gdi_monitors() -> Vec<(HMONITOR, Rect, bool, String)> {
    unsafe extern "system" fn callback(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the &mut Vec passed below and outlives the enumeration.
        let list = unsafe { &mut *(data.0 as *mut Vec<(HMONITOR, Rect, bool, String)>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: cbSize marks the struct as MONITORINFOEXW, which starts with MONITORINFO.
        if unsafe { GetMonitorInfoW(hmon, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) }.as_bool() {
            let r = info.monitorInfo.rcMonitor;
            list.push((
                hmon,
                Rect::new(r.left, r.top, r.right, r.bottom),
                info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                wide_to_string(&info.szDevice),
            ));
        }
        BOOL(1)
    }
    let mut list: Vec<(HMONITOR, Rect, bool, String)> = Vec::new();
    // SAFETY: the callback only touches `list` while EnumDisplayMonitors runs.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(callback), LPARAM(&mut list as *mut _ as isize));
    }
    list
}

/// (GDI source name, monitor friendly name, monitor device path) for each active path.
fn display_targets() -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = Vec::new();
    let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> = Vec::new();
    // The topology can change between the size query and the query itself; retry a few times.
    for _ in 0..4 {
        let (mut np, mut nm) = (0u32, 0u32);
        // SAFETY: plain out-parameters.
        if unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) } != ERROR_SUCCESS {
            return out;
        }
        paths.resize(np as usize, Default::default());
        modes.resize(nm as usize, Default::default());
        // SAFETY: buffers are sized to the counts passed in.
        let r = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut np,
                paths.as_mut_ptr(),
                &mut nm,
                modes.as_mut_ptr(),
                None,
            )
        };
        if r == ERROR_SUCCESS {
            paths.truncate(np as usize);
            break;
        }
        paths.clear();
        if r != ERROR_INSUFFICIENT_BUFFER {
            return out;
        }
    }
    for path in &paths {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
        source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
        source.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
        source.header.adapterId = path.sourceInfo.adapterId;
        source.header.id = path.sourceInfo.id;
        let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
        target.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
        target.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
        target.header.adapterId = path.targetInfo.adapterId;
        target.header.id = path.targetInfo.id;
        // SAFETY: each header's size field matches the struct it heads.
        let ok = unsafe {
            DisplayConfigGetDeviceInfo(&mut source.header) == 0
                && DisplayConfigGetDeviceInfo(&mut target.header) == 0
        };
        if ok {
            out.push((
                wide_to_string(&source.viewGdiDeviceName),
                wide_to_string(&target.monitorFriendlyDeviceName),
                wide_to_string(&target.monitorDevicePath),
            ));
        }
    }
    out
}
