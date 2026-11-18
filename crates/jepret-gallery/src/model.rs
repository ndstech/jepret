//! UI-independent logic: filter -> query, paging, LRU cache, grid math.

use std::{collections::HashMap, hash::Hash};

use chrono::{Local, NaiveDate, NaiveTime, TimeZone};
use jepret_history::{Entry, EntryKind, Query};

use crate::source::Source;

/// Entries fetched per DB page.
pub const PAGE_SIZE: u32 = 120;

// ------------------------------------------------------------------ Filter

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KindFilter {
    #[default]
    All,
    Image,
    Video,
}

/// Filter state as typed in the UI.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Filter {
    pub search: String,
    pub kind: KindFilter,
    pub favorites_only: bool,
    /// `YYYY-MM-DD` or empty (local time, inclusive).
    pub from: String,
    pub to: String,
}

fn parse_day(s: &str, end_of_day: bool) -> Result<Option<i64>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let date = NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| format!("Tanggal tidak valid: \"{s}\" (format YYYY-MM-DD)"))?;
    let time = if end_of_day {
        NaiveTime::from_hms_milli_opt(23, 59, 59, 999)
    } else {
        NaiveTime::from_hms_opt(0, 0, 0)
    }
    .ok_or_else(|| "waktu tidak valid".to_string())?;
    Local
        .from_local_datetime(&date.and_time(time))
        .earliest()
        .map(|d| Some(d.timestamp_millis()))
        .ok_or_else(|| format!("Tanggal tidak valid: \"{s}\""))
}

impl Filter {
    /// Build a [`Query`] with `offset = 0, limit = 0` (paging fills those in).
    pub fn to_query(&self) -> Result<Query, String> {
        let from = parse_day(&self.from, false)?;
        let to = parse_day(&self.to, true)?;
        if let (Some(f), Some(t)) = (from, to) {
            if f > t {
                return Err("Tanggal \"dari\" lebih besar dari \"sampai\"".into());
            }
        }
        let search = self.search.trim();
        Ok(Query {
            offset: 0,
            limit: 0,
            search: (!search.is_empty()).then(|| search.to_string()),
            kind: match self.kind {
                KindFilter::All => None,
                KindFilter::Image => Some(EntryKind::Image),
                KindFilter::Video => Some(EntryKind::Video),
            },
            favorites_only: self.favorites_only,
            from,
            to,
        })
    }
}

// ------------------------------------------------- Sidebar filter mapping

/// "Koleksi" rows of the filter panel (mutually exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Collection {
    All,
    Favorites,
    Images,
    Videos,
}

/// "Tanggal" presets of the filter panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatePreset {
    Today,
    Yesterday,
    /// Today and the six days before it.
    Last7,
}

impl DatePreset {
    /// `(from, to)` as typed into [`Filter`] (`YYYY-MM-DD`, empty = open).
    pub fn range(self, today: NaiveDate) -> (String, String) {
        let fmt = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
        match self {
            DatePreset::Today => (fmt(today), fmt(today)),
            DatePreset::Yesterday => {
                let y = today.pred_opt().unwrap_or(today);
                (fmt(y), fmt(y))
            }
            DatePreset::Last7 => (fmt(today - chrono::Duration::days(6)), String::new()),
        }
    }
}

impl Filter {
    pub fn collection(&self) -> Collection {
        if self.favorites_only {
            return Collection::Favorites;
        }
        match self.kind {
            KindFilter::All => Collection::All,
            KindFilter::Image => Collection::Images,
            KindFilter::Video => Collection::Videos,
        }
    }

    pub fn set_collection(&mut self, c: Collection) {
        self.favorites_only = c == Collection::Favorites;
        self.kind = match c {
            Collection::Images => KindFilter::Image,
            Collection::Videos => KindFilter::Video,
            Collection::All | Collection::Favorites => KindFilter::All,
        };
    }

    /// Preset matching the current date fields exactly, if any.
    pub fn date_preset(&self, today: NaiveDate) -> Option<DatePreset> {
        let cur = (self.from.trim(), self.to.trim());
        [DatePreset::Today, DatePreset::Yesterday, DatePreset::Last7].into_iter().find(|p| {
            let (f, t) = p.range(today);
            (f.as_str(), t.as_str()) == cur
        })
    }

