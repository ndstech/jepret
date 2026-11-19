//! Editor document model: annotation shapes + unlimited undo/redo.
//!
//! Pure data, no egui. All coordinates are **image pixels** (f32, origin at the
//! top-left of the original image, regardless of zoom/pan/crop).

use jepret_core::{EffectSpec, Rect};

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
    TextBox,
    Callout,
    Highlight,
    Blur,
    Pixelate,
    Step,
    Pen,
    Polygon,
    Spotlight,
    Magnify,
    Eyedropper,
}

impl Tool {
    /// Tools whose click places or edits text.
    pub fn is_text_tool(self) -> bool {
        matches!(self, Tool::Text | Tool::TextBox | Tool::Callout)
    }

    pub const ALL: [Tool; 18] = [
        Tool::Select,
        Tool::Crop,
        Tool::Arrow,
        Tool::Line,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Text,
        Tool::TextBox,
        Tool::Callout,
        Tool::Highlight,
        Tool::Blur,
        Tool::Pixelate,
        Tool::Step,
        Tool::Pen,
        Tool::Polygon,
        Tool::Spotlight,
        Tool::Magnify,
        Tool::Eyedropper,
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
            Tool::TextBox => "Text box",
            Tool::Callout => "Callout",
            Tool::Highlight => "Highlight",
            Tool::Blur => "Blur",
            Tool::Pixelate => "Pixelate",
            Tool::Step => "Step",
            Tool::Pen => "Pen",
            Tool::Polygon => "Polygon",
            Tool::Spotlight => "Spotlight",
            Tool::Magnify => "Magnify",
            Tool::Eyedropper => "Pick color",
        }
    }

    /// Shortcut-catalogue id (`jepret_core::shortcuts`).
    pub fn id(self) -> &'static str {
        match self {
            Tool::Select => "tool.select",
            Tool::Crop => "tool.crop",
            Tool::Arrow => "tool.arrow",
            Tool::Line => "tool.line",
            Tool::Rect => "tool.rect",
            Tool::Ellipse => "tool.ellipse",
            Tool::Text => "tool.text",
            Tool::TextBox => "tool.textbox",
            Tool::Callout => "tool.callout",
            Tool::Highlight => "tool.highlight",
            Tool::Blur => "tool.blur",
            Tool::Pixelate => "tool.pixelate",
            Tool::Step => "tool.step",
            Tool::Pen => "tool.pen",
            Tool::Polygon => "tool.polygon",
            Tool::Spotlight => "tool.spotlight",
            Tool::Magnify => "tool.magnify",
            Tool::Eyedropper => "tool.eyedropper",
        }
    }

    pub fn from_id(id: &str) -> Option<Tool> {
        Tool::ALL.iter().copied().find(|t| t.id() == id)
    }

    /// Default `width` (stroke px / strength / size knob) for new shapes.
    pub fn default_width(self) -> f32 {
        match self {
            Tool::Highlight => 1.0,
            Tool::Blur | Tool::Pixelate => 6.0,
            Tool::Text => 4.0,
            Tool::TextBox | Tool::Callout => 3.0,
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
    /// Text wrapped inside a box. `a`/`b` = opposite corners of the dragged
    /// area; the box grows downwards when the text needs more room.
    /// `fill` = solid box (text in a contrasting colour), else outline only.
    TextBox { a: Pt, b: Pt, text: String },
    /// Speech bubble with a tail to `tip`. `a`/`b` = opposite corners of the
    /// bubble box; text wraps inside it and the height grows (away from the
    /// tip) to fit. A (near) zero-width box is auto-sized to the text.
    Callout { tip: Pt, a: Pt, b: Pt, text: String },
    /// Numbered circle; radius = `width * 4`.
    Step { center: Pt, n: u32 },
    Pen { points: Vec<Pt> },
    /// Closed polygon through `points` (>= 3); outline, or filled with `fill`.
    Polygon { points: Vec<Pt> },
    /// Snagit-style spotlight: everything *outside* the union of all
    /// spotlights is darkened ([`SPOTLIGHT_DIM_ALPHA`]). `a`/`b` = opposite
    /// corners; `fill` = elliptical instead of rectangular. Only the shared
    /// dimming is drawn (no outline). The darkening is applied once, at the
    /// z-position of the topmost spotlight: image and shapes below it are
    /// dimmed, shapes above it are not.
    Spotlight { a: Pt, b: Pt },
    /// Magnifier: a circle (`center`, `radius`) showing the *base image*
    /// around `center` enlarged `zoom` times (annotations are not magnified),
    /// with a border in the shape colour / width.
    Magnify { center: Pt, radius: f32, zoom: f32 },
}

/// Black dimming applied outside the spotlights (0..=255).
pub const SPOTLIGHT_DIM_ALPHA: u8 = 140;
/// Selectable magnifier zoom factors.
pub const MAGNIFY_ZOOMS: [f32; 4] = [1.5, 2.0, 3.0, 4.0];
/// Default magnifier zoom.
pub const MAGNIFY_DEFAULT_ZOOM: f32 = 2.0;

impl ShapeKind {
    /// The text of a text-bearing shape.
    pub fn text(&self) -> Option<&str> {
        match self {
            ShapeKind::Text { text, .. } | ShapeKind::TextBox { text, .. } | ShapeKind::Callout { text, .. } => {
                Some(text)
            }
            _ => None,
        }
    }

    pub fn text_mut(&mut self) -> Option<&mut String> {
        match self {
            ShapeKind::Text { text, .. } | ShapeKind::TextBox { text, .. } | ShapeKind::Callout { text, .. } => {
                Some(text)
            }
            _ => None,
        }
    }

    pub fn is_text(&self) -> bool {
        self.text().is_some()
    }

    /// The tool that creates this kind of shape (icon / name in the object list).
    pub fn tool(&self) -> Tool {
        match self {
            ShapeKind::Arrow { .. } => Tool::Arrow,
            ShapeKind::Line { .. } => Tool::Line,
            ShapeKind::Rect { .. } => Tool::Rect,
            ShapeKind::Ellipse { .. } => Tool::Ellipse,
            ShapeKind::Highlight { .. } => Tool::Highlight,
            ShapeKind::Blur { .. } => Tool::Blur,
            ShapeKind::Pixelate { .. } => Tool::Pixelate,
            ShapeKind::Text { .. } => Tool::Text,
            ShapeKind::TextBox { .. } => Tool::TextBox,
            ShapeKind::Callout { .. } => Tool::Callout,
            ShapeKind::Step { .. } => Tool::Step,
            ShapeKind::Pen { .. } => Tool::Pen,
            ShapeKind::Polygon { .. } => Tool::Polygon,
            ShapeKind::Spotlight { .. } => Tool::Spotlight,
            ShapeKind::Magnify { .. } => Tool::Magnify,
        }
    }

    /// Short Indonesian name used for generated object labels.
    pub fn display_name(&self) -> &'static str {
        match self {
            ShapeKind::Arrow { .. } => "Panah",
            ShapeKind::Line { .. } => "Garis",
            ShapeKind::Rect { .. } => "Kotak",
            ShapeKind::Ellipse { .. } => "Elips",
            ShapeKind::Highlight { .. } => "Highlight",
            ShapeKind::Blur { .. } => "Blur",
            ShapeKind::Pixelate { .. } => "Pixelate",
            ShapeKind::Text { .. } => "Teks",
            ShapeKind::TextBox { .. } => "Kotak teks",
            ShapeKind::Callout { .. } => "Callout",
            ShapeKind::Step { .. } => "Nomor",
            ShapeKind::Pen { .. } => "Pen",
            ShapeKind::Polygon { .. } => "Polygon",
            ShapeKind::Spotlight { .. } => "Sorotan",
            ShapeKind::Magnify { .. } => "Kaca pembesar",
        }
    }
}

