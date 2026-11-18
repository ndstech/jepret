//! Editor session state + shortcut-id dispatch. Pure (no egui): the UI maps
//! key events to shortcut ids ([`crate::input`]) and calls
//! [`Session::dispatch`]; anything that needs the window/clipboard/files is
//! returned as an [`Effect`].

use std::collections::HashMap;

use jepret_core::config::EditorConfig;

use crate::doc::{Document, Pt, Rgba, Shape, ShapeKind, Tool};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub color: Rgba,
    pub width: f32,
    pub fill: bool,
    pub shadow: bool,
    pub outline: bool,
}

pub const MIN_WIDTH: f32 = 1.0;
pub const MAX_WIDTH: f32 = 24.0;
/// Alpha used when a fully opaque colour is applied to a highlighter.
pub const HIGHLIGHT_ALPHA: u8 = 110;
pub const DUPLICATE_OFFSET: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomCmd {
    In,
    Out,
    Fit,
    Actual,
}

/// Things the UI has to perform (window / clipboard / disk).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    None,
    Copy,
    CopyAndClose,
    Save,
    SaveAs,
    SaveAndClose,
    CopyPath,
    OpenWith,
    OpenFolder,
    Close,
    Cheatsheet,
    Zoom(ZoomCmd),
}

pub struct Session {
    pub doc: Document,
    pub tool: Tool,
    prev_tool: Tool,
    pub styles: HashMap<Tool, Style>,
    sel: Vec<usize>,
    pub palette: Vec<Rgba>,
}

