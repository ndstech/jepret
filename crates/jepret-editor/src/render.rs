//! Flatten a [`Document`] onto an [`Image`] (save / clipboard). Pure CPU,
//! no egui. Anti-aliasing via signed-distance coverage; blur/pixelate read
//! from the *original* pixels.

use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use jepret_core::{Image, Rect};

use crate::doc::{Document, Pt, Rgba, Shape, ShapeKind};

/// The UI font (Segoe UI Variable / Segoe UI) so saved text looks like the
/// on-screen text; egui's bundled Ubuntu Light as the fallback.
fn font() -> Option<&'static FontRef<'static>> {
    use std::sync::OnceLock;
    static FONT: OnceLock<Option<FontRef<'static>>> = OnceLock::new();
    FONT.get_or_init(|| {
        jepret_ui_theme::fonts::ui_font()
            .and_then(|(_, b)| FontRef::try_from_slice(b).ok())
            .or_else(|| FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).ok())
    })
    .as_ref()
}

/// Line height in px at font size `px`.
/// egui sizes a font by its em (`size * height / units_per_em` px) while
/// this renderer sizes by line height (`PxScale(px)`). Multiply a render
/// size by this to get the egui size that draws the same glyphs.
pub fn egui_font_factor() -> f32 {
    font()
        .and_then(|f| f.units_per_em().map(|u| u / f.height_unscaled()))
        .filter(|k| k.is_finite() && *k > 0.0)
        .unwrap_or(1.0)
}

pub fn line_height(px: f32) -> f32 {
    match font() {
        Some(f) => {
            let sf = f.as_scaled(PxScale::from(px));
            sf.height() + sf.line_gap()
        }
        None => px * 1.2,
    }
}

/// Width in px of a single line.
pub fn line_px_width(line: &str, px: f32) -> f32 {
    match font() {
        Some(f) => line_width(&f.as_scaled(PxScale::from(px)), line),
        None => px * 0.5 * line.chars().count() as f32,
    }
}

/// Greedy word wrap to `max_w` px. `\n` always breaks; words longer than a
/// line are split between characters. Never returns an empty vec.
pub fn wrap_text(text: &str, px: f32, max_w: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for (wi, word) in para.split(' ').enumerate() {
            let candidate = if wi == 0 { word.to_string() } else { format!("{line} {word}") };
            if line_px_width(&candidate, px) <= max_w {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            // `word` alone may still be too wide: split by characters.
            for ch in word.chars() {
                let mut t = line.clone();
                t.push(ch);
                if !line.is_empty() && line_px_width(&t, px) > max_w {
                    out.push(std::mem::take(&mut line));
                    line.push(ch);
                } else {
                    line = t;
                }
            }
        }
        out.push(line);
    }
    out
}

/// Inner padding of text boxes / callouts, as a fraction of the font size.
pub const BOX_PAD_EM: f32 = 0.5;

/// Resolved geometry of a [`ShapeKind::TextBox`] or [`ShapeKind::Callout`].
#[derive(Debug, Clone, PartialEq)]
pub struct BoxLayout {
    pub min: Pt,
    pub max: Pt,
    /// Top-left of the first text line.
    pub text_pos: Pt,
    /// Width available for text (wrap width for text boxes).
    pub text_w: f32,
    pub lines: Vec<String>,
    pub radius: f32,
}

/// Text box: width from the drag, height grows to fit the wrapped text.
pub fn textbox_layout(a: Pt, b: Pt, text: &str, px: f32) -> BoxLayout {
    let (min, max) = norm(a, b);
    let pad = px * BOX_PAD_EM;
    let w = (max.x - min.x).max(px * 2.0 + 2.0 * pad);
    let text_w = w - 2.0 * pad;
    let lines = wrap_text(text, px, text_w);
    let need = lines.len() as f32 * line_height(px) + 2.0 * pad;
    let h = (max.y - min.y).max(need);
    BoxLayout {
        min,
        max: Pt::new(min.x + w, min.y + h),
        text_pos: Pt::new(min.x + pad, min.y + pad),
        text_w,
        lines,
        radius: px * 0.3,
    }
}

/// Widest an auto-sized callout bubble gets before its text wraps (in em).
pub const CALLOUT_MAX_EM: f32 = 16.0;

/// Callout bubble. `a`/`b` are opposite corners of the box. When the box is
/// (almost) zero wide it is *auto*-sized: as wide as the text (up to
/// [`CALLOUT_MAX_EM`], then wrapping), extending away from `tip`. The height
/// grows to fit the wrapped text, away from the tip (upwards for a bubble
/// that sits above it).
pub fn callout_layout(tip: Pt, a: Pt, b: Pt, text: &str, px: f32) -> BoxLayout {
    let pad = px * BOX_PAD_EM;
    let (min, max) = norm(a, b);
    let min_w = px * 2.0 + 2.0 * pad;
    let (x0, w) = if (b.x - a.x).abs() < 1.0 {
        let longest = text.split('\n').map(|l| line_px_width(l, px)).fold(px, f32::max);
        let w = (longest.min(px * CALLOUT_MAX_EM) + 2.0 * pad + 1.0).max(min_w);
        (if a.x >= tip.x { a.x } else { a.x - w }, w)
    } else {
        (min.x, (max.x - min.x).max(min_w))
    };
    let text_w = w - 2.0 * pad;
    let lines = wrap_text(text, px, text_w);
    let need = lines.len() as f32 * line_height(px) + 2.0 * pad;
    let h = (max.y - min.y).max(need);
    let y0 = if (min.y + max.y) / 2.0 < tip.y { max.y - h } else { min.y };
    BoxLayout {
        min: Pt::new(x0, y0),
        max: Pt::new(x0 + w, y0 + h),
        text_pos: Pt::new(x0 + pad, y0 + pad),
        text_w,
        lines,
        radius: px * 0.45,
    }
}

/// Black or white, whichever reads better on `c`.
pub fn contrast_on(c: Rgba) -> Rgba {
    let l = 0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32;
    if l > 150.0 {
        [0, 0, 0, 255]
    } else {
        [255, 255, 255, 255]
    }
}

/// Halo radius (px) for outlined text of font size `px`.
pub fn halo_radius(px: f32) -> f32 {
    (px * 0.1).max(1.5)
}

/// Border width (px) of an outline-only text box / callout.
pub fn box_border(px: f32) -> f32 {
    (px * 0.1).max(1.5)
}

/// Size `(w, h)` in px of `text` (multi-line allowed) at font size `px`.
pub fn text_size(text: &str, px: f32) -> (f32, f32) {
    let Some(f) = font() else {
        return (px * 0.5 * text.chars().count() as f32, px);
    };
    let sf = f.as_scaled(PxScale::from(px));
    let line_h = sf.height() + sf.line_gap();
    let mut w = 0f32;
    let mut lines = 0;
    for line in text.split('\n') {
        lines += 1;
        w = w.max(line_width(&sf, line));
    }
    (w, line_h * lines.max(1) as f32)
}

fn line_width<F: Font, SF: ScaleFont<F>>(sf: &SF, line: &str) -> f32 {
    let mut x = 0.0;
    let mut prev = None;
    for c in line.chars() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            x += sf.kern(p, id);
        }
        x += sf.h_advance(id);
        prev = Some(id);
    }
    x
}

/// Render the document: annotations over `base`, then crop.
pub fn flatten(base: &Image, doc: &Document) -> Image {
    let mut canvas = base.clone();
    // The spotlight dimming is one shared layer, applied right after the
    // topmost spotlight's z-position (see `ShapeKind::Spotlight`).
    let last_spot = doc.shapes().iter().rposition(|s| matches!(s.kind, ShapeKind::Spotlight { .. }));
    for (i, shape) in doc.shapes().iter().enumerate() {
        draw_shape(&mut canvas, base, shape);
        if last_spot == Some(i) {
            dim_outside_spotlights(&mut canvas, doc.shapes());
        }
    }
    match doc.crop() {
        Some(r) => canvas.crop(r).unwrap_or(canvas),
        None => canvas,
    }
}

