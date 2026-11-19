//! Image effects (pure, no Win32/egui): rounded corners, border, drop
//! shadow, torn edges and a text watermark. Used by the workflow action
//! `effects` and by the editor's effect panel.

#![allow(clippy::chunks_exact_to_as_chunks)]

use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use jepret_core::effects::{parse_hex_color, Anchor, Watermark};
use jepret_core::{EffectSpec, Image};

/// Box-blur radius per pass for a requested shadow radius (3 passes).
fn pass_radius(blur: u32) -> usize {
    blur.div_ceil(3) as usize
}

/// Margin the blurred shadow extends beyond the shape.
fn blur_extent(blur: u32) -> i64 {
    3 * pass_radius(blur) as i64
}

/// Apply `spec` to `image` and return the result. Watermark text must be
/// already expanded (placeholders) by the caller. The output may be larger
/// than the input (shadow margin) and has transparent pixels outside the
/// shape (corners, torn edges, shadow area).
pub fn apply(image: &Image, spec: &EffectSpec) -> Image {
    if spec.is_noop() {
        return image.clone();
    }
    let (w, h) = (image.width as usize, image.height as usize);
    if image.pixels.len() != w * h * 4 {
        tracing::warn!("effects: pixel buffer does not match dimensions");
        return image.clone();
    }
    let t0 = std::time::Instant::now();
    let mut img = image.clone();
    if w > 0 && h > 0 {
        torn_edges(&mut img, spec);
        round_corners(&mut img, spec.corner_radius);
        border(&mut img, spec);
        if let Some(wm) = &spec.watermark {
            watermark(&mut img, wm);
        }
    }
    let out = if spec.shadow_blur > 0 { drop_shadow(img, spec) } else { img };
    tracing::debug!("effects applied in {:?}", t0.elapsed());
    out
}

/// Size of [`apply`]'s output for an input of `w`×`h`, and the offset of
/// the original image's top-left inside it (for previews).
pub fn output_layout(w: u32, h: u32, spec: &EffectSpec) -> (u32, u32, i32, i32) {
    if spec.is_noop() || spec.shadow_blur == 0 {
        return (w, h, 0, 0);
    }
    let e = blur_extent(spec.shadow_blur);
    let span = |len: i64, o: i64| {
        let min = (o - e).min(0);
        let max = len.max(len + o + e);
        ((max - min) as u32, (-min) as i32)
    };
    let (ow, dx) = span(w as i64, spec.shadow_offset_x as i64);
    let (oh, dy) = span(h as i64, spec.shadow_offset_y as i64);
    (ow, oh, dx, dy)
}

// ---------------------------------------------------------------- helpers

#[inline]
fn mul_alpha(img: &mut Image, x: usize, y: usize, cov: f32) {
    let i = (y * img.width as usize + x) * 4 + 3;
    let a = img.pixels[i] as f32 * cov.clamp(0.0, 1.0);
    img.pixels[i] = (a + 0.5) as u8;
}

/// Straight-alpha "over" of `src` (rgb, alpha 0..1) onto the pixel `px`.
#[inline]
fn over(px: &mut [u8], src: [u8; 3], sa: f32) {
    if sa <= 0.0 {
        return;
    }
    let da = px[3] as f32 / 255.0;
    let oa = sa + da * (1.0 - sa);
    if oa <= 0.0 {
        return;
    }
    for c in 0..3 {
        let v = (src[c] as f32 * sa + px[c] as f32 * da * (1.0 - sa)) / oa;
        px[c] = (v + 0.5).clamp(0.0, 255.0) as u8;
    }
    px[3] = (oa * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
}

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }
    /// Uniform in 0..1 (splitmix64).
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 40) as f32 / (1u64 << 24) as f32
    }
}

// ------------------------------------------------------------ torn edges

/// Per-pixel tear depth along a side of `len` px: saw-tooth with random
/// tooth widths and heights, in 0..=`size`.
fn torn_profile(len: usize, size: f32, seed: u64) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(len);
    let mut x0 = 0usize;
    let mut h0 = size * (0.15 + 0.85 * rng.next());
    while x0 < len {
        let step = ((size * (0.4 + 0.8 * rng.next())) as usize).max(2);
        let x1 = x0 + step;
        let h1 = size * (0.15 + 0.85 * rng.next());
        for x in x0..x1.min(len) {
            let t = (x - x0) as f32 / step as f32;
            out.push(h0 + (h1 - h0) * t);
        }
        x0 = x1;
        h0 = h1;
    }
    out
}

