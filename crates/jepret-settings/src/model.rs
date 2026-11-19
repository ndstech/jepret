//! Non-UI logic: validation, normalization, and small UI-state <-> Config conversions.

use std::{collections::BTreeMap, path::Path, path::PathBuf};

use jepret_core::{
    config::ExternalApp,
    shortcuts::{self, EDITOR_SHORTCUTS},
    Action, Config, KeyCombo,
};

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
    for (i, c) in cfg.editor.palette.iter().enumerate() {
        if !is_hex_color(c) {
            out.push(Issue::err(format!("Palet editor #{}: \"{c}\" bukan #rrggbb / #rrggbbaa", i + 1)));
        }
    }
    if cfg.editor.palette.len() != 9 {
        out.push(Issue::warn("Palet editor sebaiknya berisi tepat 9 warna"));
    }
    if !EDITOR_SHORTCUTS.iter().any(|s| s.id == cfg.editor.default_tool && s.id.starts_with("tool.")) {
        out.push(Issue::err(format!("Tool awal editor tidak dikenal: {}", cfg.editor.default_tool)));
    }
    let (_, warns) = shortcuts::resolve_editor_keymap(&cfg.editor.shortcuts);
    for w in warns {
        out.push(Issue::warn(format!("Shortcut editor: {w}")));
    }
    for (combo, ids) in editor_conflicts(cfg) {
        out.push(Issue::warn(format!("Shortcut editor bentrok: {} dipakai oleh {}", combo.pretty(), ids.join(", "))));
    }
    if let Some(n) = cfg.capture.sdr_white_nits {
        if !n.is_finite() || n <= 0.0 {
            out.push(Issue::err("SDR white level harus > 0 nits"));
        }
    }
    out
}

// ---- editor shortcuts (overrides stored only when different from default) ----

/// Canonical default combos of an editor shortcut id.
pub fn editor_defaults(id: &str) -> Vec<String> {
    EDITOR_SHORTCUTS
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.defaults.iter().filter_map(|d| KeyCombo::parse(d).ok()).map(|c| c.to_string()).collect())
        .unwrap_or_default()
}

/// Effective canonical combos: override if present, else defaults.
pub fn editor_effective(cfg: &Config, id: &str) -> Vec<String> {
    match cfg.editor.shortcuts.get(id) {
        Some(list) => list.iter().filter_map(|s| KeyCombo::parse(s).ok()).map(|c| c.to_string()).collect(),
        None => editor_defaults(id),
    }
}

pub fn editor_is_overridden(cfg: &Config, id: &str) -> bool {
    cfg.editor.shortcuts.contains_key(id)
}

/// Set the combos for `id`; stores an override only when it differs from the defaults.
/// Duplicates are dropped, order preserved.
pub fn set_editor_combos(cfg: &mut Config, id: &str, combos: &[String]) {
    let mut canon: Vec<String> = Vec::new();
    for c in combos {
        if let Ok(k) = KeyCombo::parse(c) {
            let s = k.to_string();
            if !canon.contains(&s) {
                canon.push(s);
            }
        }
    }
    if canon == editor_defaults(id) {
        cfg.editor.shortcuts.remove(id);
    } else {
        cfg.editor.shortcuts.insert(id.to_string(), canon);
    }
}

pub fn reset_all_editor(cfg: &mut Config) {
    cfg.editor.shortcuts.clear();
}

/// Editor combos bound to more than one command.
pub fn editor_conflicts(cfg: &Config) -> Vec<(KeyCombo, Vec<&'static str>)> {
    let (map, _) = shortcuts::resolve_editor_keymap(&cfg.editor.shortcuts);
    shortcuts::conflicts(&map)
}

/// `#rrggbb` or `#rrggbbaa`.
pub fn is_hex_color(s: &str) -> bool {
    s.strip_prefix('#').is_some_and(|h| (h.len() == 6 || h.len() == 8) && h.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Parse `#rrggbb[aa]` to RGBA bytes.
pub fn parse_hex_color(s: &str) -> Option<[u8; 4]> {
    if !is_hex_color(s) {
        return None;
    }
    let h = &s[1..];
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([b(0)?, b(2)?, b(4)?, if h.len() == 8 { b(6)? } else { 255 }])
}

pub fn format_hex_color(c: [u8; 4]) -> String {
    if c[3] == 255 {
        format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3])
    }
}

