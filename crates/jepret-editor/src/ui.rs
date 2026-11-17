//! egui front-end of the editor. Shapes are painted live with the egui
//! painter; the final pixels come from [`crate::render::flatten`].
//! All keyboard behaviour goes through [`crate::input::Keymap`] ->
//! shortcut id -> [`Session::dispatch`] -> [`Effect`].

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Instant;

use egui::{
    pos2, vec2, Align2, Color32, ColorImage, Context, FontId, Key, Modifiers, PointerButton, Pos2,
    Rect as ERect, Sense, Shape as EShape, Stroke, TextureHandle, TextureOptions, Vec2,
};
use jepret_core::config::ExternalApp;
use jepret_core::keys::KeyCombo;
use jepret_core::shortcuts::{editor_shortcut, EDITOR_SHORTCUTS};
use jepret_ui_theme::tokens::{
    CONTROL_H, RADIUS_CONTROL, RADIUS_PANEL, SP_1, SP_2, SP_3, SP_4, TEXT_BODY, TEXT_SMALL, TOOL_BTN,
};
use jepret_ui_theme::widgets::{button, kbd, primary_button, section_header, toggle, IconButton};
use jepret_ui_theme::{icons, Icon, Palette};
use jepret_core::{Config, Image, Rect};

use crate::doc::{Pt, Shape, ShapeKind, Tool};
use crate::input::{combo_from_key, Keymap};
use crate::session::{constrain_drag, DragKind, Effect, Session, ZoomCmd};
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
    pal: Palette,
    keymap: Keymap,
    base: Option<Arc<Image>>,
    load_rx: Option<Receiver<LoadResult>>,
    load_started: Instant,
    load_err: Option<String>,
    tex: Option<TextureHandle>,

    s: Session,
    drawing: Option<Shape>,
    drag_start: Pt,
    moving: bool,
    prop_editing: bool,

    zoom: f32,
    pan: Vec2,
    fit_pending: bool,
    canvas_rect: ERect,

    text_edit: Option<TextEdit>,
    confirm_close: bool,
    force_close: bool,
    show_help: bool,
    help_query: String,
    help_was_open: bool,
    open_with_menu: bool,
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
        let keymap = Keymap::from_config(&config.editor);
        for w in &keymap.warnings {
            tracing::warn!("editor shortcuts: {w}");
        }
        let s = Session::new(&config.editor);
        Self {
            path,
            pal: Palette::for_mode(config.general.theme),
            config,
            keymap,
            base: None,
            load_rx: Some(rx),
            load_started: Instant::now(),
            load_err: None,
            tex: None,
            s,
            drawing: None,
            drag_start: Pt::default(),
            moving: false,
            prop_editing: false,
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit_pending: true,
            canvas_rect: ERect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)),
            text_edit: None,
            confirm_close: false,
            force_close: false,
            show_help: false,
            help_query: String::new(),
            help_was_open: false,
            open_with_menu: false,
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
                    "editor: decoded {}x{} at {:?} since start",
                    img.width,
                    img.height,
                    self.load_started.elapsed()
                );
                self.tex = Some(make_texture(ctx, &img));
                tracing::debug!("editor: texture ready at {:?}", self.load_started.elapsed());
                self.base = Some(Arc::new(img));
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

    /// Tooltip text with the bound shortcut(s) when hints are enabled.
    fn tip(&self, label: &str, id: &str) -> String {
        let hint = self.keymap.hint_all(id);
        if self.config.editor.show_shortcut_hints && !hint.is_empty() {
            format!("{label}  ({hint})")
        } else {
            label.to_string()
        }
    }

    // ---- actions ---------------------------------------------------------

    fn flatten(&self) -> Option<Image> {
        let base = self.base.as_ref()?;
        let t = Instant::now();
        let out = render::flatten(base, &self.s.doc);
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
                self.s.doc.mark_saved();
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

    fn copy_path(&mut self) {
        let r = arboard::Clipboard::new().and_then(|mut c| c.set_text(self.path.display().to_string()));
        self.status = match r {
            Ok(()) => format!("Path copied: {}", self.path.display()),
            Err(e) => format!("Copy path failed: {e}"),
        };
    }

    fn open_folder(&mut self) {
        let r = std::process::Command::new("explorer")
            .arg(format!("/select,{}", self.path.display()))
            .spawn();
        if let Err(e) = r {
            self.status = format!("Cannot open folder: {e}");
        }
    }

    /// Open the current picture (with annotations, via a temp file when
    /// there are unsaved edits) in an external app from `[apps]`.
    fn launch_app(&mut self, key: &str, app: &ExternalApp) {
        let mut file = self.path.clone();
        if self.s.doc.is_dirty() {
            let name = self.path.file_name().and_then(|n| n.to_str()).unwrap_or("capture.png");
            let tmp = std::env::temp_dir().join(format!("jepret-edit-{name}"));
            let Some(img) = self.flatten() else { return };
            if let Err(e) = io::save_image(&img, &tmp, self.config.general.image_format, self.config.general.jpeg_quality) {
                self.status = format!("Cannot write temp file: {e:#}");
                return;
            }
            file = tmp;
        }
        let file_s = file.display().to_string();
        let args: Vec<String> = app.args.iter().map(|a| a.replace("{file}", &file_s)).collect();
        match std::process::Command::new(&app.path).args(args).spawn() {
            Ok(_) => self.status = format!("Opened in {}", app.name.as_deref().unwrap_or(key)),
            Err(e) => self.status = format!("Cannot start {}: {e}", app.path.display()),
        }
    }

    fn close(&mut self, ctx: &Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn zoom_by(&mut self, f: f32) {
        let old = self.zoom;
        let new = (old * f).clamp(0.02, 64.0);
        let local = self.canvas_rect.size() / 2.0;
        let local = pos2(local.x, local.y);
        let pan = pos2(self.pan.x, self.pan.y);
        let np = local - (local - pan) * (new / old);
        self.pan = vec2(np.x, np.y);
        self.zoom = new;
        self.fit_pending = false;
    }

    fn commit_text(&mut self) {
        let Some(te) = self.text_edit.take() else { return };
        match te.editing {
            Some(i) => {
                if te.text.trim().is_empty() {
                    self.s.doc.remove_shape(i);
                    self.s.clear_selection();
                } else {
                    self.s.doc.edit_shape(i, |sh| {
                        if let ShapeKind::Text { text, .. } = &mut sh.kind {
                            *text = te.text.clone();
                        }
                    });
                }
            }
            None => {
                if !te.text.trim().is_empty() {
                    let sh = self.s.new_shape(ShapeKind::Text { pos: te.pos, text: te.text });
                    self.s.add_and_select(sh);
                }
            }
        }
    }

    // ---- commands --------------------------------------------------------

    fn run_effect(&mut self, ctx: &Context, e: Effect) {
        match e {
            Effect::None => {}
            Effect::Copy => {
                self.copy();
            }
            Effect::CopyAndClose => {
                if self.copy() {
                    self.force_close = true;
                    self.close(ctx);
                }
            }
            Effect::Save => {
                self.save(false);
            }
            Effect::SaveAs => {
                self.save(true);
            }
            Effect::SaveAndClose => {
                if self.save(false) {
                    self.force_close = true;
                    self.close(ctx);
                }
            }
            Effect::CopyPath => self.copy_path(),
            Effect::OpenWith => {
                if self.config.apps.is_empty() {
                    self.status = "No external apps configured ([apps] in config.toml)".into();
                } else {
                    self.open_with_menu = !self.open_with_menu;
                }
            }
            Effect::OpenFolder => self.open_folder(),
            Effect::Close => self.close(ctx),
            Effect::Cheatsheet => self.show_help = !self.show_help,
            Effect::Zoom(z) => match z {
                ZoomCmd::In => self.zoom_by(1.25),
                ZoomCmd::Out => self.zoom_by(0.8),
                ZoomCmd::Fit => self.fit_pending = true,
                ZoomCmd::Actual => {
                    self.zoom = 1.0 / ctx.pixels_per_point();
                    self.fit_pending = false;
                }
            },
        }
    }

    /// Run a shortcut / toolbar command by catalogue id.
    fn run_command(&mut self, ctx: &Context, id: &str, big: bool) {
        // Tiered "close": cancel drag -> close overlays -> deselect -> close.
        if id == "file.close" {
            if self.drawing.take().is_some() {
                return;
            }
            if self.show_help {
                self.show_help = false;
                return;
            }
            if self.open_with_menu {
                self.open_with_menu = false;
                return;
            }
        }
        let was = self.s.tool;
        let e = self.s.dispatch(id, big);
        if self.s.tool != was {
            self.drawing = None;
        }
        self.run_effect(ctx, e);
    }

    fn handle_keys(&mut self, ctx: &Context) {
        // Typing in a text object: only Esc and Ctrl+Enter are shortcuts.
        if self.text_edit.is_some() {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                self.text_edit = None;
            } else if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::Enter)) {
                self.commit_text();
            }
            return;
        }
        let events = ctx.input(|i| i.events.clone());
        let mods = ctx.input(|i| i.modifiers);
        let mut combos: Vec<(KeyCombo, bool)> = Vec::new();
        for ev in &events {
            match ev {
                egui::Event::Key { key, pressed: true, repeat, modifiers, .. } => {
                    if let Some(c) = combo_from_key(*key, *modifiers) {
                        combos.push((c, *repeat));
                    }
                }
                // egui-winit turns Ctrl(+Shift)+C / Ctrl+X into these and
                // drops the key event.
                egui::Event::Copy => {
                    let mut c = KeyCombo::key("c");
                    c.ctrl = true;
                    c.shift = mods.shift;
                    combos.push((c, false));
                }
                egui::Event::Cut => {
                    let mut c = KeyCombo::key("x");
                    c.ctrl = true;
                    c.shift = mods.shift;
                    combos.push((c, false));
                }
                _ => {}
            }
        }
        for (combo, repeat) in combos {
            if self.confirm_close {
                if combo.to_string() == "escape" {
                    self.confirm_close = false;
                }
                continue;
            }
            let Some((id, big)) = self.keymap.resolve(&combo) else { continue };
            // The cheat-sheet has a search box: typing must not fire shortcuts.
            if self.show_help && !(id == "file.close" || (id == "view.cheatsheet" && combo.key == "f1")) {
                continue;
            }
            let repeatable = id.starts_with("edit.nudge")
                || id.starts_with("style.width")
                || id == "edit.undo"
                || id == "edit.redo"
                || id == "edit.delete";
            if repeat && !repeatable {
                continue;
            }
            self.run_command(ctx, id, big);
        }
    }

    // ---- panels ----------------------------------------------------------

    /// Optional shortcut text for a button, honouring `show_shortcut_hints`.
    fn sc(&self, id: &str) -> Option<String> {
        let h = self.keymap.hint(id);
        (self.config.editor.show_shortcut_hints && !h.is_empty()).then_some(h)
    }

    /// Top/bottom/left/right panels (the central canvas is added after).
    fn toolbar(&mut self, ctx: &Context) {
        let pal = self.pal;
        let cmd = self.command_bar(ctx);
        let status = self.status_bar(ctx);
        let tools = self.tool_panel(ctx);
        let props = self.props_panel(ctx);
        // 1 px separators drawn on top of the panel fills.
        let p = ctx.layer_painter(egui::LayerId::background());
        let line = Stroke::new(1.0_f32, pal.line);
        p.hline(cmd.x_range(), cmd.bottom() - 0.5, line);
        p.hline(status.x_range(), status.top() + 0.5, line);
        p.vline(tools.right() - 0.5, tools.y_range(), line);
        p.vline(props.left() + 0.5, props.y_range(), line);
    }

    fn command_bar(&mut self, ctx: &Context) -> ERect {
        let pal = self.pal;
        let (h_undo, h_redo) = (self.sc("edit.undo"), self.sc("edit.redo"));
        let (h_copy, h_save, h_open, h_close) =
            (self.sc("file.copy"), self.sc("file.save"), self.sc("file.open_with"), self.sc("file.copy_close"));
        let info = self.info_text();
        let can_undo = self.s.doc.can_undo();
        let can_redo = self.s.doc.can_redo();
        let has_crop = self.s.doc.crop().is_some();
        let mut cmd: Option<&'static str> = None;
        let mut reset_crop = false;
        let r = egui::TopBottomPanel::top("cmdbar")
            .exact_height(52.0)
            .frame(egui::Frame::none().fill(pal.surface).inner_margin(egui::Margin::symmetric(SP_3, 0.0)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = SP_2;
                    ui.add_enabled_ui(can_undo, |ui| {
                        if IconButton::new(Icon::Undo, "Undo").shortcut(h_undo.as_deref()).show(ui, &pal).clicked() {
                            cmd = Some("edit.undo");
                        }
                    });
                    ui.add_enabled_ui(can_redo, |ui| {
                        if IconButton::new(Icon::Redo, "Redo").shortcut(h_redo.as_deref()).show(ui, &pal).clicked() {
                            cmd = Some("edit.redo");
                        }
                    });
                    let (sep, _) = ui.allocate_exact_size(vec2(1.0, 24.0), Sense::hover());
                    ui.painter().rect_filled(sep, 0.0, pal.line);
                    ui.label(egui::RichText::new(info).size(TEXT_SMALL).color(pal.muted));
                    if has_crop && button(ui, &pal, None, "Reset crop", None).clicked() {
                        reset_crop = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if primary_button(ui, &pal, "Copy & tutup", h_close.as_deref()).clicked() {
                            cmd = Some("file.copy_close");
                        }
                        if button(ui, &pal, Some(Icon::OpenExternal), "Buka di…", h_open.as_deref()).clicked() {
                            cmd = Some("file.open_with");
                        }
                        if button(ui, &pal, Some(Icon::Save), "Simpan", h_save.as_deref()).clicked() {
                            cmd = Some("file.save");
                        }
                        if button(ui, &pal, Some(Icon::Copy), "Copy", h_copy.as_deref()).clicked() {
                            cmd = Some("file.copy");
                        }
                    });
                });
            });
        if reset_crop {
            self.s.doc.set_crop(None);
        }
        if let Some(id) = cmd {
            self.commit_text();
            self.run_command(ctx, id, false);
        }
        r.response.rect
    }

    /// "Panah · warna 1 · tebal 4 px"
    fn info_text(&self) -> String {
        let tool = self.s.style_tool();
        let name = editor_shortcut(tool.id()).map(|d| d.label).unwrap_or(tool.label());
        let cur = self.s.color_now();
        let color = match self.s.palette.iter().take(9).position(|c| c[..3] == cur[..3]) {
            Some(i) => format!("warna {}", i + 1),
            None => "warna kustom".to_string(),
        };
        let (kind, unit) = self.width_kind();
        format!("{name} · {color} · {} {:.0}{unit}", kind.to_lowercase(), self.s.width_now())
    }

    /// Label and unit of the width knob for the active tool / selection.
    fn width_kind(&self) -> (&'static str, &'static str) {
        let sel_kind = self.s.selection().first().and_then(|i| self.s.doc.shapes().get(*i)).map(|s| &s.kind);
        match (self.s.style_tool(), sel_kind) {
            (Tool::Blur | Tool::Pixelate, _) | (_, Some(ShapeKind::Blur { .. } | ShapeKind::Pixelate { .. })) => {
                ("Kekuatan", "")
            }
            (Tool::Text | Tool::Step, _) | (_, Some(ShapeKind::Text { .. } | ShapeKind::Step { .. })) => ("Ukuran", ""),
            _ => ("Tebal", " px"),
        }
    }

    fn tool_tooltip(t: Tool) -> String {
        let name = editor_shortcut(t.id()).map(|d| d.label).unwrap_or(t.label());
        match t {
            Tool::Arrow | Tool::Line => format!("{name} · Shift = lurus 45°"),
            Tool::Rect | Tool::Ellipse | Tool::Highlight | Tool::Blur | Tool::Pixelate => {
                format!("{name} · Shift = proporsional · Alt = dari tengah")
            }
            _ => name.to_string(),
        }
    }

    fn tool_panel(&mut self, ctx: &Context) -> ERect {
        let pal = self.pal;
        let mut picked: Option<Tool> = None;
        let r = egui::SidePanel::left("tools")
            .exact_width(56.0)
            .resizable(false)
            .frame(egui::Frame::none().fill(pal.surface).inner_margin(egui::Margin { top: 10.0, ..Default::default() }))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.vertical_centered(|ui| {
                    for (t, (icon, _)) in Tool::ALL.iter().zip(Icon::TOOLS.iter()) {
                        let hint = self.sc(t.id());
                        let tip = Self::tool_tooltip(*t);
                        let b = IconButton::new(*icon, &tip)
                            .size(TOOL_BTN)
                            .selected(self.s.tool == *t)
                            .shortcut(hint.as_deref())
                            .corner_key(true);
                        if b.show(ui, &pal).clicked() {
                            picked = Some(*t);
                        }
                    }
                });
            });
        if let Some(t) = picked {
            self.commit_text();
            self.run_command(ctx, t.id(), false);
        }
        r.response.rect
    }

    fn status_bar(&mut self, ctx: &Context) -> ERect {
        let pal = self.pal;
        let size = self.base.as_ref().map(|b| match self.s.doc.crop() {
            Some(c) => (c.width, c.height),
            None => (b.width, b.height),
        });
        let zoom = self.zoom * ctx.pixels_per_point() * 100.0;
        let (h_fit, h_100, h_help) =
            (self.keymap.hint("view.zoom_fit"), self.keymap.hint("view.zoom_100"), self.keymap.hint("view.cheatsheet"));
        let count = self.s.doc.shapes().len();
        let r = egui::TopBottomPanel::bottom("status")
            .exact_height(28.0)
            .frame(egui::Frame::none().fill(pal.surface).inner_margin(egui::Margin::symmetric(14.0, 0.0)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = SP_4;
                    let muted = |t: String| egui::RichText::new(t).size(TEXT_SMALL).color(pal.muted);
                    let mono = |t: &str| egui::RichText::new(t.to_owned()).font(FontId::monospace(10.0)).color(pal.muted);
                    if let Some((w, h)) = size {
                        ui.label(muted(format!("{w} × {h} px")));
                    }
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = SP_1;
                        ui.label(muted(format!("Zoom {zoom:.0}%")));
                        if !h_fit.is_empty() {
                            ui.label(mono(&h_fit));
                            ui.label(muted("pas ·".into()));
                        }
                        if !h_100.is_empty() {
                            ui.label(mono(&h_100));
                            ui.label(muted("100%".into()));
                        }
                    });
                    ui.label(muted(format!("{count} objek")));
                    if !self.status.is_empty() {
                        ui.label(egui::RichText::new(&self.status).size(TEXT_SMALL).color(pal.text));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = SP_2;
                        if !h_help.is_empty() {
                            ui.label(egui::RichText::new("Semua shortcut").size(TEXT_SMALL).color(pal.text));
                            kbd(ui, &pal, &h_help);
                        }
                        ui.label(muted("Space+drag geser · Ctrl+scroll zoom".into()));
                    });
                });
            });
        r.response.rect
    }

    fn props_panel(&mut self, ctx: &Context) -> ERect {
        let pal = self.pal;
        let r = egui::SidePanel::right("props")
            .exact_width(248.0)
            .resizable(false)
            .frame(egui::Frame::none().fill(pal.surface).inner_margin(egui::Margin::same(SP_4)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 10.0;
                    self.style_section(ui, ctx);
                    self.object_section(ui, ctx);
                });
            });
        r.response.rect
    }

    /// Colour swatches, width slider, fill/shadow toggles.
    fn style_section(&mut self, ui: &mut egui::Ui, ctx: &Context) {
        let pal = self.pal;
        // Crop has no style; keep the panel quiet until something is selected.
        if self.s.tool == Tool::Crop && self.s.selection().is_empty() {
            return;
        }
        section_header(ui, &pal, "Warna");
        let palette = self.s.palette.clone();
        let cur = self.s.color_now();
        let cell = ((ui.available_width() - 4.0 * SP_2) / 5.0).floor();
        let mut cur32 = Color32::from_rgba_unmultiplied(cur[0], cur[1], cur[2], cur[3]);
        let mut picker_changed = false;
        ui.spacing_mut().item_spacing = vec2(SP_2, SP_2);
        for row in 0..2 {
            ui.horizontal(|ui| {
                for col in 0..5 {
                    let i = row * 5 + col;
                    if i < 9 {
                        let Some(c) = palette.get(i) else { continue };
                        let fill = Color32::from_rgb(c[0], c[1], c[2]);
                        let (rect, resp) = ui.allocate_exact_size(vec2(cell, 34.0), Sense::click());
                        let on = c[..3] == cur[..3];
                        let stroke = if on { Stroke::new(2.0_f32, pal.text) } else { Stroke::new(1.0_f32, pal.line) };
                        ui.painter().rect(rect, RADIUS_CONTROL, fill, stroke);
                        let lum = 0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32;
                        // Dark label on light swatches, white on dark ones.
                        let label = if lum > 140.0 { Palette::LIGHT.text } else { Color32::WHITE };
                        ui.painter().text(
                            rect.right_bottom() - vec2(3.0, 1.0),
                            Align2::RIGHT_BOTTOM,
                            (i + 1).to_string(),
                            FontId::monospace(9.0),
                            label,
                        );
                        let id = format!("style.color_{}", i + 1);
                        if resp.on_hover_text(self.tip(&format!("Warna {}", i + 1), &id)).clicked() {
                            self.run_command(ctx, &id, false);
                        }
                    } else if i == 9 {
                        let (rect, resp) = ui.allocate_exact_size(vec2(cell, 34.0), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(rect, RADIUS_CONTROL, pal.raised);
                        }
                        let b = rect.shrink(0.5);
                        let pts = [b.left_top(), b.right_top(), b.right_bottom(), b.left_bottom(), b.left_top()];
                        ui.painter().extend(EShape::dashed_line(&pts, Stroke::new(1.0_f32, pal.line_strong), 4.0, 3.0));
                        ui.painter().text(rect.center(), Align2::CENTER_CENTER, "+", FontId::proportional(16.0), pal.muted);
                        let pid = ui.make_persistent_id("custom_color");
                        if resp.clone().on_hover_text("Warna lain").clicked() {
                            ui.memory_mut(|m| m.toggle_popup(pid));
                        }
                        egui::popup::popup_below_widget(
                            ui,
                            pid,
                            &resp,
                            egui::PopupCloseBehavior::CloseOnClickOutside,
                            |ui| {
                                picker_changed = egui::color_picker::color_picker_color32(
                                    ui,
                                    &mut cur32,
                                    egui::color_picker::Alpha::OnlyBlend,
                                );
                            },
                        );
                    }
                }
            });
        }

        ui.add_space(SP_2);
        let (kind, unit) = self.width_kind();
        let mut width = self.s.width_now();
        ui.horizontal(|ui| {
            section_header(ui, &pal, kind);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = SP_1;
                let (up, down) = (self.keymap.hint("style.width_up"), self.keymap.hint("style.width_down"));
                if !up.is_empty() || !down.is_empty() {
                    ui.label(egui::RichText::new(format!("{down} {up}")).font(FontId::monospace(10.0)).color(pal.muted));
                }
                ui.label(egui::RichText::new(format!("{width:.0}{unit}")).size(TEXT_SMALL));
            });
        });
        let slider_changed = width_slider(ui, &pal, &mut width);

        if picker_changed || slider_changed {
            if !self.s.selection().is_empty() && !self.prop_editing {
                self.s.doc.begin_edit();
                self.prop_editing = true;
            }
            let [r, g, b, a] = cur32.to_srgba_unmultiplied();
            self.s.live_style(picker_changed.then_some([r, g, b, a]), slider_changed.then_some(width));
        }
        if ui.input(|i| !i.pointer.any_down()) {
            self.prop_editing = false;
        }

        ui.add_space(SP_2);
        ui.spacing_mut().item_spacing.y = SP_1;
        let mut fill = self.s.fill_now();
        let h = self.sc("style.toggle_fill");
        if toggle(ui, &pal, &mut fill, "Isi bentuk", h.as_deref()).changed() {
            self.s.apply_fill(fill);
        }
        let mut sh = self.s.shadow_now();
        let h = self.sc("style.toggle_shadow");
        if toggle(ui, &pal, &mut sh, "Bayangan", h.as_deref()).changed() {
            self.s.apply_shadow(sh);
        }
        ui.add_space(SP_2);
    }

    fn object_section(&mut self, ui: &mut egui::Ui, ctx: &Context) {
        let pal = self.pal;
        let has_sel = !self.s.selection().is_empty();
        ui.add_space(SP_2);
        section_header(ui, &pal, "Objek terpilih");
        ui.spacing_mut().item_spacing.y = SP_2;
        for (label, id) in [
            ("Duplikat", "edit.duplicate"),
            ("Ke depan", "edit.bring_front"),
            ("Ke belakang", "edit.send_back"),
            ("Hapus", "edit.delete"),
        ] {
            let h = self.keymap.hint(id);
            if list_button(ui, &pal, label, &h, has_sel).clicked() {
                self.run_command(ctx, id, false);
            }
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
                    if ui.button("Cancel (Esc)").clicked() {
                        self.confirm_close = false;
                    }
                });
            });
    }

    fn open_with_dialog(&mut self, ctx: &Context) {
        if !self.open_with_menu {
            return;
        }
        let apps: Vec<(String, ExternalApp)> = self.config.apps.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let mut picked: Option<(String, ExternalApp)> = None;
        let mut open = true;
        egui::Window::new("Open with")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                for (k, app) in &apps {
                    let name = app.name.clone().unwrap_or_else(|| k.clone());
                    if ui.button(name).clicked() {
                        picked = Some((k.clone(), app.clone()));
                    }
                }
            });
        if let Some((k, app)) = picked {
            self.launch_app(&k, &app);
            self.open_with_menu = false;
        }
        if !open {
            self.open_with_menu = false;
        }
    }

    fn open_settings(&mut self) {
        let r = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg("--settings").spawn());
        if let Err(e) = r {
            self.status = format!("Tidak bisa membuka Settings: {e}");
        }
    }

    /// Rows for the cheat-sheet from the *resolved* keymap, grouped into the
    /// design's four columns. Repetitive ids (colours 1-9, width up/down,
    /// nudges, front/back) collapse into one row like the mockup.
    fn cheat_columns(&self) -> Vec<(&'static str, Vec<(String, String)>)> {
        let km = &self.keymap;
        let none = |s: String| if s.is_empty() { "—".to_string() } else { s };
        let mut cols: Vec<(&'static str, Vec<(String, String)>)> =
            vec![("Tool", vec![]), ("Gaya & edit", vec![]), ("Tampilan", vec![]), ("File", vec![])];
        for def in EDITOR_SHORTCUTS {
            let (label, keys) = match def.id {
                "style.color_1" => ("Warna palet".to_string(), format!("{} – {}", km.hint("style.color_1"), km.hint("style.color_9"))),
                id if id.starts_with("style.color_") => continue,
                "style.width_up" => (
                    "Tebal / tipis".to_string(),
                    format!("{} / {}", km.hint("style.width_up"), km.hint("style.width_down")),
                ),
                "style.width_down" => continue,
                "edit.nudge_left" => (
                    "Geser (Shift ×10)".to_string(),
                    ["left", "up", "down", "right"].iter().map(|d| km.hint(&format!("edit.nudge_{d}"))).collect::<Vec<_>>().join(" "),
                ),
                id if id.starts_with("edit.nudge_") => continue,
                "edit.bring_front" => (
                    "Ke depan / belakang".to_string(),
                    format!("{} / {}", km.hint("edit.bring_front"), km.hint("edit.send_back")),
                ),
                "edit.send_back" => continue,
                id => (def.label.to_string(), none(km.hint_all(id))),
            };
            let col = match def.group {
                "Tool" => 0,
                "Gaya" | "Edit" => 1,
                "Tampilan" => 2,
                _ => 3,
            };
            cols[col].1.push((label, keys));
        }
        cols
    }

    fn cheatsheet(&mut self, ctx: &Context) {
        if !self.show_help {
            self.help_was_open = false;
            return;
        }
        let pal = self.pal;
        let first = !self.help_was_open;
        if first {
            self.help_query.clear();
        }
        self.help_was_open = true;
        let screen = ctx.screen_rect();

        // Scrim: dims the editor and swallows pointer input.
        let mut close = false;
        let scrim_clicked = egui::Area::new(egui::Id::new("help_scrim"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
                ui.painter().rect_filled(rect, 0.0, Color32::from_black_alpha(if pal.dark { 153 } else { 110 }));
                resp.clicked()
            })
            .inner;

        let width = (screen.width() - 48.0).clamp(360.0, 1040.0);
        let ncols = if width >= 900.0 { 4 } else if width >= 560.0 { 2 } else { 1 };
        let query = self.help_query.to_lowercase();
        let matches = |label: &str, keys: &str| {
            query.is_empty() || label.to_lowercase().contains(&query) || keys.to_lowercase().contains(&query)
        };
        let mut groups: Vec<(&str, Vec<(String, String)>)> = self
            .cheat_columns()
            .into_iter()
            .map(|(g, rows)| (g, rows.into_iter().filter(|(l, k)| matches(l, k)).collect()))
            .collect();
        groups.retain(|(_, r)| !r.is_empty());
        let gestures: Vec<&(&str, &str)> = GESTURES.iter().filter(|(k, v)| matches(k, v)).collect();

        let mut query_edit = std::mem::take(&mut self.help_query);
        let mut open_settings = false;
        let dialog = egui::Area::new(egui::Id::new("help_dialog"))
            .order(egui::Order::Foreground)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(pal.surface)
                    .stroke(Stroke::new(1.0_f32, pal.line))
                    .rounding(RADIUS_PANEL)
                    .shadow(egui::epaint::Shadow {
                        offset: vec2(0.0, 16.0),
                        blur: 48.0,
                        spread: 0.0,
                        color: Color32::from_black_alpha(if pal.dark { 128 } else { 60 }),
                    })
                    .inner_margin(egui::Margin::symmetric(28.0, 24.0))
                    .show(ui, |ui| {
                        ui.set_width(width - 56.0);
                        ui.spacing_mut().item_spacing = vec2(SP_3, 0.0);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Shortcut editor").size(18.0).strong());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let (rect, x) = ui.allocate_exact_size(Vec2::splat(CONTROL_H), Sense::click());
                                ui.painter().rect_filled(rect, RADIUS_CONTROL, if x.hovered() { pal.line } else { pal.raised });
                                icons::paint(ui.painter(), ERect::from_center_size(rect.center(), Vec2::splat(14.0)), Icon::Close, pal.text, 1.75);
                                if x.on_hover_text("Tutup (Esc)").clicked() {
                                    close = true;
                                }
                                if ui.link("Ubah di Settings").clicked() {
                                    open_settings = true;
                                }
                                // Search field.
                                let (rect, _) = ui.allocate_exact_size(vec2(260.0, CONTROL_H), Sense::hover());
                                ui.painter().rect(rect, RADIUS_CONTROL, pal.bg, Stroke::new(1.0_f32, pal.line));
                                icons::paint(
                                    ui.painter(),
                                    ERect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), Vec2::splat(14.0)),
                                    Icon::Search,
                                    pal.muted,
                                    2.0,
                                );
                                let inner = ERect::from_min_max(pos2(rect.left() + 32.0, rect.top()), rect.max - vec2(6.0, 0.0));
                                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
                                let te = egui::TextEdit::singleline(&mut query_edit)
                                    .frame(false)
                                    .hint_text("Cari aksi atau tombol…")
                                    .desired_width(inner.width())
                                    .margin(vec2(0.0, 6.0));
                                let r = child.add(te);
                                if first {
                                    r.request_focus();
                                }
                            });
                        });
                        ui.add_space(18.0);
                        egui::ScrollArea::vertical().max_height((screen.height() - 220.0).max(120.0)).show(ui, |ui| {
                            if groups.is_empty() {
                                ui.label(egui::RichText::new("Tidak ada yang cocok.").color(pal.muted));
                            }
                            for chunk in groups.chunks(ncols) {
                                ui.columns(ncols, |cols| {
                                    for (col, (title, rows)) in cols.iter_mut().zip(chunk) {
                                        col.spacing_mut().item_spacing.y = SP_1;
                                        col.label(
                                            egui::RichText::new(title.to_uppercase()).size(TEXT_SMALL).strong().color(pal.accent),
                                        );
                                        col.add_space(2.0);
                                        for (label, keys) in rows {
                                            col.allocate_ui_with_layout(
                                                vec2(col.available_width(), 26.0),
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    kbd(ui, &pal, keys);
                                                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                                        ui.add(egui::Label::new(label.as_str()).truncate());
                                                    });
                                                },
                                            );
                                        }
                                    }
                                });
                                ui.add_space(SP_3);
                            }
                        });
                        if !gestures.is_empty() {
                            ui.add_space(SP_2);
                            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                            ui.painter().rect_filled(rect, 0.0, pal.line);
                            ui.add_space(14.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = vec2(20.0, 6.0);
                                for (k, v) in gestures {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 0.0;
                                        ui.label(egui::RichText::new(*k).size(TEXT_SMALL).strong().color(pal.text));
                                        ui.label(egui::RichText::new(*v).size(TEXT_SMALL).color(pal.muted));
                                    });
                                }
                            });
                        }
                    });
            });
        self.help_query = query_edit;
        if open_settings {
            self.open_settings();
        }
        let outside = scrim_clicked
            && ctx.input(|i| i.pointer.interact_pos()).is_none_or(|p| !dialog.response.rect.contains(p));
        if close || outside {
            self.show_help = false;
        }
    }

    // ---- canvas ----------------------------------------------------------

    fn canvas(&mut self, ctx: &Context) {
        let pal = self.pal;
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(pal.canvas))
            .show(ctx, |ui| {
                if let Some(e) = &self.load_err {
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(pal.danger, format!("Cannot open image: {e}"));
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
                self.canvas_rect = rect.translate(-rect.min.to_vec2());

                if self.fit_pending && rect.width() > 10.0 {
                    let z = ((rect.width() - 24.0) / iw)
                        .min((rect.height() - 24.0) / ih)
                        .min(1.0 / ctx.pixels_per_point());
                    self.zoom = z.max(0.01);
                    self.pan = vec2((rect.width() - iw * self.zoom) / 2.0, (rect.height() - ih * self.zoom) / 2.0);
                    self.fit_pending = false;
                }

                // Ctrl+wheel zooms around the cursor; plain wheel scrolls.
                if resp.hovered() {
                    let zf = ui.input(|i| i.zoom_delta());
                    if (zf - 1.0).abs() > f32::EPSILON {
                        if let Some(h) = resp.hover_pos() {
                            let old = self.zoom;
                            let new = (old * zf).clamp(0.02, 64.0);
                            let local = h - rect.min;
                            self.pan = local - (local - self.pan) * (new / old);
                            self.zoom = new;
                        }
                    } else {
                        let sd = ui.input(|i| i.smooth_scroll_delta);
                        self.pan += sd;
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

                if !panning && !space {
                    self.interact(ctx, &resp, &to_img, &base);
                }

                painter.add(
                    egui::epaint::Shadow {
                        offset: vec2(0.0, 6.0),
                        blur: 24.0,
                        spread: 0.0,
                        color: Color32::from_black_alpha(if pal.dark { 110 } else { 41 }),
                    }
                    .as_shape(img_rect, 2.0),
                );
                painter.rect_filled(img_rect, 0.0, pal.surface);
                painter.image(tex.id(), img_rect, ERect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);

                self.effect_used.clear();
                let shapes: Vec<Shape> = self.s.doc.shapes().to_vec();
                for (i, sh) in shapes.iter().enumerate() {
                    let skip_effect = self.moving && self.s.is_selected(i);
                    self.paint_shape(ctx, &painter, &base, sh, &to_screen, zoom, skip_effect);
                }
                let used = std::mem::take(&mut self.effect_used);
                self.effect_cache.retain(|k, _| used.contains(k));

                if let Some(d) = self.drawing.clone() {
                    if let ShapeKind::Blur { a, b } | ShapeKind::Pixelate { a, b } = &d.kind {
                        let r = ERect::from_two_pos(to_screen(*a), to_screen(*b));
                        painter.rect_filled(r, 0.0, Color32::from_white_alpha(40));
                        painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, Color32::WHITE));
                    } else if self.s.tool == Tool::Crop {
                        if let ShapeKind::Rect { a, b } = &d.kind {
                            Self::paint_crop_overlay(&painter, img_rect, ERect::from_two_pos(to_screen(*a), to_screen(*b)));
                        }
                    } else {
                        self.paint_shape(ctx, &painter, &base, &d, &to_screen, zoom, true);
                    }
                } else if let Some(c) = self.s.doc.crop() {
                    let r = ERect::from_min_max(
                        to_screen(Pt::new(c.x as f32, c.y as f32)),
                        to_screen(Pt::new(c.right() as f32, c.bottom() as f32)),
                    );
                    Self::paint_crop_overlay(&painter, img_rect, r);
                }

                for i in self.s.selection() {
                    if let Some(sh) = shapes.get(*i) {
                        let (min, max) = sh.bounds();
                        let r = ERect::from_min_max(to_screen(min), to_screen(max)).expand(5.0);
                        let st = Stroke::new(1.5_f32, pal.accent);
                        let c = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
                        painter.extend(EShape::dashed_line(&c, st, 5.0, 3.0));
                        for h in &c[..4] {
                            let hr = ERect::from_center_size(*h, Vec2::splat(7.0));
                            painter.rect(hr, 0.0, Color32::WHITE, st);
                        }
                    }
                }

                if self.text_edit.is_some() {
                    self.text_overlay(ctx, &to_screen, zoom);
                }
            });
    }

    fn paint_crop_overlay(painter: &egui::Painter, full: ERect, crop: ERect) {
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
        let mods = ctx.input(|i| i.modifiers);

        if resp.drag_started_by(PointerButton::Primary) {
            if let Some(p) = pos {
                self.commit_text();
                self.begin_drag(p, tol);
            }
        }
        if resp.dragged_by(PointerButton::Primary) {
            if let Some(p) = pos {
                self.update_drag(p, resp.drag_delta() / self.zoom, mods);
            }
        }
        if resp.drag_stopped_by(PointerButton::Primary) {
            self.finish_drag(base);
        }
        if resp.clicked_by(PointerButton::Primary) {
            if let Some(p) = pos {
                match self.s.tool {
                    Tool::Select => {
                        self.commit_text();
                        let hit = self.s.doc.hit_test(p, tol);
                        self.s.select_one(hit);
                    }
                    Tool::Text => {
                        self.commit_text();
                        self.text_edit = Some(TextEdit { pos: p, text: String::new(), editing: None, focus_pending: true });
                    }
                    Tool::Step => {
                        let n = self.s.doc.next_step_number();
                        let sh = self.s.new_shape(ShapeKind::Step { center: p, n });
                        self.s.add_and_select(sh);
                    }
                    Tool::Eyedropper => {
                        let x = (p.x.floor().max(0.0) as u32).min(base.width.saturating_sub(1));
                        let y = (p.y.floor().max(0.0) as u32).min(base.height.saturating_sub(1));
                        let px = base.pixel(x, y);
                        self.s.pick_color([px[0], px[1], px[2], 255]);
                        self.status = format!("Picked #{:02x}{:02x}{:02x}", px[0], px[1], px[2]);
                    }
                    _ => {}
                }
            }
        }
        if resp.double_clicked_by(PointerButton::Primary) && self.s.tool == Tool::Select {
            if let Some(p) = pos {
                if let Some(i) = self.s.doc.hit_test(p, tol) {
                    if let ShapeKind::Text { pos, text } = &self.s.doc.shapes()[i].kind {
                        self.text_edit = Some(TextEdit { pos: *pos, text: text.clone(), editing: Some(i), focus_pending: true });
                    }
                }
            }
        }
    }

    fn begin_drag(&mut self, p: Pt, tol: f32) {
        self.moving = false;
        self.drag_start = p;
        let kind = match self.s.tool {
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
        if self.s.tool == Tool::Select {
            let hit = self.s.doc.hit_test(p, tol);
            match hit {
                Some(i) if self.s.is_selected(i) => {}
                other => self.s.select_one(other),
            }
        } else if let Some(kind) = kind {
            self.drawing = Some(self.s.new_shape(kind));
        }
    }

    fn update_drag(&mut self, p: Pt, delta: Vec2, mods: Modifiers) {
        let start = self.drag_start;
        if let Some(d) = &mut self.drawing {
            match &mut d.kind {
                ShapeKind::Arrow { a, b } | ShapeKind::Line { a, b } => {
                    (*a, *b) = constrain_drag(DragKind::Segment, start, p, mods.shift, mods.alt);
                }
                ShapeKind::Rect { a, b }
                | ShapeKind::Ellipse { a, b }
                | ShapeKind::Highlight { a, b }
                | ShapeKind::Blur { a, b }
                | ShapeKind::Pixelate { a, b } => {
                    (*a, *b) = constrain_drag(DragKind::Boxed, start, p, mods.shift, mods.alt);
                }
                ShapeKind::Pen { points } if points.last().is_none_or(|l| l.dist(p) >= 1.0) => {
                    points.push(p);
                }
                _ => {}
            }
        } else if self.s.tool == Tool::Select && !self.s.selection().is_empty() && delta != Vec2::ZERO {
            if !self.moving {
                self.s.doc.begin_edit();
                self.moving = true;
            }
            for i in self.s.selection().to_vec() {
                self.s.doc.translate_live(i, delta.x, delta.y);
            }
        }
    }

    fn finish_drag(&mut self, base: &Image) {
        self.moving = false;
        let Some(d) = self.drawing.take() else { return };
        if self.s.tool == Tool::Crop {
            if let ShapeKind::Rect { a, b } = d.kind {
                let x0 = a.x.min(b.x).round().max(0.0);
                let y0 = a.y.min(b.y).round().max(0.0);
                let x1 = a.x.max(b.x).round().min(base.width as f32);
                let y1 = a.y.max(b.y).round().min(base.height as f32);
                if x1 - x0 >= 4.0 && y1 - y0 >= 4.0 {
                    self.s.doc.set_crop(Some(Rect::new(x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32)));
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
            self.s.add_and_select(d);
        }
    }

    fn text_overlay(&mut self, ctx: &Context, to_screen: &dyn Fn(Pt) -> Pos2, zoom: f32) {
        let style = self.s.style_of(Tool::Text);
        let Some(te) = &mut self.text_edit else { return };
        let (color, px) = match te.editing.and_then(|i| self.s.doc.shapes().get(i)) {
            Some(s) => (s.color, s.text_px()),
            None => (style.color, (style.width * crate::doc::TEXT_PX_PER_WIDTH).max(6.0)),
        };
        let pos = to_screen(te.pos);
        let mut commit = false;
        egui::Area::new(egui::Id::new("text_edit")).fixed_pos(pos).order(egui::Order::Foreground).show(ctx, |ui| {
            let edit = egui::TextEdit::multiline(&mut te.text)
                .font(FontId::proportional((px * zoom).max(8.0)))
                .text_color(Color32::from_rgba_unmultiplied(color[0], color[1], color[2], 255))
                .desired_rows(1)
                .desired_width(160.0)
                .hint_text("Ctrl+Enter / click outside = done, Esc = cancel");
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
        if let Some(sh) = s.shadow_of() {
            self.paint_shape(ctx, painter, base, &sh, to_screen, zoom, skip_effect);
        }
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
                let r = ERect::from_two_pos(to_screen(*a), to_screen(*b));
                if s.fill {
                    painter.rect_filled(r.expand(w * zoom / 2.0), 0.0, c);
                } else {
                    painter.rect_stroke(r, 0.0, stroke);
                }
            }
            ShapeKind::Ellipse { a, b } => {
                let r = ERect::from_two_pos(to_screen(*a), to_screen(*b));
                let (c0, mut rad) = (r.center(), r.size() / 2.0);
                if s.fill {
                    rad += vec2(w * zoom / 2.0, w * zoom / 2.0);
                }
                let pts: Vec<Pos2> = (0..64)
                    .map(|k| {
                        let t = k as f32 / 64.0 * std::f32::consts::TAU;
                        pos2(c0.x + rad.x * t.cos(), c0.y + rad.y * t.sin())
                    })
                    .collect();
                if s.fill {
                    painter.add(EShape::convex_polygon(pts, c, Stroke::NONE));
                } else {
                    painter.add(EShape::closed_line(pts, stroke));
                }
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
                if let std::collections::hash_map::Entry::Vacant(e) = self.effect_cache.entry(key) {
                    if let Some((reg, img)) = render::effect_preview(base, s) {
                        e.insert((reg, make_texture(ctx, &img)));
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
                painter.text(to_screen(*pos), Align2::LEFT_TOP, text, FontId::proportional(s.text_px() * zoom), c);
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

/// Fixed (non-remappable) gestures shown under the cheat-sheet columns.
const GESTURES: &[(&str, &str)] = &[
    ("Shift", " saat menggambar = lurus / kotak / lingkaran"),
    ("Alt", " = dari tengah"),
    ("Space", "+drag = geser kanvas"),
    ("Ctrl", "+scroll = zoom"),
    ("Esc", " batal dulu, baru tutup"),
];

/// Full-width outlined row button: label left, shortcut right (design:
/// "Objek terpilih" actions). Local because `widgets::button` is content-sized.
fn list_button(ui: &mut egui::Ui, pal: &Palette, label: &str, shortcut: &str, enabled: bool) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), CONTROL_H), sense);
    let fill = if enabled && resp.hovered() { pal.raised } else { Color32::TRANSPARENT };
    let (fg, sc) = if enabled { (pal.text, pal.muted) } else { (pal.muted, pal.line_strong) };
    let p = ui.painter();
    p.rect(rect, RADIUS_CONTROL, fill, Stroke::new(1.0_f32, pal.line));
    p.text(pos2(rect.left() + 10.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(TEXT_BODY), fg);
    if !shortcut.is_empty() {
        p.text(pos2(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER, shortcut, FontId::monospace(10.0), sc);
    }
    resp
}

/// Thin accent slider (4 px rail, 16 px knob) over `MIN_WIDTH..=MAX_WIDTH`.
/// Returns true when the value changed.
fn width_slider(ui: &mut egui::Ui, pal: &Palette, value: &mut f32) -> bool {
    use crate::session::{MAX_WIDTH, MIN_WIDTH};
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::click_and_drag());
    let (l, r) = (rect.left() + 8.0, rect.right() - 8.0);
    let mut changed = false;
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.clicked() {
            let t = ((p.x - l) / (r - l).max(1.0)).clamp(0.0, 1.0);
            let v = MIN_WIDTH + t * (MAX_WIDTH - MIN_WIDTH);
            if (v - *value).abs() > f32::EPSILON {
                *value = v;
                changed = true;
            }
        }
    }
    let t = ((*value - MIN_WIDTH) / (MAX_WIDTH - MIN_WIDTH)).clamp(0.0, 1.0);
    let x = l + t * (r - l);
    let y = rect.center().y;
    let p = ui.painter();
    p.rect_filled(ERect::from_min_max(pos2(l, y - 2.0), pos2(r, y + 2.0)), 2.0, pal.line);
    p.rect_filled(ERect::from_min_max(pos2(l, y - 2.0), pos2(x, y + 2.0)), 2.0, pal.accent);
    p.circle(pos2(x, y), 8.0, pal.surface, Stroke::new(2.0_f32, pal.accent));
    changed
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

        // Buttons/sliders must not keep keyboard focus: Enter/Space/arrows
        // belong to the shortcut map (except while typing text).
        if self.text_edit.is_none() && !self.show_help {
            if let Some(id) = ctx.memory(|m| m.focused()) {
                ctx.memory_mut(|m| m.surrender_focus(id));
            }
        }

        if ctx.input(|i| i.viewport().close_requested()) && !self.force_close && self.s.doc.is_dirty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }

        self.handle_keys(ctx);
        self.toolbar(ctx);
        self.canvas(ctx);
        self.confirm_dialog(ctx);
        self.open_with_dialog(ctx);
        self.cheatsheet(ctx);

        let dirty = self.s.doc.is_dirty();
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
    let theme = config.general.theme;
    let mut app = EditorApp::new(path, config);
    eframe::run_native(
        "Jepret Editor",
        options,
        Box::new(move |cc| {
            app.pal = jepret_ui_theme::install(&cc.egui_ctx, theme);
            Ok(Box::new(app))
        }),
    )
        .map_err(|e| anyhow::anyhow!("editor window failed: {e}"))
}
