//! Scroll driver. Owner: **T08**.
//!
//! Loop: capture region (`jepret_capture::capture_rect`) → push to
//! [`Stitcher`](crate::Stitcher) → send mouse wheel to the window under the
//! region center (SendInput with cursor moved there, restore cursor after)
//! → wait `scroll_delay_ms` → repeat. Stop on 2× `NoChange`, `max_frames`,
//! or Esc pressed (GetAsyncKeyState polling between frames is fine here).

use jepret_core::{Image, Rect, Result};

#[derive(Debug, Clone)]
pub struct ScrollOptions {
    pub max_frames: u32,
    pub scroll_delay_ms: u32,
    pub wheel_notches: u32,
    pub capture: jepret_capture::CaptureOptions,
    pub stitch: crate::StitchOptions,
}

/// Blocks until done. Returns the stitched image.
pub fn capture_scrolling(_region: Rect, _opts: &ScrollOptions) -> Result<Image> {
    anyhow::bail!("TODO(T08): capture_scrolling")
}
