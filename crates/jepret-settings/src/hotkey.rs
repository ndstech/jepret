//! Thin wrapper over [`jepret_core::KeyCombo`] (the single source of truth for
//! the grammar; canonical example: `ctrl+shift+printscreen`) plus the
//! settings-specific rules: a *global* hotkey needs a modifier unless it is
//! `printscreen`, `pause`, `scrolllock` or `f1`..`f24`.
//!
//! Also maps Win32 virtual-key codes to canonical key names for the recorder.

use jepret_core::KeyCombo;

/// Keys that may be bound as a global hotkey without any modifier.
fn allowed_bare(key: &str) -> bool {
    matches!(key, "printscreen" | "pause" | "scrolllock")
        || (key.len() > 1 && key.starts_with('f') && key[1..].chars().all(|c| c.is_ascii_digit()))
}

/// Parse a global hotkey string, enforcing the modifier rule.
pub fn parse(s: &str) -> Result<KeyCombo, String> {
    let c = KeyCombo::parse(s).map_err(|e| e.to_string())?;
    check_global(&c)?;
    Ok(c)
}

pub fn check_global(c: &KeyCombo) -> Result<(), String> {
    if !c.has_modifier() && !allowed_bare(&c.key) {
        return Err(format!("'{}' perlu minimal satu modifier (ctrl/alt/shift/win)", c.key));
    }
    Ok(())
}

/// Parse and re-serialize in canonical form (global-hotkey rules).
pub fn normalize(s: &str) -> Result<String, String> {
    parse(s).map(|c| c.to_string())
}

/// Index pairs `(first, later)` whose hotkeys are equal after normalization.
/// Unparseable entries are ignored (reported separately).
pub fn find_duplicates<S: AsRef<str>>(keys: &[S]) -> Vec<(usize, usize)> {
    let norm: Vec<Option<String>> = keys.iter().map(|k| normalize(k.as_ref()).ok()).collect();
    let mut out = Vec::new();
    for j in 0..norm.len() {
        let Some(nj) = &norm[j] else { continue };
        if let Some(i) = (0..j).find(|&i| norm[i].as_ref() == Some(nj)) {
            out.push((i, j));
        }
    }
    out
}

/// Canonical key name for a Win32 virtual-key code (non-modifier keys only).
pub fn key_name_from_vk(vk: u32) -> Option<String> {
    let s = match vk {
        0x2C => "printscreen",
        0x0D => "enter",
        0x1B => "escape",
        0x20 => "space",
        0x09 => "tab",
        0x08 => "backspace",
        0x2E => "delete",
        0x2D => "insert",
        0x24 => "home",
        0x23 => "end",
        0x21 => "pageup",
        0x22 => "pagedown",
        0x25 => "left",
        0x26 => "up",
        0x27 => "right",
        0x28 => "down",
        0x13 => "pause",
        0x91 => "scrolllock",
        0xBB => "plus",
        0xBD => "minus",
        0xBC => "comma",
        0xBE => "period",
        0xBF => "slash",
        0xDC => "backslash",
        0xBA => "semicolon",
        0xDE => "quote",
        0xC0 => "backquote",
        0xDB => "bracketleft",
        0xDD => "bracketright",
        0x6B => "numadd",
        0x6D => "numsubtract",
        0x6A => "nummultiply",
        0x6F => "numdivide",
        0x6E => "numdecimal",
        0x30..=0x39 | 0x41..=0x5A => return Some(((vk as u8) as char).to_ascii_lowercase().to_string()),
        0x60..=0x69 => return Some(format!("num{}", vk - 0x60)),
        0x70..=0x87 => return Some(format!("f{}", vk - 0x70 + 1)),
        _ => return None,
    };
    Some(s.to_string())
}

/// Build a combo from a captured key press.
pub fn combo_from_vk(vk: u32, ctrl: bool, alt: bool, shift: bool, win: bool) -> Result<KeyCombo, String> {
    let key = key_name_from_vk(vk).ok_or_else(|| format!("tombol tidak didukung (VK 0x{vk:02X})"))?;
    let mut c = KeyCombo::key(&key);
    c.ctrl = ctrl;
    c.alt = alt;
    c.shift = shift;
    c.win = win;
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_to_core_canonical() {
        assert_eq!(normalize("Ctrl+Shift+4").unwrap(), "ctrl+shift+4");
        assert_eq!(normalize("shift + CONTROL + PrtSc").unwrap(), "ctrl+shift+printscreen");
        assert_eq!(normalize("alt+prtsc").unwrap(), "alt+printscreen");
        assert_eq!(normalize("win+ctrl+alt+shift+f5").unwrap(), "ctrl+alt+shift+win+f5");
        assert_eq!(normalize("ctrl+=").unwrap(), "ctrl+plus");
        assert_eq!(normalize("ctrl+]").unwrap(), "ctrl+bracketright");
        assert_eq!(normalize("ctrl+Esc").unwrap(), "ctrl+escape");
        assert_eq!(normalize("ctrl+`").unwrap(), "ctrl+backquote");
    }

    #[test]
    fn canonical_is_idempotent() {
        for s in ["ctrl+shift+4", "printscreen", "alt+printscreen", "ctrl+alt+f12", "win+space", "ctrl+plus"] {
            assert_eq!(normalize(s).unwrap(), s);
        }
    }

    #[test]
    fn rejects_invalid_global() {
        for s in ["", "ctrl+shift", "ctrl+a+b", "ctrl+foo", "a", "4", "space", "f25"] {
            assert!(normalize(s).is_err(), "{s} should be invalid");
        }
    }

    #[test]
    fn bare_special_keys_allowed() {
        assert_eq!(normalize("F9").unwrap(), "f9");
        assert_eq!(normalize("PrtSc").unwrap(), "printscreen");
        assert_eq!(normalize("pause").unwrap(), "pause");
    }

    #[test]
    fn duplicates_detected_after_normalization() {
        let keys = ["ctrl+shift+4", "printscreen", "Shift+Ctrl+4", "bogus", "bogus", "PRTSC"];
        assert_eq!(find_duplicates(&keys), vec![(0, 2), (1, 5)]);
        assert!(find_duplicates(&["ctrl+a", "ctrl+b"]).is_empty());
    }

    #[test]
    fn vk_mapping() {
        assert_eq!(combo_from_vk(0x2C, false, false, false, false).unwrap().to_string(), "printscreen");
        assert_eq!(combo_from_vk(0x2C, true, false, true, false).unwrap().to_string(), "ctrl+shift+printscreen");
        assert_eq!(combo_from_vk(0x34, true, false, false, true).unwrap().to_string(), "ctrl+win+4");
        assert_eq!(combo_from_vk(0x41, true, false, false, false).unwrap().to_string(), "ctrl+a");
        assert_eq!(combo_from_vk(0x7B, false, false, false, false).unwrap().to_string(), "f12");
        assert_eq!(combo_from_vk(0xDD, true, false, false, false).unwrap().to_string(), "ctrl+bracketright");
        assert_eq!(combo_from_vk(0x65, false, false, false, false).unwrap().to_string(), "num5");
        assert!(combo_from_vk(0xFF, false, false, false, false).is_err());
        // every mapped name must be accepted by the core parser
        for vk in 0..=0xFFu32 {
            if let Some(n) = key_name_from_vk(vk) {
                assert_eq!(KeyCombo::parse(&n).unwrap().key, n, "vk {vk:#x}");
            }
        }
    }
}
