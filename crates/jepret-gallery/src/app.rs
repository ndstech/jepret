//! egui front-end: filter panel, grouped virtual grid, detail panel, actions,
//! thumbnail workers.
//!
//! Look = the editor's (`jepret-ui-theme`): top action bar, left filter panel,
//! canvas-grey centre, right detail panel, status bar. Layout is virtual in
//! both axes of cost: only visible rows touch the DB (`PagedList`) and the
//! decoder threads; day-group boundaries come from a light background scan
//! that keeps only `(day, count)` runs, never the entries themselves.

use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    process::Command,
    sync::{mpsc, Arc, Condvar, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use chrono::{Local, NaiveDate, TimeZone};
use egui::{
    pos2, vec2, Align, Align2, Color32, CursorIcon, FontFamily, FontId, Id, Key, Layout, Margin, Modifiers, Order,
    PopupCloseBehavior, Rect, Response, Sense, Stroke, TextureHandle, TextureOptions, Ui, UiBuilder, Vec2,
};
use jepret_core::{config::CleanupRules, Config};
use jepret_history::{Entry, EntryKind, Query};
use jepret_ui_theme::{
    icons,
    tokens::{CONTROL_H, FAVORITE, RADIUS_CONTROL, SP_1, SP_2, SP_3, SP_4, TEXT_BODY, TEXT_SMALL},
    widgets::{self, BannerKind},
    Icon, Palette,
};

use crate::{
    model::{
        card_width, cleanup_summary, day_label, fit_inside, grid_columns, human_bytes, local_day, move_selection,
        push_day, short_date, source_label, thousands, Collection, Counts, DatePreset, DayRun, Dir, Filter,
        GridLayout, LruCache, PagedList, Row,
    },
    source::{Source, Thumb},
};

const GAP: f32 = 14.0;
/// Card padding.
const PAD: f32 = 8.0;
const HEADER_H: f32 = 36.0;
const LEFT_W: f32 = 208.0;
const RIGHT_W: f32 = 272.0;
const BAR_H: f32 = 52.0;
const STATUS_H: f32 = 28.0;
const TEXTURE_CACHE: usize = 300;
const PAGE_CACHE: usize = 6;
const WORKERS: usize = 2;
const TOAST_SECS: u64 = 6;
const SCAN_PAGE: u32 = 500;

/// Thumbnail size toggle in the top bar.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ThumbSize {
    Large,
    Small,
}

impl ThumbSize {
    /// Preferred card width; the real width stretches to fill the row.
    fn target_w(self) -> f32 {
        match self {
            ThumbSize::Large => 220.0,
            ThumbSize::Small => 160.0,
        }
    }

    fn thumb_h(self) -> f32 {
        match self {
            ThumbSize::Large => 132.0,
            ThumbSize::Small => 96.0,
        }
    }

    /// padding + thumbnail + gap + name row + chip row + padding.
    fn card_h(self) -> f32 {
        PAD + self.thumb_h() + 8.0 + 16.0 + 20.0 + PAD
    }

    fn row_h(self) -> f32 {
        self.card_h() + GAP
    }
}

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

