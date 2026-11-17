//! History index. Owner: **T05**. See PRD F9, F10.
//!
//! SQLite with WAL mode so the daemon (writer) and gallery process (reader)
//! can open the DB concurrently. Schema migrations via `PRAGMA user_version`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use rusqlite::{params, params_from_iter, types::Value, Connection};

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

/// Schema (user_version = 1): table `entries(id PK, path, name (lowercase
/// file name), kind 0/1, created_at ms, width, height, bytes, source,
/// favorite 0/1, missing 0/1)`, indexes on `created_at` and
/// `(favorite, created_at)`.
///
/// `Send` (owns one `Connection`), not `Sync`: each thread/process opens its
/// own handle; WAL + busy_timeout make that safe.
pub struct HistoryDb {
    conn: Connection,
    thumbs_dir: PathBuf,
}

const SCHEMA_V1: &str = "
CREATE TABLE IF NOT EXISTS entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL,
    name TEXT NOT NULL,
    kind INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    bytes INTEGER NOT NULL,
    source TEXT NOT NULL,
    favorite INTEGER NOT NULL DEFAULT 0,
    missing INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_entries_created ON entries(created_at);
CREATE INDEX IF NOT EXISTS idx_entries_fav ON entries(favorite, created_at);
";

const COLS: &str = "id, path, kind, created_at, width, height, bytes, source, favorite, missing";

type Victim = (i64, String, i64);

fn kind_to_i(k: EntryKind) -> i64 {
    match k {
        EntryKind::Image => 0,
        EntryKind::Video => 1,
    }
}

fn row_to_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    let path: String = r.get(1)?;
    Ok(Entry {
        id: r.get(0)?,
        path: PathBuf::from(path),
        kind: if r.get::<_, i64>(2)? == 1 { EntryKind::Video } else { EntryKind::Image },
        created_at: r.get(3)?,
        width: r.get::<_, i64>(4)? as u32,
        height: r.get::<_, i64>(5)? as u32,
        bytes: r.get::<_, i64>(6)? as u64,
        source: r.get(7)?,
        favorite: r.get::<_, i64>(8)? != 0,
        missing: r.get::<_, i64>(9)? != 0,
    })
}

fn like_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('%');
    for c in s.to_lowercase().chars() {
        if matches!(c, '%' | '_' | '\\') {
            o.push('\\');
        }
        o.push(c);
    }
    o.push('%');
    o
}

fn where_clause(q: &Query) -> (String, Vec<Value>) {
    let mut conds: Vec<&str> = Vec::new();
    let mut args: Vec<Value> = Vec::new();
    if let Some(s) = q.search.as_deref().filter(|s| !s.is_empty()) {
        conds.push("name LIKE ? ESCAPE '\\'");
        args.push(Value::Text(like_escape(s)));
    }
    if let Some(k) = q.kind {
        conds.push("kind = ?");
        args.push(Value::Integer(kind_to_i(k)));
    }
    if q.favorites_only {
        conds.push("favorite = 1");
    }
    if let Some(f) = q.from {
        conds.push("created_at >= ?");
        args.push(Value::Integer(f));
    }
    if let Some(t) = q.to {
        conds.push("created_at <= ?");
        args.push(Value::Integer(t));
    }
    let w = if conds.is_empty() { String::new() } else { format!(" WHERE {}", conds.join(" AND ")) };
    (w, args)
}

