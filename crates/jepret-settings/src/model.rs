//! Non-UI logic: validation, normalization, and small UI-state <-> Config conversions.

use std::{collections::BTreeMap, path::Path, path::PathBuf};

use jepret_core::{config::ExternalApp, Action, Config};

use crate::hotkey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Blocks saving.
    Error,
    /// Shown, but saving is allowed.
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub severity: Severity,
    pub message: String,
}

impl Issue {
    fn err(m: impl Into<String>) -> Self {
        Self { severity: Severity::Error, message: m.into() }
    }
    fn warn(m: impl Into<String>) -> Self {
        Self { severity: Severity::Warning, message: m.into() }
    }
}

pub fn has_errors(issues: &[Issue]) -> bool {
    issues.iter().any(|i| i.severity == Severity::Error)
}

/// Validate a whole config. Pure except for `Path::exists` checks (warnings only).
pub fn validate(cfg: &Config) -> Vec<Issue> {
    let mut out = Vec::new();

    // Hotkeys: syntax, then duplicates.
    for (i, hk) in cfg.hotkeys.iter().enumerate() {
        if let Err(e) = hotkey::normalize(&hk.keys) {
            out.push(Issue::err(format!("Hotkey #{} \"{}\": {e}", i + 1, hk.keys)));
        }
    }
    let keys: Vec<&str> = cfg.hotkeys.iter().map(|h| h.keys.as_str()).collect();
    for (a, b) in hotkey::find_duplicates(&keys) {
        out.push(Issue::err(format!(
            "Hotkey duplikat: #{} dan #{} sama-sama \"{}\"",
            a + 1,
            b + 1,
            hotkey::normalize(keys[a]).unwrap_or_default()
        )));
    }

    // Actions referencing apps / commands.
    let mut check_actions = |ctx: &str, actions: &[Action]| {
        for a in actions {
            match a {
                Action::OpenWith(k) if !cfg.apps.contains_key(k) => {
                    out.push(Issue::err(format!("{ctx}: aplikasi \"{k}\" belum didefinisikan di tab Aplikasi")));
                }
                Action::RunCommand(c) => {
                    if c.path.as_os_str().is_empty() {
                        out.push(Issue::err(format!("{ctx}: run_command tanpa path")));
                    } else if !c.path.exists() {
                        out.push(Issue::warn(format!("{ctx}: program tidak ditemukan: {}", c.path.display())));
                    }
                }
                _ => {}
            }
        }
    };
    for (i, hk) in cfg.hotkeys.iter().enumerate() {
        check_actions(&format!("Hotkey #{} ({})", i + 1, hk.keys), &hk.actions);
    }
    check_actions("Recording", &cfg.recording.actions);

    for (k, app) in &cfg.apps {
        if app.path.as_os_str().is_empty() {
            out.push(Issue::err(format!("Aplikasi \"{k}\": path kosong")));
        } else if !app.path.exists() {
            out.push(Issue::warn(format!("Aplikasi \"{k}\": file tidak ada: {}", app.path.display())));
        }
    }

    if cfg.general.output_dir.as_os_str().is_empty() {
        out.push(Issue::err("Folder output kosong"));
    }
    if cfg.general.filename_pattern.trim().is_empty() {
        out.push(Issue::err("Pola nama file kosong"));
    }
    if !(1..=100).contains(&cfg.general.jpeg_quality) {
        out.push(Issue::err("Kualitas JPEG harus 1-100"));
    }
    if cfg.recording.fps == 0 {
        out.push(Issue::err("FPS recording harus > 0"));
    }
    if cfg.recording.bitrate_kbps == 0 {
        out.push(Issue::err("Bitrate recording harus > 0"));
    }
    if cfg.scroll.max_frames == 0 {
        out.push(Issue::err("Scroll: max frame harus > 0"));
    }
    if cfg.scroll.wheel_notches == 0 {
        out.push(Issue::err("Scroll: wheel notches harus > 0"));
    }
    if let Some(n) = cfg.capture.sdr_white_nits {
        if !n.is_finite() || n <= 0.0 {
            out.push(Issue::err("SDR white level harus > 0 nits"));
        }
    }
    out
}

/// Rewrite hotkey strings into canonical form. Invalid ones are left untouched
/// (validation reports them). Call before saving.
pub fn normalize_config(cfg: &mut Config) {
    for hk in &mut cfg.hotkeys {
        if let Ok(n) = hotkey::normalize(&hk.keys) {
            hk.keys = n;
        }
    }
    if let Some(m) = &cfg.recording.mic_device {
        if m.trim().is_empty() {
            cfg.recording.mic_device = None;
        }
    }
}

// ---- command-line args <-> single editable line ----

/// Split `a "b c" d` into `["a", "b c", "d"]`. `\"` inside quotes is a literal quote.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut has = false;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '"' => {
                in_q = !in_q;
                has = true;
            }
            '\\' if in_q && it.peek() == Some(&'"') => {
                cur.push('"');
                it.next();
            }
            c if c.is_whitespace() && !in_q => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

/// Inverse of [`split_args`].
pub fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.is_empty() || a.chars().any(|c| c.is_whitespace() || c == '"') {
                format!("\"{}\"", a.replace('"', "\\\""))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---- external apps ----

/// Table key from a file stem: lowercase `[a-z0-9_-]`, others become `_`.
pub fn app_key_from_path(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("app");
    let k: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '_' })
        .collect();
    if k.is_empty() { "app".into() } else { k }
}