    pub fn set_date_preset(&mut self, p: Option<DatePreset>, today: NaiveDate) {
        let (f, t) = p.map(|p| p.range(today)).unwrap_or_default();
        self.from = f;
        self.to = t;
    }

    /// Any date bound that is not one of the presets.
    pub fn has_custom_date(&self, today: NaiveDate) -> bool {
        (!self.from.trim().is_empty() || !self.to.trim().is_empty()) && self.date_preset(today).is_none()
    }
}

/// Whole-history totals for the sidebar counts and status bar. Filled by one
/// pass over the list; entries themselves are never kept.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub all: u64,
    pub images: u64,
    pub videos: u64,
    pub favorites: u64,
    pub today: u64,
    pub yesterday: u64,
    pub last7: u64,
    pub missing: u64,
    pub bytes: u64,
}

impl Counts {
    pub fn add(&mut self, e: &Entry, today: NaiveDate) {
        self.all += 1;
        self.bytes += e.bytes;
        self.missing += u64::from(e.missing);
        self.favorites += u64::from(e.favorite);
        match e.kind {
            EntryKind::Image => self.images += 1,
            EntryKind::Video => self.videos += 1,
        }
        let age = (today - local_day(e.created_at)).num_days();
        if (0..=6).contains(&age) {
            self.last7 += 1;
        }
        match age {
            0 => self.today += 1,
            1 => self.yesterday += 1,
            _ => {}
        }
    }
}

/// Size of a `w x h` image scaled to fit inside `box_w x box_h` keeping the
/// aspect ratio (never cropped or stretched). Unknown size fills the box.
pub fn fit_inside(box_w: f32, box_h: f32, w: f32, h: f32) -> (f32, f32) {
    if w <= 0.0 || h <= 0.0 || box_w <= 0.0 || box_h <= 0.0 {
        return (box_w.max(0.0), box_h.max(0.0));
    }
    let k = (box_w / w).min(box_h / h);
    (w * k, h * k)
}

/// Card width that makes `cols` cards plus gaps fill `avail` exactly.
pub fn card_width(avail: f32, cols: usize, gap: f32) -> f32 {
    ((avail - gap * (cols.max(1) as f32 - 1.0)) / cols.max(1) as f32).floor().max(120.0)
}

/// "18 Nov 2026".
pub fn short_date(day: NaiveDate) -> String {
    use chrono::Datelike;
    format!("{} {} {}", day.day(), &MONTHS_ID[day.month0() as usize][..3], day.year())
}

// --------------------------------------------------------------------- LRU

/// Small LRU cache (tick based; eviction is O(n), n is a few hundred).
pub struct LruCache<K, V> {
    cap: usize,
    tick: u64,
    map: HashMap<K, (V, u64)>,
}

impl<K: Eq + Hash + Clone, V> LruCache<K, V> {
    pub fn new(cap: usize) -> Self {
        Self { cap: cap.max(1), tick: 0, map: HashMap::new() }
    }

    pub fn get(&mut self, k: &K) -> Option<&V> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(k).map(|(v, used)| {
            *used = t;
            &*v
        })
    }

    pub fn contains(&self, k: &K) -> bool {
        self.map.contains_key(k)
    }

    pub fn put(&mut self, k: K, v: V) {
        self.tick += 1;
        self.map.insert(k, (v, self.tick));
        while self.map.len() > self.cap {
            let oldest = self.map.iter().min_by_key(|(_, (_, t))| *t).map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    self.map.remove(&k);
                }
                None => break,
            }
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }
}

// ------------------------------------------------------------------ Paging

/// Lazily paged view of a query result; only a few pages stay in memory.
pub struct PagedList {
    query: Query,
    total: u64,
    pages: LruCache<u64, Vec<Entry>>,
    pub last_error: Option<String>,
}