/// Max characters of a text shape's label in the object list.
const LABEL_MAX_CHARS: usize = 40;

/// Labels for the object list, one per shape in stacking order.
/// Text shapes show their first non-empty line (elided); others are
/// "<name> <n>" numbered per kind in creation order; steps show "Nomor <n>"
/// with their own number.
pub fn object_labels(shapes: &[Shape]) -> Vec<String> {
    let mut counts: std::collections::HashMap<&'static str, usize> = std::collections::HashMap::new();
    shapes
        .iter()
        .map(|s| {
            let name = s.kind.display_name();
            let c = counts.entry(name).or_insert(0);
            *c += 1;
            if let ShapeKind::Step { n, .. } = &s.kind {
                return format!("{name} {n}");
            }
            if let Some(t) = s.kind.text() {
                if let Some(line) = t.lines().map(str::trim).find(|l| !l.is_empty()) {
                    return elide(line, LABEL_MAX_CHARS);
                }
            }
            format!("{name} {c}")
        })
        .collect()
}

/// Cut `s` to `max` chars, ending with an ellipsis when shortened.
pub fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub kind: ShapeKind,
    pub color: Rgba,
    /// Stroke width in px; reinterpreted per kind (see [`ShapeKind`]).
    pub width: f32,
    /// Filled instead of outline (Rect / Ellipse).
    pub fill: bool,
    /// Offset drop shadow.
    pub shadow: bool,
    /// Contrasting halo around text (Text / TextBox / Callout) so it stays
    /// readable on any background.
    pub outline: bool,
}

pub const SHADOW_OFFSET: f32 = 3.0;
pub const SHADOW_COLOR: Rgba = [0, 0, 0, 100];
pub const TEXT_PX_PER_WIDTH: f32 = 6.0;
pub const STEP_RADIUS_PER_WIDTH: f32 = 4.0;

impl Shape {
    pub fn new(kind: ShapeKind, color: Rgba, width: f32) -> Self {
        Self { kind, color, width, fill: false, shadow: false, outline: false }
    }

    /// The drop-shadow companion shape, if this shape casts one.
    pub fn shadow_of(&self) -> Option<Shape> {
        if !self.shadow {
            return None;
        }
        let mut sh = match &self.kind {
            ShapeKind::Highlight { .. }
            | ShapeKind::Blur { .. }
            | ShapeKind::Pixelate { .. }
            | ShapeKind::Spotlight { .. } => return None,
            ShapeKind::Magnify { center, radius, .. } => {
                let mut s = Shape::new(
                    ShapeKind::Ellipse {
                        a: Pt::new(center.x - radius, center.y - radius),
                        b: Pt::new(center.x + radius, center.y + radius),
                    },
                    SHADOW_COLOR,
                    0.0,
                );
                s.fill = true;
                s
            }
            ShapeKind::Step { center, .. } => {
                let r = self.step_radius();
                let mut s = Shape::new(
                    ShapeKind::Ellipse {
                        a: Pt::new(center.x - r, center.y - r),
                        b: Pt::new(center.x + r, center.y + r),
                    },
                    SHADOW_COLOR,
                    0.0,
                );
                s.fill = true;
                s
            }
            _ => {
                let mut s = self.clone();
                s.color = SHADOW_COLOR;
                s.outline = false;
                s
            }
        };
        sh.shadow = false;
        sh.translate(SHADOW_OFFSET, SHADOW_OFFSET);
        Some(sh)
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
            | ShapeKind::Spotlight { a, b }
            | ShapeKind::Pixelate { a, b } => {
                mv(a);
                mv(b);
            }
            ShapeKind::Text { pos, .. } => mv(pos),
            ShapeKind::TextBox { a, b, .. } => {
                mv(a);
                mv(b);
            }
            ShapeKind::Callout { tip, a, b, .. } => {
                mv(tip);
                mv(a);
                mv(b);
            }
            ShapeKind::Step { center, .. } | ShapeKind::Magnify { center, .. } => mv(center),
            ShapeKind::Pen { points } | ShapeKind::Polygon { points } => points.iter_mut().for_each(mv),
        }
    }

    /// Scale every coordinate and the width by `k` (about the origin).
    pub fn scale(&mut self, k: f32) {
        let sc = |p: &mut Pt| {
            p.x *= k;
            p.y *= k;
        };
        match &mut self.kind {
            ShapeKind::Arrow { a, b }
            | ShapeKind::Line { a, b }
            | ShapeKind::Rect { a, b }
            | ShapeKind::Ellipse { a, b }
            | ShapeKind::Highlight { a, b }
            | ShapeKind::Blur { a, b }
            | ShapeKind::Pixelate { a, b }
            | ShapeKind::Spotlight { a, b }
            | ShapeKind::TextBox { a, b, .. } => {
                sc(a);
                sc(b);
            }
            ShapeKind::Text { pos, .. } => sc(pos),
            ShapeKind::Callout { tip, a, b, .. } => {
                sc(tip);
                sc(a);
                sc(b);
            }
            ShapeKind::Step { center, .. } => sc(center),
            ShapeKind::Magnify { center, radius, .. } => {
                sc(center);
                *radius *= k;
            }
            ShapeKind::Pen { points } | ShapeKind::Polygon { points } => points.iter_mut().for_each(sc),
        }
        self.width *= k;
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
            | ShapeKind::Spotlight { a, b }
            | ShapeKind::Pixelate { a, b } => two(a, b),
            ShapeKind::Text { pos, text } => {
                let (w, h) = crate::render::text_size(text, self.text_px());
                (*pos, Pt::new(pos.x + w, pos.y + h))
            }
            ShapeKind::TextBox { a, b, text } => {
                let l = crate::render::textbox_layout(*a, *b, text, self.text_px());
                (l.min, l.max)
            }
            ShapeKind::Callout { tip, a, b, text } => {
                let l = crate::render::callout_layout(*tip, *a, *b, text, self.text_px());
                (
                    Pt::new(l.min.x.min(tip.x), l.min.y.min(tip.y)),
                    Pt::new(l.max.x.max(tip.x), l.max.y.max(tip.y)),
                )
            }
            ShapeKind::Step { center, .. } => {
                let r = self.step_radius();
                (Pt::new(center.x - r, center.y - r), Pt::new(center.x + r, center.y + r))
            }
            ShapeKind::Magnify { center, radius, .. } => {
                (Pt::new(center.x - radius, center.y - radius), Pt::new(center.x + radius, center.y + radius))
            }
            ShapeKind::Pen { points } | ShapeKind::Polygon { points } => {
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
            ShapeKind::Magnify { center, radius, .. } => center.dist(p) <= radius + tol,
            ShapeKind::Highlight { .. }
            | ShapeKind::Blur { .. }
            | ShapeKind::Pixelate { .. }
            | ShapeKind::Spotlight { .. }
            | ShapeKind::Text { .. }
            | ShapeKind::TextBox { .. } => {
                let (min, max) = self.bounds();
                p.x >= min.x - tol && p.x <= max.x + tol && p.y >= min.y - tol && p.y <= max.y + tol
            }
            ShapeKind::Callout { tip, a, b, text } => {
                let l = crate::render::callout_layout(*tip, *a, *b, text, self.text_px());
                let in_bubble =
                    p.x >= l.min.x - tol && p.x <= l.max.x + tol && p.y >= l.min.y - tol && p.y <= l.max.y + tol;
                let c = Pt::new((l.min.x + l.max.x) / 2.0, (l.min.y + l.max.y) / 2.0);
                in_bubble || seg_dist(p, *tip, c) <= self.text_px() * 0.4 + tol
            }
            ShapeKind::Step { center, .. } => center.dist(p) <= self.step_radius() + tol,
            ShapeKind::Polygon { points } => {
                if points.len() < 2 {
                    return points.first().is_some_and(|q| q.dist(p) <= half);
                }
                if points.len() == 2 {
                    return seg_dist(p, points[0], points[1]) <= half;
                }
                let sd = crate::render::sd_polygon(p, points);
                if self.fill { sd <= tol } else { sd.abs() <= half }
            }
            ShapeKind::Pen { points } => {
                if points.len() == 1 {
                    return points[0].dist(p) <= half;
                }
                points.windows(2).any(|w| seg_dist(p, w[0], w[1]) <= half)
            }
        }
    }
}