/// Parse `#rrggbb` / `#rrggbbaa` (leading `#` optional).
pub fn parse_hex_color(s: &str) -> Option<Rgba> {
    let h = s.trim().trim_start_matches('#');
    if !(h.len() == 6 || h.len() == 8) || !h.is_ascii() {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([b(0)?, b(2)?, b(4)?, if h.len() == 8 { b(6)? } else { 255 }])
}

fn default_style(t: Tool) -> Style {
    Style {
        color: t.default_color(),
        width: t.default_width(),
        // Boxes/bubbles read best solid; free text gets a halo by default so
        // it stays readable on any background.
        fill: matches!(t, Tool::TextBox | Tool::Callout),
        shadow: false,
        outline: t == Tool::Text,
    }
}

/// Opaque colours become translucent on highlighters.
fn fit_alpha(highlighter: bool, c: Rgba) -> Rgba {
    if highlighter && c[3] == 255 {
        [c[0], c[1], c[2], HIGHLIGHT_ALPHA]
    } else {
        c
    }
}

impl Session {
    pub fn new(cfg: &EditorConfig) -> Self {
        let mut palette: Vec<Rgba> = cfg.palette.iter().filter_map(|s| parse_hex_color(s)).collect();
        if palette.is_empty() {
            palette = jepret_core::config::EditorConfig::default()
                .palette
                .iter()
                .filter_map(|s| parse_hex_color(s))
                .collect();
        }
        let tool = Tool::from_id(&cfg.default_tool).unwrap_or(Tool::Arrow);
        Self {
            doc: Document::new(),
            tool,
            prev_tool: tool,
            styles: Tool::ALL.iter().map(|t| (*t, default_style(*t))).collect(),
            sel: Vec::new(),
            palette,
        }
    }

    pub fn selection(&self) -> &[usize] {
        &self.sel
    }

    pub fn set_selection(&mut self, sel: Vec<usize>) {
        self.sel = sel;
        self.sel.retain(|i| *i < self.doc.shapes().len());
    }

    pub fn select_one(&mut self, i: Option<usize>) {
        self.sel = i.into_iter().collect();
    }

    pub fn select_all(&mut self) {
        self.sel = (0..self.doc.shapes().len()).collect();
    }

    pub fn clear_selection(&mut self) {
        self.sel.clear();
    }

    pub fn is_selected(&self, i: usize) -> bool {
        self.sel.contains(&i)
    }

    /// Tool whose style the style commands edit (eyedropper edits the tool
    /// it was entered from).
    pub fn style_tool(&self) -> Tool {
        if self.tool == Tool::Eyedropper {
            self.prev_tool
        } else {
            self.tool
        }
    }

    pub fn style(&self) -> Style {
        self.styles.get(&self.style_tool()).copied().unwrap_or_else(|| default_style(Tool::Arrow))
    }

    pub fn style_of(&self, t: Tool) -> Style {
        self.styles.get(&t).copied().unwrap_or_else(|| default_style(t))
    }

    pub fn set_tool(&mut self, t: Tool) {
        if t == Tool::Eyedropper && self.tool != Tool::Eyedropper {
            self.prev_tool = self.tool;
        }
        self.tool = t;
    }

    /// Eyedropper result: set colour, return to the previous tool.
    pub fn pick_color(&mut self, c: Rgba) {
        self.apply_color(c);
        if self.tool == Tool::Eyedropper {
            self.tool = self.prev_tool;
        }
    }

    /// Set colour on the current tool style and the selection (one undo step).
    pub fn apply_color(&mut self, c: Rgba) {
        let t = self.style_tool();
        let st = self.styles.entry(t).or_insert_with(|| default_style(t));
        st.color = fit_alpha(t == Tool::Highlight, c);
        let sel = self.sel.clone();
        self.doc.edit_shapes(&sel, |s| {
            s.color = fit_alpha(matches!(s.kind, ShapeKind::Highlight { .. }), c);
        });
    }

    pub fn apply_width(&mut self, w: f32) {
        let w = w.clamp(MIN_WIDTH, MAX_WIDTH);
        let t = self.style_tool();
        self.styles.entry(t).or_insert_with(|| default_style(t)).width = w;
        let sel = self.sel.clone();
        self.doc.edit_shapes(&sel, |s| s.width = w);
    }

    pub fn apply_fill(&mut self, on: bool) {
        let t = self.style_tool();
        self.styles.entry(t).or_insert_with(|| default_style(t)).fill = on;
        let sel = self.sel.clone();
        self.doc.edit_shapes(&sel, |s| s.fill = on);
    }

    pub fn apply_shadow(&mut self, on: bool) {
        let t = self.style_tool();
        self.styles.entry(t).or_insert_with(|| default_style(t)).shadow = on;
        let sel = self.sel.clone();
        self.doc.edit_shapes(&sel, |s| s.shadow = on);
    }

    pub fn apply_outline(&mut self, on: bool) {
        let t = self.style_tool();
        self.styles.entry(t).or_insert_with(|| default_style(t)).outline = on;
        let sel = self.sel.clone();
        self.doc.edit_shapes(&sel, |s| s.outline = on);
    }

    pub fn outline_now(&self) -> bool {
        self.sel
            .first()
            .and_then(|i| self.doc.shapes().get(*i))
            .map(|s| s.outline)
            .unwrap_or_else(|| self.style().outline)
    }

    /// Current value shown for fill/shadow: first selected shape if any.
    pub fn fill_now(&self) -> bool {
        self.sel
            .first()
            .and_then(|i| self.doc.shapes().get(*i))
            .map(|s| s.fill)
            .unwrap_or_else(|| self.style().fill)
    }

    pub fn shadow_now(&self) -> bool {
        self.sel
            .first()
            .and_then(|i| self.doc.shapes().get(*i))
            .map(|s| s.shadow)
            .unwrap_or_else(|| self.style().shadow)
    }

    /// Build a fresh shape of `kind` using the current tool style.
    pub fn new_shape(&self, kind: ShapeKind) -> Shape {
        let st = self.style();
        let mut s = Shape::new(kind, st.color, st.width);
        s.fill = st.fill;
        s.shadow = st.shadow;
        s.outline = st.outline;
        s
    }

    /// Add a shape and select it.
    pub fn add_and_select(&mut self, shape: Shape) -> usize {
        let i = self.doc.add_shape(shape);
        self.sel = vec![i];
        i
    }

    pub fn undo(&mut self) {
        self.doc.undo();
        self.sel.clear();
    }

    pub fn redo(&mut self) {
        self.doc.redo();
        self.sel.clear();
    }

    /// Jump to history state `idx` (see [`Document::jump_to`]); the selection
    /// is cleared like on undo/redo.
    pub fn jump_history(&mut self, idx: usize) {
        if self.doc.jump_to(idx) {
            self.sel.clear();
        }
    }

    /// Move the object at doc index `from` to `to` (one undo step), keeping
    /// the selection on the same objects.
    pub fn move_object(&mut self, from: usize, to: usize) {
        if self.doc.move_shape(from, to) {
            for i in &mut self.sel {
                *i = index_after_move(*i, from, to);
            }
        }
    }

    pub fn delete_selection(&mut self) {
        let sel = std::mem::take(&mut self.sel);
        self.doc.remove_shapes(&sel);
    }

    /// Run a shortcut by catalogue id. `big` = Shift was held (nudge 10 px).
    /// Unknown ids are ignored.
    pub fn dispatch(&mut self, id: &str, big: bool) -> Effect {
        if let Some(t) = Tool::from_id(id) {
            self.set_tool(t);
            return Effect::None;
        }
        if let Some(n) = id.strip_prefix("style.color_").and_then(|n| n.parse::<usize>().ok()) {
            if let Some(c) = self.palette.get(n.wrapping_sub(1)).copied() {
                self.apply_color(c);
            }
            return Effect::None;
        }
        match id {
            "style.width_up" => self.apply_width(self.width_now() + 1.0),
            "style.width_down" => self.apply_width(self.width_now() - 1.0),
            "style.toggle_fill" => self.apply_fill(!self.fill_now()),
            "style.toggle_shadow" => self.apply_shadow(!self.shadow_now()),
            "style.toggle_outline" => self.apply_outline(!self.outline_now()),
            "edit.undo" => self.undo(),
            "edit.redo" => self.redo(),
            "edit.delete" => self.delete_selection(),
            "edit.duplicate" => {
                let sel = self.sel.clone();
                let new = self.doc.duplicate_shapes(&sel, DUPLICATE_OFFSET);
                if !new.is_empty() {
                    self.sel = new;
                    // Only the Select tool moves objects; stay there so the
                    // copy can be dragged straight away.
                    self.tool = Tool::Select;
                }
            }
            "edit.select_all" => {
                self.tool = Tool::Select;
                self.select_all();
            }
            "edit.bring_front" => {
                let sel = self.sel.clone();
                self.sel = self.doc.bring_to_front(&sel);
            }
            "edit.send_back" => {
                let sel = self.sel.clone();
                self.sel = self.doc.send_to_back(&sel);
            }
            "edit.nudge_left" => self.nudge(-1.0, 0.0, big),
            "edit.nudge_right" => self.nudge(1.0, 0.0, big),
            "edit.nudge_up" => self.nudge(0.0, -1.0, big),
            "edit.nudge_down" => self.nudge(0.0, 1.0, big),
            "edit.reset_steps" => {
                self.doc.reset_steps();
            }
            "view.zoom_in" => return Effect::Zoom(ZoomCmd::In),
            "view.zoom_out" => return Effect::Zoom(ZoomCmd::Out),
            "view.zoom_fit" => return Effect::Zoom(ZoomCmd::Fit),
            "view.zoom_100" => return Effect::Zoom(ZoomCmd::Actual),
            "view.cheatsheet" => return Effect::Cheatsheet,
            "file.copy" => return Effect::Copy,
            "file.copy_close" => return Effect::CopyAndClose,
            "file.save" => return Effect::Save,
            "file.save_as" => return Effect::SaveAs,
            "file.save_close" => return Effect::SaveAndClose,
            "file.copy_path" => return Effect::CopyPath,
            "file.open_with" => return Effect::OpenWith,
            "file.open_folder" => return Effect::OpenFolder,
            "file.close" => {
                // tiered Esc: deselect first, then close
                if !self.sel.is_empty() {
                    self.sel.clear();
                } else {
                    return Effect::Close;
                }
            }
            _ => {}
        }
        Effect::None
    }

    /// Colour shown in the UI: first selected shape, else the tool style.
    pub fn color_now(&self) -> Rgba {
        self.sel
            .first()
            .and_then(|i| self.doc.shapes().get(*i))
            .map(|s| s.color)
            .unwrap_or_else(|| self.style().color)
    }

    /// Live (no undo step of its own) colour/width edit for slider/picker
    /// scrubbing. The caller calls `doc.begin_edit()` once per gesture when
    /// the selection is non-empty.
    pub fn live_style(&mut self, color: Option<Rgba>, width: Option<f32>) {
        let t = self.style_tool();
        let st = self.styles.entry(t).or_insert_with(|| default_style(t));
        if let Some(c) = color {
            st.color = fit_alpha(t == Tool::Highlight, c);
        }
        if let Some(w) = width {
            st.width = w.clamp(MIN_WIDTH, MAX_WIDTH);
        }
        let sel = self.sel.clone();
        for i in sel {
            if let Some(sh) = self.doc.shape_live(i) {
                if let Some(c) = color {
                    sh.color = fit_alpha(matches!(sh.kind, ShapeKind::Highlight { .. }), c);
                }
                if let Some(w) = width {
                    sh.width = w.clamp(MIN_WIDTH, MAX_WIDTH);
                }
            }
        }
        if !self.sel.is_empty() {
            self.doc.touch();
        }
    }

    pub fn width_now(&self) -> f32 {
        self.sel
            .first()
            .and_then(|i| self.doc.shapes().get(*i))
            .map(|s| s.width)
            .unwrap_or_else(|| self.style().width)
    }

    fn nudge(&mut self, dx: f32, dy: f32, big: bool) {
        let k = if big { 10.0 } else { 1.0 };
        let sel = self.sel.clone();
        self.doc.nudge_shapes(&sel, dx * k, dy * k);
    }
}

/// How a drag is shaped by Shift/Alt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    /// Line / arrow: Shift snaps to 45 degree steps.
    Segment,
    /// Rect / ellipse / highlight / blur / pixelate / crop: Shift = square.
    Boxed,
}

