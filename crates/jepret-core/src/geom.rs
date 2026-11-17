use serde::{Deserialize, Serialize};

/// A point in physical pixels on the virtual desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// Rectangle in **physical pixels on the virtual desktop**
/// (origin = top-left of the primary monitor, may be negative).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }

    /// Build from two arbitrary corners (order does not matter).
    pub fn from_points(a: Point, b: Point) -> Self {
        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
        Self::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
    }

    /// Exclusive right edge.
    pub fn right(&self) -> i32 {
        self.x + self.width as i32
    }

    /// Exclusive bottom edge.
    pub fn bottom(&self) -> i32 {
        self.y + self.height as i32
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
    }

    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = self.right().max(other.right());
        let y1 = self.bottom().max(other.bottom());
        Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
    }

    /// Translate so that `origin` becomes (0,0) — e.g. virtual-desktop rect
    /// to coordinates inside a virtual-desktop screenshot.
    pub fn relative_to(&self, origin: Point) -> Rect {
        Rect::new(self.x - origin.x, self.y - origin.y, self.width, self.height)
    }

    pub fn origin(&self) -> Point {
        Point::new(self.x, self.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersect_and_union() {
        let a = Rect::new(-1920, 0, 1920, 1080);
        let b = Rect::new(0, 0, 2560, 1440);
        assert_eq!(a.intersect(&b), None);
        assert_eq!(a.union(&b), Rect::new(-1920, 0, 4480, 1440));
        let c = Rect::new(-100, 100, 200, 200);
        assert_eq!(a.intersect(&c), Some(Rect::new(-100, 100, 100, 200)));
    }

    #[test]
    fn from_points_any_order() {
        let r = Rect::from_points(Point::new(10, 50), Point::new(-10, 20));
        assert_eq!(r, Rect::new(-10, 20, 20, 30));
    }
}