fn torn_edges(img: &mut Image, spec: &EffectSpec) {
    let t = spec.torn;
    if !t.any() || spec.torn_size == 0 {
        return;
    }
    let (w, h) = (img.width as usize, img.height as usize);
    let base = ((img.width as u64) << 32) | img.height as u64;
    let size = |dim: usize| (spec.torn_size as f32).min(dim as f32 / 2.0);
    // (enabled, side index, length along side, max depth)
    let sides = [
        (t.top, 0u64, w, size(h)),
        (t.bottom, 1, w, size(h)),
        (t.left, 2, h, size(w)),
        (t.right, 3, h, size(w)),
    ];
    for (on, side, len, sz) in sides {
        if !on || sz <= 0.0 {
            continue;
        }
        let prof = torn_profile(len, sz, base ^ (side + 1).wrapping_mul(0xA5A5_1234_5678_9ABD));
        for (i, &d) in prof.iter().enumerate() {
            let rows = d.ceil() as usize;
            for k in 0..rows {
                let cov = (k as f32 + 1.0 - d).clamp(0.0, 1.0);
                let (x, y) = match side {
                    0 => (i, k),
                    1 => (i, h - 1 - k),
                    2 => (k, i),
                    _ => (w - 1 - k, i),
                };
                mul_alpha(img, x, y, cov);
            }
        }
    }
}

// --------------------------------------------------------- rounded corners

fn clamp_radius(w: usize, h: usize, r: u32) -> f32 {
    (r as f32).min(w.min(h) as f32 / 2.0)
}

fn round_corners(img: &mut Image, radius: u32) {
    let (w, h) = (img.width as usize, img.height as usize);
    let r = clamp_radius(w, h, radius);
    if r <= 0.0 {
        return;
    }
    let n = r.ceil() as usize;
    for y in 0..n {
        for x in 0..n {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            if px >= r || py >= r {
                continue;
            }
            let d = ((r - px).powi(2) + (r - py).powi(2)).sqrt() - r;
            let cov = (0.5 - d).clamp(0.0, 1.0);
            if cov >= 1.0 {
                continue;
            }
            for (ix, iy) in [(x, y), (w - 1 - x, y), (x, h - 1 - y), (w - 1 - x, h - 1 - y)] {
                mul_alpha(img, ix, iy, cov);
            }
        }
    }
}

// ------------------------------------------------------------------ border

fn border(img: &mut Image, spec: &EffectSpec) {
    if spec.border_width == 0 {
        return;
    }
    let Some(col) = parse_hex_color(&spec.border_color) else {
        tracing::warn!("effects: invalid border color {:?}", spec.border_color);
        return;
    };
    let (w, h) = (img.width as usize, img.height as usize);
    let bw = spec.border_width as f32;
    let r = clamp_radius(w, h, spec.corner_radius);
    let tn = spec.torn;
    let m = (bw.max(r).ceil() as usize).min(w).min(h);
    let (wf, hf) = (w as f32, h as f32);
    let ca = col[3] as f32 / 255.0;
    for y in 0..h {
        let full = y < m || y + m >= h || 2 * m >= w;
        let mut x = 0;
        while x < w {
            if !full && x == m {
                x = w - m;
            }
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut t = f32::INFINITY;
            if !tn.left {
                t = t.min(px);
            }
            if !tn.right {
                t = t.min(wf - px);
            }
            if !tn.top {
                t = t.min(py);
            }
            if !tn.bottom {
                t = t.min(hf - py);
            }
            if r > 0.0 {
                // Which corner square (if any) this pixel is in, and whether
                // both adjacent sides are intact (not torn).
                let left = px < r;
                let right = px > wf - r;
                let top = py < r;
                let bottom = py > hf - r;
                let hit = match (left, right, top, bottom) {
                    (true, _, true, _) if !tn.left && !tn.top => Some((r, r)),
                    (_, true, true, _) if !tn.right && !tn.top => Some((wf - r, r)),
                    (true, _, _, true) if !tn.left && !tn.bottom => Some((r, hf - r)),
                    (_, true, _, true) if !tn.right && !tn.bottom => Some((wf - r, hf - r)),
                    _ => None,
                };
                if let Some((cx, cy)) = hit {
                    t = r - ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                }
            }
            if t.is_finite() {
                let cov = (bw - t + 0.5).clamp(0.0, 1.0) * ca;
                if cov > 0.0 {
                    let i = (y * w + x) * 4;
                    let p = &mut img.pixels[i..i + 4];
                    for c in 0..3 {
                        p[c] = (p[c] as f32 + (col[c] as f32 - p[c] as f32) * cov + 0.5) as u8;
                    }
                }
            }
            x += 1;
        }
    }
}

// --------------------------------------------------------------- watermark

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

