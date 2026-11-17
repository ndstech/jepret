//! Frame stitching. Owner: **T07**. Pure logic, no Win32 — fully unit-testable.
//!
//! Frames are successive captures of the same region after scrolling down.
//! For each new frame find vertical offset `dy` such that the top of the new
//! frame matches the bottom of the previous one (row hashes + verification),
//! ignoring `ignore_top`/`ignore_bottom` px (sticky header/footer), then
//! append the new rows.

use std::collections::HashMap;

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

/// Max occurrences of a row hash in the previous frame for it to count as an
/// anchor (rows repeated more often carry no positional information).
const MAX_ANCHOR_OCCURRENCES: usize = 4;
/// Max candidate offsets that get verified pixel by pixel.
const MAX_CANDIDATES: usize = 24;
/// Per-channel difference tolerated when comparing rows whose hashes differ.
const CHANNEL_TOLERANCE: i32 = 8;

/// Incremental vertical stitcher.
///
/// Algorithm per [`push`](Stitcher::push): hash every row (64-bit Fx-style) ->
/// index the previous frame's non-uniform body rows by hash -> every row of the
/// new frame that hashes the same votes for an offset `dy` -> the best-voted
/// offsets are verified over the whole overlap (exact hash, or a small
/// per-pixel tolerance for anti-aliasing) -> the last `dy` body rows of the
/// new frame are appended. Sticky header/footer rows are detected from the
/// first two differing frames, excluded from matching, and re-attached once:
/// header from the first frame, footer from the latest frame.
///
/// Only downward scrolling is handled; a frame that does not overlap the
/// previous accepted frame by at least `min_overlap` body rows is `NoMatch`.
pub struct Stitcher {
    opts: StitchOptions,
    started: bool,
    width: usize,
    height: usize,
    /// Stitched rows (header + body) as chunks, so growing never reallocates and
    /// copies the whole image. Footer is kept apart.
    result: Vec<Vec<u8>>,
    result_len: usize,
    footer: Vec<u8>,
    /// Until the first append the whole first frame lives in `prev` only.
    first_pending: bool,
    sticky_done: bool,
    top: usize,
    bottom: usize,
    prev: Vec<u8>,
    prev_hashes: Vec<u64>,
}

fn hash_row(row: &[u8]) -> u64 {
    const K: u64 = 0x517c_c1b7_2722_0a95;
    let mut h: u64 = 0x9e37_79b9_7f4a_7c15;
    let (words, rest) = row.as_chunks::<8>();
    for c in words {
        h = (h.rotate_left(5) ^ u64::from_le_bytes(*c)).wrapping_mul(K);
    }
    for &b in rest {
        h = (h.rotate_left(5) ^ u64::from(b)).wrapping_mul(K);
    }
    h ^ (h >> 29)
}

fn row_hashes(pixels: &[u8], stride: usize) -> Vec<u64> {
    pixels.chunks_exact(stride).map(hash_row).collect()
}

fn is_uniform(row: &[u8]) -> bool {
    match row.get(..4) {
        Some(first) => row.as_chunks::<4>().0.iter().all(|p| p == first),
        None => true,
    }
}

/// Rows equal within tolerance: at most 1% of pixels differ by more than
/// `CHANNEL_TOLERANCE` in any RGB channel.
fn rows_close(a: &[u8], b: &[u8]) -> bool {
    let bad = a
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .filter(|(p, q)| (0..3).any(|i| (i32::from(p[i]) - i32::from(q[i])).abs() > CHANNEL_TOLERANCE))
        .count();
    bad * 100 <= a.len() / 4
}

impl Stitcher {
    pub fn new(opts: StitchOptions) -> Self {
        Self {
            opts,
            started: false,
            width: 0,
            height: 0,
            result: Vec::new(),
            result_len: 0,
            footer: Vec::new(),
            first_pending: false,
            sticky_done: false,
            top: 0,
            bottom: 0,
            prev: Vec::new(),
            prev_hashes: Vec::new(),
        }
    }

