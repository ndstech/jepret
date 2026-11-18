//! Icons from the design, as 24×24 SVG path data, painted with egui shapes.
//! Owner of the renderer: **T15a**.
//!
//! The renderer must support the path commands used below: `M m L l H h V v
//! C c S s A a Z z` (absolute + relative), multiple subpaths per string, and
//! tessellate curves/arcs to polylines (enough segments to look smooth at
//! 16–48 px). Strokes use round caps/joins.

use std::collections::HashMap;
use std::sync::OnceLock;

use egui::epaint::{Mesh, PathShape, PathStroke};
use egui::{pos2, Color32, Painter, Pos2, Rect, Shape, Stroke};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    // ---- editor tools (order = toolbar order) ----
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
    Eyedropper,
    // ---- ui ----
    /// App glyph: four viewfinder corners.
    Frame,
    Undo,
    Redo,
    Copy,
    Save,
    OpenExternal,
    Folder,
    Search,
    Star,
    Trash,
    Calendar,
    Close,
    Check,
    Warning,
    Play,
    Gallery,
    Settings,
    Record,
    Volume,
    Mic,
    Keyboard,
    Monitor,
    Window,
    History,
    Scroll,
    Plus,
    Exit,
    Workflow,
}

impl Icon {
    /// Every icon, tools first.
    pub const ALL: &'static [Icon] = &[
        Icon::Select, Icon::Crop, Icon::Arrow, Icon::Line, Icon::Rect, Icon::Ellipse, Icon::Text,
        Icon::TextBox, Icon::Callout,
        Icon::Highlight, Icon::Blur, Icon::Pixelate, Icon::Step, Icon::Pen, Icon::Eyedropper,
        Icon::Frame, Icon::Undo, Icon::Redo, Icon::Copy, Icon::Save, Icon::OpenExternal,
        Icon::Folder, Icon::Search, Icon::Star, Icon::Trash, Icon::Calendar, Icon::Close,
        Icon::Check, Icon::Warning, Icon::Play, Icon::Gallery, Icon::Settings, Icon::Record,
        Icon::Volume, Icon::Mic, Icon::Keyboard, Icon::Monitor, Icon::Window, Icon::History,
        Icon::Scroll, Icon::Plus, Icon::Exit, Icon::Workflow,
    ];

    /// Editor tools in toolbar order, matching `tool.*` shortcut ids.
    pub const TOOLS: [(Icon, &'static str); 15] = [
        (Icon::Select, "tool.select"),
        (Icon::Crop, "tool.crop"),
        (Icon::Arrow, "tool.arrow"),
        (Icon::Line, "tool.line"),
        (Icon::Rect, "tool.rect"),
        (Icon::Ellipse, "tool.ellipse"),
        (Icon::Text, "tool.text"),
        (Icon::TextBox, "tool.textbox"),
        (Icon::Callout, "tool.callout"),
        (Icon::Highlight, "tool.highlight"),
        (Icon::Blur, "tool.blur"),
        (Icon::Pixelate, "tool.pixelate"),
        (Icon::Step, "tool.step"),
        (Icon::Pen, "tool.pen"),
        (Icon::Eyedropper, "tool.eyedropper"),
    ];

    /// SVG path data in a 24×24 box (from the design canvas).
    pub fn path(self) -> &'static str {
        match self {
            Icon::Select => "M5 3l13 8-6 1.5-3 6.5z",
            Icon::Crop => "M6 2v14a2 2 0 0 0 2 2h14 M2 6h14a2 2 0 0 1 2 2v14",
            Icon::Arrow => "M5 19L19 5 M10 5h9v9",
            Icon::Line => "M5 19L19 5",
            Icon::Rect => "M4 6h16v12H4z",
            Icon::Ellipse => "M12 5c4.4 0 8 3.1 8 7s-3.6 7-8 7-8-3.1-8-7 3.6-7 8-7z",
            Icon::Text => "M5 7V4h14v3 M12 4v16 M9 20h6",
            Icon::TextBox => "M3 5h18v14H3z M8 9h8 M12 9v6",
            Icon::Callout => "M4 4h16v11h-9l-4 4v-4H4z M8 8h8 M8 11h5",
            Icon::Highlight => "M15 4l5 5-8 8H7v-5z M4 20h16",
            Icon::Blur => "M12 9a3 3 0 1 1 0 6a3 3 0 1 1 0-6z M12 3.75a1.25 1.25 0 1 1 0 2.5a1.25 1.25 0 1 1 0-2.5z M19 10.75a1.25 1.25 0 1 1 0 2.5a1.25 1.25 0 1 1 0-2.5z M12 17.75a1.25 1.25 0 1 1 0 2.5a1.25 1.25 0 1 1 0-2.5z M5 10.75a1.25 1.25 0 1 1 0 2.5a1.25 1.25 0 1 1 0-2.5z M7 7h.01 M17 7h.01 M7 17h.01 M17 17h.01",
            Icon::Pixelate => "M5.125 5.125h1.75v1.75h-1.75z M13.125 5.125h1.75v1.75h-1.75z M9.125 9.125h1.75v1.75h-1.75z M17.125 9.125h1.75v1.75h-1.75z M5.125 13.125h1.75v1.75h-1.75z M13.125 13.125h1.75v1.75h-1.75z M9.125 17.125h1.75v1.75h-1.75z M17.125 17.125h1.75v1.75h-1.75z",
            Icon::Step => "M12 3a9 9 0 1 1 0 18a9 9 0 1 1 0-18z M10.5 9l2-1.5V16",
            Icon::Pen => "M4 20l4-1 11-11-3-3L5 16z M14 7l3 3",
            Icon::Eyedropper => "M15 3l6 6 M18 6l-9 9-4 1 1-4 9-9 M3 21l2-2",
            Icon::Frame => "M4 9V5a1 1 0 0 1 1-1h4 M15 4h4a1 1 0 0 1 1 1v4 M20 15v4a1 1 0 0 1-1 1h-4 M9 20H5a1 1 0 0 1-1-1v-4",
            Icon::Undo => "M9 14L4 9l5-5 M4 9h10a6 6 0 0 1 0 12h-3",
            Icon::Redo => "M15 14l5-5-5-5 M20 9H10a6 6 0 0 0 0 12h3",
            Icon::Copy => "M9 9h11v11H9z M5 15H4V4h11v1",
            Icon::Save => "M5 3h11l3 3v15H5z M8 3v5h7V3 M8 21v-7h8v7",
            Icon::OpenExternal => "M14 4h6v6 M20 4l-9 9 M18 14v6H4V6h6",
            Icon::Folder => "M3 6h6l2 2h10v11H3z",
            Icon::Search => "M11 4a7 7 0 1 1 0 14a7 7 0 1 1 0-14z M20 20l-4-4",
            Icon::Star => "M12 3l2.8 5.7 6.2.9-4.5 4.4 1 6.2L12 17.3 6.5 20.2l1-6.2L3 9.6l6.2-.9z",
            Icon::Trash => "M4 7h16 M9 7V4h6v3 M6 7l1 13h10l1-13",
            Icon::Calendar => "M4 6h16v14H4z M4 10h16 M8 3v4 M16 3v4",
            Icon::Close => "M6 6l12 12 M18 6L6 18",
            Icon::Check => "M5 12l5 5 9-10",
            Icon::Warning => "M12 3l9 16H3z M12 10v4 M12 17h.01",
            Icon::Play => "M8 5l11 7-11 7z",
            Icon::Gallery => "M4 4h7v7H4z M13 4h7v7h-7z M4 13h7v7H4z M13 13h7v7h-7z",
            Icon::Settings => "M12 9a3 3 0 1 1 0 6a3 3 0 1 1 0-6z M12 2v3 M12 19v3 M2 12h3 M19 12h3 M5 5l2 2 M17 17l2 2 M5 19l2-2 M17 7l2-2",
            Icon::Record => "M12 8a4 4 0 1 1 0 8a4 4 0 1 1 0-8z M3 12a9 9 0 1 0 18 0a9 9 0 1 0-18 0",
            Icon::Volume => "M11 5L6 9H3v6h3l5 4z M16 9a4 4 0 0 1 0 6",
            Icon::Mic => "M9 3h6v10H9z M5 11a7 7 0 0 0 14 0 M12 18v3",
            Icon::Keyboard => "M3 7h18v10H3z M7 11h.01 M11 11h.01 M15 11h.01 M8 14h8",
            Icon::Monitor => "M3 5h18v12H3z M8 21h8 M12 17v4",
            Icon::Window => "M4 5h16v14H4z M4 9h16",
            Icon::History => "M4 12a8 8 0 1 0 3-6.2 M4 4v4h4",
            Icon::Scroll => "M7 3h10v18H7z M12 8v8 M9 13l3 3 3-3",
            Icon::Plus => "M12 5v14 M5 12h14",
            Icon::Exit => "M15 4h4v16h-4 M10 8l-4 4 4 4 M6 12h10",
            Icon::Workflow => "M4 7h16 M4 12h10 M4 17h6 M17 14l3 3-3 3",
        }
    }
}

