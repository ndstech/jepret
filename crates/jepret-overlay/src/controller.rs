//! Pure interaction controller: turns abstract input events into selection
//! changes and a final outcome. The Win32 layer only translates messages.

use jepret_core::{Point, Rect};

use crate::selection::{DragEnd, Selection};
use crate::snap::hit_test;

/// Finish as soon as the mouse button is released after a drag / window click
/// (fast path). Keyboard nudge/resize + Enter are meant for a preselected
/// (`initial`) rect and for adjusting while the button is still held.
pub const CONFIRM_ON_RELEASE: bool = true;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    Enter,
    Escape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing visible changed.
    Idle,
    /// Needs a repaint.
    Redraw,
    /// Overlay should close with this result.
    Done(Option<Rect>),
}

pub struct Controller {
    sel: Selection,
    windows: Vec<Rect>,
    hover: Option<Rect>,
    cursor: Option<Point>,
}

impl Controller {
    pub fn new(bounds: Rect, initial: Option<Rect>, windows: Vec<Rect>) -> Self {
        Self { sel: Selection::new(bounds, initial), windows, hover: None, cursor: None }
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

    pub fn mouse_move(&mut self, p: Point) -> Outcome {
        let p = self.sel.clamp_point(p);
        self.cursor = Some(p);
        if self.sel.is_dragging() {
            self.sel.update_drag(p);
            self.hover = None;
        } else {
            self.hover = hit_test(&self.windows, self.sel.bounds(), p);
        }
        Outcome::Redraw
    }

    pub fn mouse_down(&mut self, p: Point) -> Outcome {
        let p = self.sel.clamp_point(p);
        self.cursor = Some(p);
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
            DragEnd::Click(p) => match hit_test(&self.windows, self.sel.bounds(), p) {
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

    pub fn right_click(&mut self) -> Outcome {
        Outcome::Done(None)
    }

    pub fn key(&mut self, key: Key, shift: bool) -> Outcome {
        let (dx, dy) = match key {
            Key::Left => (-1, 0),
            Key::Right => (1, 0),
            Key::Up => (0, -1),
            Key::Down => (0, 1),
            Key::Escape => return Outcome::Done(None),
            Key::Enter => {
                return match self.sel.rect().or(self.hover) {
                    Some(r) => Outcome::Done(Some(r)),
                    None => Outcome::Idle,
                };
            }
        };
        let changed = if shift { self.sel.resize(dx, dy) } else { self.sel.nudge(dx, dy) };
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

    fn ctl(initial: Option<Rect>, windows: Vec<Rect>) -> Controller {
        Controller::new(B, initial, windows)
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
    fn keyboard_nudge_resize_enter_on_initial() {
        let mut c = ctl(Some(Rect::new(10, 10, 100, 50)), vec![]);
        assert_eq!(c.key(Key::Right, false), Outcome::Redraw);
        assert_eq!(c.key(Key::Down, false), Outcome::Redraw);
        assert_eq!(c.selection(), Some(Rect::new(11, 11, 100, 50)));
        assert_eq!(c.key(Key::Right, true), Outcome::Redraw);
        assert_eq!(c.key(Key::Up, true), Outcome::Redraw);
        assert_eq!(c.selection(), Some(Rect::new(11, 11, 101, 49)));
        assert_eq!(c.key(Key::Enter, false), Outcome::Done(Some(Rect::new(11, 11, 101, 49))));
    }

    #[test]
    fn nudge_at_edge_is_idle() {
        let mut c = ctl(Some(Rect::new(-100, 0, 10, 10)), vec![]);
        assert_eq!(c.key(Key::Left, false), Outcome::Idle);
        assert_eq!(c.key(Key::Up, false), Outcome::Idle);
    }

    #[test]
    fn cancel_paths() {
        let mut c = ctl(Some(Rect::new(0, 0, 10, 10)), vec![]);
        assert_eq!(c.key(Key::Escape, false), Outcome::Done(None));
        assert_eq!(c.right_click(), Outcome::Done(None));
    }

    #[test]
    fn enter_without_anything_is_idle_and_with_hover_confirms_it() {
        let w = Rect::new(0, 0, 50, 50);
        let mut c = ctl(None, vec![w]);
        assert_eq!(c.key(Key::Enter, false), Outcome::Idle);
        c.mouse_move(Point::new(5, 5));
        assert_eq!(c.key(Key::Enter, false), Outcome::Done(Some(w)));
    }

    #[test]
    fn initial_is_shown_and_cursor_clamped() {
        let mut c = ctl(Some(Rect::new(5, 5, 20, 20)), vec![]);
        assert_eq!(c.shown_rect(), Some(Rect::new(5, 5, 20, 20)));
        c.mouse_move(Point::new(100_000, -5));
        assert_eq!(c.cursor(), Some(Point::new(899, 0)));
    }
}