impl HistoryDb {
    /// Open or create (runs migrations). Thumbnails live in `<db dir>/thumbs`
    /// (override with [`HistoryDb::with_thumbs_dir`]).
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        // Returns a row ("wal"), so it must be read.
        conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get::<_, String>(0))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let thumbs_dir = path.parent().unwrap_or(Path::new("")).join("thumbs");
        let db = Self { conn, thumbs_dir };
        db.migrate()?;
        Ok(db)
    }

    /// Directory whose thumbnails are removed when an entry is deleted.
    pub fn with_thumbs_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.thumbs_dir = dir.into();
        self
    }

    pub fn thumbs_dir(&self) -> &Path {
        &self.thumbs_dir
    }

    fn migrate(&self) -> Result<()> {
        let v: i64 = self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if v < 1 {
            // IMMEDIATE so two processes migrating at once serialize.
            self.conn.execute_batch(&format!(
                "BEGIN IMMEDIATE; {SCHEMA_V1} PRAGMA user_version = 1; COMMIT;"
            ))?;
        }
        Ok(())
    }

    pub fn insert(&self, e: &NewEntry) -> Result<i64> {
        let name = e
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        self.conn.execute(
            "INSERT INTO entries (path, name, kind, created_at, width, height, bytes, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                e.path.to_string_lossy(),
                name,
                kind_to_i(e.kind),
                e.created_at,
                e.width as i64,
                e.height as i64,
                e.bytes as i64,
                e.source
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Newest first. `limit == 0` means no limit. Does not touch the disk;
    /// `missing` reflects the last [`HistoryDb::refresh_missing`]/`get`.
    pub fn list(&self, q: &Query) -> Result<Vec<Entry>> {
        let (w, mut args) = where_clause(q);
        let limit = if q.limit == 0 { -1 } else { q.limit as i64 };
        args.push(Value::Integer(limit));
        args.push(Value::Integer(q.offset as i64));
        let sql = format!(
            "SELECT {COLS} FROM entries{w} ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?"
        );
        let mut st = self.conn.prepare_cached(&sql)?;
        let rows = st.query_map(params_from_iter(args), row_to_entry)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn count(&self, q: &Query) -> Result<u64> {
        let (w, args) = where_clause(q);
        let sql = format!("SELECT COUNT(*) FROM entries{w}");
        let n: i64 = self.conn.query_row(&sql, params_from_iter(args), |r| r.get(0))?;
        Ok(n as u64)
    }

    /// Fetch one entry; checks the file on disk and updates `missing`.
    pub fn get(&self, id: i64) -> Result<Option<Entry>> {
        let found = {
            let mut st =
                self.conn.prepare_cached(&format!("SELECT {COLS} FROM entries WHERE id = ?"))?;
            let mut rows = st.query_map([id], row_to_entry)?;
            rows.next().transpose()?
        };
        let Some(mut e) = found else { return Ok(None) };
        self.refresh_missing(std::slice::from_mut(&mut e))?;
        Ok(Some(e))
    }

    /// Lazy missing check for a visible page: stats each file, updates
    /// `entry.missing` and persists changes.
    pub fn refresh_missing(&self, entries: &mut [Entry]) -> Result<()> {
        for e in entries {
            let missing = !e.path.exists();
            if missing != e.missing {
                self.conn.execute(
                    "UPDATE entries SET missing = ?1 WHERE id = ?2",
                    params![missing as i64, e.id],
                )?;
                e.missing = missing;
            }
        }
        Ok(())
    }

    pub fn set_favorite(&self, id: i64, favorite: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE entries SET favorite = ?1 WHERE id = ?2",
            params![favorite as i64, id],
        )?;
        Ok(())
    }

    /// Delete the entry; also delete the file (to recycle bin if asked)
    /// and its thumbnail. If the file cannot be deleted the entry is kept
    /// and an error is returned.
    pub fn delete(&self, id: i64, delete_file: bool, to_recycle_bin: bool) -> Result<()> {
        let path: Option<String> = match self
            .conn
            .query_row("SELECT path FROM entries WHERE id = ?", [id], |r| r.get(0))
        {
            Ok(p) => Some(p),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.into()),
        };
        let Some(path) = path else { return Ok(()) };
        if delete_file {
            remove_file(Path::new(&path), to_recycle_bin)?;
        }
        self.conn.execute("DELETE FROM entries WHERE id = ?", [id])?;
        remove_thumbs(&self.thumbs_dir, id);
        Ok(())
    }

    /// Remove entries whose file is missing. Returns count removed.
    pub fn prune_missing(&self) -> Result<u32> {
        let mut gone = Vec::new();
        {
            let mut st = self.conn.prepare("SELECT id, path FROM entries")?;
            let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            for r in rows {
                let (id, p) = r?;
                if !Path::new(&p).exists() {
                    gone.push(id);
                }
            }
        }
        let tx = self.conn.unchecked_transaction()?;
        for id in &gone {
            tx.execute("DELETE FROM entries WHERE id = ?", [id])?;
        }
        tx.commit()?;
        for id in &gone {
            remove_thumbs(&self.thumbs_dir, *id);
        }
        Ok(gone.len() as u32)
    }

    /// Apply cleanup rules: (1) age > `max_age_days`, (2) then while total
    /// size > `max_total_mb` delete oldest first; favorites are skipped when
    /// `keep_favorites`. Errors are collected, never abort. Does not check
    /// `rules.enabled` (the caller gates on it). `now_ms` injected for tests.
    pub fn cleanup(&self, rules: &CleanupRules, now_ms: i64) -> Result<CleanupReport> {
        let mut rep = CleanupReport::default();
        let fav_skip = if rules.keep_favorites { " AND favorite = 0" } else { "" };

        if let Some(days) = rules.max_age_days {
            let cutoff = now_ms.saturating_sub(days as i64 * 86_400_000);
            let victims = self.collect(
                &format!(
                    "SELECT id, path, bytes FROM entries WHERE created_at < ?1{fav_skip} ORDER BY created_at"
                ),
                params![cutoff],
            )?;
            self.remove_all(victims, rules.use_recycle_bin, &mut rep);
        }

        if let Some(mb) = rules.max_total_mb {
            let limit = mb.saturating_mul(1024 * 1024) as i64;
            let total: i64 = self
                .conn
                .query_row("SELECT COALESCE(SUM(bytes), 0) FROM entries", [], |r| r.get(0))?;
            if total > limit {
                let mut excess = total - limit;
                let all = self.collect(
                    &format!(
                        "SELECT id, path, bytes FROM entries WHERE 1=1{fav_skip} ORDER BY created_at, id"
                    ),
                    [],
                )?;
                let mut victims = Vec::new();
                for v in all {
                    if excess <= 0 {
                        break;
                    }
                    excess -= v.2;
                    victims.push(v);
                }
                self.remove_all(victims, rules.use_recycle_bin, &mut rep);
            }
        }
        Ok(rep)
    }

    fn collect(&self, sql: &str, p: impl rusqlite::Params) -> Result<Vec<Victim>> {
        let mut st = self.conn.prepare(sql)?;
        let rows = st.query_map(p, |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn remove_all(&self, victims: Vec<Victim>, recycle: bool, rep: &mut CleanupReport) {
        for (id, path, bytes) in victims {
            let p = Path::new(&path);
            let existed = p.exists();
            if existed {
                if let Err(e) = remove_file(p, recycle) {
                    rep.errors.push(format!("{path}: {e:#}"));
                    continue;
                }
            }
            match self.conn.execute("DELETE FROM entries WHERE id = ?", [id]) {
                Ok(_) => {
                    remove_thumbs(&self.thumbs_dir, id);
                    rep.entries_removed += 1;
                    if existed {
                        rep.files_deleted += 1;
                        rep.bytes_freed += bytes.max(0) as u64;
                    }
                }
                Err(e) => rep.errors.push(format!("db delete {id}: {e}")),
            }
        }
    }
}

fn remove_file(path: &Path, recycle: bool) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if recycle {
        trash::delete(path).map_err(|e| anyhow::anyhow!("recycle {}: {e}", path.display()))
    } else {
        std::fs::remove_file(path).with_context(|| format!("remove {}", path.display()))
    }
}

/// Best-effort removal of `<id>_*.jpg`.
fn remove_thumbs(dir: &Path, id: i64) {
    let prefix = format!("{id}_");
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let n = e.file_name();
        let n = n.to_string_lossy();
        if n.starts_with(&prefix) && n.ends_with(".jpg") && std::fs::remove_file(e.path()).is_err() {
            tracing::warn!("could not remove thumbnail {n}");
        }
    }
}

