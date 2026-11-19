//! `jepret --pin <file>`: show an image in a borderless always-on-top
//! window (ShareX "Pin to screen" style). One pin = one process.
//!
//! * [`logic`] — pure math (zoom steps, fit, opacity, cursor anchoring), tested.
//! * [`sys`] — Win32 cursor / work-area / DPI queries.
//! * `app` — the egui window.

mod app;
pub mod logic;
mod sys;

use std::path::Path;

use jepret_core::Result;

/// Run the pin window for `file` until closed (blocks).
pub fn run_pin(file: &Path, config_path: &Path) -> Result<()> {
    app::run(file, config_path)
}
