//! egui keyboard events -> [`KeyCombo`] -> shortcut id.

use std::collections::HashMap;

use jepret_core::config::EditorConfig;
use jepret_core::keys::KeyCombo;
use jepret_core::shortcuts::{conflicts, resolve_editor_keymap, EDITOR_SHORTCUTS};

/// Canonical key name for an egui key plus an implied Shift (for keys that
/// *are* a shifted symbol, e.g. `?` = Shift+`/`). `None` for keys we do not
/// use.
fn key_name(key: egui::Key) -> Option<(String, bool)> {
    use egui::Key as K;
    let fixed = |n: &str| Some((n.to_string(), false));
    match key {
        K::Enter => fixed("enter"),
        K::Escape => fixed("escape"),
        K::Space => fixed("space"),
        K::Tab => fixed("tab"),
        K::Backspace => fixed("backspace"),
        K::Delete => fixed("delete"),
        K::Insert => fixed("insert"),
        K::Home => fixed("home"),
        K::End => fixed("end"),
        K::PageUp => fixed("pageup"),
        K::PageDown => fixed("pagedown"),
        K::ArrowUp => fixed("up"),
        K::ArrowDown => fixed("down"),
        K::ArrowLeft => fixed("left"),
        K::ArrowRight => fixed("right"),
        K::Comma => fixed("comma"),
        K::Period => fixed("period"),
        K::Slash => fixed("slash"),
        K::Backslash => fixed("backslash"),
        K::Semicolon => fixed("semicolon"),
        K::Quote => fixed("quote"),
        K::Backtick => fixed("backquote"),
        K::Minus => fixed("minus"),
        K::OpenBracket => fixed("bracketleft"),
        K::CloseBracket => fixed("bracketright"),
        // `=` and `+` share a physical key; both mean "plus" (Shift ignored).
        K::Equals | K::Plus => fixed("plus"),
        K::Questionmark => Some(("slash".into(), true)),
        K::Colon => Some(("semicolon".into(), true)),
        K::Pipe => Some(("backslash".into(), true)),
        other => {
            let n = other.name();
            let is_fkey = n.len() >= 2 && n.starts_with('F') && n[1..].chars().all(|c| c.is_ascii_digit());
            if (n.len() == 1 && n.is_ascii()) || is_fkey {
                Some((n.to_ascii_lowercase(), false))
            } else {
                None
            }
        }
    }
}

/// Map a key press + modifiers to a [`KeyCombo`].
pub fn combo_from_key(key: egui::Key, m: egui::Modifiers) -> Option<KeyCombo> {
    let (name, implied_shift) = key_name(key)?;
    let plus_like = name == "plus";
    Some(KeyCombo {
        ctrl: m.ctrl || m.command,
        alt: m.alt,
        // Shift is meaningless for `+`/`=` (US layout needs it for `+`).
        shift: if plus_like { false } else { m.shift || implied_shift },
        win: false,
        key: name,
    })
}

/// Resolved keymap: defaults + `config.editor.shortcuts`.
pub struct Keymap {
    by_combo: HashMap<KeyCombo, &'static str>,
    by_id: Vec<(&'static str, Vec<KeyCombo>)>,
    /// Problems found (bad strings, unknown ids, conflicts) for logging.
    pub warnings: Vec<String>,
}

impl Keymap {
    pub fn from_config(cfg: &EditorConfig) -> Self {
        let (by_id, mut warnings) = resolve_editor_keymap(&cfg.shortcuts);
        for (combo, ids) in conflicts(&by_id) {
            warnings.push(format!("shortcut {} bound to {:?}; first wins", combo.pretty(), ids));
        }
        let mut by_combo = HashMap::new();
        for (id, combos) in &by_id {
            for c in combos {
                by_combo.entry(c.clone()).or_insert(*id);
            }
        }
        Self { by_combo, by_id, warnings }
    }

    pub fn lookup(&self, c: &KeyCombo) -> Option<&'static str> {
        self.by_combo.get(c).copied()
    }

