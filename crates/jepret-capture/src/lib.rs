//! Screen capture engine. Owner: **T01** (capture), **T02** (`hdr.rs`).
//!
//! All rects are virtual-desktop physical px. All returned images are
//! RGBA8 sRGB (HDR monitors are tone-mapped via [`hdr`] when
//! `CaptureOptions::hdr_tonemap` is true).
//!
//! Implementation: DXGI Desktop Duplication per output (FP16 on HDR
//! monitors), with a GDI `BitBlt` fallback for SDR when duplication is
//! unavailable (rotated outputs, RDP, duplication limit, ...). Monitors are
//! grabbed in parallel and composed into virtual-desktop space. D3D devices
//! and duplication sessions are cached between calls for speed; the daemon
//! frees them after idle via [`release_cache`].
//! WGC fallback is not implemented (see T01 report).

pub mod hdr;
mod pixels;

#[cfg(windows)]
mod dxgi;
#[cfg(windows)]
mod gdi;
#[cfg(windows)]
mod monitors;
#[cfg(windows)]
mod winrects;

pub use pixels::{bgra_to_rgba, blit, compose};

use jepret_core::{Image, MonitorId, MonitorInfo, Point, Rect, Result};

#[derive(Debug, Clone)]
pub struct CaptureOptions {
    pub show_cursor: bool,
    pub hdr_tonemap: bool,
    /// Override SDR white level; `None` = per-monitor Windows setting.
    pub sdr_white_nits: Option<f32>,
}

impl From<&jepret_core::config::CaptureConfig> for CaptureOptions {
    fn from(c: &jepret_core::config::CaptureConfig) -> Self {
        Self {
            show_cursor: c.show_cursor,
            hdr_tonemap: c.hdr_tonemap,
            sdr_white_nits: c.sdr_white_nits,
        }
    }
}

/// Make the process per-monitor-v2 DPI aware so all coordinates are physical
/// px. Idempotent; ignored if a manifest/earlier call already set it. Called
/// automatically by [`monitors`].
pub fn ensure_dpi_aware() {
    #[cfg(windows)]
    {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            use windows::Win32::UI::HiDpi::{
                SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            };
            // SAFETY: plain process-wide call; failure (already set) is fine.
            let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        });
    }
}

/// Free cached GPU resources (D3D devices / duplication sessions kept between
/// captures to make repeat captures fast). Call when the app has been idle;
/// the next capture transparently re-creates them.
pub fn release_cache() {
    #[cfg(windows)]
    dxgi::release_cache();
}

/// Enumerate monitors (incl. HDR state and SDR white level).
pub fn monitors() -> Result<Vec<MonitorInfo>> {
    #[cfg(windows)]
    {
        monitors::enumerate()
    }
    #[cfg(not(windows))]
    anyhow::bail!("jepret-capture supports Windows only")
}

/// Bounding rect of all monitors.
pub fn virtual_screen_rect() -> Result<Rect> {
    let mons = monitors()?;
    Ok(mons.iter().fold(Rect::default(), |acc, m| acc.union(&m.rect)))
}

/// Current cursor position (physical px).
pub fn cursor_pos() -> Result<Point> {
    #[cfg(windows)]
    {
        ensure_dpi_aware();
        let mut p = windows::Win32::Foundation::POINT::default();
        // SAFETY: valid out pointer.
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p) }?;
        Ok(Point::new(p.x, p.y))
    }
    #[cfg(not(windows))]
    anyhow::bail!("jepret-capture supports Windows only")
}

/// Pure: monitor containing `p`, else the one whose rect is nearest.
fn nearest_monitor(mons: &[MonitorInfo], p: Point) -> Option<&MonitorInfo> {
    if let Some(m) = mons.iter().find(|m| m.rect.contains(p)) {
        return Some(m);
    }
    mons.iter().min_by_key(|m| {
        let dx = (m.rect.x - p.x).max(p.x - (m.rect.right() - 1)).max(0) as i64;
        let dy = (m.rect.y - p.y).max(p.y - (m.rect.bottom() - 1)).max(0) as i64;
        dx * dx + dy * dy
    })
}

/// Monitor containing `p` (nearest if none).
pub fn monitor_at(p: Point) -> Result<MonitorInfo> {
    let mons = monitors()?;
    nearest_monitor(&mons, p)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no monitors"))
}

#[cfg(windows)]
fn grab_monitor(mon: &MonitorInfo, opts: &CaptureOptions) -> Result<Image> {
    use std::time::Instant;
    let t0 = Instant::now();
    let img = grab_monitor_inner(mon, opts)?;
    tracing::debug!(monitor = %mon.id.0, ms = t0.elapsed().as_secs_f32() * 1000.0, "grab_monitor");
    Ok(img)
}

#[cfg(windows)]
fn grab_monitor_inner(mon: &MonitorInfo, opts: &CaptureOptions) -> Result<Image> {
    if mon.hdr && opts.hdr_tonemap {
        let nits = opts.sdr_white_nits.unwrap_or(mon.sdr_white_nits);
        // Tone-map failure (incl. T02's stub still being `todo!()`) falls
        // through to the SDR path.
        match dxgi::grab(mon, dxgi::Mode::Hdr { sdr_white_nits: nits }) {
            Ok(img) => return Ok(img),
            Err(e) => tracing::warn!("HDR capture failed on {}: {e:#}", mon.id.0),
        }
    }
    match dxgi::grab(mon, dxgi::Mode::Sdr) {
        Ok(img) => Ok(img),
        Err(e) => {
            tracing::warn!("desktop duplication failed on {} ({e:#}), falling back to GDI", mon.id.0);
            gdi::grab_rect(mon.rect)
        }
    }
}

