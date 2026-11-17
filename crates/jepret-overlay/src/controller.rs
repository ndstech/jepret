//! Pure interaction controller: turns abstract input events into selection
//! changes and a final outcome. The Win32 layer only translates messages.
//!
//! Shortcuts (PRD F13, fixed): Enter/Space/double-click confirm; Esc/right
//! click cancel; arrows nudge 1 px (Shift 10); Ctrl+arrows resize 1 px
//! (Ctrl+Shift 10); Tab/Shift+Tab cycle snap windows; F = monitor under the
//! cursor; A = all monitors; M = magnifier; C = copy pixel colour; H = hint bar.

use jepret_core::{Point, Rect};

use crate::selection::{DragEnd, Selection};
use crate::snap::hit_test;

/// Finish as soon as the mouse button is released after a drag / window click
/// (fast path). Keyboard nudge/resize + Enter are meant for a preselected
/// (`initial`) rect, F/A/Tab selections, and adjusting while the button is held.
pub const CONFIRM_ON_RELEASE: bool = true;

/// Modifier keys held during a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
}

impl Mods {
    pub const NONE: Mods = Mods { shift: false, ctrl: false };
    pub const SHIFT: Mods = Mods { shift: true, ctrl: false };
    pub const CTRL: Mods = Mods { shift: false, ctrl: true };
    pub const CTRL_SHIFT: Mods = Mods { shift: true, ctrl: true };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    Enter,
    Space,
    Escape,
    Tab,
    /// `F`: whole monitor under the cursor.
    Monitor,
    /// `A`: all monitors.
    All,
    /// `M`: toggle magnifier.
    Magnifier,
    /// `C`: copy the colour under the cursor.
    CopyColor,
    /// `H`: toggle the hint bar.
    HintBar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing visible changed.
    Idle,
    /// Needs a repaint.
    Redraw,
    /// Overlay should close with this result.
    Done(Option<Rect>),
    /// Copy the hex colour of the pixel at this virtual-desktop point.
    CopyColor(Point),
}

/// `#RRGGBB` for an RGBA pixel.
pub fn hex_color(px: [u8; 4]) -> String {
    format!("#{:02X}{:02X}{:02X}", px[0], px[1], px[2])
}

/// Text for the bottom hint bar (two lines).
pub fn hint_bar_text() -> &'static str {
    "Enter/Space/2\u{d7}klik: OK  \u{b7}  Esc/klik kanan: batal  \u{b7}  Panah: geser 1 px (Shift 10)  \u{b7}  Ctrl+Panah: resize 1 px (Ctrl+Shift 10)\n\
     Tab/Shift+Tab: window  \u{b7}  F: monitor ini  \u{b7}  A: semua monitor  \u{b7}  M: magnifier  \u{b7}  C: salin warna  \u{b7}  H: sembunyikan bar"
}

pub struct Controller {
    sel: Selection,
    /// Window rects clipped to bounds, topmost first (also the Tab order).
    windows: Vec<Rect>,
    monitors: Vec<Rect>,
    hover: Option<Rect>,
    cursor: Option<Point>,
    magnifier: bool,
    hint_bar: bool,
    notice: Option<String>,
}

impl Controller {
    pub fn new(bounds: Rect, initial: Option<Rect>, windows: Vec<Rect>, monitors: Vec<Rect>, magnifier: bool) -> Self {
        let windows = windows
            .iter()
            .filter(|r| !r.is_empty())
            .filter_map(|r| r.intersect(&bounds))
            .collect();
        Self {
            sel: Selection::new(bounds, initial),
            windows,
            monitors,
            hover: None,
            cursor: None,
            magnifier,
            hint_bar: true,
            notice: None,
        }
    }

    pub fn bounds(&self) -> Rect {
        self.sel.bounds()
    }

    pub fn selection(&self) -> Option<Rect> {
        self.sel.rect()
    }