// ---------------------------------------------------------------------------
// Path parsing + tessellation (pure, unit-tested)
// ---------------------------------------------------------------------------

/// One flattened subpath in 24×24 icon space.
#[derive(Debug, Clone, PartialEq)]
pub struct Subpath {
    pub points: Vec<Pos2>,
    pub closed: bool,
}

/// Error from [`parse_path`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathError(pub String);

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "svg path: {}", self.0)
    }
}

impl std::error::Error for PathError {}

const CURVE_SEGMENTS: usize = 16;

struct Scanner<'a> {
    b: &'a [u8],
    i: usize,
}

impl Scanner<'_> {
    fn skip_sep(&mut self) {
        while self.i < self.b.len() && (self.b[self.i].is_ascii_whitespace() || self.b[self.i] == b',') {
            self.i += 1;
        }
    }

    fn peek_number(&mut self) -> bool {
        self.skip_sep();
        matches!(self.b.get(self.i), Some(c) if c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.'))
    }

    fn number(&mut self) -> Result<f32, PathError> {
        self.skip_sep();
        let start = self.i;
        let b = self.b;
        if matches!(b.get(self.i), Some(b'-' | b'+')) {
            self.i += 1;
        }
        let mut digits = 0;
        while matches!(b.get(self.i), Some(c) if c.is_ascii_digit()) {
            self.i += 1;
            digits += 1;
        }
        if b.get(self.i) == Some(&b'.') {
            self.i += 1;
            while matches!(b.get(self.i), Some(c) if c.is_ascii_digit()) {
                self.i += 1;
                digits += 1;
            }
        }
        if digits == 0 {
            return Err(PathError(format!("expected number at byte {start}")));
        }
        if matches!(b.get(self.i), Some(b'e' | b'E')) {
            let save = self.i;
            self.i += 1;
            if matches!(b.get(self.i), Some(b'-' | b'+')) {
                self.i += 1;
            }
            let es = self.i;
            while matches!(b.get(self.i), Some(c) if c.is_ascii_digit()) {
                self.i += 1;
            }
            if es == self.i {
                self.i = save;
            }
        }
        std::str::from_utf8(&b[start..self.i])
            .ok()
            .and_then(|t| t.parse::<f32>().ok())
            .ok_or_else(|| PathError(format!("bad number at byte {start}")))
    }

    /// Arc flag: a single `0` or `1`, possibly not separated from what follows.
    fn flag(&mut self) -> Result<bool, PathError> {
        self.skip_sep();
        match self.b.get(self.i) {
            Some(b'0') => {
                self.i += 1;
                Ok(false)
            }
            Some(b'1') => {
                self.i += 1;
                Ok(true)
            }
            _ => Err(PathError(format!("expected arc flag at byte {}", self.i))),
        }
    }

    fn pair(&mut self) -> Result<egui::Vec2, PathError> {
        Ok(egui::vec2(self.number()?, self.number()?))
    }
}

