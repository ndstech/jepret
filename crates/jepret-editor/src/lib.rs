//! Image editor. Owner: **T06**. See PRD F8.
//!
//! Runs in its own process (`jepret.exe --editor <file>`).
//! - `doc.rs`: shape model + undo/redo (pure, tested)
//! - `render.rs`: flatten shapes onto an `Image` (pure, tested)
//! - `io.rs`: decode / encode / clipboard
//! - `ui.rs`: egui front-end

#![allow(unknown_lints, clippy::chunks_exact_to_as_chunks)]

pub mod doc;
pub mod io;
pub mod render;
pub mod input;
pub mod session;
mod view;
mod ui;

use std::path::Path;

use jepret_core::{Config, Result};

pub use doc::{Document, Pt, Shape, ShapeKind, Tool};
pub use session::{Effect, Session};
pub use io::{copy_to_clipboard, load_image, save_image};
pub use render::flatten;

/// Open the editor window for `path`; blocks until the window closes.
pub fn run_editor(path: &Path, config: &Config) -> Result<()> {
    anyhow::ensure!(path.is_file(), "file not found: {}", path.display());
    ui::run(path.to_path_buf(), config.clone())
}
