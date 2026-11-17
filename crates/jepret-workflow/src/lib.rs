//! After-capture workflow runner. Owner: **T04**. See PRD F7.
//!
//! Rules:
//! * Actions run in order; a failing action is recorded in
//!   [`WorkflowOutcome::errors`] and the rest still run.
//! * If any action [`Action::needs_file`] and no file exists yet, the image
//!   is first written to `paths::temp_dir()`.
//! * Does **not** touch history — the caller (app) inserts into history
//!   using `WorkflowOutcome::saved_path`.

use std::path::{Path, PathBuf};

use jepret_core::{Action, CaptureResult, Config, Result};

pub struct WorkflowContext<'a> {
    pub config: &'a Config,
    /// Path of `jepret.exe`, used to spawn `--editor`.
    pub exe_path: PathBuf,
}

#[derive(Debug, Default)]
pub struct WorkflowOutcome {
    /// Final file on disk (output dir if `save` ran, else temp), if any.
    pub saved_path: Option<PathBuf>,
    /// True if the file is in the real output dir (should go to history).
    pub saved_permanently: bool,
    /// (action label, error message)
    pub errors: Vec<(String, String)>,
}

/// Run `actions` for an image capture.
pub fn run(
    _ctx: &WorkflowContext<'_>,
    _capture: &CaptureResult,
    _actions: &[Action],
) -> Result<WorkflowOutcome> {
    anyhow::bail!("TODO(T04): run")
}

/// Run file-based actions for an existing file (e.g. a finished recording).
/// `save` / `clipboard` (image) are skipped for non-images.
pub fn run_for_file(
    _ctx: &WorkflowContext<'_>,
    _path: &Path,
    _actions: &[Action],
) -> Result<WorkflowOutcome> {
    anyhow::bail!("TODO(T04): run_for_file")
}

/// Build a unique output path for a new capture from config
/// (`output_dir`, `monthly_subfolders`, `filename_pattern`, extension).
/// Appends `_1`, `_2`… if the file exists. Does not create the file.
pub fn output_path(_config: &Config, _ext: &str) -> Result<PathBuf> {
    anyhow::bail!("TODO(T04): output_path")
}

/// Encode and write `image` in the configured format.
pub fn write_image(_image: &jepret_core::Image, _path: &Path, _config: &Config) -> Result<()> {
    anyhow::bail!("TODO(T04): write_image")
}

/// Put an image on the clipboard (PNG + CF_DIBV5 so alpha survives where supported).
pub fn copy_image_to_clipboard(_image: &jepret_core::Image) -> Result<()> {
    anyhow::bail!("TODO(T04): copy_image_to_clipboard")
}

/// Apps registered for "Open with" on `.png` (for the settings UI picker).
#[derive(Debug, Clone)]
pub struct OpenWithCandidate {
    pub name: String,
    pub path: PathBuf,
}

pub fn open_with_candidates() -> Result<Vec<OpenWithCandidate>> {
    anyhow::bail!("TODO(T04): open_with_candidates")
}