/// Parse SVG path data (`M m L l H h V v C c S s A a Z z`) into polylines.
pub fn parse_path(d: &str) -> Result<Vec<Subpath>, PathError> {
    let mut sc = Scanner { b: d.as_bytes(), i: 0 };
    let mut out: Vec<Subpath> = Vec::new();
    let mut cur = Pos2::ZERO;
    let mut start = Pos2::ZERO;
    let mut last_ctrl: Option<Pos2> = None; // for S reflection
    let mut have_sub = false;
    let mut cmd = b' ';

    loop {
        sc.skip_sep();
        let Some(&c) = sc.b.get(sc.i) else { break };
        if c.is_ascii_alphabetic() {
            cmd = c;
            sc.i += 1;
            if matches!(cmd, b'Z' | b'z') {
                if let Some(sp) = out.last_mut() {
                    sp.closed = true;
                }
                cur = start;
                have_sub = false;
                last_ctrl = None;
                continue;
            }
        } else if !sc.peek_number() || matches!(cmd, b' ' | b'Z' | b'z') {
            return Err(PathError(format!("unexpected byte {c} at {}", sc.i)));
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { cur } else { Pos2::ZERO };
        let upper = cmd.to_ascii_uppercase();
        if !matches!(upper, b'M' | b'L' | b'H' | b'V' | b'C' | b'S' | b'A') {
            return Err(PathError(format!("unsupported command '{}'", cmd as char)));
        }
        if upper == b'M' {
            let p = base + sc.pair()?;
            cur = p;
            start = p;
            out.push(Subpath { points: vec![p], closed: false });
            have_sub = true;
            last_ctrl = None;
            // Subsequent pairs are implicit lineto.
            cmd = if rel { b'l' } else { b'L' };
            continue;
        }
        // Drawing command after Z without a new M continues from the start.
        if !have_sub {
            out.push(Subpath { points: vec![cur], closed: false });
            have_sub = true;
        }
        let Some(sub) = out.last_mut() else { continue };
        let pts = &mut sub.points;
        match upper {
            b'L' => {
                let p = base + sc.pair()?;
                pts.push(p);
                cur = p;
                last_ctrl = None;
            }
            b'H' => {
                let x = sc.number()?;
                cur = pos2(if rel { cur.x + x } else { x }, cur.y);
                pts.push(cur);
                last_ctrl = None;
            }
            b'V' => {
                let y = sc.number()?;
                cur = pos2(cur.x, if rel { cur.y + y } else { y });
                pts.push(cur);
                last_ctrl = None;
            }
            b'C' | b'S' => {
                let c1 = if upper == b'S' {
                    last_ctrl.map_or(cur, |lc| pos2(2.0 * cur.x - lc.x, 2.0 * cur.y - lc.y))
                } else {
                    base + sc.pair()?
                };
                let c2 = base + sc.pair()?;
                let p = base + sc.pair()?;
                for k in 1..=CURVE_SEGMENTS {
                    let t = k as f32 / CURVE_SEGMENTS as f32;
                    let u = 1.0 - t;
                    let q = cur.to_vec2() * (u * u * u)
                        + c1.to_vec2() * (3.0 * u * u * t)
                        + c2.to_vec2() * (3.0 * u * t * t)
                        + p.to_vec2() * (t * t * t);
                    pts.push(q.to_pos2());
                }
                cur = p;
                last_ctrl = Some(c2);
            }
            _ => {
                let r = sc.pair()?;
                let rot = sc.number()?;
                let large = sc.flag()?;
                let sweep = sc.flag()?;
                let p = base + sc.pair()?;
                arc_points(cur, r.x, r.y, rot, large, sweep, p, pts);
                cur = p;
                last_ctrl = None;
            }
        }
    }
    Ok(out)
}

/// SVG arc (endpoint parameterization, spec F.6.5) flattened to a polyline;
/// appends points after `p0` up to and including `p1`.
#[allow(clippy::too_many_arguments)]
fn arc_points(p0: Pos2, rx: f32, ry: f32, rot_deg: f32, large: bool, sweep: bool, p1: Pos2, out: &mut Vec<Pos2>) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if p0 == p1 {
        return;
    }
    if rx < 1e-6 || ry < 1e-6 {
        out.push(p1);
        return;
    }
    let phi = rot_deg.to_radians();
    let (sp, cp) = phi.sin_cos();
    let dx2 = (p0.x - p1.x) / 2.0;
    let dy2 = (p0.y - p1.y) / 2.0;
    let x1 = cp * dx2 + sp * dy2;
    let y1 = -sp * dx2 + cp * dy2;
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coef = if den == 0.0 { 0.0 } else { (num / den).max(0.0).sqrt() };
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1 / ry;
    let cyp = -coef * ry * x1 / rx;
    let cx = cp * cxp - sp * cyp + (p0.x + p1.x) / 2.0;
    let cy = sp * cxp + cp * cyp + (p0.y + p1.y) / 2.0;
    let ang = |ux: f32, uy: f32, vx: f32, vy: f32| (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    let theta1 = ang(1.0, 0.0, (x1 - cxp) / rx, (y1 - cyp) / ry);
    let mut dtheta = ang((x1 - cxp) / rx, (y1 - cyp) / ry, (-x1 - cxp) / rx, (-y1 - cyp) / ry);
    let tau = std::f32::consts::TAU;
    if !sweep && dtheta > 0.0 {
        dtheta -= tau;
    } else if sweep && dtheta < 0.0 {
        dtheta += tau;
    }
    let n = ((dtheta.abs() / (std::f32::consts::PI / 24.0)).ceil() as usize).max(2);
    for k in 1..n {
        let t = theta1 + dtheta * k as f32 / n as f32;
        let (st, ct) = t.sin_cos();
        out.push(pos2(cx + rx * ct * cp - ry * st * sp, cy + rx * ct * sp + ry * st * cp));
    }
    out.push(p1);
}

/// Parsed icons, built once.
fn parsed(icon: Icon) -> &'static [Subpath] {
    static CACHE: OnceLock<HashMap<Icon, Vec<Subpath>>> = OnceLock::new();
    let map = CACHE.get_or_init(|| {
        Icon::ALL
            .iter()
            .map(|&i| {
                let v = parse_path(i.path()).unwrap_or_else(|e| {
                    tracing::warn!("icon {i:?}: {e}");
                    Vec::new()
                });
                (i, v)
            })
            .collect()
    });
    map.get(&icon).map_or(&[], Vec::as_slice)
}