/// Endpoints `(a, b)` for a drag from `start` to `p`.
/// Shift = constrain, Alt = `start` is the centre.
pub fn constrain_drag(kind: DragKind, start: Pt, p: Pt, shift: bool, alt: bool) -> (Pt, Pt) {
    let (mut dx, mut dy) = (p.x - start.x, p.y - start.y);
    if shift {
        match kind {
            DragKind::Segment => {
                let len = dx.hypot(dy);
                if len > 0.0 {
                    let step = std::f32::consts::FRAC_PI_4;
                    let ang = (dy.atan2(dx) / step).round() * step;
                    dx = len * ang.cos();
                    dy = len * ang.sin();
                }
            }
            DragKind::Boxed => {
                let d = dx.abs().max(dy.abs());
                dx = if dx < 0.0 { -d } else { d };
                dy = if dy < 0.0 { -d } else { d };
            }
        }
    }
    let b = Pt::new(start.x + dx, start.y + dy);
    if alt {
        (Pt::new(start.x - dx, start.y - dy), b)
    } else {
        (start, b)
    }
}

/// Snap `p` so the segment from `from` is a multiple of 45 degrees.
pub fn snap_45(from: Pt, p: Pt) -> Pt {
    constrain_drag(DragKind::Segment, from, p, true, false).1
}

