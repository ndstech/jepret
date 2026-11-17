//! `RegionBorder`: click-through recording indicator around a region,
//! excluded from capture.
//!
//! The window lives on its own thread (with its own message loop) so callers
//! do not have to pump messages. The frame is drawn *outside* the region so
//! the recorded/captured pixels are never covered. Visuals: 2 px red frame,
//! a small "Merekam W × H" label above it and (for [`RegionBorder::show_recording`])
//! a timer pill below. The window is a per-pixel-alpha layered window drawn
//! with Direct2D; the timer is driven by a 1 s Win32 timer.

use std::ffi::c_void;
use std::sync::mpsc;
use std::sync::Once;
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::{anyhow, ensure};
use jepret_core::{Rect, Result};
use tracing::warn;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1SolidColorBrush, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_WEIGHT_MEDIUM,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetMonitorInfoW, MonitorFromRect, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC,
    HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetWindowLongPtrW, KillTimer, PostMessageW,
    PostQuitMessage, RegisterClassExW, SetTimer, SetWindowDisplayAffinity, SetWindowLongPtrW, ShowWindow,
    TranslateMessage, UpdateLayeredWindow, GWLP_USERDATA, MSG, SW_SHOWNOACTIVATE, ULW_ALPHA, WDA_EXCLUDEFROMCAPTURE,
    WM_CLOSE, WM_DESTROY, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::style::{self, format_timer};

const CLASS_NAME: PCWSTR = w!("JepretRegionBorderWindow");
const TIMER_ID: usize = 1;
/// Frame thickness in physical px.
pub const THICKNESS: u32 = 2;

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

/// Where the pieces of the indicator go (virtual-desktop px).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layout {
    /// Bounding rect of everything = the window rect.
    window: Rect,
    /// Outer edge of the frame.
    frame: Rect,
    label: Rect,
    pill: Option<Rect>,
}

const LABEL_GAP: i32 = 4;
const PILL_GAP: i32 = 12;

/// Place the label (above the frame, top-left aligned) and the optional timer
/// pill (below the frame, centred). When there is no room inside `bounds`
/// (usually the monitor) they move just inside the region instead.
fn layout(region: Rect, label: (u32, u32), pill: Option<(u32, u32)>, bounds: Rect) -> Layout {
    let frame = outer_rect(region, THICKNESS);
    let t = THICKNESS as i32;

    let (lw, lh) = (label.0 as i32, label.1 as i32);
    let mut ly = frame.y - lh - LABEL_GAP;
    let mut lx = frame.x;
    if ly < bounds.y {
        ly = frame.y + t + LABEL_GAP;
        lx = frame.x + t + LABEL_GAP;
    }
    lx = lx.clamp(bounds.x, (bounds.right() - lw).max(bounds.x));
    let label_r = Rect::new(lx, ly, label.0, label.1);

    let pill_r = pill.map(|(pw, ph)| {
        let (pw_i, ph_i) = (pw as i32, ph as i32);
        let mut py = frame.bottom() + PILL_GAP;
        if py + ph_i > bounds.bottom() {
            py = frame.bottom() - t - PILL_GAP - ph_i;
        }
        let px = (region.x + region.width as i32 / 2 - pw_i / 2).clamp(bounds.x, (bounds.right() - pw_i).max(bounds.x));
        Rect::new(px, py, pw, ph)
    });

    let mut window = frame.union(&label_r);
    if let Some(p) = pill_r {
        window = window.union(&p);
    }
    Layout { window, frame, label: label_r, pill: pill_r }
}

impl RegionBorder {
    /// Border with the "Merekam W × H" label.
    pub fn show(rect: Rect) -> Result<Self> {
        Self::spawn(rect, None)
    }

    /// Border + label + a timer pill below the region: red dot, running
    /// `mm:ss` (counted from this call, refreshed once a second) and
    /// `stop_hint` (e.g. "Ctrl+Alt+PrtSc untuk stop").
    pub fn show_recording(rect: Rect, stop_hint: &str) -> Result<Self> {
        Self::spawn(rect, Some(stop_hint.to_string()))
    }

