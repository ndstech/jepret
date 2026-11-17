//! Screen capture engine. Owner: **T01** (capture), **T02** (`hdr.rs`).
//!
//! All rects are virtual-desktop physical px. All returned images are
//! RGBA8 sRGB (HDR monitors are tone-mapped via [`hdr`] when
//! `CaptureOptions::hdr_tonemap` is true).

pub mod hdr;

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

/// Enumerate monitors (incl. HDR state and SDR white level).
pub fn monitors() -> Result<Vec<MonitorInfo>> {
    anyhow::bail!("TODO(T01): monitors")
}

/// Bounding rect of all monitors.
pub fn virtual_screen_rect() -> Result<Rect> {
    anyhow::bail!("TODO(T01): virtual_screen_rect")
}

/// Current cursor position (physical px).
pub fn cursor_pos() -> Result<Point> {
    anyhow::bail!("TODO(T01): cursor_pos")
}

/// Monitor containing `p` (nearest if none).
pub fn monitor_at(_p: Point) -> Result<MonitorInfo> {
    anyhow::bail!("TODO(T01): monitor_at")
}

/// Capture the whole virtual desktop into one image. Returns the image and
/// the virtual-desktop rect it covers (pixel (0,0) == rect.origin()).
/// Areas not covered by any monitor are transparent black.
pub fn capture_virtual_screen(_opts: &CaptureOptions) -> Result<(Image, Rect)> {
    anyhow::bail!("TODO(T01): capture_virtual_screen")
}

/// Capture a single monitor.
pub fn capture_monitor(_id: &MonitorId, _opts: &CaptureOptions) -> Result<(Image, Rect)> {
    anyhow::bail!("TODO(T01): capture_monitor")
}

/// Capture an arbitrary rect (may span monitors).
pub fn capture_rect(_rect: Rect, _opts: &CaptureOptions) -> Result<Image> {
    anyhow::bail!("TODO(T01): capture_rect")
}

/// Capture the foreground window without DWM shadow.
pub fn capture_foreground_window(_opts: &CaptureOptions) -> Result<(Image, Rect)> {
    anyhow::bail!("TODO(T01): capture_foreground_window")
}

/// Visible top-level window rects in Z-order (topmost first), used by the
/// overlay for snap-to-window. Excludes cloaked/invisible/tool windows.
pub fn visible_window_rects() -> Result<Vec<Rect>> {
    anyhow::bail!("TODO(T01): visible_window_rects")
}
