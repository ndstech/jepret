//! Magnifier geometry (pure). The loupe shows a `SRC_SIDE`x`SRC_SIDE` pixel
//! neighbourhood of the cursor scaled by `ZOOM` (nearest neighbour).

use jepret_core::{Point, Rect};

pub const ZOOM: u32 = 8;
pub const SRC_SIDE: u32 = 15;
pub const LABEL_HEIGHT: u32 = 40;
const OFFSET: i32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoupeLayout {
    /// Source pixels to magnify (virtual-desktop coords, inside bounds).
    pub src: Rect,
    /// Where the magnified image is drawn (virtual-desktop coords).
    pub dest: Rect,
    /// Label strip directly below `dest`.
    pub label: Rect,
    /// Cell inside `dest` that corresponds to the cursor pixel.
    pub cursor_cell: Rect,
}

/// `bounds` = region the loupe must stay inside (virtual desktop).
pub fn layout(cursor: Point, bounds: Rect) -> LoupeLayout {
    let sw = SRC_SIDE.min(bounds.width).max(1);
    let sh = SRC_SIDE.min(bounds.height).max(1);
    let half = (SRC_SIDE / 2) as i32;
    let sx = (cursor.x - half).clamp(bounds.x, (bounds.right() - sw as i32).max(bounds.x));
    let sy = (cursor.y - half).clamp(bounds.y, (bounds.bottom() - sh as i32).max(bounds.y));
    let src = Rect::new(sx, sy, sw, sh);

    let (dw, dh) = (sw * ZOOM, sh * ZOOM);
    let total_h = (dh + LABEL_HEIGHT) as i32;

    let mut x = cursor.x + OFFSET;
    if x + dw as i32 > bounds.right() {
        x = cursor.x - OFFSET - dw as i32;
    }
    let mut y = cursor.y + OFFSET;
    if y + total_h > bounds.bottom() {
        y = cursor.y - OFFSET - total_h;
    }
    x = x.clamp(bounds.x, (bounds.right() - dw as i32).max(bounds.x));
    y = y.clamp(bounds.y, (bounds.bottom() - total_h).max(bounds.y));

    let dest = Rect::new(x, y, dw, dh);
    let label = Rect::new(x, y + dh as i32, dw, LABEL_HEIGHT);
    let cursor_cell = Rect::new(
        x + (cursor.x - sx) * ZOOM as i32,
        y + (cursor.y - sy) * ZOOM as i32,
        ZOOM,
        ZOOM,
    );
    LoupeLayout { src, dest, label, cursor_cell }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: Rect = Rect::new(-1920, 0, 4480, 1440);

    #[test]
    fn centered_in_open_space() {
        let l = layout(Point::new(500, 500), B);
        assert_eq!(l.src, Rect::new(493, 493, 15, 15));
        assert_eq!(l.dest, Rect::new(520, 520, 120, 120));
        assert_eq!(l.label, Rect::new(520, 640, 120, 40));
        assert_eq!(l.cursor_cell, Rect::new(520 + 7 * 8, 520 + 7 * 8, 8, 8));
    }

    #[test]
    fn flips_near_right_bottom() {
        let l = layout(Point::new(2550, 1430), B);
        assert!(l.dest.right() <= B.right());
        assert!(l.label.bottom() <= B.bottom());
        assert!(l.dest.x < 2550 && l.dest.y < 1430);
    }

    #[test]
    fn src_stays_in_bounds_at_corners() {
        for p in [Point::new(-1920, 0), Point::new(2559, 1439), Point::new(-1920, 1439), Point::new(2559, 0)] {
            let l = layout(p, B);
            assert_eq!(B.intersect(&l.src), Some(l.src), "{p:?}");
            assert!(l.src.contains(p));
            assert_eq!(B.intersect(&l.dest), Some(l.dest));
            assert_eq!(l.dest.intersect(&l.cursor_cell), Some(l.cursor_cell));
        }
    }

    #[test]
    fn tiny_bounds_do_not_panic() {
        let l = layout(Point::new(1, 1), Rect::new(0, 0, 3, 3));
        assert_eq!(l.src, Rect::new(0, 0, 3, 3));
    }
}