/// Result of walking a query in pages: day runs (for grouping) or totals.
#[derive(Default)]
struct Scan {
    runs: Vec<DayRun>,
    counts: Counts,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScanKind {
    /// Day runs for the current filter.
    Layout,
    /// Totals for the whole history (sidebar counts, status bar).
    Stats,
}

struct ScanMsg {
    kind: ScanKind,
    job: u64,
    res: Result<Scan, String>,
}

fn scan(src: &dyn Source, query: &Query, with_runs: bool, today: NaiveDate) -> Result<Scan, String> {
    let mut out = Scan::default();
    let mut offset = 0u32;
    loop {
        let q = Query { offset, limit: SCAN_PAGE, ..query.clone() };
        let rows = src.list(&q).map_err(|e| format!("{e:#}"))?;
        for e in &rows {
            if with_runs {
                push_day(&mut out.runs, local_day(e.created_at));
            } else {
                out.counts.add(e, today);
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


fn open_dir(path: &Path) -> std::io::Result<()> {
    Command::new("explorer.exe").arg(path).spawn().map(|_| ())
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

/// "18 Nov 2026, 13.45".
fn when_text(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms).earliest() {
        Some(d) => format!("{}, {}", short_date(d.date_naive()), d.format("%H.%M")),
        None => String::new(),
    }
}

/// Last two folders of the file's location, e.g. `Jepret\2026-11`.
fn tail_dir(path: &Path) -> String {
    let parts: Vec<String> = path
        .parent()
        .map(|p| p.components().filter_map(|c| c.as_os_str().to_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    let n = parts.len();
    parts[n.saturating_sub(2)..].join("\\")
}

fn time_of_day(ms: i64) -> String {
    Local.timestamp_millis_opt(ms).earliest().map(|d| d.format("%H.%M").to_string()).unwrap_or_default()
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

/// Neutral backdrop behind thumbnails (design: #F1F3F5 / #2A2E33).
fn thumb_bg(pal: &Palette) -> Color32 {
    if pal.dark {
        Color32::from_rgb(0x2A, 0x2E, 0x33)
    } else {
        Color32::from_rgb(0xF1, 0xF3, 0xF5)
    }
}

/// Mode chip colours: region = accent, recording = danger, rest neutral.
fn mode_colors(pal: &Palette, source: &str) -> (Color32, Color32) {
    match source {
        "region" | "last_region" => (pal.accent_soft, pal.accent),
        "record_region" | "record_monitor" | "recording" => (pal.danger_soft, pal.danger),
        _ => (pal.raised, pal.muted),
    }
}

/// Thumbnail box shared by cards and the detail panel: the image is fitted
/// inside `area` (letterboxed, never cropped or stretched).
fn paint_thumb(ui: &Ui, pal: &Palette, area: Rect, e: &Entry, tex: Option<&TextureHandle>, failed: bool) {
    let p = ui.painter();
    p.rect_filled(area, 6.0, thumb_bg(pal));
    match tex {
        Some(t) => {
            let s = t.size_vec2();
            let (iw, ih) = if e.width > 0 && e.height > 0 { (e.width as f32, e.height as f32) } else { (s.x, s.y) };
            let inner = area.shrink(5.0);
            let (w, h) = fit_inside(inner.width(), inner.height(), iw, ih);
            let img = Rect::from_center_size(area.center(), vec2(w, h));
            let shadow = Color32::from_black_alpha(if pal.dark { 90 } else { 38 });
            p.rect_filled(img.translate(vec2(0.0, 1.0)).expand(0.5), 3.0, shadow);
            let tint = if e.missing { Color32::WHITE.gamma_multiply(0.45) } else { Color32::WHITE };
            egui::Image::new((t.id(), img.size())).rounding(3.0).tint(tint).paint_at(ui, img);
        }
        None => {
            let msg = if failed { "tak ada thumbnail" } else { "…" };
            p.text(area.center(), Align2::CENTER_CENTER, msg, prop(TEXT_SMALL), pal.muted);
        }
    }

    if e.kind == EntryKind::Video {
        p.circle_filled(area.center(), 18.0, Color32::from_black_alpha(140));
        icons::paint_filled(p, Rect::from_center_size(area.center() + vec2(1.0, 0.0), Vec2::splat(14.0)), Icon::Play, Color32::WHITE);
    }

    if e.missing {
        let g = elided(ui, "File hilang", prop(11.0), pal.danger, area.width() - 20.0);
        let r = Rect::from_min_size(area.left_bottom() + vec2(6.0, -6.0 - 18.0), vec2(g.size().x + 12.0, 18.0));
        p.rect_filled(r, 4.0, pal.danger_soft);
        p.galley(pos2(r.left() + 6.0, r.center().y - g.size().y / 2.0), g, pal.danger);
    }

    if e.favorite {
        let badge = Rect::from_min_size(area.left_top() + vec2(6.0, 6.0), Vec2::splat(20.0));
        p.rect_filled(badge, 6.0, Color32::WHITE.gamma_multiply(0.92));
        icons::paint_filled(p, Rect::from_center_size(badge.center(), Vec2::splat(12.0)), Icon::Star, FAVORITE);
    }
}

/// Small hover button on a card; returns true when clicked.
fn quick_button(ui: &Ui, pal: &Palette, id: Id, rect: Rect, icon: Icon, gold: bool, tip: &str) -> bool {
    let r = ui.interact(rect, id, Sense::click());
    let p = ui.painter();
    p.rect(rect, 6.0, if r.hovered() { pal.raised } else { pal.surface }, Stroke::new(1.0_f32, pal.line_strong));
    let ic = Rect::from_center_size(rect.center(), Vec2::splat(13.0));
    if gold {
        icons::paint_filled(p, ic, icon, FAVORITE);
    } else {
        icons::paint(p, ic, icon, pal.text, 1.75);
    }
    let clicked = r.clicked();
    r.on_hover_text(tip);
    clicked
}

struct CardOut {
    action: Option<CellAction>,
    select: bool,
}

#[allow(clippy::too_many_arguments)]
fn draw_card(
    ui: &Ui,
    pal: &Palette,
    rect: Rect,
    thumb_h: f32,
    e: &Entry,
    tex: Option<&TextureHandle>,
    failed: bool,
    selected: bool,
) -> CardOut {
    let id = Id::new(("card", e.id));
    let resp = ui.interact(rect, id, Sense::click());
    let hovered = resp.hovered() || resp.contains_pointer();
    let mut out = CardOut { action: None, select: resp.clicked() || resp.secondary_clicked() };
    if resp.double_clicked() {
        out.action = Some(CellAction::Open(e.clone()));
    }

    let p = ui.painter();
    let radius = 10.0;
    if selected {
        p.rect_stroke(rect.expand(1.5), radius + 1.5, Stroke::new(3.0_f32, pal.accent_soft));
    }
    p.rect(rect, radius, pal.surface, Stroke::new(1.0_f32, if hovered { pal.line_strong } else { pal.line }));
    if selected {
        p.rect_stroke(rect.shrink(1.0), radius - 1.0, Stroke::new(2.0_f32, pal.accent));
    }

    let thumb = Rect::from_min_size(rect.min + vec2(PAD, PAD), vec2(rect.width() - 2.0 * PAD, thumb_h));
    paint_thumb(ui, pal, thumb, e, tex, failed);

    if hovered {
        // Quick actions, right to left: favorite, editor, copy.
        let mut x = thumb.right() - 6.0 - 22.0;
        let y = thumb.top() + 6.0;
        let mut next = || {
            let r = Rect::from_min_size(pos2(x, y), Vec2::splat(22.0));
            x -= 26.0;
            r
        };
        let fav_tip = if e.favorite { "Hapus favorit (F)" } else { "Jadikan favorit (F)" };
        if quick_button(ui, pal, id.with("fav"), next(), Icon::Star, e.favorite, fav_tip) {
            out.action = Some(CellAction::ToggleFavorite(e.clone()));
            out.select = true;
        }
        if e.kind == EntryKind::Image {
            if quick_button(ui, pal, id.with("edit"), next(), Icon::OpenExternal, false, "Buka di editor (E)") {
                out.action = Some(CellAction::Editor(e.clone()));
                out.select = true;
            }
            if quick_button(ui, pal, id.with("copy"), next(), Icon::Copy, false, "Copy gambar (Ctrl+C)") {
                out.action = Some(CellAction::Copy(e.clone()));
                out.select = true;
            }
        }
    }

    let text_x = rect.left() + PAD + 2.0;
    let text_w = rect.width() - 2.0 * PAD - 4.0;
    let name_y = thumb.bottom() + 8.0;
    let name = elided(ui, &file_name(e), prop(TEXT_BODY), pal.text, text_w);
    p.galley(pos2(text_x, name_y), name, pal.text);

    // Chip row: mode chip, dimensions, time (right).
    let cy = name_y + 16.0 + 11.0;
    let (cbg, cfg) = mode_colors(pal, &e.source);
    let label = elided(ui, &source_label(&e.source), prop(10.5), cfg, text_w * 0.5);
    let lsz = label.size();
    let chip = Rect::from_min_size(pos2(text_x, cy - 9.0), vec2(lsz.x + 12.0, 18.0));
    p.rect_filled(chip, 5.0, cbg);
    p.galley(pos2(chip.left() + 6.0, cy - lsz.y / 2.0), label, cfg);
    let time = elided(ui, &time_of_day(e.created_at), prop(11.0), pal.muted, 60.0);
    let tx = rect.right() - PAD - 2.0 - time.size().x;
    p.galley(pos2(tx, cy - time.size().y / 2.0), time, pal.muted);
    let dims_w = (tx - 6.0) - (chip.right() + 6.0);
    if e.width > 0 && e.height > 0 && dims_w > 24.0 {
        let dims = elided(ui, &format!("{} × {}", e.width, e.height), prop(11.0), pal.muted, dims_w);
        let h = dims.size().y;
        p.galley(pos2(chip.right() + 6.0, cy - h / 2.0), dims, pal.muted);
    }

    let resp = resp.on_hover_text(e.path.display().to_string());
    resp.context_menu(|ui| {
        if let Some(a) = entry_menu(ui, pal, e) {
            out.action = Some(a);
        }
    });
    out
}

/// Filter-panel row: icon, label, optional count; accent-soft when active.
fn nav_row(ui: &mut Ui, pal: &Palette, icon: Icon, label: &str, count: Option<u64>, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    let p = ui.painter();
    if active {
        p.rect_filled(rect, 6.0, pal.accent_soft);
    } else if resp.hovered() {
        p.rect_filled(rect, 6.0, pal.raised);
    }
    let fg = if active { pal.accent } else { pal.text };
    icons::paint(p, Rect::from_center_size(pos2(rect.left() + 17.0, rect.center().y), Vec2::splat(15.0)), icon, fg, 1.75);
    p.text(pos2(rect.left() + 34.0, rect.center().y), Align2::LEFT_CENTER, label, prop(TEXT_BODY), fg);
    if let Some(n) = count {
        let col = if active { pal.accent } else { pal.muted };
        p.text(pos2(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER, thousands(n), prop(TEXT_SMALL), col);
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BtnKind {
    Primary,
    Normal,
    Danger,
}

/// Full-width button with a key hint on the right (detail panel).
fn action_button(ui: &mut Ui, pal: &Palette, label: &str, key: &str, kind: BtnKind) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), CONTROL_H), Sense::click());
    let hovered = resp.hovered();
    let (fill, stroke, fg, kc) = match kind {
        BtnKind::Primary => (
            if hovered { pal.accent.gamma_multiply(0.9) } else { pal.accent },
            Stroke::NONE,
            pal.on_accent,
            pal.on_accent.gamma_multiply(0.8),
        ),
        BtnKind::Normal => (
            if hovered { pal.raised } else { pal.surface },
            Stroke::new(1.0_f32, pal.line_strong),
            pal.text,
            pal.muted,
        ),
        BtnKind::Danger => (
            if hovered { pal.danger_soft } else { pal.surface },
            Stroke::new(1.0_f32, pal.line_strong),
            pal.danger,
            pal.danger.gamma_multiply(0.75),
        ),
    };
    let p = ui.painter();
    p.rect(rect, RADIUS_CONTROL, fill, stroke);
    p.text(pos2(rect.left() + 14.0, rect.center().y), Align2::LEFT_CENTER, label, prop(TEXT_BODY), fg);
    p.text(pos2(rect.right() - 14.0, rect.center().y), Align2::RIGHT_CENTER, key, mono(10.0), kc);
    resp
}

/// Key/value row of the detail panel.
fn kv_row(ui: &mut Ui, pal: &Palette, key: &str, value: &str, full: Option<String>) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    let p = ui.painter();
    p.text(pos2(rect.left(), rect.center().y), Align2::LEFT_CENTER, key, prop(TEXT_SMALL), pal.muted);
    let g = elided(ui, value, prop(TEXT_SMALL), pal.text, (rect.width() - 76.0).max(40.0));
    let sz = g.size();
    p.galley(pos2(rect.right() - sz.x, rect.center().y - sz.y / 2.0), g, pal.text);
    if let Some(f) = full {
        resp.on_hover_text(f);
    }
}

/// Accent text link (status bar).
fn link(ui: &mut Ui, pal: &Palette, text: &str) -> Response {
    ui.add(egui::Label::new(egui::RichText::new(text).size(TEXT_SMALL).color(pal.accent)).sense(Sense::click()))
        .on_hover_cursor(CursorIcon::PointingHand)
}

/// "Auto-cleanup > 30 hari" / "Auto-cleanup mati".
fn cleanup_state(r: &CleanupRules) -> String {
    if !r.enabled {
        "Auto-cleanup mati".into()
    } else if let Some(d) = r.max_age_days {
        format!("Auto-cleanup > {d} hari")
    } else if let Some(mb) = r.max_total_mb {
        format!("Auto-cleanup > {mb} MB")
    } else {
        "Auto-cleanup aktif".into()
    }
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
    output_dir: PathBuf,
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
    size: ThumbSize,
    /// Totals of the whole history (sidebar counts, status bar).
    stats: Option<Counts>,
    scan_tx: mpsc::Sender<ScanMsg>,
    scan_rx: mpsc::Receiver<ScanMsg>,
    layout_job: u64,
    stats_job: u64,
    selected: Option<u64>,
    /// Entry at `selected`, refreshed every frame (page-cache hit, cheap).
    sel_entry: Option<Entry>,
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
                    output_dir: config.general.output_dir.clone(),
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
                    size: ThumbSize::Large,
                    stats: None,
                    scan_tx,
                    scan_rx,
                    layout_job: 0,
                    stats_job: 0,
                    selected: None,
                    sel_entry: None,
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
        self.sel_entry = match self.selected {
            Some(i) => self.list.get(&*self.source, i),
            None => None,
        };
        let mut actions = Vec::new();
        self.top_bar(ctx);
        self.status_bar(ctx);
        self.filter_panel(ctx);
        self.detail_panel(ctx, &mut actions);
        self.apply_filter();

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(pal.canvas).inner_margin(Margin::symmetric(SP_4, SP_4)))
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
            let res = scan(&*src, &query, kind == ScanKind::Layout, Local::now().date_naive());
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
                    self.stats = Some(s.counts);
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
        let mut folder_key = false;
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
                folder_key = true;
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
        if folder_key && self.selected.is_none() {
            self.open_output_dir();
        }
        if let (Some(make), Some(sel)) = (make, self.selected) {
            if let Some(e) = self.list.get(&*self.source, sel) {
                self.apply(ctx, make(e));
            }
        }
    }
    // ---- top bar

    fn search_box(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let id = Id::new("gallery_search");
        let width = 280.0_f32.min((ui.available_width() - 300.0).max(160.0));
        let (rect, _) = ui.allocate_exact_size(vec2(width, CONTROL_H), Sense::hover());
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
        // A child UI inside the already-allocated box: `allocate_new_ui`
        // would move the parent's cursor back to the edit field's right
        // edge, so the controls after it overlapped the "Ctrl+F" hint.
        let mut child = ui.new_child(UiBuilder::new().max_rect(edit_rect).layout(Layout::left_to_right(Align::Center)));
        child.add(
            egui::TextEdit::singleline(&mut self.filter.search)
                .id(id)
                .frame(false)
                .margin(Margin::ZERO)
                .hint_text("Cari nama file…")
                .desired_width(edit_rect.width()),
        );
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        let pal = self.pal;
        let mut size_idx = usize::from(self.size == ThumbSize::Small);
        let mut open_folder = false;
        egui::TopBottomPanel::top("bar")
            .exact_height(BAR_H)
            .frame(egui::Frame::none().fill(pal.surface).inner_margin(Margin::symmetric(SP_3, 0.0)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = SP_2;
                    self.search_box(ui);
                    let (sep, _) = ui.allocate_exact_size(vec2(1.0, 24.0), Sense::hover());
                    ui.painter().rect_filled(sep, 0.0, pal.line);
                    // Thumbnail size. The history query has no sort order
                    // (newest first only), so there is no sort control.
                    widgets::segmented(ui, &pal, &mut size_idx, &["Besar", "Kecil"]);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::button(ui, &pal, Some(Icon::Folder), "Buka folder", Some("Ctrl+Shift+O")).clicked() {
                            open_folder = true;
                        }
                    });
                });
            });
        let size = if size_idx == 1 { ThumbSize::Small } else { ThumbSize::Large };
        if size != self.size {
            self.size = size;
            self.layout_dirty = true;
        }
        if open_folder {
            self.open_output_dir();
        }
    }

