//! egui front-end of the editor. Shapes are painted live with the egui
//! painter; the final pixels come from [`crate::render::flatten`].

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use egui::{
    pos2, vec2, Align2, Color32, ColorImage, Context, FontId, Key, Modifiers, PointerButton, Pos2,
    Rect as ERect, Sense, Shape as EShape, Stroke, TextureHandle, TextureOptions, Vec2,
};
use jepret_core::{Config, Image, Rect};

use crate::doc::{Document, Pt, Shape, ShapeKind, Tool};
use crate::{io, render};

type LoadResult = Result<Image, String>;

struct TextEdit {
    pos: Pt,
    text: String,
    /// Index of the shape being edited (None = new text).
    editing: Option<usize>,
    focus_pending: bool,
}

pub struct EditorApp {
    path: PathBuf,
    config: Config,
    base: Option<std::sync::Arc<Image>>,
    load_rx: Option<Receiver<LoadResult>>,
    load_started: Instant,
    load_err: Option<String>,
    tex: Option<TextureHandle>,

    doc: Document,
    tool: Tool,
    styles: HashMap<Tool, ([u8; 4], f32)>,
    selected: Option<usize>,
    drawing: Option<Shape>,
    moving: bool,
    prop_editing: bool,

    zoom: f32,
    pan: Vec2,
    fit_pending: bool,

    text_edit: Option<TextEdit>,
    confirm_close: bool,
    force_close: bool,
    status: String,
    last_title_dirty: Option<bool>,
    effect_cache: HashMap<u64, (Rect, TextureHandle)>,
    effect_used: HashSet<u64>,
}

impl EditorApp {
    pub fn new(path: PathBuf, config: Config) -> Self {
        // Decode on a worker so it overlaps with window/GL creation.
        let (tx, rx) = mpsc::channel();
        let p = path.clone();
        std::thread::spawn(move || {
            let r = io::load_image(&p).map_err(|e| format!("{e:#}"));
            let _ = tx.send(r);
        });
        let styles = Tool::ALL.iter().map(|t| (*t, (t.default_color(), t.default_width()))).collect();
        Self {
            path,
            config,
            base: None,
            load_rx: Some(rx),
            load_started: Instant::now(),
            load_err: None,
            tex: None,
            doc: Document::new(),
            tool: Tool::Arrow,
            styles,
            selected: None,
            drawing: None,
            moving: false,
            prop_editing: false,
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit_pending: true,
            text_edit: None,
            confirm_close: false,
            force_close: false,
            status: String::new(),
            last_title_dirty: None,
            effect_cache: HashMap::new(),
            effect_used: HashSet::new(),
        }
    }

    fn poll_load(&mut self, ctx: &Context) {
        let Some(rx) = &self.load_rx else { return };
        match rx.try_recv() {
            Ok(Ok(img)) => {
                tracing::debug!(
                    "editor: decoded {}x{} in {:?} (since start)",
                    img.width,
                    img.height,
                    self.load_started.elapsed()
                );
                self.tex = Some(make_texture(ctx, &img));
                tracing::debug!("editor: texture ready at {:?}", self.load_started.elapsed());
                self.base = Some(std::sync::Arc::new(img));
                self.load_rx = None;
                self.fit_pending = true;
            }
            Ok(Err(e)) => {
                self.load_err = Some(e);
                self.load_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint(),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.load_err = Some("loader thread died".into());
                self.load_rx = None;
            }
        }
    }

    fn style(&self, tool: Tool) -> ([u8; 4], f32) {
        self.styles.get(&tool).copied().unwrap_or(([255, 0, 0, 255], 4.0))
    }

    // ---- actions ---------------------------------------------------------

    fn flatten(&self) -> Option<Image> {
        let base = self.base.as_ref()?;
        let t = Instant::now();
        let out = render::flatten(base, &self.doc);
        tracing::debug!("editor: flatten {}x{} in {:?}", out.width, out.height, t.elapsed());
        Some(out)
    }

