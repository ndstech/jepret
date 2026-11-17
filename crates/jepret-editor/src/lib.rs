//! Image editor. Owner: **T06**. See PRD F8.
//!
//! Runs in its own process (`jepret.exe --editor <file>`).
//! Suggested layout: `doc.rs` (shapes model + undo stack, pure & tested),
//! `render.rs` (flatten shapes onto Image for save/copy — also pure),
//! `ui.rs` (egui). Keep `doc`/`render` free of egui so they are testable.

use std::path::Path;

use jepret_core::{Config, Result};

/// Open the editor window for `path`; blocks until the window closes.
pub fn run_editor(_path: &Path, _config: &Config) -> Result<()> {
    anyhow::bail!("TODO(T06): run_editor")
}
