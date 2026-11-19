//! Editor shortcut catalogue + user overrides.
//!
//! * [`EDITOR_SHORTCUTS`] is the single source of truth for every editor
//!   command: stable id, label, group and default key combos. The editor
//!   (T06) dispatches on the id; the settings UI (T11) lists/edits them;
//!   the editor's F1 cheat-sheet is generated from it.
//! * Users override in `config.toml`:
//!   ```toml
//!   [editor.shortcuts]
//!   "tool.arrow" = ["a", "ctrl+shift+a"]   # replace defaults
//!   "tool.pixelate" = []                    # disable
//!   ```
//!
//! Fixed (non-remappable) gestures, shown in the cheat-sheet:
//! hold **Shift** while drawing = constrain (45° / square / circle);
//! hold **Alt** while drawing = draw from center; hold **Space** + drag = pan;
//! **Ctrl+wheel** = zoom; **Shift** + nudge = 10 px; while typing in a text
//! object only `escape` and `ctrl+enter` act as shortcuts; **Esc** first
//! deselects / cancels the current drag, closes only when nothing is active.

use std::collections::BTreeMap;

use crate::keys::KeyCombo;

#[derive(Debug, Clone, Copy)]
pub struct ShortcutDef {
    pub id: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub defaults: &'static [&'static str],
}

const fn d(
    id: &'static str,
    label: &'static str,
    group: &'static str,
    defaults: &'static [&'static str],
) -> ShortcutDef {
    ShortcutDef { id, label, group, defaults }
}

pub const EDITOR_SHORTCUTS: &[ShortcutDef] = &[
    // ---- tools (single letters, like most editors) ----
    d("tool.select", "Pilih / pindah", "Tool", &["v"]),
    d("tool.crop", "Crop", "Tool", &["c"]),
    d("tool.arrow", "Panah", "Tool", &["a"]),
    d("tool.line", "Garis", "Tool", &["l"]),
    d("tool.rect", "Kotak", "Tool", &["r"]),
    d("tool.ellipse", "Elips", "Tool", &["e"]),
    d("tool.text", "Teks", "Tool", &["t"]),
    d("tool.textbox", "Kotak teks", "Tool", &["shift+t"]),
    d("tool.callout", "Callout", "Tool", &["k"]),
    d("tool.highlight", "Highlighter", "Tool", &["h"]),
    d("tool.blur", "Blur", "Tool", &["b"]),
    d("tool.pixelate", "Pixelate", "Tool", &["x"]),
    d("tool.step", "Nomor urut", "Tool", &["n"]),
    d("tool.pen", "Pen bebas", "Tool", &["p"]),
    d("tool.polygon", "Polygon", "Tool", &["g"]),
    d("tool.spotlight", "Sorotan", "Tool", &["u"]),
    d("tool.magnify", "Kaca pembesar", "Tool", &["m"]),
    d("tool.eyedropper", "Ambil warna", "Tool", &["i"]),
    // ---- style ----
    d("style.color_1", "Warna 1", "Gaya", &["1"]),
    d("style.color_2", "Warna 2", "Gaya", &["2"]),
    d("style.color_3", "Warna 3", "Gaya", &["3"]),
    d("style.color_4", "Warna 4", "Gaya", &["4"]),
    d("style.color_5", "Warna 5", "Gaya", &["5"]),
    d("style.color_6", "Warna 6", "Gaya", &["6"]),
    d("style.color_7", "Warna 7", "Gaya", &["7"]),
    d("style.color_8", "Warna 8", "Gaya", &["8"]),
    d("style.color_9", "Warna 9", "Gaya", &["9"]),
    d("style.width_up", "Tebalkan", "Gaya", &["bracketright"]),
    d("style.width_down", "Tipiskan", "Gaya", &["bracketleft"]),
    d("style.toggle_fill", "Isi / outline", "Gaya", &["f"]),
    d("style.toggle_shadow", "Bayangan on/off", "Gaya", &["s"]),
    d("style.toggle_outline", "Garis tepi teks on/off", "Gaya", &["o"]),
    // ---- edit ----
    d("edit.undo", "Undo", "Edit", &["ctrl+z"]),
    d("edit.redo", "Redo", "Edit", &["ctrl+y", "ctrl+shift+z"]),
    d("edit.delete", "Hapus objek", "Edit", &["delete", "backspace"]),
    d("edit.duplicate", "Duplikat objek", "Edit", &["ctrl+d"]),
    d("edit.select_all", "Pilih semua objek", "Edit", &["ctrl+a"]),
    d("edit.bring_front", "Ke depan", "Edit", &["ctrl+bracketright"]),
    d("edit.send_back", "Ke belakang", "Edit", &["ctrl+bracketleft"]),
    d("edit.nudge_left", "Geser kiri", "Edit", &["left"]),
    d("edit.nudge_right", "Geser kanan", "Edit", &["right"]),
    d("edit.nudge_up", "Geser atas", "Edit", &["up"]),
    d("edit.nudge_down", "Geser bawah", "Edit", &["down"]),
    d("edit.reset_steps", "Reset nomor urut ke 1", "Edit", &["ctrl+shift+n"]),
    // ---- view ----
    d("view.zoom_in", "Zoom in", "Tampilan", &["ctrl+plus"]),
    d("view.zoom_out", "Zoom out", "Tampilan", &["ctrl+minus"]),
    d("view.zoom_fit", "Pas layar", "Tampilan", &["ctrl+0"]),
    d("view.zoom_100", "Zoom 100%", "Tampilan", &["ctrl+1"]),
    d("view.cheatsheet", "Daftar shortcut", "Tampilan", &["f1", "shift+slash"]),
    // ---- file / output ----
    d("file.copy", "Copy gambar", "File", &["ctrl+c"]),
    d("file.copy_close", "Copy & tutup", "File", &["enter", "ctrl+enter"]),
    d("file.save", "Simpan", "File", &["ctrl+s"]),
    d("file.save_as", "Simpan sebagai…", "File", &["ctrl+shift+s"]),
    d("file.save_close", "Simpan & tutup", "File", &["ctrl+shift+enter"]),
    d("file.copy_path", "Copy path file", "File", &["ctrl+shift+c"]),
    d("file.open_with", "Buka di aplikasi lain…", "File", &["ctrl+o"]),
    d("file.open_folder", "Buka folder", "File", &["ctrl+shift+o"]),
    d("file.close", "Tutup", "File", &["escape"]),
];

