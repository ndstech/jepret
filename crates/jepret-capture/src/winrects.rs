//! Top-level window geometry: foreground window bounds, visible window list.

use std::ffi::c_void;

use jepret_core::{Rect, Result};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::monitors::rect_from_win;

/// Window bounds without the invisible DWM resize border / shadow.
fn frame_bounds(hwnd: HWND) -> Option<Rect> {
    let mut r = RECT::default();
    // SAFETY: out buffer is a RECT and the size passed matches.
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .is_ok();
    if !ok {
        // SAFETY: plain query with a valid out pointer.
        unsafe { GetWindowRect(hwnd, &mut r) }.ok()?;
    }
    let rect = rect_from_win(&r);
    (!rect.is_empty()).then_some(rect)
}

pub(crate) fn foreground_rect() -> Result<Rect> {
    // SAFETY: no-arg query.
    let hwnd = unsafe { GetForegroundWindow() };
    anyhow::ensure!(!hwnd.0.is_null(), "no foreground window");
    frame_bounds(hwnd).ok_or_else(|| anyhow::anyhow!("cannot get foreground window bounds"))
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    // SAFETY: out buffer is a u32 and the size passed matches.
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut c_void,
            std::mem::size_of::<u32>() as u32,
        )
    }
    .is_ok();
    ok && cloaked != 0
}

unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the &mut Vec<Rect> passed by `visible_rects`.
    let out = &mut *(lparam.0 as *mut Vec<Rect>);
    if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        return BOOL(1);
    }
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex & WS_EX_TOOLWINDOW.0 != 0 || is_cloaked(hwnd) {
        return BOOL(1);
    }
    if let Some(r) = frame_bounds(hwnd) {
        out.push(r);
    }
    BOOL(1)
}

/// EnumWindows already walks in Z-order, topmost first.
pub(crate) fn visible_rects() -> Result<Vec<Rect>> {
    let mut out: Vec<Rect> = Vec::new();
    // SAFETY: callback only pushes into `out`, which outlives the call.
    unsafe { EnumWindows(Some(enum_cb), LPARAM(&mut out as *mut Vec<Rect> as isize)) }?;
    Ok(out)
}
