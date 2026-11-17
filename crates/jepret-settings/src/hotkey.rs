//! Pure hotkey-string logic (no egui / Win32).
//!
//! Canonical format (stored in `config.toml`, consumed by T12):
//! lowercase, `+`-separated, modifiers first in the fixed order
//! `ctrl+alt+shift+win`, then exactly one key name. Examples:
//! `ctrl+shift+4`, `printscreen`, `alt+printscreen`, `ctrl+alt+f5`.
//!
//! Key names: `a`..`z`, `0`..`9`, `f1`..`f24`, `printscreen`, `pause`,
//! `scrolllock`, `space`, `enter`, `esc`, `tab`, `backspace`, `delete`,
//! `insert`, `home`, `end`, `pageup`, `pagedown`, `up`, `down`, `left`,
//! `right`, `minus`, `equals`, `comma`, `period`, `semicolon`, `slash`,
//! `backslash`, `backtick`, `quote`, `lbracket`, `rbracket`.
//! Parsing is lenient (case, spaces, aliases like `control`, `super`,
//! `prtsc`, `escape`, `arrowup`); output is always canonical.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub key: String,
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("ctrl+")?;
        }
        if self.alt {
            f.write_str("alt+")?;
        }
        if self.shift {
            f.write_str("shift+")?;
        }
        if self.win {
            f.write_str("win+")?;
        }
        f.write_str(&self.key)
    }
}

/// Canonical key name for `raw`, or `None` if unknown.
pub fn canonical_key(raw: &str) -> Option<String> {
    let k = raw.trim().to_ascii_lowercase();
    let named = match k.as_str() {
        "printscreen" | "prtsc" | "prtscn" | "print" | "snapshot" => "printscreen",
        "pause" | "break" => "pause",
        "scrolllock" | "scroll" => "scrolllock",
        "space" | "spacebar" => "space",
        "enter" | "return" => "enter",
        "esc" | "escape" => "esc",
        "tab" => "tab",
        "backspace" => "backspace",
        "delete" | "del" => "delete",
        "insert" | "ins" => "insert",
        "home" => "home",
        "end" => "end",
        "pageup" | "pgup" => "pageup",
        "pagedown" | "pgdn" | "pgdown" => "pagedown",
        "up" | "arrowup" => "up",
        "down" | "arrowdown" => "down",
        "left" | "arrowleft" => "left",
        "right" | "arrowright" => "right",
        "minus" | "-" => "minus",
        "equals" | "equal" | "=" | "plus" => "equals",
        "comma" | "," => "comma",
        "period" | "." => "period",
        "semicolon" | ";" => "semicolon",
        "slash" | "/" => "slash",
        "backslash" | "\\" => "backslash",
        "backtick" | "`" | "grave" => "backtick",
        "quote" | "'" => "quote",
        "lbracket" | "openbracket" | "[" => "lbracket",
        "rbracket" | "closebracket" | "]" => "rbracket",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.to_string());
    }
    let b = k.as_bytes();
    if b.len() == 1 && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit()) {
        return Some(k);
    }
    // "num4" (egui debug name for the digit row) -> "4"
    if let Some(d) = k.strip_prefix("num") {
        if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() {
            return Some(d.to_string());
        }
    }
    if let Some(n) = k.strip_prefix('f') {
        if let Ok(n) = n.parse::<u8>() {
            // Reject forms like "f05": must round-trip.
            if (1..=24).contains(&n) && n.to_string() == n_str(&k) {
                return Some(format!("f{n}"));
            }
        }
    }
    None
}

fn n_str(k: &str) -> &str {
    &k[1..]
}

/// Keys that may be bound without any modifier.
fn allowed_bare(key: &str) -> bool {
    key == "printscreen"
        || key == "pause"
        || key == "scrolllock"
        || (key.len() > 1 && key.starts_with('f') && key[1..].chars().all(|c| c.is_ascii_digit()))
}