pub fn editor_shortcut(id: &str) -> Option<&'static ShortcutDef> {
    EDITOR_SHORTCUTS.iter().find(|s| s.id == id)
}

/// Resolved keymap: defaults with user overrides applied.
/// Invalid override strings are skipped and reported in the second value.
pub fn resolve_editor_keymap(
    overrides: &BTreeMap<String, Vec<String>>,
) -> (Vec<(&'static str, Vec<KeyCombo>)>, Vec<String>) {
    let mut warnings = Vec::new();
    for id in overrides.keys() {
        if editor_shortcut(id).is_none() {
            warnings.push(format!("unknown editor shortcut id {id:?}"));
        }
    }
    let map = EDITOR_SHORTCUTS
        .iter()
        .map(|def| {
            let combos = match overrides.get(def.id) {
                Some(list) => list
                    .iter()
                    .filter_map(|s| match KeyCombo::parse(s) {
                        Ok(c) => Some(c),
                        Err(e) => {
                            warnings.push(format!("{}: {e}", def.id));
                            None
                        }
                    })
                    .collect(),
                None => def
                    .defaults
                    .iter()
                    .map(|s| KeyCombo::parse(s).expect("built-in default must parse"))
                    .collect(),
            };
            (def.id, combos)
        })
        .collect();
    (map, warnings)
}

/// Combos bound to more than one command: `(combo, [ids])`.
pub fn conflicts(keymap: &[(&'static str, Vec<KeyCombo>)]) -> Vec<(KeyCombo, Vec<&'static str>)> {
    let mut by_combo: BTreeMap<KeyCombo, Vec<&'static str>> = BTreeMap::new();
    for (id, combos) in keymap {
        for c in combos {
            by_combo.entry(c.clone()).or_default().push(id);
        }
    }
    by_combo.into_iter().filter(|(_, ids)| ids.len() > 1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse_unique_ids_no_conflicts() {
        let (map, warn) = resolve_editor_keymap(&BTreeMap::new());
        assert!(warn.is_empty());
        let mut ids: Vec<_> = EDITOR_SHORTCUTS.iter().map(|s| s.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), EDITOR_SHORTCUTS.len());
        assert!(conflicts(&map).is_empty(), "{:?}", conflicts(&map));
    }

    #[test]
    fn overrides_replace_and_disable() {
        let mut o = BTreeMap::new();
        o.insert("tool.arrow".to_string(), vec!["ctrl+shift+a".to_string()]);
        o.insert("tool.pixelate".to_string(), vec![]);
        o.insert("nope".to_string(), vec!["q".to_string()]);
        let (map, warn) = resolve_editor_keymap(&o);
        let get = |id| map.iter().find(|(i, _)| *i == id).unwrap().1.clone();
        assert_eq!(get("tool.arrow")[0].to_string(), "ctrl+shift+a");
        assert!(get("tool.pixelate").is_empty());
        assert_eq!(warn.len(), 1);
    }
}
