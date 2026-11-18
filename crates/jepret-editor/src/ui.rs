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
use jepret_ui_theme::widgets::{button, kbd, primary_button, toggle, IconButton};
use jepret_ui_theme::{icons, Icon, Palette};
use jepret_core::{Config, Image, Rect};

use crate::doc::{Handle, HandleKind, Pt, Shape, ShapeKind, Tool};
use crate::input::{combo_from_key, Keymap};
use crate::session::{self, constrain_drag, DragKind, Effect, Session, ZoomCmd};
use crate::{io, render, view};

type LoadResult = Result<Image, String>;

struct TextEdit {
    /// Shape being typed into (its own text is ignored; `text` is live).
    kind: ShapeKind,
    text: String,
    /// Index of the shape being edited (None = new text).
    editing: Option<usize>,
    focus_pending: bool,
}

/// Tool panel: 2 columns of `TOOL_BTN` buttons with symmetric padding.
const TOOL_GAP: f32 = 3.0;
const TOOL_PAD: f32 = 6.0;
const PANEL_W: f32 = TOOL_BTN * 2.0 + TOOL_GAP + TOOL_PAD * 2.0;
/// A resize gesture: handle index of the (single) selected shape, the
/// pointer-to-handle offset at grab time, and whether the undo point exists.
struct Resize {
    idx: usize,
    offset: Pt,
    began: bool,
}

/// Screen-space grab tolerance (points) around a resize handle.
const HANDLE_GRAB: f32 = 8.0;

/// A freshly typed (auto-width) callout gets its current width pinned so the
/// handles can resize it later; the height stays automatic.
fn pin_callout_width(sh: &mut Shape) {
    let px = sh.text_px();
    if let ShapeKind::Callout { tip, a, b, text } = &mut sh.kind {
        if (b.x - a.x).abs() < 1.0 {
            let l = render::callout_layout(*tip, *a, *b, text, px);
            let y = if (a.y + b.y) / 2.0 < tip.y { l.max.y } else { l.min.y };
            *a = Pt::new(l.min.x, y);
            *b = Pt::new(l.max.x, y);
        }
    }
}

const SCROLL_W: f32 = 8.0;

/// Where the scrollbars go this frame (screen points).
struct Overlays {
    view: ERect,
    h_track: Option<ERect>,
    v_track: Option<ERect>,
}

impl Overlays {
    fn layout(view: ERect, iw: f32, ih: f32, zoom: f32) -> Self {
        let sx = view::scrollable(view.width(), iw * zoom);
        let sy = view::scrollable(view.height(), ih * zoom);
        let m = 3.0;
        let h_track = sx.then(|| {
            let right = view.max.x - m - if sy { SCROLL_W + m } else { 0.0 };
            ERect::from_min_max(pos2(view.min.x + m, view.max.y - m - SCROLL_W), pos2(right, view.max.y - m))
        });
        let v_track = sy.then(|| {
            let bottom = view.max.y - m - if sx { SCROLL_W + m } else { 0.0 };
            ERect::from_min_max(pos2(view.max.x - m - SCROLL_W, view.min.y + m), pos2(view.max.x - m, bottom))
        });
        Self { view, h_track, v_track }
    }
}

