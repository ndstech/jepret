//! Frame stitching. Owner: **T07**. Pure logic, no Win32 — fully unit-testable.
//!
//! Frames are successive captures of the same region after scrolling down.
//! For each new frame find vertical offset `dy` such that the top of the new
//! frame matches the bottom of the previous one (row hashes + verification),
//! ignoring `ignore_top`/`ignore_bottom` px (sticky header/footer), then
//! append the new rows.

use jepret_core::Image;

#[derive(Debug, Clone)]
pub struct StitchOptions {
    /// Rows at top of each frame excluded from matching (sticky header).
    pub ignore_top: u32,
    /// Rows at bottom excluded from matching (sticky footer).
    pub ignore_bottom: u32,
    /// Minimum overlap rows required to accept a match.
    pub min_overlap: u32,
    /// Auto-detect sticky header/footer from the first two frames.
    pub auto_detect_sticky: bool,
}

impl Default for StitchOptions {
    fn default() -> Self {
        Self { ignore_top: 0, ignore_bottom: 0, min_overlap: 32, auto_detect_sticky: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushResult {
    /// First frame or new rows appended.
    Appended { rows: u32 },
    /// Frame identical to previous (end of page reached?).
    NoChange,
    /// No reliable overlap found; frame skipped.
    NoMatch,
}

pub struct Stitcher {
    _private: (),
}

impl Stitcher {
    pub fn new(_opts: StitchOptions) -> Self {
        todo!("T07: Stitcher::new")
    }

    /// All frames must have the same width/height as the first.
    pub fn push(&mut self, _frame: &Image) -> PushResult {
        todo!("T07: Stitcher::push")
    }

    pub fn height(&self) -> u32 {
        todo!("T07: Stitcher::height")
    }

    pub fn finish(self) -> Image {
        todo!("T07: Stitcher::finish")
    }
}
