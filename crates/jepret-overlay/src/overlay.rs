//! Win32 + Direct2D glue for the selection overlay. All interaction logic
//! lives in [`crate::controller`]; this file only translates messages,
//! owns the GPU resources and paints.

use std::ffi::c_void;
use std::sync::Once;
use std::time::Instant;

use anyhow::{bail, ensure, Context};
use jepret_core::{Image, MonitorInfo, Point, Rect, Result};
use tracing::{debug, warn};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_UNKNOWN, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F,
    D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush, ID2D1StrokeStyle,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1_BITMAP_PROPERTIES,
    D2D1_CAP_STYLE_FLAT, D2D1_DASH_STYLE_DASH, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_LAYER_OPTIONS_NONE,
    D2D1_LAYER_PARAMETERS, D2D1_LINE_JOIN_MITER, D2D1_PRESENT_OPTIONS_IMMEDIATELY, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_METRICS,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_UNKNOWN};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, UpdateWindow, PAINTSTRUCT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT,
    VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageTime, GetMessageW, GetWindowLongPtrW,
    LoadCursorW, PostQuitMessage, RegisterClassExW, SetCursorPos, SetForegroundWindow,
    SetWindowLongPtrW, ShowWindow, TranslateMessage, CREATESTRUCTW, CS_DBLCLKS, CS_OWNDC, GWLP_USERDATA, IDC_CROSS, MSG,
    SW_SHOW, WA_INACTIVE, WM_ACTIVATE, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_NCCREATE, WM_PAINT, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSEXW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

use crate::accel::KeyAccel;
use crate::controller::{hex_color, hint_rows, Controller, KbMode, Key, Mods, Outcome};
use crate::magnifier;
use crate::style;
use crate::OverlayOptions;

const CLASS_NAME: PCWSTR = w!("JepretOverlayWindow");
/// D2DERR_RECREATE_TARGET: device lost, rebuild render target + bitmap.
const D2DERR_RECREATE_TARGET: windows::core::HRESULT = windows::core::HRESULT(0x8899000Cu32 as i32);

/// Text formats used by the overlay (Segoe UI Variable / Cascadia Mono with
/// fallbacks, see [`style`]).
struct Fonts {
    /// 12 px UI text.
    ui: IDWriteTextFormat,
    /// 11 px mono (kbd chips, magnifier).
    mono_s: IDWriteTextFormat,
    /// 13 px semibold mono (size label).
    mono_b: IDWriteTextFormat,
}

/// GPU resources; dropped and rebuilt on device loss.
struct Gfx {
    factory: ID2D1Factory,
    rt: ID2D1HwndRenderTarget,
    bitmap: ID2D1Bitmap,
    brush: ID2D1SolidColorBrush,
    dwrite: IDWriteFactory,
    fonts: Fonts,
    /// Dashed stroke for the snap-window outline.
    dash: ID2D1StrokeStyle,
}

struct State {
    hwnd: HWND,
    origin: Rect,
    ctl: Controller,
    opts: OverlayOptions,
    monitors: Vec<MonitorInfo>,
    gfx: Option<Gfx>,
    /// Valid for the whole message loop in [`run`]; only read to rebuild the
    /// bitmap after device loss.
    frozen: *const Image,
    activated: bool,
    /// Set once a result is decided; later events (e.g. the WM_ACTIVATE sent
    /// while the window is being destroyed) must not overwrite it.
    finished: bool,
    result: Option<Rect>,
    /// Hold-to-accelerate state for the arrow keys (driven by key messages only).
    accel: KeyAccel,
}

/// Restores the thread DPI awareness on drop.
struct DpiGuard(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT);

