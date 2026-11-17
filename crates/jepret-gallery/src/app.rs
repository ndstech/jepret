//! egui front-end: filter bar, virtual grid, actions, thumbnail workers.

use std::{
    collections::{HashSet, VecDeque},
    path::Path,
    process::Command,
    sync::{mpsc, Arc, Condvar, Mutex, MutexGuard},
};

use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Sense, TextureHandle, TextureOptions};
use jepret_history::{Entry, EntryKind};

use crate::{
    model::{grid_columns, grid_rows, Filter, KindFilter, LruCache, PagedList},
    source::{Source, Thumb},
};

const CELL_W: f32 = 200.0;
const THUMB_H: f32 = 130.0;
const CELL_H: f32 = 196.0;
const TEXTURE_CACHE: usize = 300;
const PAGE_CACHE: usize = 6;
const WORKERS: usize = 2;

pub fn run(backend: Result<Arc<dyn Source>, String>, recycle: bool) -> jepret_core::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Jepret - Galeri")
            .with_inner_size([1040.0, 720.0])
            .with_min_inner_size([420.0, 320.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Jepret - Galeri",
        options,
        Box::new(move |cc| Ok(Box::new(GalleryApp::new(cc, backend, recycle)))),
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

fn entry_menu(ui: &mut egui::Ui, e: &Entry) -> Option<CellAction> {
    let mut act = None;
    let mut item = |ui: &mut egui::Ui, label: &str, make: fn(Entry) -> CellAction| {
        if ui.button(label).clicked() {
            act = Some(make(e.clone()));
            ui.close_menu();
        }
    };
    item(ui, "Buka", CellAction::Open);
    if e.kind == EntryKind::Image {
        item(ui, "Buka di editor", CellAction::Editor);
        item(ui, "Copy gambar", CellAction::Copy);
    }
    item(ui, "Copy path", CellAction::CopyPath);
    item(ui, "Buka folder", CellAction::Folder);
    item(ui, if e.favorite { "Hapus favorit" } else { "Jadikan favorit" }, CellAction::ToggleFavorite);
    ui.separator();
    item(ui, "Hapus...", CellAction::Delete);
    act
}

fn draw_cell(ui: &mut egui::Ui, e: &Entry, tex: Option<&TextureHandle>, failed: bool) -> Option<CellAction> {
    let mut action = None;
    ui.allocate_ui(vec2(CELL_W, CELL_H), |ui| {
        ui.set_min_size(vec2(CELL_W, CELL_H));
        ui.set_max_width(CELL_W);
        let (rect, resp) = ui.allocate_exact_size(vec2(CELL_W, THUMB_H), Sense::click());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
        match tex {
            Some(t) => {
                let s = t.size_vec2();
                let k = (CELL_W / s.x).min(THUMB_H / s.y);
                let r = Rect::from_center_size(rect.center(), s * k);
                let tint = if e.missing { Color32::from_gray(90) } else { Color32::WHITE };
                p.image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
            }
            None => {
                let msg = if failed { "tak ada thumbnail" } else { "..." };
                p.text(rect.center(), Align2::CENTER_CENTER, msg, FontId::proportional(12.0), Color32::GRAY);
            }
        }
        if e.kind == EntryKind::Video {
            p.text(rect.left_top() + vec2(6.0, 4.0), Align2::LEFT_TOP, "VIDEO", FontId::proportional(11.0), Color32::WHITE);
        }
        if e.favorite {
            p.text(rect.right_top() + vec2(-6.0, 2.0), Align2::RIGHT_TOP, "★", FontId::proportional(16.0), Color32::GOLD);
        }
        if e.missing {
            p.text(rect.center_bottom() + vec2(0.0, -6.0), Align2::CENTER_BOTTOM, "FILE HILANG", FontId::proportional(12.0), Color32::LIGHT_RED);
        }
        if resp.double_clicked() {
            action = Some(CellAction::Open(e.clone()));
        }
        let resp = resp.on_hover_text(e.path.display().to_string());
        resp.context_menu(|ui| {
            if let Some(a) = entry_menu(ui, e) {
                action = Some(a);
            }
        });

        ui.add(egui::Label::new(file_name(e)).truncate());
        ui.horizontal(|ui| {
            if ui.small_button(if e.favorite { "★" } else { "☆" }).clicked() {
                action = Some(CellAction::ToggleFavorite(e.clone()));
            }
            ui.menu_button("Aksi", |ui| {
                if let Some(a) = entry_menu(ui, e) {
                    action = Some(a);
                }
            });
        });
    });
    action
}

// -------------------------------------------------------------------- app

struct Ready {
    source: Arc<dyn Source>,
    recycle: bool,
    filter: Filter,
    applied: Filter,
    filter_error: Option<String>,
    list: PagedList,
    textures: LruCache<i64, TextureHandle>,
    failed: HashSet<i64>,
    loader: ThumbLoader,
    status: String,
    confirm_delete: Option<Entry>,
}

enum Backend {
    Error(String),
    Ready(Box<Ready>),
}

pub struct GalleryApp {
    backend: Backend,
}

impl GalleryApp {
    fn new(cc: &eframe::CreationContext<'_>, backend: Result<Arc<dyn Source>, String>, recycle: bool) -> Self {
        let backend = match backend {
            Err(msg) => Backend::Error(msg),
            Ok(source) => {
                let mut list = PagedList::new(PAGE_CACHE);
                let filter = Filter::default();
                if let Ok(q) = filter.to_query() {
                    list.load(&*source, q);
                }
                Backend::Ready(Box::new(Ready {
                    loader: ThumbLoader::new(source.clone(), cc.egui_ctx.clone()),
                    source,
                    recycle,
                    applied: filter.clone(),
                    filter,
                    filter_error: None,
                    list,
                    textures: LruCache::new(TEXTURE_CACHE),
                    failed: HashSet::new(),
                    status: String::new(),
                    confirm_delete: None,
                }))
            }
        };
        Self { backend }
    }
}

impl eframe::App for GalleryApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        match &mut self.backend {
            Backend::Error(msg) => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add_space(24.0);
                    ui.heading("Galeri tidak bisa dibuka");
                    ui.add_space(8.0);
                    ui.colored_label(Color32::LIGHT_RED, msg.as_str());
                });
            }
            Backend::Ready(r) => r.update(ctx),
        }
    }
}

