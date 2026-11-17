//! Editor document model: annotation shapes + unlimited undo/redo.
//!
//! Pure data, no egui. All coordinates are **image pixels** (f32, origin at the
//! top-left of the original image, regardless of zoom/pan/crop).

use jepret_core::Rect;

/// Straight-alpha sRGB colour.
pub type Rgba = [u8; 4];

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Pt {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    pub fn dist(self, o: Pt) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
}

/// Tools in toolbar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    Select,
    Crop,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Text,
    Highlight,
    Blur,
    Pixelate,
    Step,
    Pen,
}

impl Tool {
    pub const ALL: [Tool; 12] = [
        Tool::Select,
        Tool::Crop,
        Tool::Arrow,
        Tool::Line,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Text,
        Tool::Highlight,
        Tool::Blur,
        Tool::Pixelate,
        Tool::Step,
        Tool::Pen,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Crop => "Crop",
            Tool::Arrow => "Arrow",
            Tool::Line => "Line",
            Tool::Rect => "Rect",
            Tool::Ellipse => "Ellipse",
            Tool::Text => "Text",
            Tool::Highlight => "Highlight",
            Tool::Blur => "Blur",
            Tool::Pixelate => "Pixelate",
            Tool::Step => "Step",
            Tool::Pen => "Pen",
        }
    }

    /// Number-key shortcut (1..=9) -> tool.
    pub fn from_digit(d: u8) -> Option<Tool> {
        Some(match d {
            1 => Tool::Select,
            2 => Tool::Arrow,
            3 => Tool::Rect,
            4 => Tool::Ellipse,
            5 => Tool::Line,
            6 => Tool::Pen,
            7 => Tool::Text,
            8 => Tool::Step,
            9 => Tool::Blur,
            _ => return None,
        })
    }

    /// Default `width` (stroke px / strength / size knob) for new shapes.
    pub fn default_width(self) -> f32 {
        match self {
            Tool::Highlight => 1.0,
            Tool::Blur | Tool::Pixelate => 6.0,
            Tool::Text => 4.0,
            Tool::Step => 4.0,
            _ => 4.0,
        }
    }

    pub fn default_color(self) -> Rgba {
        match self {
            Tool::Highlight => [255, 230, 0, 110],
            _ => [235, 40, 40, 255],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShapeKind {
    Arrow { a: Pt, b: Pt },
    Line { a: Pt, b: Pt },
    /// Outline rectangle; `a`/`b` are any two opposite corners.
    Rect { a: Pt, b: Pt },
    Ellipse { a: Pt, b: Pt },
    /// Filled translucent rectangle.
    Highlight { a: Pt, b: Pt },
    /// Gaussian-ish blur of the original pixels; `width` = strength.
    Blur { a: Pt, b: Pt },
    /// Mosaic of the original pixels; `width` = strength.
    Pixelate { a: Pt, b: Pt },
    /// Text anchored at its top-left corner; font px = `width * 6`.
    Text { pos: Pt, text: String },
    /// Numbered circle; radius = `width * 4`.
    Step { center: Pt, n: u32 },
    Pen { points: Vec<Pt> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub kind: ShapeKind,
    pub color: Rgba,
    /// Stroke width in px; reinterpreted per kind (see [`ShapeKind`]).
    pub width: f32,
}

pub const TEXT_PX_PER_WIDTH: f32 = 6.0;
pub const STEP_RADIUS_PER_WIDTH: f32 = 4.0;

impl Shape {
    pub fn new(kind: ShapeKind, color: Rgba, width: f32) -> Self {
        Self { kind, color, width }
    }

    pub fn text_px(&self) -> f32 {
        (self.width * TEXT_PX_PER_WIDTH).max(6.0)
    }

    pub fn step_radius(&self) -> f32 {
        (self.width * STEP_RADIUS_PER_WIDTH).max(6.0)
    }

    /// Translate the whole shape.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        let mv = |p: &mut Pt| {
            p.x += dx;
            p.y += dy;
        };
        match &mut self.kind {
            ShapeKind::Arrow { a, b }
            | ShapeKind::Line { a, b }
            | ShapeKind::Rect { a, b }
            | ShapeKind::Ellipse { a, b }
            | ShapeKind::Highlight { a, b }
            | ShapeKind::Blur { a, b }
            | ShapeKind::Pixelate { a, b } => {
                mv(a);
                mv(b);
            }
            ShapeKind::Text { pos, .. } => mv(pos),
            ShapeKind::Step { center, .. } => mv(center),
            ShapeKind::Pen { points } => points.iter_mut().for_each(mv),
        }
    }

    /// Axis-aligned bounds `(min, max)` in image px (stroke not included).
    pub fn bounds(&self) -> (Pt, Pt) {
        let two = |a: &Pt, b: &Pt| {
            (Pt::new(a.x.min(b.x), a.y.min(b.y)), Pt::new(a.x.max(b.x), a.y.max(b.y)))
        };
        match &self.kind {
            ShapeKind::Arrow { a, b }
            | ShapeKind::Line { a, b }
            | ShapeKind::Rect { a, b }
            | ShapeKind::Ellipse { a, b }
            | ShapeKind::Highlight { a, b }
            | ShapeKind::Blur { a, b }
            | ShapeKind::Pixelate { a, b } => two(a, b),
            ShapeKind::Text { pos, text } => {
                let (w, h) = crate::render::text_size(text, self.text_px());
                (*pos, Pt::new(pos.x + w, pos.y + h))
            }
            ShapeKind::Step { center, .. } => {
                let r = self.step_radius();
                (Pt::new(center.x - r, center.y - r), Pt::new(center.x + r, center.y + r))
            }
            ShapeKind::Pen { points } => {
                let mut min = Pt::new(f32::MAX, f32::MAX);
                let mut max = Pt::new(f32::MIN, f32::MIN);
                for p in points {
                    min.x = min.x.min(p.x);
                    min.y = min.y.min(p.y);
                    max.x = max.x.max(p.x);
                    max.y = max.y.max(p.y);
                }
                if points.is_empty() {
                    (Pt::default(), Pt::default())
                } else {
                    (min, max)
                }
            }
        }
    }

    /// Is `p` on/inside the shape, with `tol` px of slack (for clicking).
    pub fn hit(&self, p: Pt, tol: f32) -> bool {
        let half = self.width.max(1.0) / 2.0 + tol;
        match &self.kind {
            ShapeKind::Arrow { a, b } | ShapeKind::Line { a, b } => seg_dist(p, *a, *b) <= half,
            ShapeKind::Rect { a, b } => {
                let (min, max) = self.bounds();
                let _ = (a, b);
                let inside_outer = p.x >= min.x - half
                    && p.x <= max.x + half
                    && p.y >= min.y - half
                    && p.y <= max.y + half;
                let inside_inner = p.x > min.x + half
                    && p.x < max.x - half
                    && p.y > min.y + half
                    && p.y < max.y - half;
                inside_outer && !inside_inner
            }
            ShapeKind::Ellipse { .. } => {
                let (min, max) = self.bounds();
                let c = Pt::new((min.x + max.x) / 2.0, (min.y + max.y) / 2.0);
                let rx = ((max.x - min.x) / 2.0).max(0.5);
                let ry = ((max.y - min.y) / 2.0).max(0.5);
                let nx = (p.x - c.x) / rx;
                let ny = (p.y - c.y) / ry;
                let f = (nx * nx + ny * ny).sqrt();
                // distance to outline ~ |f-1| * mean radius
                (f - 1.0).abs() * (rx + ry) / 2.0 <= half
            }
            ShapeKind::Highlight { .. }
            | ShapeKind::Blur { .. }
            | ShapeKind::Pixelate { .. }
            | ShapeKind::Text { .. } => {
                let (min, max) = self.bounds();
                p.x >= min.x - tol && p.x <= max.x + tol && p.y >= min.y - tol && p.y <= max.y + tol
            }
            ShapeKind::Step { center, .. } => center.dist(p) <= self.step_radius() + tol,
            ShapeKind::Pen { points } => {
                if points.len() == 1 {
                    return points[0].dist(p) <= half;
                }
                points.windows(2).any(|w| seg_dist(p, w[0], w[1]) <= half)
            }
        }
    }
}

/// Distance from `p` to segment `a`-`b`.
pub fn seg_dist(p: Pt, a: Pt, b: Pt) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= f32::EPSILON {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
    };
    p.dist(Pt::new(a.x + t * dx, a.y + t * dy))
}

