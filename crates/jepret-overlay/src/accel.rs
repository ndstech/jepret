//! Pure key-hold acceleration for the keyboard selection mode. Driven only by
//! the key-down messages Windows already sends (`is_repeat` = lParam bit 30,
//! `time_ms` = `GetMessageTime()`); no timers.
//!
//! Step table, measured from the first press of the current hold:
//! fresh press 1 px; repeats < 0.3 s 0 px (initial-delay guard); 0.3-0.8 s
//! 1 px; 0.8-1.5 s ramp 1 -> 4 px; 1.5-3.0 s ramp 4 -> 20 px; then 20 px.
//! Shift = fixed 10 px, Ctrl+Shift = fixed 50 px (no ramp).

use crate::controller::{Key, Mods};

pub const REPEAT_GUARD_MS: u32 = 300;
pub const SLOW_END_MS: u32 = 800;
pub const MID_END_MS: u32 = 1500;
pub const CAP_MS: u32 = 3000;
pub const MAX_STEP: i32 = 20;
pub const SHIFT_STEP: i32 = 10;
pub const CTRL_SHIFT_STEP: i32 = 50;

/// Step in px for a repeat `held_ms` after the first press of the hold.
pub fn ramp_step(held_ms: u32) -> i32 {
    if held_ms < REPEAT_GUARD_MS {
        0
    } else if held_ms < SLOW_END_MS {
        1
    } else if held_ms < MID_END_MS {
        1 + (3 * (held_ms - SLOW_END_MS) / (MID_END_MS - SLOW_END_MS)) as i32
    } else if held_ms < CAP_MS {
        4 + (16 * (held_ms - MID_END_MS) / (CAP_MS - MID_END_MS)) as i32
    } else {
        MAX_STEP
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct KeyAccel {
    /// Key and timestamp of the first press of the current hold.
    hold: Option<(Key, u32)>,
}

impl KeyAccel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget the current hold.
    pub fn reset(&mut self) {
        self.hold = None;
    }

    /// Key released (WM_KEYUP): resets only if it is the key being tracked.
    pub fn key_up(&mut self, key: Key) {
        if self.hold.is_some_and(|(k, _)| k == key) {
            self.hold = None;
        }
    }

    /// Step in px (0 = ignore this event) for a key-down event.
    pub fn step(&mut self, key: Key, is_repeat: bool, time_ms: u32, mods: Mods) -> i32 {
        let continuing = is_repeat && self.hold.is_some_and(|(k, _)| k == key);
        if !continuing {
            // Fresh press, or the direction changed: a new hold starts here.
            self.hold = Some((key, time_ms));
        }
        match (mods.shift, mods.ctrl) {
            (true, true) => return CTRL_SHIFT_STEP,
            (true, false) => return SHIFT_STEP,
            _ => {}
        }
        if !continuing {
            return 1;
        }
        let start = self.hold.map_or(time_ms, |(_, t)| t);
        // wrapping_sub: GetMessageTime wraps every ~49.7 days.
        ramp_step(time_ms.wrapping_sub(start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: Mods = Mods::NONE;

    #[test]
    fn fresh_press_is_exactly_one() {
        let mut a = KeyAccel::new();
        assert_eq!(a.step(Key::Left, false, 1000, N), 1);
        a.key_up(Key::Left);
        assert_eq!(a.step(Key::Left, false, 1010, N), 1);
    }

    #[test]
    fn early_repeats_are_ignored() {
        let mut a = KeyAccel::new();
        a.step(Key::Right, false, 0, N);
        assert_eq!(a.step(Key::Right, true, 250, N), 0);
        assert_eq!(a.step(Key::Right, true, 299, N), 0);
        assert_eq!(a.step(Key::Right, true, 300, N), 1);
        assert_eq!(a.step(Key::Right, true, 799, N), 1);
    }

    #[test]
    fn ramp_and_cap() {
        let mut a = KeyAccel::new();
        a.step(Key::Down, false, 5000, N);
        assert_eq!(a.step(Key::Down, true, 5800, N), 1);
        assert_eq!(a.step(Key::Down, true, 5000 + 1150, N), 2); // halfway 0.8-1.5 s
        assert_eq!(a.step(Key::Down, true, 5000 + 1499, N), 3);
        assert_eq!(a.step(Key::Down, true, 5000 + 1500, N), 4);
        assert_eq!(a.step(Key::Down, true, 5000 + 2250, N), 12);
        assert_eq!(a.step(Key::Down, true, 5000 + 3000, N), 20);
        assert_eq!(a.step(Key::Down, true, 5000 + 60_000, N), 20);
        let mut last = 0;
        for t in (300..4000).step_by(10) {
            let s = ramp_step(t);
            assert!(s >= last && s <= MAX_STEP);
            last = s;
        }
    }

    #[test]
    fn key_up_resets_hold() {
        let mut a = KeyAccel::new();
        a.step(Key::Up, false, 0, N);
        assert_eq!(a.step(Key::Up, true, 2000, N), 9);
        a.key_up(Key::Up);
        // a stray repeat after release restarts the hold: 1 px, then guard again
        assert_eq!(a.step(Key::Up, true, 2030, N), 1);
        assert_eq!(a.step(Key::Up, true, 2100, N), 0);
        a.reset();
        assert_eq!(a.step(Key::Up, false, 2200, N), 1);
        assert_eq!(a.step(Key::Up, true, 2300, N), 0);
    }

    #[test]
    fn key_up_of_other_key_keeps_hold() {
        let mut a = KeyAccel::new();
        a.step(Key::Up, false, 0, N);
        a.key_up(Key::Left);
        assert_eq!(a.step(Key::Up, true, 1500, N), 4);
    }

    #[test]
    fn direction_change_resets() {
        let mut a = KeyAccel::new();
        a.step(Key::Left, false, 0, N);
        assert_eq!(a.step(Key::Left, true, 3500, N), 20);
        assert_eq!(a.step(Key::Up, false, 3600, N), 1);
        assert_eq!(a.step(Key::Up, true, 3700, N), 0);
        // even a (spurious) repeat flag with a different key restarts
        assert_eq!(a.step(Key::Right, true, 3800, N), 1);
    }

    #[test]
    fn shift_is_fixed() {
        let mut a = KeyAccel::new();
        assert_eq!(a.step(Key::Left, false, 0, Mods::SHIFT), 10);
        assert_eq!(a.step(Key::Left, true, 50, Mods::SHIFT), 10);
        assert_eq!(a.step(Key::Left, true, 5000, Mods::SHIFT), 10);
        assert_eq!(a.step(Key::Left, false, 6000, Mods::CTRL_SHIFT), 50);
        assert_eq!(a.step(Key::Left, true, 9000, Mods::CTRL_SHIFT), 50);
    }

    #[test]
    fn ctrl_alone_uses_ramp() {
        let mut a = KeyAccel::new();
        assert_eq!(a.step(Key::Right, false, 0, Mods::CTRL), 1);
        assert_eq!(a.step(Key::Right, true, 100, Mods::CTRL), 0);
        assert_eq!(a.step(Key::Right, true, 500, Mods::CTRL), 1);
    }

    #[test]
    fn time_wraps_safely() {
        let mut a = KeyAccel::new();
        let start = u32::MAX - 100;
        a.step(Key::Down, false, start, N);
        // 150 ms later the counter wrapped past zero
        assert_eq!(a.step(Key::Down, true, start.wrapping_add(150), N), 0);
        assert_eq!(a.step(Key::Down, true, start.wrapping_add(400), N), 1);
        assert_eq!(a.step(Key::Down, true, start.wrapping_add(1500), N), 4);
    }
}