/// Grab several monitors in parallel (each on its own device).
#[cfg(windows)]
fn grab_many(mons: &[MonitorInfo], opts: &CaptureOptions) -> Result<Vec<(Image, Rect)>> {
    if mons.len() == 1 {
        return Ok(vec![(grab_monitor(&mons[0], opts)?, mons[0].rect)]);
    }
    let results: Vec<Result<Image>> = std::thread::scope(|s| {
        let handles: Vec<_> = mons.iter().map(|m| s.spawn(move || grab_monitor(m, opts))).collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("capture thread panicked"))))
            .collect()
    });
    let mut out = Vec::with_capacity(mons.len());
    for (m, r) in mons.iter().zip(results) {
        out.push((r?, m.rect));
    }
    Ok(out)
}

#[cfg(windows)]
fn with_cursor(mut img: Image, area: Rect, opts: &CaptureOptions) -> Image {
    if opts.show_cursor {
        gdi::draw_cursor(&mut img, area.origin());
    }
    img
}

/// Capture the whole virtual desktop into one image. Returns the image and
/// the virtual-desktop rect it covers (pixel (0,0) == rect.origin()).
/// Areas not covered by any monitor are transparent black.
pub fn capture_virtual_screen(opts: &CaptureOptions) -> Result<(Image, Rect)> {
    #[cfg(windows)]
    {
        let mons = monitors()?;
        let area = mons.iter().fold(Rect::default(), |acc, m| acc.union(&m.rect));
        let parts = grab_many(&mons, opts)?;
        let img = if parts.len() == 1 && parts[0].1 == area {
            parts.into_iter().next().map(|p| p.0).unwrap_or_else(|| Image::new(0, 0))
        } else {
            compose(area, &parts)
        };
        Ok((with_cursor(img, area, opts), area))
    }
    #[cfg(not(windows))]
    {
        let _ = opts;
        anyhow::bail!("jepret-capture supports Windows only")
    }
}

/// Capture a single monitor.
pub fn capture_monitor(id: &MonitorId, opts: &CaptureOptions) -> Result<(Image, Rect)> {
    #[cfg(windows)]
    {
        let mons = monitors()?;
        let mon = mons
            .iter()
            .find(|m| &m.id == id)
            .ok_or_else(|| anyhow::anyhow!("monitor {} not found", id.0))?;
        let img = grab_monitor(mon, opts)?;
        Ok((with_cursor(img, mon.rect, opts), mon.rect))
    }
    #[cfg(not(windows))]
    {
        let _ = (id, opts);
        anyhow::bail!("jepret-capture supports Windows only")
    }
}

/// Capture an arbitrary rect (may span monitors).
pub fn capture_rect(rect: Rect, opts: &CaptureOptions) -> Result<Image> {
    #[cfg(windows)]
    {
        anyhow::ensure!(!rect.is_empty(), "empty capture rect");
        let mons = monitors()?;
        let hit: Vec<MonitorInfo> = mons.into_iter().filter(|m| m.rect.intersect(&rect).is_some()).collect();
        anyhow::ensure!(!hit.is_empty(), "rect {rect:?} does not intersect any monitor");
        let parts = grab_many(&hit, opts)?;
        Ok(with_cursor(compose(rect, &parts), rect, opts))
    }
    #[cfg(not(windows))]
    {
        let _ = (rect, opts);
        anyhow::bail!("jepret-capture supports Windows only")
    }
}

/// Capture the foreground window without DWM shadow.
pub fn capture_foreground_window(opts: &CaptureOptions) -> Result<(Image, Rect)> {
    #[cfg(windows)]
    {
        ensure_dpi_aware();
        let wr = winrects::foreground_rect()?;
        let vs = virtual_screen_rect()?;
        let rect = wr
            .intersect(&vs)
            .ok_or_else(|| anyhow::anyhow!("foreground window is off-screen"))?;
        Ok((capture_rect(rect, opts)?, rect))
    }
    #[cfg(not(windows))]
    {
        let _ = opts;
        anyhow::bail!("jepret-capture supports Windows only")
    }
}

/// Visible top-level window rects in Z-order (topmost first), used by the
/// overlay for snap-to-window. Excludes cloaked/invisible/tool windows.
pub fn visible_window_rects() -> Result<Vec<Rect>> {
    #[cfg(windows)]
    {
        ensure_dpi_aware();
        winrects::visible_rects()
    }
    #[cfg(not(windows))]
    anyhow::bail!("jepret-capture supports Windows only")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(x: i32, y: i32, w: u32, h: u32) -> MonitorInfo {
        MonitorInfo {
            id: MonitorId(format!("m{x}")),
            name: String::new(),
            rect: Rect::new(x, y, w, h),
            work_area: Rect::new(x, y, w, h),
            scale: 1.0,
            primary: x == 0,
            hdr: false,
            sdr_white_nits: 80.0,
        }
    }

    #[test]
    fn nearest_monitor_picks_containing_then_closest() {
        let ms = [mon(-1920, 0, 1920, 1080), mon(0, 0, 2560, 1440)];
        assert_eq!(nearest_monitor(&ms, Point::new(-1, 5)).unwrap().id.0, "m-1920");
        assert_eq!(nearest_monitor(&ms, Point::new(0, 5)).unwrap().id.0, "m0");
        // below left monitor, still closer to it than the right one
        assert_eq!(nearest_monitor(&ms, Point::new(-500, 1200)).unwrap().id.0, "m-1920");
        assert_eq!(nearest_monitor(&ms, Point::new(9000, 0)).unwrap().id.0, "m0");
        assert!(nearest_monitor(&[], Point::new(0, 0)).is_none());
    }
}