fn watermark(img: &mut Image, wm: &Watermark) {
    if wm.text.trim().is_empty() || wm.size == 0 || wm.opacity == 0 {
        return;
    }
    let Some(f) = font() else { return };
    let Some(col) = parse_hex_color(&wm.color) else {
        tracing::warn!("effects: invalid watermark color {:?}", wm.color);
        return;
    };
    let (w, h) = (img.width as i64, img.height as i64);
    let px = wm.size as f32;
    let scale = PxScale::from(px);
    let sf = f.as_scaled(scale);
    let line_h = sf.height() + sf.line_gap();
    let lines: Vec<&str> = wm.text.split('\n').collect();
    let line_w = |s: &str| {
        let mut x = 0.0;
        let mut prev = None;
        for ch in s.chars() {
            let id = sf.glyph_id(ch);
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            x += sf.h_advance(id);
            prev = Some(id);
        }
        x
    };
    let bw = lines.iter().map(|l| line_w(l)).fold(0.0f32, f32::max).ceil() as i64;
    let bh = (line_h * lines.len() as f32).ceil() as i64;
    let d = ((px / 16.0).round() as i64).max(1); // shadow offset
    let pad = d + 2;
    let (cw, ch) = ((bw + 2 * pad) as usize, (bh + 2 * pad) as usize);
    if cw.saturating_mul(ch) > 64_000_000 {
        return;
    }
    let mut cov = vec![0u8; cw * ch];
    for (li, line) in lines.iter().enumerate() {
        let mut x = pad as f32;
        let baseline = pad as f32 + sf.ascent() + li as f32 * line_h;
        let mut prev = None;
        for c in line.chars() {
            let id = sf.glyph_id(c);
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            if let Some(og) = f.outline_glyph(id.with_scale_and_position(scale, point(x, baseline))) {
                let b = og.px_bounds();
                og.draw(|gx, gy, v| {
                    let (cx, cy) = (b.min.x as i64 + gx as i64, b.min.y as i64 + gy as i64);
                    if cx >= 0 && cy >= 0 && (cx as usize) < cw && (cy as usize) < ch {
                        let p = &mut cov[cy as usize * cw + cx as usize];
                        *p = (*p).max((v.clamp(0.0, 1.0) * 255.0) as u8);
                    }
                });
            }
            x += sf.h_advance(id);
            prev = Some(id);
        }
    }
    let m = wm.margin as i64;
    use Anchor::*;
    let tx = match wm.position {
        TopLeft | Left | BottomLeft => m,
        Top | Center | Bottom => (w - bw) / 2,
        TopRight | Right | BottomRight => w - m - bw,
    };
    let ty = match wm.position {
        TopLeft | Top | TopRight => m,
        Left | Center | Right => (h - bh) / 2,
        BottomLeft | Bottom | BottomRight => h - m - bh,
    };
    let op = wm.opacity.min(100) as f32 / 100.0;
    let ta = op * col[3] as f32 / 255.0;
    let (ox, oy) = (tx - pad, ty - pad);
    for by in 0..ch as i64 {
        let iy = oy + by;
        if iy < 0 || iy >= h {
            continue;
        }
        for bx in 0..cw as i64 {
            let ix = ox + bx;
            if ix < 0 || ix >= w {
                continue;
            }
            let c = cov[by as usize * cw + bx as usize] as f32 / 255.0;
            let (sx, sy) = (bx - d, by - d);
            let s = if sx >= 0 && sy >= 0 && (sx as usize) < cw && (sy as usize) < ch {
                cov[sy as usize * cw + sx as usize] as f32 / 255.0
            } else {
                0.0
            };
            if c <= 0.0 && s <= 0.0 {
                continue;
            }
            let i = (iy as usize * w as usize + ix as usize) * 4;
            let p = &mut img.pixels[i..i + 4];
            over(p, [0, 0, 0], s * 0.55 * op);
            over(p, [col[0], col[1], col[2]], c * ta);
        }
    }
}

// ------------------------------------------------------------------ shadow

/// One horizontal box-blur pass `src` -> `dst` (zero outside), radius `r`.
fn blur_h(src: &[u8], dst: &mut [u8], w: usize, r: usize) {
    let n = (2 * r + 1) as u32;
    let inv = (1u32 << 24) / n;
    for (s, d) in src.chunks_exact(w).zip(dst.chunks_exact_mut(w)) {
        let mut sum: u32 = s[..r.min(w)].iter().map(|&v| v as u32).sum();
        for x in 0..w {
            if x + r < w {
                sum += s[x + r] as u32;
            }
            if x > r {
                sum -= s[x - r - 1] as u32;
            }
            d[x] = ((sum * inv + (1 << 23)) >> 24).min(255) as u8;
        }
    }
}