impl PagedList {
    pub fn new(max_pages: usize) -> Self {
        Self {
            query: Query::default(),
            total: 0,
            pages: LruCache::new(max_pages),
            last_error: None,
        }
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    /// Switch to a new query and recount.
    pub fn load(&mut self, src: &dyn Source, query: Query) {
        self.query = Query { offset: 0, limit: 0, ..query };
        self.refresh(src);
    }

    /// Recount and drop cached pages (after favorite/delete/cleanup).
    pub fn refresh(&mut self, src: &dyn Source) {
        self.pages.clear();
        match src.count(&self.query) {
            Ok(n) => {
                self.total = n;
                self.last_error = None;
            }
            Err(e) => {
                self.total = 0;
                self.last_error = Some(format!("{e:#}"));
            }
        }
    }

    /// Entry at absolute index `idx`, fetching its page if needed.
    pub fn get(&mut self, src: &dyn Source, idx: u64) -> Option<Entry> {
        if idx >= self.total {
            return None;
        }
        let page = idx / PAGE_SIZE as u64;
        if !self.pages.contains(&page) {
            let q = Query {
                offset: (page * PAGE_SIZE as u64) as u32,
                limit: PAGE_SIZE,
                ..self.query.clone()
            };
            let rows = match src.list(&q) {
                Ok(r) => r,
                Err(e) => {
                    self.last_error = Some(format!("{e:#}"));
                    Vec::new()
                }
            };
            self.pages.put(page, rows);
        }
        self.pages.get(&page)?.get((idx % PAGE_SIZE as u64) as usize).cloned()
    }
}

// --------------------------------------------------------------- Grid math

/// Number of cells per row for the available width (at least 1).
pub fn grid_columns(avail_width: f32, cell_w: f32, spacing: f32) -> usize {
    (((avail_width + spacing) / (cell_w + spacing)).floor() as usize).max(1)
}

// ------------------------------------------------- Day groups & layout

/// A run of consecutive entries created on the same local day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayRun {
    pub day: NaiveDate,
    /// Absolute index of the first entry in the run.
    pub start: u64,
    pub count: u64,
}

/// Local calendar day of a unix-millis timestamp.
pub fn local_day(ms: i64) -> NaiveDate {
    Local
        .timestamp_millis_opt(ms)
        .earliest()
        .map(|d| d.date_naive())
        .unwrap_or(NaiveDate::MIN)
}

/// Append one entry (newest first) to `runs`, extending the last run if same day.
pub fn push_day(runs: &mut Vec<DayRun>, day: NaiveDate) {
    match runs.last_mut() {
        Some(r) if r.day == day => r.count += 1,
        _ => {
            let start = runs.last().map(|r| r.start + r.count).unwrap_or(0);
            runs.push(DayRun { day, start, count: 1 });
        }
    }
}

const MONTHS_ID: [&str; 12] = [
    "Januari", "Februari", "Maret", "April", "Mei", "Juni", "Juli", "Agustus", "September",
    "Oktober", "November", "Desember",
];

/// "Hari ini", "Kemarin", otherwise "18 November 2026".
pub fn day_label(day: NaiveDate, today: NaiveDate) -> String {
    use chrono::Datelike;
    if day == today {
        "Hari ini".into()
    } else if today.pred_opt() == Some(day) {
        "Kemarin".into()
    } else {
        format!("{} {} {}", day.day(), MONTHS_ID[day.month0() as usize], day.year())
    }
}

/// Human label for `Entry::source` (snake_case `CaptureKind` or "recording").
pub fn source_label(source: &str) -> String {
    match source {
        "region" => "Region".into(),
        "fullscreen" => "Fullscreen".into(),
        "active_monitor" => "Monitor".into(),
        "active_window" => "Window".into(),
        "last_region" => "Last region".into(),
        "scroll" => "Scroll".into(),
        "record_region" | "record_monitor" | "recording" => "Rekam".into(),
        "" => "Capture".into(),
        other => {
            let mut c = other.replace('_', " ").chars().collect::<Vec<_>>();
            if let Some(f) = c.first_mut() {
                *f = f.to_ascii_uppercase();
            }
            c.into_iter().collect()
        }
    }
}

/// `1284` -> "1.284".
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(ch);
    }
    out
}

/// "3,2 GB" / "380 MB" / "12 KB" (decimal comma).
pub fn human_bytes(b: u64) -> String {
    const MB: f64 = 1_048_576.0;
    let f = b as f64;
    if f >= 1024.0 * MB {
        format!("{:.1} GB", f / (1024.0 * MB)).replace('.', ",")
    } else if f >= MB {
        format!("{:.0} MB", f / MB)
    } else {
        format!("{:.0} KB", f / 1024.0)
    }
}

