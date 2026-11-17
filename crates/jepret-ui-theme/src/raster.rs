//! Tiny deterministic vector rasteriser for Jepret's own icons (T15f).
//!
//! Shared between `build.rs` (which writes the `.ico` + `.res` embedded in the
//! exe) and the daemon (tray + tray-menu icons) via `#[path]`, so it is
//! `std`-only. Shapes are anti-aliased through distance coverage: a stroke is
//! the union of round-capped capsules around the flattened SVG path, a fill is
//! a disc / rounded box. No assets are downloaded or shipped.
#![allow(dead_code)]

/// `Icon::Frame` of `jepret-ui-theme` (four viewfinder corners), 24-unit grid.
/// A test keeps it identical to the theme's path.
pub const FRAME_PATH: &str =
    "M4 9V5a1 1 0 0 1 1-1h4 M15 4h4a1 1 0 0 1 1 1v4 M20 15v4a1 1 0 0 1-1 1h-4 M9 20H5a1 1 0 0 1-1-1v-4";

pub const ACCENT: [u8; 3] = [0x0E, 0x7C, 0x86];
pub const TRAY_LIGHT_ON_DARK: [u8; 3] = [0x4F, 0xC3, 0xCF];
pub const TRAY_DARK_ON_LIGHT: [u8; 3] = [0x1B, 0x1F, 0x24];
pub const RECORD_RED: [u8; 3] = [0xFF, 0x6B, 0x5E];

/// A straight line segment in path units.
pub type Seg = [f32; 4];

// ------------------------------------------------------------------ path parsing

struct Tok<'a> {
    s: &'a [u8],
    i: usize,
}

impl Tok<'_> {
    fn skip(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b',' | b'\n' | b'\t') {
            self.i += 1;
        }
    }
    fn peek_cmd(&mut self) -> Option<u8> {
        self.skip();
        self.s.get(self.i).copied().filter(|c| c.is_ascii_alphabetic())
    }
    fn has_number(&mut self) -> bool {
        self.skip();
        self.s.get(self.i).is_some_and(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.'))
    }
    fn num(&mut self) -> Option<f32> {
        self.skip();
        let st = self.i;
        if self.s.get(self.i).is_some_and(|c| matches!(c, b'-' | b'+')) {
            self.i += 1;
        }
        let mut dot = false;
        while let Some(&c) = self.s.get(self.i) {
            if c.is_ascii_digit() {
                self.i += 1;
            } else if c == b'.' && !dot {
                dot = true;
                self.i += 1;
            } else {
                break;
            }
        }
        std::str::from_utf8(&self.s[st..self.i]).ok()?.parse().ok()
    }
    /// Arc flags may be glued to the next number (`011`), so read one digit.
    fn flag(&mut self) -> Option<bool> {
        self.skip();
        let c = *self.s.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

/// Flatten an SVG path (`M L H V A Z` + lower-case relatives; the subset used
/// by the icon set) into line segments. Unsupported commands end parsing.
pub fn flatten_path(d: &str) -> Vec<Seg> {
    let mut t = Tok { s: d.as_bytes(), i: 0 };
    let mut out = Vec::new();
    let (mut x, mut y, mut sx, mut sy) = (0f32, 0f32, 0f32, 0f32);
    let mut cmd = b'M';
    loop {
        if let Some(c) = t.peek_cmd() {
            t.i += 1;
            cmd = c;
            if cmd == b'z' || cmd == b'Z' {
                if (x, y) != (sx, sy) {
                    out.push([x, y, sx, sy]);
                }
                x = sx;
                y = sy;
                continue;
            }
        } else if !t.has_number() {
            break;
        }
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { (x, y) } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let (Some(a), Some(b)) = (t.num(), t.num()) else { break };
                x = ox + a;
                y = oy + b;
                sx = x;
                sy = y;
                cmd = if rel { b'l' } else { b'L' };
            }
            b'L' => {
                let (Some(a), Some(b)) = (t.num(), t.num()) else { break };
                out.push([x, y, ox + a, oy + b]);
                x = ox + a;
                y = oy + b;
            }
            b'H' => {
                let Some(a) = t.num() else { break };
                out.push([x, y, ox + a, y]);
                x = ox + a;
            }
            b'V' => {
                let Some(b) = t.num() else { break };
                out.push([x, y, x, oy + b]);
                y = oy + b;
            }
            b'A' => {
                let (Some(rx), Some(ry), Some(_rot)) = (t.num(), t.num(), t.num()) else { break };
                let (Some(large), Some(sweep)) = (t.flag(), t.flag()) else { break };
                let (Some(a), Some(b)) = (t.num(), t.num()) else { break };
                let (ex, ey) = (ox + a, oy + b);
                arc(&mut out, (x, y), (ex, ey), rx.abs(), ry.abs(), large, sweep);
                x = ex;
                y = ey;
            }
            _ => break,
        }
    }
    out
}

