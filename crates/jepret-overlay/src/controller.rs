//! Pure interaction controller: turns abstract input events into selection
//! changes and a final outcome. The Win32 layer only translates messages.
//!
//! Shortcuts (PRD F13, fixed): Enter/Space/double-click confirm; Esc/right
//! click cancel; Tab/Shift+Tab cycle snap windows; F = monitor under the
//! cursor; A = all monitors; M = magnifier; C = copy pixel colour; H = hint bar.
//!
//! Arrows, step from [`crate::accel`] (1 px, hold = faster, Shift 10,
//! Ctrl+Shift 50):
//! - no selection: move the mouse cursor ([`Outcome::MoveCursor`]);
//! - with a selection: nudge it, Ctrl+arrows resize the bottom-right edge;
//! - keyboard selection mode (`R`): `R` anchors at the cursor, arrows move the
//!   active corner (the cursor follows), Tab toggles extend/move, `R` again
//!   re-anchors, Enter/Space confirm; a mouse press leaves the mode.

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
    /// `R`: start keyboard selection (anchor at the cursor) / re-anchor.
    Anchor,
}

/// What the arrows do in keyboard selection mode (toggled with Tab).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KbMode {
    /// Move the active corner ("perluas").
    Extend,
    /// Move the whole selection ("geser").
    Move,
}

/// Interaction stage, used to pick the hint bar content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    NoSelection,
    Keyboard,
    Selected,
}