/// Remove consecutive duplicates (and a closing point equal to the first).
fn dedup(points: &[Pos2], closed: bool) -> Vec<Pos2> {
    let mut v: Vec<Pos2> = Vec::with_capacity(points.len());
    for &p in points {
        if v.last().is_none_or(|l| l.distance(p) > 1e-4) {
            v.push(p);
        }
    }
    if closed && v.len() > 1 && v[0].distance(v[v.len() - 1]) <= 1e-4 {
        v.pop();
    }
    v
}

/// Ear-clipping triangulation of a simple polygon; returns index triples.
pub fn triangulate(poly: &[Pos2]) -> Vec<[usize; 3]> {
    let n = poly.len();
    if n < 3 {
        return Vec::new();
    }
    let area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    let mut idx: Vec<usize> = if area >= 0.0 { (0..n).collect() } else { (0..n).rev().collect() };
    let cross = |a: Pos2, b: Pos2, c: Pos2| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let inside = |p: Pos2, a: Pos2, b: Pos2, c: Pos2| {
        cross(a, b, p) >= -1e-6 && cross(b, c, p) >= -1e-6 && cross(c, a, p) >= -1e-6
    };
    let mut tris = Vec::with_capacity(n - 2);
    let mut guard = 0;
    while idx.len() > 3 && guard < n * n {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for k in 0..m {
            let (ia, ib, ic) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            let (a, b, c) = (poly[ia], poly[ib], poly[ic]);
            if cross(a, b, c) <= 1e-9 {
                continue;
            }
            if idx.iter().any(|&j| j != ia && j != ib && j != ic && inside(poly[j], a, b, c)) {
                continue;
            }
            tris.push([ia, ib, ic]);
            idx.remove(k);
            clipped = true;
            break;
        }
        if !clipped {
            // Degenerate input: drop a vertex to guarantee progress.
            idx.remove(0);
        }
    }
    if idx.len() == 3 {
        tris.push([idx[0], idx[1], idx[2]]);
    }
    tris
}