    fn save(&mut self, as_new: bool) -> bool {
        let Some(img) = self.flatten() else { return false };
        let mut target = self.path.clone();
        if as_new {
            let stem = self.path.file_stem().and_then(|s| s.to_str()).unwrap_or("capture");
            let mut dlg = rfd::FileDialog::new()
                .add_filter("PNG", &["png"])
                .add_filter("JPEG", &["jpg", "jpeg"])
                .set_file_name(format!("{stem}_edited.png"));
            if let Some(dir) = self.path.parent() {
                dlg = dlg.set_directory(dir);
            }
            match dlg.save_file() {
                Some(p) => target = p,
                None => return false,
            }
        }
        match io::save_image(&img, &target, self.config.general.image_format, self.config.general.jpeg_quality) {
            Ok(()) => {
                self.path = target;
                self.doc.mark_saved();
                self.status = format!("Saved {}", self.path.display());
                true
            }
            Err(e) => {
                tracing::warn!("editor save failed: {e:#}");
                self.status = format!("Save failed: {e:#}");
                false
            }
        }
    }

    fn copy(&mut self) -> bool {
        let Some(img) = self.flatten() else { return false };
        match io::copy_to_clipboard(&img) {
            Ok(()) => {
                self.status = "Copied to clipboard".into();
                true
            }
            Err(e) => {
                tracing::warn!("editor copy failed: {e:#}");
                self.status = format!("Copy failed: {e:#}");
                false
            }
        }
    }

