//! Hover-snap hit testing (pure).

use jepret_core::{Point, Rect};

/// First rect in `rects` (topmost first) that contains `p`, clipped to
/// `bounds` (the overlay's virtual desktop). Empty rects are ignored.
pub fn hit_test(rects: &[Rect], bounds: Rect, p: Point) -> Option<Rect> {
    rects
        .iter()
        .find(|r| !r.is_empty() && r.contains(p))
        .and_then(|r| r.intersect(&bounds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topmost_wins() {
        let top = Rect::new(100, 100, 200, 200);
        let bottom = Rect::new(0, 0, 1000, 1000);
        let rects = [top, bottom];
        let b = Rect::new(0, 0, 1920, 1080);
        assert_eq!(hit_test(&rects, b, Point::new(150, 150)), Some(top));
        assert_eq!(hit_test(&rects, b, Point::new(500, 500)), Some(bottom));
        assert_eq!(hit_test(&rects, b, Point::new(1500, 500)), None);
    }

    #[test]
    fn edges_are_half_open() {
        let r = Rect::new(10, 10, 10, 10);
        let b = Rect::new(0, 0, 100, 100);
        assert!(hit_test(&[r], b, Point::new(10, 10)).is_some());
        assert!(hit_test(&[r], b, Point::new(19, 19)).is_some());
        assert!(hit_test(&[r], b, Point::new(20, 19)).is_none());
    }

    #[test]
    fn clipped_to_bounds_and_empty_ignored() {
        let b = Rect::new(0, 0, 100, 100);
        let r = Rect::new(-50, -50, 100, 100);
        assert_eq!(hit_test(&[r], b, Point::new(10, 10)), Some(Rect::new(0, 0, 50, 50)));
        assert_eq!(hit_test(&[Rect::new(5, 5, 0, 10)], b, Point::new(5, 6)), None);
    }

    #[test]
    fn empty_list_disables_snapping() {
        assert_eq!(hit_test(&[], Rect::new(0, 0, 10, 10), Point::new(1, 1)), None);
    }
}