/// SVG endpoint-parameterised arc (no axis rotation) -> polyline.
fn arc(out: &mut Vec<Seg>, p0: (f32, f32), p1: (f32, f32), mut rx: f32, mut ry: f32, large: bool, sweep: bool) {
    if p0 == p1 {
        return;
    }
    if rx == 0.0 || ry == 0.0 {
        out.push([p0.0, p0.1, p1.0, p1.1]);
        return;
    }
    let (dx, dy) = ((p0.0 - p1.0) / 2.0, (p0.1 - p1.1) / 2.0);
    let lam = (dx * dx) / (rx * rx) + (dy * dy) / (ry * ry);
    if lam > 1.0 {
        let s = lam.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * dy * dy - ry * ry * dx * dx;
    let den = rx * rx * dy * dy + ry * ry * dx * dx;
    let mut k = (num / den).max(0.0).sqrt();
    if large == sweep {
        k = -k;
    }
    let (cxp, cyp) = (k * rx * dy / ry, -k * ry * dx / rx);
    let (cx, cy) = (cxp + (p0.0 + p1.0) / 2.0, cyp + (p0.1 + p1.1) / 2.0);
    let a0 = ((dy - cyp) / ry).atan2((dx - cxp) / rx);
    let a1 = ((-dy - cyp) / ry).atan2((-dx - cxp) / rx);
    let mut da = a1 - a0;
    let tau = std::f32::consts::TAU;
    if sweep && da < 0.0 {
        da += tau;
    } else if !sweep && da > 0.0 {
        da -= tau;
    }
    let n = ((da.abs() * rx.max(ry)).ceil() as usize * 2).clamp(4, 96);
    let mut prev = p0;
    for i in 1..=n {
        let a = a0 + da * i as f32 / n as f32;
        let p = if i == n { p1 } else { (cx + rx * a.cos(), cy + ry * a.sin()) };
        out.push([prev.0, prev.1, p.0, p.1]);
        prev = p;
    }
}

// ------------------------------------------------------------------ canvas

/// Premultiplied RGBA float canvas.
pub struct Canvas {
    pub size: u32,
    px: Vec<[f32; 4]>,
}

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

fn dist_seg(px: f32, py: f32, s: &Seg) -> f32 {
    let (vx, vy) = (s[2] - s[0], s[3] - s[1]);
    let (wx, wy) = (px - s[0], py - s[1]);
    let l2 = vx * vx + vy * vy;
    let t = if l2 > 0.0 { clamp01((wx * vx + wy * vy) / l2) } else { 0.0 };
    let (qx, qy) = (s[0] + t * vx, s[1] + t * vy);
    ((px - qx).powi(2) + (py - qy).powi(2)).sqrt()
}

impl Canvas {
    pub fn new(size: u32) -> Self {
        Canvas { size, px: vec![[0.0; 4]; (size * size) as usize] }
    }

    fn paint(&mut self, color: [u8; 3], mut cover: impl FnMut(f32, f32) -> f32) {
        let c = color.map(|v| v as f32 / 255.0);
        for y in 0..self.size {
            for x in 0..self.size {
                let a = clamp01(cover(x as f32 + 0.5, y as f32 + 0.5));
                if a <= 0.0 {
                    continue;
                }
                let d = &mut self.px[(y * self.size + x) as usize];
                let inv = 1.0 - a;
                for (dk, ck) in d.iter_mut().zip(c) {
                    *dk = *dk * inv + ck * a;
                }
                d[3] = d[3] * inv + a;
            }
        }
    }

    /// Rounded box filling the whole canvas.
    pub fn rounded_bg(&mut self, color: [u8; 3], radius: f32) {
        let h = self.size as f32 / 2.0;
        self.paint(color, |x, y| {
            let (qx, qy) = ((x - h).abs() - (h - radius), (y - h).abs() - (h - radius));
            let d = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;
            0.5 - d
        });
    }

    pub fn disc(&mut self, color: [u8; 3], cx: f32, cy: f32, r: f32) {
        self.paint(color, |x, y| r + 0.5 - (x - cx).hypot(y - cy));
    }

    /// Stroke flattened segments (already in pixel space) with round caps.
    pub fn stroke(&mut self, color: [u8; 3], segs: &[Seg], width: f32) {
        let hw = width / 2.0;
        self.paint(color, |x, y| {
            let d = segs.iter().map(|s| dist_seg(x, y, s)).fold(f32::MAX, f32::min);
            hw + 0.5 - d
        });
    }

    /// Straight-alpha RGBA8.
    pub fn into_rgba(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.px.len() * 4);
        for p in &self.px {
            let a = p[3];
            for &ch in &p[..3] {
                let v = if a > 0.0 { ch / a } else { 0.0 };
                out.push((clamp01(v) * 255.0 + 0.5) as u8);
            }
            out.push((clamp01(a) * 255.0 + 0.5) as u8);
        }
        out
    }
}

