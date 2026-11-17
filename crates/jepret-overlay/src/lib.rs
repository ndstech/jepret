//! Region selection overlay. Owner: **T03**.
//!
//! One borderless topmost layered window covering the virtual desktop,
//! drawing the frozen screenshot with Direct2D, darkened outside the
//! selection. See PRD F3 for interactions.
//!
//! Does **not** depend on `jepret-capture`: the caller passes everything in,
//! so this crate can be developed and demoed with a synthetic image.

use jepret_core::{Image, MonitorInfo, Rect, Result};

#[derive(Debug, Clone, Default)]
pub struct OverlayOptions {
    /// Preselect this rect (e.g. last region) — user can adjust or press Enter.
    pub initial: Option<Rect>,
    pub magnifier: bool,
    /// Window rects (virtual-desktop px, topmost first) for hover-snap.
    /// Empty = snapping disabled.
    pub window_rects: Vec<Rect>,
    /// Hint text shown at the top (e.g. "Scroll capture: pilih area").
    pub hint: Option<String>,
}

/// Show the overlay and block until the user confirms or cancels.
///
/// * `frozen` — screenshot of the virtual desktop; pixel (0,0) is `origin.origin()`.
/// * `origin` — virtual-desktop rect covered by `frozen`.
///
/// Returns the selected rect in virtual-desktop px, or `None` if cancelled.
/// Must be called on a thread that may pump Win32 messages (it runs its own
/// modal message loop). Target: window visible ≤ 50 ms after call.
pub fn select_region(
    _frozen: &Image,
    _origin: Rect,
    _monitors: &[MonitorInfo],
    _opts: &OverlayOptions,
) -> Result<Option<Rect>> {
    anyhow::bail!("TODO(T03): select_region")
}

/// Non-interactive click-through border drawn around `rect` while
/// recording / scroll-capturing. Dropping the handle removes it.
pub struct RegionBorder {
    _private: (),
}

impl RegionBorder {
    pub fn show(_rect: Rect) -> Result<Self> {
        anyhow::bail!("TODO(T03): RegionBorder::show")
    }
}