    /// Resolve a pressed combo. Returns `(id, shift_extra)`; `shift_extra`
    /// is set when only the Shift-less combo is bound (arrow keys: Shift =
    /// 10 px) and the id accepts it.
    pub fn resolve(&self, c: &KeyCombo) -> Option<(&'static str, bool)> {
        if let Some(id) = self.lookup(c) {
            return Some((id, false));
        }
        if c.shift {
            let mut plain = c.clone();
            plain.shift = false;
            if let Some(id) = self.lookup(&plain) {
                if id.starts_with("edit.nudge") {
                    return Some((id, true));
                }
            }
        }
        None
    }

    pub fn combos(&self, id: &str) -> &[KeyCombo] {
        self.by_id.iter().find(|(i, _)| *i == id).map(|(_, c)| c.as_slice()).unwrap_or(&[])
    }

    /// Pretty label of the first combo bound to `id` (empty if unbound).
    pub fn hint(&self, id: &str) -> String {
        self.combos(id).first().map(|c| c.pretty()).unwrap_or_default()
    }

    /// All combos pretty-printed, joined by " / ".
    pub fn hint_all(&self, id: &str) -> String {
        self.combos(id).iter().map(|c| c.pretty()).collect::<Vec<_>>().join(" / ")
    }

    /// `(group, [(label, keys)])` in catalogue order, for the cheat-sheet.
    pub fn cheatsheet(&self) -> Vec<(&'static str, Vec<(&'static str, String)>)> {
        let mut out: Vec<(&'static str, Vec<(&'static str, String)>)> = Vec::new();
        for def in EDITOR_SHORTCUTS {
            let keys = match self.hint_all(def.id) {
                k if k.is_empty() => "(none)".to_string(),
                k => k,
            };
            match out.iter_mut().find(|(g, _)| *g == def.group) {
                Some((_, v)) => v.push((def.label, keys)),
                None => out.push((def.group, vec![(def.label, keys)])),
            }
        }
        out
    }
}

