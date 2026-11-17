//! Settings UI. Owner: **T11**. See PRD F12.
//!
//! Runs in its own process (`jepret.exe --settings`). Edits every field of
//! `jepret_core::Config` and writes it with `Config::save`. The daemon
//! picks up changes (T12 decides: file watcher or reload message).
//!
//! Saved hotkey strings are normalized to `jepret_core::KeyCombo`'s canonical
//! form (e.g. `ctrl+shift+printscreen`); T12 should parse with `KeyCombo`.
//!
//! The "Open with" picker uses `jepret_workflow::open_with_candidates`; if it
//! fails the list is simply empty and the user can pick an `.exe` manually.

use std::path::Path;

use jepret_core::{Config, Result};

pub mod hotkey;
mod keyhook;
pub mod model;
mod ui;

/// Open the settings window for the config at `config_path`; blocks until closed.
pub fn run_settings(config_path: &Path) -> Result<()> {
    // A broken file must not be silently overwritten: surface the parse error.
    let cfg = Config::load(config_path)?;
    let app = ui::SettingsApp::new(config_path.to_path_buf(), cfg);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Jepret - Pengaturan")
            .with_inner_size([820.0, 600.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native("Jepret Settings", options, Box::new(move |_cc| Ok(Box::new(app))))
        .map_err(|e| anyhow::anyhow!("settings window failed: {e}"))
}