/// Map path-space segments (`view` = side of the square viewBox region shown,
/// starting at `(vx, vy)`) onto a `size`-pixel canvas, shrunk to `frac` of the
/// canvas and centred. Returns the segments and the units->pixels scale.
fn place(segs: &[Seg], vx: f32, vy: f32, view: f32, size: u32, frac: f32) -> (Vec<Seg>, f32) {
    let scale = size as f32 * frac / view;
    let off = size as f32 * (1.0 - frac) / 2.0;
    let f = |x: f32, y: f32| ((x - vx) * scale + off, (y - vy) * scale + off);
    let out = segs
        .iter()
        .map(|s| {
            let (a, b) = f(s[0], s[1]);
            let (c, d) = f(s[2], s[3]);
            [a, b, c, d]
        })
        .collect();
    (out, scale)
}

fn circle_segs(cx: f32, cy: f32, r: f32) -> Vec<Seg> {
    flatten_path(&format!("M{} {}a{r} {r} 0 1 1 {} 0a{r} {r} 0 1 1 {} 0", cx - r, cy, 2.0 * r, -2.0 * r))
}

// ------------------------------------------------------------------ icons

/// Application icon: accent rounded square (radius 16/64) with the white
/// viewfinder glyph and lens ring. Returns `(straight RGBA, w, h)`.
pub fn app_icon_rgba(size: u32) -> (Vec<u8>, u32, u32) {
    let mut c = Canvas::new(size);
    c.rounded_bg(ACCENT, size as f32 * 16.0 / 64.0);
    let frac = if size >= 48 { 0.6 } else { 0.7 };
    let mut segs = flatten_path(FRAME_PATH);
    segs.extend(circle_segs(12.0, 12.0, 3.0));
    let (segs, scale) = place(&segs, 0.0, 0.0, 24.0, size, frac);
    c.stroke([255, 255, 255], &segs, (2.2 * scale).max(1.4));
    (c.into_rgba(), size, size)
}

/// Tray icon: corners only (idle) or corners + solid dot (recording), no
/// background. `light_taskbar` picks the dark corner colour for a light taskbar.
pub fn tray_icon_rgba(size: u32, recording: bool, light_taskbar: bool) -> Vec<u8> {
    let mut c = Canvas::new(size);
    let color = if recording {
        RECORD_RED
    } else if light_taskbar {
        TRAY_DARK_ON_LIGHT
    } else {
        TRAY_LIGHT_ON_DARK
    };
    let (segs, scale) = place(&flatten_path(FRAME_PATH), 3.0, 3.0, 18.0, size, 1.0);
    c.stroke(color, &segs, (size as f32 * 0.115).max(1.6));
    if recording {
        let h = size as f32 / 2.0;
        c.disc(RECORD_RED, h, h, 3.5 * scale);
    }
    c.into_rgba()
}

/// Menu icon: an SVG path on the 24-unit grid, stroked 1.75 units.
pub fn menu_icon_rgba(size: u32, path: &str, color: [u8; 3]) -> Vec<u8> {
    let mut c = Canvas::new(size);
    let (segs, scale) = place(&flatten_path(path), 0.0, 0.0, 24.0, size, 0.9);
    c.stroke(color, &segs, (1.75 * scale).max(1.2));
    c.into_rgba()
}

/// Solid stop square for the "Stop rekaman" menu item.
pub fn stop_icon_rgba(size: u32, color: [u8; 3]) -> Vec<u8> {
    let mut c = Canvas::new(size);
    let (segs, scale) = place(&flatten_path("M9 9h6v6H9z"), 0.0, 0.0, 24.0, size, 0.9);
    c.stroke(color, &segs, 3.0 * scale);
    c.into_rgba()
}

