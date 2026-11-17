//! Key-combo strings shared by global hotkeys (daemon), editor shortcuts
//! and the settings UI.
//!
//! Canonical form: lowercase, `+`-separated, modifiers in the order
//! `ctrl+alt+shift+win+<key>` — e.g. `"ctrl+shift+printscreen"`.
//! Parsing is lenient (case, spaces, aliases such as `prtsc`, `esc`, `del`,
//! `control`, `cmd`, `=`/`-`); [`KeyCombo::to_string`] always yields the
//! canonical form and [`KeyCombo::pretty`] the UI label (`Ctrl+Shift+PrtSc`).

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyCombo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    /// Canonical key name, see [`KEY_NAMES`].
    pub key: String,
}

/// Every non-modifier key name accepted in canonical form
/// (plus `a`–`z`, `0`–`9`, `f1`–`f24`, `num0`–`num9`).
pub const KEY_NAMES: &[&str] = &[
    "printscreen", "enter", "escape", "space", "tab", "backspace", "delete", "insert",
    "home", "end", "pageup", "pagedown", "up", "down", "left", "right",
    "plus", "minus", "comma", "period", "slash", "backslash", "semicolon", "quote",
    "backquote", "bracketleft", "bracketright", "pause", "scrolllock",
    "numadd", "numsubtract", "nummultiply", "numdivide", "numdecimal",
];

fn normalize_key(raw: &str) -> Option<String> {
    let k = raw.trim().to_ascii_lowercase();
    let alias = match k.as_str() {
        "prtsc" | "prtscn" | "prtscr" | "print" | "printscr" | "snapshot" => "printscreen",
        "return" => "enter",
        "esc" => "escape",
        "del" => "delete",
        "ins" => "insert",
        "pgup" => "pageup",
        "pgdn" | "pgdown" => "pagedown",
        "arrowup" => "up",
        "arrowdown" => "down",
        "arrowleft" => "left",
        "arrowright" => "right",
        "=" | "equal" | "equals" => "plus",
        "-" | "dash" | "hyphen" => "minus",
        "," => "comma",
        "." | "dot" => "period",
        "/" | "?" => "slash",
        "\\" => "backslash",
        ";" => "semicolon",
        "'" => "quote",
        "`" | "grave" | "tilde" => "backquote",
        "[" => "bracketleft",
        "]" => "bracketright",
        other => other,
    };
    let valid = KEY_NAMES.contains(&alias)
        || (alias.len() == 1 && alias.chars().all(|c| c.is_ascii_alphanumeric()))
        || alias
            .strip_prefix('f')
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| (1..=24).contains(&n))
        || alias
            .strip_prefix("num")
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| n <= 9);
    valid.then(|| alias.to_string())
}

impl KeyCombo {
    pub fn key(key: &str) -> Self {
        Self { ctrl: false, alt: false, shift: false, win: false, key: key.into() }
    }

    pub fn parse(s: &str) -> crate::Result<Self> {
        let mut c = KeyCombo::key("");
        let mut key: Option<String> = None;
        // Allow "ctrl++" / "ctrl+=" style: split on '+' but treat a trailing empty part as "plus".
        let parts: Vec<&str> = s.split('+').collect();
        for (i, part) in parts.iter().enumerate() {
            let p = part.trim().to_ascii_lowercase();
            if p.is_empty() {
                if i == parts.len() - 1 && i > 0 {
                    key = Some("plus".into());
                    continue;
                }
                if i < parts.len() - 1 {
                    continue; // the "+" of "ctrl++" handled by the trailing empty part
                }
                anyhow::bail!("empty key combo");
            }
            match p.as_str() {
                "ctrl" | "control" | "ctl" => c.ctrl = true,
                "alt" | "option" => c.alt = true,
                "shift" => c.shift = true,
                "win" | "super" | "meta" | "cmd" | "windows" => c.win = true,
                other => {
                    anyhow::ensure!(key.is_none(), "more than one key in combo {s:?}");
                    key = Some(
                        normalize_key(other)
                            .ok_or_else(|| anyhow::anyhow!("unknown key {other:?} in {s:?}"))?,
                    );
                }
            }
        }
        c.key = key.ok_or_else(|| anyhow::anyhow!("combo {s:?} has only modifiers"))?;
        Ok(c)
    }

    pub fn has_modifier(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.win
    }

    /// UI label, e.g. `Ctrl+Shift+PrtSc`, `]`, `F1`.
    pub fn pretty(&self) -> String {
        let mut out = String::new();
        for (on, name) in [(self.ctrl, "Ctrl"), (self.alt, "Alt"), (self.shift, "Shift"), (self.win, "Win")] {
            if on {
                out.push_str(name);
                out.push('+');
            }
        }
        let k = match self.key.as_str() {
            "printscreen" => "PrtSc".to_string(),
            "escape" => "Esc".into(),
            "pageup" => "PgUp".into(),
            "pagedown" => "PgDn".into(),
            "up" => "↑".into(),
            "down" => "↓".into(),
            "left" => "←".into(),
            "right" => "→".into(),
            "plus" => "+".into(),
            "minus" => "-".into(),
            "comma" => ",".into(),
            "period" => ".".into(),
            "slash" => "/".into(),
            "backslash" => "\\".into(),
            "semicolon" => ";".into(),
            "quote" => "'".into(),
            "backquote" => "`".into(),
            "bracketleft" => "[".into(),
            "bracketright" => "]".into(),
            k => {
                let mut ch = k.chars();
                match ch.next() {
                    Some(f) => f.to_ascii_uppercase().to_string() + ch.as_str(),
                    None => String::new(),
                }
            }
        };
        out.push_str(&k);
        out
    }
}

impl fmt::Display for KeyCombo {
    /// Canonical config form.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [(self.ctrl, "ctrl"), (self.alt, "alt"), (self.shift, "shift"), (self.win, "win")] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&self.key)
    }
}

impl std::str::FromStr for KeyCombo {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> crate::Result<Self> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_canonicalize() {
        let c = KeyCombo::parse(" Shift + CTRL + PrtSc ").unwrap();
        assert_eq!(c.to_string(), "ctrl+shift+printscreen");
        assert_eq!(c.pretty(), "Ctrl+Shift+PrtSc");
        assert_eq!(KeyCombo::parse("ctrl++").unwrap().to_string(), "ctrl+plus");
        assert_eq!(KeyCombo::parse("ctrl+=").unwrap().to_string(), "ctrl+plus");
        assert_eq!(KeyCombo::parse("]").unwrap().to_string(), "bracketright");
        assert_eq!(KeyCombo::parse("Esc").unwrap().to_string(), "escape");
        assert_eq!(KeyCombo::parse("f12").unwrap().pretty(), "F12");
    }

    #[test]
    fn rejects_bad() {
        assert!(KeyCombo::parse("ctrl+shift").is_err());
        assert!(KeyCombo::parse("ctrl+a+b").is_err());
        assert!(KeyCombo::parse("hyper+x").is_err());
        assert!(KeyCombo::parse("").is_err());
        assert!(KeyCombo::parse("f25").is_err());
    }
}
