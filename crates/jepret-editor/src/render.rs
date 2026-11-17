//! Flatten a [`Document`] onto an [`Image`] (save / clipboard). Pure CPU,
//! no egui. Anti-aliasing via signed-distance coverage; blur/pixelate read
//! from the *original* pixels.

use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use jepret_core::{Image, Rect};

use crate::doc::{Document, Pt, Rgba, Shape, ShapeKind};

fn font() -> Option<&'static FontRef<'static>> {
    use std::sync::OnceLock;
    static FONT: OnceLock<Option<FontRef<'static>>> = OnceLock::new();
    FONT.get_or_init(|| FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).ok())
        .as_ref()
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
    for shape in doc.shapes() {
        draw_shape(&mut canvas, base, shape);
    }
    match doc.crop() {
        Some(r) => canvas.crop(r).unwrap_or(canvas),
        None => canvas,
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
        ShapeKind::Text { pos, text } => {
            draw_text(canvas, *pos, text, shape.text_px(), c, false);
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
    fn no_shapes_is_identity() {
        let base = gradient(16, 16);
        let out = flatten(&base, &Document::new());
        assert_eq!(out.pixels, base.pixels);
    }
}