impl DpiGuard {
    fn per_monitor_v2() -> Self {
        // SAFETY: plain Win32 call without pointers.
        Self(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for DpiGuard {
    fn drop(&mut self) {
        if !self.0 .0.is_null() {
            // SAFETY: restoring the context returned by the earlier call.
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
}

fn register_class() -> Result<()> {
    static ONCE: Once = Once::new();
    let mut err = None;
    ONCE.call_once(|| {
        // SAFETY: valid WNDCLASSEXW; class name is a static wide string.
        unsafe {
            let hinst = match GetModuleHandleW(None) {
                Ok(h) => h,
                Err(e) => {
                    err = Some(anyhow::Error::from(e));
                    return;
                }
            };
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_OWNDC | CS_DBLCLKS,
                lpfnWndProc: Some(wndproc),
                hInstance: hinst.into(),
                hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            if RegisterClassExW(&wc) == 0 {
                err = Some(anyhow::anyhow!("RegisterClassExW failed: {}", windows::core::Error::from_win32()));
            }
        }
    });
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

pub(crate) fn run(
    frozen: &Image,
    origin: Rect,
    monitors: &[MonitorInfo],
    opts: &OverlayOptions,
) -> Result<Option<Rect>> {
    let t0 = Instant::now();
    ensure!(!origin.is_empty(), "overlay origin rect is empty");
    ensure!(
        frozen.width == origin.width && frozen.height == origin.height,
        "frozen image is {}x{} but origin is {}x{}",
        frozen.width,
        frozen.height,
        origin.width,
        origin.height
    );
    let _dpi = DpiGuard::per_monitor_v2();
    register_class()?;

    let mut state = Box::new(State {
        hwnd: HWND::default(),
        origin,
        ctl: Controller::new(
            origin,
            opts.initial,
            opts.window_rects.clone(),
            monitors.iter().map(|m| m.rect).collect(),
            opts.magnifier,
        ),
        opts: opts.clone(),
        monitors: monitors.to_vec(),
        gfx: None,
        frozen: frozen as *const Image,
        activated: false,
        finished: false,
        result: None,
        accel: KeyAccel::new(),
    });

    // SAFETY: `state` outlives the window (we pump messages until it is
    // destroyed below) and is only accessed from this thread via the
    // GWLP_USERDATA pointer handed over in WM_NCCREATE.
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            CLASS_NAME,
            w!("Jepret overlay"),
            WS_POPUP,
            origin.x,
            origin.y,
            origin.width as i32,
            origin.height as i32,
            None,
            None,
            hinst,
            Some(&mut *state as *mut State as *const c_void),
        )?;
        state.hwnd = hwnd;
        match create_gfx(hwnd, frozen) {
            Ok(g) => state.gfx = Some(g),
            Err(e) => {
                let _ = DestroyWindow(hwnd);
                return Err(e.context("failed to create Direct2D resources"));
            }
        }
        if let Ok(pt) = cursor_pos() {
            state.ctl.mouse_move(pt);
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(hwnd);
        let _ = UpdateWindow(hwnd);
        debug!(elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0, "overlay visible");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(state.result)
}

fn cursor_pos() -> Result<Point> {
    let mut pt = POINT::default();
    // SAFETY: pt is a valid out pointer.
    unsafe { GetCursorPos(&mut pt)? };
    Ok(Point::new(pt.x, pt.y))
}

unsafe fn create_gfx(hwnd: HWND, frozen: &Image) -> Result<Gfx> {
    let size = D2D_SIZE_U { width: frozen.width, height: frozen.height };
    if frozen.width > 16384 || frozen.height > 16384 {
        bail!("virtual desktop {}x{} exceeds the Direct2D bitmap limit", frozen.width, frozen.height);
    }
    let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
    let props = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_UNKNOWN, alphaMode: D2D1_ALPHA_MODE_UNKNOWN },
        // 96 dpi: 1 DIP == 1 physical pixel, so we can draw in pixel coords.
        dpiX: 96.0,
        dpiY: 96.0,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    };
    let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
        hwnd,
        pixelSize: size,
        presentOptions: D2D1_PRESENT_OPTIONS_IMMEDIATELY,
    };
    let rt = factory.CreateHwndRenderTarget(&props, &hwnd_props)?;
    // RGBA8 straight, alpha ignored (screenshot is opaque): uploaded as-is,
    // no CPU-side swizzle needed.
    let bp = D2D1_BITMAP_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_R8G8B8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
        dpiX: 96.0,
        dpiY: 96.0,
    };
    let bitmap = rt
        .CreateBitmap(size, Some(frozen.pixels.as_ptr() as *const c_void), frozen.stride() as u32, &bp)
        .context("CreateBitmap")?;
    let brush = rt.CreateSolidColorBrush(&style::WHITE, None)?;
    let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
    let ui = style::pick_family(&dwrite, &style::UI_FONTS);
    let mono = style::pick_family(&dwrite, &style::MONO_FONTS);
    let fonts = Fonts {
        ui: style::text_format(&dwrite, ui, 12.0, DWRITE_FONT_WEIGHT_NORMAL)?,
        mono_s: style::text_format(&dwrite, mono, 11.0, DWRITE_FONT_WEIGHT_NORMAL)?,
        mono_b: style::text_format(&dwrite, mono, 13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
    };
    let dash = factory.CreateStrokeStyle(
        &D2D1_STROKE_STYLE_PROPERTIES {
            startCap: D2D1_CAP_STYLE_FLAT,
            endCap: D2D1_CAP_STYLE_FLAT,
            dashCap: D2D1_CAP_STYLE_FLAT,
            lineJoin: D2D1_LINE_JOIN_MITER,
            miterLimit: 10.0,
            dashStyle: D2D1_DASH_STYLE_DASH,
            dashOffset: 0.0,
        },
        None,
    )?;
    Ok(Gfx { factory, rt, bitmap, brush, dwrite, fonts, dash })
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        // SAFETY: for WM_NCCREATE lParam points to a CREATESTRUCTW.
        let cs = &*(lp.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    // SAFETY: set in WM_NCCREATE from a Box that lives until the loop ends;
    // single-threaded, and handlers never re-enter before returning.
    let st = &mut *ptr;
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut ps);
            st.paint();
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            let p = st.desktop_point(lp);
            let o = st.ctl.mouse_move(p);
            st.apply(o);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            SetCapture(hwnd);
            let p = st.desktop_point(lp);
            let o = st.ctl.mouse_down(p);
            st.apply(o);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let _ = ReleaseCapture();
            let p = st.desktop_point(lp);
            let o = st.ctl.mouse_up(p);
            st.apply(o);
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            let p = st.desktop_point(lp);
            let o = st.ctl.double_click(p);
            st.apply(o);
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            let o = st.ctl.right_click();
            st.apply(o);
            LRESULT(0)
        }
        WM_KEYUP | WM_SYSKEYUP => {
            if let Some(k) = arrow_key(wp.0 as u16) {
                st.accel.key_up(k);
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let key = match wp.0 as u16 {
                x if x == VK_LEFT.0 => Some(Key::Left),
                x if x == VK_RIGHT.0 => Some(Key::Right),
                x if x == VK_UP.0 => Some(Key::Up),
                x if x == VK_DOWN.0 => Some(Key::Down),
                x if x == VK_RETURN.0 => Some(Key::Enter),
                x if x == VK_ESCAPE.0 => Some(Key::Escape),
                x if x == VK_SPACE.0 => Some(Key::Space),
                x if x == VK_TAB.0 => Some(Key::Tab),
                0x46 => Some(Key::Monitor),   // F
                0x41 => Some(Key::All),       // A
                0x4D => Some(Key::Magnifier), // M
                0x43 => Some(Key::CopyColor), // C
                0x48 => Some(Key::HintBar),   // H
                0x52 => Some(Key::Anchor),    // R
                _ => None,
            };
            match key {
                Some(k) => {
                    let mods = Mods {
                        shift: GetKeyState(VK_SHIFT.0 as i32) < 0,
                        ctrl: GetKeyState(VK_CONTROL.0 as i32) < 0,
                    };
                    // Letter shortcuts are plain keys; leave Ctrl+letter to the system.
                    let is_letter = matches!(k, Key::Monitor | Key::All | Key::Magnifier | Key::CopyColor | Key::HintBar | Key::Anchor);
                    if is_letter && mods.ctrl {
                        return DefWindowProcW(hwnd, msg, wp, lp);
                    }
                    let o = if arrow_key(wp.0 as u16).is_some() {
                        // lParam bit 30 = key was already down (auto-repeat);
                        // GetMessageTime = when Windows queued this message.
                        let repeat = (lp.0 >> 30) & 1 == 1;
                        // SAFETY: plain Win32 call without pointers.
                        let time = unsafe { GetMessageTime() } as u32;
                        let step = st.accel.step(k, repeat, time, mods);
                        if step == 0 {
                            return LRESULT(0);
                        }
                        st.ctl.key_step(k, mods, step)
                    } else {
                        st.ctl.key(k, mods)
                    };
                    st.apply(o);
                    LRESULT(0)
                }
                None => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
        WM_ACTIVATE => {
            if (wp.0 & 0xffff) as u32 == WA_INACTIVE {
                // Lost focus (Alt+Tab, another topmost window...): cancel, a
                // stuck overlay that cannot see Esc is worse than a cancel.
                if st.activated {
                    st.apply(Outcome::Done(None));
                }
            } else {
                st.activated = true;
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Arrow keys (the only ones that accelerate).
fn arrow_key(vk: u16) -> Option<Key> {
    match vk {
        x if x == VK_LEFT.0 => Some(Key::Left),
        x if x == VK_RIGHT.0 => Some(Key::Right),
        x if x == VK_UP.0 => Some(Key::Up),
        x if x == VK_DOWN.0 => Some(Key::Down),
        _ => None,
    }
}

impl State {
    /// Client-area lParam -> virtual-desktop point.
    fn desktop_point(&self, lp: LPARAM) -> Point {
        let x = (lp.0 & 0xffff) as u16 as i16 as i32;
        let y = ((lp.0 >> 16) & 0xffff) as u16 as i16 as i32;
        Point::new(self.origin.x + x, self.origin.y + y)
    }

    fn apply(&mut self, o: Outcome) {
        // SAFETY: hwnd is our live window.
        unsafe {
            match o {
                Outcome::Idle => {}
                Outcome::Redraw => {
                    let _ = InvalidateRect(self.hwnd, None, false);
                }
                Outcome::MoveCursor(p) => {
                    // SAFETY: plain Win32 call; coordinates are virtual-desktop px.
                    if let Err(e) = SetCursorPos(p.x, p.y) {
                        warn!("overlay: SetCursorPos({}, {}) failed: {e}", p.x, p.y);
                    }
                    let _ = InvalidateRect(self.hwnd, None, false);
                }
                Outcome::CopyColor(p) => {
                    // SAFETY: `frozen` is valid for the whole message loop.
                    let img = &*self.frozen;
                    let (x, y) = (p.x - self.origin.x, p.y - self.origin.y);
                    if x >= 0 && y >= 0 && (x as u32) < img.width && (y as u32) < img.height {
                        let hex = hex_color(img.pixel(x as u32, y as u32));
                        match set_clipboard_text(self.hwnd, &hex) {
                            Ok(()) => self.ctl.set_notice(format!("Warna {hex} disalin")),
                            Err(e) => {
                                warn!("overlay: copy colour failed: {e:#}");
                                self.ctl.set_notice("Gagal menyalin warna");
                            }
                        }
                    }
                    let _ = InvalidateRect(self.hwnd, None, false);
                }
                Outcome::Done(r) => {
                    if self.finished {
                        return;
                    }
                    self.finished = true;
                    self.result = r;
                    let _ = DestroyWindow(self.hwnd);
                }
            }
        }
    }

    unsafe fn paint(&mut self) {
        if self.gfx.is_none() {
            // SAFETY: `frozen` is valid for the whole message loop.
            match create_gfx(self.hwnd, &*self.frozen) {
                Ok(g) => self.gfx = Some(g),
                Err(e) => {
                    warn!("overlay: recreating Direct2D resources failed: {e:#}");
                    return;
                }
            }
        }
        let Some(g) = self.gfx.as_ref() else { return };
        let t = Instant::now();
        g.rt.BeginDraw();
        // SAFETY: `frozen` is valid for the whole message loop.
        draw_scene(g, &self.ctl, &self.opts, &self.monitors, self.origin, &*self.frozen);
        match g.rt.EndDraw(None, None) {
            Ok(()) => {}
            Err(e) if e.code() == D2DERR_RECREATE_TARGET => {
                self.gfx = None;
                let _ = InvalidateRect(self.hwnd, None, false);
            }
            Err(e) => warn!("overlay: EndDraw failed: {e}"),
        }
        debug!(paint_ms = t.elapsed().as_secs_f64() * 1000.0, "overlay paint");
    }
}

fn rect_f(r: Rect, origin: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: (r.x - origin.x) as f32,
        top: (r.y - origin.y) as f32,
        right: (r.right() - origin.x) as f32,
        bottom: (r.bottom() - origin.y) as f32,
    }
}

fn rr(r: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }
}

fn inset(r: D2D_RECT_F, d: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: r.left + d, top: r.top + d, right: r.right - d, bottom: r.bottom - d }
}

unsafe fn draw_scene(
    g: &Gfx,
    ctl: &Controller,
    opts: &OverlayOptions,
    monitors: &[MonitorInfo],
    origin: Rect,
    frozen: &Image,
) {
    let nn = D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR;
    let full = rect_f(origin, origin);
    g.rt.DrawBitmap(&g.bitmap, Some(&full), 1.0, nn, Some(&full));
    g.brush.SetColor(&style::DIM);
    g.rt.FillRectangle(&full, &g.brush);

    if let Some(r) = ctl.shown_rect() {
        let hover = ctl.showing_hover();
        let rf = rect_f(r, origin);
        // Re-draw the selected area undimmed.
        g.rt.DrawBitmap(&g.bitmap, Some(&rf), 1.0, nn, Some(&rf));
        let outline = inset(rf, 1.0);
        if hover {
            g.brush.SetColor(&style::SNAP);
            g.rt.DrawRectangle(&outline, &g.brush, 2.0, &g.dash);
        } else {
            g.brush.SetColor(&style::ACCENT);
            g.rt.DrawRectangle(&outline, &g.brush, 2.0, None);
            // Corner handles: 8 px white squares with an accent border.
            for (cx, cy) in [
                (rf.left + 1.0, rf.top + 1.0),
                (rf.right - 1.0, rf.top + 1.0),
                (rf.left + 1.0, rf.bottom - 1.0),
                (rf.right - 1.0, rf.bottom - 1.0),
            ] {
                let h = D2D_RECT_F { left: cx - 4.0, top: cy - 4.0, right: cx + 4.0, bottom: cy + 4.0 };
                g.brush.SetColor(&style::WHITE);
                g.rt.FillRoundedRectangle(&rr(h, 2.0), &g.brush);
                g.brush.SetColor(&style::ACCENT);
                g.rt.DrawRoundedRectangle(&rr(inset(h, 0.75), 2.0), &g.brush, 1.5, None);
            }
        }

        // Size label: accent pill, above-left of the selection (inside when
        // there is no room above).
        let text = format!("{} \u{d7} {}", r.width, r.height);
        let (tw, th) = g.measure(&g.fonts.mono_b, &text);
        let (pw, ph) = (tw + 20.0, th + 8.0);
        let x = (rf.left - 2.0).clamp(0.0, (full.right - pw).max(0.0));
        let y = if rf.top - ph - 8.0 >= 0.0 { rf.top - ph - 8.0 } else { rf.top + 8.0 };
        g.brush.SetColor(&style::ACCENT);
        g.rt.FillRoundedRectangle(&rr(D2D_RECT_F { left: x, top: y, right: x + pw, bottom: y + ph }, 6.0), &g.brush);
        g.text(&g.fonts.mono_b, &text, x + 10.0, y + 4.0, &style::WHITE);

        let kb_label = ctl.kb_mode().map(|m| match m {
            KbMode::Extend => "Mode: perluas \u{b7} Tab: geser",
            KbMode::Move => "Mode: geser \u{b7} Tab: perluas",
        });
        if hover || kb_label.is_some() {
            let label = kb_label.unwrap_or("Tab \u{b7} snap ke window ini");
            let (lw, lh) = g.measure(&g.fonts.ui, label);
            let (bw, bh) = (lw + 16.0, lh + 6.0);
            let bx = (x + pw + 8.0).min((full.right - bw).max(0.0));
            let by = y + (ph - bh) / 2.0;
            g.brush.SetColor(&style::SNAP_LABEL_BG);
            g.rt.FillRoundedRectangle(
                &rr(D2D_RECT_F { left: bx, top: by, right: bx + bw, bottom: by + bh }, 6.0),
                &g.brush,
            );
            g.text(&g.fonts.ui, label, bx + 8.0, by + 3.0, &style::TEXT);
        }
    }

    if let Some(a) = ctl.anchor() {
        // Anchor marker: small crosshair on the anchor pixel.
        let ar = rect_f(Rect::new(a.x, a.y, 1, 1), origin);
        let (ax, ay) = ((ar.left + ar.right) / 2.0, (ar.top + ar.bottom) / 2.0);
        for (c, w) in [(&style::WHITE, 4.0), (&style::ACCENT, 2.0)] {
            g.brush.SetColor(c);
            g.rt.DrawLine(D2D_POINT_2F { x: ax - 8.0, y: ay }, D2D_POINT_2F { x: ax + 8.0, y: ay }, &g.brush, w, None);
            g.rt.DrawLine(D2D_POINT_2F { x: ax, y: ay - 8.0 }, D2D_POINT_2F { x: ax, y: ay + 8.0 }, &g.brush, w, None);
        }
    }

    let mon = ctl
        .cursor()
        .and_then(|c| monitors.iter().find(|m| m.rect.contains(c)))
        .map(|m| m.rect)
        .unwrap_or(origin);
    let cx = (mon.x - origin.x) as f32 + mon.width as f32 / 2.0;
    let mut top = (mon.y - origin.y) as f32 + 16.0;
    for text in [opts.hint.as_deref(), ctl.notice()].into_iter().flatten() {
        let (lw, lh) = g.measure(&g.fonts.ui, text);
        let (bw, bh) = (lw + 24.0, lh + 12.0);
        let x = (cx - bw / 2.0).max(0.0);
        g.brush.SetColor(&style::HINT_BG);
        g.rt.FillRoundedRectangle(&rr(D2D_RECT_F { left: x, top, right: x + bw, bottom: top + bh }, 8.0), &g.brush);
        g.text(&g.fonts.ui, text, x + 12.0, top + 6.0, &style::TEXT);
        top += bh + 6.0;
    }
    if ctl.hint_bar_on() {
        draw_hint_bar(g, hint_rows(ctl.stage()), cx, (mon.y - origin.y) as f32 + mon.height as f32 - 28.0);
    }

    if ctl.magnifier_on() {
        if let Some(c) = ctl.cursor() {
            draw_loupe(g, c, origin, ctl.bounds(), frozen);
        }
    }
}

/// Two-row key hint bar, centred on `cx`, bottom edge at `bottom`.
/// (key, description, key width, key height, description width).
type HintItem<'a> = (&'a str, &'a str, f32, f32, f32);

unsafe fn draw_hint_bar(g: &Gfx, hint: &[&[(&str, &str)]; 2], cx: f32, bottom: f32) {
    const GAP_ITEM: f32 = 16.0;
    const GAP_KBD: f32 = 6.0;
    // Pre-measure: (kbd text w, kbd text h, desc w, desc h) per item.
    let rows: Vec<Vec<HintItem>> = hint
        .iter()
        .map(|row| {
            row.iter()
                .map(|&(k, t)| {
                    let (kw, kh) = g.measure(&g.fonts.mono_s, k);
                    let (tw, _) = g.measure(&g.fonts.ui, t);
                    (k, t, kw, kh, tw)
                })
                .collect()
        })
        .collect();
    // kbd chip: padding 1px 6px, border 1px (2px at the bottom).
    let chip_h = rows.iter().flatten().map(|i| i.3).fold(0.0, f32::max) + 2.0 + 3.0;
    let row_w = |row: &Vec<HintItem>| {
        row.iter().map(|i| (i.2 + 14.0) + GAP_KBD + i.4).sum::<f32>() + GAP_ITEM * row.len().saturating_sub(1) as f32
    };
    let inner_w = rows.iter().map(row_w).fold(0.0, f32::max);
    let (box_w, box_h) = (inner_w + 32.0, chip_h * 2.0 + 6.0 + 20.0);
    let left = (cx - box_w / 2.0).max(0.0);
    let top = bottom - box_h;
    let panel = D2D_RECT_F { left, top, right: left + box_w, bottom };
    g.brush.SetColor(&style::HINT_BG);
    g.rt.FillRoundedRectangle(&rr(panel, 12.0), &g.brush);
    g.brush.SetColor(&style::PANEL_BORDER);
    g.rt.DrawRoundedRectangle(&rr(inset(panel, 0.5), 12.0), &g.brush, 1.0, None);

    let mut y = top + 10.0;
    for row in &rows {
        let mut x = left + 16.0;
        for &(k, t, kw, kh, tw) in row {
            let chip = D2D_RECT_F { left: x, top: y, right: x + kw + 14.0, bottom: y + chip_h };
            g.brush.SetColor(&style::KBD_BG);
            g.rt.FillRoundedRectangle(&rr(chip, 4.0), &g.brush);
            g.brush.SetColor(&style::KBD_BORDER);
            g.rt.DrawRoundedRectangle(&rr(inset(chip, 0.5), 4.0), &g.brush, 1.0, None);
            // Thicker bottom edge.
            g.rt.DrawLine(
                D2D_POINT_2F { x: chip.left + 3.0, y: chip.bottom - 1.5 },
                D2D_POINT_2F { x: chip.right - 3.0, y: chip.bottom - 1.5 },
                &g.brush,
                1.0,
                None,
            );
            g.text(&g.fonts.mono_s, k, x + 7.0, y + 1.0 + (chip_h - 3.0 - 2.0 - kh) / 2.0 + 1.0, &style::TEXT);
            x += kw + 14.0 + GAP_KBD;
            let (_, th) = g.measure(&g.fonts.ui, t);
            g.text(&g.fonts.ui, t, x, y + (chip_h - th) / 2.0, &style::TEXT_HINT);
            x += tw + GAP_ITEM;
        }
        y += chip_h + 6.0;
    }
}

unsafe fn draw_loupe(g: &Gfx, cursor: Point, origin: Rect, bounds: Rect, frozen: &Image) {
    let l = magnifier::layout(cursor, bounds);
    let nn = D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR;
    let dest = rect_f(l.dest, origin);
    let src = rect_f(l.src, origin);
    let strip = rect_f(l.label, origin);
    let panel = D2D_RECT_F { left: dest.left, top: dest.top, right: dest.right, bottom: strip.bottom };

    g.brush.SetColor(&style::PANEL_BG);
    g.rt.FillRoundedRectangle(&rr(panel, 10.0), &g.brush);

    // Pixel grid, clipped to the panel's rounded top corners.
    let mask_geo = g.factory.CreateRoundedRectangleGeometry(&rr(panel, 10.0)).ok();
    let layer = g.rt.CreateLayer(None).ok();
    let mut pushed = false;
    if let (Some(geo), Some(layer)) = (mask_geo.as_ref(), layer.as_ref()) {
        let params = D2D1_LAYER_PARAMETERS {
            contentBounds: panel,
            geometricMask: std::mem::ManuallyDrop::new(Some(geo.clone().into())),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: 1.0,
            opacityBrush: std::mem::ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS_NONE,
        };
        g.rt.PushLayer(&params, layer);
        // Drop the extra reference we put into the ManuallyDrop mask.
        std::mem::ManuallyDrop::into_inner(params.geometricMask);
        pushed = true;
    }
    g.rt.DrawBitmap(&g.bitmap, Some(&dest), 1.0, nn, Some(&src));
    // Faint cell borders.
    g.brush.SetColor(&style::rgba(0x000000, 0.12));
    let z = magnifier::ZOOM as f32;
    for i in 1..magnifier::SRC_SIDE {
        let o = i as f32 * z;
        g.rt.DrawLine(
            D2D_POINT_2F { x: dest.left + o - 0.5, y: dest.top },
            D2D_POINT_2F { x: dest.left + o - 0.5, y: dest.bottom },
            &g.brush,
            1.0,
            None,
        );
        g.rt.DrawLine(
            D2D_POINT_2F { x: dest.left, y: dest.top + o - 0.5 },
            D2D_POINT_2F { x: dest.right, y: dest.top + o - 0.5 },
            &g.brush,
            1.0,
            None,
        );
    }
    // Crosshair: the cursor cell's row and column, accent at 70 %.
    let cell = rect_f(l.cursor_cell, origin);
    g.brush.SetColor(&style::rgba(0x4FC3CF, 0.7));
    for x in [cell.left + 0.5, cell.right - 0.5] {
        g.rt.DrawLine(
            D2D_POINT_2F { x, y: dest.top },
            D2D_POINT_2F { x, y: dest.bottom },
            &g.brush,
            1.0,
            None,
        );
    }
    for y in [cell.top + 0.5, cell.bottom - 0.5] {
        g.rt.DrawLine(
            D2D_POINT_2F { x: dest.left, y },
            D2D_POINT_2F { x: dest.right, y },
            &g.brush,
            1.0,
            None,
        );
    }
    if pushed {
        g.rt.PopLayer();
    }

    g.brush.SetColor(&style::PANEL_BORDER);
    g.rt.DrawRoundedRectangle(&rr(inset(panel, 0.5), 10.0), &g.brush, 1.0, None);

    // Coordinates, then colour swatch + hex + "C".
    g.text(&g.fonts.mono_s, &format!("X {} \u{b7} Y {}", cursor.x, cursor.y), strip.left + 8.0, strip.top + 6.0, &style::TEXT);
    let (_, lh) = g.measure(&g.fonts.mono_s, "0");
    let y2 = strip.top + 6.0 + lh + 2.0;
    let (px, py) = (cursor.x - origin.x, cursor.y - origin.y);
    if px >= 0 && py >= 0 && (px as u32) < frozen.width && (py as u32) < frozen.height {
        let p = frozen.pixel(px as u32, py as u32);
        let sw = D2D_RECT_F { left: strip.left + 8.0, top: y2 + (lh - 10.0) / 2.0, right: strip.left + 18.0, bottom: y2 + (lh - 10.0) / 2.0 + 10.0 };
        g.brush.SetColor(&D2D1_COLOR_F { r: p[0] as f32 / 255.0, g: p[1] as f32 / 255.0, b: p[2] as f32 / 255.0, a: 1.0 });
        g.rt.FillRoundedRectangle(&rr(sw, 2.0), &g.brush);
        let hex = hex_color(p);
        let (hw, _) = g.measure(&g.fonts.mono_s, &hex);
        g.text(&g.fonts.mono_s, &hex, strip.left + 24.0, y2, &style::TEXT);
        g.text(&g.fonts.mono_s, "C", strip.left + 24.0 + hw + 6.0, y2, &style::MUTED);
    }
}

impl Gfx {
    /// Text metrics (width, height) for `text` in `format`.
    unsafe fn measure(&self, format: &IDWriteTextFormat, text: &str) -> (f32, f32) {
        self.layout(format, text).map(|(_, w, h)| (w, h)).unwrap_or((0.0, 0.0))
    }

    unsafe fn layout(&self, format: &IDWriteTextFormat, text: &str) -> Option<(IDWriteTextLayout, f32, f32)> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let layout = self.dwrite.CreateTextLayout(&wide, format, 2000.0, 200.0).ok()?;
        let mut m = DWRITE_TEXT_METRICS::default();
        layout.GetMetrics(&mut m).ok()?;
        Some((layout, m.width, m.height))
    }

    /// Text with its top-left at (x, y).
    unsafe fn text(&self, format: &IDWriteTextFormat, text: &str, x: f32, y: f32, color: &D2D1_COLOR_F) {
        if let Some((layout, _, _)) = self.layout(format, text) {
            self.brush.SetColor(color);
            self.rt.DrawTextLayout(D2D_POINT_2F { x, y }, &layout, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
    }
}

/// Put `text` on the clipboard as CF_UNICODETEXT.
fn set_clipboard_text(hwnd: HWND, text: &str) -> Result<()> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * 2;
    // SAFETY: standard clipboard protocol; the allocation is handed to the
    // system on success and freed by us only on failure paths before that.
    unsafe {
        OpenClipboard(hwnd).context("OpenClipboard")?;
        let res = (|| -> Result<()> {
            EmptyClipboard()?;
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes)?;
            let dst = GlobalLock(h) as *mut u16;
            if dst.is_null() {
                let _ = GlobalFree(h);
                bail!("GlobalLock failed");
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
            let _ = GlobalUnlock(h);
            if let Err(e) = SetClipboardData(CF_UNICODETEXT.0 as u32, HANDLE(h.0)) {
                let _ = GlobalFree(h);
                return Err(e.into());
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        res
    }
}
