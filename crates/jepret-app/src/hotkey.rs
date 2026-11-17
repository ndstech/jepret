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

/// Parse a hotkey string (case-insensitive, parts separated by `+`).
pub fn parse(s: &str) -> Result<Hotkey, String> {
    let mut modifiers = 0u32;
    let mut vk: Option<u32> = None;
    for raw in s.split('+') {
        let part = raw.trim().to_ascii_lowercase();
        if part.is_empty() {
            return Err(format!("empty part in hotkey {s:?}"));
        }
        let m = match part.as_str() {
            "ctrl" | "control" | "ctl" => MOD_CONTROL,
            "shift" => MOD_SHIFT,
            "alt" => MOD_ALT,
            "win" | "windows" | "meta" | "super" | "cmd" => MOD_WIN,
            _ => 0,
        };
        if m != 0 {
            modifiers |= m;
            continue;
        }
        let code = key_code(&part).ok_or_else(|| format!("unknown key {raw:?} in hotkey {s:?}"))?;
        if vk.replace(code).is_some() {
            return Err(format!("more than one non-modifier key in hotkey {s:?}"));
        }
    }
    let vk = vk.ok_or_else(|| format!("no key in hotkey {s:?}"))?;
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
    if let Some(n) = name.strip_prefix("numpad").and_then(|n| n.parse::<u32>().ok()) {
        if n <= 9 {
            return Some(0x60 + n);
        }
    }
    Some(match name {
        "printscreen" | "prtsc" | "prtscn" | "print" | "snapshot" => 0x2C,
        "scrolllock" => 0x91,
        "pause" | "break" => 0x13,
        "backspace" => 0x08,
        "tab" => 0x09,
        "enter" | "return" => 0x0D,
        "esc" | "escape" => 0x1B,
        "space" => 0x20,
        "pageup" | "pgup" => 0x21,
        "pagedown" | "pgdn" => 0x22,
        "end" => 0x23,
        "home" => 0x24,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "insert" | "ins" => 0x2D,
        "delete" | "del" => 0x2E,
        "numlock" => 0x90,
        "-" | "minus" => 0xBD,
        "=" | "equals" | "plus_key" => 0xBB,
        "," | "comma" => 0xBC,
        "." | "period" => 0xBE,
        ";" | "semicolon" => 0xBA,
        "/" | "slash" => 0xBF,
        "`" | "backtick" => 0xC0,
        "[" => 0xDB,
        "]" => 0xDD,
        "\\" => 0xDC,
        "'" | "quote" => 0xDE,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(parse("ctrl+numpad5").unwrap().vk, 0x65);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("").is_err());
        assert!(parse("ctrl+shift").is_err());
        assert!(parse("ctrl++a").is_err());
        assert!(parse("a+b").is_err());
        assert!(parse("ctrl+blah").is_err());
        assert!(parse("f25").is_err());
    }
}
