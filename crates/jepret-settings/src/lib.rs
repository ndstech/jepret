//! Settings UI. Owner: **T11**. See PRD F12.
//!
//! Runs in its own process (`jepret.exe --settings`). Edits every field of
//! `jepret_core::Config` and writes it with `Config::save`. The daemon
//! picks up changes (T12 decides: file watcher or reload message).
//!
//! The "Open with" app picker needs `jepret_workflow::open_with_candidates`;
//! add that dependency when T04 lands (stub already exists, so it compiles).

use std::path::Path;

use jepret_core::Result;

/// Open the settings window for the config at `config_path`; blocks until closed.
pub fn run_settings(_config_path: &Path) -> Result<()> {
    anyhow::bail!("TODO(T11): run_settings")
}
