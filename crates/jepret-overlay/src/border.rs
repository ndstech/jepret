//! `RegionBorder`: click-through frame around a region, excluded from capture.
//!
//! The window lives on its own thread (with its own message loop) so callers
//! do not have to pump messages. The frame is drawn *outside* the region so
//! the recorded/captured pixels are never covered.

use std::ffi::c_void;
use std::sync::mpsc;
use std::sync::Once;
use std::thread::JoinHandle;

use anyhow::{anyhow, ensure};
use jepret_core::{Rect, Result};
use tracing::warn;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{CombineRgn, CreateRectRgn, CreateSolidBrush, SetWindowRgn, RGN_DIFF};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostMessageW, PostQuitMessage, RegisterClassExW,
    SetLayeredWindowAttributes, SetWindowDisplayAffinity, ShowWindow, TranslateMessage, LWA_ALPHA, MSG,
    SW_SHOWNOACTIVATE, WDA_EXCLUDEFROMCAPTURE, WM_CLOSE, WM_DESTROY, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

const CLASS_NAME: PCWSTR = w!("JepretRegionBorderWindow");
/// Frame thickness in physical px.
pub const THICKNESS: u32 = 3;

/// Non-interactive click-through border drawn around `rect` while
/// recording / scroll-capturing. Dropping the handle removes it.
pub struct RegionBorder {
    hwnd: usize,
    thread: Option<JoinHandle<()>>,
}

/// Outer window rect for a frame of `thickness` px around `rect` (pure).
pub fn outer_rect(rect: Rect, thickness: u32) -> Rect {
    let t = thickness as i32;
    Rect::new(rect.x - t, rect.y - t, rect.width + 2 * thickness, rect.height + 2 * thickness)
}

impl RegionBorder {
    pub fn show(rect: Rect) -> Result<Self> {
        ensure!(!rect.is_empty(), "region border rect is empty");
        let (tx, rx) = mpsc::channel::<std::result::Result<usize, String>>();
        let thread = std::thread::Builder::new()
            .name("jepret-region-border".into())
            .spawn(move || thread_main(rect, tx))?;
        match rx.recv() {
            Ok(Ok(hwnd)) => Ok(Self { hwnd, thread: Some(thread) }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(anyhow!(e))
            }
            Err(_) => Err(anyhow!("region border thread exited before creating its window")),
        }
    }
}

impl Drop for RegionBorder {
    fn drop(&mut self) {
        // SAFETY: PostMessageW to a window owned by another thread is allowed;
        // if the window is already gone the call just fails.
        unsafe {
            let _ = PostMessageW(HWND(self.hwnd as *mut c_void), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn register_class() -> std::result::Result<(), String> {
    static ONCE: Once = Once::new();
    let mut err = None;
    ONCE.call_once(|| {
        // SAFETY: valid WNDCLASSEXW; brush is owned by the class for the process lifetime.
        unsafe {
            let hinst = match GetModuleHandleW(None) {
                Ok(h) => h,
                Err(e) => {
                    err = Some(e.to_string());
                    return;
                }
            };
            // COLORREF is 0x00BBGGRR: accent blue (59, 130, 246).
            let brush = CreateSolidBrush(COLORREF(0x00F6_823B));
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: hinst.into(),
                hbrBackground: brush,
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            if RegisterClassExW(&wc) == 0 {
                err = Some(format!("RegisterClassExW failed: {}", windows::core::Error::from_win32()));
            }
        }
    });
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn thread_main(rect: Rect, tx: mpsc::Sender<std::result::Result<usize, String>>) {
    // SAFETY: Win32 calls with valid arguments; the window is created and
    // destroyed on this thread, which also pumps its messages.
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if let Err(e) = register_class() {
            let _ = tx.send(Err(e));
            return;
        }
        let outer = outer_rect(rect, THICKNESS);
        let hinst = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(e) => {
                let _ = tx.send(Err(e.to_string()));
                return;
            }
        };
        let hwnd = match CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            CLASS_NAME,
            w!("Jepret region border"),
            WS_POPUP,
            outer.x,
            outer.y,
            outer.width as i32,
            outer.height as i32,
            None,
            None,
            hinst,
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                let _ = tx.send(Err(format!("CreateWindowExW: {e}")));
                return;
            }
        };
        // Layered windows draw nothing until attributes are set.
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        // Keep the frame out of screenshots / recordings (Windows 10 2004+).
        if let Err(e) = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) {
            warn!("region border: WDA_EXCLUDEFROMCAPTURE not applied: {e}");
        }
        // Frame = outer rect minus inner rect (window-relative coords).
        let t = THICKNESS as i32;
        let region = CreateRectRgn(0, 0, outer.width as i32, outer.height as i32);
        let hole = CreateRectRgn(t, t, t + rect.width as i32, t + rect.height as i32);
        CombineRgn(region, region, hole, RGN_DIFF);
        // Ownership of `region` passes to the window; `hole` is a leftover.
        SetWindowRgn(hwnd, region, true);
        let _ = windows::Win32::Graphics::Gdi::DeleteObject(hole);

        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = tx.send(Ok(hwnd.0 as usize));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outer_rect_surrounds_region() {
        let r = Rect::new(-10, 20, 100, 50);
        let o = outer_rect(r, 3);
        assert_eq!(o, Rect::new(-13, 17, 106, 56));
        assert_eq!(o.intersect(&r), Some(r));
    }
}