/// `spec` with the watermark text's placeholders expanded
/// (`{date}`, `{time}`, `{datetime}`, `{width}`, `{height}`).
pub fn expand_effects(spec: &jepret_core::EffectSpec, now: chrono::NaiveDateTime, dims: (u32, u32)) -> jepret_core::EffectSpec {
    let mut spec = spec.clone();
    if let Some(w) = spec.watermark.as_mut() {
        let vars = [
            ("datetime", now.format("%Y-%m-%d_%H-%M-%S").to_string()),
            ("date", now.format("%Y-%m-%d").to_string()),
            ("time", now.format("%H-%M-%S").to_string()),
            ("width", dims.0.to_string()),
            ("height", dims.1.to_string()),
        ];
        w.text = jepret_core::template::expand(&w.text, &vars);
    }
    spec
}

/// Final export pixels: [`flatten`] (annotations + crop), then the document's
/// effects ([`jepret_effects::apply`]). Skips the effects pass when it is a no-op.
pub fn export(base: &Image, doc: &Document, now: chrono::NaiveDateTime) -> Image {
    let out = flatten(base, doc);
    let spec = expand_effects(doc.effects(), now, (out.width, out.height));
    if spec.is_noop() {
        out
    } else {
        jepret_effects::apply(&out, &spec)
    }
}

/// Draw one shape onto `canvas`; `src` supplies original pixels for
/// blur/pixelate (normally the un-annotated base).
pub fn draw_shape(canvas: &mut Image, src: &Image, shape: &Shape) {
    if let Some(sh) = shape.shadow_of() {
        draw_shape(canvas, src, &sh);
    }
    let c = shape.color;
    let w = shape.width.max(1.0);
    match &shape.kind {
        ShapeKind::Line { a, b } => stroke_segments(canvas, &[(*a, *b)], c, w),
        ShapeKind::Arrow { a, b } => {
            let mut segs = vec![(*a, *b)];
            let (dx, dy) = (a.x - b.x, a.y - b.y);
            let len = (dx * dx + dy * dy).sqrt();
            if len > 0.5 {
                let head = (w * 4.0).max(12.0).min(len);
                let ang = dy.atan2(dx);
                for s in [-0.5f32, 0.5] {
                    let t = ang + s;
                    segs.push((*b, Pt::new(b.x + head * t.cos(), b.y + head * t.sin())));
                }
            }
            stroke_segments(canvas, &segs, c, w);
        }
        ShapeKind::Pen { points } => {
            if points.len() == 1 {
                stroke_segments(canvas, &[(points[0], points[0])], c, w);
            } else {
                let segs: Vec<_> = points.windows(2).map(|p| (p[0], p[1])).collect();
                stroke_segments(canvas, &segs, c, w);
            }
        }
        ShapeKind::Rect { a, b } => {
            let (min, max) = norm(*a, *b);
            let (cx, cy) = ((min.x + max.x) / 2.0, (min.y + max.y) / 2.0);
            let (hx, hy) = ((max.x - min.x) / 2.0, (max.y - min.y) / 2.0);
            let bb = bbox(min, max, w / 2.0 + 1.0);
            paint(canvas, bb, c, |x, y| {
                let qx = (x - cx).abs() - hx;
                let qy = (y - cy).abs() - hy;
                let sd = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0);
                if shape.fill { sd - w / 2.0 } else { sd.abs() - w / 2.0 }
            });
        }
        ShapeKind::Ellipse { a, b } => {
            let (min, max) = norm(*a, *b);
            let (cx, cy) = ((min.x + max.x) / 2.0, (min.y + max.y) / 2.0);
            let rx = ((max.x - min.x) / 2.0).max(0.5);
            let ry = ((max.y - min.y) / 2.0).max(0.5);
            let bb = bbox(min, max, w / 2.0 + 1.0);
            paint(canvas, bb, c, |x, y| {
                let (dx, dy) = (x - cx, y - cy);
                let f = ((dx / rx).powi(2) + (dy / ry).powi(2)).sqrt();
                let gx = dx / (rx * rx);
                let gy = dy / (ry * ry);
                let g = (gx * gx + gy * gy).sqrt() / f.max(1e-6);
                // first-order signed distance to the ellipse outline
                let sd = if g > 1e-9 { (f - 1.0) / g } else { -rx.min(ry) };
                if shape.fill { sd - w / 2.0 } else { sd.abs() - w / 2.0 }
            });
        }
        ShapeKind::Highlight { a, b } => {
            let (min, max) = norm(*a, *b);
            let bb = bbox(min, max, 1.0);
            paint(canvas, bb, c, |x, y| {
                let qx = (x - (min.x + max.x) / 2.0).abs() - (max.x - min.x) / 2.0;
                let qy = (y - (min.y + max.y) / 2.0).abs() - (max.y - min.y) / 2.0;
                qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0)
            });
        }
        ShapeKind::Blur { a, b } => {
            if let Some(r) = region(canvas, *a, *b) {
                blur_region(canvas, src, r, (shape.width * 2.0).round().max(1.0) as usize);
            }
        }
        ShapeKind::Pixelate { a, b } => {
            if let Some(r) = region(canvas, *a, *b) {
                pixelate_region(canvas, src, r, (shape.width * 2.0).round().max(2.0) as u32);
            }
        }
        ShapeKind::Text { .. } | ShapeKind::TextBox { .. } | ShapeKind::Callout { .. } => {
            draw_text_shape(canvas, shape, false);
        }
        // Drawn collectively by `dim_outside_spotlights` (see `flatten`).
        ShapeKind::Spotlight { .. } => {}
        ShapeKind::Magnify { center, radius, zoom } => {
            let r = radius.max(1.0);
            let z = zoom.max(1.0);
            let bb = bbox(Pt::new(center.x - r, center.y - r), Pt::new(center.x + r, center.y + r), 1.0);
            let (x0, y0) = (bb.0.max(0), bb.1.max(0));
            let (x1, y1) = (bb.2.min(canvas.width as i32), bb.3.min(canvas.height as i32));
            for y in y0..y1 {
                for x in x0..x1 {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let d = (px - center.x).hypot(py - center.y) - r;
                    let cov = (0.5 - d).clamp(0.0, 1.0);
                    if cov > 0.0 {
                        let col = sample_bilinear(src, center.x + (px - center.x) / z, center.y + (py - center.y) / z);
                        blend(canvas, x, y, col, cov);
                    }
                }
            }
            // Border sits inside the circle edge.
            let rc = (r - w / 2.0).max(0.0);
            paint(canvas, bb, c, |x, y| ((x - center.x).hypot(y - center.y) - rc).abs() - w / 2.0);
        }
        ShapeKind::Polygon { points } => {
            if points.len() >= 3 {
                let (min, max) = shape.bounds();
                let bb = bbox(min, max, w / 2.0 + 2.0);
                paint(canvas, bb, c, |x, y| {
                    let sd = sd_polygon(Pt::new(x, y), points);
                    if shape.fill { sd } else { sd.abs() - w / 2.0 }
                });
            } else if points.len() == 2 {
                stroke_segments(canvas, &[(points[0], points[1])], c, w);
            }
        }
        ShapeKind::Step { center, n } => {
            let r = shape.step_radius();
            let bb = bbox(
                Pt::new(center.x - r, center.y - r),
                Pt::new(center.x + r, center.y + r),
                1.0,
            );
            paint(canvas, bb, c, |x, y| (x - center.x).hypot(y - center.y) - r);
            let label = n.to_string();
            let px = r * 1.15;
            let (tw, th) = text_size(&label, px);
            let white = [255, 255, 255, 255];
            draw_text(canvas, Pt::new(center.x - tw / 2.0, center.y - th / 2.0), &label, px, white, true);
        }
    }
}

