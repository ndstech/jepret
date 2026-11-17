//! Hotkey string parser: `"ctrl+shift+4"`, `"alt+printscreen"`, `"printscreen"`.
//!
//! Pure (no Win32 calls) so it is unit-testable. Modifier bit values equal
//! Win32 `MOD_*`, key values equal Win32 virtual-key codes.

pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: u32,
    pub vk: u32,
}

/// Parse a hotkey string via [`jepret_core::KeyCombo`] (lenient, case-insensitive).
pub fn parse(s: &str) -> Result<Hotkey, String> {
    let combo = jepret_core::KeyCombo::parse(s).map_err(|e| format!("{e:#}"))?;
    from_combo(&combo)
}

pub fn from_combo(c: &jepret_core::KeyCombo) -> Result<Hotkey, String> {
    let mut modifiers = 0;
    for (on, m) in [(c.ctrl, MOD_CONTROL), (c.alt, MOD_ALT), (c.shift, MOD_SHIFT), (c.win, MOD_WIN)] {
        if on {
            modifiers |= m;
        }
    }
    let vk = key_code(&c.key).ok_or_else(|| format!("no virtual-key mapping for {:?}", c.key))?;
    Ok(Hotkey { modifiers, vk })
}

fn key_code(name: &str) -> Option<u32> {
    let b = name.as_bytes();
    if b.len() == 1 {
        match b[0] {
            c @ b'a'..=b'z' => return Some(c.to_ascii_uppercase() as u32),
            c @ b'0'..=b'9' => return Some(c as u32),
            _ => {}
        }
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    }
    if let Some(n) = name.strip_prefix("num").and_then(|n| n.parse::<u32>().ok()) {
        if n <= 9 {
            return Some(0x60 + n);
        }
    }
    Some(match name {
        "printscreen" => 0x2C,
        "scrolllock" => 0x91,
        "pause" => 0x13,
        "backspace" => 0x08,
        "tab" => 0x09,
        "enter" => 0x0D,
        "escape" => 0x1B,
        "space" => 0x20,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "end" => 0x23,
        "home" => 0x24,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "insert" => 0x2D,
        "delete" => 0x2E,
        "numadd" => 0x6B,
        "numsubtract" => 0x6D,
        "nummultiply" => 0x6A,
        "numdivide" => 0x6F,
        "numdecimal" => 0x6E,
        "plus" => 0xBB,
        "minus" => 0xBD,
        "comma" => 0xBC,
        "period" => 0xBE,
        "semicolon" => 0xBA,
        "slash" => 0xBF,
        "backquote" => 0xC0,
        "bracketleft" => 0xDB,
        "bracketright" => 0xDD,
        "backslash" => 0xDC,
        "quote" => 0xDE,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printscreen_with_every_modifier_combo() {
        let cases = [
            ("printscreen", 0),
            ("ctrl+printscreen", MOD_CONTROL),
            ("alt+printscreen", MOD_ALT),
            ("shift+printscreen", MOD_SHIFT),
            ("ctrl+shift+printscreen", MOD_CONTROL | MOD_SHIFT),
            ("ctrl+alt+printscreen", MOD_CONTROL | MOD_ALT),
            ("alt+shift+printscreen", MOD_ALT | MOD_SHIFT),
            ("PrtSc", 0),
        ];
        for (s, m) in cases {
            assert_eq!(parse(s), Ok(Hotkey { modifiers: m, vk: 0x2C }), "{s}");
        }
    }

    #[test]
    fn parses_defaults() {
        assert_eq!(parse("printscreen"), Ok(Hotkey { modifiers: 0, vk: 0x2C }));
        assert_eq!(parse("alt+printscreen"), Ok(Hotkey { modifiers: MOD_ALT, vk: 0x2C }));
        assert_eq!(
            parse("ctrl+shift+4"),
            Ok(Hotkey { modifiers: MOD_CONTROL | MOD_SHIFT, vk: b'4' as u32 })
        );
    }

    #[test]
    fn case_space_and_order_insensitive() {
        assert_eq!(parse("Shift + CTRL + A"), parse("ctrl+shift+a"));
        assert_eq!(parse("ctrl+shift+a").unwrap().vk, b'A' as u32);
    }

    #[test]
    fn function_and_named_keys() {
        assert_eq!(parse("win+f12").unwrap(), Hotkey { modifiers: MOD_WIN, vk: 0x7B });
        assert_eq!(parse("f1").unwrap().vk, 0x70);
        assert_eq!(parse("ctrl+space").unwrap().vk, 0x20);
        assert_eq!(parse("ctrl+num5").unwrap().vk, 0x65);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("").is_err());
        assert!(parse("ctrl+shift").is_err());
        assert_eq!(parse("ctrl+=").unwrap().vk, 0xBB);
        assert!(parse("a+b").is_err());
        assert!(parse("ctrl+blah").is_err());
        assert!(parse("f25").is_err());
    }
}