/// Gestures that are not remappable (shown in the cheat-sheet).
pub const FIXED_GESTURES: &[(&str, &str)] = &[
    ("Shift saat menggambar", "lurus 45 derajat / kotak / lingkaran"),
    ("Alt saat menggambar", "gambar dari titik tengah"),
    ("Space + drag, drag tombol tengah", "geser (pan)"),
    ("Ctrl + scroll", "zoom"),
    ("Shift + panah", "geser objek 10 px"),
    ("Dobel-klik teks (tool Pilih)", "ubah teks"),
    ("Saat mengetik teks", "hanya Esc (batal) dan Ctrl+Enter (selesai) aktif"),
    ("Esc", "batalkan drag, lalu lepas seleksi, lalu tutup"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Key, Modifiers};

    const NONE: Modifiers = Modifiers::NONE;
    const SHIFT: Modifiers = Modifiers::SHIFT;
    const CTRL: Modifiers = Modifiers::CTRL;

    fn c(key: Key, m: Modifiers) -> String {
        combo_from_key(key, m).map(|c| c.to_string()).unwrap_or_default()
    }

    #[test]
    fn letters_digits_and_f_keys() {
        assert_eq!(c(Key::A, NONE), "a");
        assert_eq!(c(Key::A, SHIFT), "shift+a");
        assert_eq!(c(Key::Num7, NONE), "7");
        assert_eq!(c(Key::Num0, CTRL), "ctrl+0");
        assert_eq!(c(Key::F1, NONE), "f1");
        assert_eq!(c(Key::F12, NONE), "f12");
    }

    #[test]
    fn modifiers_and_special_keys() {
        assert_eq!(c(Key::Z, CTRL), "ctrl+z");
        assert_eq!(c(Key::Z, Modifiers::CTRL | Modifiers::SHIFT), "ctrl+shift+z");
        assert_eq!(c(Key::S, Modifiers::COMMAND), "ctrl+s");
        assert_eq!(c(Key::Enter, Modifiers::CTRL | Modifiers::SHIFT), "ctrl+shift+enter");
        assert_eq!(c(Key::Escape, NONE), "escape");
        assert_eq!(c(Key::ArrowLeft, SHIFT), "shift+left");
        assert_eq!(c(Key::Delete, NONE), "delete");
        assert_eq!(c(Key::Backspace, NONE), "backspace");
    }

    #[test]
    fn punctuation() {
        assert_eq!(c(Key::OpenBracket, NONE), "bracketleft");
        assert_eq!(c(Key::CloseBracket, NONE), "bracketright");
        assert_eq!(c(Key::CloseBracket, CTRL), "ctrl+bracketright");
        assert_eq!(c(Key::OpenBracket, Modifiers::CTRL), "ctrl+bracketleft");
        assert_eq!(c(Key::Minus, CTRL), "ctrl+minus");
        // `=` and `+` both mean plus, with or without Shift
        assert_eq!(c(Key::Equals, CTRL), "ctrl+plus");
        assert_eq!(c(Key::Plus, Modifiers::CTRL | Modifiers::SHIFT), "ctrl+plus");
        assert_eq!(c(Key::Slash, SHIFT), "shift+slash");
        assert_eq!(c(Key::Questionmark, NONE), "shift+slash");
        assert_eq!(c(Key::Questionmark, SHIFT), "shift+slash");
        assert_eq!(c(Key::Slash, NONE), "slash");
    }

    #[test]
    fn default_keymap_resolves_event_to_ids() {
        let km = Keymap::from_config(&EditorConfig::default());
        assert!(km.warnings.is_empty(), "{:?}", km.warnings);
        let id = |k, m| km.resolve(&combo_from_key(k, m).unwrap()).map(|r| r.0);
        assert_eq!(id(Key::V, NONE), Some("tool.select"));
        assert_eq!(id(Key::X, NONE), Some("tool.pixelate"));
        assert_eq!(id(Key::Num3, NONE), Some("style.color_3"));
        assert_eq!(id(Key::CloseBracket, NONE), Some("style.width_up"));
        assert_eq!(id(Key::OpenBracket, NONE), Some("style.width_down"));
        assert_eq!(id(Key::CloseBracket, CTRL), Some("edit.bring_front"));
        assert_eq!(id(Key::Y, CTRL), Some("edit.redo"));
        assert_eq!(id(Key::Z, Modifiers::CTRL | Modifiers::SHIFT), Some("edit.redo"));
        assert_eq!(id(Key::Equals, CTRL), Some("view.zoom_in"));
        assert_eq!(id(Key::Minus, CTRL), Some("view.zoom_out"));
        assert_eq!(id(Key::Questionmark, SHIFT), Some("view.cheatsheet"));
        assert_eq!(id(Key::F1, NONE), Some("view.cheatsheet"));
        assert_eq!(id(Key::Enter, NONE), Some("file.copy_close"));
        assert_eq!(id(Key::Escape, NONE), Some("file.close"));
        assert_eq!(id(Key::C, Modifiers::CTRL | Modifiers::SHIFT), Some("file.copy_path"));
        assert_eq!(id(Key::Q, NONE), None);
        // Shift+arrow = nudge x10, but Shift+letter is not a shortcut
        assert_eq!(km.resolve(&combo_from_key(Key::ArrowLeft, SHIFT).unwrap()), Some(("edit.nudge_left", true)));
        assert_eq!(km.resolve(&combo_from_key(Key::ArrowLeft, NONE).unwrap()), Some(("edit.nudge_left", false)));
        assert_eq!(id(Key::A, SHIFT), None);
    }

    #[test]
    fn user_overrides_apply() {
        let mut cfg = EditorConfig::default();
        cfg.shortcuts.insert("tool.arrow".into(), vec!["q".into()]);
        cfg.shortcuts.insert("tool.pixelate".into(), vec![]);
        let km = Keymap::from_config(&cfg);
        let id = |k| km.resolve(&combo_from_key(k, NONE).unwrap()).map(|r| r.0);
        assert_eq!(id(Key::Q), Some("tool.arrow"));
        assert_eq!(id(Key::A), None);
        assert_eq!(id(Key::X), None);
        assert_eq!(km.hint("tool.arrow"), "Q");
        assert_eq!(km.hint("tool.pixelate"), "");
    }

    #[test]
    fn cheatsheet_covers_catalogue() {
        let km = Keymap::from_config(&EditorConfig::default());
        let sheet = km.cheatsheet();
        let n: usize = sheet.iter().map(|(_, v)| v.len()).sum();
        assert_eq!(n, EDITOR_SHORTCUTS.len());
        assert!(sheet.iter().any(|(g, _)| *g == "Tool"));
        assert_eq!(km.hint_all("edit.redo"), "Ctrl+Y / Ctrl+Shift+Z");
    }
}