    fn spawn(rect: Rect, stop_hint: Option<String>) -> Result<Self> {
        ensure!(!rect.is_empty(), "region border rect is empty");
        let (tx, rx) = mpsc::channel::<std::result::Result<usize, String>>();
        let thread = std::thread::Builder::new()
            .name("jepret-region-border".into())
            .spawn(move || thread_main(rect, stop_hint, tx))?;
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
        // SAFETY: valid WNDCLASSEXW; class name is a static wide string.
        unsafe {
            let hinst = match GetModuleHandleW(None) {
                Ok(h) => h,
                Err(e) => {
                    err = Some(e.to_string());
                    return;
                }
            };
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: hinst.into(),
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

/// Everything needed to repaint the indicator; lives on the window thread.
struct Painter {
    hwnd: HWND,
    layout: Layout,
    label_text: String,
    stop_hint: Option<String>,
    start: Instant,
    dwrite: IDWriteFactory,
    rt: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
    label_fmt: IDWriteTextFormat,
    timer_fmt: IDWriteTextFormat,
    hint_fmt: IDWriteTextFormat,
    mem_dc: HDC,
    dib: HBITMAP,
    old: HGDIOBJ,
}

impl Drop for Painter {
    fn drop(&mut self) {
        // SAFETY: handles were created by us in `Painter::new` and are not
        // used afterwards; the DIB is deselected before deletion.
        unsafe {
            SelectObject(self.mem_dc, self.old);
            let _ = DeleteObject(self.dib);
            let _ = DeleteDC(self.mem_dc);
        }
    }
}

const DOT: f32 = 10.0;
const PILL_H: f32 = 36.0;

impl Painter {
    /// Measure text, compute the layout and build the render target.
    /// Returns the painter and the window rect.
    unsafe fn new(rect: Rect, stop_hint: Option<String>) -> std::result::Result<(Self, Rect), String> {
        let e = |what: &'static str| move |err: windows::core::Error| format!("{what}: {err}");
        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).map_err(e("DWriteCreateFactory"))?;
        let ui = style::pick_family(&dwrite, &style::UI_FONTS);
        let mono = style::pick_family(&dwrite, &style::MONO_FONTS);
        let label_fmt = style::text_format(&dwrite, ui, 11.0, DWRITE_FONT_WEIGHT_SEMI_BOLD).map_err(e("font"))?;
        let timer_fmt = style::text_format(&dwrite, mono, 14.0, DWRITE_FONT_WEIGHT_MEDIUM).map_err(e("font"))?;
        let hint_fmt = style::text_format(&dwrite, ui, 12.0, DWRITE_FONT_WEIGHT_NORMAL).map_err(e("font"))?;

        let label_text = format!("Merekam {} \u{d7} {}", rect.width, rect.height);
        let size_of = |fmt: &IDWriteTextFormat, s: &str| {
            style::text_layout(&dwrite, fmt, s).map(|(_, w, h)| (w, h)).unwrap_or((0.0, 0.0))
        };
        let (lw, lh) = size_of(&label_fmt, &label_text);
        let label = ((lw + 16.0).ceil() as u32, (lh + 4.0).ceil() as u32);
        let pill = stop_hint.as_ref().map(|hint| {
            // Reserve room for "000:00" so the pill never resizes.
            let (tw, _) = size_of(&timer_fmt, "000:00");
            let (hw, _) = size_of(&hint_fmt, hint);
            ((14.0 + DOT + 12.0 + tw + 12.0 + hw + 14.0).ceil() as u32, PILL_H as u32)
        });

        // Keep the label/pill on the monitor that holds the region.
        let rc = RECT { left: rect.x, top: rect.y, right: rect.right(), bottom: rect.bottom() };
        let hmon = MonitorFromRect(&rc, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let bounds = if GetMonitorInfoW(hmon, &mut mi).as_bool() {
            let m = mi.rcMonitor;
            Rect::new(m.left, m.top, (m.right - m.left) as u32, (m.bottom - m.top) as u32)
        } else {
            outer_rect(rect, 4096)
        };
        let lay = layout(rect, label, pill, bounds);

        let factory: windows::Win32::Graphics::Direct2D::ID2D1Factory =
            D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).map_err(e("D2D1CreateFactory"))?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let rt = factory.CreateDCRenderTarget(&props).map_err(e("CreateDCRenderTarget"))?;
        let brush = rt.CreateSolidColorBrush(&style::WHITE, None).map_err(e("CreateSolidColorBrush"))?;

        let mem_dc = CreateCompatibleDC(None);
        if mem_dc.is_invalid() {
            return Err("CreateCompatibleDC failed".into());
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: lay.window.width as i32,
                biHeight: -(lay.window.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let dib = match CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(err) => {
                let _ = DeleteDC(mem_dc);
                return Err(format!("CreateDIBSection: {err}"));
            }
        };
        let old = SelectObject(mem_dc, dib);
        let window = lay.window;
        Ok((
            Self {
                hwnd: HWND::default(),
                layout: lay,
                label_text,
                stop_hint,
                start: Instant::now(),
                dwrite,
                rt,
                brush,
                label_fmt,
                timer_fmt,
                hint_fmt,
                mem_dc,
                dib,
                old,
            },
            window,
        ))
    }

    fn elapsed_secs(&self) -> u64 {
        // Small bias so a timer tick landing just before the second boundary
        // does not show the previous second again.
        (self.start.elapsed().as_millis() as u64 + 50) / 1000
    }

    unsafe fn text(&self, fmt: &IDWriteTextFormat, s: &str, x: f32, y: f32, color: &D2D1_COLOR_F) {
        if let Some((l, _, _)) = style::text_layout(&self.dwrite, fmt, s) {
            self.brush.SetColor(color);
            self.rt.DrawTextLayout(D2D_POINT_2F { x, y }, &l, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
    }

    /// Redraw into the DIB and push it to the layered window.
    unsafe fn render(&self) {
        let win = self.layout.window;
        let (ox, oy) = (win.x as f32, win.y as f32);
        let rf = |r: Rect| D2D_RECT_F {
            left: r.x as f32 - ox,
            top: r.y as f32 - oy,
            right: r.right() as f32 - ox,
            bottom: r.bottom() as f32 - oy,
        };
        let rc = RECT { left: 0, top: 0, right: win.width as i32, bottom: win.height as i32 };
        if self.rt.BindDC(self.mem_dc, &rc).is_err() {
            return;
        }
        self.rt.BeginDraw();
        self.rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));

        // Frame (drawn outside the region).
        let frame = rf(self.layout.frame);
        let half = THICKNESS as f32 / 2.0;
        let line = D2D_RECT_F { left: frame.left + half, top: frame.top + half, right: frame.right - half, bottom: frame.bottom - half };
        self.brush.SetColor(&style::DANGER);
        self.rt.DrawRoundedRectangle(
            &D2D1_ROUNDED_RECT { rect: line, radiusX: 2.0, radiusY: 2.0 },
            &self.brush,
            THICKNESS as f32,
            None,
        );

        // Label.
        let l = rf(self.layout.label);
        self.brush.SetColor(&style::DANGER);
        self.rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: l, radiusX: 4.0, radiusY: 4.0 }, &self.brush);
        self.text(&self.label_fmt, &self.label_text, l.left + 8.0, l.top + 2.0, &style::ON_DANGER);

        // Timer pill.
        if let (Some(p), Some(hint)) = (self.layout.pill, self.stop_hint.as_deref()) {
            let p = rf(p);
            let rad = (p.bottom - p.top) / 2.0;
            let rr = D2D1_ROUNDED_RECT { rect: p, radiusX: rad, radiusY: rad };
            self.brush.SetColor(&style::TIMER_BG);
            self.rt.FillRoundedRectangle(&rr, &self.brush);
            self.brush.SetColor(&style::PANEL_BORDER);
            self.rt.DrawRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F { left: p.left + 0.5, top: p.top + 0.5, right: p.right - 0.5, bottom: p.bottom - 0.5 },
                    radiusX: rad,
                    radiusY: rad,
                },
                &self.brush,
                1.0,
                None,
            );
            let cy = (p.top + p.bottom) / 2.0;
            let mut x = p.left + 14.0;
            self.brush.SetColor(&style::DANGER);
            self.rt.FillEllipse(
                &D2D1_ELLIPSE { point: D2D_POINT_2F { x: x + DOT / 2.0, y: cy }, radiusX: DOT / 2.0, radiusY: DOT / 2.0 },
                &self.brush,
            );
            x += DOT + 12.0;
            let timer = format_timer(self.elapsed_secs());
            let (tw, th) = style::text_layout(&self.dwrite, &self.timer_fmt, "000:00").map(|(_, w, h)| (w, h)).unwrap_or((0.0, 0.0));
            self.text(&self.timer_fmt, &timer, x, cy - th / 2.0, &style::TEXT);
            x += tw + 12.0;
            let hh = style::text_layout(&self.dwrite, &self.hint_fmt, hint).map(|(_, _, h)| h).unwrap_or(0.0);
            self.text(&self.hint_fmt, hint, x, cy - hh / 2.0, &style::MUTED);
        }