/// Bilinear sample of `img` at image-px position `(x, y)` (pixel centres at
/// +0.5), edge-clamped.
pub fn sample_bilinear(img: &Image, x: f32, y: f32) -> Rgba {
    if img.width == 0 || img.height == 0 {
        return [0; 4];
    }
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let at = |xi: f32, yi: f32| {
        let xc = (xi as i64).clamp(0, img.width as i64 - 1) as u32;
        let yc = (yi as i64).clamp(0, img.height as i64 - 1) as u32;
        img.pixel(xc, yc)
    };
    let (p00, p10, p01, p11) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
    let mut out = [0u8; 4];
    for k in 0..4 {
        let top = p00[k] as f32 * (1.0 - tx) + p10[k] as f32 * tx;
        let bot = p01[k] as f32 * (1.0 - tx) + p11[k] as f32 * tx;
        out[k] = (top * (1.0 - ty) + bot * ty).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// Coverage (0..=1, anti-aliased) of a spotlight shape at `(x, y)`.
fn spotlight_coverage(shape: &Shape, x: f32, y: f32) -> f32 {
    let ShapeKind::Spotlight { a, b } = &shape.kind else { return 0.0 };
    let (min, max) = norm(*a, *b);
    let (cx, cy) = ((min.x + max.x) / 2.0, (min.y + max.y) / 2.0);
    let (hx, hy) = (((max.x - min.x) / 2.0).max(0.5), ((max.y - min.y) / 2.0).max(0.5));
    let sd = if shape.fill {
        let (dx, dy) = (x - cx, y - cy);
        let f = ((dx / hx).powi(2) + (dy / hy).powi(2)).sqrt();
        let (gx, gy) = (dx / (hx * hx), dy / (hy * hy));
        let g = (gx * gx + gy * gy).sqrt() / f.max(1e-6);
        if g > 1e-9 { (f - 1.0) / g } else { -hx.min(hy) }
    } else {
        let qx = (x - cx).abs() - hx;
        let qy = (y - cy).abs() - hy;
        qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0)
    };
    (0.5 - sd).clamp(0.0, 1.0)
}

/// Darken everything outside the union of all spotlights in `shapes`
/// (one shared layer; overlapping spotlights do not stack).
pub fn dim_outside_spotlights(canvas: &mut Image, shapes: &[Shape]) {
    let spots: Vec<&Shape> = shapes.iter().filter(|s| matches!(s.kind, ShapeKind::Spotlight { .. })).collect();
    if spots.is_empty() {
        return;
    }
    let dim = [0, 0, 0, crate::doc::SPOTLIGHT_DIM_ALPHA];
    // Per-spotlight bounds to skip the expensive coverage for far pixels.
    let bounds: Vec<(Pt, Pt)> = spots.iter().map(|s| s.bounds()).collect();
    for y in 0..canvas.height as i32 {
        for x in 0..canvas.width as i32 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut inside = 0f32;
            for (s, (min, max)) in spots.iter().zip(&bounds) {
                if px < min.x - 1.0 || px > max.x + 1.0 || py < min.y - 1.0 || py > max.y + 1.0 {
                    continue;
                }
                inside = inside.max(spotlight_coverage(s, px, py));
                if inside >= 1.0 {
                    break;
                }
            }
            if inside < 1.0 {
                blend(canvas, x, y, dim, 1.0 - inside);
            }
        }
    }
}

fn norm(a: Pt, b: Pt) -> (Pt, Pt) {
    (Pt::new(a.x.min(b.x), a.y.min(b.y)), Pt::new(a.x.max(b.x), a.y.max(b.y)))
}

fn bbox(min: Pt, max: Pt, pad: f32) -> (i32, i32, i32, i32) {
    (
        (min.x - pad).floor() as i32,
        (min.y - pad).floor() as i32,
        (max.x + pad).ceil() as i32,
        (max.y + pad).ceil() as i32,
    )
}