    fn close(&mut self, ctx: &Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn commit_text(&mut self) {
        let Some(te) = self.text_edit.take() else { return };
        let (color, width) = self.style(Tool::Text);
        match te.editing {
            Some(i) => {
                if te.text.trim().is_empty() {
                    self.doc.remove_shape(i);
                    self.selected = None;
                } else {
                    self.doc.edit_shape(i, |s| {
                        if let ShapeKind::Text { text, .. } = &mut s.kind {
                            *text = te.text.clone();
                        }
                    });
                }
            }
            None => {
                if !te.text.trim().is_empty() {
                    let i = self.doc.add_shape(Shape::new(
                        ShapeKind::Text { pos: te.pos, text: te.text },
                        color,
                        width,
                    ));
                    self.selected = Some(i);
                }
            }
        }
    }

    fn delete_selected(&mut self) {
        if let Some(i) = self.selected.take() {
            self.doc.remove_shape(i);
        }
    }

    // ---- input -----------------------------------------------------------

    fn handle_keys(&mut self, ctx: &Context) {
        let typing = ctx.wants_keyboard_input();
        let copy_evt = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        let (save_as, save, redo_shift, undo, redo, copy_key) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::S),
                i.consume_key(Modifiers::COMMAND, Key::S),
                i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z),
                i.consume_key(Modifiers::COMMAND, Key::Z),
                i.consume_key(Modifiers::COMMAND, Key::Y),
                i.consume_key(Modifiers::COMMAND, Key::C),
            )
        });
        if self.text_edit.is_some() {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.text_edit = None;
            }
            // other shortcuts are for the text field now, except save.
            if save_as {
                self.commit_text();
                self.save(true);
            } else if save {
                self.commit_text();
                self.save(false);
            }
            return;
        }
        if save_as {
            self.save(true);
        } else if save {
            self.save(false);
        }
        if redo_shift || redo {
            self.doc.redo();
            self.selected = None;
        } else if undo {
            self.doc.undo();
            self.selected = None;
        }
        if !typing && (copy_key || copy_evt) {
            self.copy();
        }
        if typing {
            return;
        }
        if ctx.input(|i| i.key_pressed(Key::Enter) && i.modifiers.is_none()) {
            if self.copy() {
                self.force_close = true;
                self.close(ctx);
            }
            return;
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if self.drawing.is_some() {
                self.drawing = None;
            } else {
                self.close(ctx);
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace)) {
            self.delete_selected();
        }
        const DIGITS: [Key; 9] = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        for (i, k) in DIGITS.iter().enumerate() {
            if ctx.input(|inp| inp.key_pressed(*k) && inp.modifiers.is_none()) {
                if let Some(t) = Tool::from_digit(i as u8 + 1) {
                    self.set_tool(t);
                }
            }
        }
        for (k, t) in [(Key::H, Tool::Highlight), (Key::P, Tool::Pixelate), (Key::C, Tool::Crop)] {
            if ctx.input(|inp| inp.key_pressed(k) && inp.modifiers.is_none()) {
                self.set_tool(t);
            }
        }
    }

    fn set_tool(&mut self, t: Tool) {
        if self.tool != t {
            self.tool = t;
            self.drawing = None;
            if t != Tool::Select {
                // keep selection only if it is useful for property edits
            }
        }
    }

    // ---- panels ----------------------------------------------------------

    fn toolbar(&mut self, ctx: &Context) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (n, t) in Tool::ALL.iter().enumerate() {
                    let digit = (1..=9).find(|d| Tool::from_digit(*d) == Some(*t));
                    let label = match digit {
                        Some(d) => format!("{d} {}", t.label()),
                        None => t.label().to_string(),
                    };
                    if ui.selectable_label(self.tool == *t, label).clicked() {
                        self.commit_text();
                        self.set_tool(*t);
                    }
                    if n == 1 || n == 6 {
                        ui.separator();
                    }
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(self.doc.can_undo(), egui::Button::new("Undo")).clicked() {
                    self.doc.undo();
                    self.selected = None;
                }
                if ui.add_enabled(self.doc.can_redo(), egui::Button::new("Redo")).clicked() {
                    self.doc.redo();
                    self.selected = None;
                }
                ui.separator();
                self.properties(ui);
                ui.separator();
                if ui.add_enabled(self.selected.is_some(), egui::Button::new("Delete")).clicked() {
                    self.delete_selected();
                }
                if ui.add_enabled(self.doc.crop().is_some(), egui::Button::new("Reset crop")).clicked() {
                    self.doc.set_crop(None);
                }
                ui.separator();
                if ui.button("Save").clicked() {
                    self.save(false);
                }
                if ui.button("Save As...").clicked() {
                    self.save(true);
                }
                if ui.button("Copy").clicked() {
                    self.copy();
                }
                if ui.button("Copy & close").clicked() && self.copy() {
                    self.force_close = true;
                    self.close(ctx);
                }
                ui.separator();
                if ui.button("Fit").clicked() {
                    self.fit_pending = true;
                }
                if ui.button("100%").clicked() {
                    self.zoom = 1.0 / ctx.pixels_per_point();
                    self.fit_pending = false;
                }
                ui.label(format!("{:.0}%", self.zoom * ctx.pixels_per_point() * 100.0));
            });
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let dirty = if self.doc.is_dirty() { " *" } else { "" };
                ui.label(format!("{}{dirty}", self.path.display()));
                if let Some(b) = &self.base {
                    ui.separator();
                    let (w, h) = match self.doc.crop() {
                        Some(c) => (c.width, c.height),
                        None => (b.width, b.height),
                    };
                    ui.label(format!("{w} x {h}"));
                }
                if !self.status.is_empty() {
                    ui.separator();
                    ui.label(&self.status);
                }
            });
        });
    }

    /// Colour + width controls; edit the selection (live, one undo step per
    /// gesture) and the current tool's default style.
    fn properties(&mut self, ui: &mut egui::Ui) {
        let sel_shape = self
            .selected
            .and_then(|i| self.doc.shapes().get(i).cloned().map(|s| (i, s)));
        let editing_sel = sel_shape.is_some() && (self.tool == Tool::Select || self.styles.contains_key(&self.tool));
        let (mut color, mut width) = match (&sel_shape, self.tool) {
            (Some((_, s)), Tool::Select) => (s.color, s.width),
            _ => self.style(self.tool),
        };
        if matches!(self.tool, Tool::Crop) && sel_shape.is_none() {
            return;
        }
        let kind_for_label = sel_shape.as_ref().map(|(_, s)| &s.kind);
        let label = match (self.tool, kind_for_label) {
            (Tool::Blur | Tool::Pixelate, _) => "Strength",
            (Tool::Text, _) | (Tool::Select, Some(ShapeKind::Text { .. })) => "Size",
            (Tool::Step, _) | (Tool::Select, Some(ShapeKind::Step { .. })) => "Size",
            (Tool::Select, Some(ShapeKind::Blur { .. } | ShapeKind::Pixelate { .. })) => "Strength",
            _ => "Width",
        };
        let mut c32 = Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]);
        let r1 = ui.color_edit_button_srgba(&mut c32);
        let r2 = ui.add(egui::Slider::new(&mut width, 1.0..=24.0).text(label));
        let changed = r1.changed() || r2.changed();
        if changed {
            let [r, g, b, a] = c32.to_srgba_unmultiplied();
            color = [r, g, b, a];
            if self.tool != Tool::Select {
                self.styles.insert(self.tool, (color, width));
            }
            if editing_sel {
                if let Some((i, _)) = sel_shape {
                    if !self.prop_editing {
                        self.doc.begin_edit();
                        self.prop_editing = true;
                    }
                    if let Some(s) = self.doc.shape_live(i) {
                        s.color = color;
                        s.width = width;
                    }
                    self.doc.touch();
                }
            }
        }
        if ui.input(|i| !i.pointer.any_down()) {
            self.prop_editing = false;
        }
    }

    fn confirm_dialog(&mut self, ctx: &Context) {
        if !self.confirm_close {
            return;
        }
        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Discard your annotations?");
                ui.horizontal(|ui| {
                    if ui.button("Save & close").clicked() && self.save(false) {
                        self.force_close = true;
                        self.confirm_close = false;
                        self.close(ctx);
                    }
                    if ui.button("Discard").clicked() {
                        self.force_close = true;
                        self.confirm_close = false;
                        self.close(ctx);
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_close = false;
                    }
                });
            });
    }

    // ---- canvas ----------------------------------------------------------

    fn canvas(&mut self, ctx: &Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_gray(38)))
            .show(ctx, |ui| {
                if let Some(e) = &self.load_err {
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(Color32::LIGHT_RED, format!("Cannot open image: {e}"));
                    });
                    return;
                }
                let (Some(base), Some(tex)) = (self.base.clone(), self.tex.clone()) else {
                    ui.centered_and_justified(|ui| {
                        ui.spinner();
                    });
                    return;
                };
                let (iw, ih) = (base.width as f32, base.height as f32);
                let avail = ui.available_size();
                let (resp, painter) = ui.allocate_painter(avail, Sense::click_and_drag());
                let rect = resp.rect;

                if self.fit_pending && rect.width() > 10.0 {
                    let z = ((rect.width() - 24.0) / iw).min((rect.height() - 24.0) / ih).min(1.0 / ctx.pixels_per_point());
                    self.zoom = z.max(0.01);
                    self.pan = vec2((rect.width() - iw * self.zoom) / 2.0, (rect.height() - ih * self.zoom) / 2.0);
                    self.fit_pending = false;
                }

                // zoom around the cursor
                if resp.hovered() {
                    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                    if scroll != 0.0 {
                        if let Some(h) = resp.hover_pos() {
                            let old = self.zoom;
                            let new = (old * (scroll * 0.0015).exp()).clamp(0.02, 64.0);
                            let local = h - rect.min;
                            self.pan = local - (local - self.pan) * (new / old);
                            self.zoom = new;
                        }
                    }
                }
                let space = ui.input(|i| i.key_down(Key::Space));
                let panning = resp.dragged_by(PointerButton::Middle) || (space && resp.dragged_by(PointerButton::Primary));
                if panning {
                    self.pan += resp.drag_delta();
                }

                let zoom = self.zoom;
                let origin = rect.min + self.pan;
                let to_screen = |p: Pt| pos2(origin.x + p.x * zoom, origin.y + p.y * zoom);
                let to_img = |p: Pos2| Pt::new((p.x - origin.x) / zoom, (p.y - origin.y) / zoom);
                let img_rect = ERect::from_min_size(origin, vec2(iw * zoom, ih * zoom));

                // ---- tool interaction (not while panning) ----
                let primary = !panning && !space;
                if primary {
                    self.interact(ctx, &resp, &to_img, &base);
                }

                // ---- paint ----
                painter.rect_filled(img_rect, 0.0, Color32::from_gray(20));
                painter.image(tex.id(), img_rect, ERect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);

                self.effect_used.clear();
                let shapes: Vec<Shape> = self.doc.shapes().to_vec();
                for (i, s) in shapes.iter().enumerate() {
                    let skip_effect = self.moving && self.selected == Some(i);
                    self.paint_shape(ctx, &painter, &base, s, &to_screen, zoom, skip_effect);
                }
                let used = std::mem::take(&mut self.effect_used);
                self.effect_cache.retain(|k, _| used.contains(k));

                if let Some(d) = self.drawing.clone() {
                    if let ShapeKind::Blur { a, b } | ShapeKind::Pixelate { a, b } = &d.kind {
                        let r = ERect::from_two_pos(to_screen(*a), to_screen(*b));
                        painter.rect_filled(r, 0.0, Color32::from_white_alpha(40));
                        painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, Color32::WHITE));
                    } else if self.tool == Tool::Crop {
                        if let ShapeKind::Rect { a, b } = &d.kind {
                            self.paint_crop_overlay(&painter, img_rect, ERect::from_two_pos(to_screen(*a), to_screen(*b)));
                        }
                    } else {
                        self.paint_shape(ctx, &painter, &base, &d, &to_screen, zoom, true);
                    }
                } else if let Some(c) = self.doc.crop() {
                    let r = ERect::from_min_max(
                        to_screen(Pt::new(c.x as f32, c.y as f32)),
                        to_screen(Pt::new(c.right() as f32, c.bottom() as f32)),
                    );
                    self.paint_crop_overlay(&painter, img_rect, r);
                }

                if let Some(s) = self.selected.and_then(|i| shapes.get(i)) {
                    let (min, max) = s.bounds();
                    let r = ERect::from_min_max(to_screen(min), to_screen(max)).expand(5.0);
                    painter.rect_stroke(r, 2.0, Stroke::new(1.5_f32, Color32::from_rgb(60, 140, 255)));
                }

                // text editor overlay
                if self.text_edit.is_some() {
                    self.text_overlay(ctx, &to_screen, zoom);
                }
            });
    }

    fn paint_crop_overlay(&self, painter: &egui::Painter, full: ERect, crop: ERect) {
        let dim = Color32::from_black_alpha(140);
        let c = crop.intersect(full);
        painter.rect_filled(ERect::from_min_max(full.min, pos2(full.max.x, c.min.y)), 0.0, dim);
        painter.rect_filled(ERect::from_min_max(pos2(full.min.x, c.max.y), full.max), 0.0, dim);
        painter.rect_filled(ERect::from_min_max(pos2(full.min.x, c.min.y), pos2(c.min.x, c.max.y)), 0.0, dim);
        painter.rect_filled(ERect::from_min_max(pos2(c.max.x, c.min.y), pos2(full.max.x, c.max.y)), 0.0, dim);
        painter.rect_stroke(c, 0.0, Stroke::new(1.5_f32, Color32::WHITE));
    }

    fn interact(&mut self, ctx: &Context, resp: &egui::Response, to_img: &dyn Fn(Pos2) -> Pt, base: &Image) {
        let tol = 6.0 / self.zoom;
        let pos = resp.interact_pointer_pos().or_else(|| resp.hover_pos()).map(to_img);

        if resp.drag_started_by(PointerButton::Primary) {
            if let Some(p) = pos {
                self.commit_text();
                self.begin_drag(p, tol);
            }
        }
        if resp.dragged_by(PointerButton::Primary) {
            if let Some(p) = pos {
                self.update_drag(p, resp.drag_delta() / self.zoom);
            }
        }
        if resp.drag_stopped_by(PointerButton::Primary) {
            self.finish_drag(base);
        }
        if resp.clicked_by(PointerButton::Primary) {
            if let Some(p) = pos {
                match self.tool {
                    Tool::Select => {
                        self.commit_text();
                        self.selected = self.doc.hit_test(p, tol);
                    }
                    Tool::Text => {
                        self.commit_text();
                        self.text_edit = Some(TextEdit { pos: p, text: String::new(), editing: None, focus_pending: true });
                    }
                    Tool::Step => {
                        let (c, w) = self.style(Tool::Step);
                        let n = self.doc.next_step_number();
                        let i = self.doc.add_shape(Shape::new(ShapeKind::Step { center: p, n }, c, w));
                        self.selected = Some(i);
                    }
                    _ => {}
                }
            }
        }
        if resp.double_clicked_by(PointerButton::Primary) && self.tool == Tool::Select {
            if let Some(p) = pos {
                if let Some(i) = self.doc.hit_test(p, tol) {
                    if let ShapeKind::Text { pos, text } = &self.doc.shapes()[i].kind {
                        self.text_edit =
                            Some(TextEdit { pos: *pos, text: text.clone(), editing: Some(i), focus_pending: true });
                    }
                }
            }
        }
        let _ = ctx;
    }

    fn begin_drag(&mut self, p: Pt, tol: f32) {
        self.moving = false;
        let (color, width) = self.style(self.tool);
        let two = |t: Tool| match t {
            Tool::Arrow => Some(ShapeKind::Arrow { a: p, b: p }),
            Tool::Line => Some(ShapeKind::Line { a: p, b: p }),
            Tool::Rect | Tool::Crop => Some(ShapeKind::Rect { a: p, b: p }),
            Tool::Ellipse => Some(ShapeKind::Ellipse { a: p, b: p }),
            Tool::Highlight => Some(ShapeKind::Highlight { a: p, b: p }),
            Tool::Blur => Some(ShapeKind::Blur { a: p, b: p }),
            Tool::Pixelate => Some(ShapeKind::Pixelate { a: p, b: p }),
            Tool::Pen => Some(ShapeKind::Pen { points: vec![p] }),
            _ => None,
        };
        match self.tool {
            Tool::Select => {
                self.selected = self.doc.hit_test(p, tol);
            }
            t => {
                if let Some(kind) = two(t) {
                    self.drawing = Some(Shape::new(kind, color, width));
                }
            }
        }
    }

    fn update_drag(&mut self, p: Pt, delta: Vec2) {
        if let Some(d) = &mut self.drawing {
            match &mut d.kind {
                ShapeKind::Arrow { b, .. }
                | ShapeKind::Line { b, .. }
                | ShapeKind::Rect { b, .. }
                | ShapeKind::Ellipse { b, .. }
                | ShapeKind::Highlight { b, .. }
                | ShapeKind::Blur { b, .. }
                | ShapeKind::Pixelate { b, .. } => *b = p,
                ShapeKind::Pen { points } => {
                    if points.last().is_none_or(|l| l.dist(p) >= 1.0) {
                        points.push(p);
                    }
                }
                _ => {}
            }
        } else if self.tool == Tool::Select {
            if let Some(i) = self.selected {
                if delta != Vec2::ZERO {
                    if !self.moving {
                        self.doc.begin_edit();
                        self.moving = true;
                    }
                    self.doc.translate_live(i, delta.x, delta.y);
                }
            }
        }
    }

    fn finish_drag(&mut self, base: &Image) {
        self.moving = false;
        let Some(d) = self.drawing.take() else { return };
        if self.tool == Tool::Crop {
            if let ShapeKind::Rect { a, b } = d.kind {
                let x0 = a.x.min(b.x).round().max(0.0);
                let y0 = a.y.min(b.y).round().max(0.0);
                let x1 = a.x.max(b.x).round().min(base.width as f32);
                let y1 = a.y.max(b.y).round().min(base.height as f32);
                if x1 - x0 >= 4.0 && y1 - y0 >= 4.0 {
                    self.doc.set_crop(Some(Rect::new(x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32)));
                }
            }
            return;
        }
        let big_enough = match &d.kind {
            ShapeKind::Pen { points } => points.len() >= 2,
            ShapeKind::Arrow { a, b }
            | ShapeKind::Line { a, b }
            | ShapeKind::Rect { a, b }
            | ShapeKind::Ellipse { a, b }
            | ShapeKind::Highlight { a, b }
            | ShapeKind::Blur { a, b }
            | ShapeKind::Pixelate { a, b } => a.dist(*b) >= 3.0,
            _ => true,
        };
        if big_enough {
            let i = self.doc.add_shape(d);
            self.selected = Some(i);
        }
    }

    fn text_overlay(&mut self, ctx: &Context, to_screen: &dyn Fn(Pt) -> Pos2, zoom: f32) {
        let (color, width) = self.style(Tool::Text);
        let Some(te) = &mut self.text_edit else { return };
        let (color, px) = match te.editing.and_then(|i| self.doc.shapes().get(i)) {
            Some(s) => (s.color, s.text_px()),
            None => (color, (width * crate::doc::TEXT_PX_PER_WIDTH).max(6.0)),
        };
        let pos = to_screen(te.pos);
        let mut commit = false;
        egui::Area::new(egui::Id::new("text_edit")).fixed_pos(pos).order(egui::Order::Foreground).show(ctx, |ui| {
            let edit = egui::TextEdit::multiline(&mut te.text)
                .font(FontId::proportional((px * zoom).max(8.0)))
                .text_color(Color32::from_rgba_unmultiplied(color[0], color[1], color[2], 255))
                .desired_rows(1)
                .desired_width(160.0)
                .hint_text("text, click outside to finish");
            let r = ui.add(edit);
            if te.focus_pending {
                r.request_focus();
                te.focus_pending = false;
            }
            if r.lost_focus() {
                commit = true;
            }
        });
        if commit {
            self.commit_text();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_shape(
        &mut self,
        ctx: &Context,
        painter: &egui::Painter,
        base: &Image,
        s: &Shape,
        to_screen: &dyn Fn(Pt) -> Pos2,
        zoom: f32,
        skip_effect: bool,
    ) {
        let c = Color32::from_rgba_unmultiplied(s.color[0], s.color[1], s.color[2], s.color[3]);
        let w = s.width.max(1.0);
        let stroke = Stroke::new((w * zoom).max(1.0), c);
        match &s.kind {
            ShapeKind::Line { a, b } => {
                painter.line_segment([to_screen(*a), to_screen(*b)], stroke);
            }
            ShapeKind::Arrow { a, b } => {
                painter.line_segment([to_screen(*a), to_screen(*b)], stroke);
                let (dx, dy) = (a.x - b.x, a.y - b.y);
                let len = (dx * dx + dy * dy).sqrt();
                if len > 0.5 {
                    let head = (w * 4.0).max(12.0).min(len);
                    let ang = dy.atan2(dx);
                    for off in [-0.5f32, 0.5] {
                        let t = ang + off;
                        let tip = Pt::new(b.x + head * t.cos(), b.y + head * t.sin());
                        painter.line_segment([to_screen(*b), to_screen(tip)], stroke);
                    }
                }
            }
            ShapeKind::Rect { a, b } => {
                painter.rect_stroke(ERect::from_two_pos(to_screen(*a), to_screen(*b)), 0.0, stroke);
            }
            ShapeKind::Ellipse { a, b } => {
                let r = ERect::from_two_pos(to_screen(*a), to_screen(*b));
                let (c0, rad) = (r.center(), r.size() / 2.0);
                let pts: Vec<Pos2> = (0..64)
                    .map(|k| {
                        let t = k as f32 / 64.0 * std::f32::consts::TAU;
                        pos2(c0.x + rad.x * t.cos(), c0.y + rad.y * t.sin())
                    })
                    .collect();
                painter.add(EShape::closed_line(pts, stroke));
            }
            ShapeKind::Highlight { a, b } => {
                painter.rect_filled(ERect::from_two_pos(to_screen(*a), to_screen(*b)), 0.0, c);
            }
            ShapeKind::Blur { a, b } | ShapeKind::Pixelate { a, b } => {
                let r = ERect::from_two_pos(to_screen(*a), to_screen(*b));
                if skip_effect {
                    painter.rect_filled(r, 0.0, Color32::from_white_alpha(40));
                    painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, Color32::WHITE));
                    return;
                }
                let key = {
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    matches!(s.kind, ShapeKind::Blur { .. }).hash(&mut h);
                    for v in [a.x, a.y, b.x, b.y, s.width] {
                        v.to_bits().hash(&mut h);
                    }
                    h.finish()
                };
                self.effect_used.insert(key);
                if !self.effect_cache.contains_key(&key) {
                    if let Some((reg, img)) = render::effect_preview(base, s) {
                        let tex = make_texture(ctx, &img);
                        self.effect_cache.insert(key, (reg, tex));
                    }
                }
                if let Some((reg, tex)) = self.effect_cache.get(&key) {
                    let rr = ERect::from_min_max(
                        to_screen(Pt::new(reg.x as f32, reg.y as f32)),
                        to_screen(Pt::new(reg.right() as f32, reg.bottom() as f32)),
                    );
                    painter.image(tex.id(), rr, ERect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
            }
            ShapeKind::Text { pos, text } => {
                painter.text(
                    to_screen(*pos),
                    Align2::LEFT_TOP,
                    text,
                    FontId::proportional(s.text_px() * zoom),
                    c,
                );
            }
            ShapeKind::Step { center, n } => {
                let r = s.step_radius();
                painter.circle_filled(to_screen(*center), r * zoom, c);
                painter.text(
                    to_screen(*center),
                    Align2::CENTER_CENTER,
                    n.to_string(),
                    FontId::proportional(r * 1.15 * zoom),
                    Color32::WHITE,
                );
            }
            ShapeKind::Pen { points } => {
                let pts: Vec<Pos2> = points.iter().map(|p| to_screen(*p)).collect();
                if pts.len() == 1 {
                    painter.circle_filled(pts[0], (w * zoom / 2.0).max(0.5), c);
                } else {
                    painter.add(EShape::line(pts, stroke));
                }
            }
        }
    }
}

fn make_texture(ctx: &Context, img: &Image) -> TextureHandle {
    let max = ctx.input(|i| i.max_texture_side).max(512);
    let name = "jepret-image";
    let (w, h) = (img.width as usize, img.height as usize);
    if w <= max && h <= max {
        let ci = ColorImage::from_rgba_unmultiplied([w, h], &img.pixels);
        return ctx.load_texture(name, ci, TextureOptions::LINEAR);
    }
    // Larger than the GPU limit: nearest-neighbour downscale for display only.
    let f = w.max(h).div_ceil(max);
    let (nw, nh) = (w / f, h / f);
    let mut px = Vec::with_capacity(nw * nh * 4);
    for y in 0..nh {
        for x in 0..nw {
            let i = (y * f * w + x * f) * 4;
            px.extend_from_slice(&img.pixels[i..i + 4]);
        }
    }
    let ci = ColorImage::from_rgba_unmultiplied([nw, nh], &px);
    ctx.load_texture(name, ci, TextureOptions::LINEAR)
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.poll_load(ctx);

        if ctx.input(|i| i.viewport().close_requested()) && !self.force_close && self.doc.is_dirty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }

        self.handle_keys(ctx);
        self.toolbar(ctx);
        self.canvas(ctx);
        self.confirm_dialog(ctx);

        let dirty = self.doc.is_dirty();
        if self.last_title_dirty != Some(dirty) {
            self.last_title_dirty = Some(dirty);
            let name = self.path.file_name().and_then(|n| n.to_str()).unwrap_or("image");
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                "Jepret Editor - {name}{}",
                if dirty { " *" } else { "" }
            )));
        }
    }
}

/// Build native options + run the app (blocks).
pub fn run(path: PathBuf, config: Config) -> jepret_core::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([640.0, 400.0])
            .with_title("Jepret Editor"),
        ..Default::default()
    };
    let app = EditorApp::new(path, config);
    eframe::run_native("Jepret Editor", options, Box::new(move |_cc| Ok(Box::new(app))))
        .map_err(|e| anyhow::anyhow!("editor window failed: {e}"))
}