/// Thumbnail path for an entry, generated on first request into
/// `thumbs_dir` (JPEG, longest side = `max_side`, never upscaled), named
/// `<id>_<max_side>.jpg`; regenerated if the source is newer. Videos get a
/// generated placeholder (dark tile with play triangle).
pub fn thumbnail(entry: &Entry, thumbs_dir: &Path, max_side: u32) -> Result<PathBuf> {
    let max_side = max_side.max(1);
    let out = thumbs_dir.join(format!("{}_{}.jpg", entry.id, max_side));
    let src_mtime = std::fs::metadata(&entry.path)
        .with_context(|| format!("stat {}", entry.path.display()))?
        .modified()
        .ok();
    if let (Ok(tm), Some(sm)) = (std::fs::metadata(&out).and_then(|m| m.modified()), src_mtime) {
        if tm >= sm {
            return Ok(out);
        }
    }
    std::fs::create_dir_all(thumbs_dir)?;
    let rgb = match entry.kind {
        EntryKind::Image => {
            let img = image::open(&entry.path)
                .with_context(|| format!("decode {}", entry.path.display()))?;
            let img = if img.width() > max_side || img.height() > max_side {
                img.thumbnail(max_side, max_side)
            } else {
                img
            };
            img.to_rgb8()
        }
        EntryKind::Video => video_placeholder(max_side),
    };
    let tmp = thumbs_dir.join(format!("{}_{}.tmp", entry.id, max_side));
    rgb.save_with_format(&tmp, image::ImageFormat::Jpeg)?;
    std::fs::rename(&tmp, &out)?;
    Ok(out)
}

