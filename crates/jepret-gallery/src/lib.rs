//! History gallery. Owner: **T10**. See PRD F9.
//!
//! Runs in its own process (`jepret.exe --gallery`). Virtualized grid:
//! only query/decode thumbnails for visible rows; decode on worker threads.
//! The UI talks to a small [`Source`] trait so it can run against
//! [`FakeSource`] (synthetic data) or [`HistorySource`] (real SQLite index).

mod app;
mod model;
mod source;

use std::sync::Arc;

use jepret_core::{paths, Config, Result};

pub use model::{Filter, KindFilter, LruCache, PagedList};
pub use source::{FakeSource, HistorySource, Source, Thumb};

/// Open the gallery window; blocks until closed.
/// "Open in editor" spawns `std::env::current_exe() --editor <file>`.
///
/// Uses the real history DB. If it cannot be opened, shows an error in the
/// window, unless env `JEPRET_FAKE_GALLERY=1` is set (fake data fallback).
pub fn run_gallery(config: &Config) -> Result<()> {
    let thumbs = paths::thumbs_dir();
    let opened = paths::ensure_dir(&thumbs)
        .and_then(|_| HistorySource::open(&paths::history_db(), &thumbs, config.history.cleanup.clone()));
    let backend: std::result::Result<Arc<dyn Source>, String> = match opened {
        Ok(s) => Ok(Arc::new(s)),
        Err(e) => {
            if std::env::var("JEPRET_FAKE_GALLERY").as_deref() == Ok("1") {
                tracing::warn!("history unavailable ({e:#}); using fake gallery data");
                Ok(Arc::new(FakeSource::new(10_000)))
            } else {
                Err(format!("Tidak bisa membuka riwayat: {e:#}"))
            }
        }
    };
    app::run(backend, config.history.cleanup.use_recycle_bin)
}

/// Open the gallery over an arbitrary [`Source`] (used by `examples/fake.rs`).
pub fn run_gallery_with(source: Arc<dyn Source>, config: &Config) -> Result<()> {
    app::run(Ok(source), config.history.cleanup.use_recycle_bin)
}