/// Undoable state. `rev` is a unique id of this exact state, so dirtiness is
/// an O(1) comparison.
#[derive(Debug, Clone)]
struct Snapshot {
    shapes: Vec<Shape>,
    crop: Option<Rect>,
    rev: u64,
}

/// The annotation document. Does not own the image pixels.
#[derive(Debug)]
pub struct Document {
    cur: Snapshot,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    saved_rev: u64,
    next_rev: u64,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    pub fn new() -> Self {
        Self {
            cur: Snapshot { shapes: Vec::new(), crop: None, rev: 0 },
            undo: Vec::new(),
            redo: Vec::new(),
            saved_rev: 0,
            next_rev: 1,
        }
    }

    pub fn shapes(&self) -> &[Shape] {
        &self.cur.shapes
    }

    /// Crop rectangle relative to the original image, if any.
    pub fn crop(&self) -> Option<Rect> {
        self.cur.crop
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Has the document changed since [`mark_saved`](Self::mark_saved)
    /// (or creation)?
    pub fn is_dirty(&self) -> bool {
        self.cur.rev != self.saved_rev
    }

    pub fn mark_saved(&mut self) {
        self.saved_rev = self.cur.rev;
    }

    /// Opaque id of the current state; changes on every edit/undo/redo.
    pub fn revision(&self) -> u64 {
        self.cur.rev
    }

    fn bump(&mut self) {
        self.cur.rev = self.next_rev;
        self.next_rev += 1;
    }

    /// Push an undo point and clear redo. Call once before a group of
    /// `*_live` mutations (a drag, a slider scrub).
    pub fn begin_edit(&mut self) {
        self.undo.push(self.cur.clone());
        self.redo.clear();
        self.bump();
    }

    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some(prev) => {
                let cur = std::mem::replace(&mut self.cur, prev);
                self.redo.push(cur);
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.redo.pop() {
            Some(next) => {
                let cur = std::mem::replace(&mut self.cur, next);
                self.undo.push(cur);
                true
            }
            None => false,
        }
    }

    /// Add a shape (one undo step). Returns its index.
    pub fn add_shape(&mut self, shape: Shape) -> usize {
        self.begin_edit();
        self.cur.shapes.push(shape);
        self.cur.shapes.len() - 1
    }

    /// Remove a shape (one undo step).
    pub fn remove_shape(&mut self, idx: usize) -> Option<Shape> {
        if idx >= self.cur.shapes.len() {
            return None;
        }
        self.begin_edit();
        Some(self.cur.shapes.remove(idx))
    }

    /// Set/clear the crop rect (one undo step).
    pub fn set_crop(&mut self, crop: Option<Rect>) {
        if self.cur.crop == crop {
            return;
        }
        self.begin_edit();
        self.cur.crop = crop;
    }

    /// Mutate a shape in place **without** an undo point; call
    /// [`begin_edit`](Self::begin_edit) first (once per gesture).
    pub fn shape_live(&mut self, idx: usize) -> Option<&mut Shape> {
        self.cur.shapes.get_mut(idx)
    }

    /// Translate a shape without an undo point (see [`shape_live`](Self::shape_live)).
    pub fn translate_live(&mut self, idx: usize, dx: f32, dy: f32) {
        if let Some(s) = self.cur.shapes.get_mut(idx) {
            s.translate(dx, dy);
            self.bump();
        }
    }

    /// Replace a shape without an undo point.
    pub fn replace_live(&mut self, idx: usize, shape: Shape) {
        if let Some(s) = self.cur.shapes.get_mut(idx) {
            *s = shape;
            self.bump();
        }
    }

    /// Mark the current state as modified after direct `shape_live` edits.
    pub fn touch(&mut self) {
        self.bump();
    }

    /// Convenience: one-undo-step mutation of a shape.
    pub fn edit_shape(&mut self, idx: usize, f: impl FnOnce(&mut Shape)) {
        if idx < self.cur.shapes.len() {
            self.begin_edit();
            f(&mut self.cur.shapes[idx]);
        }
    }

    /// Topmost shape under `p`.
    pub fn hit_test(&self, p: Pt, tol: f32) -> Option<usize> {
        self.cur.shapes.iter().rposition(|s| s.hit(p, tol))
    }

    /// Next step-number label (max existing + 1).
    pub fn next_step_number(&self) -> u32 {
        self.cur
            .shapes
            .iter()
            .filter_map(|s| match s.kind {
                ShapeKind::Step { n, .. } => Some(n),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Shape {
        Shape::new(
            ShapeKind::Rect { a: Pt::new(x0, y0), b: Pt::new(x1, y1) },
            [255, 0, 0, 255],
            2.0,
        )
    }

    #[test]
    fn add_undo_redo() {
        let mut d = Document::new();
        assert!(!d.can_undo() && !d.can_redo());
        d.add_shape(rect(0.0, 0.0, 10.0, 10.0));
        d.add_shape(rect(5.0, 5.0, 20.0, 20.0));
        assert_eq!(d.shapes().len(), 2);
        assert!(d.undo());
        assert_eq!(d.shapes().len(), 1);
        assert!(d.undo());
        assert_eq!(d.shapes().len(), 0);
        assert!(!d.undo());
        assert!(d.redo());
        assert!(d.redo());
        assert_eq!(d.shapes().len(), 2);
        assert!(!d.redo());
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut d = Document::new();
        d.add_shape(rect(0.0, 0.0, 1.0, 1.0));
        d.undo();
        assert!(d.can_redo());
        d.add_shape(rect(2.0, 2.0, 3.0, 3.0));
        assert!(!d.can_redo());
    }

    #[test]
    fn unlimited_undo() {
        let mut d = Document::new();
        for i in 0..2000 {
            d.add_shape(rect(0.0, 0.0, i as f32, 1.0));
        }
        for _ in 0..2000 {
            assert!(d.undo());
        }
        assert!(d.shapes().is_empty());
    }

    #[test]
    fn remove_and_undo_restores_order() {
        let mut d = Document::new();
        d.add_shape(rect(0.0, 0.0, 1.0, 1.0));
        d.add_shape(rect(0.0, 0.0, 2.0, 2.0));
        d.add_shape(rect(0.0, 0.0, 3.0, 3.0));
        let removed = d.remove_shape(1).unwrap();
        assert_eq!(d.shapes().len(), 2);
        d.undo();
        assert_eq!(d.shapes()[1], removed);
        assert!(d.remove_shape(99).is_none());
    }

    #[test]
    fn drag_is_single_undo_step() {
        let mut d = Document::new();
        let i = d.add_shape(rect(0.0, 0.0, 10.0, 10.0));
        d.begin_edit();
        for _ in 0..5 {
            d.translate_live(i, 1.0, 2.0);
        }
        assert_eq!(d.shapes()[i].bounds().0, Pt::new(5.0, 10.0));
        d.undo();
        assert_eq!(d.shapes()[i].bounds().0, Pt::new(0.0, 0.0));
        d.redo();
        assert_eq!(d.shapes()[i].bounds().0, Pt::new(5.0, 10.0));
    }

    #[test]
    fn dirty_tracking() {
        let mut d = Document::new();
        assert!(!d.is_dirty());
        d.add_shape(rect(0.0, 0.0, 1.0, 1.0));
        assert!(d.is_dirty());
        d.mark_saved();
        assert!(!d.is_dirty());
        d.undo();
        assert!(d.is_dirty());
        d.redo();
        assert!(!d.is_dirty());
        d.edit_shape(0, |s| s.color = [0, 0, 0, 255]);
        assert!(d.is_dirty());
    }

    #[test]
    fn crop_undoable_and_noop_when_same() {
        let mut d = Document::new();
        d.set_crop(None);
        assert!(!d.can_undo());
        d.set_crop(Some(Rect::new(1, 2, 3, 4)));
        assert_eq!(d.crop(), Some(Rect::new(1, 2, 3, 4)));
        d.undo();
        assert_eq!(d.crop(), None);
    }

    #[test]
    fn hit_test_topmost_and_outline() {
        let mut d = Document::new();
        d.add_shape(rect(0.0, 0.0, 100.0, 100.0));
        d.add_shape(Shape::new(
            ShapeKind::Highlight { a: Pt::new(40.0, 40.0), b: Pt::new(60.0, 60.0) },
            [255, 255, 0, 100],
            1.0,
        ));
        assert_eq!(d.hit_test(Pt::new(50.0, 50.0), 2.0), Some(1));
        // inside the big rect but not on its outline and not in highlight
        assert_eq!(d.hit_test(Pt::new(20.0, 20.0), 2.0), None);
        assert_eq!(d.hit_test(Pt::new(0.0, 50.0), 2.0), Some(0));
    }

    #[test]
    fn step_numbers_increment() {
        let mut d = Document::new();
        assert_eq!(d.next_step_number(), 1);
        d.add_shape(Shape::new(ShapeKind::Step { center: Pt::new(5.0, 5.0), n: 1 }, [0; 4], 4.0));
        d.add_shape(Shape::new(ShapeKind::Step { center: Pt::new(9.0, 9.0), n: 2 }, [0; 4], 4.0));
        assert_eq!(d.next_step_number(), 3);
    }

    #[test]
    fn line_hit_and_pen_hit() {
        let line = Shape::new(
            ShapeKind::Line { a: Pt::new(0.0, 0.0), b: Pt::new(100.0, 0.0) },
            [0; 4],
            2.0,
        );
        assert!(line.hit(Pt::new(50.0, 3.0), 2.0));
        assert!(!line.hit(Pt::new(50.0, 10.0), 2.0));
        let pen = Shape::new(
            ShapeKind::Pen { points: vec![Pt::new(0.0, 0.0), Pt::new(10.0, 10.0)] },
            [0; 4],
            2.0,
        );
        assert!(pen.hit(Pt::new(5.0, 5.0), 1.0));
    }
}
