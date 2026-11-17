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

pub fn grid_rows(total: u64, cols: usize) -> usize {
    (total as usize).div_ceil(cols.max(1))
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
        assert_eq!(grid_rows(10_000, 5), 2000);
        assert_eq!(grid_rows(10, 4), 3);
        assert_eq!(grid_rows(0, 4), 0);
    }
}
