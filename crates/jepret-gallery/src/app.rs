//! egui front-end: toolbar, grouped virtual grid, actions, thumbnail workers.
//!
//! Look = `jepret-ui-theme` (artboard "Galeri riwayat"). Layout is virtual in
//! both axes of cost: only visible rows touch the DB (`PagedList`) and the
//! decoder threads; day-group boundaries come from a light background scan
//! that keeps only `(day, count)` runs, never the entries themselves.

use std::{
    collections::{HashSet, VecDeque},
    path::Path,
    process::Command,
    sync::{mpsc, Arc, Condvar, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use chrono::{Local, TimeZone};
use egui::{
    pos2, vec2, Align, Align2, Color32, FontFamily, FontId, Id, Key, Layout, Margin, Modifiers, Order, PopupCloseBehavior,
    Rect, Response, Sense, Stroke, TextureHandle, TextureOptions, Ui, UiBuilder, Vec2,
};
use jepret_core::{config::CleanupRules, Config};
use jepret_history::{Entry, EntryKind, Query};
use jepret_ui_theme::{
    icons,
    tokens::{CONTROL_H, RADIUS_CONTROL, TEXT_BODY, TEXT_SMALL, FAVORITE},
    widgets::{self, BannerKind},
    Icon, Palette,
};

use crate::{
    model::{
        cleanup_summary, day_label, grid_columns, human_bytes, local_day, move_selection, push_day, source_label,
        thousands, DayRun, Dir, Filter, GridLayout, KindFilter, LruCache, PagedList, Row,
    },
    source::{Source, Thumb},
};

/// Preferred card width; the real width stretches to fill the row.
const CARD_TARGET_W: f32 = 220.0;
const GAP: f32 = 14.0;
/// Card padding (6 px) + selection border (2 px).
const PAD: f32 = 8.0;
const THUMB_H: f32 = 132.0;
const CARD_H: f32 = PAD + THUMB_H + 6.0 + 16.0 + 14.0 + PAD;
const ROW_H: f32 = CARD_H + GAP;
const HEADER_H: f32 = 34.0;
const TEXTURE_CACHE: usize = 300;
const PAGE_CACHE: usize = 6;
const WORKERS: usize = 2;
const TOAST_SECS: u64 = 6;
const SCAN_PAGE: u32 = 500;

pub fn run(backend: Result<Arc<dyn Source>, String>, config: &Config) -> jepret_core::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_icon(std::sync::Arc::new(jepret_ui_theme::app_icon()))
            .with_title("Jepret - Galeri")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([420.0, 320.0]),
        ..Default::default()
    };
    let config = config.clone();
    eframe::run_native(
        "Jepret - Galeri",
        options,
        Box::new(move |cc| Ok(Box::new(GalleryApp::new(cc, backend, &config)))),
    )
    .map_err(|e| anyhow::anyhow!("gallery window failed: {e}"))
}

// ------------------------------------------------------------ thumb loader

type ThumbResult = (i64, Result<Thumb, String>);

#[derive(Default)]
struct LoaderState {
    queue: VecDeque<Entry>,
    inflight: HashSet<i64>,
    shutdown: bool,
}

struct LoaderShared {
    state: Mutex<LoaderState>,
    cv: Condvar,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Worker threads decoding thumbnails. The queue is replaced every frame with
/// what is currently visible, so fast scrolling never builds a backlog.
struct ThumbLoader {
    shared: Arc<LoaderShared>,
    rx: mpsc::Receiver<ThumbResult>,
}

impl ThumbLoader {
    fn new(src: Arc<dyn Source>, ctx: egui::Context) -> Self {
        let shared = Arc::new(LoaderShared { state: Mutex::default(), cv: Condvar::new() });
        let (tx, rx) = mpsc::channel();
        for _ in 0..WORKERS {
            let (shared, src, tx, ctx) = (shared.clone(), src.clone(), tx.clone(), ctx.clone());
            std::thread::spawn(move || loop {
                let entry = {
                    let mut st = lock(&shared.state);
                    loop {
                        if st.shutdown {
                            return;
                        }
                        if let Some(e) = st.queue.pop_front() {
                            st.inflight.insert(e.id);
                            break e;
                        }
                        st = shared.cv.wait(st).unwrap_or_else(|e| e.into_inner());
                    }
                };
                let res = src.thumbnail(&entry).map_err(|e| format!("{e:#}"));
                lock(&shared.state).inflight.remove(&entry.id);
                if tx.send((entry.id, res)).is_err() {
                    return;
                }
                ctx.request_repaint();
            });
        }
        Self { shared, rx }
    }