/// `base`, or `base_2`, `base_3`... not present in `apps`.
pub fn unique_key(apps: &BTreeMap<String, ExternalApp>, base: &str) -> String {
    if !apps.contains_key(base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base}_{n}")).find(|k| !apps.contains_key(k)).unwrap_or_default()
}

/// Add an app for `path` (display name optional). Returns the key used;
/// returns the existing key if the same path is already registered.
pub fn add_app(cfg: &mut Config, path: PathBuf, name: Option<String>) -> String {
    if let Some((k, _)) = cfg.apps.iter().find(|(_, a)| a.path == path) {
        return k.clone();
    }
    let key = unique_key(&cfg.apps, &app_key_from_path(&path));
    cfg.apps.insert(key.clone(), ExternalApp { path, args: vec!["{file}".into()], name });
    key
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use jepret_core::{config::HotkeyBinding, CaptureKind, CommandSpec};

    fn hk(keys: &str) -> HotkeyBinding {
        HotkeyBinding { keys: keys.into(), capture: CaptureKind::Region, actions: vec![] }
    }

    #[test]
    fn default_config_is_valid() {
        let issues = validate(&Config::default());
        assert!(!has_errors(&issues), "{issues:?}");
    }

    #[test]
    fn duplicate_and_invalid_hotkeys_are_errors() {
        let mut c = Config::default();
        c.hotkeys = vec![hk("ctrl+shift+4"), hk("Shift+Ctrl+4"), hk("nonsense")];
        let issues = validate(&c);
        assert!(has_errors(&issues));
        assert!(issues.iter().any(|i| i.message.contains("duplikat")));
        assert!(issues.iter().any(|i| i.message.contains("nonsense")));
    }

    #[test]
    fn missing_app_path_is_warning_unknown_ref_is_error() {
        let mut c = Config::default();
        c.apps.insert(
            "x".into(),
            ExternalApp { path: "Z:/definitely/not/here.exe".into(), args: vec![], name: None },
        );
        let issues = validate(&c);
        assert!(!has_errors(&issues));
        assert!(issues.iter().any(|i| i.severity == Severity::Warning));

        c.hotkeys[0].actions.push(Action::OpenWith("ghost".into()));
        assert!(has_errors(&validate(&c)));
    }

    #[test]
    fn run_command_empty_path_is_error() {
        let mut c = Config::default();
        c.recording.actions.push(Action::RunCommand(CommandSpec {
            path: PathBuf::new(),
            args: vec![],
            hidden: false,
        }));
        assert!(has_errors(&validate(&c)));
    }

    #[test]
    fn numeric_bounds() {
        let mut c = Config::default();
        c.general.jpeg_quality = 0;
        c.recording.fps = 0;
        c.capture.sdr_white_nits = Some(-1.0);
        assert_eq!(validate(&c).iter().filter(|i| i.severity == Severity::Error).count(), 3);
    }

    #[test]
    fn normalize_config_canonicalizes_hotkeys_and_load_is_identical() {
        let mut c = Config::default();
        c.hotkeys = vec![hk("Shift+CTRL+4"), hk("junk")];
        c.recording.mic_device = Some("  ".into());
        normalize_config(&mut c);
        assert_eq!(c.hotkeys[0].keys, "ctrl+shift+4");
        assert_eq!(c.hotkeys[1].keys, "junk");
        assert_eq!(c.recording.mic_device, None);
    }

    #[test]
    fn saved_config_reloads_identical() {
        let dir = std::env::temp_dir().join(format!("jepret-settings-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        let mut c = Config::default();
        c.general.jpeg_quality = 77;
        c.general.play_sound = true;
        c.capture.sdr_white_nits = Some(203.0);
        c.recording.output_dir = Some("D:/vids".into());
        c.recording.mic_device = Some("mic-1".into());
        c.history.cleanup.max_total_mb = Some(1234);
        c.hotkeys.push(HotkeyBinding {
            keys: "ctrl+alt+f5".into(),
            capture: CaptureKind::RecordMonitor,
            actions: vec![
                Action::Save,
                Action::OpenWith("x".into()),
                Action::RunCommand(CommandSpec { path: "C:/a b/t.exe".into(), args: vec!["{file}".into()], hidden: true }),
            ],
        });
        add_app(&mut c, "C:/Tools/Paint.NET/paint.exe".into(), Some("Paint".into()));
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), c);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn args_roundtrip() {
        assert_eq!(split_args(r#"{file} -q "a b" "" x"#), vec!["{file}", "-q", "a b", "", "x"]);
        assert_eq!(split_args("   "), Vec::<String>::new());
        for v in [
            vec!["{file}".to_string()],
            vec!["a b".into(), "".into(), r#"q"uote"#.into(), "--x=1".into()],
        ] {
            assert_eq!(split_args(&join_args(&v)), v);
        }
    }

    #[test]
    fn app_keys_unique() {
        let mut c = Config::default();
        let a = add_app(&mut c, "C:/x/My App.exe".into(), None);
        assert_eq!(a, "my_app");
        let b = add_app(&mut c, "D:/y/My App.exe".into(), None);
        assert_eq!(b, "my_app_2");
        // same path again -> same key, no duplicate
        assert_eq!(add_app(&mut c, "C:/x/My App.exe".into(), None), "my_app");
        assert_eq!(c.apps.len(), 2);
    }
}
