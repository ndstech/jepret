//! History gallery. Owner: **T10**. See PRD F9.
//!
//! Runs in its own process (`jepret.exe --gallery`). Virtualized grid:
//! only query/decode thumbnails for visible rows; decode on a worker thread.
//! Until T05 lands, develop against a fake in-memory list behind a small
//! trait so the UI is not blocked.

use jepret_core::{Config, Result};

/// Open the gallery window; blocks until closed.
/// "Open in editor" spawns `std::env::current_exe() --editor <file>`.
pub fn run_gallery(_config: &Config) -> Result<()> {
    anyhow::bail!("TODO(T10): run_gallery")
}