/// Is `p` within `radius` of the first vertex of a draft with >= 3 points
/// (i.e. clicking it closes the polygon)?
pub fn near_first_vertex(draft: &[Pt], p: Pt, radius: f32) -> bool {
    draft.len() >= 3 && draft[0].dist(p) <= radius
}

/// Finish a polygon draft: drop consecutive duplicate points (a double-click
/// adds the same vertex twice). `None` when fewer than 3 distinct points remain.
pub fn finalize_polygon(draft: &[Pt]) -> Option<Vec<Pt>> {
    let mut out: Vec<Pt> = Vec::with_capacity(draft.len());
    for &p in draft {
        if out.last().is_none_or(|q| q.dist(p) > 0.5) {
            out.push(p);
        }
    }
    if out.len() > 1 && out[0].dist(out[out.len() - 1]) <= 0.5 {
        out.pop();
    }
    (out.len() >= 3).then_some(out)
}

/// Final document index when the shape at doc index `from` is dropped at
/// list `slot` (0..=n). The list shows the front-most shape first, so slot
/// `k` means "before display row k" (display row d = doc index n-1-d).
pub fn drop_index(from: usize, slot: usize, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let fd = (n - 1).saturating_sub(from);
    let slot = slot.min(n);
    let p = if slot > fd { slot - 1 } else { slot };
    n - 1 - p.min(n - 1)
}

