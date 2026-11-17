//! Monitor enumeration (Win32): geometry, DPI, HDR state, SDR white level.

use std::collections::HashMap;
use std::mem::size_of;

use jepret_core::{MonitorId, MonitorInfo, Rect, Result};

use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

pub(crate) fn rect_from_win(r: &RECT) -> Rect {
    Rect::new(
        r.left,
        r.top,
        (r.right - r.left).max(0) as u32,
        (r.bottom - r.top).max(0) as u32,
    )
}

fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

/// Per-GDI-device display config info.
#[derive(Default, Clone)]
struct DisplayExtra {
    friendly: Option<String>,
    hdr: bool,
    sdr_white_nits: Option<f32>,
}

/// Map `\\.\DISPLAYn` -> friendly name / HDR / SDR white via DisplayConfig.
fn display_config_extras() -> HashMap<String, DisplayExtra> {
    let mut out = HashMap::new();
    // SAFETY: plain Win32 calls with correctly sized, initialised out buffers;
    // every struct we hand to DisplayConfigGetDeviceInfo has its header
    // type/size/adapter/id set first.
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm).is_err() {
            return out;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut np,
            paths.as_mut_ptr(),
            &mut nm,
            modes.as_mut_ptr(),
            None,
        )
        .is_err()
        {
            return out;
        }
        for path in &paths[..np as usize] {
            let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            src.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            src.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            src.header.adapterId = path.sourceInfo.adapterId;
            src.header.id = path.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut src.header) != 0 {
                continue;
            }
            let gdi = wide_to_string(&src.viewGdiDeviceName);

            let adapter = path.targetInfo.adapterId;
            let id = path.targetInfo.id;
            let mut extra = DisplayExtra::default();

            let mut tgt = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            tgt.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            tgt.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            tgt.header.adapterId = adapter;
            tgt.header.id = id;
            if DisplayConfigGetDeviceInfo(&mut tgt.header) == 0 {
                let n = wide_to_string(&tgt.monitorFriendlyDeviceName);
                if !n.is_empty() {
                    extra.friendly = Some(n);
                }
            }

            let mut ac = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO::default();
            ac.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO;
            ac.header.size = size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32;
            ac.header.adapterId = adapter;
            ac.header.id = id;
            if DisplayConfigGetDeviceInfo(&mut ac.header) == 0 {
                // bit 1 = advancedColorEnabled
                extra.hdr = ac.Anonymous.value & 0b10 != 0;
            }

            let mut wl = DISPLAYCONFIG_SDR_WHITE_LEVEL::default();
            wl.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL;
            wl.header.size = size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32;
            wl.header.adapterId = adapter;
            wl.header.id = id;
            if DisplayConfigGetDeviceInfo(&mut wl.header) == 0 && wl.SDRWhiteLevel > 0 {
                extra.sdr_white_nits = Some(wl.SDRWhiteLevel as f32 / 1000.0 * 80.0);
            }
            out.insert(gdi, extra);
        }
    }
    out
}

unsafe extern "system" fn enum_cb(hmon: HMONITOR, _dc: HDC, _r: *mut RECT, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the &mut Vec<HMONITOR> passed by `monitors`.
    let v = &mut *(lparam.0 as *mut Vec<HMONITOR>);
    v.push(hmon);
    BOOL(1)
}

pub(crate) fn enumerate() -> Result<Vec<MonitorInfo>> {
    crate::ensure_dpi_aware();
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: callback only pushes into `handles`, which outlives the call.
    unsafe {
        let ok = EnumDisplayMonitors(
            None,
            None,
            Some(enum_cb),
            LPARAM(&mut handles as *mut Vec<HMONITOR> as isize),
        );
        anyhow::ensure!(ok.as_bool(), "EnumDisplayMonitors failed");
    }
    let extras = display_config_extras();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: cbSize set; pointer to MONITORINFOEXW is a valid MONITORINFO prefix.
        let ok = unsafe { GetMonitorInfoW(h, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO) };
        if !ok.as_bool() {
            tracing::warn!("GetMonitorInfoW failed for a monitor, skipping");
            continue;
        }
        let device = wide_to_string(&mi.szDevice);
        let (mut dx, mut dy) = (96u32, 96u32);
        // SAFETY: out pointers are valid locals.
        if unsafe { GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) }.is_err() {
            dx = 96;
        }
        let extra = extras.get(&device).cloned().unwrap_or_default();
        out.push(MonitorInfo {
            id: MonitorId(device.clone()),
            name: extra.friendly.unwrap_or_else(|| device.clone()),
            rect: rect_from_win(&mi.monitorInfo.rcMonitor),
            work_area: rect_from_win(&mi.monitorInfo.rcWork),
            scale: dx as f32 / 96.0,
            primary: mi.monitorInfo.dwFlags & 1 != 0 /* MONITORINFOF_PRIMARY */,
            hdr: extra.hdr,
            sdr_white_nits: extra.sdr_white_nits.unwrap_or(80.0),
        });
    }
    anyhow::ensure!(!out.is_empty(), "no monitors found");
    // Stable order: primary first, then left-to-right / top-to-bottom.
    out.sort_by_key(|m| (!m.primary, m.rect.x, m.rect.y));
    Ok(out)
}