    fn set_wanted(&self, wanted: Vec<Entry>) {
        let mut st = lock(&self.shared.state);
        let inflight = st.inflight.clone();
        st.queue = wanted.into_iter().filter(|e| !inflight.contains(&e.id)).collect();
        drop(st);
        self.shared.cv.notify_all();
    }
}

impl Drop for ThumbLoader {
    fn drop(&mut self) {
        lock(&self.shared.state).shutdown = true;
        self.shared.cv.notify_all();
    }
}

// ------------------------------------------------------------- list scans

/// Result of walking a query in pages: day runs (for grouping) + totals.
#[derive(Default)]
struct Scan {
    runs: Vec<DayRun>,
    total: u64,
    bytes: u64,
    missing: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScanKind {
    /// Day runs for the current filter.
    Layout,
    /// Totals for the whole history (toolbar summary).
    Stats,
}

struct ScanMsg {
    kind: ScanKind,
    job: u64,
    res: Result<Scan, String>,
}

fn scan(src: &dyn Source, query: &Query, with_runs: bool) -> Result<Scan, String> {
    let mut out = Scan::default();
    let mut offset = 0u32;
    loop {
        let q = Query { offset, limit: SCAN_PAGE, ..query.clone() };
        let rows = src.list(&q).map_err(|e| format!("{e:#}"))?;
        for e in &rows {
            out.total += 1;
            out.bytes += e.bytes;
            out.missing += u64::from(e.missing);
            if with_runs {
                push_day(&mut out.runs, local_day(e.created_at));
            }
        }
        if rows.len() < SCAN_PAGE as usize {
            return Ok(out);
        }
        offset += rows.len() as u32;
    }
}

// ---------------------------------------------------------------- actions

enum CellAction {
    Open(Entry),
    Editor(Entry),
    Copy(Entry),
    CopyPath(Entry),
    Folder(Entry),
    ToggleFavorite(Entry),
    Delete(Entry),
}

fn file_name(e: &Entry) -> String {
    e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn open_with_shell(path: &Path) -> std::io::Result<()> {
    Command::new("explorer.exe").arg(path).spawn().map(|_| ())
}

fn reveal_in_folder(path: &Path) -> std::io::Result<()> {
    Command::new("explorer.exe").arg(format!("/select,{}", path.display())).spawn().map(|_| ())
}

fn copy_image(path: &Path) -> Result<(), String> {
    let img = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    arboard::Clipboard::new()
        .and_then(|mut c| {
            c.set_image(arboard::ImageData { width: w, height: h, bytes: img.into_raw().into() })
        })
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- widgets

fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

fn prop(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

fn measure(ui: &Ui, text: &str, font: &FontId) -> f32 {
    ui.fonts(|f| f.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE).size().x)
}

/// Menu row: label left, shortcut right (design: 32 px rows, 6 px radius).
fn menu_item(ui: &mut Ui, pal: &Palette, label: &str, key: &str, danger: bool) -> bool {
    let w = ui.available_width().max(236.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 6.0, pal.raised);
    }
    let col = if danger { pal.danger } else { pal.text };
    p.text(pos2(rect.left() + 10.0, rect.center().y), Align2::LEFT_CENTER, label, prop(TEXT_BODY), col);
    p.text(pos2(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER, key, mono(10.0), pal.muted);
    resp.clicked()
}

/// Right-click menu; mirrors the keyboard shortcuts shown on the right.
fn entry_menu(ui: &mut Ui, pal: &Palette, e: &Entry) -> Option<CellAction> {
    ui.set_min_width(236.0);
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut act = None;
    let mut item = |ui: &mut Ui, label: &str, key: &str, danger: bool, make: fn(Entry) -> CellAction| {
        if menu_item(ui, pal, label, key, danger) {
            act = Some(make(e.clone()));
            ui.close_menu();
        }
    };
    item(ui, "Buka", "Enter", false, CellAction::Open);
    if e.kind == EntryKind::Image {
        item(ui, "Buka di editor", "E", false, CellAction::Editor);
        item(ui, "Copy gambar", "Ctrl+C", false, CellAction::Copy);
    }
    item(ui, "Copy path", "Ctrl+Shift+C", false, CellAction::CopyPath);
    item(ui, "Buka folder", "Ctrl+Shift+O", false, CellAction::Folder);
    item(ui, if e.favorite { "Hapus favorit" } else { "Favorit" }, "F", false, CellAction::ToggleFavorite);
    ui.add_space(4.0);
    item(ui, "Hapus", "Del", true, CellAction::Delete);
    act
}

/// Outlined toolbar chip with icon; accent-soft when `active`.
fn chip(ui: &mut Ui, pal: &Palette, icon: Icon, label: &str, active: bool) -> Response {
    let font = prop(TEXT_BODY);
    let w = 10.0 + 14.0 + 6.0 + measure(ui, label, &font) + 10.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, CONTROL_H), Sense::click());
    let (fill, stroke, fg) = if active {
        (pal.accent_soft, pal.accent, pal.accent)
    } else {
        (if resp.hovered() { pal.raised } else { pal.surface }, pal.line_strong, pal.text)
    };
    let p = ui.painter();
    p.rect(rect, RADIUS_CONTROL, fill, Stroke::new(1.0_f32, stroke));
    icons::paint(p, Rect::from_center_size(pos2(rect.left() + 17.0, rect.center().y), Vec2::splat(14.0)), icon, fg, 1.75);
    p.text(pos2(rect.left() + 30.0, rect.center().y), Align2::LEFT_CENTER, label, font, if active { pal.text } else { fg });
    resp
}

fn danger_button(ui: &mut Ui, pal: &Palette, label: &str) -> Response {
    let font = prop(TEXT_BODY);
    let w = 12.0 + measure(ui, label, &font) + 12.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, CONTROL_H), Sense::click());
    let fill = if resp.hovered() { pal.danger.gamma_multiply(0.9) } else { pal.danger };
    let fg = if pal.dark { Color32::BLACK } else { Color32::WHITE };
    ui.painter().rect_filled(rect, RADIUS_CONTROL, fill);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, fg);
    resp
}