// ---- resize handles ---------------------------------------------------------

/// What a handle looks like / which cursor it wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleKind {
    Nw,
    N,
    Ne,
    E,
    Se,
    S,
    Sw,
    W,
    /// A free point (line end, polygon vertex).
    Point,
    /// Callout tail tip.
    Tip,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Handle {
    pub pos: Pt,
    pub kind: HandleKind,
}

/// Smallest size (px) a shape can be resized to.
pub const MIN_SIZE: f32 = 4.0;

/// Box handle order: NW, N, NE, E, SE, S, SW, W as `(hx, hy)` in {-1, 0, 1}.
const BOX_H: [(i8, i8); 8] = [(-1, -1), (0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0)];

fn box_idx(hx: i8, hy: i8) -> usize {
    BOX_H.iter().position(|h| *h == (hx, hy)).unwrap_or(0)
}

fn dir_kind(hx: i8, hy: i8) -> HandleKind {
    match (hx, hy) {
        (-1, -1) => HandleKind::Nw,
        (0, -1) => HandleKind::N,
        (1, -1) => HandleKind::Ne,
        (1, 0) => HandleKind::E,
        (1, 1) => HandleKind::Se,
        (0, 1) => HandleKind::S,
        (-1, 1) => HandleKind::Sw,
        _ => HandleKind::W,
    }
}

fn box_handles(min: Pt, max: Pt) -> Vec<Handle> {
    let mid = Pt::new((min.x + max.x) / 2.0, (min.y + max.y) / 2.0);
    let pick = |h: i8, lo: f32, m: f32, hi: f32| match h {
        -1 => lo,
        0 => m,
        _ => hi,
    };
    BOX_H
        .iter()
        .map(|&(hx, hy)| Handle {
            pos: Pt::new(pick(hx, min.x, mid.x, max.x), pick(hy, min.y, mid.y, max.y)),
            kind: dir_kind(hx, hy),
        })
        .collect()
}

/// Move handle `idx` of the box `(min, max)` to `p`. Returns the new box
/// (normalised) and the handle index that now follows the pointer (it flips
/// when dragged across the opposite side). `min_w`/`min_h` bound the size.
fn drag_box(min: Pt, max: Pt, idx: usize, p: Pt, keep_aspect: bool, min_w: f32, min_h: f32) -> (Pt, Pt, usize) {
    let (hx, hy) = BOX_H.get(idx).copied().unwrap_or((1, 1));
    let (w0, h0) = (max.x - min.x, max.y - min.y);
    // fixed edge per axis and where the moving edge wants to go
    let axis = |h: i8, lo: f32, hi: f32, want: f32, minimum: f32| -> (f32, f32, i8) {
        if h == 0 {
            return (lo, hi, 0);
        }
        let fixed = if h < 0 { hi } else { lo };
        let sign = if want > fixed {
            1.0
        } else if want < fixed {
            -1.0
        } else {
            h as f32
        };
        let moved = fixed + sign * (want - fixed).abs().max(minimum);
        let (a, b) = if moved < fixed { (moved, fixed) } else { (fixed, moved) };
        (a, b, if sign > 0.0 { 1 } else { -1 })
    };
    let (mut x0, mut x1, mut nhx) = axis(hx, min.x, max.x, p.x, min_w);
    let (mut y0, mut y1, mut nhy) = axis(hy, min.y, max.y, p.y, min_h);
    if keep_aspect && hx != 0 && hy != 0 && w0 > f32::EPSILON && h0 > f32::EPSILON {
        let s = ((x1 - x0) / w0).max((y1 - y0) / h0);
        let (w, h) = (w0 * s, h0 * s);
        let fx = if hx < 0 { max.x } else { min.x };
        let fy = if hy < 0 { max.y } else { min.y };
        (x0, x1) = if nhx > 0 { (fx, fx + w) } else { (fx - w, fx) };
        (y0, y1) = if nhy > 0 { (fy, fy + h) } else { (fy - h, fy) };
    }
    if hx == 0 {
        nhx = 0;
    }
    if hy == 0 {
        nhy = 0;
    }
    (Pt::new(x0, y0), Pt::new(x1, y1), box_idx(nhx, nhy))
}

impl Shape {
    /// The box a box-like shape (rect, text box, callout bubble...) occupies.
    fn box_rect(&self) -> Option<(Pt, Pt)> {
        match &self.kind {
            ShapeKind::Rect { .. }
            | ShapeKind::Ellipse { .. }
            | ShapeKind::Highlight { .. }
            | ShapeKind::Blur { .. }
            | ShapeKind::Pixelate { .. }
            | ShapeKind::Spotlight { .. }
            | ShapeKind::TextBox { .. } => Some(self.bounds()),
            ShapeKind::Callout { tip, a, b, text } => {
                let l = crate::render::callout_layout(*tip, *a, *b, text, self.text_px());
                Some((l.min, l.max))
            }
            _ => None,
        }
    }

    /// Minimum `(w, h)` for resizable boxes.
    fn min_box(&self) -> (f32, f32) {
        match &self.kind {
            ShapeKind::TextBox { .. } | ShapeKind::Callout { .. } => {
                let px = self.text_px();
                (px * 2.0 + px * crate::render::BOX_PAD_EM * 2.0, MIN_SIZE)
            }
            _ => (MIN_SIZE, MIN_SIZE),
        }
    }

    /// Resize handles of the shape (image px). Empty when it has none.
    /// Indices are what [`drag_handle`](Self::drag_handle) takes.
    pub fn handles(&self) -> Vec<Handle> {
        match &self.kind {
            ShapeKind::Arrow { a, b } | ShapeKind::Line { a, b } => vec![
                Handle { pos: *a, kind: HandleKind::Point },
                Handle { pos: *b, kind: HandleKind::Point },
            ],
            ShapeKind::Rect { .. }
            | ShapeKind::Ellipse { .. }
            | ShapeKind::Highlight { .. }
            | ShapeKind::Blur { .. }
            | ShapeKind::Pixelate { .. }
            | ShapeKind::Spotlight { .. }
            | ShapeKind::TextBox { .. } => {
                let (min, max) = self.bounds();
                box_handles(min, max)
            }
            ShapeKind::Callout { tip, .. } => {
                let mut v = self.box_rect().map(|(min, max)| box_handles(min, max)).unwrap_or_default();
                v.push(Handle { pos: *tip, kind: HandleKind::Tip });
                v
            }
            ShapeKind::Text { .. } => {
                let (_, max) = self.bounds();
                vec![Handle { pos: max, kind: HandleKind::Se }]
            }
            ShapeKind::Step { center, .. } => {
                vec![Handle { pos: Pt::new(center.x + self.step_radius(), center.y), kind: HandleKind::E }]
            }
            ShapeKind::Magnify { center, radius, .. } => {
                vec![Handle { pos: Pt::new(center.x + radius, center.y), kind: HandleKind::E }]
            }
            ShapeKind::Pen { points } if !points.is_empty() => {
                let (min, max) = self.bounds();
                vec![
                    Handle { pos: min, kind: HandleKind::Nw },
                    Handle { pos: Pt::new(max.x, min.y), kind: HandleKind::Ne },
                    Handle { pos: max, kind: HandleKind::Se },
                    Handle { pos: Pt::new(min.x, max.y), kind: HandleKind::Sw },
                ]
            }
            ShapeKind::Pen { .. } => Vec::new(),
            ShapeKind::Polygon { points } => {
                points.iter().map(|p| Handle { pos: *p, kind: HandleKind::Point }).collect()
            }
        }
    }