/// Integer region clamped to the image; `None` if empty.
fn region(img: &Image, a: Pt, b: Pt) -> Option<Rect> {
    let (min, max) = norm(a, b);
    let x0 = (min.x.round() as i64).clamp(0, img.width as i64);
    let y0 = (min.y.round() as i64).clamp(0, img.height as i64);
    let x1 = (max.x.round() as i64).clamp(0, img.width as i64);
    let y1 = (max.y.round() as i64).clamp(0, img.height as i64);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(Rect::new(x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Source-over blend of `color` with extra `cov` coverage at pixel (x,y).
fn blend(img: &mut Image, x: i32, y: i32, color: Rgba, cov: f32) {
    if x < 0 || y < 0 || x >= img.width as i32 || y >= img.height as i32 {
        return;
    }
    let sa = color[3] as f32 / 255.0 * cov;
    if sa <= 0.0 {
        return;
    }
    let i = (y as usize * img.width as usize + x as usize) * 4;
    let d = &mut img.pixels[i..i + 4];
    let da = d[3] as f32 / 255.0;
    let oa = sa + da * (1.0 - sa);
    if oa <= 0.0 {
        return;
    }
    for k in 0..3 {
        let s = color[k] as f32;
        let dd = d[k] as f32;
        d[k] = ((s * sa + dd * da * (1.0 - sa)) / oa).round().clamp(0.0, 255.0) as u8;
    }
    d[3] = (oa * 255.0).round().clamp(0.0, 255.0) as u8;
}

/// Evaluate a signed distance (px, negative inside) at every pixel centre in
/// `bb` and blend `color` with coverage `clamp(0.5 - d)`.
fn paint(img: &mut Image, bb: (i32, i32, i32, i32), color: Rgba, sdf: impl Fn(f32, f32) -> f32) {
    let x0 = bb.0.max(0);
    let y0 = bb.1.max(0);
    let x1 = bb.2.min(img.width as i32);
    let y1 = bb.3.min(img.height as i32);
    for y in y0..y1 {
        for x in x0..x1 {
            let d = sdf(x as f32 + 0.5, y as f32 + 0.5);
            let cov = (0.5 - d).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend(img, x, y, color, cov);
            }
        }
    }
}

fn stroke_segments(img: &mut Image, segs: &[(Pt, Pt)], color: Rgba, w: f32) {
    let mut min = Pt::new(f32::MAX, f32::MAX);
    let mut max = Pt::new(f32::MIN, f32::MIN);
    for (a, b) in segs {
        for p in [a, b] {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        }
    }
    if segs.is_empty() {
        return;
    }
    let bb = bbox(min, max, w / 2.0 + 1.0);
    paint(img, bb, color, |x, y| {
        let p = Pt::new(x, y);
        let mut d = f32::MAX;
        for (a, b) in segs {
            d = d.min(crate::doc::seg_dist(p, *a, *b));
        }
        d - w / 2.0
    });
}

fn draw_text(img: &mut Image, pos: Pt, text: &str, px: f32, color: Rgba, bold: bool) {
    let Some(f) = font() else { return };
    let scale = PxScale::from(px);
    let sf = f.as_scaled(scale);
    let line_h = sf.height() + sf.line_gap();
    let offsets: &[f32] = if bold { &[0.0, px * 0.04] } else { &[0.0] };
    for (li, line) in text.split('\n').enumerate() {
        let mut x = pos.x;
        let baseline = pos.y + sf.ascent() + li as f32 * line_h;
        let mut prev = None;
        for ch in line.chars() {
            let id = sf.glyph_id(ch);
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            for off in offsets {
                let g = id.with_scale_and_position(scale, point(x + off, baseline));
                if let Some(og) = f.outline_glyph(g) {
                    let b = og.px_bounds();
                    og.draw(|gx, gy, cov| {
                        blend(img, b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32, color, cov);
                    });
                }
            }
            x += sf.h_advance(id);
            prev = Some(id);
        }
    }
}

/// Colour the text of `shape` is drawn in (contrasting on a filled box).
pub fn text_color_of(shape: &Shape) -> Rgba {
    let boxed = matches!(shape.kind, ShapeKind::TextBox { .. } | ShapeKind::Callout { .. });
    if boxed && shape.fill && shape.color != crate::doc::SHADOW_COLOR {
        contrast_on(shape.color)
    } else {
        shape.color
    }
}

/// Draw a Text / TextBox / Callout (without its shadow). `hide_text` draws
/// only the box (the UI shows a live text field there while editing).
pub fn draw_text_shape(img: &mut Image, shape: &Shape, hide_text: bool) {
    let px = shape.text_px();
    let (lines, pos) = match &shape.kind {
        ShapeKind::Text { pos, text } => (text.split('\n').map(str::to_string).collect::<Vec<_>>(), *pos),
        ShapeKind::TextBox { a, b, text } => {
            let l = textbox_layout(*a, *b, text, px);
            let (lmin, lmax, r) = (l.min, l.max, l.radius);
            paint_box(img, l.min, l.max, shape, &|x, y| sd_round_rect(x, y, lmin, lmax, r));
            (l.lines, l.text_pos)
        }
        ShapeKind::Callout { tip, a, b, text } => {
            let l = callout_layout(*tip, *a, *b, text, px);
            let tail = callout_tail(&l, *tip, px);
            let (lmin, lmax, r) = (l.min, l.max, l.radius);
            let sdf = |x: f32, y: f32| {
                let d = sd_round_rect(x, y, lmin, lmax, r);
                match &tail {
                    Some(t) => d.min(sd_polygon(Pt::new(x, y), t)),
                    None => d,
                }
            };
            let bmin = Pt::new(l.min.x.min(tip.x), l.min.y.min(tip.y));
            let bmax = Pt::new(l.max.x.max(tip.x), l.max.y.max(tip.y));
            paint_box(img, bmin, bmax, shape, &sdf);
            (l.lines, l.text_pos)
        }
        _ => return,
    };
    if hide_text {
        return;
    }
    let tc = text_color_of(shape);
    let boxed_fill = shape.fill && !matches!(shape.kind, ShapeKind::Text { .. });
    let halo = (shape.outline && !boxed_fill).then(|| (contrast_on(tc), halo_radius(px)));
    draw_text_lines(img, pos, &lines, px, tc, halo);
}

/// Solid box, or outline (+ white paper inside a callout).
fn paint_box(img: &mut Image, min: Pt, max: Pt, shape: &Shape, sdf: &dyn Fn(f32, f32) -> f32) {
    let bw = box_border(shape.text_px());
    let bb = bbox(min, max, bw + 1.0);
    if shape.fill {
        paint(img, bb, shape.color, sdf);
    } else {
        if matches!(shape.kind, ShapeKind::Callout { .. }) && shape.color != crate::doc::SHADOW_COLOR {
            // A bubble reads as a bubble only with paper inside.
            paint(img, bb, [255, 255, 255, 255], sdf);
        }
        paint(img, bb, shape.color, |x, y| sdf(x, y).abs() - bw / 2.0);
    }
}

/// Tail triangle from inside the bubble to `tip`; `None` when the tip is
/// inside the bubble.
fn callout_tail(l: &BoxLayout, tip: Pt, px: f32) -> Option<[Pt; 3]> {
    if tip.x >= l.min.x && tip.x <= l.max.x && tip.y >= l.min.y && tip.y <= l.max.y {
        return None;
    }
    let c = Pt::new((l.min.x + l.max.x) / 2.0, (l.min.y + l.max.y) / 2.0);
    let (dx, dy) = (tip.x - c.x, tip.y - c.y);
    let len = dx.hypot(dy).max(1e-3);
    let hw = (px * 0.55).min((l.max.x - l.min.x).min(l.max.y - l.min.y) / 3.0);
    let (nx, ny) = (-dy / len * hw, dx / len * hw);
    Some([tip, Pt::new(c.x + nx, c.y + ny), Pt::new(c.x - nx, c.y - ny)])
}

fn sd_round_rect(x: f32, y: f32, min: Pt, max: Pt, r: f32) -> f32 {
    let (cx, cy) = ((min.x + max.x) / 2.0, (min.y + max.y) / 2.0);
    let r = r.min((max.x - min.x) / 2.0).min((max.y - min.y) / 2.0).max(0.0);
    let qx = (x - cx).abs() - ((max.x - min.x) / 2.0 - r);
    let qy = (y - cy).abs() - ((max.y - min.y) / 2.0 - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Signed distance to a simple polygon (negative inside).
pub(crate) fn sd_polygon(p: Pt, v: &[Pt]) -> f32 {
    let mut d = f32::MAX;
    let mut inside = false;
    let n = v.len();
    for i in 0..n {
        let (a, b) = (v[i], v[(i + 1) % n]);
        d = d.min(crate::doc::seg_dist(p, a, b));
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    if inside {
        -d
    } else {
        d
    }
}

/// Lines of text at `pos` (top-left), optionally with a halo `(colour, radius)`.
fn draw_text_lines(img: &mut Image, pos: Pt, lines: &[String], px: f32, color: Rgba, halo: Option<(Rgba, f32)>) {
    let Some(f) = font() else { return };
    let scale = PxScale::from(px);
    let sf = f.as_scaled(scale);
    let line_h = sf.height() + sf.line_gap();
    let r = halo.map(|h| h.1).unwrap_or(0.0);
    let m = r.ceil() as i32 + 2;
    let w = lines.iter().map(|l| line_width(&sf, l)).fold(0.0, f32::max).ceil() as i32 + 2 * m + 2;
    let h = (line_h * lines.len().max(1) as f32).ceil() as i32 + 2 * m + 2;
    let (ox, oy) = (pos.x.floor() as i32 - m, pos.y.floor() as i32 - m);
    // Nothing to do off-canvas.
    if ox >= img.width as i32 || oy >= img.height as i32 || ox + w <= 0 || oy + h <= 0 {
        return;
    }
    let (wu, hu) = (w as usize, h as usize);
    let mut mask = vec![0f32; wu * hu];
    for (li, line) in lines.iter().enumerate() {
        let mut x = pos.x - ox as f32;
        let baseline = pos.y - oy as f32 + sf.ascent() + li as f32 * line_h;
        let mut prev = None;
        for ch in line.chars() {
            let id = sf.glyph_id(ch);
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            let g = id.with_scale_and_position(scale, point(x, baseline));
            if let Some(og) = f.outline_glyph(g) {
                let b = og.px_bounds();
                og.draw(|gx, gy, cov| {
                    let (mx, my) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                    if mx >= 0 && my >= 0 && (mx as usize) < wu && (my as usize) < hu {
                        let k = my as usize * wu + mx as usize;
                        mask[k] = (mask[k] + cov).min(1.0);
                    }
                });
            }
            x += sf.h_advance(id);
            prev = Some(id);
        }
    }
    if let Some((hc, r)) = halo {
        let halo = dilate(&mask, wu, hu, r);
        blend_mask(img, ox, oy, wu, &halo, hc);
    }
    blend_mask(img, ox, oy, wu, &mask, color);
}

fn blend_mask(img: &mut Image, ox: i32, oy: i32, w: usize, mask: &[f32], color: Rgba) {
    for (k, v) in mask.iter().enumerate() {
        if *v > 0.0 {
            blend(img, ox + (k % w) as i32, oy + (k / w) as i32, color, *v);
        }
    }
}

/// Anti-aliased dilation of a coverage mask by a disc of radius `r` px.
pub fn dilate(mask: &[f32], w: usize, h: usize, r: f32) -> Vec<f32> {
    let ri = r.ceil() as i32 + 1;
    let mut offs = Vec::new();
    for dy in -ri..=ri {
        for dx in -ri..=ri {
            let wgt = (r + 0.5 - (dx as f32).hypot(dy as f32)).clamp(0.0, 1.0);
            if wgt > 0.0 {
                offs.push((dx, dy, wgt));
            }
        }
    }
    let mut out = vec![0f32; mask.len()];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let mut best = 0f32;
            for &(dx, dy, wgt) in &offs {
                let (sx, sy) = (x + dx, y + dy);
                if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
                    best = best.max(mask[sy as usize * w + sx as usize] * wgt);
                }
            }
            out[y as usize * w + x as usize] = best;
        }
    }
    out
}

/// Rasterise a text-bearing shape (with its shadow and halo) for the
/// on-screen preview at `scale` x image resolution. Returns the image-px
/// rectangle it covers (`min`, `max`) and the pixels.
pub fn text_shape_preview(shape: &Shape, scale: f32, hide_text: bool) -> Option<(Pt, Pt, Image)> {
    if !shape.kind.is_text() {
        return None;
    }
    let px = shape.text_px();
    let pad = halo_radius(px) + box_border(px) + crate::doc::SHADOW_OFFSET + 3.0;
    let (bmin, bmax) = shape.bounds();
    let min = Pt::new((bmin.x - pad).floor(), (bmin.y - pad).floor());
    let max = Pt::new((bmax.x + pad).ceil(), (bmax.y + pad).ceil());
    let (w, h) = (((max.x - min.x) * scale).ceil() as u32, ((max.y - min.y) * scale).ceil() as u32);
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return None;
    }
    let mut local = shape.clone();
    local.translate(-min.x, -min.y);
    local.scale(scale);
    let mut img = Image::new(w, h);
    if let Some(sh) = local.shadow_of() {
        draw_text_shape(&mut img, &sh, hide_text);
    }
    draw_text_shape(&mut img, &local, hide_text);
    Some((min, max, img))
}

/// Box blur (two passes ~ gaussian) of `src` within `r`, written to `dst`.
/// Edges are clamped to the region so nothing outside leaks in.
pub fn blur_region(dst: &mut Image, src: &Image, r: Rect, radius: usize) {
    let (w, h) = (r.width as usize, r.height as usize);
    let mut buf: Vec<f32> = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = src.row(r.y as u32 + y as u32);
        let s = r.x as usize * 4;
        buf.extend(row[s..s + w * 4].iter().map(|&v| v as f32));
    }
    let mut tmp = vec![0f32; buf.len()];
    for _ in 0..2 {
        box_pass(&buf, &mut tmp, w, h, radius, true);
        box_pass(&tmp, &mut buf, w, h, radius, false);
    }
    for y in 0..h {
        let di = ((r.y as usize + y) * dst.width as usize + r.x as usize) * 4;
        for (k, v) in buf[y * w * 4..(y + 1) * w * 4].iter().enumerate() {
            dst.pixels[di + k] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// One horizontal (`horiz`) or vertical sliding-window box pass.
fn box_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, radius: usize, horiz: bool) {
    let (len, lines, step_in, step_line) = if horiz { (w, h, 4, w * 4) } else { (h, w, w * 4, 4) };
    let win = (2 * radius + 1) as f32;
    for l in 0..lines {
        for ch in 0..4 {
            let base = l * step_line + ch;
            let at = |i: isize| src[base + (i.clamp(0, len as isize - 1) as usize) * step_in];
            let mut acc = 0f32;
            for i in -(radius as isize)..=(radius as isize) {
                acc += at(i);
            }
            for i in 0..len {
                dst[base + i * step_in] = acc / win;
                acc += at(i as isize + radius as isize + 1) - at(i as isize - radius as isize);
            }
        }
    }
}

/// Mosaic: every `block`x`block` cell (anchored at the region's top-left)
/// becomes its average colour in `src`.
pub fn pixelate_region(dst: &mut Image, src: &Image, r: Rect, block: u32) {
    let block = block.max(1);
    let mut by = r.y;
    while by < r.bottom() {
        let bh = block.min((r.bottom() - by) as u32);
        let mut bx = r.x;
        while bx < r.right() {
            let bw = block.min((r.right() - bx) as u32);
            let mut sum = [0u64; 4];
            for y in by..by + bh as i32 {
                for x in bx..bx + bw as i32 {
                    let p = src.pixel(x as u32, y as u32);
                    for k in 0..4 {
                        sum[k] += p[k] as u64;
                    }
                }
            }
            let n = (bw * bh) as u64;
            let avg = [
                (sum[0] / n) as u8,
                (sum[1] / n) as u8,
                (sum[2] / n) as u8,
                (sum[3] / n) as u8,
            ];
            for y in by..by + bh as i32 {
                for x in bx..bx + bw as i32 {
                    let i = (y as usize * dst.width as usize + x as usize) * 4;
                    dst.pixels[i..i + 4].copy_from_slice(&avg);
                }
            }
            bx += block as i32;
        }
        by += block as i32;
    }
}

/// Render the effect of a blur/pixelate shape into a standalone image of its
/// region (for UI preview). Returns `(region, image)`.
pub fn effect_preview(base: &Image, shape: &Shape) -> Option<(Rect, Image)> {
    let (a, b, blur) = match &shape.kind {
        ShapeKind::Blur { a, b } => (*a, *b, true),
        ShapeKind::Pixelate { a, b } => (*a, *b, false),
        _ => return None,
    };
    let r = region(base, a, b)?;
    let mut out = base.crop(r).ok()?;
    let local = Rect::new(0, 0, r.width, r.height);
    let src = out.clone();
    if blur {
        blur_region(&mut out, &src, local, (shape.width * 2.0).round().max(1.0) as usize);
    } else {
        pixelate_region(&mut out, &src, local, (shape.width * 2.0).round().max(2.0) as u32);
    }
    Some((r, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn egui_factor_shrinks_em_size_to_line_height() {
        let k = egui_font_factor();
        assert!(k > 0.5 && k <= 1.0, "{k}");
    }

    fn solid(w: u32, h: u32, c: Rgba) -> Image {
        let mut img = Image::new(w, h);
        for p in img.pixels.chunks_exact_mut(4) {
            p.copy_from_slice(&c);
        }
        img
    }

    fn gradient(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize * 4;
                img.pixels[i..i + 4].copy_from_slice(&[
                    (x * 7 % 256) as u8,
                    (y * 13 % 256) as u8,
                    ((x ^ y) * 5 % 256) as u8,
                    255,
                ]);
            }
        }
        img
    }

    #[test]
    fn rect_outline_at_right_position() {
        let base = solid(100, 100, [255, 255, 255, 255]);
        let mut d = Document::new();
        d.add_shape(Shape::new(
            ShapeKind::Rect { a: Pt::new(20.0, 30.0), b: Pt::new(70.0, 60.0) },
            [255, 0, 0, 255],
            4.0,
        ));
        let out = flatten(&base, &d);
        assert_eq!((out.width, out.height), (100, 100));
        // on the left edge (x=20) and top edge (y=30)
        assert_eq!(out.pixel(20, 45), [255, 0, 0, 255]);
        assert_eq!(out.pixel(45, 30), [255, 0, 0, 255]);
        assert_eq!(out.pixel(69, 45), [255, 0, 0, 255]);
        assert_eq!(out.pixel(45, 59), [255, 0, 0, 255]);
        // hollow interior and far outside untouched
        assert_eq!(out.pixel(45, 45), [255, 255, 255, 255]);
        assert_eq!(out.pixel(5, 5), [255, 255, 255, 255]);
        assert_eq!(out.pixel(90, 90), [255, 255, 255, 255]);
    }

    #[test]
    fn polygon_outline_fill_concave() {
        // L shape
        let pts = vec![
            Pt::new(10.0, 10.0),
            Pt::new(30.0, 10.0),
            Pt::new(30.0, 50.0),
            Pt::new(70.0, 50.0),
            Pt::new(70.0, 70.0),
            Pt::new(10.0, 70.0),
        ];
        let base = solid(80, 80, [255, 255, 255, 255]);
        let red = [255, 0, 0, 255];
        let white = [255, 255, 255, 255];
        let mut d = Document::new();
        d.add_shape(Shape::new(ShapeKind::Polygon { points: pts.clone() }, red, 4.0));
        let out = flatten(&base, &d);
        assert_eq!(out.pixel(10, 40), red); // left edge
        assert_eq!(out.pixel(50, 69), red); // bottom edge
        assert_eq!(out.pixel(20, 30), white); // hollow inside
        assert_eq!(out.pixel(60, 20), white); // concave notch, outside
        let mut d = Document::new();
        let mut s = Shape::new(ShapeKind::Polygon { points: pts }, red, 2.0);
        s.fill = true;
        d.add_shape(s);
        let out = flatten(&base, &d);
        assert_eq!(out.pixel(20, 30), red);
        assert_eq!(out.pixel(50, 60), red);
        assert_eq!(out.pixel(60, 20), white);
        assert_eq!(out.pixel(5, 5), white);
    }

    #[test]
    fn polygon_degenerate_no_panic() {
        let base = solid(20, 20, [255, 255, 255, 255]);
        for pts in [vec![], vec![Pt::new(3.0, 3.0)], vec![Pt::new(3.0, 3.0), Pt::new(10.0, 10.0)]] {
            let mut d = Document::new();
            d.add_shape(Shape::new(ShapeKind::Polygon { points: pts }, [0, 0, 0, 255], 2.0));
            let _ = flatten(&base, &d);
        }
    }

    #[test]
    fn highlight_blends_translucent() {
        let base = solid(20, 20, [0, 0, 0, 255]);
        let mut d = Document::new();
        d.add_shape(Shape::new(
            ShapeKind::Highlight { a: Pt::new(5.0, 5.0), b: Pt::new(15.0, 15.0) },
            [255, 255, 255, 128],
            1.0,
        ));
        let out = flatten(&base, &d);
        let p = out.pixel(10, 10);
        assert!((p[0] as i32 - 128).abs() <= 1, "{p:?}");
        assert_eq!(p[3], 255);
        assert_eq!(out.pixel(2, 2), [0, 0, 0, 255]);
    }

    #[test]
    fn pixelate_changes_only_region() {
        let base = gradient(64, 64);
        let mut d = Document::new();
        d.add_shape(Shape::new(
            ShapeKind::Pixelate { a: Pt::new(16.0, 16.0), b: Pt::new(48.0, 48.0) },
            [0; 4],
            4.0, // block = 8
        ));
        let out = flatten(&base, &d);
        let mut changed_inside = false;
        for y in 0..64u32 {
            for x in 0..64u32 {
                let inside = (16..48).contains(&x) && (16..48).contains(&y);
                if inside {
                    changed_inside |= out.pixel(x, y) != base.pixel(x, y);
                } else {
                    assert_eq!(out.pixel(x, y), base.pixel(x, y), "outside changed at {x},{y}");
                }
            }
        }
        assert!(changed_inside);
        // each 8x8 block is uniform
        let c = out.pixel(16, 16);
        for y in 16..24 {
            for x in 16..24 {
                assert_eq!(out.pixel(x, y), c);
            }
        }
    }

    #[test]
    fn blur_smooths_and_stays_in_region() {
        let mut base = solid(40, 40, [0, 0, 0, 255]);
        for y in 0..40u32 {
            for x in 20..40u32 {
                let i = (y * 40 + x) as usize * 4;
                base.pixels[i..i + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let mut d = Document::new();
        d.add_shape(Shape::new(
            ShapeKind::Blur { a: Pt::new(10.0, 10.0), b: Pt::new(30.0, 30.0) },
            [0; 4],
            2.0,
        ));
        let out = flatten(&base, &d);
        // hard edge at x=19/20 becomes intermediate
        let mid = out.pixel(19, 20)[0];
        assert!(mid > 0 && mid < 255, "mid={mid}");
        assert_eq!(out.pixel(5, 20), base.pixel(5, 20));
        assert_eq!(out.pixel(35, 35), base.pixel(35, 35));
        assert_eq!(out.pixel(20, 5), base.pixel(20, 5));
    }

    #[test]
    fn crop_applies_after_annotations() {
        let base = solid(50, 50, [255, 255, 255, 255]);
        let mut d = Document::new();
        d.add_shape(Shape::new(
            ShapeKind::Highlight { a: Pt::new(10.0, 10.0), b: Pt::new(20.0, 20.0) },
            [0, 0, 0, 255],
            1.0,
        ));
        d.set_crop(Some(Rect::new(10, 10, 20, 15)));
        let out = flatten(&base, &d);
        assert_eq!((out.width, out.height), (20, 15));
        assert_eq!(out.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(out.pixel(15, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn ellipse_and_arrow_and_pen_draw_something_in_bounds() {
        let base = solid(80, 80, [255, 255, 255, 255]);
        let mut d = Document::new();
        d.add_shape(Shape::new(
            ShapeKind::Ellipse { a: Pt::new(10.0, 10.0), b: Pt::new(70.0, 50.0) },
            [0, 0, 255, 255],
            3.0,
        ));
        let out = flatten(&base, &d);
        assert_eq!(out.pixel(10, 30)[2], 255);
        assert_eq!(out.pixel(10, 30)[0], 0);
        assert_eq!(out.pixel(40, 10)[0], 0);
        assert_eq!(out.pixel(40, 30), [255, 255, 255, 255]);

        let mut d = Document::new();
        d.add_shape(Shape::new(
            ShapeKind::Arrow { a: Pt::new(10.0, 70.0), b: Pt::new(70.0, 70.0) },
            [0, 0, 0, 255],
            4.0,
        ));
        d.add_shape(Shape::new(
            ShapeKind::Pen { points: vec![Pt::new(5.0, 5.0), Pt::new(30.0, 5.0), Pt::new(30.0, 20.0)] },
            [0, 128, 0, 255],
            3.0,
        ));
        let out = flatten(&base, &d);
        assert_eq!(out.pixel(40, 70), [0, 0, 0, 255]);
        assert_eq!(out.pixel(17, 5)[1], 128);
        // arrow head: near the tip, off the shaft
        assert_ne!(out.pixel(62, 64), [255, 255, 255, 255]);
    }

    #[test]
    fn step_and_text_render_pixels() {
        let base = solid(120, 80, [255, 255, 255, 255]);
        let mut d = Document::new();
        d.add_shape(Shape::new(ShapeKind::Step { center: Pt::new(30.0, 30.0), n: 1 }, [200, 0, 0, 255], 4.0));
        d.add_shape(Shape::new(
            ShapeKind::Text { pos: Pt::new(60.0, 10.0), text: "Hi".into() },
            [0, 0, 0, 255],
            4.0,
        ));
        let out = flatten(&base, &d);
        // circle body (off the digit)
        assert_eq!(out.pixel(30 - 13, 30), [200, 0, 0, 255]);
        // some dark text pixels in the text box
        let (tw, th) = text_size("Hi", 24.0);
        assert!(tw > 10.0 && th > 10.0);
        let mut dark = 0;
        for y in 10..(10 + th as u32) {
            for x in 60..(60 + tw as u32 + 1) {
                if out.pixel(x, y)[0] < 128 {
                    dark += 1;
                }
            }
        }
        assert!(dark > 20, "dark={dark}");
    }

    #[test]
    fn out_of_bounds_shapes_do_not_panic() {
        let base = solid(10, 10, [255, 255, 255, 255]);
        let mut d = Document::new();
        d.add_shape(Shape::new(ShapeKind::Rect { a: Pt::new(-50.0, -50.0), b: Pt::new(500.0, 500.0) }, [0, 0, 0, 255], 3.0));
        d.add_shape(Shape::new(ShapeKind::Blur { a: Pt::new(-5.0, -5.0), b: Pt::new(500.0, 500.0) }, [0; 4], 3.0));
        d.add_shape(Shape::new(ShapeKind::Pixelate { a: Pt::new(100.0, 100.0), b: Pt::new(200.0, 200.0) }, [0; 4], 3.0));
        d.add_shape(Shape::new(ShapeKind::Text { pos: Pt::new(-30.0, 5.0), text: "x\ny".into() }, [0, 0, 0, 255], 4.0));
        let out = flatten(&base, &d);
        assert_eq!((out.width, out.height), (10, 10));
    }

    #[test]
    fn fill_and_shadow() {
        let base = solid(60, 60, [255, 255, 255, 255]);
        let mut d = Document::new();
        let mut s = Shape::new(
            ShapeKind::Rect { a: Pt::new(10.0, 10.0), b: Pt::new(40.0, 40.0) },
            [255, 0, 0, 255],
            2.0,
        );
        s.fill = true;
        s.shadow = true;
        d.add_shape(s);
        let out = flatten(&base, &d);
        assert_eq!(out.pixel(25, 25), [255, 0, 0, 255]); // filled interior
        let sh = out.pixel(42, 42); // shadow only
        assert!(sh[0] < 255 && sh[0] == sh[1], "{sh:?}");
        assert_eq!(out.pixel(55, 55), [255, 255, 255, 255]);
    }

    #[test]
    fn wrap_breaks_words_and_long_runs() {
        let px = 20.0;
        let w = line_px_width("hello world", px);
        let lines = wrap_text("hello world hello", px, w + 0.5);
        assert_eq!(lines, vec!["hello world".to_string(), "hello".to_string()]);
        let long = wrap_text("aaaaaaaaaaaaaaaaaaaa", px, line_px_width("aaaaa", px) + 0.5);
        assert_eq!(long.len(), 4);
        assert!(long.iter().all(|l| l == "aaaaa"));
        assert_eq!(wrap_text("a\n\nb", px, 500.0).len(), 3);
        assert_eq!(wrap_text("", px, 500.0), vec![String::new()]);
    }

    #[test]
    fn textbox_grows_to_fit() {
        let px = 18.0;
        let one = textbox_layout(Pt::new(10.0, 10.0), Pt::new(200.0, 12.0), "hi", px);
        let many = textbox_layout(Pt::new(10.0, 10.0), Pt::new(200.0, 12.0), "word word word word word word word word", px);
        assert!(one.max.y > 12.0);
        assert!(many.lines.len() > 1);
        assert!(many.max.y > one.max.y);
        assert_eq!(many.max.x, 200.0);
        // a tall drag keeps its height
        let tall = textbox_layout(Pt::new(0.0, 0.0), Pt::new(100.0, 300.0), "hi", px);
        assert_eq!(tall.max.y, 300.0);
    }

    #[test]
    fn contrast_picks_readable() {
        assert_eq!(contrast_on([255, 255, 255, 255]), [0, 0, 0, 255]);
        assert_eq!(contrast_on([253, 216, 53, 255]), [0, 0, 0, 255]);
        assert_eq!(contrast_on([229, 57, 53, 255]), [255, 255, 255, 255]);
        assert_eq!(contrast_on([0, 0, 0, 255]), [255, 255, 255, 255]);
    }

    #[test]
    fn dilate_grows_a_dot() {
        let (w, h) = (9, 9);
        let mut m = vec![0f32; w * h];
        m[4 * w + 4] = 1.0;
        let d = dilate(&m, w, h, 2.0);
        assert_eq!(d[4 * w + 4], 1.0);
        assert_eq!(d[4 * w + 5], 1.0);
        assert!((d[4 * w + 6] - 0.5).abs() < 1e-6); // anti-aliased rim
        assert_eq!(d[0], 0.0);
    }

    #[test]
    fn outlined_text_gets_a_halo_on_same_colour_background() {
        // Red text on a red background: without halo nothing changes.
        let red = [229, 57, 53, 255];
        let base = solid(160, 60, red);
        let mut d = Document::new();
        let mut s = Shape::new(ShapeKind::Text { pos: Pt::new(10.0, 10.0), text: "Halo".into() }, red, 4.0);
        d.add_shape(s.clone());
        assert_eq!(flatten(&base, &d).pixels, base.pixels);
        s.outline = true;
        let mut d = Document::new();
        d.add_shape(s);
        let out = flatten(&base, &d);
        let white = out.pixels.chunks_exact(4).filter(|p| p[0] > 240 && p[1] > 240).count();
        assert!(white > 30, "white={white}");
    }

    #[test]
    fn filled_textbox_uses_contrasting_text() {
        let base = solid(240, 120, [255, 255, 255, 255]);
        let mut s = Shape::new(
            ShapeKind::TextBox { a: Pt::new(10.0, 10.0), b: Pt::new(200.0, 20.0), text: "Catatan".into() },
            [30, 136, 229, 255],
            3.0,
        );
        s.fill = true;
        let mut d = Document::new();
        d.add_shape(s.clone());
        let out = flatten(&base, &d);
        // corner inside the padding is the fill colour
        assert_eq!(out.pixel(16, 16), [30, 136, 229, 255]);
        // some white text pixels inside the box
        let (min, max) = s.bounds();
        let mut white = 0;
        for y in min.y as u32..max.y as u32 {
            for x in min.x as u32..max.x as u32 {
                if out.pixel(x, y)[0] > 200 {
                    white += 1;
                }
            }
        }
        assert!(white > 20, "white={white}");
        // outline-only: interior stays the background
        s.fill = false;
        let mut d = Document::new();
        d.add_shape(s);
        let out = flatten(&base, &d);
        assert_eq!(out.pixel(16, 16), [255, 255, 255, 255]);
        assert!(out.pixel(100, 10)[0] < 100, "border {:?}", out.pixel(100, 10));
    }

    #[test]
    fn callout_tail_reaches_tip() {
        let base = solid(300, 200, [0, 0, 0, 255]);
        let mut s = Shape::new(
            ShapeKind::Callout {
                tip: Pt::new(40.0, 170.0),
                a: Pt::new(120.0, 150.0),
                b: Pt::new(120.0, 150.0),
                text: "Klik ini".into(),
            },
            [255, 255, 255, 255],
            3.0,
        );
        s.fill = true;
        let mut d = Document::new();
        d.add_shape(s.clone());
        let out = flatten(&base, &d);
        // near the tip, along the tail
        assert!(out.pixel(60, 163)[0] > 128, "{:?}", out.pixel(60, 163));
        // far from both bubble and tail
        assert_eq!(out.pixel(250, 180), [0, 0, 0, 255]);
        let (min, max) = s.bounds();
        assert!(min.x <= 40.0 && max.y >= 170.0);
        assert!(s.hit(Pt::new(60.0, 163.0), 2.0));
        assert!(!s.hit(Pt::new(250.0, 180.0), 2.0));
    }

    #[test]
    fn callout_auto_width_extends_away_from_tip() {
        let px = 18.0;
        let tip = Pt::new(100.0, 100.0);
        // corner right of the tip: grows right
        let r = callout_layout(tip, Pt::new(120.0, 60.0), Pt::new(120.0, 60.0), "Halo", px);
        assert_eq!(r.min.x, 120.0);
        assert!(r.max.x > 120.0 + px);
        // corner left of the tip: grows left, ends at the corner
        let l = callout_layout(tip, Pt::new(80.0, 60.0), Pt::new(80.0, 60.0), "Halo", px);
        assert!((l.max.x - 80.0).abs() < 1e-3);
        // above the tip: grows upwards, bottom edge stays at the corner
        assert!(l.min.y < 60.0 && (l.max.y - 60.0).abs() < 1e-3);
        // below the tip: grows downwards
        let d = callout_layout(tip, Pt::new(120.0, 140.0), Pt::new(120.0, 140.0), "Halo", px);
        assert_eq!(d.min.y, 140.0);
        assert!(d.max.y > 140.0);
    }

    #[test]
    fn callout_wraps_long_text_at_16em_and_explicit_width_wraps() {
        let px = 18.0;
        let tip = Pt::new(0.0, 0.0);
        let long = "kata ".repeat(40);
        let auto = callout_layout(tip, Pt::new(50.0, 50.0), Pt::new(50.0, 50.0), &long, px);
        assert!(auto.max.x - auto.min.x <= px * CALLOUT_MAX_EM + 2.0 * px * BOX_PAD_EM + 2.0);
        assert!(auto.lines.len() > 1);
        let one = callout_layout(tip, Pt::new(50.0, 50.0), Pt::new(50.0, 50.0), "hi", px);
        assert_eq!(one.lines.len(), 1);
        // explicit narrow box: wraps more, taller
        let narrow = callout_layout(tip, Pt::new(50.0, 50.0), Pt::new(50.0 + px * 6.0, 60.0), &long, px);
        assert!(narrow.lines.len() > auto.lines.len());
        assert!((narrow.max.x - narrow.min.x - px * 6.0).abs() < 1e-3);
    }

    #[test]
    fn preview_matches_flatten() {
        let base = Image::new(200, 100);
        let mut s = Shape::new(
            ShapeKind::TextBox { a: Pt::new(20.0, 20.0), b: Pt::new(150.0, 30.0), text: "abc def".into() },
            [229, 57, 53, 255],
            3.0,
        );
        s.fill = true;
        let mut d = Document::new();
        d.add_shape(s.clone());
        let flat = flatten(&base, &d);
        let (min, _max, img) = text_shape_preview(&s, 1.0, false).unwrap();
        for y in 0..img.height {
            for x in 0..img.width {
                let (gx, gy) = (min.x as i32 + x as i32, min.y as i32 + y as i32);
                if gx >= 0 && gy >= 0 && (gx as u32) < 200 && (gy as u32) < 100 {
                    assert_eq!(img.pixel(x, y), flat.pixel(gx as u32, gy as u32), "at {gx},{gy}");
                }
            }
        }
    }

    #[test]
    fn no_shapes_is_identity() {
        let base = gradient(16, 16);
        let out = flatten(&base, &Document::new());
        assert_eq!(out.pixels, base.pixels);
    }

    fn hgrad(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize * 4;
                img.pixels[i..i + 4].copy_from_slice(&[x as u8, x as u8, x as u8, 255]);
            }
        }
        img
    }

    fn spot(x0: f32, y0: f32, x1: f32, y1: f32) -> Shape {
        Shape::new(ShapeKind::Spotlight { a: Pt::new(x0, y0), b: Pt::new(x1, y1) }, [0; 4], 1.0)
    }

    #[test]
    fn spotlight_dims_outside_union_only() {
        let base = solid(100, 100, [255, 255, 255, 255]);
        let mut d = Document::new();
        d.add_shape(spot(20.0, 20.0, 60.0, 60.0));
        d.add_shape(spot(40.0, 40.0, 90.0, 90.0)); // overlaps the first
        let out = flatten(&base, &d);
        assert_eq!(out.pixel(30, 30), [255, 255, 255, 255], "inside 1st");
        assert_eq!(out.pixel(50, 50), [255, 255, 255, 255], "inside both");
        assert_eq!(out.pixel(80, 80), [255, 255, 255, 255], "inside 2nd");
        let dark = out.pixel(5, 5);
        let want = 255.0 * (1.0 - crate::doc::SPOTLIGHT_DIM_ALPHA as f32 / 255.0);
        assert!((dark[0] as f32 - want).abs() <= 1.5, "{dark:?} vs {want}");
        assert_eq!(dark[3], 255);
        // A pixel outside both is dimmed once, not once per spotlight.
        assert_eq!(out.pixel(95, 10), dark);
    }

    #[test]
    fn spotlight_ellipse_dims_bbox_corner() {
        let base = solid(100, 100, [255, 255, 255, 255]);
        let mut d = Document::new();
        let mut s = spot(10.0, 10.0, 90.0, 90.0);
        s.fill = true; // elliptical
        d.add_shape(s);
        let out = flatten(&base, &d);
        assert_eq!(out.pixel(50, 50), [255, 255, 255, 255]);
        assert!(out.pixel(12, 12)[0] < 200, "corner outside the ellipse is dimmed");
    }

    #[test]
    fn spotlight_dims_shapes_below_not_above() {
        let base = solid(100, 100, [255, 255, 255, 255]);
        let mut d = Document::new();
        let red_rect = |a: Pt, b: Pt| Shape { fill: true, ..Shape::new(ShapeKind::Rect { a, b }, [255, 0, 0, 255], 1.0) };
        d.add_shape(red_rect(Pt::new(0.0, 0.0), Pt::new(20.0, 20.0)));
        d.add_shape(spot(40.0, 40.0, 60.0, 60.0));
        d.add_shape(red_rect(Pt::new(80.0, 80.0), Pt::new(100.0, 100.0)));
        let out = flatten(&base, &d);
        assert!(out.pixel(5, 5)[1] < 100 && out.pixel(5, 5)[0] > 100, "red below the spotlight is dimmed");
        assert_eq!(out.pixel(90, 90), [255, 0, 0, 255], "red above stays bright");
    }

    #[test]
    fn magnify_samples_offset_over_zoom() {
        let base = hgrad(200, 100);
        let mut d = Document::new();
        d.add_shape(Shape::new(
            ShapeKind::Magnify { center: Pt::new(100.0, 50.0), radius: 40.0, zoom: 2.0 },
            [255, 0, 0, 255],
            2.0,
        ));
        let out = flatten(&base, &d);
        // pixel 120 is +20.5 from the centre -> source x = 100 + 20.5 / 2 = 110.25
        // (pixel i holds value i at its centre i + 0.5, so the value is ~109.75).
        let v = out.pixel(120, 50)[0] as i32;
        assert!((v - 110).abs() <= 1, "{v}");
        let v = out.pixel(80, 50)[0] as i32; // -19.5 -> 90.25 -> ~89.75
        assert!((v - 90).abs() <= 1, "{v}");
        // Outside the circle and its border: untouched.
        assert_eq!(out.pixel(150, 50), base.pixel(150, 50));
        assert_eq!(out.pixel(100, 5), base.pixel(100, 5));
        // The border (inside the edge) takes the shape colour.
        assert_eq!(out.pixel(100 + 39, 50), [255, 0, 0, 255]);
    }

    #[test]
    fn expand_effects_fills_placeholders() {
        let spec = jepret_core::EffectSpec {
            watermark: Some(jepret_core::effects::Watermark { text: "{date} {width}x{height}".into(), ..Default::default() }),
            ..Default::default()
        };
        let now = chrono::NaiveDate::from_ymd_opt(2026, 3, 4).and_then(|d| d.and_hms_opt(5, 6, 7)).unwrap_or_default();
        let out = expand_effects(&spec, now, (10, 20));
        assert_eq!(out.watermark.map(|w| w.text).as_deref(), Some("2026-03-04 10x20"));
    }

    #[test]
    fn export_without_effects_equals_flatten() {
        let base = gradient(20, 20);
        let d = Document::new();
        let now = chrono::NaiveDateTime::default();
        assert_eq!(export(&base, &d, now).pixels, flatten(&base, &d).pixels);
    }
}