/// New index of the shape at `i` after the shape at `from` moved to `to`.
pub fn index_after_move(i: usize, from: usize, to: usize) -> usize {
    if i == from {
        to
    } else if from < to && i > from && i <= to {
        i - 1
    } else if from > to && i >= to && i < from {
        i + 1
    } else {
        i
    }
}

/// Selection after clicking list row (doc index) `i`: plain = only it,
/// Ctrl = toggle, Shift = range from the last selected (or just `i`).
pub fn click_select(cur: &[usize], i: usize, ctrl: bool, shift: bool) -> Vec<usize> {
    if shift {
        let anchor = cur.last().copied().unwrap_or(i);
        return (anchor.min(i)..=anchor.max(i)).collect();
    }
    if ctrl {
        let mut v: Vec<usize> = cur.to_vec();
        if let Some(k) = v.iter().position(|x| *x == i) {
            v.remove(k);
        } else {
            v.push(i);
        }
        return v;
    }
    vec![i]
}

#[cfg(test)]
mod tests {
    use super::*;
    use jepret_core::shortcuts::EDITOR_SHORTCUTS;

    #[test]
    fn polygon_helpers() {
        let o = Pt::new(0.0, 0.0);
        let s = snap_45(o, Pt::new(10.0, 1.0));
        assert!(s.y.abs() < 1e-3 && s.x > 9.0);
        let d = vec![o, Pt::new(10.0, 0.0), Pt::new(10.0, 10.0)];
        assert!(near_first_vertex(&d, Pt::new(3.0, 3.0), 5.0));
        assert!(!near_first_vertex(&d, Pt::new(30.0, 3.0), 5.0));
        assert!(!near_first_vertex(&d[..2], o, 5.0));
        let mut dup = d.clone();
        dup.push(Pt::new(10.0, 10.0));
        assert_eq!(finalize_polygon(&dup).unwrap().len(), 3);
        assert!(finalize_polygon(&d[..2]).is_none());
        assert!(finalize_polygon(&[o, o, o]).is_none());
    }

    fn sess() -> Session {
        Session::new(&EditorConfig::default())
    }

    fn rect(x: f32) -> Shape {
        Shape::new(ShapeKind::Rect { a: Pt::new(x, 0.0), b: Pt::new(x + 10.0, 10.0) }, [1, 2, 3, 255], 3.0)
    }

    #[test]
    fn every_catalogue_id_is_handled() {
        // No id may fall through silently: each one either changes state or
        // yields an effect on a populated session.
        for def in EDITOR_SHORTCUTS {
            let mut s = sess();
            s.set_tool(Tool::Rect);
            s.doc.add_shape(rect(0.0));
            s.doc.add_shape(rect(20.0));
            s.doc.add_shape(Shape::new(ShapeKind::Step { center: Pt::new(0.0, 0.0), n: 5 }, [0; 4], 4.0));
            s.select_all();
            let rev = s.doc.revision();
            let before = (s.tool, s.styles[&Tool::Rect], s.selection().to_vec());
            let eff = s.dispatch(def.id, false);
            let after = (s.tool, s.style_of(Tool::Rect), s.selection().to_vec());
            let changed = eff != Effect::None || rev != s.doc.revision() || before != after || s.tool != Tool::Rect;
            // bring_front on [0,1] of 2 is a legit no-op; fill/etc. always change.
            let allowed_noop = matches!(def.id, "edit.bring_front" | "edit.send_back" | "tool.rect" | "tool.select" | "edit.select_all");
            assert!(changed || allowed_noop, "id {:?} did nothing", def.id);
        }
    }

