//! Key-combo recorder based on a Win32 low-level keyboard hook
//! (`WH_KEYBOARD_LL`). Needed because egui/winit never reports PrintScreen
//! or the Win key, and Windows would otherwise act on them.
//!
//! The hook only exists while a [`Recorder`] value is alive (installed on the
//! calling/UI thread, which must keep pumping messages — eframe does). While
//! active it swallows every (non-injected) key event so the Win key does not
//! open Start and PrtSc does not trigger Snipping Tool. It is removed on drop
//! and auto-expires after [`TIMEOUT`] so the user can never be stuck.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use windows::Win32::{
    Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
        CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, WH_KEYBOARD_LL,
        WM_KEYDOWN, WM_SYSKEYDOWN,
    },
};

use crate::hotkey;
use jepret_core::KeyCombo;

pub const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Combo(KeyCombo),
    /// Plain Esc pressed.
    Cancelled,
    /// A key we cannot represent; keep recording.
    Unsupported(String),
}

/// Pure state machine behind the hook (unit-tested without Win32).
#[derive(Debug, Default)]
pub struct Tracker {
    ctrl: bool,
    alt: bool,
    shift: bool,
    win: bool,
    pub outcome: Option<Outcome>,
}

const VK_ESCAPE: u32 = 0x1B;

impl Tracker {
    pub fn on_key(&mut self, vk: u32, down: bool) {
        match vk {
            0x11 | 0xA2 | 0xA3 => self.ctrl = down,
            0x12 | 0xA4 | 0xA5 => self.alt = down,
            0x10 | 0xA0 | 0xA1 => self.shift = down,
            0x5B | 0x5C => self.win = down,
            _ if !down || self.outcome.is_some() => {}
            VK_ESCAPE if !(self.ctrl || self.alt || self.shift || self.win) => {
                self.outcome = Some(Outcome::Cancelled);
            }
            _ => {
                self.outcome = Some(match hotkey::combo_from_vk(vk, self.ctrl, self.alt, self.shift, self.win) {
                    Ok(c) => Outcome::Combo(c),
                    Err(e) => Outcome::Unsupported(e),
                });
            }
        }
    }
}

static STATE: Mutex<Option<Tracker>> = Mutex::new(None);

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: for WH_KEYBOARD_LL with code >= 0, lparam points to a valid KBDLLHOOKSTRUCT.
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let injected = kb.flags.0 & 0x10 != 0; // LLKHF_INJECTED
        if !injected {
            let m = wparam.0 as u32;
            let down = m == WM_KEYDOWN || m == WM_SYSKEYDOWN;
            if let Ok(mut g) = STATE.lock() {
                if let Some(t) = g.as_mut() {
                    t.on_key(kb.vkCode, down);
                }
            }
            return LRESULT(1); // swallow
        }
    }
    // SAFETY: plain forwarding of the hook arguments.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

pub struct Recorder {
    hook: HHOOK,
    started: Instant,
}

impl Recorder {
    /// Install the hook. `None` if it could not be installed (caller falls back).
    pub fn start() -> Option<Self> {
        let mut g = STATE.lock().ok()?;
        if g.is_some() {
            return None; // already recording
        }
        // SAFETY: hook_proc is a valid HOOKPROC and never unwinds; the hook is removed in Drop.
        let hook = unsafe {
            let hm = GetModuleHandleW(None).ok()?;
            SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), HINSTANCE(hm.0), 0).ok()?
        };
        *g = Some(Tracker::default());
        Some(Self { hook, started: Instant::now() })
    }

    /// Final result, if any. Returns `Cancelled` on timeout. `Unsupported`
    /// is reported once and recording continues.
    pub fn poll(&self) -> Option<Outcome> {
        if self.started.elapsed() > TIMEOUT {
            return Some(Outcome::Cancelled);
        }
        let mut g = STATE.lock().ok()?;
        let t = g.as_mut()?;
        t.outcome.take()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        // SAFETY: `hook` was returned by SetWindowsHookExW and is unhooked exactly once.
        unsafe {
            let _ = UnhookWindowsHookEx(self.hook);
        }
        if let Ok(mut g) = STATE.lock() {
            *g = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(t: &mut Tracker, seq: &[(u32, bool)]) {
        for &(vk, down) in seq {
            t.on_key(vk, down);
        }
    }

    #[test]
    fn modifiers_then_key() {
        let mut t = Tracker::default();
        feed(&mut t, &[(0xA2, true), (0xA0, true), (0x2C, true)]);
        assert_eq!(t.outcome, Some(Outcome::Combo(hotkey::parse("ctrl+shift+printscreen").unwrap())));
    }

    #[test]
    fn bare_printscreen_and_win_combo() {
        let mut t = Tracker::default();
        feed(&mut t, &[(0x2C, true)]);
        assert_eq!(t.outcome, Some(Outcome::Combo(KeyCombo::key("printscreen"))));
        let mut t = Tracker::default();
        feed(&mut t, &[(0x5B, true), (0xA0, true), (0x53, true)]);
        assert_eq!(t.outcome, Some(Outcome::Combo(hotkey::parse("shift+win+s").unwrap())));
    }

    #[test]
    fn escape_cancels_only_when_bare_and_released_modifiers_clear() {
        let mut t = Tracker::default();
        feed(&mut t, &[(0x1B, true)]);
        assert_eq!(t.outcome, Some(Outcome::Cancelled));
        let mut t = Tracker::default();
        feed(&mut t, &[(0xA2, true), (0xA2, false), (0x1B, true)]);
        assert_eq!(t.outcome, Some(Outcome::Cancelled));
        let mut t = Tracker::default();
        feed(&mut t, &[(0xA2, true), (0x1B, true)]);
        assert_eq!(t.outcome, Some(Outcome::Combo(hotkey::parse("ctrl+escape").unwrap())));
    }

    #[test]
    fn modifier_only_and_keyup_do_not_finish() {
        let mut t = Tracker::default();
        feed(&mut t, &[(0xA2, true), (0xA0, true), (0xA0, false), (0x41, false)]);
        assert_eq!(t.outcome, None);
    }

    #[test]
    fn unknown_key_is_unsupported() {
        let mut t = Tracker::default();
        feed(&mut t, &[(0xE8, true)]);
        assert!(matches!(t.outcome, Some(Outcome::Unsupported(_))));
    }
}