struct Xf {
    origin: Pos2,
    scale: f32,
}

impl Xf {
    fn new(rect: Rect) -> Self {
        let scale = rect.width().min(rect.height()) / 24.0;
        Xf { origin: rect.center() - egui::vec2(12.0, 12.0) * scale, scale }
    }
    fn apply(&self, p: Pos2) -> Pos2 {
        self.origin + p.to_vec2() * self.scale
    }
}

/// Stroke the icon into `rect` (scaled from 24×24, centred, aspect kept).
/// `width` is the stroke width in 24-unit space (design: 1.75 for 20–24 px
/// icons, 2.0–2.6 for 12–16 px). Caps and joins are round.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32, width: f32) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let xf = Xf::new(rect);
    let px = (width * xf.scale).max(0.5);
    let stroke = PathStroke::new(px, color);
    let r = px / 2.0;
    // Round caps/joins are circles on top of the polyline; skipped for
    // translucent colours where the overlap would show.
    let rounds = color.a() == 255 && r > 0.6;
    for sp in parsed(icon) {
        let pts = dedup(&sp.points, sp.closed);
        let screen: Vec<Pos2> = pts.iter().map(|&p| xf.apply(p)).collect();
        match screen.len() {
            0 => {}
            1 => {
                painter.circle_filled(screen[0], r.max(0.5), color);
            }
            n => {
                let closed = sp.closed && n > 2;
                painter.add(PathShape { points: screen.clone(), closed, fill: Color32::TRANSPARENT, stroke: stroke.clone() });
                if rounds {
                    for i in 0..n {
                        let end = !closed && (i == 0 || i == n - 1);
                        let corner = end || {
                            let a = screen[(i + n - 1) % n];
                            let b = screen[i];
                            let c = screen[(i + 1) % n];
                            (b - a).normalized().dot((c - b).normalized()) < 0.985
                        };
                        if corner {
                            painter.circle_filled(screen[i], r, color);
                        }
                    }
                }
            }
        }
    }
}