    #[test]
    fn tool_ids_switch_tools() {
        let mut s = sess();
        assert_eq!(s.tool, Tool::Arrow);
        for t in Tool::ALL {
            assert_eq!(s.dispatch(t.id(), false), Effect::None);
            assert_eq!(s.tool, t);
        }
    }

    #[test]
    fn default_tool_from_config() {
        let cfg = EditorConfig { default_tool: "tool.blur".into(), ..Default::default() };
        assert_eq!(Session::new(&cfg).tool, Tool::Blur);
        let cfg = EditorConfig { default_tool: "bogus".into(), ..Default::default() };
        assert_eq!(Session::new(&cfg).tool, Tool::Arrow);
    }

    #[test]
    fn digit_colors_use_palette_and_selection() {
        let mut s = sess();
        let i = s.doc.add_shape(rect(0.0));
        s.select_one(Some(i));
        s.dispatch("style.color_5", false);
        assert_eq!(s.doc.shapes()[i].color, [0x1e, 0x88, 0xe5, 255]);
        assert_eq!(s.style().color, [0x1e, 0x88, 0xe5, 255]);
        assert!(s.doc.is_dirty());
        s.undo();
        assert_eq!(s.doc.shapes()[i].color, [1, 2, 3, 255]);
    }

    #[test]
    fn short_palette_ignores_missing_slots() {
        let cfg = EditorConfig { palette: vec!["#112233".into(), "nonsense".into()], ..Default::default() };
        let mut s = Session::new(&cfg);
        assert_eq!(s.palette.len(), 1);
        let before = s.style();
        s.dispatch("style.color_9", false);
        assert_eq!(s.style(), before);
    }

    #[test]
    fn highlighter_stays_translucent() {
        let mut s = sess();
        s.dispatch("tool.highlight", false);
        s.dispatch("style.color_3", false);
        assert_eq!(s.style().color[3], HIGHLIGHT_ALPHA);
    }

    #[test]
    fn width_keys_clamp() {
        let mut s = sess();
        let w0 = s.style().width;
        s.dispatch("style.width_up", false);
        assert_eq!(s.style().width, w0 + 1.0);
        for _ in 0..100 {
            s.dispatch("style.width_down", false);
        }
        assert_eq!(s.style().width, MIN_WIDTH);
        for _ in 0..100 {
            s.dispatch("style.width_up", false);
        }
        assert_eq!(s.style().width, MAX_WIDTH);
    }

    #[test]
    fn fill_and_shadow_toggle() {
        let mut s = sess();
        let i = s.doc.add_shape(rect(0.0));
        s.select_one(Some(i));
        s.dispatch("style.toggle_fill", false);
        s.dispatch("style.toggle_shadow", false);
        assert!(s.doc.shapes()[i].fill && s.doc.shapes()[i].shadow);
        s.dispatch("style.toggle_fill", false);
        assert!(!s.doc.shapes()[i].fill);
    }

    #[test]
    fn delete_duplicate_select_all() {
        let mut s = sess();
        s.doc.add_shape(rect(0.0));
        s.doc.add_shape(rect(20.0));
        s.dispatch("edit.select_all", false);
        assert_eq!(s.selection(), &[0, 1]);
        s.dispatch("edit.duplicate", false);
        assert_eq!(s.doc.shapes().len(), 4);
        assert_eq!(s.selection(), &[2, 3]);
        s.dispatch("edit.delete", false);
        assert_eq!(s.doc.shapes().len(), 2);
        assert!(s.selection().is_empty());
        s.dispatch("edit.undo", false);
        assert_eq!(s.doc.shapes().len(), 4);
        s.dispatch("edit.redo", false);
        assert_eq!(s.doc.shapes().len(), 2);
    }