/// One vertical box-blur pass using a running per-column sum (row-wise, cache friendly).
fn blur_v(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, acc: &mut [u32]) {
    let n = (2 * r + 1) as u32;
    let inv = (1u32 << 24) / n;
    acc.fill(0);
    for y in 0..r.min(h) {
        for (a, &v) in acc.iter_mut().zip(&src[y * w..(y + 1) * w]) {
            *a += v as u32;
        }
    }
    for y in 0..h {
        if y + r < h {
            for (a, &v) in acc.iter_mut().zip(&src[(y + r) * w..(y + r + 1) * w]) {
                *a += v as u32;
            }
        }
        if y > r {
            for (a, &v) in acc.iter_mut().zip(&src[(y - r - 1) * w..(y - r) * w]) {
                *a -= v as u32;
            }
        }
        for (d, &a) in dst[y * w..(y + 1) * w].iter_mut().zip(acc.iter()) {
            *d = ((a * inv + (1 << 23)) >> 24).min(255) as u8;
        }
    }
}

fn drop_shadow(img: Image, spec: &EffectSpec) -> Image {
    let (w, h) = (img.width as usize, img.height as usize);
    let (cw, ch, dx, dy) = output_layout(img.width, img.height, spec);
    let (cw, ch, dx, dy) = (cw as usize, ch as usize, dx as usize, dy as usize);
    let mut out = Image::new(cw as u32, ch as u32);
    if cw == 0 || ch == 0 {
        return out;
    }
    let col = parse_hex_color(&spec.shadow_color).unwrap_or([0, 0, 0, 0x66]);
    let r = pass_radius(spec.shadow_blur);

    // Alpha mask of the shape, placed at its shadow position.
    let mut a = vec![0u8; cw * ch];
    let sx = (dx as i64 + spec.shadow_offset_x as i64) as usize;
    let sy = (dy as i64 + spec.shadow_offset_y as i64) as usize;
    for y in 0..h {
        let row = &mut a[(sy + y) * cw + sx..(sy + y) * cw + sx + w];
        for (m, px) in row.iter_mut().zip(img.pixels[y * w * 4..(y + 1) * w * 4].chunks_exact(4)) {
            *m = px[3];
        }
    }
    let mut b = vec![0u8; cw * ch];
    let mut acc = vec![0u32; cw];
    for _ in 0..3 {
        blur_h(&a, &mut b, cw, r);
        blur_v(&b, &mut a, cw, ch, r, &mut acc);
    }
    drop(b);

    // Shadow colour + blurred alpha.
    let sa = col[3] as u32;
    for (px, &m) in out.pixels.chunks_exact_mut(4).zip(&a) {
        if m != 0 {
            px[0] = col[0];
            px[1] = col[1];
            px[2] = col[2];
            px[3] = ((m as u32 * sa + 127) / 255) as u8;
        }
    }
    // Image over shadow.
    for y in 0..h {
        let o = ((dy + y) * cw + dx) * 4;
        let dst = &mut out.pixels[o..o + w * 4];
        let src = &img.pixels[y * w * 4..(y + 1) * w * 4];
        for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            match s[3] {
                0 => {}
                255 => d.copy_from_slice(s),
                al => over(d, [s[0], s[1], s[2]], al as f32 / 255.0),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use jepret_core::effects::TornEdges;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Image {
        let mut im = Image::new(w, h);
        for p in im.pixels.chunks_exact_mut(4) {
            p.copy_from_slice(&c);
        }
        im
    }

    #[test]
    fn noop_identity() {
        let im = solid(10, 8, [1, 2, 3, 255]);
        assert_eq!(apply(&im, &EffectSpec::default()), im);
        assert_eq!(output_layout(10, 8, &EffectSpec::default()), (10, 8, 0, 0));
    }

    #[test]
    fn layout_matches_apply() {
        for (ox, oy) in [(5, 7), (-9, -4), (0, 0), (-3, 12)] {
            let spec = EffectSpec { shadow_blur: 8, shadow_offset_x: ox, shadow_offset_y: oy, ..Default::default() };
            let im = solid(40, 30, [200, 10, 10, 255]);
            let out = apply(&im, &spec);
            let (w, h, dx, dy) = output_layout(40, 30, &spec);
            assert_eq!((out.width, out.height), (w, h), "offset {ox},{oy}");
            assert!(dx >= 0 && dy >= 0);
            assert_eq!(out.pixel(dx as u32 + 20, dy as u32 + 15), [200, 10, 10, 255]);
        }
    }

    #[test]
    fn rounded_corners() {
        let im = solid(60, 60, [255, 0, 0, 255]);
        let out = apply(&im, &EffectSpec { corner_radius: 20, ..Default::default() });
        assert_eq!(out.pixel(0, 0)[3], 0);
        assert_eq!(out.pixel(59, 59)[3], 0);
        assert_eq!(out.pixel(30, 30)[3], 255);
        assert_eq!(out.pixel(30, 0)[3], 255);
    }

    #[test]
    fn border_present() {
        let im = solid(40, 40, [255, 255, 255, 255]);
        let spec = EffectSpec { border_width: 3, border_color: "#ff0000".into(), corner_radius: 8, ..Default::default() };
        let out = apply(&im, &spec);
        assert_eq!(out.pixel(20, 0), [255, 0, 0, 255]);
        assert_eq!(out.pixel(0, 20), [255, 0, 0, 255]);
        assert_eq!(out.pixel(20, 20), [255, 255, 255, 255]);
        // follows the rounded corner: a point just inside the arc is border
        assert_eq!(out.pixel(3, 3)[3], 255);
        assert_ne!(out.pixel(3, 3), [255, 255, 255, 255]);
    }

    #[test]
    fn torn_reproducible_and_opposite_intact() {
        let im = solid(80, 60, [10, 20, 30, 255]);
        let spec = EffectSpec { torn: TornEdges { top: true, ..Default::default() }, torn_size: 10, ..Default::default() };
        let a = apply(&im, &spec);
        assert_eq!(a, apply(&im, &spec));
        assert!((0..80).any(|x| a.pixel(x, 0)[3] == 0), "top is torn");
        for x in 0..80 {
            assert_eq!(a.pixel(x, 59), [10, 20, 30, 255]);
            assert_eq!(a.pixel(x, 20)[3], 255);
        }
        for y in 0..60 {
            assert!(a.pixel(0, y)[3] == 255 || y < 10);
            assert!(a.pixel(79, y)[3] == 255 || y < 10);
        }
    }

    #[test]
    fn watermark_local() {
        let im = solid(200, 100, [0, 0, 255, 255]);
        let spec = EffectSpec {
            watermark: Some(Watermark { text: "Hello".into(), position: Anchor::BottomRight, size: 20, margin: 5, opacity: 100, ..Default::default() }),
            ..Default::default()
        };
        let out = apply(&im, &spec);
        let changed: Vec<(u32, u32)> = (0..100u32)
            .flat_map(|y| (0..200u32).map(move |x| (x, y)))
            .filter(|&(x, y)| out.pixel(x, y) != im.pixel(x, y))
            .collect();
        assert!(!changed.is_empty());
        assert!(changed.iter().all(|&(x, y)| x > 100 && y > 60), "only near bottom-right");
    }

    #[test]
    fn blur_conserves_alpha() {
        let (cw, ch, r) = (120usize, 100usize, 4usize);
        let mut a = vec![0u8; cw * ch];
        for y in 30..60 {
            for x in 40..80 {
                a[y * cw + x] = 255;
            }
        }
        let before: u64 = a.iter().map(|&v| v as u64).sum();
        let mut b = vec![0u8; cw * ch];
        let mut acc = vec![0u32; cw];
        for _ in 0..3 {
            blur_h(&a, &mut b, cw, r);
            blur_v(&b, &mut a, cw, ch, r, &mut acc);
        }
        let after: u64 = a.iter().map(|&v| v as u64).sum();
        let diff = after.abs_diff(before) as f64 / before as f64;
        assert!(diff < 0.02, "diff {diff}");
        assert!(a[45 * cw + 38] > 0 && a[45 * cw + 60] == 255);
    }

    #[test]
    fn extremes_do_not_panic() {
        let spec = EffectSpec {
            corner_radius: 1000,
            border_width: 500,
            torn: TornEdges { top: true, right: true, bottom: true, left: true },
            torn_size: 500,
            shadow_blur: 50,
            shadow_offset_x: -100,
            shadow_offset_y: 100,
            watermark: Some(Watermark { text: "x\ny".into(), size: 500, ..Default::default() }),
            ..Default::default()
        };
        for (w, h) in [(0, 0), (1, 1), (3, 200), (30, 20)] {
            let out = apply(&solid(w, h, [1, 2, 3, 255]), &spec);
            let (ow, oh, _, _) = output_layout(w, h, &spec);
            assert_eq!((out.width, out.height), (ow, oh));
        }
        let s0 = EffectSpec { corner_radius: 5, shadow_blur: 0, ..Default::default() };
        let _ = apply(&solid(4, 4, [0; 4]), &s0);
    }
}