/// Toast text for a finished cleanup.
pub fn cleanup_summary(r: &jepret_history::CleanupReport, recycle: bool, keep_favorites: bool) -> String {
    let mut s = if r.files_deleted == 0 && r.entries_removed == 0 {
        "Cleanup selesai: tidak ada yang perlu dibersihkan".to_string()
    } else {
        format!(
            "Cleanup selesai: {} file · {} {}",
            r.files_deleted,
            human_bytes(r.bytes_freed),
            if recycle { "dipindah ke Recycle Bin" } else { "dihapus" }
        )
    };
    if keep_favorites {
        s.push_str(" · favorit aman");
    }
    if !r.errors.is_empty() {
        s.push_str(&format!(" · {} error ({})", r.errors.len(), r.errors[0]));
    }
    s
}
/// One vertical slot in the virtual grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Header { run: usize, start: u64 },
    /// `count` cards starting at absolute index `start`, belonging to `run`.
    Cards { run: usize, start: u64, count: usize },
}

/// Row geometry for the virtualized, grouped grid.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GridLayout {
    pub cols: usize,
    pub rows: Vec<Row>,
    /// Top y of each row; `tops.len() == rows.len()`.
    pub tops: Vec<f32>,
    pub height: f32,
}

impl GridLayout {
    pub fn build(runs: &[DayRun], cols: usize, header_h: f32, row_h: f32) -> Self {
        let cols = cols.max(1);
        let mut rows = Vec::new();
        let mut tops = Vec::new();
        let mut y = 0.0_f32;
        for (ri, run) in runs.iter().enumerate() {
            rows.push(Row::Header { run: ri, start: run.start });
            tops.push(y);
            y += header_h;
            let mut done = 0u64;
            while done < run.count {
                let n = (run.count - done).min(cols as u64) as usize;
                rows.push(Row::Cards { run: ri, start: run.start + done, count: n });
                tops.push(y);
                y += row_h;
                done += n as u64;
            }
        }
        Self { cols, rows, tops, height: y }
    }

    fn row_height(&self, i: usize) -> f32 {
        self.tops.get(i + 1).copied().unwrap_or(self.height) - self.tops[i]
    }

    /// Rows intersecting the vertical span `[y0, y1]`.
    pub fn visible(&self, y0: f32, y1: f32) -> std::ops::Range<usize> {
        let first = self.tops.partition_point(|t| *t <= y0).saturating_sub(1);
        let last = self.tops.partition_point(|t| *t < y1);
        first.min(self.rows.len())..last.max(first).min(self.rows.len())
    }