impl Ready {
    fn update(&mut self, ctx: &egui::Context) {
        self.poll_thumbs(ctx);

        egui::TopBottomPanel::top("filters").show(ctx, |ui| self.filter_bar(ui));
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("{} item", self.list.total()));
                if let Some(e) = &self.list.last_error {
                    ui.colored_label(Color32::LIGHT_RED, e);
                }
                if !self.status.is_empty() {
                    ui.separator();
                    ui.label(&self.status);
                }
            });
        });

        let mut actions = Vec::new();
        egui::CentralPanel::default().show(ctx, |ui| self.grid(ui, &mut actions));
        for a in actions {
            self.apply(ctx, a);
        }
        self.delete_dialog(ctx);
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

    fn filter_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.filter.search).hint_text("Cari nama file").desired_width(160.0));
            egui::ComboBox::from_id_salt("kind")
                .selected_text(match self.filter.kind {
                    KindFilter::All => "Semua",
                    KindFilter::Image => "Gambar",
                    KindFilter::Video => "Video",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.filter.kind, KindFilter::All, "Semua");
                    ui.selectable_value(&mut self.filter.kind, KindFilter::Image, "Gambar");
                    ui.selectable_value(&mut self.filter.kind, KindFilter::Video, "Video");
                });
            ui.checkbox(&mut self.filter.favorites_only, "Favorit");
            ui.add(egui::TextEdit::singleline(&mut self.filter.from).hint_text("dari YYYY-MM-DD").desired_width(110.0));
            ui.add(egui::TextEdit::singleline(&mut self.filter.to).hint_text("sampai YYYY-MM-DD").desired_width(110.0));
            ui.separator();
            if ui.button("Bersihkan sekarang").clicked() {
                self.cleanup();
            }
            if ui.button("Hapus entri hilang").clicked() {
                self.prune();
            }
        });
        if let Some(e) = &self.filter_error {
            ui.colored_label(Color32::LIGHT_RED, e);
        }
        ui.add_space(4.0);

        if self.filter != self.applied {
            self.applied = self.filter.clone();
            match self.filter.to_query() {
                Ok(q) => {
                    self.filter_error = None;
                    self.list.load(&*self.source, q);
                }
                Err(e) => self.filter_error = Some(e),
            }
        }
    }

    fn grid(&mut self, ui: &mut egui::Ui, actions: &mut Vec<CellAction>) {
        let spacing = ui.spacing().item_spacing;
        let cols = grid_columns(ui.available_width() - ui.spacing().scroll.bar_width, CELL_W, spacing.x);
        let rows = grid_rows(self.list.total(), cols);
        let mut wanted = Vec::new();
        let Self { source, list, textures, failed, .. } = self;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show_rows(ui, CELL_H, rows, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    for c in 0..cols {
                        let idx = (row * cols + c) as u64;
                        let Some(e) = list.get(&**source, idx) else { break };
                        let tex = textures.get(&e.id);
                        let has_failed = failed.contains(&e.id);
                        if tex.is_none() && !has_failed {
                            wanted.push(e.clone());
                        }
                        if let Some(a) = draw_cell(ui, &e, tex, has_failed) {
                            actions.push(a);
                        }
                    }
                });
            }
        });
        self.loader.set_wanted(wanted);
    }

    fn apply(&mut self, ctx: &egui::Context, a: CellAction) {
        match a {
            CellAction::Open(e) => {
                if let Err(err) = open_with_shell(&e.path) {
                    self.status = format!("Gagal membuka: {err}");
                }
            }
            CellAction::Editor(e) => {
                let r = std::env::current_exe().and_then(|exe| Command::new(exe).arg("--editor").arg(&e.path).spawn());
                if let Err(err) = r {
                    self.status = format!("Gagal membuka editor: {err}");
                }
            }
            CellAction::Copy(e) => {
                self.status = match copy_image(&e.path) {
                    Ok(()) => "Gambar disalin ke clipboard".into(),
                    Err(err) => format!("Gagal menyalin gambar: {err}"),
                };
            }
            CellAction::CopyPath(e) => {
                ctx.copy_text(e.path.display().to_string());
                self.status = "Path disalin".into();
            }
            CellAction::Folder(e) => {
                if let Err(err) = reveal_in_folder(&e.path) {
                    self.status = format!("Gagal membuka folder: {err}");
                }
            }
            CellAction::ToggleFavorite(e) => match self.source.set_favorite(e.id, !e.favorite) {
                Ok(()) => self.list.refresh(&*self.source),
                Err(err) => self.status = format!("Gagal mengubah favorit: {err:#}"),
            },
            CellAction::Delete(e) => self.confirm_delete = Some(e),
        }
    }

    fn delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(e) = self.confirm_delete.clone() else { return };
        let mut decision = None;
        egui::Window::new("Hapus item?")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("Hapus {} dari riwayat dan dari disk?", file_name(&e)));
                if self.recycle {
                    ui.label("File dipindahkan ke Recycle Bin.");
                } else {
                    ui.colored_label(Color32::LIGHT_RED, "File dihapus permanen.");
                }
                ui.horizontal(|ui| {
                    if ui.button("Hapus").clicked() {
                        decision = Some(true);
                    }
                    if ui.button("Batal").clicked() {
                        decision = Some(false);
                    }
                });
            });
        match decision {
            Some(true) => {
                self.status = match self.source.delete(e.id, true, self.recycle) {
                    Ok(()) => format!("Dihapus: {}", file_name(&e)),
                    Err(err) => format!("Gagal menghapus: {err:#}"),
                };
                self.confirm_delete = None;
                self.list.refresh(&*self.source);
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
        self.status = match self.source.cleanup(now) {
            Ok(r) => {
                let mut s = format!(
                    "Cleanup: {} file dihapus, {:.1} MB dibebaskan, {} entri dibuang",
                    r.files_deleted,
                    r.bytes_freed as f64 / 1_048_576.0,
                    r.entries_removed
                );
                if !r.errors.is_empty() {
                    s.push_str(&format!(", {} error ({})", r.errors.len(), r.errors[0]));
                }
                s
            }
            Err(e) => format!("Cleanup gagal: {e:#}"),
        };
        self.list.refresh(&*self.source);
    }

    fn prune(&mut self) {
        self.status = match self.source.prune_missing() {
            Ok(n) => format!("{n} entri hilang dibuang"),
            Err(e) => format!("Gagal: {e:#}"),
        };
        self.list.refresh(&*self.source);
    }
}