/// Straight RGBA -> BGRA. With `premultiply`, colour channels are multiplied
/// by alpha (what `MENUITEMINFOW.hbmpItem` bitmaps need).
pub fn rgba_to_bgra(px: &[u8], premultiply: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(px.len());
    for p in px.as_chunks::<4>().0 {
        let a = p[3] as u32;
        let m = |v: u8| if premultiply { ((v as u32 * a + 127) / 255) as u8 } else { v };
        out.extend_from_slice(&[m(p[2]), m(p[1]), m(p[0]), p[3]]);
    }
    out
}

// ------------------------------------------------------------------ .ico / .res

pub const ICON_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

fn le16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn le32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

/// DIB (BITMAPINFOHEADER + bottom-up BGRA + 1-bpp AND mask) for one icon image.
fn dib(size: u32, rgba: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    le32(&mut v, 40);
    le32(&mut v, size);
    le32(&mut v, size * 2);
    le16(&mut v, 1);
    le16(&mut v, 32);
    for _ in 0..6 {
        le32(&mut v, 0);
    }
    let bgra = rgba_to_bgra(rgba, false);
    let row = size as usize * 4;
    for r in (0..size as usize).rev() {
        v.extend_from_slice(&bgra[r * row..(r + 1) * row]);
    }
    let mask_row = (size as usize).div_ceil(32) * 4;
    v.resize(v.len() + mask_row * size as usize, 0);
    v
}

/// All sizes of the app icon as DIB images, ascending.
pub fn icon_images() -> Vec<(u32, Vec<u8>)> {
    ICON_SIZES.iter().map(|&s| (s, dib(s, &app_icon_rgba(s).0))).collect()
}

fn dim_byte(s: u32) -> u8 {
    if s >= 256 {
        0
    } else {
        s as u8
    }
}

/// A complete `.ico` file.
pub fn ico_file(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut v = Vec::new();
    le16(&mut v, 0);
    le16(&mut v, 1);
    le16(&mut v, images.len() as u16);
    let mut off = 6 + 16 * images.len() as u32;
    for (s, d) in images {
        let b = dim_byte(*s);
        v.extend_from_slice(&[b, b, 0, 0]);
        le16(&mut v, 1);
        le16(&mut v, 32);
        le32(&mut v, d.len() as u32);
        le32(&mut v, off);
        off += d.len() as u32;
    }
    for (_, d) in images {
        v.extend_from_slice(d);
    }
    v
}

