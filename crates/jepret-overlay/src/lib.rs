//! Region selection overlay. Owner: **T03**.
//!
//! One borderless topmost window covering the virtual desktop, drawing the
//! frozen screenshot with Direct2D, darkened outside the selection. See PRD F3
//! for interactions.
//!
//! Does **not** depend on `jepret-capture`: the caller passes everything in,
//! so this crate can be developed and demoed with a synthetic image.
//!
//! Layout: pure logic ([`selection`], [`snap`], [`magnifier`], [`controller`])
//! has no Win32 and is unit tested; `overlay` / `border` hold the Win32 and
//! Direct2D glue.
//!
//! Interaction summary: drag selects (pixel-inclusive, release confirms);
//! click without drag picks the hovered window (needs `window_rects`);
//! arrows nudge 1 px, Shift+arrows resize the bottom-right edge; Enter
//! confirms; Esc / right click cancel; losing focus cancels.

pub mod controller;
pub mod magnifier;
pub mod selection;
pub mod snap;

mod border;
mod overlay;
mod style;

pub use border::RegionBorder;

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
///
/// The calling thread is switched to Per-Monitor-DPI-V2 awareness for the
/// duration of the call so window geometry is in physical pixels regardless
/// of the process manifest.
pub fn select_region(
    frozen: &Image,
    origin: Rect,
    monitors: &[MonitorInfo],
    opts: &OverlayOptions,
) -> Result<Option<Rect>> {
    overlay::run(frozen, origin, monitors, opts)
}