fn time_of_day(ms: i64) -> String {
    Local.timestamp_millis_opt(ms).earliest().map(|d| d.format("%H.%M").to_string()).unwrap_or_default()
}

fn meta_text(e: &Entry) -> String {
    let when = time_of_day(e.created_at);
    if e.width > 0 && e.height > 0 {
        format!("{} × {} · {} · {}", e.width, e.height, source_label(&e.source), when)
    } else {
        format!("{} · {}", source_label(&e.source), when)
    }
}

struct CardOut {
    action: Option<CellAction>,
    select: bool,
}

fn elided(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping {
        max_width: width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.fonts(|f| f.layout_job(job))
}

fn draw_card(
    ui: &Ui,
    pal: &Palette,
    rect: Rect,
    e: &Entry,
    tex: Option<&TextureHandle>,
    failed: bool,
    selected: bool,
) -> CardOut {
    let id = Id::new(("card", e.id));
    let resp = ui.interact(rect, id, Sense::click());
    let hovered = resp.hovered();
    let mut out = CardOut { action: None, select: resp.clicked() || resp.secondary_clicked() };
    if resp.double_clicked() {
        out.action = Some(CellAction::Open(e.clone()));
    }

    let p = ui.painter();
    let fill = if selected {
        pal.accent_soft
    } else if hovered {
        pal.raised
    } else {
        Color32::TRANSPARENT
    };
    p.rect_filled(rect, 10.0, fill);
    if selected {
        p.rect_stroke(rect.shrink(1.0), 9.0, Stroke::new(2.0_f32, pal.accent));
    }

    let thumb = Rect::from_min_size(rect.min + vec2(PAD, PAD), vec2(rect.width() - 2.0 * PAD, THUMB_H));
    p.rect_filled(thumb, 6.0, pal.canvas);
    match tex {
        Some(t) => {
            // "cover": fill the thumbnail box, crop the overflow.
            let s = t.size_vec2();
            let ts = thumb.size();
            let k = (ts.x / s.x).max(ts.y / s.y);
            let vis = ts / k;
            let uv = Rect::from_center_size(pos2(0.5, 0.5), vec2(vis.x / s.x, vis.y / s.y));
            let tint = if e.missing { Color32::WHITE.gamma_multiply(0.45) } else { Color32::WHITE };
            egui::Image::new((t.id(), ts)).uv(uv).rounding(6.0).tint(tint).paint_at(ui, thumb);
        }
        None => {
            let msg = if failed { "tak ada thumbnail" } else { "…" };
            p.text(thumb.center(), Align2::CENTER_CENTER, msg, prop(TEXT_SMALL), pal.muted);
        }
    }

    if e.kind == EntryKind::Video {
        p.circle_filled(thumb.center(), 18.0, Color32::from_black_alpha(140));
        icons::paint_filled(p, Rect::from_center_size(thumb.center() + vec2(1.0, 0.0), Vec2::splat(14.0)), Icon::Play, Color32::WHITE);
    }

    if e.missing {
        let g = elided(ui, "File hilang", prop(11.0), pal.danger, thumb.width() - 20.0);
        let r = Rect::from_min_size(thumb.left_bottom() + vec2(6.0, -6.0 - 18.0), vec2(g.size().x + 12.0, 18.0));
        p.rect_filled(r, 4.0, pal.danger_soft);
        p.galley(pos2(r.left() + 6.0, r.center().y - g.size().y / 2.0), g, pal.danger);
    }

    if e.favorite || hovered {
        let badge = Rect::from_min_size(pos2(thumb.right() - 6.0 - 22.0, thumb.top() + 6.0), Vec2::splat(22.0));
        let star = ui.interact(badge, id.with("star"), Sense::click());
        p.rect_filled(badge, 6.0, Color32::WHITE.gamma_multiply(0.92));
        let ic = Rect::from_center_size(badge.center(), Vec2::splat(13.0));
        if e.favorite {
            icons::paint_filled(p, ic, Icon::Star, FAVORITE);
        } else {
            icons::paint(p, ic, Icon::Star, Palette::LIGHT.muted, 1.75);
        }
        if star.clicked() {
            out.action = Some(CellAction::ToggleFavorite(e.clone()));
            out.select = true;
        }
        star.on_hover_text(if e.favorite { "Hapus favorit (F)" } else { "Jadikan favorit (F)" });
    }

    let text_w = rect.width() - 2.0 * PAD - 4.0;
    let name = elided(ui, &file_name(e), prop(TEXT_SMALL), pal.text, text_w);
    p.galley(pos2(rect.left() + PAD + 2.0, thumb.bottom() + 6.0), name, pal.text);
    let meta = elided(ui, &meta_text(e), prop(11.0), pal.muted, text_w);
    p.galley(pos2(rect.left() + PAD + 2.0, thumb.bottom() + 6.0 + 17.0), meta, pal.muted);

    let resp = resp.on_hover_text(e.path.display().to_string());
    resp.context_menu(|ui| {
        if let Some(a) = entry_menu(ui, pal, e) {
            out.action = Some(a);
        }
    });
    out
}

// -------------------------------------------------------------------- app

struct Toast {
    text: String,
    ok: bool,
    until: Instant,
}

struct Ready {
    pal: Palette,
    source: Arc<dyn Source>,
    rules: CleanupRules,
    filter: Filter,
    applied: Filter,
    filter_error: Option<String>,
    query: Query,
    list: PagedList,
    textures: LruCache<i64, TextureHandle>,
    failed: HashSet<i64>,
    loader: ThumbLoader,
    runs: Vec<DayRun>,
    layout: GridLayout,
    layout_dirty: bool,
    scanning: bool,
    /// (count, bytes, missing) of the whole history.
    stats: Option<(u64, u64, u64)>,
    scan_tx: mpsc::Sender<ScanMsg>,
    scan_rx: mpsc::Receiver<ScanMsg>,
    layout_job: u64,
    stats_job: u64,
    selected: Option<u64>,
    scroll_to_selected: bool,
    toast: Option<Toast>,
    confirm_delete: Option<Entry>,
    ctx: egui::Context,
}

enum Backend {
    Error(String),
    Ready(Box<Ready>),
}

pub struct GalleryApp {
    pal: Palette,
    backend: Backend,
}

impl GalleryApp {
    fn new(cc: &eframe::CreationContext<'_>, backend: Result<Arc<dyn Source>, String>, config: &Config) -> Self {
        let pal = jepret_ui_theme::install(&cc.egui_ctx, config.general.theme);
        let backend = match backend {
            Err(msg) => Backend::Error(msg),
            Ok(source) => {
                let mut list = PagedList::new(PAGE_CACHE);
                let filter = Filter::default();
                let query = filter.to_query().unwrap_or_default();
                list.load(&*source, query.clone());
                let (scan_tx, scan_rx) = mpsc::channel();
                let mut r = Ready {
                    pal,
                    loader: ThumbLoader::new(source.clone(), cc.egui_ctx.clone()),
                    source,
                    rules: config.history.cleanup.clone(),
                    applied: filter.clone(),
                    filter,
                    filter_error: None,
                    query,
                    list,
                    textures: LruCache::new(TEXTURE_CACHE),
                    failed: HashSet::new(),
                    runs: Vec::new(),
                    layout: GridLayout::default(),
                    layout_dirty: true,
                    scanning: false,
                    stats: None,
                    scan_tx,
                    scan_rx,
                    layout_job: 0,
                    stats_job: 0,
                    selected: None,
                    scroll_to_selected: false,
                    toast: None,
                    confirm_delete: None,
                    ctx: cc.egui_ctx.clone(),
                };
                r.start_scan();
                r.start_stats();
                Backend::Ready(Box::new(r))
            }
        };
        Self { pal, backend }
    }
}

impl eframe::App for GalleryApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        match &mut self.backend {
            Backend::Error(msg) => {
                let pal = self.pal;
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add_space(24.0);
                    ui.heading("Galeri tidak bisa dibuka");
                    ui.add_space(12.0);
                    widgets::banner(ui, &pal, BannerKind::Danger, msg, None);
                });
            }
            Backend::Ready(r) => r.update(ctx),
        }
    }
}

