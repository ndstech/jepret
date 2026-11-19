//! Thin Win32 helpers: cursor position and the work area / DPI of the
//! monitor under it.

/// Cursor and monitor info, all physical px in virtual-desktop coordinates.
#[derive(Debug, Clone, Copy)]
pub struct Env {
    pub cursor: (f32, f32),
    /// x, y, w, h
    pub work: (f32, f32, f32, f32),
    /// DPI scale of that monitor (1.0 = 96 dpi).
    pub scale: f32,
}

#[cfg(windows)]
pub fn query() -> Option<Env> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let mut pt = POINT::default();
    // SAFETY: `pt` is a valid out pointer for the duration of the call.
    unsafe { GetCursorPos(&mut pt) }.ok()?;
    // SAFETY: plain value argument; DEFAULTTONEAREST never yields a null monitor.
    let mon = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `mi` has the correct cbSize and outlives the call.
    if !unsafe { GetMonitorInfoW(mon, &mut mi) }.as_bool() {
        return None;
    }
    let (mut dx, mut dy) = (96u32, 96u32);
    // SAFETY: valid out pointers; on failure the 96 dpi defaults remain.
    let _ = unsafe { GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
    let w = mi.rcWork;
    Some(Env {
        cursor: (pt.x as f32, pt.y as f32),
        work: (
            w.left as f32,
            w.top as f32,
            (w.right - w.left) as f32,
            (w.bottom - w.top) as f32,
        ),
        scale: (dx as f32 / 96.0).max(0.5),
    })
}

#[cfg(not(windows))]
pub fn query() -> Option<Env> {
    None
}

/// Make this process per-monitor-DPI aware before any window exists so the
/// numbers from [`query`] are real physical pixels. Already-set is fine.
#[cfg(windows)]
pub fn init_dpi_awareness() {
    use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    // SAFETY: no pointers; fails harmlessly if awareness is already set.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

#[cfg(not(windows))]
pub fn init_dpi_awareness() {}