/// Fill the icon's closed subpaths (e.g. a filled star or play triangle).
pub fn paint_filled(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let xf = Xf::new(rect);
    for sp in parsed(icon) {
        if !sp.closed {
            continue;
        }
        let pts: Vec<Pos2> = dedup(&sp.points, true).iter().map(|&p| xf.apply(p)).collect();
        if pts.len() < 3 {
            continue;
        }
        let mut mesh = Mesh::default();
        for &p in &pts {
            mesh.colored_vertex(p, color);
        }
        for [a, b, c] in triangulate(&pts) {
            mesh.add_triangle(a as u32, b as u32, c as u32);
        }
        painter.add(Shape::mesh(mesh));
        // Hairline outline gives the fill anti-aliased edges.
        painter.add(PathShape::closed_line(pts, Stroke::new(1.0_f32, color)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_icons_parse_and_stay_in_box() {
        assert_eq!(Icon::ALL.len(), 43); // 15 tools + 28 ui icons
        for &i in Icon::ALL {
            let sp = parse_path(i.path()).unwrap_or_else(|e| panic!("{i:?}: {e}"));
            assert!(!sp.is_empty(), "{i:?}");
            for s in &sp {
                assert!(!s.points.is_empty());
                for p in &s.points {
                    assert!(p.x >= -0.5 && p.x <= 24.5 && p.y >= -0.5 && p.y <= 24.5, "{i:?}: {p:?}");
                }
            }
        }
    }

    #[test]
    fn all_icons_listed_once() {
        let set: std::collections::HashSet<_> = Icon::ALL.iter().collect();
        assert_eq!(set.len(), Icon::ALL.len());
        for (i, _) in Icon::TOOLS {
            assert!(set.contains(&i));
        }
    }

    #[test]
    fn full_circle_arc_has_constant_radius() {
        let sp = parse_path(Icon::Step.path()).unwrap();
        let circle = &sp[0];
        assert!(circle.closed);
        assert!(circle.points.len() > 40);
        for p in &circle.points {
            let d = p.distance(pos2(12.0, 12.0));
            assert!((d - 9.0).abs() < 0.05, "{d}");
        }
    }

    #[test]
    fn relative_equals_absolute() {
        let a = parse_path("M5 3L18 11L12 12.5L9 19z").unwrap();
        let b = parse_path("M5 3l13 8-6 1.5-3 6.5z").unwrap();
        assert_eq!(a, b);
        let c = parse_path("M4 4H10V10H4z").unwrap();
        let d = parse_path("m4 4h6v6h-6z").unwrap();
        assert_eq!(c, d);
        let e = parse_path("M4 12C4 8 8 4 12 4S20 8 20 12").unwrap();
        let f = parse_path("M4 12c0-4 4-8 8-8s8 4 8 8").unwrap();
        for (p, q) in e[0].points.iter().zip(&f[0].points) {
            assert!(p.distance(*q) < 1e-4);
        }
        let g = parse_path("M3 12A9 9 0 0 1 21 12").unwrap();
        let h = parse_path("M3 12a9 9 0 0 1 18 0").unwrap();
        assert_eq!(g, h);
    }

    #[test]
    fn compact_numbers() {
        let sp = parse_path("M1.5-3.25l.9.1-2-.5").unwrap();
        let p = &sp[0].points;
        assert_eq!(p[0], pos2(1.5, -3.25));
        assert!((p[1].x - 2.4).abs() < 1e-5 && (p[1].y + 3.15).abs() < 1e-5);
        assert!((p[2].x - 0.4).abs() < 1e-5 && (p[2].y + 3.65).abs() < 1e-5);
    }

    #[test]
    fn compact_arc_flags() {
        let a = parse_path("M3 12a9 9 0 0118 0").unwrap();
        let b = parse_path("M3 12a9 9 0 0 1 18 0").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn multiple_subpaths_and_close() {
        let sp = parse_path(Icon::Gallery.path()).unwrap();
        assert_eq!(sp.len(), 4);
        assert!(sp.iter().all(|s| s.closed));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_path("M1").is_err());
        assert!(parse_path("X1 2").is_err());
        assert!(parse_path("1 2").is_err());
    }

    #[test]
    fn triangulation_covers_area() {
        // Concave "L" shape, area 3.
        let l = [pos2(0.0, 0.0), pos2(2.0, 0.0), pos2(2.0, 1.0), pos2(1.0, 1.0), pos2(1.0, 2.0), pos2(0.0, 2.0)];
        let tris = triangulate(&l);
        assert_eq!(tris.len(), 4);
        let area: f32 = tris
            .iter()
            .map(|[a, b, c]| ((l[*b] - l[*a]).x * (l[*c] - l[*a]).y - (l[*b] - l[*a]).y * (l[*c] - l[*a]).x).abs() / 2.0)
            .sum();
        assert!((area - 3.0).abs() < 1e-5);
        let sp = parse_path(Icon::Star.path()).unwrap();
        let pts = dedup(&sp[0].points, true);
        assert_eq!(triangulate(&pts).len(), pts.len() - 2);
    }
}
