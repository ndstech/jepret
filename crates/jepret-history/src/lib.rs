//! History index. Owner: **T05**. See PRD F9, F10.
//!
//! SQLite with WAL mode so the daemon (writer) and gallery process (reader)
//! can open the DB concurrently. Schema migrations via `PRAGMA user_version`.

use std::path::{Path, PathBuf};

use jepret_core::{config::CleanupRules, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Image,
    Video,
}

#[derive(Debug, Clone)]
pub struct NewEntry {
    pub path: PathBuf,
    pub kind: EntryKind,
    /// Unix millis.
    pub created_at: i64,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    /// e.g. "region", "fullscreen" — `CaptureKind` snake_case name.
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: i64,
    pub path: PathBuf,
    pub kind: EntryKind,
    pub created_at: i64,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub source: String,
    pub favorite: bool,
    /// File no longer exists on disk (checked lazily).
    pub missing: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Query {
    pub offset: u32,
    pub limit: u32,
    /// Substring match on file name.
    pub search: Option<String>,
    pub kind: Option<EntryKind>,
    pub favorites_only: bool,
    /// Unix millis, inclusive.
    pub from: Option<i64>,
    pub to: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct CleanupReport {
    pub files_deleted: u32,
    pub bytes_freed: u64,
    pub entries_removed: u32,
    pub errors: Vec<String>,
}

pub struct HistoryDb {
    _private: (),
}

impl HistoryDb {
    /// Open or create (runs migrations).
    pub fn open(_path: &Path) -> Result<Self> {
        anyhow::bail!("TODO(T05): open")
    }

    pub fn insert(&self, _e: &NewEntry) -> Result<i64> {
        anyhow::bail!("TODO(T05): insert")
    }

    /// Newest first.
    pub fn list(&self, _q: &Query) -> Result<Vec<Entry>> {
        anyhow::bail!("TODO(T05): list")
    }

    pub fn count(&self, _q: &Query) -> Result<u64> {
        anyhow::bail!("TODO(T05): count")
    }

    pub fn get(&self, _id: i64) -> Result<Option<Entry>> {
        anyhow::bail!("TODO(T05): get")
    }

    pub fn set_favorite(&self, _id: i64, _favorite: bool) -> Result<()> {
        anyhow::bail!("TODO(T05): set_favorite")
    }

    /// Delete the entry; also delete the file (to recycle bin if asked)
    /// and its thumbnail.
    pub fn delete(&self, _id: i64, _delete_file: bool, _to_recycle_bin: bool) -> Result<()> {
        anyhow::bail!("TODO(T05): delete")
    }

    /// Remove entries whose file is missing. Returns count removed.
    pub fn prune_missing(&self) -> Result<u32> {
        anyhow::bail!("TODO(T05): prune_missing")
    }

    /// Apply cleanup rules (age, total size oldest-first, keep favorites).
    /// `now_ms` injected for testability.
    pub fn cleanup(&self, _rules: &CleanupRules, _now_ms: i64) -> Result<CleanupReport> {
        anyhow::bail!("TODO(T05): cleanup")
    }
}

/// Thumbnail path for an entry, generated on first request into
/// `thumbs_dir` (JPEG, longest side = `max_side`). Videos: placeholder
/// icon or first frame (Windows shell thumbnail provider) — T05 decides.
pub fn thumbnail(_entry: &Entry, _thumbs_dir: &Path, _max_side: u32) -> Result<PathBuf> {
    anyhow::bail!("TODO(T05): thumbnail")
}
