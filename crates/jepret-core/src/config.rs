//! `config.toml` schema. Every section has defaults so a partial file works.

use std::{collections::BTreeMap, path::Path, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{action::Action, capture::CaptureKind};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub capture: CaptureConfig,
    pub hotkeys: Vec<HotkeyBinding>,
    /// External apps usable via `{ open_with = "<key>" }`.
    pub apps: BTreeMap<String, ExternalApp>,
    pub history: HistoryConfig,
    pub recording: RecordingConfig,
    pub scroll: ScrollConfig,
    pub editor: EditorConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    /// Overrides of [`crate::shortcuts::EDITOR_SHORTCUTS`]: id → combos.
    /// Present = replaces defaults; empty list = disabled. Absent = default.
    pub shortcuts: BTreeMap<String, Vec<String>>,
    /// Palette for `style.color_1`..`style.color_9`, `#rrggbb` or `#rrggbbaa`.
    pub palette: Vec<String>,
    /// Tool active when the editor opens (shortcut id, e.g. "tool.arrow").
    pub default_tool: String,
    /// Show shortcut hints in tooltips and next to menu items.
    pub show_shortcut_hints: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            shortcuts: BTreeMap::new(),
            palette: [
                "#e53935", "#fb8c00", "#fdd835", "#43a047", "#1e88e5", "#8e24aa", "#000000",
                "#ffffff", "#757575",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            default_tool: "tool.arrow".into(),
            show_shortcut_hints: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub output_dir: PathBuf,
    /// Without extension; supports template placeholders.
    pub filename_pattern: String,
    pub image_format: ImageFormat,
    pub jpeg_quality: u8,
    /// Group output into `yyyy-mm` subfolders.
    pub monthly_subfolders: bool,
    pub play_sound: bool,
    pub start_with_windows: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    Png,
    Jpeg,
}

impl ImageFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    pub show_cursor: bool,
    /// Tone-map HDR monitors to SDR.
    pub hdr_tonemap: bool,
    /// Override SDR white level (nits). `None` = use Windows setting.
    pub sdr_white_nits: Option<f32>,
    pub overlay_magnifier: bool,
    pub overlay_snap_to_windows: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HotkeyBinding {
    /// e.g. `"ctrl+shift+4"`, `"printscreen"`, `"alt+printscreen"`.
    pub keys: String,
    pub capture: CaptureKind,
    #[serde(default)]
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalApp {
    pub path: PathBuf,
    #[serde(default = "default_app_args")]
    pub args: Vec<String>,
    /// Display name in UI; defaults to the table key.
    #[serde(default)]
    pub name: Option<String>,
}

fn default_app_args() -> Vec<String> {
    vec!["{file}".into()]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    pub enabled: bool,
    pub cleanup: CleanupRules,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CleanupRules {
    pub enabled: bool,
    pub max_age_days: Option<u32>,
    pub max_total_mb: Option<u64>,
    pub keep_favorites: bool,
    pub use_recycle_bin: bool,
    pub run_on_startup: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingConfig {
    pub output_dir: Option<PathBuf>,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub system_audio: bool,
    pub microphone: bool,
    /// Device id from `jepret_recorder::list_microphones`; `None` = default mic.
    pub mic_device: Option<String>,
    pub show_cursor: bool,
    /// Actions run on the finished video file (file-based actions only).
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrollConfig {
    pub max_frames: u32,
    pub scroll_delay_ms: u32,
    /// Mouse wheel notches per step.
    pub wheel_notches: u32,
}

// ---------- defaults ----------

impl Default for Config {
    fn default() -> Self {
        use Action::*;
        use CaptureKind::*;
        let hk = |keys: &str, capture, actions: Vec<Action>| HotkeyBinding {
            keys: keys.into(),
            capture,
            actions,
        };
        Self {
            general: General::default(),
            capture: CaptureConfig::default(),
            // Everything lives on the PrtSc key so it is easy to remember:
            // plain = the most used action, modifiers = variants.
            hotkeys: vec![
                hk("printscreen", Region, vec![Save, Clipboard, OpenEditor]),
                hk("ctrl+printscreen", Fullscreen, vec![Save, Clipboard, Notify]),
                hk("alt+printscreen", ActiveWindow, vec![Save, Clipboard, Notify]),
                hk("shift+printscreen", LastRegion, vec![Save, Clipboard, Notify]),
                hk("ctrl+shift+printscreen", Scroll, vec![Save, Clipboard, OpenEditor]),
                hk("ctrl+alt+printscreen", RecordRegion, vec![]),
                hk("alt+shift+printscreen", RecordMonitor, vec![]),
            ],
            apps: BTreeMap::new(),
            history: HistoryConfig::default(),
            recording: RecordingConfig::default(),
            scroll: ScrollConfig::default(),
            editor: EditorConfig::default(),
        }
    }
}

impl Default for General {
    fn default() -> Self {
        Self {
            output_dir: crate::paths::default_output_dir(),
            filename_pattern: "jepret_{datetime}".into(),
            image_format: ImageFormat::Png,
            jpeg_quality: 90,
            monthly_subfolders: true,
            play_sound: false,
            start_with_windows: false,
        }
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            show_cursor: false,
            hdr_tonemap: true,
            sdr_white_nits: None,
            overlay_magnifier: true,
            overlay_snap_to_windows: true,
        }
    }
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self { enabled: true, cleanup: CleanupRules::default() }
    }
}

impl Default for CleanupRules {
    fn default() -> Self {
        Self {
            enabled: false,
            max_age_days: Some(30),
            max_total_mb: None,
            keep_favorites: true,
            use_recycle_bin: true,
            run_on_startup: true,
        }
    }
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            output_dir: None,
            fps: 30,
            bitrate_kbps: 8000,
            system_audio: true,
            microphone: false,
            mic_device: None,
            show_cursor: true,
            actions: vec![Action::Notify, Action::CopyPath],
        }
    }
}

impl Default for ScrollConfig {
    fn default() -> Self {
        Self { max_frames: 100, scroll_delay_ms: 250, wheel_notches: 3 }
    }
}

// ---------- io ----------

impl Config {
    /// Missing file → defaults. Parse error → `Err` (don't silently drop the user's edits).
    pub fn load(path: &Path) -> crate::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(toml::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> crate::Result<()> {
        if let Some(dir) = path.parent() {
            crate::paths::ensure_dir(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    /// Output folder for a capture made now (applies `monthly_subfolders`).
    pub fn image_output_dir(&self, year: i32, month: u32) -> PathBuf {
        if self.general.monthly_subfolders {
            self.general.output_dir.join(format!("{year:04}-{month:02}"))
        } else {
            self.general.output_dir.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::CommandSpec;

    #[test]
    fn parses_prd_example() {
        let src = r#"
            [general]
            output_dir = "C:/Users/me/Pictures/Jepret"
            filename_pattern = "jepret_{datetime}"
            image_format = "png"

            [[hotkeys]]
            keys = "ctrl+shift+5"
            capture = "region"
            actions = ["save", "clipboard", { open_with = "photocraft" },
                       { run_command = { path = "C:/x.exe", args = ["{file}", "-q"] } }]

            [apps.photocraft]
            path = "C:/Program Files/Photocraft/photocraft.exe"

            [history.cleanup]
            max_age_days = 7
            max_total_mb = 5000
        "#;
        let c: Config = toml::from_str(src).unwrap();
        assert_eq!(c.hotkeys.len(), 1);
        assert_eq!(c.hotkeys[0].capture, CaptureKind::Region);
        assert_eq!(
            c.hotkeys[0].actions,
            vec![
                Action::Save,
                Action::Clipboard,
                Action::OpenWith("photocraft".into()),
                Action::RunCommand(CommandSpec {
                    path: "C:/x.exe".into(),
                    args: vec!["{file}".into(), "-q".into()],
                    hidden: false,
                }),
            ]
        );
        assert_eq!(c.apps["photocraft"].args, vec!["{file}".to_string()]);
        assert_eq!(c.history.cleanup.max_age_days, Some(7));
        assert!(c.history.cleanup.keep_favorites);
    }

    #[test]
    fn default_hotkeys_parse_and_are_unique() {
        let c = Config::default();
        let mut combos: Vec<String> = c
            .hotkeys
            .iter()
            .map(|h| crate::keys::KeyCombo::parse(&h.keys).unwrap().to_string())
            .collect();
        let n = combos.len();
        combos.sort();
        combos.dedup();
        assert_eq!(combos.len(), n);
    }

    #[test]
    fn editor_shortcut_overrides_parse() {
        let c: Config = toml::from_str(
            r#"
            [editor.shortcuts]
            "tool.arrow" = ["a", "ctrl+shift+a"]
            "tool.pixelate" = []
            "#,
        )
        .unwrap();
        assert_eq!(c.editor.shortcuts["tool.arrow"].len(), 2);
        assert!(c.editor.shortcuts["tool.pixelate"].is_empty());
        assert_eq!(c.editor.palette.len(), 9);
    }

    #[test]
    fn default_roundtrips() {
        let c = Config::default();
        let s = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(c, back);
    }
}
