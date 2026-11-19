use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::effects::EffectSpec;

/// One step of an after-capture workflow.
///
/// TOML form (serde externally tagged):
/// ```toml
/// actions = ["save", "clipboard", { open_with = "photocraft" },
///            { run_command = { path = "C:/tools/x.exe", args = ["{file}"] } }]
/// ```
///
/// Backlog actions (telegram, share) will be added as new variants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Save to `general.output_dir` using `general.filename_pattern`.
    Save,
    /// Copy the image to the clipboard.
    Clipboard,
    /// Copy the saved file path to the clipboard.
    CopyPath,
    /// Open the Jepret editor (separate process).
    OpenEditor,
    /// Open Explorer with the file selected.
    OpenFolder,
    /// Toast notification with thumbnail.
    Notify,
    /// Open with an app defined under `[apps.<name>]`.
    OpenWith(String),
    /// Run an arbitrary program.
    RunCommand(CommandSpec),
    /// Recognise text (Windows OCR) and copy it to the clipboard.
    Ocr,
    /// Show the capture pinned on screen (always-on-top window).
    Pin,
    /// Apply image effects; later actions see the processed image.
    Effects(EffectSpec),
}

impl Action {
    /// Whether this action needs the capture to exist as a file on disk.
    pub fn needs_file(&self) -> bool {
        !matches!(self, Action::Save | Action::Clipboard | Action::Notify | Action::Ocr | Action::Effects(_))
    }

    /// Short label for logs / UI.
    pub fn label(&self) -> String {
        match self {
            Action::Save => "save".into(),
            Action::Clipboard => "clipboard".into(),
            Action::CopyPath => "copy_path".into(),
            Action::OpenEditor => "open_editor".into(),
            Action::OpenFolder => "open_folder".into(),
            Action::Notify => "notify".into(),
            Action::OpenWith(app) => format!("open_with:{app}"),
            Action::RunCommand(c) => format!("run_command:{}", c.path.display()),
            Action::Ocr => "ocr".into(),
            Action::Pin => "pin".into(),
            Action::Effects(_) => "effects".into(),
        }
    }
}

/// External program invocation. `args` support template placeholders
/// (see [`crate::template`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub path: PathBuf,
    #[serde(default = "default_args")]
    pub args: Vec<String>,
    /// Hide the console window of the spawned process.
    #[serde(default)]
    pub hidden: bool,
}

fn default_args() -> Vec<String> {
    vec!["{file}".into()]
}