fn res_entry(out: &mut Vec<u8>, ty: u16, name: u16, data: &[u8]) {
    le32(out, data.len() as u32);
    le32(out, 32);
    le16(out, 0xFFFF);
    le16(out, ty);
    le16(out, 0xFFFF);
    le16(out, name);
    le32(out, 0); // data version
    le16(out, 0x1030); // MOVEABLE | PURE | DISCARDABLE
    le16(out, 0); // language neutral
    le32(out, 0); // version
    le32(out, 0); // characteristics
    out.extend_from_slice(data);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// A 32-bit `.res` file with RT_ICON 1..=n and RT_GROUP_ICON id 1 (the exe icon).
pub fn res_file(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    res_entry(&mut out, 0, 0, &[]); // mandatory empty first entry
    let mut grp = Vec::new();
    le16(&mut grp, 0);
    le16(&mut grp, 1);
    le16(&mut grp, images.len() as u16);
    for (i, (s, d)) in images.iter().enumerate() {
        let id = i as u16 + 1;
        res_entry(&mut out, 3, id, d);
        let b = dim_byte(*s);
        grp.extend_from_slice(&[b, b, 0, 0]);
        le16(&mut grp, 1);
        le16(&mut grp, 32);
        le32(&mut grp, d.len() as u32);
        le16(&mut grp, id);
    }
    res_entry(&mut out, 14, 1, &grp);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(px: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    #[test]
    fn frame_path_matches_theme() {
        assert_eq!(FRAME_PATH, crate::Icon::Frame.path());
    }

    #[test]
    fn path_parser_handles_relative_arcs_and_closing() {
        let s = flatten_path("M4 9V5a1 1 0 0 1 1-1h4");
        assert_eq!(s[0], [4.0, 9.0, 4.0, 5.0]);
        let last = s.last().unwrap();
        assert!((last[2] - 9.0).abs() < 1e-4 && (last[3] - 4.0).abs() < 1e-4);
        assert!(s.iter().any(|g| (g[2] - 5.0).abs() < 1e-4 && (g[3] - 4.0).abs() < 1e-4));
        let sq = flatten_path("M4 6h16v12H4z");
        assert_eq!(sq.len(), 4);
        assert_eq!(*sq.last().unwrap(), [4.0, 18.0, 4.0, 6.0]);
    }

    #[test]
    fn full_circle_arc_stays_on_radius() {
        for g in circle_segs(12.0, 12.0, 3.0) {
            for (x, y) in [(g[0], g[1]), (g[2], g[3])] {
                assert!(((x - 12.0).hypot(y - 12.0) - 3.0).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn stroke_antialiases_edges() {
        let mut c = Canvas::new(8);
        c.stroke([255, 255, 255], &[[0.0, 4.0, 8.0, 4.0]], 2.0);
        let px = c.into_rgba();
        assert_eq!(at(&px, 8, 4, 3)[3], 255);
        assert_eq!(at(&px, 8, 4, 4)[3], 255);
        assert_eq!(at(&px, 8, 4, 0)[3], 0);
        let mut c = Canvas::new(8);
        c.stroke([255, 255, 255], &[[0.0, 4.0, 8.0, 4.0]], 1.0);
        let px = c.into_rgba();
        assert_eq!(at(&px, 8, 4, 3)[3], 128, "half-covered row");
    }

    #[test]
    fn app_icon_is_deterministic_rounded_and_accented() {
        for s in ICON_SIZES {
            let (a, w, h) = app_icon_rgba(s);
            assert_eq!((w, h, a.len()), (s, s, (s * s * 4) as usize));
            assert_eq!(a, app_icon_rgba(s).0);
            assert_eq!(at(&a, s, 0, 0)[3], 0, "rounded corner transparent at {s}");
            assert_eq!(at(&a, s, s / 2, 1)[3], 255, "top edge opaque at {s}");
        }
        let (a, ..) = app_icon_rgba(256);
        assert_eq!(at(&a, 256, 128, 8)[..3], [0x0E, 0x7C, 0x86], "accent background");
        // lens ring is white: it crosses (12+3, 12) units.
        let scale = 256.0 * 0.6 / 24.0;
        let x = (15.0 * scale + 256.0 * 0.2) as u32;
        assert_eq!(at(&a, 256, x, 128), [255, 255, 255, 255]);
        assert_eq!(at(&a, 256, 128, 128)[..3], [0x0E, 0x7C, 0x86]);
    }

    #[test]
    fn tray_icons_follow_mockup_colours() {
        for s in [16u32, 20, 24, 32, 40] {
            let idle = tray_icon_rgba(s, false, false);
            assert_eq!(at(&idle, s, s / 2, s / 2)[3], 0, "idle centre empty");
            assert!(idle.chunks(4).any(|p| p[3] == 255 && p[..3] == [0x4F, 0xC3, 0xCF]));
            let light = tray_icon_rgba(s, false, true);
            assert!(light.chunks(4).any(|p| p[3] == 255 && p[..3] == [0x1B, 0x1F, 0x24]));
            assert!(!light.chunks(4).any(|p| p[3] == 255 && p[..3] == [0x4F, 0xC3, 0xCF]));
            let rec = tray_icon_rgba(s, true, true);
            assert_eq!(at(&rec, s, s / 2, s / 2), [0xFF, 0x6B, 0x5E, 255], "rec dot at {s}");
        }
    }

    #[test]
    fn bgra_premultiply() {
        assert_eq!(rgba_to_bgra(&[200, 100, 50, 128], true), vec![25, 50, 100, 128]);
        assert_eq!(rgba_to_bgra(&[1, 2, 3, 4], false), vec![3, 2, 1, 4]);
    }

    #[test]
    fn ico_and_res_are_well_formed() {
        let imgs = icon_images();
        let ico = ico_file(&imgs);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 7, 0]);
        assert_eq!(ico[6 + 16 * 6], 0, "256 px stored as 0");
        let (s, d) = &imgs[0];
        assert_eq!(d.len(), 40 + (*s * *s * 4) as usize + (*s as usize * 4));
        let off = u32::from_le_bytes(ico[6 + 12..6 + 16].try_into().unwrap()) as usize;
        assert_eq!(&ico[off..off + d.len()], &d[..]);
        let res = res_file(&imgs);
        assert_eq!(&res[..8], &[0, 0, 0, 0, 32, 0, 0, 0]);
        assert_eq!(res.len() % 4, 0);
    }
}