fn video_placeholder(side: u32) -> image::RgbImage {
    let (w, h) = (side, (side * 9 / 16).max(1));
    let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([32, 32, 36]));
    let (cx, cy, r) = (w as f32 / 2.0, h as f32 / 2.0, h as f32 / 4.0);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 - cx + r / 3.0, (y as f32 - cy).abs());
            // right-pointing triangle
            if dx >= -r && dx <= r && dy <= (r - dx) * 0.55 {
                img.put_pixel(x, y, image::Rgb([230, 230, 230]));
            }
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const DAY: i64 = 86_400_000;
    const NOW: i64 = 1_800_000_000_000;

    fn db(dir: &Path) -> HistoryDb {
        HistoryDb::open(&dir.join("history.db")).unwrap()
    }

    fn add(db: &HistoryDb, dir: &Path, name: &str, age_days: i64, bytes: usize) -> i64 {
        let path = dir.join(name);
        std::fs::write(&path, vec![0u8; bytes]).unwrap();
        db.insert(&NewEntry {
            path,
            kind: EntryKind::Image,
            created_at: NOW - age_days * DAY,
            width: 10,
            height: 10,
            bytes: bytes as u64,
            source: "region".into(),
        })
        .unwrap()
    }

    fn rules() -> CleanupRules {
        CleanupRules {
            enabled: true,
            max_age_days: None,
            max_total_mb: None,
            keep_favorites: true,
            use_recycle_bin: false,
            run_on_startup: false,
        }
    }

    fn q(limit: u32) -> Query {
        Query { limit, ..Default::default() }
    }

    #[test]
    fn is_send() {
        fn s<T: Send>() {}
        s::<HistoryDb>();
    }

    #[test]
    fn insert_list_paging_search_favorite_delete() {
        let t = tempfile::tempdir().unwrap();
        let d = db(t.path());
        let a = add(&d, t.path(), "Alpha_1.png", 3, 5);
        let b = add(&d, t.path(), "beta.png", 2, 5);
        let c = add(&d, t.path(), "gamma%.png", 1, 5);
        let all = d.list(&q(10)).unwrap();
        assert_eq!(all.iter().map(|e| e.id).collect::<Vec<_>>(), vec![c, b, a]);
        assert_eq!(d.count(&q(0)).unwrap(), 3);
        let p2 = d.list(&Query { offset: 1, limit: 1, ..Default::default() }).unwrap();
        assert_eq!(p2.len(), 1);
        assert_eq!(p2[0].id, b);
        let s = |x: &str| Query { limit: 10, search: Some(x.into()), ..Default::default() };
        assert_eq!(d.list(&s("ALPHA")).unwrap().len(), 1);
        assert_eq!(d.list(&s("%")).unwrap().len(), 1);
        assert_eq!(d.list(&s("_")).unwrap().len(), 1);
        let r = d
            .list(&Query { limit: 10, from: Some(NOW - 2 * DAY), to: Some(NOW - 2 * DAY), ..Default::default() })
            .unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, b);
        d.set_favorite(b, true).unwrap();
        let f = Query { limit: 10, favorites_only: true, ..Default::default() };
        assert_eq!(d.list(&f).unwrap()[0].id, b);
        assert_eq!(d.count(&f).unwrap(), 1);
        let v = Query { limit: 10, kind: Some(EntryKind::Video), ..Default::default() };
        assert_eq!(d.list(&v).unwrap().len(), 0);
        let e = d.get(b).unwrap().unwrap();
        assert!(e.favorite && !e.missing);
        d.delete(b, true, false).unwrap();
        assert!(d.get(b).unwrap().is_none());
        assert!(!t.path().join("beta.png").exists());
        d.delete(a, false, false).unwrap();
        assert!(t.path().join("Alpha_1.png").exists());
        assert_eq!(d.count(&q(0)).unwrap(), 1);
    }

    #[test]
    fn cleanup_by_age_keeps_favorites() {
        let t = tempfile::tempdir().unwrap();
        let d = db(t.path());
        let old = add(&d, t.path(), "old.png", 40, 100);
        let oldfav = add(&d, t.path(), "oldfav.png", 50, 100);
        let new = add(&d, t.path(), "new.png", 1, 100);
        d.set_favorite(oldfav, true).unwrap();
        let rep = d.cleanup(&CleanupRules { max_age_days: Some(30), ..rules() }, NOW).unwrap();
        assert_eq!((rep.files_deleted, rep.bytes_freed, rep.entries_removed), (1, 100, 1));
        assert!(rep.errors.is_empty());
        assert!(d.get(old).unwrap().is_none());
        assert!(d.get(oldfav).unwrap().is_some());
        assert!(d.get(new).unwrap().is_some());
        let r2 = CleanupRules { max_age_days: Some(30), keep_favorites: false, ..rules() };
        let rep = d.cleanup(&r2, NOW).unwrap();
        assert_eq!(rep.files_deleted, 1);
        assert!(d.get(oldfav).unwrap().is_none());
    }

    #[test]
    fn cleanup_by_size_oldest_first_skips_favorites() {
        let t = tempfile::tempdir().unwrap();
        let d = db(t.path());
        let mb = 1024 * 1024;
        let a = add(&d, t.path(), "a.png", 5, mb); // oldest, favorite
        let b = add(&d, t.path(), "b.png", 4, mb);
        let c = add(&d, t.path(), "c.png", 3, mb);
        let e = add(&d, t.path(), "e.png", 2, mb);
        d.set_favorite(a, true).unwrap();
        // total 4 MB, limit 2 MB -> remove 2 MB: b then c (a protected)
        let rep = d.cleanup(&CleanupRules { max_total_mb: Some(2), ..rules() }, NOW).unwrap();
        assert_eq!(rep.files_deleted, 2);
        assert_eq!(rep.bytes_freed, 2 * mb as u64);
        assert!(d.get(a).unwrap().is_some());
        assert!(d.get(b).unwrap().is_none());
        assert!(d.get(c).unwrap().is_none());
        assert!(d.get(e).unwrap().is_some());
    }

    #[test]
    fn cleanup_handles_already_missing_file() {
        let t = tempfile::tempdir().unwrap();
        let d = db(t.path());
        let a = add(&d, t.path(), "a.png", 40, 10);
        let b = add(&d, t.path(), "b.png", 41, 10);
        std::fs::remove_file(t.path().join("a.png")).unwrap();
        let rep = d.cleanup(&CleanupRules { max_age_days: Some(30), ..rules() }, NOW).unwrap();
        assert_eq!(rep.entries_removed, 2);
        assert_eq!(rep.files_deleted, 1);
        assert!(d.get(a).unwrap().is_none() && d.get(b).unwrap().is_none());
    }

    #[test]
    fn prune_missing_and_flag() {
        let t = tempfile::tempdir().unwrap();
        let d = db(t.path());
        let a = add(&d, t.path(), "a.png", 1, 1);
        let b = add(&d, t.path(), "b.png", 2, 1);
        std::fs::remove_file(t.path().join("a.png")).unwrap();
        assert!(d.get(a).unwrap().unwrap().missing);
        assert!(d.list(&q(10)).unwrap().iter().any(|e| e.id == a && e.missing));
        assert_eq!(d.prune_missing().unwrap(), 1);
        assert!(d.get(a).unwrap().is_none());
        assert!(d.get(b).unwrap().is_some());
    }

    #[test]
    fn reopen_persists_and_wal_enabled() {
        let t = tempfile::tempdir().unwrap();
        let id = {
            let d = db(t.path());
            add(&d, t.path(), "a.png", 1, 1)
        };
        let d1 = db(t.path());
        let d2 = db(t.path());
        assert_eq!(d1.get(id).unwrap().unwrap().id, id);
        let mode: String = d2.conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(mode, "wal");
        let v: i64 = d2.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 1);
        add(&d1, t.path(), "b.png", 1, 1);
        add(&d2, t.path(), "c.png", 1, 1);
        assert_eq!(d1.count(&q(0)).unwrap(), 3);
    }

    #[test]
    fn thumbnail_generate_cache_regenerate_and_delete() {
        let t = tempfile::tempdir().unwrap();
        let d = db(t.path());
        let src = t.path().join("s.png");
        image::RgbaImage::from_pixel(400, 200, image::Rgba([200, 10, 10, 255])).save(&src).unwrap();
        let id = d
            .insert(&NewEntry {
                path: src.clone(),
                kind: EntryKind::Image,
                created_at: NOW,
                width: 400,
                height: 200,
                bytes: 1,
                source: "region".into(),
            })
            .unwrap();
        let e = d.get(id).unwrap().unwrap();
        let th = thumbnail(&e, d.thumbs_dir(), 100).unwrap();
        assert_eq!(th.file_name().unwrap().to_string_lossy(), format!("{id}_100.jpg"));
        let im = image::open(&th).unwrap();
        assert_eq!((im.width(), im.height()), (100, 50));
        let m1 = std::fs::metadata(&th).unwrap().modified().unwrap();
        thumbnail(&e, d.thumbs_dir(), 100).unwrap();
        assert_eq!(m1, std::fs::metadata(&th).unwrap().modified().unwrap());
        // newer source -> regenerated
        std::thread::sleep(Duration::from_millis(50));
        image::RgbaImage::from_pixel(80, 80, image::Rgba([0, 0, 255, 255])).save(&src).unwrap();
        thumbnail(&e, d.thumbs_dir(), 100).unwrap();
        let im = image::open(&th).unwrap();
        assert_eq!((im.width(), im.height()), (80, 80)); // not upscaled
        d.delete(id, false, false).unwrap();
        assert!(!th.exists());
    }

    #[test]
    fn video_thumbnail_placeholder() {
        let t = tempfile::tempdir().unwrap();
        let v = t.path().join("v.mp4");
        std::fs::write(&v, b"x").unwrap();
        let e = Entry {
            id: 7,
            path: v,
            kind: EntryKind::Video,
            created_at: NOW,
            width: 0,
            height: 0,
            bytes: 1,
            source: "recording".into(),
            favorite: false,
            missing: false,
        };
        let th = thumbnail(&e, &t.path().join("thumbs"), 160).unwrap();
        assert!(th.exists());
    }

    #[test]
    fn bench_list_10k() {
        let t = tempfile::tempdir().unwrap();
        let d = db(t.path());
        d.conn.execute_batch("BEGIN").unwrap();
        for i in 0..10_000i64 {
            d.insert(&NewEntry {
                path: PathBuf::from(format!("C:/x/shot_{i}.png")),
                kind: EntryKind::Image,
                created_at: NOW + i,
                width: 1,
                height: 1,
                bytes: 1,
                source: "region".into(),
            })
            .unwrap();
        }
        d.conn.execute_batch("COMMIT").unwrap();
        d.list(&q(200)).unwrap(); // warm
        let t0 = Instant::now();
        let r = d.list(&q(200)).unwrap();
        let el = t0.elapsed();
        assert_eq!(r.len(), 200);
        assert_eq!(r[0].created_at, NOW + 9_999);
        println!("list 200 of 10k: {el:?}");
        // debug builds are slower; release target is 5 ms.
        let budget = if cfg!(debug_assertions) { 50 } else { 5 };
        assert!(el.as_millis() <= budget, "too slow: {el:?}");
    }

    #[test]
    #[ignore = "touches the real Windows recycle bin"]
    fn cleanup_to_recycle_bin() {
        let t = tempfile::tempdir().unwrap();
        let d = db(t.path());
        add(&d, t.path(), "old.png", 40, 10);
        let r = CleanupRules { max_age_days: Some(30), use_recycle_bin: true, ..rules() };
        let rep = d.cleanup(&r, NOW).unwrap();
        assert_eq!(rep.files_deleted, 1, "{:?}", rep.errors);
        assert!(!t.path().join("old.png").exists());
    }
}