#[derive(Debug, Clone, Copy)]
struct KbSel {
    anchor: Point,
    corner: Point,
    mode: KbMode,
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
    /// Move the real mouse cursor to this virtual-desktop point (already
    /// clamped; the controller's cursor state is updated), then repaint.
    MoveCursor(Point),
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

type HintRow = &'static [(&'static str, &'static str)];

const ARROWS: &str = "\u{2190}\u{2191}\u{2193}\u{2192}";

const HINT_ROW2: HintRow = &[
    ("Tab", "Window berikutnya"),
    ("F", "Satu monitor"),
    ("A", "Semua monitor"),
    ("M", "Kaca pembesar"),
    ("C", "Salin warna"),
    ("H", "Sembunyikan petunjuk"),
];

/// Hint bar content as (key chip, description) pairs, one slice per row, for
/// the normal selection stage.
pub const HINT_ROWS: [HintRow; 2] = [
    &[
        ("Enter", "Ambil"),
        ("Esc", "Batal"),
        (ARROWS, "Geser 1 px"),
        ("Ctrl+\u{2190}\u{2191}\u{2193}\u{2192}", "Ubah ukuran"),
        ("Shift", "\u{d7}10"),
    ],
    HINT_ROW2,
];

/// Hint rows before any selection exists (cursor keys + `R`).
pub const HINT_ROWS_IDLE: [HintRow; 2] = [
    &[
        (ARROWS, "Geser kursor"),
        ("R", "Seleksi keyboard"),
        ("Shift", "\u{d7}10"),
        ("Tahan", "Cepat"),
        ("Enter", "Ambil"),
        ("Esc", "Batal"),
    ],
    HINT_ROW2,
];

/// Hint rows in keyboard selection mode.
pub const HINT_ROWS_KEYBOARD: [HintRow; 2] = [
    &[
        (ARROWS, "Perluas / geser (Tab)"),
        ("Shift", "\u{d7}10"),
        ("Tahan", "Cepat"),
        ("R", "Jangkar ulang"),
        ("Enter", "Ambil"),
        ("Esc", "Batal"),
    ],
    &[("M", "Kaca pembesar"), ("C", "Salin warna"), ("H", "Sembunyikan petunjuk")],
];

pub fn hint_rows(stage: Stage) -> &'static [HintRow; 2] {
    match stage {
        Stage::NoSelection => &HINT_ROWS_IDLE,
        Stage::Keyboard => &HINT_ROWS_KEYBOARD,
        Stage::Selected => &HINT_ROWS,
    }
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
    /// Keyboard selection mode (`R`), if active.
    kb: Option<KbSel>,
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
            kb: None,
        }
    }

    /// Anchor marker position while in keyboard selection mode.
    pub fn anchor(&self) -> Option<Point> {
        self.kb.map(|k| k.anchor)
    }

    pub fn kb_mode(&self) -> Option<KbMode> {
        self.kb.map(|k| k.mode)
    }

    pub fn stage(&self) -> Stage {
        if self.kb.is_some() {
            Stage::Keyboard
        } else if self.sel.rect().is_some() {
            Stage::Selected
        } else {
            Stage::NoSelection
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
        if self.kb.is_some() && !self.sel.is_dragging() {
            // Keyboard mode owns the cursor: ignore pointer motion (also the
            // echo of our own SetCursorPos) until a press leaves the mode.
            return Outcome::Idle;
        }
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
        if self.kb.take().is_some() {
            // Mouse takes over: drop the keyboard rect, behave like a fresh press.
            self.sel.clear();
        }
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

    /// Key press with the base step (1 px, Shift 10, Ctrl+Shift 10).
    pub fn key(&mut self, key: Key, mods: Mods) -> Outcome {
        self.key_step(key, mods, if mods.shift { 10 } else { 1 })
    }

    /// Key press where arrows move by `step` px (already accelerated; the
    /// caller drops events with step 0). `mods.ctrl` still selects resize.
    pub fn key_step(&mut self, key: Key, mods: Mods, step: i32) -> Outcome {
        self.notice = None;
        let step = step.max(1);
        let (dx, dy) = match key {
            Key::Left => (-step, 0),
            Key::Right => (step, 0),
            Key::Up => (0, -step),
            Key::Down => (0, step),
            Key::Escape => return Outcome::Done(None),
            Key::Enter | Key::Space => return self.confirm(),
            Key::Tab => {
                if let Some(k) = self.kb.as_mut() {
                    k.mode = match k.mode {
                        KbMode::Extend => KbMode::Move,
                        KbMode::Move => KbMode::Extend,
                    };
                    return Outcome::Redraw;
                }
                return self.cycle_windows(mods.shift);
            }
            Key::Anchor => return self.start_anchor(),
            Key::Monitor => {
                self.kb = None;
                let m = self.monitor_under_cursor();
                self.sel.set_rect(m);
                return Outcome::Redraw;
            }
            Key::All => {
                self.kb = None;
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
        if self.kb.is_some() {
            return self.kb_arrow(dx, dy, mods.ctrl);
        }
        if self.sel.rect().is_none() {
            // Stage 1: no selection yet, arrows steer the cursor.
            return self.move_cursor_by(dx, dy);
        }
        let changed = if mods.ctrl { self.sel.resize(dx, dy) } else { self.sel.nudge(dx, dy) };
        if changed {
            Outcome::Redraw
        } else {
            Outcome::Idle
        }
    }

    fn move_cursor_by(&mut self, dx: i32, dy: i32) -> Outcome {
        let Some(c) = self.cursor else { return Outcome::Idle };
        let p = self.sel.clamp_point(Point::new(c.x.saturating_add(dx), c.y.saturating_add(dy)));
        if p == c {
            return Outcome::Idle;
        }
        self.cursor = Some(p);
        self.hover = self.hit(p);
        Outcome::MoveCursor(p)
    }

    /// `R`: anchor at the cursor (or re-anchor); the selection starts as 1x1.
    fn start_anchor(&mut self) -> Outcome {
        let Some(c) = self.cursor else { return Outcome::Idle };
        let p = self.sel.clamp_point(c);
        self.cursor = Some(p);
        self.hover = None;
        self.sel.set_span(p, p);
        self.kb = Some(KbSel { anchor: p, corner: p, mode: KbMode::Extend });
        Outcome::Redraw
    }

    /// Arrow in keyboard selection mode.
    fn kb_arrow(&mut self, dx: i32, dy: i32, resize: bool) -> Outcome {
        let Some(mut k) = self.kb else { return Outcome::Idle };
        let before = self.sel.rect();
        if resize {
            self.sel.resize(dx, dy);
            // Re-derive anchor/corner from the (normalised) rect.
            if let Some(r) = self.sel.rect() {
                k.anchor = Point::new(r.x, r.y);
                k.corner = Point::new(r.right() - 1, r.bottom() - 1);
            }
        } else if k.mode == KbMode::Extend {
            k.corner = self.sel.clamp_point(Point::new(k.corner.x.saturating_add(dx), k.corner.y.saturating_add(dy)));
            self.sel.set_span(k.anchor, k.corner);
        } else if let Some(old) = before {
            self.sel.nudge(dx, dy);
            if let Some(new) = self.sel.rect() {
                let (mx, my) = (new.x - old.x, new.y - old.y);
                k.anchor = Point::new(k.anchor.x + mx, k.anchor.y + my);
                k.corner = Point::new(k.corner.x + mx, k.corner.y + my);
            }
        }
        self.kb = Some(k);
        // The real cursor follows the active corner (magnifier tracks it).
        let moved = self.cursor != Some(k.corner);
        self.cursor = Some(k.corner);
        if moved {
            Outcome::MoveCursor(k.corner)
        } else if self.sel.rect() != before {
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

    // ---- keyboard selection mode ----

    fn at(p: Point) -> Controller {
        let mut c = ctl(None, vec![]);
        c.mouse_move(p);
        c
    }

    #[test]
    fn arrows_without_selection_move_cursor_clamped() {
        let mut c = at(Point::new(10, 10));
        assert_eq!(c.key(Key::Right, N), Outcome::MoveCursor(Point::new(11, 10)));
        assert_eq!(c.cursor(), Some(Point::new(11, 10)));
        assert_eq!(c.key_step(Key::Down, N, 7), Outcome::MoveCursor(Point::new(11, 17)));
        assert_eq!(c.key(Key::Left, Mods::SHIFT), Outcome::MoveCursor(Point::new(1, 17)));
        // clamp at the top-left edge, then idle
        let mut c = at(Point::new(-98, 2));
        assert_eq!(c.key(Key::Left, Mods::SHIFT), Outcome::MoveCursor(Point::new(-100, 2)));
        assert_eq!(c.key(Key::Left, N), Outcome::Idle);
        assert_eq!(c.key(Key::Up, Mods::SHIFT), Outcome::MoveCursor(Point::new(-100, 0)));
        // clamp at the bottom-right (last pixel)
        let mut c = at(Point::new(895, 595));
        assert_eq!(c.key_step(Key::Right, N, 20), Outcome::MoveCursor(Point::new(899, 595)));
        assert_eq!(c.key_step(Key::Down, N, 20), Outcome::MoveCursor(Point::new(899, 599)));
        assert_eq!(c.key_step(Key::Down, N, 20), Outcome::Idle);
    }

    #[test]
    fn cursor_move_updates_hover() {
        let w = Rect::new(0, 0, 100, 100);
        let mut c = ctl(None, vec![w]);
        c.mouse_move(Point::new(99, 50));
        assert_eq!(c.shown_rect(), Some(w));
        c.key(Key::Right, N);
        assert_eq!(c.shown_rect(), None);
    }

    #[test]
    fn arrows_without_cursor_are_idle() {
        let mut c = ctl(None, vec![]);
        assert_eq!(c.key(Key::Right, N), Outcome::Idle);
        assert_eq!(c.key(Key::Anchor, N), Outcome::Idle);
    }

    #[test]
    fn existing_selection_keeps_nudge_not_cursor_move() {
        let mut c = ctl(Some(Rect::new(10, 10, 20, 20)), vec![]);
        c.mouse_move(Point::new(50, 50));
        assert_eq!(c.key(Key::Right, N), Outcome::Redraw);
        assert_eq!(c.selection(), Some(Rect::new(11, 10, 20, 20)));
        assert_eq!(c.cursor(), Some(Point::new(50, 50)));
    }

    #[test]
    fn r_anchors_and_arrows_extend() {
        let mut c = at(Point::new(100, 100));
        assert_eq!(c.stage(), Stage::NoSelection);
        assert_eq!(c.key(Key::Anchor, N), Outcome::Redraw);
        assert_eq!(c.stage(), Stage::Keyboard);
        assert_eq!(c.anchor(), Some(Point::new(100, 100)));
        assert_eq!(c.selection(), Some(Rect::new(100, 100, 1, 1)));
        assert_eq!(c.key(Key::Right, N), Outcome::MoveCursor(Point::new(101, 100)));
        c.key_step(Key::Down, N, 4);
        assert_eq!(c.selection(), Some(Rect::new(100, 100, 2, 5)));
        assert_eq!(c.cursor(), Some(Point::new(101, 104)));
        assert_eq!(c.anchor(), Some(Point::new(100, 100)));
    }

    #[test]
    fn crossing_the_anchor_flips() {
        let mut c = at(Point::new(100, 100));
        c.key(Key::Anchor, N);
        c.key_step(Key::Left, N, 10);
        assert_eq!(c.selection(), Some(Rect::new(90, 100, 11, 1)));
        c.key_step(Key::Up, N, 5);
        assert_eq!(c.selection(), Some(Rect::new(90, 95, 11, 6)));
        c.key_step(Key::Right, N, 20);
        assert_eq!(c.selection(), Some(Rect::new(100, 95, 11, 6)));
    }

    #[test]
    fn extend_clamps_to_bounds() {
        let mut c = at(Point::new(890, 590));
        c.key(Key::Anchor, N);
        c.key_step(Key::Right, N, 50);
        c.key_step(Key::Down, N, 50);
        assert_eq!(c.cursor(), Some(Point::new(899, 599)));
        assert_eq!(c.selection(), Some(Rect::new(890, 590, 10, 10)));
        assert_eq!(c.key(Key::Right, N), Outcome::Idle);
    }

    #[test]
    fn tab_toggles_extend_and_move_in_kb_mode() {
        let mut c = at(Point::new(100, 100));
        c.key_step(Key::Right, N, 1); // stage 1 cursor move
        c.key(Key::Anchor, N);
        c.key_step(Key::Right, N, 9);
        c.key_step(Key::Down, N, 4);
        assert_eq!(c.selection(), Some(Rect::new(101, 100, 10, 5)));
        assert_eq!(c.kb_mode(), Some(KbMode::Extend));
        assert_eq!(c.key(Key::Tab, N), Outcome::Redraw);
        assert_eq!(c.kb_mode(), Some(KbMode::Move));
        assert_eq!(c.key_step(Key::Right, N, 5), Outcome::MoveCursor(Point::new(115, 104)));
        assert_eq!(c.selection(), Some(Rect::new(106, 100, 10, 5)));
        assert_eq!(c.anchor(), Some(Point::new(106, 100)));
        c.key(Key::Tab, Mods::SHIFT);
        assert_eq!(c.kb_mode(), Some(KbMode::Extend));
        c.key_step(Key::Right, N, 2);
        assert_eq!(c.selection(), Some(Rect::new(106, 100, 12, 5)));
    }

    #[test]
    fn move_mode_stops_at_bounds() {
        let mut c = at(Point::new(880, 100));
        c.key(Key::Anchor, N);
        c.key_step(Key::Right, N, 10);
        c.key(Key::Tab, N);
        c.key_step(Key::Right, N, 50);
        assert_eq!(c.selection(), Some(Rect::new(889, 100, 11, 1)));
        assert_eq!(c.cursor(), Some(Point::new(899, 100)));
    }

    #[test]
    fn tab_outside_kb_mode_still_cycles_windows() {
        let w = Rect::new(0, 0, 100, 100);
        let mut c = ctl(None, vec![w]);
        assert_eq!(c.key(Key::Tab, N), Outcome::Redraw);
        assert_eq!(c.selection(), Some(w));
        assert_eq!(c.kb_mode(), None);
    }

    #[test]
    fn r_again_reanchors_at_cursor() {
        let mut c = at(Point::new(100, 100));
        c.key(Key::Anchor, N);
        c.key_step(Key::Right, N, 30);
        c.key(Key::Anchor, N);
        assert_eq!(c.anchor(), Some(Point::new(130, 100)));
        assert_eq!(c.selection(), Some(Rect::new(130, 100, 1, 1)));
        assert_eq!(c.kb_mode(), Some(KbMode::Extend));
    }

    #[test]
    fn enter_and_space_confirm_keyboard_rect_esc_cancels() {
        let mut c = at(Point::new(100, 100));
        c.key(Key::Anchor, N);
        c.key_step(Key::Right, N, 19);
        c.key_step(Key::Down, N, 9);
        let r = Rect::new(100, 100, 20, 10);
        assert_eq!(c.key(Key::Enter, N), Outcome::Done(Some(r)));
        assert_eq!(c.key(Key::Space, N), Outcome::Done(Some(r)));
        assert_eq!(c.key(Key::Escape, N), Outcome::Done(None));
    }

    #[test]
    fn mouse_press_leaves_kb_mode() {
        let mut c = at(Point::new(100, 100));
        c.key(Key::Anchor, N);
        c.key_step(Key::Right, N, 19);
        c.mouse_down(Point::new(300, 300));
        assert_eq!(c.kb_mode(), None);
        assert_eq!(c.selection(), None);
        c.mouse_move(Point::new(340, 330));
        assert_eq!(c.mouse_up(Point::new(340, 330)), Outcome::Done(Some(Rect::new(300, 300, 41, 31))));
    }

    #[test]
    fn pointer_motion_ignored_in_kb_mode_but_not_after_leaving() {
        let mut c = at(Point::new(100, 100));
        c.key(Key::Anchor, N);
        assert_eq!(c.mouse_move(Point::new(400, 400)), Outcome::Idle);
        assert_eq!(c.cursor(), Some(Point::new(100, 100)));
        c.mouse_down(Point::new(400, 400));
        assert_eq!(c.mouse_move(Point::new(450, 450)), Outcome::Redraw);
    }

    #[test]
    fn ctrl_arrows_resize_in_kb_mode_and_cursor_follows() {
        let mut c = at(Point::new(100, 100));
        c.key(Key::Anchor, N);
        c.key_step(Key::Right, N, 9);
        c.key_step(Key::Down, N, 9);
        assert_eq!(c.key_step(Key::Right, Mods::CTRL, 5), Outcome::MoveCursor(Point::new(114, 109)));
        assert_eq!(c.selection(), Some(Rect::new(100, 100, 15, 10)));
        c.key_step(Key::Left, Mods::CTRL, 3);
        assert_eq!(c.selection(), Some(Rect::new(100, 100, 12, 10)));
        assert_eq!(c.cursor(), Some(Point::new(111, 109)));
    }

    #[test]
    fn f_and_a_leave_kb_mode() {
        let mut c = at(Point::new(100, 100));
        c.key(Key::Anchor, N);
        c.key(Key::Monitor, N);
        assert_eq!(c.stage(), Stage::Selected);
        assert_eq!(c.anchor(), None);
    }

    #[test]
    fn hint_rows_follow_stage() {
        assert_eq!(hint_rows(Stage::NoSelection)[0][0].1, "Geser kursor");
        assert_eq!(hint_rows(Stage::Keyboard)[0][0].1, "Perluas / geser (Tab)");
        assert_eq!(hint_rows(Stage::Selected)[0][2].1, "Geser 1 px");
    }
}