    #[test]
    fn layer_order_commands() {
        let mut s = sess();
        s.doc.add_shape(rect(0.0));
        s.doc.add_shape(rect(20.0));
        s.select_one(Some(0));
        s.dispatch("edit.bring_front", false);
        assert_eq!(s.selection(), &[1]);
        assert_eq!(s.doc.shapes()[1].bounds().0.x, 0.0);
        s.dispatch("edit.send_back", false);
        assert_eq!(s.selection(), &[0]);
        assert_eq!(s.doc.shapes()[0].bounds().0.x, 0.0);
    }

    #[test]
    fn nudge_small_and_big() {
        let mut s = sess();
        let i = s.doc.add_shape(rect(0.0));
        s.select_one(Some(i));
        s.dispatch("edit.nudge_right", false);
        s.dispatch("edit.nudge_down", true);
        assert_eq!(s.doc.shapes()[i].bounds().0, Pt::new(1.0, 10.0));
        // nothing selected: nothing happens, no undo step
        s.clear_selection();
        let rev = s.doc.revision();
        s.dispatch("edit.nudge_left", false);
        assert_eq!(s.doc.revision(), rev);
    }

    #[test]
    fn reset_steps_command() {
        let mut s = sess();
        for n in [4, 5] {
            s.doc.add_shape(Shape::new(ShapeKind::Step { center: Pt::new(0.0, 0.0), n }, [0; 4], 4.0));
        }
        s.dispatch("edit.reset_steps", false);
        assert_eq!(s.doc.next_step_number(), 3);
    }

    #[test]
    fn esc_is_tiered() {
        let mut s = sess();
        let i = s.doc.add_shape(rect(0.0));
        s.select_one(Some(i));
        assert_eq!(s.dispatch("file.close", false), Effect::None);
        assert!(s.selection().is_empty());
        assert_eq!(s.dispatch("file.close", false), Effect::Close);
    }

    #[test]
    fn file_and_view_ids_map_to_effects() {
        let mut s = sess();
        for (id, e) in [
            ("file.copy", Effect::Copy),
            ("file.copy_close", Effect::CopyAndClose),
            ("file.save", Effect::Save),
            ("file.save_as", Effect::SaveAs),
            ("file.save_close", Effect::SaveAndClose),
            ("file.copy_path", Effect::CopyPath),
            ("file.open_with", Effect::OpenWith),
            ("file.open_folder", Effect::OpenFolder),
            ("view.cheatsheet", Effect::Cheatsheet),
            ("view.zoom_in", Effect::Zoom(ZoomCmd::In)),
            ("view.zoom_out", Effect::Zoom(ZoomCmd::Out)),
            ("view.zoom_fit", Effect::Zoom(ZoomCmd::Fit)),
            ("view.zoom_100", Effect::Zoom(ZoomCmd::Actual)),
        ] {
            assert_eq!(s.dispatch(id, false), e, "{id}");
        }
        assert_eq!(s.dispatch("nope.unknown", false), Effect::None);
    }

    #[test]
    fn eyedropper_returns_to_previous_tool() {
        let mut s = sess();
        s.dispatch("tool.rect", false);
        s.dispatch("tool.eyedropper", false);
        assert_eq!(s.style_tool(), Tool::Rect);
        s.pick_color([9, 8, 7, 255]);
        assert_eq!(s.tool, Tool::Rect);
        assert_eq!(s.style().color, [9, 8, 7, 255]);
    }

    #[test]
    fn hex_parse() {
        assert_eq!(parse_hex_color("#ff8000"), Some([255, 128, 0, 255]));
        assert_eq!(parse_hex_color("11223344"), Some([0x11, 0x22, 0x33, 0x44]));
        assert_eq!(parse_hex_color("#12"), None);
        assert_eq!(parse_hex_color("#gggggg"), None);
    }