/// Rewrite hotkey strings into canonical form. Invalid ones are left untouched
/// (validation reports them). Call before saving.
pub fn normalize_config(cfg: &mut Config) {
    for hk in &mut cfg.hotkeys {
        if let Ok(n) = hotkey::normalize(&hk.keys) {
            hk.keys = n;
        }
    }
    for list in cfg.editor.shortcuts.values_mut() {
        for c in list.iter_mut() {
            if let Ok(k) = KeyCombo::parse(c) {
                *c = k.to_string();
            }
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

/// Preset for "Tambah hotkey OCR": region capture -> OCR, keys left empty.
pub fn ocr_binding() -> jepret_core::config::HotkeyBinding {
    jepret_core::config::HotkeyBinding {
        keys: String::new(),
        capture: jepret_core::CaptureKind::Region,
        actions: vec![Action::Ocr],
        delay_secs: 0,
    }
}

/// Longest capture delay the UI offers (seconds).
pub const MAX_DELAY_SECS: u32 = 30;

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use jepret_core::{config::HotkeyBinding, CaptureKind, CommandSpec};

    #[test]
    fn ocr_preset_is_region_ocr_without_keys() {
        let b = ocr_binding();
        assert_eq!(b.capture, CaptureKind::Region);
        assert_eq!(b.actions, vec![Action::Ocr]);
        assert!(b.keys.is_empty());
        assert_eq!(b.delay_secs, 0);
    }

    fn hk(keys: &str) -> HotkeyBinding {
        HotkeyBinding { keys: keys.into(), capture: CaptureKind::Region, actions: vec![], delay_secs: 0 }
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
            delay_secs: 3,
        });
        add_app(&mut c, "C:/Tools/Paint.NET/paint.exe".into(), Some("Paint".into()));
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), c);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn editor_override_stored_only_when_different() {
        let mut c = Config::default();
        assert_eq!(editor_effective(&c, "tool.arrow"), vec!["a"]);
        set_editor_combos(&mut c, "tool.arrow", &["Ctrl+Shift+A".into(), "ctrl+shift+a".into()]);
        assert_eq!(c.editor.shortcuts["tool.arrow"], vec!["ctrl+shift+a"]);
        assert!(editor_is_overridden(&c, "tool.arrow"));
        // back to default (lenient spelling) -> override removed
        set_editor_combos(&mut c, "tool.arrow", &["A".into()]);
        assert!(c.editor.shortcuts.is_empty());
        // multi-combo default compares as ordered list
        set_editor_combos(&mut c, "edit.redo", &["ctrl+y".into(), "ctrl+shift+z".into()]);
        assert!(c.editor.shortcuts.is_empty());
        // disabling = empty override
        set_editor_combos(&mut c, "tool.pixelate", &[]);
        assert_eq!(c.editor.shortcuts["tool.pixelate"], Vec::<String>::new());
        assert!(editor_effective(&c, "tool.pixelate").is_empty());
        reset_all_editor(&mut c);
        assert!(c.editor.shortcuts.is_empty());
    }

    #[test]
    fn editor_conflict_is_warning() {
        let mut c = Config::default();
        set_editor_combos(&mut c, "tool.arrow", &["r".into()]); // clashes with tool.rect
        assert_eq!(editor_conflicts(&c).len(), 1);
        let issues = validate(&c);
        assert!(!has_errors(&issues));
        assert!(issues.iter().any(|i| i.message.contains("bentrok")));
    }

    #[test]
    fn editor_config_validation_and_colors() {
        let mut c = Config::default();
        c.editor.palette[0] = "red".into();
        c.editor.default_tool = "edit.undo".into();
        assert_eq!(validate(&c).iter().filter(|i| i.severity == Severity::Error).count(), 2);
        assert_eq!(parse_hex_color("#e53935"), Some([0xe5, 0x39, 0x35, 255]));
        assert_eq!(parse_hex_color("#11223344"), Some([0x11, 0x22, 0x33, 0x44]));
        assert_eq!(parse_hex_color("#12"), None);
        assert_eq!(format_hex_color([1, 2, 3, 255]), "#010203");
        assert_eq!(parse_hex_color(&format_hex_color([9, 8, 7, 6])), Some([9, 8, 7, 6]));
    }

    #[test]
    fn default_hotkeys_have_no_duplicates() {
        let c = Config::default();
        let ks: Vec<&str> = c.hotkeys.iter().map(|h| h.keys.as_str()).collect();
        assert!(hotkey::find_duplicates(&ks).is_empty());
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