fn handle_cursor(k: HandleKind) -> egui::CursorIcon {
    use egui::CursorIcon as C;
    match k {
        HandleKind::Nw | HandleKind::Se => C::ResizeNwSe,
        HandleKind::Ne | HandleKind::Sw => C::ResizeNeSw,
        HandleKind::N | HandleKind::S => C::ResizeVertical,
        HandleKind::E | HandleKind::W => C::ResizeHorizontal,
        HandleKind::Point | HandleKind::Tip => C::Crosshair,
    }
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
    /// Polygon tool: vertices placed so far (image px); not in the document until finished.
    poly_draft: Option<Vec<Pt>>,
    drag_start: Pt,
    moving: bool,
    /// Drag that grabbed an existing object while a drawing tool is active.
    grabbing: bool,
    /// Frames rendered (the real maximize is sent once the window shows).
    frames: u32,
    /// Handle-resize gesture in progress.
    resizing: Option<Resize>,
    prop_editing: bool,
    /// Sidebar panel open state: PROPERTI, OBJEK, RIWAYAT, NAVIGATOR (session only).
    panels_open: [bool; 4],
    /// Object under the pointer in the OBJEK list (highlighted on the canvas).
    hover_obj: Option<usize>,
    /// Doc index of the list row being dragged to reorder.
    obj_drag: Option<usize>,

    zoom: f32,
    pan: Vec2,
    fit_pending: bool,
    /// View right after the last fit; while unchanged, a resized window refits.
    fitted: Option<(f32, Vec2)>,
    /// Image region shown at the last frame; a change (crop applied / reset / undone) refits.
    last_vis: Option<(f32, f32, f32, f32)>,
    /// Last click in the OBJEK list (row, time, pointer) for double-click detection.
    list_click: Option<(usize, Instant, Pos2)>,
    /// (position, length) of the history at the last frame, to auto-scroll RIWAYAT.
    hist_seen: (usize, usize),
    canvas_rect: ERect,
    /// Pointer offset inside a scrollbar thumb while it is dragged.
    scroll_grab: f32,
    /// Crop before the current Crop-tool session (Esc restores it).
    crop_before: Option<Option<Rect>>,

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
    /// Rasterised text shapes: image-px top-left, image-px size, texture.
    text_cache: HashMap<u64, (Pt, Vec2, TextureHandle)>,
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
            poly_draft: None,
            drag_start: Pt::default(),
            moving: false,
            grabbing: false,
            frames: 0,
            resizing: None,
            prop_editing: false,
            panels_open: [true; 4],
            hover_obj: None,
            obj_drag: None,
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit_pending: true,
            fitted: None,
            last_vis: None,
            list_click: None,
            hist_seen: (usize::MAX, 0),
            canvas_rect: ERect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)),
            scroll_grab: 0.0,
            crop_before: None,
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
            text_cache: HashMap::new(),
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
                        if let Some(text) = sh.kind.text_mut() {
                            *text = te.text.clone();
                        }
                    });
                }
            }
            None => {
                if !te.text.trim().is_empty() {
                    let mut kind = te.kind;
                    if let Some(text) = kind.text_mut() {
                        *text = te.text;
                    }
                    let mut sh = self.s.new_shape(kind);
                    pin_callout_width(&mut sh);
                    self.s.add_and_select(sh);
                }
            }
        }
    }

    /// Start typing into existing text shape `i`.
    fn edit_text_shape(&mut self, i: usize) {
        let Some(sh) = self.s.doc.shapes().get(i) else { return };
        let Some(text) = sh.kind.text() else { return };
        self.text_edit = Some(TextEdit { kind: sh.kind.clone(), text: text.to_string(), editing: Some(i), focus_pending: true });
    }

    /// Text shape for a click (or a too-small drag) with a text tool at `p`.
    fn text_kind_at(&self, tool: Tool, p: Pt) -> Option<ShapeKind> {
        let px = (self.s.style_of(tool).width * crate::doc::TEXT_PX_PER_WIDTH).max(6.0);
        match tool {
            Tool::Text => Some(ShapeKind::Text { pos: p, text: String::new() }),
            Tool::TextBox => Some(ShapeKind::TextBox { a: p, b: Pt::new(p.x + px * 12.0, p.y), text: String::new() }),
            Tool::Callout => {
                let c = Pt::new(p.x + px * 1.5, p.y - px * 3.5);
                Some(ShapeKind::Callout { tip: p, a: c, b: c, text: String::new() })
            }
            _ => None,
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

    /// Turn the polygon draft into a shape (>= 3 distinct points) or drop it.
    fn finish_polygon(&mut self) {
        let Some(draft) = self.poly_draft.take() else { return };
        if let Some(points) = session::finalize_polygon(&draft) {
            let sh = self.s.new_shape(ShapeKind::Polygon { points });
            self.s.add_and_select(sh);
        }
    }

    /// Polygon tool click at `p`: add a vertex, or close on the first one.
    fn polygon_click(&mut self, p: Pt, shift: bool) {
        let radius = 8.0 / self.zoom;
        let draft = self.poly_draft.get_or_insert_with(Vec::new);
        if session::near_first_vertex(draft, p, radius) {
            self.finish_polygon();
        } else {
            let q = match draft.last() {
                Some(last) if shift => session::snap_45(*last, p),
                _ => p,
            };
            draft.push(q);
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
            if was == Tool::Polygon {
                self.finish_polygon();
            }
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
        // An active polygon draft owns Enter / Esc / Backspace.
        if self.poly_draft.is_some() && !self.confirm_close && !self.show_help {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
                self.finish_polygon();
            } else if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                self.poly_draft = None;
            } else if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Backspace)) {
                if let Some(d) = self.poly_draft.as_mut() {
                    d.pop();
                    if d.is_empty() {
                        self.poly_draft = None;
                    }
                }
            }
        }
        // Crop tool owns Enter (apply) / Esc (cancel) so they don't close the editor.
        if self.s.tool == Tool::Crop && self.drawing.is_none() && !self.confirm_close && !self.show_help {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
                self.crop_before = None;
                self.s.set_tool(Tool::Select);
                self.status = "Crop diterapkan".into();
            } else if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                if let Some(prev) = self.crop_before.take() {
                    self.s.doc.set_crop(prev);
                }
                self.s.set_tool(Tool::Select);
                self.status = "Crop dibatalkan".into();
            }
        }
        if self.s.tool != Tool::Crop {
            self.crop_before = None;
        }
        // Enter / F2 edits the single selected text object (instead of "Copy & tutup").
        if self.s.tool != Tool::Crop && self.poly_draft.is_none() && !self.confirm_close && !self.show_help {
            let target = match self.s.selection() {
                [i] if self.s.doc.shapes().get(*i).is_some_and(|s| s.kind.is_text()) => Some(*i),
                _ => None,
            };
            if let Some(i) = target {
                let hit = ctx.input_mut(|inp| {
                    inp.consume_key(Modifiers::NONE, Key::Enter) | inp.consume_key(Modifiers::NONE, Key::F2)
                });
                if hit {
                    self.edit_text_shape(i);
                }
            }
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
            (Tool::Text | Tool::TextBox | Tool::Callout | Tool::Step, _)
            | (
                _,
                Some(
                    ShapeKind::Text { .. }
                    | ShapeKind::TextBox { .. }
                    | ShapeKind::Callout { .. }
                    | ShapeKind::Step { .. },
                ),
            ) => ("Ukuran", ""),
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
            Tool::TextBox => format!("{name} · drag = lebar kotak, teks membungkus"),
            Tool::Polygon => format!(
                "{name} · klik titik-titik, klik titik awal/Enter = selesai, Backspace = hapus titik, Esc = batal"
            ),
            Tool::Callout => format!("{name} · drag dari titik yang ditunjuk ke posisi balon"),
            Tool::Crop => format!("{name} · drag area, Enter = terapkan, Esc = batal"),
            _ => name.to_string(),
        }
    }

    fn tool_panel(&mut self, ctx: &Context) -> ERect {
        let pal = self.pal;
        let mut picked: Option<Tool> = None;
        let r = egui::SidePanel::left("tools")
            .exact_width(PANEL_W)
            .resizable(false)
            .frame(egui::Frame::none().fill(pal.surface).inner_margin(egui::Margin { top: 10.0, ..Default::default() }))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(TOOL_GAP, TOOL_GAP);
                    let pairs: Vec<(Tool, Icon)> =
                        Tool::ALL.iter().copied().zip(Icon::TOOLS.iter().map(|(i, _)| *i)).collect();
                    for row in pairs.chunks(2) {
                        ui.horizontal(|ui| {
                            ui.add_space(TOOL_PAD);
                            for (t, icon) in row {
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

    /// Right sidebar: PROPERTI, OBJEK, NAVIGATOR (each collapsible). OBJEK takes
    /// the height the other two leave; the whole column scrolls as a fallback.
    fn props_panel(&mut self, ctx: &Context) -> ERect {
        let pal = self.pal;
        self.hover_obj = None;
        let r = egui::SidePanel::right("props")
            .exact_width(SIDE_W)
            .resizable(false)
            .frame(egui::Frame::none().fill(pal.surface))
            .show(ctx, |ui| {
                let total = ui.available_height();
                egui::ScrollArea::vertical().id_salt("side_scroll").auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    let top = ui.cursor().top();
                    self.properties_panel(ui, ctx, total * 0.6);
                    let used = ui.cursor().top() - top;
                    let nav_h = HEAD_H + 1.0 + if self.panels_open[3] { NAV_BODY_H } else { 0.0 };
                    let hist_h = HEAD_H + 1.0 + if self.panels_open[2] { HIST_BODY_H } else { 0.0 };
                    let obj_h = (total - used - nav_h - hist_h).max(OBJ_MIN_H);
                    self.objects_panel(ui, ctx, obj_h);
                    self.history_panel(ui);
                    self.navigator_panel(ui, ctx);
                });
            });
        r.response.rect
    }

    /// 1 px horizontal separator across the sidebar.
    fn side_sep(&self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().rect_filled(rect, 0.0, self.pal.line);
    }

    fn properties_panel(&mut self, ui: &mut egui::Ui, ctx: &Context, max_h: f32) {
        let pal = self.pal;
        let kind = match self.s.selection() {
            [] => editor_shortcut(self.s.style_tool().id()).map(|d| d.label.to_string()).unwrap_or_default(),
            [i] => self.s.doc.shapes().get(*i).map(|s| s.kind.display_name().to_string()).unwrap_or_default(),
            sel => format!("{} objek", sel.len()),
        };
        let (_, clicked) = panel_header(ui, &pal, self.panels_open[0], "Properti", &kind, 0.0);
        if clicked {
            self.panels_open[0] = !self.panels_open[0];
        }
        if !self.panels_open[0] {
            return;
        }
        egui::ScrollArea::vertical().id_salt("props_scroll").max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
            egui::Frame::none().inner_margin(egui::Margin { left: SP_3, right: SP_3, top: 2.0, bottom: SP_3 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.style_section(ui, ctx);
            });
        });
    }

    /// Colour swatches, width slider, fill/shadow/outline toggles.
    fn style_section(&mut self, ui: &mut egui::Ui, ctx: &Context) {
        let pal = self.pal;
        // Crop has no style; keep the panel quiet until something is selected.
        if self.s.tool == Tool::Crop && self.s.selection().is_empty() {
            ui.label(egui::RichText::new("Crop tidak punya properti").size(TEXT_SMALL).color(pal.muted));
            return;
        }
        let palette = self.s.palette.clone();
        let cur = self.s.color_now();
        let cell = ((ui.available_width() - 4.0 * SP_1) / 5.0).floor();
        let mut cur32 = Color32::from_rgba_unmultiplied(cur[0], cur[1], cur[2], cur[3]);
        let mut picker_changed = false;
        ui.spacing_mut().item_spacing = vec2(SP_1, SP_1);
        for row in 0..2 {
            ui.horizontal(|ui| {
                for col in 0..5 {
                    let i = row * 5 + col;
                    if i < 9 {
                        let Some(c) = palette.get(i) else { continue };
                        let fill = Color32::from_rgb(c[0], c[1], c[2]);
                        let (rect, resp) = ui.allocate_exact_size(vec2(cell, 28.0), Sense::click());
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
                        let (rect, resp) = ui.allocate_exact_size(vec2(cell, 28.0), Sense::click());
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
            ui.label(egui::RichText::new(kind).size(TEXT_SMALL).color(pal.muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = SP_1;
                ui.label(egui::RichText::new(format!("{width:.0}{unit}")).size(TEXT_SMALL));
                let (up, down) = (self.keymap.hint("style.width_up"), self.keymap.hint("style.width_down"));
                if !up.is_empty() || !down.is_empty() {
                    ui.label(egui::RichText::new(format!("{down} {up}")).font(FontId::monospace(10.0)).color(pal.muted));
                }
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
        let sel_text = self.s.selection().first().and_then(|i| self.s.doc.shapes().get(*i)).is_some_and(|s| s.kind.is_text());
        let tool_text = matches!(self.s.style_tool(), Tool::Text | Tool::TextBox | Tool::Callout);
        let show_outline = sel_text || (self.s.selection().is_empty() && tool_text);
        let (mut fill, mut sh, mut ol) = (self.s.fill_now(), self.s.shadow_now(), self.s.outline_now());
        let (h_fill, h_sh, h_ol) =
            (self.sc("style.toggle_fill"), self.sc("style.toggle_shadow"), self.sc("style.toggle_outline"));
        let (mut c_fill, mut c_sh, mut c_ol) = (false, false, false);
        ui.columns(2, |cols| {
            c_fill = toggle(&mut cols[0], &pal, &mut fill, "Isi", h_fill.as_deref()).changed();
            c_sh = toggle(&mut cols[1], &pal, &mut sh, "Bayangan", h_sh.as_deref()).changed();
            if show_outline {
                c_ol = toggle(&mut cols[0], &pal, &mut ol, "Garis tepi", h_ol.as_deref()).changed();
            }
        });
        if c_fill {
            self.s.apply_fill(fill);
        }
        if c_sh {
            self.s.apply_shadow(sh);
        }
        if c_ol {
            self.s.apply_outline(ol);
        }
    }

    /// OBJEK: header with order/duplicate/delete buttons, the z-ordered list
    /// (front-most first) and a hint footer. Occupies exactly `height`.
    fn objects_panel(&mut self, ui: &mut egui::Ui, ctx: &Context, height: f32) {
        let pal = self.pal;
        self.side_sep(ui);
        let w = ui.available_width();
        ui.allocate_ui_with_layout(vec2(w, height), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_min_height(height);
            let n = self.s.doc.shapes().len();
            let has_sel = !self.s.selection().is_empty();
            let title = format!("Objek ({n})");
            let (head, clicked) = panel_header(ui, &pal, self.panels_open[1], &title, "", 4.0 * 26.0 + SP_2);
            // Action buttons sit on top of the header (later widgets win the click).
            let mut pressed: Option<&'static str> = None;
            let acts: [(&str, &'static str); 4] = [
                ("Ke depan", "edit.bring_front"),
                ("Ke belakang", "edit.send_back"),
                ("Duplikat", "edit.duplicate"),
                ("Hapus", "edit.delete"),
            ];
            let mut x = head.right() - SP_2 - 24.0;
            for (k, &(label, id)) in acts.iter().enumerate().rev() {
                let r = ERect::from_min_size(pos2(x, head.center().y - 12.0), vec2(24.0, 24.0));
                x -= 26.0;
                let resp = ui.interact(r, ui.id().with(("obj_act", k)), Sense::click()).on_hover_text(self.tip(label, id));
                let hot = has_sel && resp.hovered();
                if hot {
                    ui.painter().rect_filled(r, RADIUS_CONTROL, pal.raised);
                }
                let col = if !has_sel {
                    pal.line_strong
                } else if hot {
                    pal.text
                } else {
                    pal.muted
                };
                let ir = ERect::from_center_size(r.center(), Vec2::splat(15.0));
                match k {
                    0 | 1 => paint_order_glyph(ui.painter(), ir, k == 0, col),
                    2 => icons::paint(ui.painter(), ir, Icon::Copy, col, 2.0),
                    _ => icons::paint(ui.painter(), ir, Icon::Trash, col, 2.0),
                }
                if has_sel && resp.clicked() {
                    pressed = Some(id);
                }
            }
            if clicked {
                self.panels_open[1] = !self.panels_open[1];
            }
            if let Some(id) = pressed {
                self.commit_text();
                self.run_command(ctx, id, false);
            }
            if !self.panels_open[1] {
                return;
            }
            self.side_sep(ui);
            let foot_h = 38.0;
            let list_h = (height - HEAD_H - 2.0 - foot_h).max(ROW_H);
            self.object_list(ui, ctx, list_h);
            self.side_sep(ui);
            egui::Frame::none().inner_margin(egui::Margin::symmetric(SP_3, 6.0)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new("Ctrl+klik pilih banyak \u{b7} drag = urutan \u{b7} dobel-klik / Enter = edit teks")
                        .size(11.0)
                        .color(pal.muted),
                );
            });
        });
    }

    fn object_list(&mut self, ui: &mut egui::Ui, ctx: &Context, list_h: f32) {
        let pal = self.pal;
        let n = self.s.doc.shapes().len();
        if n == 0 {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), list_h), Sense::hover());
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, "Belum ada objek", FontId::proportional(TEXT_SMALL), pal.muted);
            return;
        }
        let labels = crate::doc::object_labels(self.s.doc.shapes());
        let mods = ui.input(|i| i.modifiers);
        let mut rects: Vec<ERect> = Vec::with_capacity(n);
        let mut click: Option<usize> = None;
        let mut dbl: Option<usize> = None;
        let mut started: Option<usize> = None;
        let mut stopped: Option<usize> = None;
        let area = egui::ScrollArea::vertical().id_salt("obj_list").max_height(list_h).auto_shrink([false, false]);
        let out = area.show(ui, |ui| {
            for d in 0..n {
                let i = n - 1 - d;
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click_and_drag());
                rects.push(rect);
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let selected = self.s.is_selected(i);
                if resp.hovered() {
                    self.hover_obj = Some(i);
                }
                let p = ui.painter();
                if selected {
                    p.rect_filled(rect, 0.0, pal.accent_soft);
                } else if resp.hovered() {
                    p.rect_filled(rect, 0.0, pal.raised);
                }
                let cy = rect.center().y;
                // Drag grip: 2 x 3 dots.
                for cx in [rect.left() + 11.0, rect.left() + 15.0] {
                    for dy in [-4.0, 0.0, 4.0] {
                        p.circle_filled(pos2(cx, cy + dy), 1.0, pal.line_strong);
                    }
                }
                let sh = &self.s.doc.shapes()[i];
                let fg = if selected { pal.accent } else { pal.text };
                let icon = tool_icon(sh.kind.tool());
                icons::paint(p, ERect::from_center_size(pos2(rect.left() + 32.0, cy), Vec2::splat(16.0)), icon, fg, 2.0);
                let label_rect = ERect::from_min_max(pos2(rect.left() + 48.0, rect.top()), pos2(rect.right() - 34.0, rect.bottom()));
                p.with_clip_rect(label_rect).text(
                    pos2(label_rect.left(), cy),
                    Align2::LEFT_CENTER,
                    labels.get(i).map(String::as_str).unwrap_or(""),
                    FontId::proportional(TEXT_BODY),
                    fg,
                );
                let [r, g, b, _] = sh.color;
                p.rect(
                    ERect::from_center_size(pos2(rect.right() - 20.0, cy), Vec2::splat(14.0)),
                    3.0,
                    Color32::from_rgb(r, g, b),
                    Stroke::new(1.0_f32, pal.line_strong),
                );
                if resp.clicked() {
                    click = Some(i);
                }
                if resp.drag_started() {
                    started = Some(i);
                }
                if resp.drag_stopped() {
                    stopped = Some(i);
                }
            }
        });
        if let Some(i) = started {
            self.commit_text();
            self.obj_drag = Some(i);
            if !self.s.is_selected(i) {
                self.s.select_one(Some(i));
            }
        }
        let ptr = ctx.input(|i| i.pointer.latest_pos());
        // Insertion slot (0..=n, in list order) under the pointer.
        let slot = ptr.map(|p| rects.iter().filter(|r| r.center().y < p.y).count());
        if let (Some(_), Some(slot)) = (self.obj_drag, slot) {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            if stopped.is_none() {
                let view = out.inner_rect;
                let y = match rects.get(slot) {
                    Some(r) => r.top(),
                    None => rects.last().map(|r| r.bottom()).unwrap_or(view.top()),
                };
                if y >= view.top() - 1.0 && y <= view.bottom() + 1.0 {
                    let layer = ui.painter().with_clip_rect(view);
                    layer.rect_filled(ERect::from_min_max(pos2(view.left() + 6.0, y - 1.0), pos2(view.right() - 6.0, y + 1.0)), 1.0, pal.accent);
                    layer.circle_filled(pos2(view.left() + 6.0, y), 3.0, pal.accent);
                }
                // Scroll when dragging near the list edges.
                if let Some(p) = ptr {
                    let dy = if p.y < view.top() + 18.0 {
                        6.0
                    } else if p.y > view.bottom() - 18.0 {
                        -6.0
                    } else {
                        0.0
                    };
                    if dy != 0.0 {
                        ui.scroll_with_delta(vec2(0.0, dy));
                        ctx.request_repaint();
                    }
                }
            }
        }
        if let (Some(i), Some(slot)) = (stopped, slot) {
            // The list is drawn front-first, so list slot k maps to doc order.
            let to = session::drop_index(i, slot, n);
            self.s.move_object(i, to);
            self.obj_drag = None;
        } else if stopped.is_some() {
            self.obj_drag = None;
        }
        if let Some(i) = click {
            // Selecting can reflow the sidebar (PROPERTI changes height), moving
            // rows under the pointer between the two clicks, so egui's own
            // double-click is unreliable here: pair clicks by row, time and place.
            let now = Instant::now();
            let at = ptr.unwrap_or(Pos2::ZERO);
            let prev = self.list_click.take();
            match prev {
                Some((pi, t, pp)) if now.duration_since(t).as_millis() < 500 && pp.distance(at) < 8.0 => {
                    dbl = Some(pi);
                }
                _ => self.list_click = Some((i, now, at)),
            }
            self.commit_text();
            let sel = session::click_select(self.s.selection(), i, mods.ctrl || mods.command, mods.shift);
            self.s.set_selection(sel);
        }
        if let Some(i) = dbl {
            if self.s.doc.shapes().get(i).is_some_and(|s| s.kind.is_text()) {
                self.s.select_one(Some(i));
                self.edit_text_shape(i);
            }
        }
    }

    /// RIWAYAT: every document state (original, undo steps, then redo steps
    /// greyed out); clicking a row jumps straight to that state.
    fn history_panel(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        self.side_sep(ui);
        let pos = self.s.doc.history_pos();
        let len = self.s.doc.history_len();
        let (_, clicked) = panel_header(ui, &pal, self.panels_open[2], "Riwayat", &format!("{} langkah", len - 1), 0.0);
        if clicked {
            self.panels_open[2] = !self.panels_open[2];
        }
        if !self.panels_open[2] {
            return;
        }
        self.side_sep(ui);
        let labels = self.s.doc.history_labels();
        let follow = self.hist_seen != (pos, len);
        self.hist_seen = (pos, len);
        let mut jump: Option<usize> = None;
        egui::ScrollArea::vertical().id_salt("hist_list").max_height(HIST_BODY_H).auto_shrink([false, false]).show(ui, |ui| {
            for (i, label) in labels.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                if !ui.is_rect_visible(rect) && !(follow && i == pos) {
                    continue;
                }
                let current = i == pos;
                let future = i > pos;
                if current {
                    ui.painter().rect_filled(rect, 0.0, pal.accent_soft);
                    if follow {
                        ui.scroll_to_rect(rect, Some(egui::Align::Center));
                    }
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 0.0, pal.raised);
                }
                let fg = if current {
                    pal.accent
                } else if future {
                    pal.muted
                } else {
                    pal.text
                };
                let cy = rect.center().y;
                ui.painter().text(
                    pos2(rect.left() + SP_3, cy),
                    Align2::LEFT_CENTER,
                    format!("{i}"),
                    FontId::monospace(10.0),
                    pal.line_strong,
                );
                ui.painter().with_clip_rect(rect).text(
                    pos2(rect.left() + 36.0, cy),
                    Align2::LEFT_CENTER,
                    label,
                    FontId::proportional(TEXT_BODY),
                    fg,
                );
                if resp.clicked() && !current {
                    jump = Some(i);
                }
            }
        });
        if let Some(i) = jump {
            self.commit_text();
            self.s.jump_history(i);
            self.drawing = None;
        }
    }

    /// NAVIGATOR: thumbnail with the visible area; click/drag pans the view.
    fn navigator_panel(&mut self, ui: &mut egui::Ui, ctx: &Context) {
        let pal = self.pal;
        self.side_sep(ui);
        let zoom_txt = format!("{:.0}%", self.zoom * ctx.pixels_per_point() * 100.0);
        let (_, clicked) = panel_header(ui, &pal, self.panels_open[3], "Navigator", &zoom_txt, 0.0);
        if clicked {
            self.panels_open[3] = !self.panels_open[3];
        }
        if !self.panels_open[3] {
            return;
        }
        let w = ui.available_width();
        let (outer, _) = ui.allocate_exact_size(vec2(w, NAV_BODY_H), Sense::hover());
        let boxr = ERect::from_min_max(
            pos2(outer.left() + SP_3, outer.top() + 2.0),
            pos2(outer.right() - SP_3, outer.bottom() - SP_3),
        );
        ui.painter().rect(boxr, RADIUS_CONTROL, pal.canvas, Stroke::new(1.0_f32, pal.line));
        let (Some(base), Some(tex)) = (self.base.clone(), self.tex.clone()) else { return };
        let (full_w, full_h) = (base.width as f32, base.height as f32);
        let (vx, vy, iw, ih) = view::visible_rect(full_w, full_h, self.s.doc.crop(), self.s.tool == Tool::Crop);
        let (nw, nh) = view::navigator_size(iw, ih, boxr.width() - 16.0, boxr.height() - 16.0);
        if nw <= 0.0 {
            return;
        }
        let nav = ERect::from_center_size(boxr.center(), vec2(nw, nh));
        let painter = ui.painter();
        let uv = ERect::from_min_max(pos2(vx / full_w, vy / full_h), pos2((vx + iw) / full_w, (vy + ih) / full_h));
        painter.image(tex.id(), nav, uv, Color32::WHITE);
        let zoom = self.zoom;
        let view_size = self.canvas_rect.size();
        let k = nav.width() / iw.max(1.0) / zoom;
        let vis = ERect::from_min_size(nav.min + (-self.pan) * k, view_size * k).intersect(nav);
        painter.rect_filled(vis, 0.0, pal.accent.gamma_multiply(0.18));
        painter.rect_stroke(vis, 0.0, Stroke::new(1.5_f32, pal.accent));
        let r = ui.interact(boxr, egui::Id::new("jepret_navigator"), Sense::click_and_drag());
        if r.hovered() || r.dragged() {
            ctx.set_cursor_icon(egui::CursorIcon::Move);
        }
        if r.dragged() || r.clicked() || r.drag_started() {
            if let Some(p) = r.interact_pointer_pos() {
                let fx = ((p.x - nav.min.x) / nav.width()).clamp(0.0, 1.0);
                let fy = ((p.y - nav.min.y) / nav.height()).clamp(0.0, 1.0);
                self.pan = vec2(
                    view::pan_centering(fx * iw, view_size.x, zoom),
                    view::pan_centering(fy * ih, view_size.y, zoom),
                );
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
                // With a crop (and not in the Crop tool) only that region is the canvas content.
                let vis = view::visible_rect(iw, ih, self.s.doc.crop(), self.s.tool == Tool::Crop);
                let (vx, vy, vw, vh) = vis;
                if self.last_vis != Some(vis) {
                    self.last_vis = Some(vis);
                    self.fit_pending = true;
                }
                let avail = ui.available_size();
                let (resp, painter) = ui.allocate_painter(avail, Sense::click_and_drag());
                let rect = resp.rect;

                let resized = (rect.size() - self.canvas_rect.size()).length() > 0.5;
                if resized && self.fitted == Some((self.zoom, self.pan)) {
                    self.fit_pending = true;
                }
                self.canvas_rect = rect.translate(-rect.min.to_vec2());
                if self.fit_pending && rect.width() > 10.0 {
                    let z = ((rect.width() - 24.0) / vw)
                        .min((rect.height() - 24.0) / vh)
                        .min(1.0 / ctx.pixels_per_point());
                    self.zoom = z.max(0.01);
                    self.pan = vec2(((rect.width() - vw * self.zoom) / 2.0).round(), ((rect.height() - vh * self.zoom) / 2.0).round());
                    self.fitted = Some((self.zoom, self.pan));
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
                // Space (unless typing) turns the pointer into a hand, like Photoshop.
                let space = ui.input(|i| i.key_down(Key::Space)) && !ctx.wants_keyboard_input();
                let panning = resp.dragged_by(PointerButton::Middle) || (space && resp.dragged_by(PointerButton::Primary));
                if panning {
                    self.pan += resp.drag_delta();
                    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                } else if space && resp.hovered() {
                    ctx.set_cursor_icon(egui::CursorIcon::Grab);
                }

                // Scrollbars appear once the image outgrows the view.
                let ov = Overlays::layout(rect, vw, vh, self.zoom);
                let over_overlay = self.overlay_input(ui, &ov, vw, vh);
                let (cw, ch) = (vw * self.zoom, vh * self.zoom);
                self.pan = vec2(view::clamp_pan(self.pan.x, rect.width(), cw), view::clamp_pan(self.pan.y, rect.height(), ch));

                let zoom = self.zoom;
                // `pan` positions the visible region; image coordinates stay original.
                let origin = rect.min + self.pan - vec2(vx, vy) * zoom;
                let to_screen = |p: Pt| pos2(origin.x + p.x * zoom, origin.y + p.y * zoom);
                let to_img = |p: Pos2| Pt::new((p.x - origin.x) / zoom, (p.y - origin.y) / zoom);
                let img_rect = ERect::from_min_size(origin, vec2(iw * zoom, ih * zoom));
                let vis_rect = ERect::from_min_size(origin + vec2(vx, vy) * zoom, vec2(vw * zoom, vh * zoom));
                let cropped_view = vis != (0.0, 0.0, iw, ih);

                if !panning && !space && !over_overlay {
                    self.interact(ctx, &resp, &to_img, &base);
                }

                painter.add(
                    egui::epaint::Shadow {
                        offset: vec2(0.0, 6.0),
                        blur: 24.0,
                        spread: 0.0,
                        color: Color32::from_black_alpha(if pal.dark { 110 } else { 41 }),
                    }
                    .as_shape(vis_rect, 2.0),
                );
                let ui_painter = painter.clone();
                let painter = if cropped_view { painter.with_clip_rect(rect.intersect(vis_rect)) } else { painter };
                painter.rect_filled(img_rect, 0.0, pal.surface);
                painter.image(tex.id(), img_rect, ERect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);

                self.effect_used.clear();
                let shapes: Vec<Shape> = self.s.doc.shapes().to_vec();
                let editing = self.text_edit.as_ref().map(|te| (te.editing, te.text.clone()));
                for (i, sh) in shapes.iter().enumerate() {
                    if let Some((Some(ei), text)) = &editing {
                        if *ei == i {
                            let mut live = sh.clone();
                            if let Some(t) = live.kind.text_mut() {
                                t.clone_from(text);
                            }
                            self.paint_text_shape(ctx, &painter, &live, &to_screen, zoom, true);
                            continue;
                        }
                    }
                    let skip_effect = self.moving && self.s.is_selected(i);
                    self.paint_shape(ctx, &painter, &base, sh, &to_screen, zoom, skip_effect);
                }
                if let Some(live) = self.live_text_shape().filter(|_| matches!(&editing, Some((None, _)))) {
                    self.paint_text_shape(ctx, &painter, &live, &to_screen, zoom, true);
                }
                let used = std::mem::take(&mut self.effect_used);
                self.effect_cache.retain(|k, _| used.contains(k));
                self.text_cache.retain(|k, _| used.contains(k));

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
                } else if let (Some(c), Tool::Crop) = (self.s.doc.crop(), self.s.tool) {
                    let r = ERect::from_min_max(
                        to_screen(Pt::new(c.x as f32, c.y as f32)),
                        to_screen(Pt::new(c.right() as f32, c.bottom() as f32)),
                    );
                    Self::paint_crop_overlay(&painter, img_rect, r);
                }

                if let Some(draft) = self.poly_draft.clone().filter(|d| !d.is_empty()) {
                    let mut pts = draft.clone();
                    if let Some(h) = ctx.input(|i| i.pointer.hover_pos()).filter(|h| rect.contains(*h)) {
                        let q = to_img(h);
                        let last = *draft.last().unwrap_or(&q);
                        pts.push(if ui.input(|i| i.modifiers.shift) { session::snap_45(last, q) } else { q });
                    }
                    let mut sh = self.s.new_shape(ShapeKind::Pen { points: pts });
                    sh.fill = false;
                    self.paint_shape(ctx, &painter, &base, &sh, &to_screen, zoom, true);
                    for (k, p) in draft.iter().enumerate() {
                        let sp = to_screen(*p);
                        painter.circle(sp, if k == 0 { 5.0 } else { 3.0 }, Color32::WHITE, Stroke::new(1.5_f32, pal.accent));
                    }
                }

                if let Some(sh) = self.hover_obj.and_then(|i| shapes.get(i)) {
                    let (min, max) = sh.bounds();
                    let r = ERect::from_min_max(to_screen(min), to_screen(max)).expand(3.0);
                    painter.rect_filled(r, 2.0, pal.accent.gamma_multiply(0.12));
                    painter.rect_stroke(r, 2.0, Stroke::new(1.5_f32, pal.accent));
                }

                let with_handles = !self.single_handles().is_empty();
                for i in self.s.selection() {
                    if let Some(sh) = shapes.get(*i) {
                        let (min, max) = sh.bounds();
                        let r = ERect::from_min_max(to_screen(min), to_screen(max)).expand(5.0);
                        let st = Stroke::new(1.5_f32, pal.accent);
                        let c = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
                        painter.extend(EShape::dashed_line(&c, st, 5.0, 3.0));
                        if with_handles {
                            for h in sh.handles() {
                                let c = to_screen(h.pos);
                                if h.kind == HandleKind::Tip {
                                    painter.circle(c, 5.5, Color32::from_rgb(255, 150, 0), Stroke::new(1.5_f32, Color32::WHITE));
                                } else {
                                    painter.rect(ERect::from_center_size(c, Vec2::splat(7.0)), 0.0, Color32::WHITE, st);
                                }
                            }
                        }
                    }
                }

                self.paint_overlays(&ui_painter, &ov, vw, vh);

                if self.text_edit.is_some() {
                    self.text_overlay(ctx, &to_screen, zoom);
                }
            });
    }

    /// Scrollbar / navigator interaction; returns true while the pointer
    /// is on (or dragging) one of them so the canvas tools stay idle.
    fn overlay_input(&mut self, ui: &egui::Ui, ov: &Overlays, iw: f32, ih: f32) -> bool {
        let view = ov.view;
        let zoom = self.zoom;
        let mut busy = false;
        for (axis, track) in [(0usize, ov.h_track), (1, ov.v_track)] {
            let Some(track) = track else { continue };
            let r = ui.interact(track, egui::Id::new(("jepret_scroll", axis)), Sense::click_and_drag());
            busy |= r.hovered() || r.dragged();
            let (t0, tl, vlen, clen, pan) = if axis == 0 {
                (track.min.x, track.width(), view.width(), iw * zoom, self.pan.x)
            } else {
                (track.min.y, track.height(), view.height(), ih * zoom, self.pan.y)
            };
            let Some(p) = r.interact_pointer_pos() else { continue };
            let at = if axis == 0 { p.x } else { p.y } - t0;
            let (start, len) = view::thumb(tl, vlen, clen, pan);
            if r.drag_started() || r.clicked() {
                // Grab the thumb where it was hit; a track click centres it.
                self.scroll_grab = if (start..=start + len).contains(&at) { at - start } else { len / 2.0 };
            }
            if r.dragged() || r.clicked() || r.drag_started() {
                let np = view::pan_for_thumb(tl, vlen, clen, at - self.scroll_grab);
                if axis == 0 {
                    self.pan.x = np;
                } else {
                    self.pan.y = np;
                }
            }
        }
        busy
    }

    fn paint_overlays(&self, painter: &egui::Painter, ov: &Overlays, iw: f32, ih: f32) {
        let pal = self.pal;
        let view = ov.view;
        let (cw, ch) = (iw * self.zoom, ih * self.zoom);
        let track_c = Color32::from_black_alpha(if pal.dark { 70 } else { 22 });
        let thumb_c = if pal.dark { Color32::from_white_alpha(110) } else { Color32::from_black_alpha(100) };
        if let Some(t) = ov.h_track {
            painter.rect_filled(t, SCROLL_W / 2.0, track_c);
            let (s, l) = view::thumb(t.width(), view.width(), cw, self.pan.x);
            painter.rect_filled(ERect::from_min_size(pos2(t.min.x + s, t.min.y), vec2(l, t.height())), SCROLL_W / 2.0, thumb_c);
        }
        if let Some(t) = ov.v_track {
            painter.rect_filled(t, SCROLL_W / 2.0, track_c);
            let (s, l) = view::thumb(t.height(), view.height(), ch, self.pan.y);
            painter.rect_filled(ERect::from_min_size(pos2(t.min.x, t.min.y + s), vec2(t.width(), l)), SCROLL_W / 2.0, thumb_c);
        }
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

        if let Some(r) = &self.resizing {
            let kind = self.single_handles().get(r.idx).map(|h| h.kind);
            if let Some(k) = kind {
                ctx.set_cursor_icon(handle_cursor(k));
            }
        } else if resp.hovered() && !self.moving {
            if let Some(p) = resp.hover_pos().map(to_img) {
                if let Some((_, k)) = self.handle_at(p) {
                    ctx.set_cursor_icon(handle_cursor(k));
                } else if self.grab_hit(p, tol, mods.command).is_some() {
                    ctx.set_cursor_icon(egui::CursorIcon::Move);
                }
            }
        }

        if resp.drag_started_by(PointerButton::Primary) {
            // egui reports a drag only after the pointer moved past a
            // threshold; hit-test handles at the press point, not there.
            let origin = ctx.input(|i| i.pointer.press_origin()).map(to_img);
            if let Some(p) = origin.or(pos) {
                self.commit_text();
                self.begin_drag(p, tol, mods.command);
            }
        }
        if resp.dragged_by(PointerButton::Primary) {
            if let Some(p) = pos {
                self.update_drag(p, resp.drag_delta() / self.zoom, mods);
            }
        }
        if resp.drag_stopped_by(PointerButton::Primary) {
            let grabbed = self.grabbing;
            self.finish_drag(base);
            // A click that wobbled into a tiny drag still places a vertex.
            if self.s.tool == Tool::Polygon && !grabbed {
                if let Some(p) = pos {
                    self.commit_text();
                    self.polygon_click(p, mods.shift);
                }
            }
        }
        if resp.clicked_by(PointerButton::Primary) {
            // Double-click on a text object edits it with any tool that can grab
            // objects. This must come first: the second click of the pair would
            // otherwise be consumed (select + return) by the branch below.
            if resp.double_clicked_by(PointerButton::Primary) {
                let hit = pos
                    .and_then(|p| self.grab_hit(p, tol, mods.command))
                    .filter(|i| self.s.doc.shapes().get(*i).is_some_and(|s| s.kind.is_text()));
                if let Some(i) = hit {
                    self.commit_text();
                    self.s.select_one(Some(i));
                    self.edit_text_shape(i);
                    return;
                }
            }
            if let Some(i) = pos.and_then(|p| self.grab_hit(p, tol, mods.command)) {
                if !self.s.tool.is_text_tool() {
                    self.commit_text();
                    self.s.select_one(Some(i));
                    return;
                }
            }
            if let Some(p) = pos {
                match self.s.tool {
                    Tool::Select => {
                        self.commit_text();
                        let hit = self.visible_hit(p, tol);
                        self.s.select_one(hit);
                    }
                    Tool::Text | Tool::TextBox | Tool::Callout => {
                        self.commit_text();
                        let tool = self.s.tool;
                        let hit = self.visible_hit(p, tol).filter(|i| self.s.doc.shapes()[*i].kind.is_text());
                        if let Some(i) = hit {
                            self.edit_text_shape(i);
                        } else if let Some(kind) = self.text_kind_at(tool, p) {
                            self.text_edit = Some(TextEdit { kind, text: String::new(), editing: None, focus_pending: true });
                        }
                    }
                    Tool::Polygon => {
                        self.commit_text();
                        if resp.double_clicked_by(PointerButton::Primary) {
                            // the first click of the pair already added the vertex
                            self.finish_polygon();
                        } else {
                            self.polygon_click(p, mods.shift);
                        }
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
    }

    /// `hit_test` limited to what is visible: with a crop applied, points
    /// outside the cropped region and shapes entirely outside it never hit.
    fn visible_hit(&self, p: Pt, tol: f32) -> Option<usize> {
        let hit = self.s.doc.hit_test(p, tol)?;
        let Some(base) = &self.base else { return Some(hit) };
        let (vx, vy, vw, vh) = view::visible_rect(base.width as f32, base.height as f32, self.s.doc.crop(), self.s.tool == Tool::Crop);
        if p.x < vx || p.y < vy || p.x > vx + vw || p.y > vy + vh {
            return None;
        }
        let (min, max) = self.s.doc.shapes().get(hit)?.bounds();
        (max.x >= vx && max.y >= vy && min.x <= vx + vw && min.y <= vy + vh).then_some(hit)
    }

    /// Drawing tools can grab existing objects (ShareX style): pressing on
    /// one moves/selects it, pressing on empty canvas draws. Ctrl forces
    /// drawing. Returns the object under `p` when it may be grabbed.
    fn grab_hit(&self, p: Pt, tol: f32, force_draw: bool) -> Option<usize> {
        let tool = self.s.tool;
        let grabs = match tool {
            Tool::Select => true,
            Tool::Crop | Tool::Eyedropper => false,
            Tool::Polygon => self.poly_draft.is_none(),
            _ => !force_draw,
        };
        if !grabs || self.drawing.is_some() {
            return None;
        }
        self.visible_hit(p, tol)
    }

    /// Handles of the selected shape, when exactly one is selected and the
    /// tool can grab objects (empty otherwise).
    fn single_handles(&self) -> Vec<Handle> {
        let tool_ok = !matches!(self.s.tool, Tool::Crop | Tool::Eyedropper) && self.poly_draft.is_none();
        match self.s.selection() {
            [i] if tool_ok => {
                self.s.doc.shapes().get(*i).map(Shape::handles).unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    /// Nearest resize handle of the selected shape within the grab radius.
    fn handle_at(&self, p: Pt) -> Option<(usize, HandleKind)> {
        let tol = HANDLE_GRAB / self.zoom.max(0.01);
        self.single_handles()
            .iter()
            .enumerate()
            .map(|(i, h)| (i, h.kind, h.pos.dist(p)))
            .filter(|(_, _, d)| *d <= tol)
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(i, k, _)| (i, k))
    }

    fn begin_drag(&mut self, p: Pt, tol: f32, force_draw: bool) {
        self.moving = false;
        self.grabbing = false;
        self.resizing = None;
        self.drag_start = p;
        if let Some((idx, _)) = self.handle_at(p) {
            let offset = self.single_handles().get(idx).map(|h| Pt::new(h.pos.x - p.x, h.pos.y - p.y)).unwrap_or_default();
            self.resizing = Some(Resize { idx, offset, began: false });
            return;
        }
        if self.s.tool != Tool::Select {
            if let Some(i) = self.grab_hit(p, tol, force_draw) {
                if !self.s.is_selected(i) {
                    self.s.select_one(Some(i));
                }
                self.grabbing = true;
                return;
            }
        }
        let kind = match self.s.tool {
            Tool::Arrow => Some(ShapeKind::Arrow { a: p, b: p }),
            Tool::Line => Some(ShapeKind::Line { a: p, b: p }),
            Tool::Rect | Tool::Crop => Some(ShapeKind::Rect { a: p, b: p }),
            Tool::Ellipse => Some(ShapeKind::Ellipse { a: p, b: p }),
            Tool::Highlight => Some(ShapeKind::Highlight { a: p, b: p }),
            Tool::Blur => Some(ShapeKind::Blur { a: p, b: p }),
            Tool::Pixelate => Some(ShapeKind::Pixelate { a: p, b: p }),
            Tool::Pen => Some(ShapeKind::Pen { points: vec![p] }),
            Tool::TextBox => Some(ShapeKind::TextBox { a: p, b: p, text: String::new() }),
            Tool::Callout => Some(ShapeKind::Callout { tip: p, a: p, b: p, text: String::new() }),
            _ => None,
        };
        if self.s.tool == Tool::Select {
            let hit = self.visible_hit(p, tol);
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
        if let Some(r) = &mut self.resizing {
            let Some(&i) = self.s.selection().first() else { return };
            if !r.began {
                self.s.doc.begin_edit();
                r.began = true;
            }
            let target = Pt::new(p.x + r.offset.x, p.y + r.offset.y);
            if let Some(sh) = self.s.doc.shape_live(i) {
                r.idx = sh.drag_handle(r.idx, target, mods.shift);
                self.s.doc.touch();
            }
            return;
        }
        if let Some(d) = &mut self.drawing {
            match &mut d.kind {
                ShapeKind::Arrow { a, b } | ShapeKind::Line { a, b } => {
                    (*a, *b) = constrain_drag(DragKind::Segment, start, p, mods.shift, mods.alt);
                }
                ShapeKind::Rect { a, b }
                | ShapeKind::Ellipse { a, b }
                | ShapeKind::Highlight { a, b }
                | ShapeKind::Blur { a, b }
                | ShapeKind::Pixelate { a, b }
                | ShapeKind::TextBox { a, b, .. } => {
                    (*a, *b) = constrain_drag(DragKind::Boxed, start, p, mods.shift, mods.alt);
                }
                // The drag end is the bubble corner nearest the tip; the
                // bubble grows away from the tip (auto width until resized).
                ShapeKind::Callout { a, b, .. } => {
                    *a = p;
                    *b = p;
                }
                ShapeKind::Pen { points } if points.last().is_none_or(|l| l.dist(p) >= 1.0) => {
                    points.push(p);
                }
                _ => {}
            }
        } else if (self.s.tool == Tool::Select || self.grabbing) && !self.s.selection().is_empty() && delta != Vec2::ZERO {
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
        self.grabbing = false;
        if self.resizing.take().is_some() {
            return;
        }
        let Some(d) = self.drawing.take() else { return };
        if self.s.tool == Tool::Crop {
            if let ShapeKind::Rect { a, b } = d.kind {
                let x0 = a.x.min(b.x).round().max(0.0);
                let y0 = a.y.min(b.y).round().max(0.0);
                let x1 = a.x.max(b.x).round().min(base.width as f32);
                let y1 = a.y.max(b.y).round().min(base.height as f32);
                if x1 - x0 >= 4.0 && y1 - y0 >= 4.0 {
                    if self.crop_before.is_none() {
                        self.crop_before = Some(self.s.doc.crop());
                    }
                    self.s.doc.set_crop(Some(Rect::new(x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32)));
                }
            }
            return;
        }
        // Text box / callout: the drag only places it; now type the text.
        if let ShapeKind::TextBox { a, b, .. } | ShapeKind::Callout { tip: a, b, .. } = &d.kind {
            let kind = if a.dist(*b) < 8.0 { self.text_kind_at(self.s.tool, *a) } else { Some(d.kind.clone()) };
            if let Some(kind) = kind {
                self.text_edit = Some(TextEdit { kind, text: String::new(), editing: None, focus_pending: true });
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

    /// The shape being typed, with the live text (style from the shape being
    /// edited, else from the current tool).
    fn live_text_shape(&self) -> Option<Shape> {
        let te = self.text_edit.as_ref()?;
        let mut sh = match te.editing.and_then(|i| self.s.doc.shapes().get(i)) {
            Some(s) => s.clone(),
            None => self.s.new_shape(te.kind.clone()),
        };
        if let Some(t) = sh.kind.text_mut() {
            t.clone_from(&te.text);
        }
        Some(sh)
    }

    fn text_overlay(&mut self, ctx: &Context, to_screen: &dyn Fn(Pt) -> Pos2, zoom: f32) {
        let Some(live) = self.live_text_shape() else { return };
        let px = live.text_px();
        let c = render::text_color_of(&live);
        let color = Color32::from_rgba_unmultiplied(c[0], c[1], c[2], 255);
        // (text top-left, wrap width in screen points, framed field?)
        let (origin, width, framed) = match &live.kind {
            ShapeKind::TextBox { a, b, text } => {
                let l = render::textbox_layout(*a, *b, text, px);
                (l.text_pos, l.text_w * zoom, false)
            }
            ShapeKind::Callout { tip, a, b, text } => {
                let l = render::callout_layout(*tip, *a, *b, text, px);
                // auto-width bubbles get room to type into; fixed ones wrap
                let slack = if (b.x - a.x).abs() < 1.0 { px } else { 0.0 };
                (l.text_pos, (l.text_w + slack) * zoom, false)
            }
            ShapeKind::Text { pos, .. } => (*pos, 160.0, true),
            _ => return,
        };
        let Some(te) = &mut self.text_edit else { return };
        let mut commit = false;
        egui::Area::new(egui::Id::new("text_edit")).fixed_pos(to_screen(origin)).order(egui::Order::Foreground).show(ctx, |ui| {
            // Same glyph size, line spacing and (nearly) wrap width as the
            // CPU renderer, so the box grows exactly where the text wraps.
            let size = (px * zoom * render::egui_font_factor()).max(6.0);
            let row_h = render::line_height(px) * zoom;
            let mut layouter = |ui: &egui::Ui, s: &str, wrap: f32| {
                let mut job = egui::text::LayoutJob::single_section(
                    s.to_owned(),
                    egui::TextFormat { font_id: FontId::proportional(size), color, line_height: Some(row_h), ..Default::default() },
                );
                job.wrap.max_width = wrap;
                ui.fonts(|f| f.layout_job(job))
            };
            let mut edit = egui::TextEdit::multiline(&mut te.text)
                .font(FontId::proportional(size))
                .text_color(color)
                .layouter(&mut layouter)
                .desired_rows(1)
                .desired_width(width + 0.5);
            edit = if framed {
                edit.hint_text("Ctrl+Enter / click outside = done, Esc = cancel")
            } else {
                edit.frame(false).margin(egui::Margin::ZERO).hint_text("Ketik teks…")
            };
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

    /// Text / text box / callout via the CPU renderer (exactly what gets
    /// saved), cached as a texture per shape state and zoom bucket.
    fn paint_text_shape(
        &mut self,
        ctx: &Context,
        painter: &egui::Painter,
        s: &Shape,
        to_screen: &dyn Fn(Pt) -> Pos2,
        zoom: f32,
        hide_text: bool,
    ) {
        let scale = ((zoom * ctx.pixels_per_point()).clamp(1.0, 4.0) * 2.0).round() / 2.0;
        let key = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            format!("{s:?}").hash(&mut h);
            scale.to_bits().hash(&mut h);
            hide_text.hash(&mut h);
            h.finish()
        };
        self.effect_used.insert(key);
        if let std::collections::hash_map::Entry::Vacant(e) = self.text_cache.entry(key) {
            if let Some((min, _max, img)) = render::text_shape_preview(s, scale, hide_text) {
                let size = vec2(img.width as f32 / scale, img.height as f32 / scale);
                e.insert((min, size, make_texture(ctx, &img)));
            }
        }
        if let Some((min, size, tex)) = self.text_cache.get(&key) {
            let r = ERect::from_min_max(to_screen(*min), to_screen(Pt::new(min.x + size.x, min.y + size.y)));
            painter.image(tex.id(), r, ERect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
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
        if s.kind.is_text() {
            // Shadow and halo are part of the rasterised text.
            self.paint_text_shape(ctx, painter, s, to_screen, zoom, false);
            return;
        }
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
            ShapeKind::Text { .. } | ShapeKind::TextBox { .. } | ShapeKind::Callout { .. } => {}
            ShapeKind::Polygon { points } => {
                let pts: Vec<Pos2> = points.iter().map(|p| to_screen(*p)).collect();
                if pts.len() >= 3 {
                    if s.fill {
                        let mut mesh = egui::Mesh::default();
                        for p in &pts {
                            mesh.colored_vertex(*p, c);
                        }
                        for t in jepret_ui_theme::icons::triangulate(&pts) {
                            mesh.add_triangle(t[0] as u32, t[1] as u32, t[2] as u32);
                        }
                        painter.add(EShape::mesh(mesh));
                    } else {
                        painter.add(EShape::closed_line(pts, stroke));
                    }
                } else if pts.len() == 2 {
                    painter.line_segment([pts[0], pts[1]], stroke);
                }
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
    ("Ctrl", "+drag di atas objek = tetap menggambar"),
    ("Dobel-klik", " / Enter / F2 pada teks = edit teks"),
    ("Esc", " batal dulu, baru tutup"),
];

const SIDE_W: f32 = 272.0;
const HEAD_H: f32 = 34.0;
const ROW_H: f32 = 30.0;
/// Fixed height of the RIWAYAT list.
const HIST_BODY_H: f32 = 160.0;
const NAV_BODY_H: f32 = 2.0 + 136.0 + SP_3;
/// Smallest OBJEK region (header + a couple of rows + footer) before the
/// sidebar falls back to scrolling.
const OBJ_MIN_H: f32 = HEAD_H + 2.0 * ROW_H + 40.0;

/// Collapsible panel header: chevron, upper-case muted title, optional
/// right-hand text (kept clear of `reserve_right` px for extra buttons).
/// Returns the header rect and whether it was clicked.
fn panel_header(ui: &mut egui::Ui, pal: &Palette, open: bool, title: &str, right: &str, reserve_right: f32) -> (ERect, bool) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), HEAD_H), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 0.0, pal.raised.gamma_multiply(0.5));
    }
    let c = pos2(rect.left() + 18.0, rect.center().y);
    let st = Stroke::new(1.5_f32, pal.muted);
    if open {
        p.line_segment([c + vec2(-4.0, -2.0), c + vec2(0.0, 2.0)], st);
        p.line_segment([c + vec2(0.0, 2.0), c + vec2(4.0, -2.0)], st);
    } else {
        p.line_segment([c + vec2(-2.0, -4.0), c + vec2(2.0, 0.0)], st);
        p.line_segment([c + vec2(2.0, 0.0), c + vec2(-2.0, 4.0)], st);
    }
    p.text(
        pos2(rect.left() + 32.0, rect.center().y),
        Align2::LEFT_CENTER,
        title.to_uppercase(),
        FontId::proportional(TEXT_SMALL),
        pal.muted,
    );
    if !right.is_empty() {
        p.text(
            pos2(rect.right() - 12.0 - reserve_right, rect.center().y),
            Align2::RIGHT_CENTER,
            right,
            FontId::proportional(TEXT_SMALL),
            pal.text,
        );
    }
    (rect, resp.clicked())
}

/// Icon of the tool that creates shapes of kind `t`.
fn tool_icon(t: Tool) -> Icon {
    Tool::ALL
        .iter()
        .position(|x| *x == t)
        .and_then(|i| Icon::TOOLS.get(i))
        .map(|(icon, _)| *icon)
        .unwrap_or(Icon::Select)
}

/// "Ke depan" / "Ke belakang" glyph: a bar with an arrow pointing at it.
fn paint_order_glyph(p: &egui::Painter, r: ERect, front: bool, col: Color32) {
    let c = r.center();
    let f = if front { -1.0 } else { 1.0 };
    let st = Stroke::new(1.5_f32, col);
    p.line_segment([c + vec2(-5.0, f * 6.0), c + vec2(5.0, f * 6.0)], st);
    p.line_segment([c + vec2(0.0, -f * 6.0), c + vec2(0.0, f * 3.0)], st);
    p.line_segment([c + vec2(-4.0, -f), c + vec2(0.0, f * 3.0)], st);
    p.line_segment([c + vec2(4.0, -f), c + vec2(0.0, f * 3.0)], st);
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
        // eframe shows the window after the first frame; a maximize set on
        // the builder is lost on Windows, so ask once it is visible.
        if self.frames < 3 {
            self.frames += 1;
            if self.frames == 2 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            }
            ctx.request_repaint();
        }

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
            .with_icon(std::sync::Arc::new(jepret_ui_theme::app_icon()))
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