    #[test]
    fn constrain_segment_snaps_45() {
        let (a, b) = constrain_drag(DragKind::Segment, Pt::new(0.0, 0.0), Pt::new(100.0, 10.0), true, false);
        assert_eq!(a, Pt::new(0.0, 0.0));
        assert!(b.y.abs() < 1e-3 && (b.x - 100.5).abs() < 1.0);
        let (_, b) = constrain_drag(DragKind::Segment, Pt::new(0.0, 0.0), Pt::new(50.0, 60.0), true, false);
        assert!((b.x - b.y).abs() < 1e-3);
        // no shift: untouched
        let (_, b) = constrain_drag(DragKind::Segment, Pt::new(0.0, 0.0), Pt::new(50.0, 7.0), false, false);
        assert_eq!(b, Pt::new(50.0, 7.0));
    }

    #[test]
    fn constrain_box_square_and_center() {
        let (_, b) = constrain_drag(DragKind::Boxed, Pt::new(10.0, 10.0), Pt::new(-20.0, 15.0), true, false);
        assert_eq!(b, Pt::new(-20.0, 40.0));
        let (a, b) = constrain_drag(DragKind::Boxed, Pt::new(50.0, 50.0), Pt::new(60.0, 70.0), false, true);
        assert_eq!(a, Pt::new(40.0, 30.0));
        assert_eq!(b, Pt::new(60.0, 70.0));
        let (a, b) = constrain_drag(DragKind::Boxed, Pt::new(50.0, 50.0), Pt::new(60.0, 70.0), true, true);
        assert_eq!(a, Pt::new(30.0, 30.0));
        assert_eq!(b, Pt::new(70.0, 70.0));
    }

    #[test]
    fn drop_index_maps_list_slots() {
        // 4 shapes; list top = doc 3.
        assert_eq!(drop_index(0, 0, 4), 3);
        assert_eq!(drop_index(3, 4, 4), 0);
        assert_eq!(drop_index(2, 1, 4), 2);
        assert_eq!(drop_index(2, 2, 4), 2);
        assert_eq!(drop_index(3, 2, 4), 2);
        assert_eq!(drop_index(0, 0, 0), 0);
    }

    #[test]
    fn index_after_move_follows_shifts() {
        assert_eq!(index_after_move(1, 1, 3), 3);
        assert_eq!(index_after_move(2, 1, 3), 1);
        assert_eq!(index_after_move(3, 1, 3), 2);
        assert_eq!(index_after_move(0, 1, 3), 0);
        assert_eq!(index_after_move(1, 3, 1), 2);
        assert_eq!(index_after_move(3, 3, 1), 1);
        assert_eq!(index_after_move(0, 3, 1), 0);
    }

    #[test]
    fn click_select_modes() {
        assert_eq!(click_select(&[1, 2], 4, false, false), vec![4]);
        assert_eq!(click_select(&[1, 2], 2, true, false), vec![1]);
        assert_eq!(click_select(&[1], 3, true, false), vec![1, 3]);
        assert_eq!(click_select(&[1], 3, false, true), vec![1, 2, 3]);
        assert_eq!(click_select(&[3], 1, false, true), vec![1, 2, 3]);
        assert_eq!(click_select(&[], 2, false, true), vec![2]);
    }

    #[test]
    fn move_object_keeps_selection_on_same_shapes() {
        let mut s = Session::new(&jepret_core::config::EditorConfig::default());
        for x in [0.0, 10.0, 20.0] {
            let sh = s.new_shape(ShapeKind::Rect { a: Pt::new(x, 0.0), b: Pt::new(x + 5.0, 5.0) });
            s.doc.add_shape(sh);
        }
        s.set_selection(vec![0]);
        s.move_object(0, 2);
        assert_eq!(s.selection(), &[2]);
        assert_eq!(s.doc.shapes()[2].bounds().0.x, 0.0);
    }
}