    /// (row index, top, height) of the card row holding entry `idx`.
    pub fn row_of(&self, idx: u64) -> Option<(usize, f32, f32)> {
        let i = self.rows.partition_point(|r| match r {
            Row::Cards { start, .. } | Row::Header { start, .. } => *start <= idx,
        });
        let i = i.checked_sub(1)?;
        match self.rows.get(i)? {
            Row::Cards { start, count, .. } if idx < start + *count as u64 => {
                Some((i, self.tops[i], self.row_height(i)))
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Keyboard navigation over the grouped grid. Rows never mix days, so
/// Up/Down keep the column and clamp to the last card of the target row.
pub fn move_selection(runs: &[DayRun], cols: usize, idx: u64, dir: Dir) -> u64 {
    let cols = cols.max(1) as u64;
    let total = runs.last().map(|r| r.start + r.count).unwrap_or(0);
    if total == 0 {
        return 0;
    }
    let idx = idx.min(total - 1);
    match dir {
        Dir::Left => idx.saturating_sub(1),
        Dir::Right => (idx + 1).min(total - 1),
        Dir::Up | Dir::Down => {
            let ri = runs.partition_point(|r| r.start <= idx).saturating_sub(1);
            let run = &runs[ri];
            let rel = idx - run.start;
            let (row, col) = (rel / cols, rel % cols);
            let rows_in_run = run.count.div_ceil(cols);
            // card at (row, col) of `run`, clamped to the row's last card
            let target = |run: &DayRun, row: u64| {
                let first = row * cols;
                run.start + (first + col).min(run.count - 1).max(first)
            };
            if dir == Dir::Up {
                if row > 0 {
                    target(run, row - 1)
                } else if ri > 0 {
                    let prev = &runs[ri - 1];
                    target(prev, prev.count.div_ceil(cols) - 1)
                } else {
                    idx
                }
            } else if row + 1 < rows_in_run {
                target(run, row + 1)
            } else if ri + 1 < runs.len() {
                target(&runs[ri + 1], 0)
            } else {
                idx
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use jepret_core::Result;
    use jepret_history::CleanupReport;

    use super::*;
    use crate::source::{FakeSource, Thumb};

    /// Wraps a source and counts `list` calls.
    struct Counting {
        inner: FakeSource,
        lists: Arc<AtomicUsize>,
    }

    impl Source for Counting {
        fn list(&self, q: &Query) -> Result<Vec<Entry>> {
            self.lists.fetch_add(1, Ordering::SeqCst);
            self.inner.list(q)
        }
        fn count(&self, q: &Query) -> Result<u64> {
            self.inner.count(q)
        }
        fn thumbnail(&self, e: &Entry) -> Result<Thumb> {
            self.inner.thumbnail(e)
        }
        fn set_favorite(&self, id: i64, f: bool) -> Result<()> {
            self.inner.set_favorite(id, f)
        }
        fn delete(&self, id: i64, a: bool, b: bool) -> Result<()> {
            self.inner.delete(id, a, b)
        }
        fn cleanup(&self, now: i64) -> Result<CleanupReport> {
            self.inner.cleanup(now)
        }
        fn prune_missing(&self) -> Result<u32> {
            self.inner.prune_missing()
        }
    }

    #[test]
    fn lru_evicts_least_recently_used() {
        let mut c = LruCache::new(2);
        c.put(1, "a");
        c.put(2, "b");
        assert_eq!(c.get(&1), Some(&"a")); // 1 is now fresher than 2
        c.put(3, "c");
        assert!(c.contains(&1));
        assert!(!c.contains(&2));
        assert!(c.contains(&3));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn lru_put_existing_does_not_grow() {
        let mut c = LruCache::new(2);
        c.put(1, 1);
        c.put(1, 2);
        c.put(2, 3);
        assert_eq!(c.len(), 2);
        assert_eq!(c.get(&1), Some(&2));
    }

    #[test]
    fn paging_returns_newest_first_and_caches() {
        let lists = Arc::new(AtomicUsize::new(0));
        let src = Counting { inner: FakeSource::new(1000), lists: lists.clone() };
        let mut p = PagedList::new(3);
        p.load(&src, Query::default());
        assert_eq!(p.total(), 1000);
        assert_eq!(p.get(&src, 0).unwrap().id, 1000);
        assert_eq!(p.get(&src, 5).unwrap().id, 995);
        assert_eq!(lists.load(Ordering::SeqCst), 1, "same page = one fetch");
        let across = PAGE_SIZE as u64;
        assert_eq!(p.get(&src, across).unwrap().id, 1000 - across as i64);
        assert_eq!(lists.load(Ordering::SeqCst), 2);
        assert!(p.get(&src, 1000).is_none());
        assert_eq!(p.get(&src, 999).unwrap().id, 1);
    }

    #[test]
    fn paging_evicts_old_pages() {
        let lists = Arc::new(AtomicUsize::new(0));
        let src = Counting { inner: FakeSource::new(1000), lists: lists.clone() };
        let mut p = PagedList::new(2);
        p.load(&src, Query::default());
        for page in 0..3u64 {
            p.get(&src, page * PAGE_SIZE as u64);
        }
        assert_eq!(lists.load(Ordering::SeqCst), 3);
        p.get(&src, 0); // page 0 was evicted
        assert_eq!(lists.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn paging_with_filter_and_refresh() {
        let src = FakeSource::new(100);
        let mut p = PagedList::new(2);
        let q = Filter { favorites_only: true, ..Default::default() }.to_query().unwrap();
        p.load(&src, q);
        let fav = (0..100).filter(|i| i % 7 == 0).count() as u64;
        assert_eq!(p.total(), fav);
        let first = p.get(&src, 0).unwrap();
        assert!(first.favorite);
        src.set_favorite(first.id, false).unwrap();
        p.refresh(&src);
        assert_eq!(p.total(), fav - 1);
        assert_ne!(p.get(&src, 0).unwrap().id, first.id);
    }

    #[test]
    fn filter_defaults_to_empty_query() {
        let q = Filter::default().to_query().unwrap();
        assert!(q.search.is_none() && q.kind.is_none() && !q.favorites_only);
        assert!(q.from.is_none() && q.to.is_none());
    }

    #[test]
    fn filter_maps_fields() {
        let f = Filter {
            search: "  foo ".into(),
            kind: KindFilter::Video,
            favorites_only: true,
            from: "2026-01-15".into(),
            to: "2026-01-15".into(),
        };
        let q = f.to_query().unwrap();
        assert_eq!(q.search.as_deref(), Some("foo"));
        assert_eq!(q.kind, Some(EntryKind::Video));
        assert!(q.favorites_only);
        let span = q.to.unwrap() - q.from.unwrap();
        // one local day (23-25h to tolerate DST), end is inclusive
        assert!((23 * 3_600_000..=25 * 3_600_000).contains(&span), "span {span}");
    }

    #[test]
    fn filter_rejects_bad_dates() {
        let bad = Filter { from: "15/01/2026".into(), ..Default::default() };
        assert!(bad.to_query().is_err());
        let rev = Filter { from: "2026-02-01".into(), to: "2026-01-01".into(), ..Default::default() };
        assert!(rev.to_query().is_err());
    }

    #[test]
    fn fake_source_filters_by_search_and_kind() {
        let src = FakeSource::new(50);
        let q = Filter { search: "00050".into(), ..Default::default() }.to_query().unwrap();
        assert_eq!(src.count(&q).unwrap(), 1);
        let q = Filter { kind: KindFilter::Video, ..Default::default() }.to_query().unwrap();
        assert_eq!(src.count(&q).unwrap(), 5);
    }

    #[test]
    fn grid_math() {
        assert_eq!(grid_columns(100.0, 200.0, 8.0), 1);
        assert_eq!(grid_columns(1000.0, 200.0, 8.0), 4);
        assert_eq!(grid_columns(1040.0, 200.0, 8.0), 5);


    }

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, d).unwrap()
    }

    fn runs_of(counts: &[(u32, u64)]) -> Vec<DayRun> {
        let mut v = Vec::new();
        for (d, c) in counts {
            for _ in 0..*c {
                push_day(&mut v, day(*d));
            }
        }
        v
    }

    #[test]
    fn day_runs_group_consecutive_days() {
        let r = runs_of(&[(18, 3), (17, 2)]);
        assert_eq!(r.len(), 2);
        assert_eq!((r[1].start, r[1].count), (3, 2));
    }

    #[test]
    fn day_labels() {
        assert_eq!(day_label(day(18), day(18)), "Hari ini");
        assert_eq!(day_label(day(17), day(18)), "Kemarin");
        assert_eq!(day_label(day(3), day(18)), "3 November 2026");
    }

    #[test]
    fn formats() {
        assert_eq!(thousands(1284), "1.284");
        assert_eq!(thousands(12), "12");
        assert_eq!(thousands(1_000_000), "1.000.000");
        assert_eq!(human_bytes(3_435_973_837), "3,2 GB");
        assert_eq!(human_bytes(398_458_880), "380 MB");
        assert_eq!(source_label("active_window"), "Window");
        assert_eq!(source_label("foo_bar"), "Foo bar");
    }

    #[test]
    fn layout_rows_and_visibility() {
        let runs = runs_of(&[(18, 7), (17, 2)]);
        let l = GridLayout::build(&runs, 3, 30.0, 100.0);
        // header, 3 card rows (3+3+1), header, 1 card row
        assert_eq!(l.rows.len(), 6);
        assert_eq!(l.height, 30.0 + 300.0 + 30.0 + 100.0);
        assert_eq!(l.visible(0.0, 10.0), 0..1);
        assert_eq!(l.visible(35.0, 150.0), 1..3);
        assert_eq!(l.row_of(6).map(|r| r.0), Some(3));
        assert_eq!(l.row_of(7).map(|r| r.0), Some(5));
        assert!(l.row_of(9).is_none());
    }

    #[test]
    fn selection_moves_across_groups() {
        let runs = runs_of(&[(18, 7), (17, 2)]);
        let m = |i, d| move_selection(&runs, 3, i, d);
        assert_eq!(m(0, Dir::Left), 0);
        assert_eq!(m(0, Dir::Right), 1);
        assert_eq!(m(1, Dir::Down), 4);
        assert_eq!(m(5, Dir::Down), 6); // clamps to last card of short row
        assert_eq!(m(6, Dir::Down), 7); // next group, first row, col 0
        assert_eq!(m(8, Dir::Up), 6); // col 2 -> clamp to last card of last row
        assert_eq!(m(7, Dir::Up), 6);
        assert_eq!(m(8, Dir::Right), 8);
        assert_eq!(m(1, Dir::Up), 1);
    }

    #[test]
    fn collection_and_date_mapping() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        let mut f = Filter::default();
        assert_eq!(f.collection(), Collection::All);
        for c in [Collection::Favorites, Collection::Images, Collection::Videos, Collection::All] {
            f.set_collection(c);
            assert_eq!(f.collection(), c);
        }
        f.set_collection(Collection::Favorites);
        assert!(f.favorites_only && f.kind == KindFilter::All);
        for p in [DatePreset::Today, DatePreset::Yesterday, DatePreset::Last7] {
            f.set_date_preset(Some(p), today);
            assert_eq!(f.date_preset(today), Some(p));
            assert!(!f.has_custom_date(today));
            assert!(f.to_query().is_ok());
        }
        assert_eq!(DatePreset::Last7.range(today), ("2026-11-12".into(), String::new()));
        assert_eq!(DatePreset::Yesterday.range(today).0, "2026-11-17");
        f.from = "2026-01-01".into();
        assert!(f.has_custom_date(today));
        f.set_date_preset(None, today);
        assert!(f.from.is_empty() && f.to.is_empty());
    }

    #[test]
    fn counts_bucket_by_kind_and_age() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        let noon = |d: NaiveDate| {
            Local.from_local_datetime(&d.and_hms_opt(12, 0, 0).unwrap()).earliest().unwrap().timestamp_millis()
        };
        let mk = |days: i64, kind, fav, missing| Entry {
            id: 1,
            path: "a.png".into(),
            kind,
            created_at: noon(today - chrono::Duration::days(days)),
            width: 1,
            height: 1,
            bytes: 10,
            source: String::new(),
            favorite: fav,
            missing,
        };
        let mut c = Counts::default();
        c.add(&mk(0, EntryKind::Image, true, false), today);
        c.add(&mk(1, EntryKind::Video, false, true), today);
        c.add(&mk(6, EntryKind::Image, false, false), today);
        c.add(&mk(30, EntryKind::Image, false, false), today);
        assert_eq!((c.all, c.images, c.videos, c.favorites), (4, 3, 1, 1));
        assert_eq!((c.today, c.yesterday, c.last7, c.missing, c.bytes), (1, 1, 3, 1, 40));
    }

    #[test]
    fn fit_and_card_width() {
        assert_eq!(fit_inside(200.0, 100.0, 400.0, 100.0), (200.0, 50.0));
        assert_eq!(fit_inside(200.0, 100.0, 100.0, 400.0), (25.0, 100.0));
        assert_eq!(fit_inside(200.0, 100.0, 0.0, 0.0), (200.0, 100.0));
        assert_eq!(card_width(454.0, 2, 14.0), 220.0);
        assert_eq!(card_width(50.0, 1, 14.0), 120.0);
        let d = NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
        assert_eq!(short_date(d), "8 Nov 2026");
    }

    #[test]
    fn cleanup_text() {
        let r = jepret_history::CleanupReport {
            files_deleted: 42,
            bytes_freed: 398_458_880,
            entries_removed: 42,
            errors: vec![],
        };
        assert_eq!(
            cleanup_summary(&r, true, true),
            "Cleanup selesai: 42 file · 380 MB dipindah ke Recycle Bin · favorit aman"
        );
        let none = jepret_history::CleanupReport::default();
        assert!(cleanup_summary(&none, false, false).contains("tidak ada"));
    }}