    fn open_output_dir(&mut self) {
        if !self.output_dir.is_dir() {
            let msg = format!("Folder capture belum ada: {}", self.output_dir.display());
            self.notify(false, msg);
        } else if let Err(e) = open_dir(&self.output_dir) {
            self.notify(false, format!("Gagal membuka folder: {e}"));
        }
    }

    // ---- left filter panel

    fn section(ui: &mut Ui, pal: &Palette, title: &str) {
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = 18.0;
            ui.horizontal(|ui| {
                ui.add_space(SP_2);
                widgets::section_header(ui, pal, title);
            });
        });
    }

    fn date_row(&mut self, ui: &mut Ui, today: NaiveDate) {
        let pal = self.pal;
        let custom = self.filter.has_custom_date(today);
        let (from, to) = (self.filter.from.trim().to_owned(), self.filter.to.trim().to_owned());
        let label = if !custom {
            "Pilih rentang…".to_owned()
        } else {
            match (from.is_empty(), to.is_empty()) {
                (false, true) => format!("Sejak {from}"),
                (true, false) => format!("Sampai {to}"),
                _ => format!("{from} – {to}"),
            }
        };
        let r = nav_row(ui, &pal, Icon::Calendar, &label, None, custom);
        let pid = Id::new("gallery_date_popup");
        if r.clicked() {
            ui.memory_mut(|m| m.toggle_popup(pid));
        }
        let Self { filter, filter_error, .. } = self;
        egui::popup_below_widget(ui, pid, &r, PopupCloseBehavior::CloseOnClickOutside, |ui| {
            ui.set_min_width(200.0);
            widgets::section_header(ui, &pal, "Rentang tanggal");
            ui.add(egui::TextEdit::singleline(&mut filter.from).hint_text("dari YYYY-MM-DD").desired_width(180.0));
            ui.add(egui::TextEdit::singleline(&mut filter.to).hint_text("sampai YYYY-MM-DD").desired_width(180.0));
            if let Some(e) = filter_error.as_deref() {
                ui.colored_label(pal.danger, e);
            }
            if widgets::button(ui, &pal, None, "Semua tanggal", None).clicked() {
                filter.from.clear();
                filter.to.clear();
            }
        });
    }

    /// KOLEKSI + TANGGAL. There is no MODE CAPTURE section: the history query
    /// cannot filter by capture mode (the mode is shown on each card instead).
    fn filter_panel(&mut self, ctx: &egui::Context) {
        let pal = self.pal;
        let today = Local::now().date_naive();
        let counts = self.stats;
        let n = |f: fn(&Counts) -> u64| counts.as_ref().map(f);
        egui::SidePanel::left("filters")
            .exact_width(LEFT_W)
            .resizable(false)
            .frame(egui::Frame::none().fill(pal.surface).inner_margin(Margin {
                left: SP_2,
                right: SP_2,
                top: SP_3,
                bottom: SP_2,
            }))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    Self::section(ui, &pal, "Koleksi");
                    let cur = self.filter.collection();
                    let rows = [
                        (Collection::All, Icon::Gallery, "Semua capture", n(|c| c.all)),
                        (Collection::Favorites, Icon::Star, "Favorit", n(|c| c.favorites)),
                        (Collection::Images, Icon::Frame, "Gambar", n(|c| c.images)),
                        (Collection::Videos, Icon::Play, "Video", n(|c| c.videos)),
                    ];
                    for (col, icon, label, count) in rows {
                        if nav_row(ui, &pal, icon, label, count, cur == col).clicked() {
                            self.filter.set_collection(col);
                        }
                    }

                    ui.add_space(SP_3);
                    Self::section(ui, &pal, "Tanggal");
                    let cur = self.filter.date_preset(today);
                    let rows = [
                        (DatePreset::Today, "Hari ini", n(|c| c.today)),
                        (DatePreset::Yesterday, "Kemarin", n(|c| c.yesterday)),
                        (DatePreset::Last7, "7 hari terakhir", n(|c| c.last7)),
                    ];
                    for (preset, label, count) in rows {
                        if nav_row(ui, &pal, Icon::Calendar, label, count, cur == Some(preset)).clicked() {
                            // Clicking the active preset clears the date filter.
                            let next = if cur == Some(preset) { None } else { Some(preset) };
                            self.filter.set_date_preset(next, today);
                        }
                    }
                    self.date_row(ui, today);
                    if let Some(e) = &self.filter_error {
                        ui.add_space(SP_1);
                        ui.colored_label(pal.danger, e);
                    }
                });
            });
    }

    /// Turn a changed [`Filter`] into a new query.
    fn apply_filter(&mut self) {
        if self.filter == self.applied {
            return;
        }
        self.applied = self.filter.clone();
        match self.filter.to_query() {
            Ok(q) => {
                self.filter_error = None;
                self.query = q.clone();
                self.list.load(&*self.source, q);
                self.selected = None;
                self.sel_entry = None;
                self.start_scan();
            }
            Err(e) => self.filter_error = Some(e),
        }
    }

    // ---- right detail panel

    fn detail_panel(&mut self, ctx: &egui::Context, actions: &mut Vec<CellAction>) {
        let pal = self.pal;
        let sel = self.sel_entry.clone();
        let tex = sel.as_ref().and_then(|e| self.textures.get(&e.id).cloned());
        let failed = sel.as_ref().is_some_and(|e| self.failed.contains(&e.id));
        egui::SidePanel::right("detail")
            .exact_width(RIGHT_W)
            .resizable(false)
            .frame(egui::Frame::none().fill(pal.surface).inner_margin(Margin::same(SP_4)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 10.0;
                    widgets::section_header(ui, &pal, "Detail");
                    let Some(e) = &sel else {
                        ui.label(egui::RichText::new("Pilih capture untuk melihat detail dan aksinya.").size(TEXT_SMALL).color(pal.muted));
                        return;
                    };
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 176.0), Sense::hover());
                    paint_thumb(ui, &pal, rect, e, tex.as_ref(), failed);
                    ui.add(egui::Label::new(egui::RichText::new(file_name(e)).size(TEXT_BODY).color(pal.text)).truncate());

                    ui.scope(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        if e.width > 0 && e.height > 0 {
                            kv_row(ui, &pal, "Ukuran", &format!("{} × {} px", e.width, e.height), None);
                        }
                        kv_row(ui, &pal, "Mode", &source_label(&e.source), None);
                        let ext = e.path.extension().map(|x| x.to_string_lossy().to_uppercase()).unwrap_or_default();
                        kv_row(ui, &pal, "File", &format!("{ext} · {}", human_bytes(e.bytes)), None);
                        kv_row(ui, &pal, "Diambil", &when_text(e.created_at), None);
                        let dir = e.path.parent().map(|p| p.display().to_string());
                        kv_row(ui, &pal, "Lokasi", &tail_dir(&e.path), dir);
                    });

                    ui.add_space(SP_2);
                    widgets::section_header(ui, &pal, "Aksi");
                    ui.spacing_mut().item_spacing.y = 6.0;
                    let image = e.kind == EntryKind::Image;
                    // Enter opens in the default viewer; the editor is on E.
                    if image {
                        if action_button(ui, &pal, "Buka di editor", "E", BtnKind::Primary).clicked() {
                            actions.push(CellAction::Editor(e.clone()));
                        }
                        if action_button(ui, &pal, "Buka", "Enter", BtnKind::Normal).clicked() {
                            actions.push(CellAction::Open(e.clone()));
                        }
                        if action_button(ui, &pal, "Copy gambar", "Ctrl+C", BtnKind::Normal).clicked() {
                            actions.push(CellAction::Copy(e.clone()));
                        }
                    } else if action_button(ui, &pal, "Buka", "Enter", BtnKind::Primary).clicked() {
                        actions.push(CellAction::Open(e.clone()));
                    }
                    if action_button(ui, &pal, "Copy path", "Ctrl+Shift+C", BtnKind::Normal).clicked() {
                        actions.push(CellAction::CopyPath(e.clone()));
                    }
                    if action_button(ui, &pal, "Tampilkan di folder", "Ctrl+Shift+O", BtnKind::Normal).clicked() {
                        actions.push(CellAction::Folder(e.clone()));
                    }
                    let fav = if e.favorite { "Hapus favorit" } else { "Favorit" };
                    if action_button(ui, &pal, fav, "F", BtnKind::Normal).clicked() {
                        actions.push(CellAction::ToggleFavorite(e.clone()));
                    }
                    if action_button(ui, &pal, "Hapus", "Del", BtnKind::Danger).clicked() {
                        actions.push(CellAction::Delete(e.clone()));
                    }
                });
            });
    }

    // ---- status bar

    fn status_bar(&mut self, ctx: &egui::Context) {
        let pal = self.pal;
        let total = self.list.total();
        let filtered = self.filter != Filter::default();
        let summary = match self.stats {
            Some(c) if filtered => {
                format!("{} dari {} capture · {}", thousands(total), thousands(c.all), human_bytes(c.bytes))
            }
            Some(c) => format!("{} capture · {}", thousands(c.all), human_bytes(c.bytes)),
            None => format!("{} capture", thousands(total)),
        };
        let missing = self.stats.map(|c| c.missing).unwrap_or(0);
        let cleanup_text = cleanup_state(&self.rules);
        let has_sel = self.selected.is_some();
        let (mut do_cleanup, mut do_prune, mut do_settings) = (false, false, false);
        egui::TopBottomPanel::bottom("status")
            .exact_height(STATUS_H)
            .frame(egui::Frame::none().fill(pal.surface).inner_margin(Margin::symmetric(14.0, 0.0)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = SP_4;
                    let muted = |t: &str| egui::RichText::new(t.to_owned()).size(TEXT_SMALL).color(pal.muted);
                    ui.label(muted(&summary));
                    if has_sel {
                        ui.label(muted("1 terpilih"));
                    }
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = SP_1;
                        ui.label(muted(&cleanup_text));
                        ui.label(muted("·"));
                        do_settings = link(ui, &pal, "Atur").clicked();
                        ui.label(muted("·"));
                        do_cleanup = link(ui, &pal, "Bersihkan sekarang").clicked();
                    });
                    if missing > 0 {
                        do_prune = link(ui, &pal, &format!("Buang {missing} entri hilang")).clicked();
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let hints = "Enter buka · Ctrl+C copy · F favorit · Del hapus";
                        if ui.available_width() > measure(ui, hints, &prop(TEXT_SMALL)) + 24.0 {
                            ui.label(muted(hints));
                        }
                    });
                });
            });
        if do_cleanup {
            self.cleanup();
        }
        if do_prune {
            self.prune();
        }
        if do_settings {
            let r = std::env::current_exe().and_then(|exe| Command::new(exe).arg("--settings").spawn());
            if let Err(e) = r {
                self.notify(false, format!("Gagal membuka pengaturan: {e}"));
            }
        }
    }

    // ---- grid

    fn grid(&mut self, ui: &mut Ui, actions: &mut Vec<CellAction>) {
        let pal = self.pal;
        let size = self.size;
        // Inset keeps the selection ring of edge cards inside the scroll clip.
        const INSET: f32 = 4.0;
        let avail = ui.available_width() - ui.spacing().scroll.bar_width - 2.0 * INSET;
        let cols = grid_columns(avail, size.target_w(), GAP);
        if self.layout.cols != cols || self.layout_dirty {
            self.layout = GridLayout::build(&self.runs, cols, HEADER_H, size.row_h());
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

        let card_w = card_width(avail, cols, GAP);
        let (card_h, row_h, thumb_h) = (size.card_h(), size.row_h(), size.thumb_h());
        let today = Local::now().date_naive();
        let scroll_to = std::mem::take(&mut self.scroll_to_selected);
        let mut wanted = Vec::new();
        let sel_entry = self.sel_entry.clone();
        let Self { source, list, textures, failed, layout, runs, selected, .. } = self;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show_viewport(ui, |ui, viewport| {
            let origin = ui.max_rect().min + vec2(INSET, 0.0);
            ui.allocate_space(vec2(avail, layout.height + 16.0));
            if scroll_to {
                if let Some((_, top, h)) = selected.and_then(|s| layout.row_of(s)) {
                    let r = Rect::from_min_size(pos2(origin.x, origin.y + top), vec2(avail, h - GAP));
                    ui.scroll_to_rect(r, None);
                    ui.ctx().request_repaint();
                }
            }
            for ri in layout.visible(viewport.min.y - row_h, viewport.max.y + row_h) {
                let top = origin.y + layout.tops[ri];
                match &layout.rows[ri] {
                    Row::Header { run, .. } => {
                        if let Some(r) = runs.get(*run) {
                            let y = top + HEADER_H - 10.0;
                            let label = day_label(r.day, today).to_uppercase();
                            let rc = ui.painter().text(pos2(origin.x + 2.0, y), Align2::LEFT_BOTTOM, label, prop(TEXT_SMALL), pal.muted);
                            ui.painter().text(
                                pos2(rc.right() + 10.0, y),
                                Align2::LEFT_BOTTOM,
                                format!("{} capture", thousands(r.count)),
                                prop(TEXT_SMALL),
                                pal.muted.gamma_multiply(0.75),
                            );
                        }
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
                            let rect = Rect::from_min_size(pos2(x, top), vec2(card_w, card_h));
                            let out = draw_card(ui, &pal, rect, thumb_h, &e, tex, has_failed, *selected == Some(idx));
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
        // The detail panel's preview may be off-screen (keyboard jump).
        if let Some(e) = sel_entry {
            if !self.textures.contains(&e.id) && !self.failed.contains(&e.id) && !wanted.iter().any(|w| w.id == e.id) {
                wanted.push(e);
            }
        }
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
