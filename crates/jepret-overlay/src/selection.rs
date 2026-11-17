//! Pure selection state machine (no Win32). All coordinates are physical
//! pixels on the virtual desktop.
//!
//! Selections are **pixel-inclusive**: dragging from pixel A to pixel B yields
//! a rect that contains both A and B, so the pixel under the cursor is always
//! part of the selection and there is never a 1 px gap at monitor borders.

use jepret_core::{Point, Rect};

/// Cursor must travel at least this many px (either axis) before a press
/// counts as a drag rather than a click.
pub const DRAG_THRESHOLD: i32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragEnd {
    /// Drag finished; the selection holds the final rect.
    Selected(Rect),
    /// Press+release without moving past the threshold (position given).
    Click(Point),
    /// Release without a matching press.
    None,
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    anchor: Point,
    moved: bool,
}

#[derive(Debug, Clone)]
pub struct Selection {
    bounds: Rect,
    rect: Option<Rect>,
    drag: Option<Drag>,
}

/// Rect covering both pixels `a` and `b` (inclusive).
pub fn inclusive_rect(a: Point, b: Point) -> Rect {
    let r = Rect::from_points(a, b);
    Rect::new(r.x, r.y, r.width + 1, r.height + 1)
}

impl Selection {
    /// `bounds` = virtual-desktop rect the selection must stay inside.
    pub fn new(bounds: Rect, initial: Option<Rect>) -> Self {
        let mut s = Self { bounds, rect: None, drag: None };
        if let Some(r) = initial {
            s.set_rect(r);
        }
        s
    }

    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    pub fn rect(&self) -> Option<Rect> {
        self.rect
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// True once a press has moved past the click threshold.
    pub fn is_drag_active(&self) -> bool {
        self.drag.is_some_and(|d| d.moved)
    }

    /// Clamp a point so it lies on a pixel inside the bounds.
    pub fn clamp_point(&self, p: Point) -> Point {
        let b = self.bounds;
        if b.is_empty() {
            return p;
        }
        Point::new(p.x.clamp(b.x, b.right() - 1), p.y.clamp(b.y, b.bottom() - 1))
    }

    /// Replace the selection (clipped to bounds). Empty/outside -> cleared.
    pub fn set_rect(&mut self, r: Rect) {
        self.rect = self.bounds.intersect(&r);
    }

    pub fn clear(&mut self) {
        self.rect = None;
    }

    pub fn begin_drag(&mut self, p: Point) {
        self.drag = Some(Drag { anchor: self.clamp_point(p), moved: false });
    }

    /// Returns true when the selection rect changed.
    pub fn update_drag(&mut self, p: Point) -> bool {
        let p = self.clamp_point(p);
        let Some(d) = self.drag.as_mut() else { return false };
        if !d.moved && (p.x - d.anchor.x).abs() < DRAG_THRESHOLD && (p.y - d.anchor.y).abs() < DRAG_THRESHOLD {
            return false;
        }
        d.moved = true;
        let r = inclusive_rect(d.anchor, p);
        let changed = self.rect != Some(r);
        self.rect = Some(r);
        changed
    }

    pub fn end_drag(&mut self, p: Point) -> DragEnd {
        let p = self.clamp_point(p);
        self.update_drag(p);
        match self.drag.take() {
            None => DragEnd::None,
            Some(d) if d.moved => match self.rect {
                Some(r) => DragEnd::Selected(r),
                None => DragEnd::None,
            },
            Some(_) => DragEnd::Click(p),
        }
    }

    /// Move the selection by (dx, dy), staying inside bounds. Size is kept.
    pub fn nudge(&mut self, dx: i32, dy: i32) -> bool {
        let Some(r) = self.rect else { return false };
        let b = self.bounds;
        let x = (r.x + dx).clamp(b.x, (b.right() - r.width as i32).max(b.x));
        let y = (r.y + dy).clamp(b.y, (b.bottom() - r.height as i32).max(b.y));
        let n = Rect::new(x, y, r.width, r.height);
        self.rect = Some(n);
        n != r
    }

    /// Grow/shrink the bottom-right edge by (dw, dh), min 1 px, inside bounds.
    pub fn resize(&mut self, dw: i32, dh: i32) -> bool {
        let Some(r) = self.rect else { return false };
        let b = self.bounds;
        let max_w = (b.right() - r.x).max(1);
        let max_h = (b.bottom() - r.y).max(1);
        let w = (r.width as i32 + dw).clamp(1, max_w) as u32;
        let h = (r.height as i32 + dh).clamp(1, max_h) as u32;
        let n = Rect::new(r.x, r.y, w, h);
        self.rect = Some(n);
        n != r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: Rect = Rect::new(-1920, 0, 4480, 1440);

    #[test]
    fn drag_is_pixel_inclusive_any_direction() {
        let mut s = Selection::new(B, None);
        s.begin_drag(Point::new(100, 100));
        s.update_drag(Point::new(10, 50));
        assert_eq!(s.rect(), Some(Rect::new(10, 50, 91, 51)));
        let end = s.end_drag(Point::new(10, 50));
        assert_eq!(end, DragEnd::Selected(Rect::new(10, 50, 91, 51)));
        assert!(!s.is_dragging());
    }

    #[test]
    fn click_below_threshold() {
        let mut s = Selection::new(B, None);
        s.begin_drag(Point::new(5, 5));
        assert!(!s.update_drag(Point::new(7, 6)));
        assert_eq!(s.end_drag(Point::new(7, 6)), DragEnd::Click(Point::new(7, 6)));
        assert_eq!(s.rect(), None);
    }

    #[test]
    fn click_keeps_existing_selection() {
        let mut s = Selection::new(B, Some(Rect::new(0, 0, 10, 10)));
        s.begin_drag(Point::new(500, 500));
        s.end_drag(Point::new(500, 500));
        assert_eq!(s.rect(), Some(Rect::new(0, 0, 10, 10)));
    }

    #[test]
    fn drag_clamps_to_bounds_inclusive_last_pixel() {
        let mut s = Selection::new(B, None);
        s.begin_drag(Point::new(2000, 1000));
        s.update_drag(Point::new(99999, 99999));
        // last pixel is (2559, 1439) -> rect reaches exactly the bounds edge.
        assert_eq!(s.rect(), Some(Rect::new(2000, 1000, 560, 440)));
        assert_eq!(s.rect().unwrap().right(), B.right());
        s.begin_drag(Point::new(-1900, 10));
        s.update_drag(Point::new(-99999, -5));
        assert_eq!(s.rect(), Some(Rect::new(-1920, 0, 21, 11)));
    }

    #[test]
    fn end_without_begin() {
        let mut s = Selection::new(B, None);
        assert_eq!(s.end_drag(Point::new(1, 1)), DragEnd::None);
    }

    #[test]
    fn initial_is_clipped() {
        let s = Selection::new(B, Some(Rect::new(2500, 1400, 500, 500)));
        assert_eq!(s.rect(), Some(Rect::new(2500, 1400, 60, 40)));
        let s = Selection::new(B, Some(Rect::new(9000, 9000, 5, 5)));
        assert_eq!(s.rect(), None);
    }

    #[test]
    fn nudge_moves_exactly_one_px_and_stops_at_edges() {
        let mut s = Selection::new(B, Some(Rect::new(10, 10, 100, 50)));
        assert!(s.nudge(1, 0));
        assert!(s.nudge(0, -1));
        assert_eq!(s.rect(), Some(Rect::new(11, 9, 100, 50)));
        s.set_rect(Rect::new(-1920, 0, 100, 50));
        assert!(!s.nudge(-1, 0));
        assert!(!s.nudge(0, -1));
        s.set_rect(Rect::new(2460, 1390, 100, 50));
        assert!(!s.nudge(1, 0));
        assert!(!s.nudge(0, 1));
        assert!(s.nudge(-1, -1));
        assert_eq!(s.rect(), Some(Rect::new(2459, 1389, 100, 50)));
    }

    #[test]
    fn resize_min_one_and_clamped() {
        let mut s = Selection::new(B, Some(Rect::new(2550, 1430, 10, 10)));
        assert!(!s.resize(1, 1)); // already at bounds
        assert!(s.resize(-20, -20));
        assert_eq!(s.rect(), Some(Rect::new(2550, 1430, 1, 1)));
        assert!(!s.resize(-1, -1));
        assert!(s.resize(3, 0));
        assert_eq!(s.rect(), Some(Rect::new(2550, 1430, 4, 1)));
    }

    #[test]
    fn keys_without_selection_are_noops() {
        let mut s = Selection::new(B, None);
        assert!(!s.nudge(1, 1));
        assert!(!s.resize(1, 1));
    }
}