        if let Err(e) = self.rt.EndDraw(None, None) {
            warn!("region border: EndDraw failed: {e}");
            return;
        }
        let pt_dst = POINT { x: win.x, y: win.y };
        let size = SIZE { cx: win.width as i32, cy: win.height as i32 };
        let pt_src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        if let Err(e) = UpdateLayeredWindow(
            self.hwnd,
            None,
            Some(&pt_dst),
            Some(&size),
            self.mem_dc,
            Some(&pt_src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        ) {
            warn!("region border: UpdateLayeredWindow failed: {e}");
        }
    }
}

fn thread_main(rect: Rect, stop_hint: Option<String>, tx: mpsc::Sender<std::result::Result<usize, String>>) {
    // SAFETY: Win32 calls with valid arguments; the window is created and
    // destroyed on this thread, which also pumps its messages. The painter
    // box is owned by this function and only reachable through the window's
    // GWLP_USERDATA until the loop ends.
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if let Err(e) = register_class() {
            let _ = tx.send(Err(e));
            return;
        }
        let recording = stop_hint.is_some();
        let (painter, outer) = match Painter::new(rect, stop_hint) {
            Ok(p) => p,
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        };
        let mut painter = Box::new(painter);
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
        painter.hwnd = hwnd;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *painter as *mut Painter as isize);
        // Keep the indicator out of screenshots / recordings (Windows 10 2004+).
        if let Err(e) = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) {
            warn!("region border: WDA_EXCLUDEFROMCAPTURE not applied: {e}");
        }
        painter.render();
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        if recording && SetTimer(hwnd, TIMER_ID, 1000, None) == 0 {
            warn!("region border: SetTimer failed, timer will not advance");
        }
        let _ = tx.send(Ok(hwnd.0 as usize));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // Window is gone; drop the painter only after the loop.
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        drop(painter);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if wp.0 == TIMER_ID => {
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Painter;
            // SAFETY: pointer set in `thread_main` to a Box that outlives the
            // message loop; cleared before the Box is dropped.
            if let Some(p) = p.as_ref() {
                p.render();
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, TIMER_ID);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: Rect = Rect::new(0, 0, 1920, 1080);

    #[test]
    fn outer_rect_surrounds_region() {
        let r = Rect::new(-10, 20, 100, 50);
        let o = outer_rect(r, 3);
        assert_eq!(o, Rect::new(-13, 17, 106, 56));
        assert_eq!(o.intersect(&r), Some(r));
    }

    #[test]
    fn label_above_and_pill_below_centered() {
        let region = Rect::new(400, 300, 640, 360);
        let l = layout(region, (120, 18), Some((300, 36)), MON);
        assert_eq!(l.frame, outer_rect(region, THICKNESS));
        assert_eq!(l.label, Rect::new(398, 298 - 18 - LABEL_GAP, 120, 18));
        let p = l.pill.unwrap();
        assert_eq!(p.y, l.frame.bottom() + PILL_GAP);
        assert_eq!(p.x + 150, 400 + 320);
        // Window covers everything and never the region interior's pixels differently:
        assert_eq!(l.window.intersect(&l.pill.unwrap()), l.pill);
        assert_eq!(l.window.intersect(&l.label), Some(l.label));
        assert_eq!(l.window.intersect(&l.frame), Some(l.frame));
    }

    #[test]
    fn label_and_pill_move_inside_at_screen_edges() {
        let region = Rect::new(0, 0, 1920, 1080);
        let l = layout(region, (120, 18), Some((300, 36)), MON);
        assert!(MON.intersect(&l.label) == Some(l.label));
        assert!(l.label.y >= THICKNESS as i32);
        let p = l.pill.unwrap();
        assert!(MON.intersect(&p) == Some(p));
        assert!(p.bottom() <= region.bottom());
    }

    #[test]
    fn no_pill_without_recording() {
        let l = layout(Rect::new(100, 100, 50, 50), (80, 18), None, MON);
        assert!(l.pill.is_none());
        assert_eq!(l.window, l.frame.union(&l.label));
    }
}
