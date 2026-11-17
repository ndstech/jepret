//! Data source abstraction for the gallery.

use std::{
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

use jepret_core::{config::CleanupRules, Result};
use jepret_history::{CleanupReport, Entry, EntryKind, HistoryDb, Query};

/// Longest side of thumbnails requested from the history crate.
pub const THUMB_SIDE: u32 = 200;

/// Decoded thumbnail, RGBA8 straight alpha.
pub struct Thumb {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

pub trait Source: Send + Sync {
    /// Newest first.
    fn list(&self, q: &Query) -> Result<Vec<Entry>>;
    fn count(&self, q: &Query) -> Result<u64>;
    fn thumbnail(&self, entry: &Entry) -> Result<Thumb>;
    fn set_favorite(&self, id: i64, favorite: bool) -> Result<()>;
    fn delete(&self, id: i64, delete_file: bool, to_recycle_bin: bool) -> Result<()>;
    /// "Bersihkan sekarang": apply cleanup rules.
    fn cleanup(&self, now_ms: i64) -> Result<CleanupReport>;
    /// Remove entries whose file is gone. Returns count removed.
    fn prune_missing(&self) -> Result<u32>;
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Does `e` satisfy the filter part of `q` (ignores offset/limit)?
pub fn matches(e: &Entry, q: &Query) -> bool {
    if q.kind.is_some_and(|k| e.kind != k) {
        return false;
    }
    if q.favorites_only && !e.favorite {
        return false;
    }
    if q.from.is_some_and(|f| e.created_at < f) || q.to.is_some_and(|t| e.created_at > t) {
        return false;
    }
    if let Some(s) = q.search.as_deref().filter(|s| !s.is_empty()) {
        let name = e
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !name.contains(&s.to_lowercase()) {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------- Fake

/// In-memory fake data: synthetic entries and colored thumbnails.
pub struct FakeSource {
    entries: Mutex<Vec<Entry>>,
}

impl FakeSource {
    pub fn new(n: usize) -> Self {
        let base = 1_800_000_000_000i64; // fixed, deterministic
        let entries = (0..n)
            .map(|i| {
                let id = (n - i) as i64; // newest first => highest id first
                let video = i % 10 == 9;
                Entry {
                    id,
                    path: PathBuf::from(format!(
                        "C:\\fake\\Jepret_{id:05}.{}",
                        if video { "mp4" } else { "png" }
                    )),
                    kind: if video { EntryKind::Video } else { EntryKind::Image },
                    created_at: base - (i as i64) * 3_600_000,
                    width: 1920,
                    height: 1080,
                    bytes: 100_000 + (id as u64 % 997) * 1000,
                    source: "region".into(),
                    favorite: i % 7 == 0,
                    missing: i % 50 == 13,
                }
            })
            .collect();
        Self { entries: Mutex::new(entries) }
    }
}

fn hsv(h: f32, s: f32, v: f32) -> [u8; 3] {
    let c = v * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8]
}

impl Source for FakeSource {
    fn list(&self, q: &Query) -> Result<Vec<Entry>> {
        let g = lock(&self.entries);
        let it = g.iter().filter(|e| matches(e, q)).skip(q.offset as usize);
        Ok(if q.limit == 0 {
            it.cloned().collect()
        } else {
            it.take(q.limit as usize).cloned().collect()
        })
    }

    fn count(&self, q: &Query) -> Result<u64> {
        Ok(lock(&self.entries).iter().filter(|e| matches(e, q)).count() as u64)
    }

    fn thumbnail(&self, entry: &Entry) -> Result<Thumb> {
        let (w, h) = (200usize, 125usize);
        let hue = (entry.id as f32 * 47.0) % 360.0;
        let top = hsv(hue, 0.55, 0.95);
        let bot = hsv(hue + 40.0, 0.7, 0.45);
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            let t = y as f32 / (h - 1) as f32;
            let px = [0, 1, 2].map(|i| (top[i] as f32 * (1.0 - t) + bot[i] as f32 * t) as u8);
            for x in 0..w {
                if x < 2 || y < 2 || x >= w - 2 || y >= h - 2 {
                    rgba.extend_from_slice(&[255, 255, 255, 255]);
                } else {
                    rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
            }
        }
        Ok(Thumb { width: w, height: h, rgba })
    }

    fn set_favorite(&self, id: i64, favorite: bool) -> Result<()> {
        if let Some(e) = lock(&self.entries).iter_mut().find(|e| e.id == id) {
            e.favorite = favorite;
        }
        Ok(())
    }

    fn delete(&self, id: i64, _delete_file: bool, _to_recycle_bin: bool) -> Result<()> {
        lock(&self.entries).retain(|e| e.id != id);
        Ok(())
    }

    fn cleanup(&self, _now_ms: i64) -> Result<CleanupReport> {
        let n = self.prune_missing()?;
        Ok(CleanupReport { entries_removed: n, ..Default::default() })
    }

    fn prune_missing(&self) -> Result<u32> {
        let mut g = lock(&self.entries);
        let before = g.len();
        g.retain(|e| !e.missing);
        Ok((before - g.len()) as u32)
    }
}

// ------------------------------------------------------------- History

/// Real source over [`HistoryDb`].
pub struct HistorySource {
    db: Mutex<HistoryDb>,
    thumbs_dir: PathBuf,
    rules: CleanupRules,
}

impl HistorySource {
    pub fn open(db_path: &Path, thumbs_dir: &Path, rules: CleanupRules) -> Result<Self> {
        Ok(Self {
            db: Mutex::new(HistoryDb::open(db_path)?),
            thumbs_dir: thumbs_dir.to_path_buf(),
            rules,
        })
    }
}

impl Source for HistorySource {
    fn list(&self, q: &Query) -> Result<Vec<Entry>> {
        lock(&self.db).list(q)
    }

    fn count(&self, q: &Query) -> Result<u64> {
        lock(&self.db).count(q)
    }

    fn thumbnail(&self, entry: &Entry) -> Result<Thumb> {
        // DB lock not held: thumbnail generation only touches files.
        let path = jepret_history::thumbnail(entry, &self.thumbs_dir, THUMB_SIDE)?;
        let img = image::open(&path)?.to_rgba8();
        Ok(Thumb {
            width: img.width() as usize,
            height: img.height() as usize,
            rgba: img.into_raw(),
        })
    }

    fn set_favorite(&self, id: i64, favorite: bool) -> Result<()> {
        lock(&self.db).set_favorite(id, favorite)
    }

    fn delete(&self, id: i64, delete_file: bool, to_recycle_bin: bool) -> Result<()> {
        lock(&self.db).delete(id, delete_file, to_recycle_bin)
    }

    fn cleanup(&self, now_ms: i64) -> Result<CleanupReport> {
        // The user asked explicitly, so run even if auto-cleanup is disabled.
        let mut rules = self.rules.clone();
        rules.enabled = true;
        lock(&self.db).cleanup(&rules, now_ms)
    }

    fn prune_missing(&self) -> Result<u32> {
        lock(&self.db).prune_missing()
    }
}