    /// Drag handle `idx` (from [`handles`](Self::handles)) to `p`.
    /// `keep_aspect` (Shift): 45 degree snap for lines, aspect ratio for box
    /// corners and pen. Returns the index of the handle that now follows the
    /// pointer (it changes when a box/pen is dragged across its opposite side).
    pub fn drag_handle(&mut self, idx: usize, p: Pt, keep_aspect: bool) -> usize {
        let (min_w, min_h) = self.min_box();
        let box_rect = self.box_rect();
        let width = self.width;
        let text_bounds = self.bounds();
        let mut new_width = None;
        let out = match &mut self.kind {
            ShapeKind::Arrow { a, b } | ShapeKind::Line { a, b } => {
                let (fixed, moving) = if idx == 0 { (*b, &mut *a) } else { (*a, &mut *b) };
                let (_, np) =
                    crate::session::constrain_drag(crate::session::DragKind::Segment, fixed, p, keep_aspect, false);
                *moving = np;
                idx
            }
            ShapeKind::TextBox { a, b, .. } | ShapeKind::Callout { a, b, .. } if is_corner(idx) => {
                // Corners scale box and font together (aspect kept); edges reflow.
                let Some((min, max)) = box_rect else { return idx };
                let (nmin, nmax, ni) = drag_box(min, max, idx, p, true, min_w, min_h);
                let k = (nmax.x - nmin.x) / (max.x - min.x).max(f32::EPSILON);
                (*a, *b) = (nmin, nmax);
                new_width = Some((width * k).max(1.0));
                ni
            }
            ShapeKind::Rect { a, b }
            | ShapeKind::Ellipse { a, b }
            | ShapeKind::Highlight { a, b }
            | ShapeKind::Blur { a, b }
            | ShapeKind::Pixelate { a, b }
            | ShapeKind::Spotlight { a, b }
            | ShapeKind::TextBox { a, b, .. } => {
                let Some((min, max)) = box_rect else { return idx };
                let (nmin, nmax, ni) = drag_box(min, max, idx, p, keep_aspect, min_w, min_h);
                (*a, *b) = (nmin, nmax);
                ni
            }
            ShapeKind::Callout { tip, a, b, .. } => {
                if idx >= 8 {
                    *tip = p;
                    return idx;
                }
                let Some((min, max)) = box_rect else { return idx };
                let (nmin, nmax, ni) = drag_box(min, max, idx, p, keep_aspect, min_w, min_h);
                (*a, *b) = (nmin, nmax);
                ni
            }
            ShapeKind::Text { .. } => {
                let (min, max) = text_bounds;
                let (w, h) = (max.x - min.x, max.y - min.y);
                if w > f32::EPSILON && h > f32::EPSILON {
                    let s = ((p.x - min.x) / w + (p.y - min.y) / h) / 2.0;
                    self.width = (width * s).max(1.0);
                }
                idx
            }
            ShapeKind::Magnify { center, radius, .. } => {
                *radius = center.dist(p).max(MIN_SIZE);
                idx
            }
            ShapeKind::Step { center, .. } => {
                self.width = (center.dist(p) / STEP_RADIUS_PER_WIDTH).max(6.0 / STEP_RADIUS_PER_WIDTH);
                idx
            }
            ShapeKind::Pen { points } => {
                if points.is_empty() {
                    return idx;
                }
                let (min, max) = text_bounds;
                // corners: 0 NW, 1 NE, 2 SE, 3 SW
                let (hx, hy): (i8, i8) = match idx {
                    0 => (-1, -1),
                    1 => (1, -1),
                    2 => (1, 1),
                    _ => (-1, 1),
                };
                let fixed = Pt::new(if hx < 0 { max.x } else { min.x }, if hy < 0 { max.y } else { min.y });
                let held = Pt::new(if hx < 0 { min.x } else { max.x }, if hy < 0 { min.y } else { max.y });
                let scale_of = |want: f32, fixed: f32, held: f32| -> f32 {
                    let ext = held - fixed;
                    if ext.abs() < MIN_SIZE {
                        return 1.0;
                    }
                    let s = (want - fixed) / ext;
                    let floor = MIN_SIZE / ext.abs();
                    if s.abs() < floor {
                        floor * if s < 0.0 { -1.0 } else { 1.0 }
                    } else {
                        s
                    }
                };
                let (mut sx, mut sy) = (scale_of(p.x, fixed.x, held.x), scale_of(p.y, fixed.y, held.y));
                if keep_aspect {
                    let s = sx.abs().max(sy.abs());
                    sx = s * if sx < 0.0 { -1.0 } else { 1.0 };
                    sy = s * if sy < 0.0 { -1.0 } else { 1.0 };
                }
                for q in points.iter_mut() {
                    q.x = fixed.x + (q.x - fixed.x) * sx;
                    q.y = fixed.y + (q.y - fixed.y) * sy;
                }
                let nhx = if sx < 0.0 { -hx } else { hx };
                let nhy = if sy < 0.0 { -hy } else { hy };
                match (nhx, nhy) {
                    (-1, -1) => 0,
                    (1, -1) => 1,
                    (1, 1) => 2,
                    _ => 3,
                }
            }
            ShapeKind::Polygon { points } => {
                if let Some(q) = points.get_mut(idx) {
                    *q = p;
                }
                idx
            }
        };
        if let Some(w) = new_width {
            self.width = w;
        }
        out
    }
}

/// Corner handles of [`box_handles`] (both directions non-zero).
fn is_corner(idx: usize) -> bool {
    BOX_H.get(idx).is_some_and(|(x, y)| *x != 0 && *y != 0)
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
    effects: EffectSpec,
    rev: u64,
}