    pub fn cursor(&self) -> Option<Point> {
        self.cursor
    }

    pub fn magnifier_on(&self) -> bool {
        self.magnifier
    }

    pub fn hint_bar_on(&self) -> bool {
        self.hint_bar
    }

    /// Transient message (e.g. "Warna #FF00AA disalin"); cleared on next input.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn set_notice(&mut self, text: impl Into<String>) {
        self.notice = Some(text.into());
    }

    /// Rect to highlight: the selection, else the hover-snap candidate.
    pub fn shown_rect(&self) -> Option<Rect> {
        if self.sel.is_drag_active() {
            self.sel.rect()
        } else {
            self.sel.rect().or(self.hover)
        }
    }

    /// True while a hover candidate (not a real selection) is what is shown.
    pub fn showing_hover(&self) -> bool {
        !self.sel.is_drag_active() && self.sel.rect().is_none() && self.hover.is_some()
    }

    fn hit(&self, p: Point) -> Option<Rect> {
        hit_test(&self.windows, self.sel.bounds(), p)
    }

    pub fn mouse_move(&mut self, p: Point) -> Outcome {
        let p = self.sel.clamp_point(p);
        self.cursor = Some(p);
        if self.sel.is_dragging() {
            self.sel.update_drag(p);
            self.hover = None;
        } else {
            self.hover = self.hit(p);
        }
        Outcome::Redraw
    }

    pub fn mouse_down(&mut self, p: Point) -> Outcome {
        let p = self.sel.clamp_point(p);
        self.cursor = Some(p);
        self.notice = None;
        self.sel.begin_drag(p);
        Outcome::Redraw
    }

    pub fn mouse_up(&mut self, p: Point) -> Outcome {
        let p = self.sel.clamp_point(p);
        self.cursor = Some(p);
        match self.sel.end_drag(p) {
            DragEnd::Selected(r) => {
                if CONFIRM_ON_RELEASE {
                    Outcome::Done(Some(r))
                } else {
                    Outcome::Redraw
                }
            }
            DragEnd::Click(p) => match self.hit(p) {
                Some(w) => {
                    self.sel.set_rect(w);
                    if CONFIRM_ON_RELEASE {
                        Outcome::Done(self.sel.rect())
                    } else {
                        Outcome::Redraw
                    }
                }
                None => Outcome::Redraw,
            },
            DragEnd::None => Outcome::Idle,
        }
    }

    /// Double-click confirms the current selection (or the hovered window).
    pub fn double_click(&mut self, p: Point) -> Outcome {
        let p = self.sel.clamp_point(p);
        self.cursor = Some(p);
        match self.sel.rect().or_else(|| self.hit(p)) {
            Some(r) => Outcome::Done(Some(r)),
            None => Outcome::Idle,
        }
    }

    pub fn right_click(&mut self) -> Outcome {
        Outcome::Done(None)
    }

    fn confirm(&self) -> Outcome {
        match self.sel.rect().or(self.hover) {
            Some(r) => Outcome::Done(Some(r)),
            None => Outcome::Idle,
        }
    }

    fn cycle_windows(&mut self, back: bool) -> Outcome {
        let n = self.windows.len();
        if n == 0 {
            return Outcome::Idle;
        }
        let cur = self.sel.rect().or(self.hover);
        let idx = cur.and_then(|c| self.windows.iter().position(|w| *w == c));
        let next = match (idx, back) {
            (Some(i), false) => (i + 1) % n,
            (Some(i), true) => (i + n - 1) % n,
            (None, false) => 0,
            (None, true) => n - 1,
        };
        self.sel.set_rect(self.windows[next]);
        Outcome::Redraw
    }

    fn monitor_under_cursor(&self) -> Rect {
        let b = self.sel.bounds();
        let m = self
            .cursor
            .and_then(|c| self.monitors.iter().find(|m| m.contains(c)))
            .or_else(|| self.monitors.first())
            .copied()
            .unwrap_or(b);
        m.intersect(&b).unwrap_or(b)
    }

    fn all_monitors(&self) -> Rect {
        let b = self.sel.bounds();
        let u = self.monitors.iter().fold(Rect::default(), |a, m| a.union(m));
        if u.is_empty() {
            b
        } else {
            u.intersect(&b).unwrap_or(b)
        }
    }

    pub fn key(&mut self, key: Key, mods: Mods) -> Outcome {
        self.notice = None;
        let step = if mods.shift { 10 } else { 1 };
        let (dx, dy) = match key {
            Key::Left => (-step, 0),
            Key::Right => (step, 0),
            Key::Up => (0, -step),
            Key::Down => (0, step),
            Key::Escape => return Outcome::Done(None),
            Key::Enter | Key::Space => return self.confirm(),
            Key::Tab => return self.cycle_windows(mods.shift),
            Key::Monitor => {
                let m = self.monitor_under_cursor();
                self.sel.set_rect(m);
                return Outcome::Redraw;
            }
            Key::All => {
                let m = self.all_monitors();
                self.sel.set_rect(m);
                return Outcome::Redraw;
            }
            Key::Magnifier => {
                self.magnifier = !self.magnifier;
                return Outcome::Redraw;
            }
            Key::HintBar => {
                self.hint_bar = !self.hint_bar;
                return Outcome::Redraw;
            }
            Key::CopyColor => {
                return match self.cursor {
                    Some(p) => Outcome::CopyColor(p),
                    None => Outcome::Idle,
                };
            }
        };
        let changed = if mods.ctrl { self.sel.resize(dx, dy) } else { self.sel.nudge(dx, dy) };
        if changed {
            Outcome::Redraw
        } else {
            Outcome::Idle
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: Rect = Rect::new(-100, 0, 1000, 600);
    const MON_A: Rect = Rect::new(-100, 0, 400, 600);
    const MON_B: Rect = Rect::new(300, 0, 600, 600);
    const N: Mods = Mods::NONE;

    fn ctl(initial: Option<Rect>, windows: Vec<Rect>) -> Controller {
        Controller::new(B, initial, windows, vec![MON_A, MON_B], true)
    }

    #[test]
    fn drag_release_confirms_inclusive_rect() {
        let mut c = ctl(None, vec![]);
        c.mouse_down(Point::new(10, 10));
        assert_eq!(c.mouse_move(Point::new(60, 40)), Outcome::Redraw);
        assert_eq!(c.shown_rect(), Some(Rect::new(10, 10, 51, 31)));
        assert_eq!(c.mouse_up(Point::new(60, 40)), Outcome::Done(Some(Rect::new(10, 10, 51, 31))));
    }

    #[test]
    fn hover_snaps_and_click_selects_window() {
        let w = Rect::new(0, 0, 200, 100);
        let mut c = ctl(None, vec![w]);
        c.mouse_move(Point::new(50, 50));
        assert_eq!(c.shown_rect(), Some(w));
        assert!(c.showing_hover());
        c.mouse_down(Point::new(50, 50));
        assert_eq!(c.mouse_up(Point::new(51, 50)), Outcome::Done(Some(w)));
    }

    #[test]
    fn click_on_nothing_does_not_finish() {
        let mut c = ctl(None, vec![Rect::new(0, 0, 10, 10)]);
        c.mouse_down(Point::new(500, 500));
        assert_eq!(c.mouse_up(Point::new(500, 500)), Outcome::Redraw);
        assert_eq!(c.selection(), None);
    }

    #[test]
    fn hover_disappears_while_dragging() {
        let mut c = ctl(None, vec![Rect::new(0, 0, 300, 300)]);
        c.mouse_move(Point::new(20, 20));
        c.mouse_down(Point::new(20, 20));
        c.mouse_move(Point::new(120, 120));
        assert_eq!(c.shown_rect(), Some(Rect::new(20, 20, 101, 101)));
        assert!(!c.showing_hover());
    }

    #[test]
    fn arrows_nudge_1_and_10() {
        let mut c = ctl(Some(Rect::new(10, 10, 100, 50)), vec![]);
        assert_eq!(c.key(Key::Right, N), Outcome::Redraw);
        assert_eq!(c.key(Key::Down, N), Outcome::Redraw);
        assert_eq!(c.selection(), Some(Rect::new(11, 11, 100, 50)));
        c.key(Key::Right, Mods::SHIFT);
        c.key(Key::Up, Mods::SHIFT);
        assert_eq!(c.selection(), Some(Rect::new(21, 1, 100, 50)));
    }

    #[test]
    fn ctrl_arrows_resize_1_and_10() {
        let mut c = ctl(Some(Rect::new(10, 10, 100, 50)), vec![]);
        c.key(Key::Right, Mods::CTRL);
        c.key(Key::Up, Mods::CTRL);
        assert_eq!(c.selection(), Some(Rect::new(10, 10, 101, 49)));
        c.key(Key::Right, Mods::CTRL_SHIFT);
        c.key(Key::Down, Mods::CTRL_SHIFT);
        assert_eq!(c.selection(), Some(Rect::new(10, 10, 111, 59)));
        c.key(Key::Left, Mods::CTRL_SHIFT);
        assert_eq!(c.selection(), Some(Rect::new(10, 10, 101, 59)));
    }

    #[test]
    fn nudge_clamps_at_edge_and_is_idle() {
        let mut c = ctl(Some(Rect::new(-100, 0, 10, 10)), vec![]);
        assert_eq!(c.key(Key::Left, N), Outcome::Idle);
        assert_eq!(c.key(Key::Up, Mods::SHIFT), Outcome::Idle);
        // shift 10 px near the edge moves only what fits
        let mut c = ctl(Some(Rect::new(-95, 0, 10, 10)), vec![]);
        assert_eq!(c.key(Key::Left, Mods::SHIFT), Outcome::Redraw);
        assert_eq!(c.selection(), Some(Rect::new(-100, 0, 10, 10)));
    }

    #[test]
    fn confirm_and_cancel_keys() {
        let mut c = ctl(Some(Rect::new(0, 0, 10, 10)), vec![]);
        assert_eq!(c.key(Key::Enter, N), Outcome::Done(Some(Rect::new(0, 0, 10, 10))));
        assert_eq!(c.key(Key::Space, N), Outcome::Done(Some(Rect::new(0, 0, 10, 10))));
        assert_eq!(c.key(Key::Escape, N), Outcome::Done(None));
        assert_eq!(c.right_click(), Outcome::Done(None));
    }

    #[test]
    fn double_click_confirms_selection_or_hover() {
        let mut c = ctl(Some(Rect::new(0, 0, 10, 10)), vec![]);
        assert_eq!(c.double_click(Point::new(300, 300)), Outcome::Done(Some(Rect::new(0, 0, 10, 10))));
        let w = Rect::new(100, 100, 50, 50);
        let mut c = ctl(None, vec![w]);
        assert_eq!(c.double_click(Point::new(110, 110)), Outcome::Done(Some(w)));
        assert_eq!(c.double_click(Point::new(500, 500)), Outcome::Idle);
    }

    #[test]
    fn enter_without_anything_is_idle_and_with_hover_confirms_it() {
        let w = Rect::new(0, 0, 50, 50);
        let mut c = ctl(None, vec![w]);
        assert_eq!(c.key(Key::Enter, N), Outcome::Idle);
        c.mouse_move(Point::new(5, 5));
        assert_eq!(c.key(Key::Enter, N), Outcome::Done(Some(w)));
    }

    #[test]
    fn tab_cycles_windows_forward_and_back() {
        let (a, b, d) = (Rect::new(0, 0, 100, 100), Rect::new(200, 0, 100, 100), Rect::new(400, 0, 100, 100));
        let mut c = ctl(None, vec![a, b, d]);
        assert_eq!(c.key(Key::Tab, N), Outcome::Redraw);
        assert_eq!(c.selection(), Some(a));
        c.key(Key::Tab, N);
        assert_eq!(c.selection(), Some(b));
        c.key(Key::Tab, N);
        c.key(Key::Tab, N);
        assert_eq!(c.selection(), Some(a)); // wraps
        c.key(Key::Tab, Mods::SHIFT);
        assert_eq!(c.selection(), Some(d)); // back-wraps
    }

    #[test]
    fn tab_starts_from_hover_and_shift_tab_from_end() {
        let (a, b) = (Rect::new(0, 0, 100, 100), Rect::new(200, 0, 100, 100));
        let mut c = ctl(None, vec![a, b]);
        c.mouse_move(Point::new(10, 10));
        c.key(Key::Tab, N);
        assert_eq!(c.selection(), Some(b));
        let mut c = ctl(None, vec![a, b]);
        c.key(Key::Tab, Mods::SHIFT);
        assert_eq!(c.selection(), Some(b));
    }

    #[test]
    fn tab_without_windows_is_idle() {
        let mut c = ctl(None, vec![]);
        assert_eq!(c.key(Key::Tab, N), Outcome::Idle);
    }

    #[test]
    fn f_selects_monitor_under_cursor_and_a_selects_all() {
        let mut c = ctl(None, vec![]);
        c.mouse_move(Point::new(500, 100));
        c.key(Key::Monitor, N);
        assert_eq!(c.selection(), Some(MON_B));
        c.mouse_move(Point::new(0, 100));
        c.key(Key::Monitor, N);
        assert_eq!(c.selection(), Some(MON_A));
        c.key(Key::All, N);
        assert_eq!(c.selection(), Some(B));
        // result can then be confirmed with Enter
        assert_eq!(c.key(Key::Enter, N), Outcome::Done(Some(B)));
    }

    #[test]
    fn f_without_cursor_uses_first_monitor() {
        let mut c = ctl(None, vec![]);
        c.key(Key::Monitor, N);
        assert_eq!(c.selection(), Some(MON_A));
    }

    #[test]
    fn toggles() {
        let mut c = ctl(None, vec![]);
        assert!(c.magnifier_on() && c.hint_bar_on());
        c.key(Key::Magnifier, N);
        c.key(Key::HintBar, N);
        assert!(!c.magnifier_on() && !c.hint_bar_on());
        c.key(Key::Magnifier, N);
        assert!(c.magnifier_on());
    }

    #[test]
    fn copy_color_needs_cursor_and_notice_clears_on_next_key() {
        let mut c = ctl(None, vec![]);
        assert_eq!(c.key(Key::CopyColor, N), Outcome::Idle);
        c.mouse_move(Point::new(7, 9));
        assert_eq!(c.key(Key::CopyColor, N), Outcome::CopyColor(Point::new(7, 9)));
        c.set_notice("ok");
        assert_eq!(c.notice(), Some("ok"));
        c.key(Key::Magnifier, N);
        assert_eq!(c.notice(), None);
    }

    #[test]
    fn hex_format() {
        assert_eq!(hex_color([255, 0, 171, 255]), "#FF00AB");
        assert_eq!(hex_color([1, 2, 3, 0]), "#010203");
    }

    #[test]
    fn initial_is_shown_and_cursor_clamped() {
        let mut c = ctl(Some(Rect::new(5, 5, 20, 20)), vec![]);
        assert_eq!(c.shown_rect(), Some(Rect::new(5, 5, 20, 20)));
        c.mouse_move(Point::new(100_000, -5));
        assert_eq!(c.cursor(), Some(Point::new(899, 0)));
    }
}