impl Hotkey {
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut hk = Hotkey { ctrl: false, alt: false, shift: false, win: false, key: String::new() };
        if s.trim().is_empty() {
            return Err("kombinasi kosong".into());
        }
        for part in s.split('+') {
            let p = part.trim().to_ascii_lowercase();
            if p.is_empty() {
                return Err("bagian kosong di antara '+'".into());
            }
            match p.as_str() {
                "ctrl" | "control" => hk.ctrl = true,
                "alt" => hk.alt = true,
                "shift" => hk.shift = true,
                "win" | "super" | "meta" | "cmd" | "windows" => hk.win = true,
                _ => {
                    if !hk.key.is_empty() {
                        return Err("hanya boleh satu tombol non-modifier".into());
                    }
                    hk.key = canonical_key(&p).ok_or_else(|| format!("tombol tidak dikenal: '{p}'"))?;
                }
            }
        }
        if hk.key.is_empty() {
            return Err("tidak ada tombol utama (hanya modifier)".into());
        }
        if !hk.has_modifier() && !allowed_bare(&hk.key) {
            return Err(format!("'{}' perlu minimal satu modifier (ctrl/alt/shift/win)", hk.key));
        }
        Ok(hk)
    }

    pub fn has_modifier(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.win
    }

    /// Build from a recorded key press. `key_name` is any name accepted by [`canonical_key`].
    pub fn from_parts(ctrl: bool, alt: bool, shift: bool, win: bool, key_name: &str) -> Result<Self, String> {
        let key = canonical_key(key_name).ok_or_else(|| format!("tombol tidak didukung: '{key_name}'"))?;
        let hk = Hotkey { ctrl, alt, shift, win, key };
        if !hk.has_modifier() && !allowed_bare(&hk.key) {
            return Err(format!("'{}' perlu minimal satu modifier", hk.key));
        }
        Ok(hk)
    }
}

/// Parse and re-serialize in canonical form.
pub fn normalize(s: &str) -> Result<String, String> {
    Hotkey::parse(s).map(|h| h.to_string())
}

/// Index pairs `(first, later)` whose hotkeys are the same after normalization.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_order_aliases() {
        assert_eq!(normalize("Ctrl+Shift+4").unwrap(), "ctrl+shift+4");
        assert_eq!(normalize("shift + CONTROL + 4").unwrap(), "ctrl+shift+4");
        assert_eq!(normalize("PrintScreen").unwrap(), "printscreen");
        assert_eq!(normalize("alt+PrtSc").unwrap(), "alt+printscreen");
        assert_eq!(normalize("win+ctrl+alt+shift+f5").unwrap(), "ctrl+alt+shift+win+f5");
        assert_eq!(normalize("ctrl+Num4").unwrap(), "ctrl+4");
        assert_eq!(normalize("ctrl+ArrowUp").unwrap(), "ctrl+up");
    }

    #[test]
    fn canonical_is_idempotent() {
        for s in ["ctrl+shift+4", "printscreen", "alt+printscreen", "ctrl+alt+f12", "win+space"] {
            assert_eq!(normalize(s).unwrap(), s);
        }
    }

    #[test]
    fn rejects_invalid() {
        for s in ["", "ctrl+", "ctrl+shift", "ctrl+a+b", "ctrl+foo", "a", "4", "space", "ctrl++4", "f25", "ctrl+f05"] {
            assert!(normalize(s).is_err(), "{s} should be invalid");
        }
    }

    #[test]
    fn bare_function_keys_allowed() {
        assert_eq!(normalize("F9").unwrap(), "f9");
        assert_eq!(normalize("pause").unwrap(), "pause");
    }

    #[test]
    fn duplicates_detected_after_normalization() {
        let keys = ["ctrl+shift+4", "printscreen", "Shift+Ctrl+4", "bogus", "bogus", "PRINTSCREEN"];
        assert_eq!(find_duplicates(&keys), vec![(0, 2), (1, 5)]);
        assert!(find_duplicates(&["ctrl+a", "ctrl+b"]).is_empty());
    }

    #[test]
    fn from_parts_works() {
        assert_eq!(Hotkey::from_parts(true, false, true, false, "Num4").unwrap().to_string(), "ctrl+shift+4");
        assert!(Hotkey::from_parts(false, false, false, false, "A").is_err());
        assert!(Hotkey::from_parts(true, false, false, false, "???").is_err());
    }
}