/// Label for the step that turned snapshot `a` into `b`.
fn describe_step(a: &Snapshot, b: &Snapshot) -> String {
    let (sa, sb) = (&a.shapes, &b.shapes);
    if sb.len() > sa.len() {
        let n = sb.len() - sa.len();
        return match sb.last() {
            Some(s) if n == 1 => format!("Tambah {}", s.kind.display_name()),
            _ => format!("Tambah {n} objek"),
        };
    }
    if sb.len() < sa.len() {
        let n = sa.len() - sb.len();
        if n == 1 {
            let at = (0..sb.len()).find(|&i| sa[i] != sb[i]).unwrap_or(sb.len());
            return format!("Hapus {}", sa[at].kind.display_name());
        }
        return format!("Hapus {n} objek");
    }
    let changed: Vec<usize> = (0..sa.len()).filter(|&i| sa[i] != sb[i]).collect();
    if changed.is_empty() {
        if a.effects != b.effects && a.crop == b.crop {
            return "Efek".to_string();
        }
        return match (a.crop == b.crop, b.crop) {
            (false, None) => "Reset crop".to_string(),
            (false, Some(_)) => "Crop".to_string(),
            (true, _) => "Ubah".to_string(),
        };
    }
    if changed.iter().all(|&i| sb.contains(&sa[i])) && changed.iter().all(|&i| sa.contains(&sb[i])) {
        return "Urutan".to_string();
    }
    if let [i] = changed[..] {
        let (x, y) = (&sa[i], &sb[i]);
        let name = y.kind.display_name();
        let style_same = x.color == y.color && x.width == y.width && x.fill == y.fill && x.shadow == y.shadow && x.outline == y.outline;
        if x.kind == y.kind {
            return format!("Ubah gaya {name}");
        }
        if x.kind.text() != y.kind.text() {
            return "Edit teks".to_string();
        }
        return if style_same { format!("Ubah {name}") } else { format!("Ubah {name} & gaya") };
    }
    format!("Ubah {} objek", changed.len())
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
            cur: Snapshot { shapes: Vec::new(), crop: None, effects: EffectSpec::default(), rev: 0 },
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

    /// Output effects (rounded corners, border, shadow, torn edges,
    /// watermark) applied after crop + annotations on export.
    pub fn effects(&self) -> &EffectSpec {
        &self.cur.effects
    }

    /// Replace the effects (one undo step). No-op when unchanged.
    pub fn set_effects(&mut self, spec: EffectSpec) {
        if self.cur.effects == spec {
            return;
        }
        self.begin_edit();
        self.cur.effects = spec;
    }

    /// Mutate the effects **without** an undo point; call
    /// [`begin_edit`](Self::begin_edit) first (once per slider drag).
    pub fn effects_live(&mut self) -> &mut EffectSpec {
        self.bump();
        &mut self.cur.effects
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

    /// Number of states in the history (the original + one per undo step +
    /// the redo states).
    pub fn history_len(&self) -> usize {
        self.undo.len() + 1 + self.redo.len()
    }

    /// Index of the current state in the history (0 = original).
    pub fn history_pos(&self) -> usize {
        self.undo.len()
    }

    /// One label per history state: "Buka" for the original, then what each
    /// step changed (derived by comparing consecutive snapshots).
    pub fn history_labels(&self) -> Vec<String> {
        let states: Vec<&Snapshot> = self.undo.iter().chain(std::iter::once(&self.cur)).chain(self.redo.iter().rev()).collect();
        let mut out = Vec::with_capacity(states.len());
        out.push("Buka".to_string());
        for w in states.windows(2) {
            out.push(describe_step(w[0], w[1]));
        }
        out
    }

    /// Jump to history state `idx` (clamped), undoing/redoing as many steps
    /// as needed. Revisions of the snapshots are kept, so dirtiness stays
    /// correct. Returns whether the state changed.
    pub fn jump_to(&mut self, idx: usize) -> bool {
        let idx = idx.min(self.history_len() - 1);
        let from = self.history_pos();
        while self.undo.len() > idx {
            self.undo();
        }
        while self.undo.len() < idx && self.redo() {}
        self.history_pos() != from
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

    /// Remove several shapes in one undo step.
    pub fn remove_shapes(&mut self, idxs: &[usize]) {
        let mut v: Vec<usize> = idxs.iter().copied().filter(|i| *i < self.cur.shapes.len()).collect();
        if v.is_empty() {
            return;
        }
        v.sort_unstable();
        v.dedup();
        self.begin_edit();
        for i in v.into_iter().rev() {
            self.cur.shapes.remove(i);
        }
    }

    /// Duplicate shapes (offset by `offset` px, appended on top). Returns the
    /// new indices. One undo step.
    pub fn duplicate_shapes(&mut self, idxs: &[usize], offset: f32) -> Vec<usize> {
        let mut v: Vec<usize> = idxs.iter().copied().filter(|i| *i < self.cur.shapes.len()).collect();
        v.sort_unstable();
        v.dedup();
        if v.is_empty() {
            return Vec::new();
        }
        self.begin_edit();
        let mut out = Vec::new();
        for i in v {
            let mut s = self.cur.shapes[i].clone();
            s.translate(offset, offset);
            self.cur.shapes.push(s);
            out.push(self.cur.shapes.len() - 1);
        }
        out
    }

    /// Move shapes by (dx, dy) in one undo step.
    pub fn nudge_shapes(&mut self, idxs: &[usize], dx: f32, dy: f32) {
        if idxs.is_empty() {
            return;
        }
        self.begin_edit();
        for i in idxs {
            if let Some(s) = self.cur.shapes.get_mut(*i) {
                s.translate(dx, dy);
            }
        }
    }

    /// Mutate several shapes in one undo step.
    pub fn edit_shapes(&mut self, idxs: &[usize], mut f: impl FnMut(&mut Shape)) {
        let ok: Vec<usize> = idxs.iter().copied().filter(|i| *i < self.cur.shapes.len()).collect();
        if ok.is_empty() {
            return;
        }
        self.begin_edit();
        for i in ok {
            f(&mut self.cur.shapes[i]);
        }
    }

    /// Move shapes to the top of the stack (order kept). Returns new indices.
    pub fn bring_to_front(&mut self, idxs: &[usize]) -> Vec<usize> {
        self.reorder(idxs, true)
    }

    /// Move shapes to the bottom of the stack (order kept). Returns new indices.
    pub fn send_to_back(&mut self, idxs: &[usize]) -> Vec<usize> {
        self.reorder(idxs, false)
    }

    fn reorder(&mut self, idxs: &[usize], front: bool) -> Vec<usize> {
        let n = self.cur.shapes.len();
        let mut sel: Vec<usize> = idxs.iter().copied().filter(|i| *i < n).collect();
        sel.sort_unstable();
        sel.dedup();
        if sel.is_empty() {
            return sel;
        }
        let k = sel.len();
        let target: Vec<usize> = if front { (n - k..n).collect() } else { (0..k).collect() };
        if sel == target {
            return sel;
        }
        self.begin_edit();
        let shapes = std::mem::take(&mut self.cur.shapes);
        let (mut picked, mut rest) = (Vec::new(), Vec::new());
        for (i, s) in shapes.into_iter().enumerate() {
            if sel.binary_search(&i).is_ok() {
                picked.push(s);
            } else {
                rest.push(s);
            }
        }
        self.cur.shapes = if front {
            rest.extend(picked);
            rest
        } else {
            picked.extend(rest);
            picked
        };
        target
    }

    /// Move the shape at `from` so it ends up at index `to` (one undo step).
    /// Returns false (no undo point) when nothing would change.
    pub fn move_shape(&mut self, from: usize, to: usize) -> bool {
        let n = self.cur.shapes.len();
        if from >= n || to >= n || from == to {
            return false;
        }
        self.begin_edit();
        let s = self.cur.shapes.remove(from);
        self.cur.shapes.insert(to, s);
        true
    }

    /// Renumber all step shapes 1..n in stacking order. Returns whether
    /// anything changed (one undo step if so).
    pub fn reset_steps(&mut self) -> bool {
        let mut want = 1;
        let mut changed = false;
        for s in &self.cur.shapes {
            if let ShapeKind::Step { n, .. } = s.kind {
                changed |= n != want;
                want += 1;
            }
        }
        if !changed {
            return false;
        }
        self.begin_edit();
        let mut k = 1;
        for s in &mut self.cur.shapes {
            if let ShapeKind::Step { n, .. } = &mut s.kind {
                *n = k;
                k += 1;
            }
        }
        true
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

    #[test]
    fn textbox_corner_scales_font_edge_reflows() {
        let mut s = Shape::new(ShapeKind::TextBox { a: Pt::new(0.0, 0.0), b: Pt::new(200.0, 50.0), text: "hi".into() }, [0; 4], 4.0);
        let se = box_idx(1, 1);
        let (_, max) = s.bounds();
        s.drag_handle(se, Pt::new(max.x * 2.0, max.y * 2.0), false);
        assert!((s.width - 8.0).abs() < 0.05, "corner doubles font: {}", s.width);
        let w = s.width;
        let e = box_idx(1, 0);
        s.drag_handle(e, Pt::new(600.0, 10.0), false);
        assert_eq!(s.width, w, "edge keeps font size");
        let (min, max) = s.bounds();
        assert!((max.x - min.x - 600.0).abs() < 0.5);
    }

    #[test]
    fn callout_corner_scales_font_keeps_tip() {
        let tip = Pt::new(0.0, 0.0);
        let mut s = Shape::new(ShapeKind::Callout { tip, a: Pt::new(50.0, 50.0), b: Pt::new(250.0, 120.0), text: "hi".into() }, [0; 4], 4.0);
        let hs = s.handles();
        let se = box_idx(1, 1);
        let from = hs[se].pos;
        s.drag_handle(se, Pt::new(50.0 + (from.x - 50.0) * 1.5, 50.0 + (from.y - 50.0) * 1.5), false);
        assert!(s.width > 5.0, "font grew: {}", s.width);
        assert!(matches!(s.kind, ShapeKind::Callout { tip: t, .. } if t == tip));
    }

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Shape {
        Shape::new(
            ShapeKind::Rect { a: Pt::new(x0, y0), b: Pt::new(x1, y1) },
            [255, 0, 0, 255],
            2.0,
        )
    }

    #[test]
    fn polygon_hit_edge_and_fill() {
        let pts = vec![Pt::new(0.0, 0.0), Pt::new(20.0, 0.0), Pt::new(20.0, 40.0), Pt::new(60.0, 40.0), Pt::new(60.0, 60.0), Pt::new(0.0, 60.0)];
        let mut s = Shape::new(ShapeKind::Polygon { points: pts }, [0, 0, 0, 255], 2.0);
        assert!(s.hit(Pt::new(0.0, 30.0), 3.0)); // on edge
        assert!(!s.hit(Pt::new(10.0, 20.0), 3.0)); // inside, hollow
        s.fill = true;
        assert!(s.hit(Pt::new(10.0, 20.0), 3.0));
        assert!(!s.hit(Pt::new(40.0, 10.0), 3.0)); // concave notch
        assert!(!s.hit(Pt::new(-30.0, 30.0), 3.0));
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
    fn multi_ops_are_single_undo_steps() {
        let mut d = Document::new();
        for i in 0..4 {
            d.add_shape(rect(0.0, 0.0, 1.0 + i as f32, 1.0));
        }
        d.remove_shapes(&[0, 2]);
        assert_eq!(d.shapes().len(), 2);
        d.undo();
        assert_eq!(d.shapes().len(), 4);
        let new = d.duplicate_shapes(&[1, 3], 10.0);
        assert_eq!(new, vec![4, 5]);
        assert_eq!(d.shapes()[4].bounds().0, Pt::new(10.0, 10.0));
        d.undo();
        assert_eq!(d.shapes().len(), 4);
        d.nudge_shapes(&[0, 1], 2.0, 3.0);
        assert_eq!(d.shapes()[1].bounds().0, Pt::new(2.0, 3.0));
        d.undo();
        assert_eq!(d.shapes()[1].bounds().0, Pt::new(0.0, 0.0));
    }

    #[test]
    fn layer_order() {
        let mut d = Document::new();
        for i in 0..4 {
            d.add_shape(rect(0.0, 0.0, 10.0 * (i + 1) as f32, 1.0));
        }
        let w = |d: &Document| -> Vec<i32> { d.shapes().iter().map(|s| s.bounds().1.x as i32).collect() };
        let new = d.bring_to_front(&[0, 1]);
        assert_eq!(w(&d), vec![30, 40, 10, 20]);
        assert_eq!(new, vec![2, 3]);
        let new = d.send_to_back(&[3]);
        assert_eq!(w(&d), vec![20, 30, 40, 10]);
        assert_eq!(new, vec![0]);
        // already at front: no undo step
        let before = d.revision();
        d.bring_to_front(&[3]);
        assert_eq!(d.revision(), before);
    }

    #[test]
    fn reset_steps_renumbers() {
        let mut d = Document::new();
        for n in [3, 7, 9] {
            d.add_shape(Shape::new(ShapeKind::Step { center: Pt::new(1.0, 1.0), n }, [0; 4], 4.0));
        }
        assert!(d.reset_steps());
        let ns: Vec<u32> = d
            .shapes()
            .iter()
            .filter_map(|s| match s.kind {
                ShapeKind::Step { n, .. } => Some(n),
                _ => None,
            })
            .collect();
        assert_eq!(ns, vec![1, 2, 3]);
        assert!(!d.reset_steps());
        d.undo();
        assert_eq!(d.next_step_number(), 10);
    }

    #[test]
    fn shadow_companion() {
        let mut s = rect(10.0, 10.0, 20.0, 20.0);
        assert!(s.shadow_of().is_none());
        s.shadow = true;
        let sh = s.shadow_of().unwrap();
        assert_eq!(sh.bounds().0, Pt::new(13.0, 13.0));
        assert!(!sh.shadow);
        let mut hl = Shape::new(ShapeKind::Highlight { a: Pt::new(0.0, 0.0), b: Pt::new(5.0, 5.0) }, [0; 4], 1.0);
        hl.shadow = true;
        assert!(hl.shadow_of().is_none());
    }

    #[test]
    fn tool_ids_roundtrip() {
        for t in Tool::ALL {
            assert_eq!(Tool::from_id(t.id()), Some(t));
        }
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

    // ---- handles ----

    fn pt(x: f32, y: f32) -> Pt {
        Pt::new(x, y)
    }

    fn mk(kind: ShapeKind) -> Shape {
        Shape::new(kind, [0, 0, 0, 255], 3.0)
    }

    #[test]
    fn line_handles_move_endpoint_and_snap() {
        let mut s = mk(ShapeKind::Line { a: pt(0.0, 0.0), b: pt(10.0, 0.0) });
        assert_eq!(s.handles().len(), 2);
        s.drag_handle(1, pt(50.0, 7.0), false);
        assert_eq!(s.kind, ShapeKind::Line { a: pt(0.0, 0.0), b: pt(50.0, 7.0) });
        s.drag_handle(1, pt(50.0, 7.0), true);
        if let ShapeKind::Line { a, b } = s.kind {
            assert_eq!(a, pt(0.0, 0.0));
            assert!(b.y.abs() < 1e-3 && b.x > 49.0);
        }
        s.drag_handle(0, pt(-5.0, -5.0), false);
        if let ShapeKind::Line { a, .. } = s.kind {
            assert_eq!(a, pt(-5.0, -5.0));
        }
    }

    #[test]
    fn rect_corner_and_edge_resize() {
        let mut s = rect(10.0, 10.0, 50.0, 30.0);
        let h = s.handles();
        assert_eq!(h.len(), 8);
        assert_eq!(h[0].kind, HandleKind::Nw);
        assert_eq!(h[0].pos, pt(10.0, 10.0));
        assert_eq!(h[4].pos, pt(50.0, 30.0));
        assert_eq!(h[1].pos, pt(30.0, 10.0));
        // SE corner
        assert_eq!(s.drag_handle(4, pt(80.0, 60.0), false), 4);
        assert_eq!(s.bounds(), (pt(10.0, 10.0), pt(80.0, 60.0)));
        // E edge only changes x
        s.drag_handle(3, pt(100.0, 999.0), false);
        assert_eq!(s.bounds(), (pt(10.0, 10.0), pt(100.0, 60.0)));
        // N edge only changes y
        s.drag_handle(1, pt(-9.0, 20.0), false);
        assert_eq!(s.bounds(), (pt(10.0, 20.0), pt(100.0, 60.0)));
    }

    #[test]
    fn rect_flip_and_min_size() {
        let mut s = rect(10.0, 10.0, 50.0, 30.0);
        // drag E handle past the west edge: flips to the W handle
        let ni = s.drag_handle(3, pt(0.0, 20.0), false);
        assert_eq!(ni, 7);
        assert_eq!(s.bounds(), (pt(0.0, 10.0), pt(10.0, 30.0)));
        // keep dragging with the new index: stays consistent
        s.drag_handle(ni, pt(-20.0, 20.0), false);
        assert_eq!(s.bounds(), (pt(-20.0, 10.0), pt(10.0, 30.0)));
        // collapse is prevented
        let mut s = rect(10.0, 10.0, 50.0, 30.0);
        s.drag_handle(4, pt(10.5, 10.5), false);
        let (min, max) = s.bounds();
        assert!(max.x - min.x >= MIN_SIZE && max.y - min.y >= MIN_SIZE);
    }

    #[test]
    fn rect_corner_keeps_aspect_with_shift() {
        let mut s = rect(0.0, 0.0, 40.0, 20.0);
        s.drag_handle(4, pt(100.0, 30.0), true);
        let (min, max) = s.bounds();
        assert!(((max.x - min.x) / (max.y - min.y) - 2.0).abs() < 1e-3, "{min:?} {max:?}");
        assert_eq!(min, pt(0.0, 0.0));
        // flipped corner with aspect: dragged to the NW of the fixed corner
        let mut s = rect(0.0, 0.0, 40.0, 20.0);
        let ni = s.drag_handle(4, pt(-40.0, -10.0), true);
        assert_eq!(ni, 0);
        let (min, max) = s.bounds();
        assert!(((max.x - min.x) / (max.y - min.y) - 2.0).abs() < 1e-3);
        assert_eq!(max, pt(0.0, 0.0));
    }

    #[test]
    fn ellipse_highlight_blur_pixelate_have_eight_handles() {
        for k in [
            ShapeKind::Ellipse { a: pt(0.0, 0.0), b: pt(10.0, 10.0) },
            ShapeKind::Highlight { a: pt(0.0, 0.0), b: pt(10.0, 10.0) },
            ShapeKind::Blur { a: pt(0.0, 0.0), b: pt(10.0, 10.0) },
            ShapeKind::Pixelate { a: pt(0.0, 0.0), b: pt(10.0, 10.0) },
        ] {
            let mut s = mk(k);
            assert_eq!(s.handles().len(), 8);
            s.drag_handle(2, pt(30.0, -20.0), false);
            assert_eq!(s.bounds(), (pt(0.0, -20.0), pt(30.0, 10.0)));
        }
    }

    #[test]
    fn textbox_resize_has_min_width() {
        let mut s = mk(ShapeKind::TextBox { a: pt(0.0, 0.0), b: pt(200.0, 30.0), text: "hello".into() });
        assert_eq!(s.handles().len(), 8);
        s.drag_handle(3, pt(2.0, 10.0), false);
        let (min, max) = s.bounds();
        assert!(max.x - min.x >= s.text_px() * 2.0, "{}", max.x - min.x);
        // handles sit on the (grown) layout box
        let h = s.handles();
        assert_eq!(h[4].pos, max);
    }

    #[test]
    fn text_handle_scales_font() {
        let mut s = mk(ShapeKind::Text { pos: pt(10.0, 10.0), text: "Hello".into() });
        let h = s.handles();
        assert_eq!(h.len(), 1);
        let (min, max) = s.bounds();
        assert_eq!(h[0].pos, max);
        let (w, hh) = (max.x - min.x, max.y - min.y);
        s.drag_handle(0, pt(min.x + 2.0 * w, min.y + 2.0 * hh), false);
        assert!((s.width - 6.0).abs() < 0.1, "{}", s.width);
        s.drag_handle(0, pt(min.x + 1.0, min.y + 1.0), false);
        assert!(s.width >= 1.0);
    }

    #[test]
    fn step_handle_changes_radius() {
        let mut s = mk(ShapeKind::Step { center: pt(50.0, 50.0), n: 1 });
        let h = s.handles();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].pos, pt(50.0 + s.step_radius(), 50.0));
        s.drag_handle(0, pt(50.0, 90.0), false);
        assert!((s.step_radius() - 40.0).abs() < 1e-3);
        s.drag_handle(0, pt(50.0, 50.0), false);
        assert!(s.step_radius() >= 6.0);
    }

    #[test]
    fn pen_corners_scale_points() {
        let mut s = mk(ShapeKind::Pen { points: vec![pt(0.0, 0.0), pt(10.0, 20.0), pt(20.0, 10.0)] });
        let h = s.handles();
        assert_eq!(h.len(), 4);
        assert_eq!(h[2].pos, pt(20.0, 20.0));
        // SE x2
        assert_eq!(s.drag_handle(2, pt(40.0, 40.0), false), 2);
        assert_eq!(s.kind, ShapeKind::Pen { points: vec![pt(0.0, 0.0), pt(20.0, 40.0), pt(40.0, 20.0)] });
        // dragging NW across the SE corner flips the handle index
        let ni = s.drag_handle(0, pt(50.0, 50.0), false);
        assert_eq!(ni, 2);
        let (min, max) = s.bounds();
        assert!(max.x - min.x >= MIN_SIZE - 1e-3);
    }

    #[test]
    fn polygon_vertices_are_handles() {
        let mut s = mk(ShapeKind::Polygon { points: vec![pt(0.0, 0.0), pt(10.0, 0.0), pt(5.0, 9.0)] });
        assert_eq!(s.handles().len(), 3);
        assert!(s.handles().iter().all(|h| h.kind == HandleKind::Point));
        s.drag_handle(2, pt(6.0, 20.0), false);
        assert_eq!(s.kind, ShapeKind::Polygon { points: vec![pt(0.0, 0.0), pt(10.0, 0.0), pt(6.0, 20.0)] });
        // out-of-range index is harmless
        s.drag_handle(9, pt(1.0, 1.0), false);
    }

    #[test]
    fn callout_box_and_tip_handles() {
        let mut s = mk(ShapeKind::Callout {
            tip: pt(0.0, 100.0),
            a: pt(40.0, 60.0),
            b: pt(240.0, 20.0),
            text: "Klik".into(),
        });
        let h = s.handles();
        assert_eq!(h.len(), 9);
        assert_eq!(h[8].kind, HandleKind::Tip);
        assert_eq!(h[8].pos, pt(0.0, 100.0));
        // tip moves freely
        assert_eq!(s.drag_handle(8, pt(-30.0, 120.0), false), 8);
        if let ShapeKind::Callout { tip, .. } = &s.kind {
            assert_eq!(*tip, pt(-30.0, 120.0));
        }
        // widen the box through the E handle
        let h = s.handles();
        s.drag_handle(3, pt(h[3].pos.x + 50.0, 40.0), false);
        let hh = s.handles();
        assert!((hh[3].pos.x - (h[3].pos.x + 50.0)).abs() < 1e-3);
        assert!((hh[7].pos.x - h[7].pos.x).abs() < 1e-3);
    }

    #[test]
    fn object_labels_number_per_kind() {
        let arrow = || mk(ShapeKind::Arrow { a: pt(0.0, 0.0), b: pt(1.0, 1.0) });
        let rect = || mk(ShapeKind::Rect { a: pt(0.0, 0.0), b: pt(1.0, 1.0) });
        let text = |t: &str| mk(ShapeKind::Text { pos: pt(0.0, 0.0), text: t.into() });
        let step = |n| mk(ShapeKind::Step { center: pt(0.0, 0.0), n });
        let shapes = vec![arrow(), rect(), arrow(), text("Halo\ndunia"), step(7), text("  "), text(&"x".repeat(60))];
        let l = object_labels(&shapes);
        assert_eq!(l[0], "Panah 1");
        assert_eq!(l[1], "Kotak 1");
        assert_eq!(l[2], "Panah 2");
        assert_eq!(l[3], "Halo");
        assert_eq!(l[4], "Nomor 7");
        assert_eq!(l[5], "Teks 2");
        assert_eq!(l[6].chars().count(), 40);
        assert!(l[6].ends_with('\u{2026}'));
    }

    #[test]
    fn move_shape_reorders_with_one_undo() {
        let r = |x: f32| mk(ShapeKind::Rect { a: pt(x, 0.0), b: pt(x + 1.0, 1.0) });
        let mut d = Document::new();
        for x in [0.0, 10.0, 20.0] {
            d.add_shape(r(x));
        }
        assert!(!d.move_shape(1, 1));
        assert!(!d.move_shape(5, 0));
        assert!(d.move_shape(0, 2));
        let xs: Vec<f32> = d.shapes().iter().map(|s| s.bounds().0.x).collect();
        assert_eq!(xs, vec![10.0, 20.0, 0.0]);
        assert!(d.undo());
        let xs: Vec<f32> = d.shapes().iter().map(|s| s.bounds().0.x).collect();
        assert_eq!(xs, vec![0.0, 10.0, 20.0]);
    }

    #[test]
    fn history_labels_and_jump() {
        let r = |x: f32| mk(ShapeKind::Rect { a: pt(x, 0.0), b: pt(x + 1.0, 1.0) });
        let mut d = Document::new();
        assert_eq!(d.history_labels(), vec!["Buka"]);
        d.add_shape(r(0.0));
        d.add_shape(mk(ShapeKind::Text { pos: pt(1.0, 1.0), text: "a".into() }));
        d.set_crop(Some(jepret_core::Rect::new(0, 0, 5, 5)));
        d.edit_shape(1, |s| {
            if let Some(t) = s.kind.text_mut() {
                *t = "b".into();
            }
        });
        d.edit_shape(0, |s| s.color = [1, 2, 3, 255]);
        d.move_shape(0, 1);
        d.remove_shape(0);
        d.set_crop(None);
        assert_eq!(
            d.history_labels(),
            vec!["Buka", "Tambah Kotak", "Tambah Teks", "Crop", "Edit teks", "Ubah gaya Kotak", "Urutan", "Hapus Teks", "Reset crop"]
        );
        let n = d.history_len();
        assert_eq!(d.history_pos(), n - 1);
        let rev_end = d.revision();
        d.mark_saved();
        // jump back several steps
        assert!(d.jump_to(2));
        assert_eq!(d.shapes().len(), 2);
        assert!(d.crop().is_none());
        assert!(d.is_dirty());
        assert_eq!(d.history_len(), n);
        assert_eq!(d.history_pos(), 2);
        // jump forward past the end clamps
        assert!(d.jump_to(99));
        assert_eq!(d.history_pos(), n - 1);
        assert_eq!(d.revision(), rev_end);
        assert!(!d.is_dirty());
        assert!(!d.jump_to(n - 1));
        d.jump_to(0);
        assert!(d.shapes().is_empty());
        // a new edit drops the redo tail
        d.add_shape(r(5.0));
        assert_eq!(d.history_len(), 2);
    }

    #[test]
    fn spotlight_and_magnify_hit() {
        let sp = Shape::new(ShapeKind::Spotlight { a: Pt::new(10.0, 10.0), b: Pt::new(50.0, 40.0) }, [0; 4], 1.0);
        assert!(sp.hit(Pt::new(30.0, 25.0), 2.0));
        assert!(!sp.hit(Pt::new(70.0, 25.0), 2.0));
        let mg = Shape::new(ShapeKind::Magnify { center: Pt::new(100.0, 100.0), radius: 30.0, zoom: 2.0 }, [0; 4], 3.0);
        assert!(mg.hit(Pt::new(100.0, 100.0), 2.0), "centre");
        assert!(mg.hit(Pt::new(125.0, 100.0), 2.0), "inside");
        assert!(!mg.hit(Pt::new(140.0, 100.0), 2.0), "outside");
        let mut d = Document::new();
        d.add_shape(sp);
        d.add_shape(mg);
        assert_eq!(d.hit_test(Pt::new(100.0, 100.0), 2.0), Some(1));
        assert_eq!(d.hit_test(Pt::new(20.0, 20.0), 2.0), Some(0));
    }

    #[test]
    fn magnify_handle_resizes_radius_and_names() {
        let mut mg = Shape::new(ShapeKind::Magnify { center: Pt::new(50.0, 50.0), radius: 20.0, zoom: 3.0 }, [0; 4], 3.0);
        let hs = mg.handles();
        assert_eq!(hs.len(), 1);
        assert_eq!(hs[0].pos, Pt::new(70.0, 50.0));
        mg.drag_handle(0, Pt::new(50.0, 110.0), false);
        assert!(matches!(mg.kind, ShapeKind::Magnify { radius, zoom, .. } if (radius - 60.0).abs() < 1e-3 && zoom == 3.0));
        let sp = Shape::new(ShapeKind::Spotlight { a: Pt::new(0.0, 0.0), b: Pt::new(5.0, 5.0) }, [0; 4], 1.0);
        let labels = object_labels(&[sp.clone(), mg.clone(), sp, mg]);
        assert_eq!(labels, ["Sorotan 1", "Kaca pembesar 1", "Sorotan 2", "Kaca pembesar 2"]);
    }

    #[test]
    fn history_labels_for_new_tools_and_effects() {
        let mut d = Document::new();
        d.add_shape(Shape::new(ShapeKind::Spotlight { a: Pt::new(0.0, 0.0), b: Pt::new(9.0, 9.0) }, [0; 4], 1.0));
        d.add_shape(Shape::new(ShapeKind::Magnify { center: Pt::new(5.0, 5.0), radius: 8.0, zoom: 2.0 }, [0; 4], 1.0));
        let spec = EffectSpec { corner_radius: 12, ..EffectSpec::default() };
        d.set_effects(spec);
        assert_eq!(d.history_labels(), ["Buka", "Tambah Sorotan", "Tambah Kaca pembesar", "Efek"]);
    }

    #[test]
    fn effects_undo_redo_and_live_edit_is_one_step() {
        let mut d = Document::new();
        assert!(d.effects().is_noop());
        let spec = EffectSpec { border_width: 4, ..EffectSpec::default() };
        d.set_effects(spec.clone());
        d.set_effects(spec); // unchanged: no new step
        assert_eq!(d.history_len(), 2);
        d.begin_edit();
        for r in 1..=20 {
            d.effects_live().corner_radius = r;
        }
        assert_eq!(d.history_len(), 3, "a drag is one undo step");
        assert_eq!(d.effects().corner_radius, 20);
        assert!(d.undo());
        assert_eq!(d.effects().corner_radius, 0);
        assert_eq!(d.effects().border_width, 4);
        assert!(d.undo());
        assert!(d.effects().is_noop());
        assert!(d.redo() && d.redo());
        assert_eq!(d.effects().corner_radius, 20);
        assert!(d.is_dirty());
    }
}