impl Ready {
    fn update(&mut self, ctx: &egui::Context) {
        self.poll_thumbs(ctx);
        self.poll_scans();
        self.handle_keys(ctx);

        let pal = self.pal;
        egui::TopBottomPanel::top("toolbar")
            .frame(egui::Frame::none().fill(pal.surface).inner_margin(Margin::symmetric(20.0, 12.0)))
            .show(ctx, |ui| self.toolbar(ui));

        let mut actions = Vec::new();
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(pal.bg).inner_margin(Margin::symmetric(20.0, 16.0)))
            .show(ctx, |ui| self.grid(ui, &mut actions));
        for a in actions {
            self.apply(ctx, a);
        }
        self.delete_dialog(ctx);
        self.show_toast(ctx);
    }

    // ---- background work

    fn spawn_scan(&self, kind: ScanKind, job: u64, query: Query) {
        let (src, tx, ctx) = (self.source.clone(), self.scan_tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let res = scan(&*src, &query, kind == ScanKind::Layout);
            let _ = tx.send(ScanMsg { kind, job, res });
            ctx.request_repaint();
        });
    }

    fn start_scan(&mut self) {
        self.layout_job += 1;
        self.scanning = true;
        self.spawn_scan(ScanKind::Layout, self.layout_job, self.query.clone());
    }

    fn start_stats(&mut self) {
        self.stats_job += 1;
        self.spawn_scan(ScanKind::Stats, self.stats_job, Query::default());
    }

    fn poll_scans(&mut self) {
        while let Ok(m) = self.scan_rx.try_recv() {
            match (m.kind, m.res) {
                (ScanKind::Layout, res) if m.job == self.layout_job => {
                    self.scanning = false;
                    match res {
                        Ok(s) => {
                            self.runs = s.runs;
                            self.layout_dirty = true;
                        }
                        Err(e) => self.list.last_error = Some(e),
                    }
                }
                (ScanKind::Stats, Ok(s)) if m.job == self.stats_job => {
                    self.stats = Some((s.total, s.bytes, s.missing));
                }
                _ => {}
            }
        }
    }

    fn poll_thumbs(&mut self, ctx: &egui::Context) {
        while let Ok((id, res)) = self.loader.rx.try_recv() {
            match res {
                Ok(t) if t.width > 0 && t.height > 0 && t.rgba.len() == t.width * t.height * 4 => {
                    let img = egui::ColorImage::from_rgba_unmultiplied([t.width, t.height], &t.rgba);
                    let tex = ctx.load_texture(format!("thumb{id}"), img, TextureOptions::LINEAR);
                    self.textures.put(id, tex);
                }
                Ok(_) => {
                    self.failed.insert(id);
                }
                Err(e) => {
                    tracing::debug!("thumbnail {id}: {e}");
                    self.failed.insert(id);
                }
            }
        }
    }

    /// After favorite/delete/cleanup: recount, regroup, refresh totals.
    fn reload(&mut self) {
        self.list.refresh(&*self.source);
        let total = self.list.total();
        self.selected = match self.selected {
            Some(_) if total == 0 => None,
            Some(i) => Some(i.min(total - 1)),
            None => None,
        };
        self.start_scan();
        self.start_stats();
    }

    // ---- toast

    fn notify(&mut self, ok: bool, text: impl Into<String>) {
        self.toast = Some(Toast { text: text.into(), ok, until: Instant::now() + Duration::from_secs(TOAST_SECS) });
    }

    fn show_toast(&mut self, ctx: &egui::Context) {
        let Some(t) = &self.toast else { return };
        let now = Instant::now();
        if now >= t.until {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(t.until - now);
        // Inverted palette: dark toast on light theme (and vice versa).
        let inv = if self.pal.dark { Palette::LIGHT } else { Palette::DARK };
        let pal = self.pal;
        egui::Area::new(Id::new("toast"))
            .order(Order::Foreground)
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -20.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(pal.tooltip)
                    .rounding(10.0)
                    .inner_margin(Margin::symmetric(14.0, 10.0))
                    .shadow(egui::epaint::Shadow {
                        offset: vec2(0.0, 8.0),
                        blur: 24.0,
                        spread: 0.0,
                        color: Color32::from_black_alpha(50),
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                            if t.ok {
                                icons::paint(ui.painter(), r, Icon::Check, inv.accent, 2.0);
                            } else {
                                icons::paint(ui.painter(), r, Icon::Warning, inv.danger, 2.0);
                            }
                            ui.label(egui::RichText::new(&t.text).color(pal.tooltip_text));
                        });
                    });
            });
    }

    // ---- keyboard

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let search_id = Id::new("gallery_search");
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::F)) {
            ctx.memory_mut(|m| m.request_focus(search_id));
        }
        if self.confirm_delete.is_some() {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.confirm_delete = None;
            }
            return;
        }
        if ctx.wants_keyboard_input() {
            return;
        }

        let mut nav = None;
        let mut make: Option<fn(Entry) -> CellAction> = None;
        ctx.input_mut(|i| {
            for (k, d) in [(Key::ArrowLeft, Dir::Left), (Key::ArrowRight, Dir::Right), (Key::ArrowUp, Dir::Up), (Key::ArrowDown, Dir::Down)] {
                if i.consume_key(Modifiers::NONE, k) {
                    nav = Some(d);
                }
            }
            // egui-winit turns Ctrl+C and Ctrl+Shift+C into `Event::Copy` (no key event).
            if i.events.iter().any(|e| matches!(e, egui::Event::Copy)) {
                i.events.retain(|e| !matches!(e, egui::Event::Copy));
                make = Some(if i.modifiers.shift { CellAction::CopyPath } else { CellAction::Copy });
            }
            if i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::O) {
                make = Some(CellAction::Folder);
            }
            if i.consume_key(Modifiers::NONE, Key::Enter) {
                make = Some(CellAction::Open);
            }
            if i.consume_key(Modifiers::NONE, Key::E) {
                make = Some(CellAction::Editor);
            }
            if i.consume_key(Modifiers::NONE, Key::F) {
                make = Some(CellAction::ToggleFavorite);
            }
            if i.consume_key(Modifiers::NONE, Key::Delete) {
                make = Some(CellAction::Delete);
            }
        });

        if let Some(d) = nav {
            let cur = self.selected.unwrap_or(0);
            let next = if self.selected.is_some() { move_selection(&self.runs, self.layout.cols, cur, d) } else { 0 };
            self.selected = (self.list.total() > 0).then_some(next);
            self.scroll_to_selected = true;
        }
        if let (Some(make), Some(sel)) = (make, self.selected) {
            if let Some(e) = self.list.get(&*self.source, sel) {
                self.apply(ctx, make(e));
            }
        }
    }

    // ---- toolbar

    fn search_box(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let id = Id::new("gallery_search");
        let (rect, _) = ui.allocate_exact_size(vec2(280.0, CONTROL_H), Sense::hover());
        let focused = ui.memory(|m| m.has_focus(id));
        let hint = "Ctrl+F";
        let hint_w = measure(ui, hint, &mono(10.0));
        {
            let p = ui.painter();
            p.rect(
                rect,
                RADIUS_CONTROL,
                pal.surface,
                Stroke::new(1.0_f32, if focused { pal.accent } else { pal.line_strong }),
            );
            let ic = Rect::from_center_size(pos2(rect.left() + 17.0, rect.center().y), Vec2::splat(14.0));
            icons::paint(p, ic, Icon::Search, pal.muted, 2.0);
            p.text(pos2(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER, hint, mono(10.0), pal.muted);
        }
        let edit_rect = Rect::from_min_max(
            pos2(rect.left() + 32.0, rect.top() + 2.0),
            pos2(rect.right() - 10.0 - hint_w - 8.0, rect.bottom() - 2.0),
        );
        ui.allocate_new_ui(
            UiBuilder::new().max_rect(edit_rect).layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter.search)
                        .id(id)
                        .frame(false)
                        .margin(Margin::ZERO)
                        .hint_text("Cari nama file…")
                        .desired_width(edit_rect.width()),
                );
            },
        );
    }

    fn date_chip(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let (from, to) = (self.filter.from.trim().to_owned(), self.filter.to.trim().to_owned());
        let label = match (from.is_empty(), to.is_empty()) {
            (true, true) => "Semua tanggal".to_owned(),
            (false, true) => format!("Sejak {from}"),
            (true, false) => format!("Sampai {to}"),
            (false, false) => format!("{from} – {to}"),
        };
        let active = !(from.is_empty() && to.is_empty());
        let r = chip(ui, &pal, Icon::Calendar, &label, active);
        let pid = Id::new("gallery_date_popup");
        if r.clicked() {
            ui.memory_mut(|m| m.toggle_popup(pid));
        }
        let Self { filter, filter_error, .. } = self;
        egui::popup_below_widget(ui, pid, &r, PopupCloseBehavior::CloseOnClickOutside, |ui| {
            ui.set_min_width(220.0);
            widgets::section_header(ui, &pal, "Rentang tanggal");
            ui.add(egui::TextEdit::singleline(&mut filter.from).hint_text("dari YYYY-MM-DD").desired_width(200.0));
            ui.add(egui::TextEdit::singleline(&mut filter.to).hint_text("sampai YYYY-MM-DD").desired_width(200.0));
            if let Some(e) = filter_error.as_deref() {
                ui.colored_label(pal.danger, e);
            }
            if widgets::button(ui, &pal, None, "Semua tanggal", None).clicked() {
                filter.from.clear();
                filter.to.clear();
            }
        });
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        ui.spacing_mut().item_spacing = vec2(10.0, 8.0);
        let mut do_cleanup = false;
        let mut do_prune = false;
        ui.horizontal_wrapped(|ui| {
            self.search_box(ui);

            let mut k = match self.filter.kind {
                KindFilter::All => 0,
                KindFilter::Image => 1,
                KindFilter::Video => 2,
            };
            if widgets::segmented(ui, &pal, &mut k, &["Semua", "Gambar", "Video"]).changed() {
                self.filter.kind = [KindFilter::All, KindFilter::Image, KindFilter::Video][k.min(2)];
            }
            if chip(ui, &pal, Icon::Star, "Favorit", self.filter.favorites_only).clicked() {
                self.filter.favorites_only = !self.filter.favorites_only;
            }
            self.date_chip(ui);

            let summary = self.summary();
            let missing = self.stats.map(|s| s.2).unwrap_or(0);
            let prune_label = format!("Buang {missing} entri hilang");
            let font = prop(TEXT_SMALL);
            let btn_w = |ui: &Ui, s: &str| 12.0 + 16.0 + 8.0 + measure(ui, s, &prop(TEXT_BODY)) + 12.0;
            let mut right_w = measure(ui, &summary, &font) + 10.0 + btn_w(ui, "Bersihkan sekarang");
            if missing > 0 {
                right_w += 10.0 + btn_w(ui, &prune_label);
            }
            let space = ui.available_width() - right_w;
            if space > 0.0 {
                ui.add_space(space);
            }
            ui.label(egui::RichText::new(summary).size(TEXT_SMALL).color(pal.muted));
            if missing > 0 && widgets::button(ui, &pal, None, &prune_label, None).clicked() {
                do_prune = true;
            }
            if widgets::button(ui, &pal, Some(Icon::Trash), "Bersihkan sekarang", None).clicked() {
                do_cleanup = true;
            }
        });
        if let Some(e) = &self.filter_error {
            ui.colored_label(pal.danger, e);
        }
        if do_cleanup {
            self.cleanup();
        }
        if do_prune {
            self.prune();
        }

        if self.filter != self.applied {
            self.applied = self.filter.clone();
            match self.filter.to_query() {
                Ok(q) => {
                    self.filter_error = None;
                    self.query = q.clone();
                    self.list.load(&*self.source, q);
                    self.selected = None;
                    self.start_scan();
                }
                Err(e) => self.filter_error = Some(e),
            }
        }
    }

    /// "1.284 capture · 3,2 GB · auto-cleanup > 30 hari"
    fn summary(&self) -> String {
        let (count, bytes) = match self.stats {
            Some((c, b, _)) => (c, Some(b)),
            None => (self.list.total(), None),
        };
        let mut s = format!("{} capture", thousands(count));
        if let Some(b) = bytes {
            s.push_str(&format!(" · {}", human_bytes(b)));
        }
        let r = &self.rules;
        if !r.enabled {
            s.push_str(" · auto-cleanup mati");
        } else if let Some(d) = r.max_age_days {
            s.push_str(&format!(" · auto-cleanup > {d} hari"));
        } else if let Some(mb) = r.max_total_mb {
            s.push_str(&format!(" · auto-cleanup > {mb} MB"));
        }
        s
    }

    // ---- grid

    fn grid(&mut self, ui: &mut Ui, actions: &mut Vec<CellAction>) {
        let pal = self.pal;
        let avail = ui.available_width() - ui.spacing().scroll.bar_width;
        let cols = grid_columns(avail, CARD_TARGET_W, GAP);
        if self.layout.cols != cols || self.layout_dirty {
            self.layout = GridLayout::build(&self.runs, cols, HEADER_H, ROW_H);
            self.layout_dirty = false;
        }
        if self.layout.rows.is_empty() {
            let msg = if self.scanning {
                "Memuat…"
            } else if self.filter != Filter::default() {
                "Tidak ada capture yang cocok dengan filter"
            } else {
                "Belum ada capture"
            };
            ui.add_space(40.0);
            ui.vertical_centered(|ui| ui.label(egui::RichText::new(msg).color(pal.muted)));
            return;
        }

        let card_w = ((avail - GAP * (cols as f32 - 1.0)) / cols as f32).floor().max(120.0);
        let today = Local::now().date_naive();
        let scroll_to = std::mem::take(&mut self.scroll_to_selected);
        let mut wanted = Vec::new();
        let Self { source, list, textures, failed, layout, runs, selected, .. } = self;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show_viewport(ui, |ui, viewport| {
            let origin = ui.max_rect().min;
            ui.allocate_space(vec2(avail, layout.height + 16.0));
            if scroll_to {
                if let Some((_, top, h)) = selected.and_then(|s| layout.row_of(s)) {
                    let r = Rect::from_min_size(pos2(origin.x, origin.y + top), vec2(avail, h - GAP));
                    ui.scroll_to_rect(r, None);
                    ui.ctx().request_repaint();
                }
            }
            for ri in layout.visible(viewport.min.y - ROW_H, viewport.max.y + ROW_H) {
                let top = origin.y + layout.tops[ri];
                match &layout.rows[ri] {
                    Row::Header { run, .. } => {
                        let label = runs.get(*run).map(|r| day_label(r.day, today)).unwrap_or_default();
                        ui.painter().text(
                            pos2(origin.x + 2.0, top + HEADER_H - 10.0),
                            Align2::LEFT_BOTTOM,
                            label,
                            prop(TEXT_BODY),
                            pal.muted,
                        );
                    }
                    Row::Cards { start, count, .. } => {
                        for c in 0..*count {
                            let idx = start + c as u64;
                            let Some(e) = list.get(&**source, idx) else { break };
                            let tex = textures.get(&e.id);
                            let has_failed = failed.contains(&e.id);
                            if tex.is_none() && !has_failed {
                                wanted.push(e.clone());
                            }
                            let x = origin.x + c as f32 * (card_w + GAP);
                            let rect = Rect::from_min_size(pos2(x, top), vec2(card_w, CARD_H));
                            let out = draw_card(ui, &pal, rect, &e, tex, has_failed, *selected == Some(idx));
                            if out.select {
                                *selected = Some(idx);
                            }
                            if let Some(a) = out.action {
                                actions.push(a);
                            }
                        }
                    }
                }
            }
        });
        self.loader.set_wanted(wanted);
    }

    // ---- actions

    fn apply(&mut self, ctx: &egui::Context, a: CellAction) {
        match a {
            CellAction::Open(e) => {
                if let Err(err) = open_with_shell(&e.path) {
                    self.notify(false, format!("Gagal membuka: {err}"));
                }
            }
            CellAction::Editor(e) => {
                if e.kind != EntryKind::Image {
                    return;
                }
                let r = std::env::current_exe().and_then(|exe| Command::new(exe).arg("--editor").arg(&e.path).spawn());
                if let Err(err) = r {
                    self.notify(false, format!("Gagal membuka editor: {err}"));
                }
            }
            CellAction::Copy(e) => {
                if e.kind != EntryKind::Image {
                    return;
                }
                match copy_image(&e.path) {
                    Ok(()) => self.notify(true, "Gambar disalin ke clipboard"),
                    Err(err) => self.notify(false, format!("Gagal menyalin gambar: {err}")),
                }
            }
            CellAction::CopyPath(e) => {
                ctx.copy_text(e.path.display().to_string());
                self.notify(true, "Path disalin");
            }
            CellAction::Folder(e) => {
                if let Err(err) = reveal_in_folder(&e.path) {
                    self.notify(false, format!("Gagal membuka folder: {err}"));
                }
            }
            CellAction::ToggleFavorite(e) => match self.source.set_favorite(e.id, !e.favorite) {
                Ok(()) => self.reload(),
                Err(err) => self.notify(false, format!("Gagal mengubah favorit: {err:#}")),
            },
            CellAction::Delete(e) => self.confirm_delete = Some(e),
        }
    }

    fn delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(e) = self.confirm_delete.clone() else { return };
        let pal = self.pal;
        let mut decision = None;
        egui::Window::new("Hapus item?")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size(vec2(360.0, 0.0))
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("Hapus item?").size(15.0).strong());
                ui.add_space(4.0);
                ui.label(format!("Hapus {} dari riwayat dan dari disk?", file_name(&e)));
                if self.rules.use_recycle_bin {
                    ui.label(egui::RichText::new("File dipindahkan ke Recycle Bin.").color(pal.muted));
                } else {
                    ui.label(egui::RichText::new("File dihapus permanen.").color(pal.danger));
                }
                ui.add_space(8.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if danger_button(ui, &pal, "Hapus").clicked() {
                        decision = Some(true);
                    }
                    if widgets::button(ui, &pal, None, "Batal", Some("Esc")).clicked() {
                        decision = Some(false);
                    }
                });
            });
        match decision {
            Some(true) => {
                match self.source.delete(e.id, true, self.rules.use_recycle_bin) {
                    Ok(()) => self.notify(true, format!("Dihapus: {}", file_name(&e))),
                    Err(err) => self.notify(false, format!("Gagal menghapus: {err:#}")),
                }
                self.confirm_delete = None;
                self.reload();
            }
            Some(false) => self.confirm_delete = None,
            None => {}
        }
    }

    fn cleanup(&mut self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        match self.source.cleanup(now) {
            Ok(r) => {
                let text = cleanup_summary(&r, self.rules.use_recycle_bin, self.rules.keep_favorites);
                self.notify(r.errors.is_empty(), text);
            }
            Err(e) => self.notify(false, format!("Cleanup gagal: {e:#}")),
        }
        self.reload();
    }

    fn prune(&mut self) {
        match self.source.prune_missing() {
            Ok(n) => self.notify(true, format!("{n} entri hilang dibuang")),
            Err(e) => self.notify(false, format!("Gagal: {e:#}")),
        }
        self.reload();
    }
}