    /// All frames must have the same width/height as the first. Mismatching or
    /// malformed frames are rejected with `NoMatch` (never panics).
    pub fn push(&mut self, frame: &Image) -> PushResult {
        let (w, h) = (frame.width as usize, frame.height as usize);
        if w == 0 || h == 0 || frame.pixels.len() != w * h * 4 {
            return PushResult::NoMatch;
        }
        let stride = w * 4;
        if !self.started {
            self.started = true;
            self.width = w;
            self.height = h;
            self.top = (self.opts.ignore_top as usize).min(h);
            self.bottom = (self.opts.ignore_bottom as usize).min(h - self.top);
            self.first_pending = true;
            self.prev = frame.pixels.clone();
            self.prev_hashes = row_hashes(&frame.pixels, stride);
            return PushResult::Appended { rows: frame.height };
        }
        if w != self.width || h != self.height {
            return PushResult::NoMatch;
        }

        let hashes = row_hashes(&frame.pixels, stride);
        if hashes == self.prev_hashes {
            return PushResult::NoChange;
        }
        if !self.sticky_done {
            self.sticky_done = true;
            self.detect_sticky(&hashes);
        }
        let min_overlap = (self.opts.min_overlap as usize).max(1);
        let (top, bottom) = (self.top, self.bottom);
        if top + bottom + min_overlap >= h {
            return PushResult::NoMatch;
        }
        let body = top..h - bottom;
        if hashes[body.clone()] == self.prev_hashes[body] {
            return PushResult::NoChange;
        }
        let Some(dy) = self.find_dy(&hashes, &frame.pixels, min_overlap) else {
            return PushResult::NoMatch;
        };
        let from = (h - bottom - dy) * stride;
        let to = (h - bottom) * stride;
        if self.first_pending {
            // The first frame (minus footer) becomes the head of the result
            // without an extra copy.
            self.first_pending = false;
            let mut first = std::mem::replace(&mut self.prev, frame.pixels.clone());
            first.truncate((h - bottom) * stride);
            self.result_len = first.len();
            self.result.push(first);
        } else {
            self.prev.clear();
            self.prev.extend_from_slice(&frame.pixels);
        }
        self.result.push(frame.pixels[from..to].to_vec());
        self.result_len += to - from;
        if bottom > 0 {
            self.footer.clear();
            self.footer.extend_from_slice(&frame.pixels[to..]);
        }
        self.prev_hashes = hashes;
        PushResult::Appended { rows: dy as u32 }
    }

    /// Sticky bands = rows identical at the same position in both frames
    /// (trimmed so they do not end in uniform padding, which is ambiguous with
    /// blank page background).
    fn detect_sticky(&mut self, hashes: &[u64]) {
        if !self.opts.auto_detect_sticky {
            return;
        }
        let h = self.height;
        let stride = self.width * 4;
        let row = |y: usize| &self.prev[y * stride..(y + 1) * stride];
        let mut t = hashes.iter().zip(&self.prev_hashes).take_while(|(a, b)| a == b).count();
        while t > 0 && is_uniform(row(t - 1)) {
            t -= 1;
        }
        let mut b =
            hashes.iter().rev().zip(self.prev_hashes.iter().rev()).take_while(|(a, b)| a == b).count();
        while b > 0 && is_uniform(row(h - b)) {
            b -= 1;
        }
        let min_overlap = (self.opts.min_overlap as usize).max(1);
        if t + b + min_overlap >= h {
            return;
        }
        self.top = self.top.max(t);
        self.bottom = self.bottom.max(b);
        if self.top + self.bottom + min_overlap >= h {
            self.top = (self.opts.ignore_top as usize).min(h);
            self.bottom = (self.opts.ignore_bottom as usize).min(h - self.top);
        }
    }

    fn find_dy(&self, hashes: &[u64], pixels: &[u8], min_overlap: usize) -> Option<usize> {
        let stride = self.width * 4;
        let (top, bh) = (self.top, self.height - self.top - self.bottom);
        // Index previous body rows (non-uniform, not too repetitive) by hash.
        let mut index: HashMap<u64, Vec<u32>> = HashMap::with_capacity(bh);
        for y in 0..bh {
            let r = top + y;
            if is_uniform(&self.prev[r * stride..(r + 1) * stride]) {
                continue;
            }
            let e = index.entry(self.prev_hashes[r]).or_default();
            if e.len() <= MAX_ANCHOR_OCCURRENCES {
                e.push(y as u32);
            }
        }
        index.retain(|_, v| v.len() <= MAX_ANCHOR_OCCURRENCES);
        // Each matching row votes for the offset it implies.
        let mut votes: HashMap<usize, u32> = HashMap::new();
        for a in 0..bh {
            if let Some(list) = index.get(&hashes[top + a]) {
                for &p in list {
                    let p = p as usize;
                    if p > a && p - a + min_overlap <= bh {
                        *votes.entry(p - a).or_insert(0) += 1;
                    }
                }
            }
        }
        let mut cands: Vec<(u32, usize)> = votes.into_iter().map(|(dy, v)| (v, dy)).collect();
        cands.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        cands.truncate(MAX_CANDIDATES);
        cands.into_iter().map(|(_, dy)| dy).find(|&dy| {
            (0..bh - dy).all(|y| {
                let (n, p) = (top + y, top + y + dy);
                hashes[n] == self.prev_hashes[p]
                    || rows_close(
                        &pixels[n * stride..(n + 1) * stride],
                        &self.prev[p * stride..(p + 1) * stride],
                    )
            })
        })
    }

    /// Total rows of the stitched image so far.
    pub fn height(&self) -> u32 {
        let stride = (self.width * 4).max(1);
        if self.first_pending {
            return (self.prev.len() / stride) as u32;
        }
        ((self.result_len + self.footer.len()) / stride) as u32
    }

    pub fn finish(self) -> Image {
        if !self.started {
            return Image::new(0, 0);
        }
        if self.first_pending {
            return Image { width: self.width as u32, height: self.height as u32, pixels: self.prev };
        }
        let height = self.height();
        let mut pixels = Vec::with_capacity(self.result_len + self.footer.len());
        for c in &self.result {
            pixels.extend_from_slice(c);
        }
        pixels.extend_from_slice(&self.footer);
        Image { width: self.width as u32, height, pixels }
    }
}
