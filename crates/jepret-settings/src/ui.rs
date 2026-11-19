//! egui front-end. All state lives in a draft `Config`; logic is in `model` / `hotkey`.
//! Look & feel comes from `jepret-ui-theme` (artboard "Settings — tab Shortcut").

use std::{collections::HashMap, path::PathBuf};

use egui::{
    Align, Align2, FontFamily, FontId, Layout, Margin, Rect, Response, RichText, Rounding, Sense, Shape, Stroke, Ui,
    UiBuilder, Vec2,
};
use jepret_core::{
    config::{HotkeyBinding, ImageFormat, ThemeMode},
    shortcuts::EDITOR_SHORTCUTS,
    Action, CaptureKind, CommandSpec, Config, KeyCombo,
};
use jepret_ui_theme::{
    icons,
    tokens::{self, Palette},
    widgets::{self, BannerKind},
    Icon,
};
use jepret_workflow::OpenWithCandidate;

use crate::{
    hotkey, keyhook,
    model::{self, Issue, Severity},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Hotkeys,
    Shortcuts,
    Apps,
    History,
    Recording,
    Capture,
}

const TABS: [(Tab, Icon, &str); 7] = [
    (Tab::General, Icon::Settings, "Umum"),
    (Tab::Hotkeys, Icon::Workflow, "Hotkey & workflow"),
    (Tab::Shortcuts, Icon::Keyboard, "Shortcut"),
    (Tab::Apps, Icon::OpenExternal, "Aplikasi eksternal"),
    (Tab::History, Icon::Trash, "Riwayat & cleanup"),
    (Tab::Recording, Icon::Record, "Rekam layar"),
    (Tab::Capture, Icon::Frame, "Capture"),
];

const CAPTURE_KINDS: [(CaptureKind, &str); 8] = [
    (CaptureKind::Fullscreen, "Seluruh layar (semua monitor)"),
    (CaptureKind::ActiveMonitor, "Monitor aktif"),
    (CaptureKind::ActiveWindow, "Window aktif"),
    (CaptureKind::Region, "Region (overlay)"),
    (CaptureKind::LastRegion, "Region terakhir"),
    (CaptureKind::Scroll, "Scroll capture"),
    (CaptureKind::RecordRegion, "Rekam region"),
    (CaptureKind::RecordMonitor, "Rekam monitor aktif"),
];

fn kind_label(k: CaptureKind) -> &'static str {
    CAPTURE_KINDS.iter().find(|(c, _)| *c == k).map(|(_, l)| *l).unwrap_or("?")
}

#[derive(Clone, PartialEq, Eq)]
enum RecTarget {
    /// Replace the combo of global hotkey `i`.
    Global(usize),
    /// Add a combo to editor shortcut `id`.
    EditorAdd(String),
}

pub struct SettingsApp {
    path: PathBuf,
    cfg: Config,
    saved: Config,
    tab: Tab,
    status: Option<(bool, String)>,
    /// What the key recorder is currently capturing for.
    recording: Option<RecTarget>,
    record_msg: Option<String>,
    /// Low-level keyboard hook; exists only while recording.
    hook: Option<keyhook::Recorder>,
    search: String,
    /// Selected editor shortcut group chip (index into the group list).
    group: usize,
    /// Text buffers for args lines, keyed by a stable-ish id; cleared on structural edits.
    bufs: HashMap<String, String>,
    candidates: Option<(Vec<OpenWithCandidate>, Option<String>)>,
    pal: Palette,
    applied_theme: ThemeMode,
}

impl SettingsApp {
    pub fn new(path: PathBuf, cfg: Config) -> Self {
        let theme = cfg.general.theme;
        Self {
            path,
            saved: cfg.clone(),
            cfg,
            tab: Tab::General,
            status: None,
            recording: None,
            record_msg: None,
            hook: None,
            search: String::new(),
            group: 0,
            bufs: HashMap::new(),
            candidates: None,
            pal: Palette::for_mode(theme),
            applied_theme: theme,
        }
    }

    /// Load fonts and apply the visuals for the current theme (call once at startup).
    pub fn install_theme(&mut self, ctx: &egui::Context) {
        self.applied_theme = self.cfg.general.theme;
        self.pal = jepret_ui_theme::install(ctx, self.applied_theme);
    }

    fn save(&mut self) {
        let mut cfg = self.cfg.clone();
        model::normalize_config(&mut cfg);
        if model::has_errors(&model::validate(&cfg)) {
            self.status = Some((false, "Tidak disimpan: perbaiki error dulu.".into()));
            return;
        }
        match cfg.save(&self.path) {
            Ok(()) => {
                self.cfg = cfg.clone();
                self.saved = cfg;
                self.bufs.clear();
                self.status = Some((true, format!("Tersimpan: {}", self.path.display())));
            }
            Err(e) => self.status = Some((false, format!("Gagal menyimpan: {e}"))),
        }
    }

    fn start_recording(&mut self, t: RecTarget) {
        self.hook = None;
        self.hook = keyhook::Recorder::start();
        self.recording = Some(t);
        self.record_msg = None;
    }

    fn stop_recording(&mut self) {
        self.hook = None; // drop = unhook
        self.recording = None;
        self.record_msg = None;
    }

    fn apply_combo(&mut self, combo: KeyCombo) {
        let Some(t) = self.recording.clone() else { return };
        match t {
            RecTarget::Global(i) => {
                if let Err(e) = hotkey::check_global(&combo) {
                    self.record_msg = Some(e);
                    return;
                }
                if let Some(b) = self.cfg.hotkeys.get_mut(i) {
                    b.keys = combo.to_string();
                }
            }
            RecTarget::EditorAdd(id) => {
                let mut list = model::editor_effective(&self.cfg, &id);
                list.push(combo.to_string());
                model::set_editor_combos(&mut self.cfg, &id, &list);
            }
        }
        self.stop_recording();
    }

    fn poll_recording(&mut self, ctx: &egui::Context) {
        if self.recording.is_none() {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(60));
        if let Some(h) = &self.hook {
            match h.poll() {
                Some(keyhook::Outcome::Combo(c)) => self.apply_combo(c),
                Some(keyhook::Outcome::Cancelled) => self.stop_recording(),
                Some(keyhook::Outcome::Unsupported(m)) => self.record_msg = Some(m),
                None => {}
            }
            return;
        }
        // Fallback when the hook could not be installed: egui events (no PrtSc / Win).
        let events = ctx.input(|i| i.events.clone());
        for ev in events {
            let egui::Event::Key { key, pressed: true, modifiers, repeat: false, .. } = ev else { continue };
            if key == egui::Key::Escape && !modifiers.any() {
                self.stop_recording();
                return;
            }
            let name = match format!("{key:?}").as_str() {
                n if n.starts_with("Num") && n.len() == 4 => n[3..].to_string(),
                "Backtick" => "backquote".into(),
                "OpenBracket" => "bracketleft".into(),
                "CloseBracket" => "bracketright".into(),
                "Equals" => "plus".into(),
                n => n.to_string(),
            };
            let mut c = match KeyCombo::parse(&name) {
                Ok(c) => c,
                Err(e) => {
                    self.record_msg = Some(e.to_string());
                    return;
                }
            };
            c.ctrl = modifiers.ctrl;
            c.alt = modifiers.alt;
            c.shift = modifiers.shift;
            self.apply_combo(c);
            return;
        }
    }

    fn nav(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        ui.spacing_mut().item_spacing.y = 2.0;
        for (t, icon, label) in TABS {
            let on = self.tab == t;
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::click());
            let p = ui.painter();
            if on {
                p.rect_filled(rect, tokens::RADIUS_CONTROL, pal.canvas);
                let bar = Rect::from_min_size(rect.min + Vec2::new(0.0, 10.0), Vec2::new(3.0, 16.0));
                p.rect_filled(bar, 2.0, pal.accent);
            } else if resp.hovered() {
                p.rect_filled(rect, tokens::RADIUS_CONTROL, pal.raised);
            }
            let ic = Rect::from_center_size(egui::pos2(rect.left() + 12.0 + 8.0, rect.center().y), Vec2::splat(16.0));
            icons::paint(p, ic, icon, pal.text, 1.75);
            p.text(
                egui::pos2(rect.left() + 12.0 + 16.0 + 10.0, rect.center().y),
                Align2::LEFT_CENTER,
                label,
                FontId::new(tokens::TEXT_BODY, FontFamily::Proportional),
                pal.text,
            );
            if resp.clicked() {
                self.tab = t;
            }
        }
    }

    fn footer(&mut self, ui: &mut Ui, issues: &[Issue], dirty: bool, blocked: bool) {
        let pal = self.pal;
        let full = ui.max_rect();
        ui.painter().hline(full.x_range(), full.top(), Stroke::new(1.0_f32, pal.line));
        let r = full.shrink2(Vec2::new(32.0, 0.0));

        let (dot, text) = if blocked {
            let first = issues.iter().find(|i| i.severity == Severity::Error).map(|i| i.message.as_str());
            (pal.danger, format!("Tidak bisa disimpan · {}", first.unwrap_or("ada error")))
        } else if dirty {
            let mut t = String::from("Perubahan belum disimpan");
            if let Some((false, m)) = &self.status {
                t = format!("{t} · {m}");
            } else if let Some(i) = issues.first() {
                t = format!("{t} · {}", i.message);
            }
            (tokens::FAVORITE, t)
        } else if let Some((ok, m)) = &self.status {
            (if *ok { pal.accent } else { pal.danger }, m.clone())
        } else {
            (pal.accent, "Tidak ada perubahan".to_string())
        };
        let tip: Vec<String> = issues.iter().map(|i| i.message.clone()).collect();

        // Buttons (right-aligned).
        let mut do_save = false;
        let mut do_cancel = false;
        cell(ui, r, true, |ui| {
            let can_save = dirty && !blocked;
            let saved = ui
                .scope(|ui| {
                    if !can_save {
                        ui.disable();
                    }
                    widgets::primary_button(ui, &pal, "Simpan", Some("Ctrl+S"))
                })
                .inner;
            do_save = saved.clicked() && can_save;
            let cancel = ui
                .scope(|ui| {
                    if !dirty {
                        ui.disable();
                    }
                    widgets::button(ui, &pal, None, "Batalkan", None)
                })
                .inner;
            do_cancel = cancel.clicked() && dirty;
        });
        // Status (left).
        let left = Rect::from_min_max(r.min, egui::pos2((r.right() - 240.0).max(r.left()), r.bottom()));
        cell(ui, left, false, |ui| {
            let (d, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
            ui.painter().circle_filled(d.center(), 4.0, dot);
            let l = ui.add(egui::Label::new(RichText::new(text).color(pal.muted)).truncate());
            if tip.len() > 1 {
                l.on_hover_text(tip.join("\n"));
            }
        });
        if do_save {
            self.save();
        }
        if do_cancel {
            self.cfg = self.saved.clone();
            self.bufs.clear();
            self.stop_recording();
        }
    }
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.cfg.general.theme != self.applied_theme {
            // Live preview of the chosen theme.
            self.applied_theme = self.cfg.general.theme;
            self.pal = Palette::for_mode(self.applied_theme);
            jepret_ui_theme::apply(ctx, &self.pal);
        }
        let pal = self.pal;
        self.poll_recording(ctx);
        let tab_before = self.tab;
        let mut preview = self.cfg.clone();
        model::normalize_config(&mut preview);
        let issues = model::validate(&preview);
        let dirty = self.cfg != self.saved;
        let blocked = model::has_errors(&issues);

        if ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::CTRL, egui::Key::S)))
            && dirty
            && !blocked
        {
            self.save();
        }

        egui::SidePanel::left("nav")
            .exact_width(240.0)
            .resizable(false)
            .frame(egui::Frame::none().fill(pal.bg).inner_margin(Margin::symmetric(12.0, 8.0)))
            .show(ctx, |ui| self.nav(ui));

        egui::TopBottomPanel::bottom("footer")
            .exact_height(60.0)
            .resizable(false)
            .frame(egui::Frame::none().fill(pal.surface))
            .show(ctx, |ui| self.footer(ui, &issues, dirty, blocked));

        if self.tab != tab_before {
            self.stop_recording();
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(pal.surface).rounding(Rounding { nw: tokens::RADIUS_PANEL, ..Rounding::ZERO }))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    egui::Frame::none().inner_margin(Margin::symmetric(32.0, 24.0)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 10.0;
                        match self.tab {
                            Tab::General => {
                                title(ui, "Umum");
                                general_tab(ui, &pal, &mut self.cfg);
                            }
                            Tab::Hotkeys => {
                                title(ui, "Hotkey & workflow");
                                self.hotkeys_tab(ui);
                            }
                            Tab::Shortcuts => self.shortcuts_tab(ui),
                            Tab::Apps => {
                                title(ui, "Aplikasi eksternal");
                                self.apps_tab(ui);
                            }
                            Tab::History => {
                                title(ui, "Riwayat & cleanup");
                                history_tab(ui, &pal, &mut self.cfg);
                            }
                            Tab::Recording => {
                                title(ui, "Rekam layar");
                                self.recording_tab(ui);
                            }
                            Tab::Capture => {
                                title(ui, "Capture");
                                capture_tab(ui, &pal, &mut self.cfg);
                            }
                        }
                    });
                });
            });
    }
}

// ---------- small design helpers ----------

fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(tokens::TEXT_TITLE).strong());
}

fn h2(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(tokens::TEXT_SECTION).strong());
}

fn muted(ui: &mut Ui, pal: &Palette, text: &str) {
    ui.label(RichText::new(text).size(tokens::TEXT_SMALL).color(pal.muted));
}

fn section(ui: &mut Ui, pal: &Palette, text: &str) {
    ui.add_space(6.0);
    widgets::section_header(ui, pal, text);
}

fn text_size(ui: &Ui, text: &str, font: &FontId) -> Vec2 {
    ui.fonts(|f| f.layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE).size())
}

/// Child UI occupying exactly `rect`, children centred vertically.
fn cell<R>(ui: &mut Ui, rect: Rect, right: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
    let layout = if right { Layout::right_to_left(Align::Center) } else { Layout::left_to_right(Align::Center) };
    let mut c = ui.new_child(UiBuilder::new().max_rect(rect).layout(layout));
    c.set_clip_rect(rect.intersect(ui.clip_rect()));
    c.spacing_mut().item_spacing.x = 6.0;
    add(&mut c)
}

/// Mockup grid: label 220 / combos 1fr / actions 220, 12 px gaps, 10 px side padding.
fn row_rects(ui: &mut Ui, h: f32) -> (Rect, [Rect; 3]) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
    let inner = rect.shrink2(Vec2::new(10.0, 0.0));
    let gap = 12.0;
    let side = 220.0_f32.min(inner.width() * 0.28);
    let mid = (inner.width() - 2.0 * side - 2.0 * gap).max(0.0);
    let l = Rect::from_min_size(inner.min, Vec2::new(side, h));
    let m = Rect::from_min_size(egui::pos2(l.right() + gap, inner.top()), Vec2::new(mid, h));
    let r = Rect::from_min_size(egui::pos2(m.right() + gap, inner.top()), Vec2::new(side, h));
    (rect, [l, m, r])
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Chip {
    Outline,
    Ghost,
    /// Ghost with accent text.
    GhostAccent,
    Dashed,
}

fn chip_button(ui: &mut Ui, pal: &Palette, label: &str, h: f32, kind: Chip) -> Response {
    let font = FontId::new(tokens::TEXT_SMALL, FontFamily::Proportional);
    let w = text_size(ui, label, &font).x + 20.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let hov = resp.hovered() && ui.is_enabled();
    let p = ui.painter();
    let r = 6.0;
    let fg = match kind {
        Chip::Outline => pal.text,
        Chip::Ghost | Chip::Dashed => pal.muted,
        Chip::GhostAccent => pal.accent,
    };
    match kind {
        Chip::Outline => {
            p.rect(rect, r, if hov { pal.raised } else { pal.surface }, Stroke::new(1.0_f32, pal.line_strong));
        }
        Chip::Ghost | Chip::GhostAccent => {
            if hov {
                p.rect_filled(rect, r, pal.raised);
            }
        }
        Chip::Dashed => {
            if hov {
                p.rect_filled(rect, r, pal.raised);
            }
            let pts = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
            p.extend(Shape::dashed_line(&pts, Stroke::new(1.0_f32, pal.line_strong), 3.0, 3.0));
        }
    }
    p.text(rect.center(), Align2::CENTER_CENTER, label, font, fg);
    resp
}

/// 28 px outlined button; returns clicked.
fn cbtn(ui: &mut Ui, pal: &Palette, label: &str) -> bool {
    chip_button(ui, pal, label, 28.0, Chip::Outline).clicked()
}

/// 24 px outlined button; returns clicked.
fn sbtn(ui: &mut Ui, pal: &Palette, label: &str) -> bool {
    chip_button(ui, pal, label, 24.0, Chip::Outline).clicked()
}

fn sbtn_en(ui: &mut Ui, pal: &Palette, enabled: bool, label: &str) -> bool {
    ui.add_enabled_ui(enabled, |ui| sbtn(ui, pal, label)).inner
}

fn pill(ui: &mut Ui, pal: &Palette, label: &str, active: bool) -> Response {
    let font = FontId::new(tokens::TEXT_SMALL, FontFamily::Proportional);
    let w = text_size(ui, label, &font).x + 24.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 28.0), Sense::click());
    let (fill, border, fg) = if active {
        (pal.text, pal.text, pal.surface)
    } else {
        (if resp.hovered() { pal.raised } else { pal.surface }, pal.line_strong, pal.text)
    };
    ui.painter().rect(rect, 14.0, fill, Stroke::new(1.0_f32, border));
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, fg);
    resp
}

/// Key chip (12 px mono, thick bottom border). `bad` = conflict/invalid look.
fn kbd_chip(ui: &mut Ui, pal: &Palette, text: &str, bad: bool, clickable: bool) -> Response {
    let font = FontId::new(tokens::TEXT_SMALL, FontFamily::Monospace);
    let size = text_size(ui, text, &font) + Vec2::new(16.0, 6.0);
    let (rect, resp) = ui.allocate_exact_size(size, if clickable { Sense::click() } else { Sense::hover() });
    let (fill, border, fg) = if bad {
        (pal.surface, pal.danger.gamma_multiply(0.5), pal.danger)
    } else {
        (pal.kbd_bg, pal.line_strong, pal.text)
    };
    let border = if clickable && resp.hovered() { pal.danger } else { border };
    let p = ui.painter();
    p.rect_filled(rect.translate(Vec2::new(0.0, 1.0)), 6.0, border);
    p.rect(rect, 6.0, fill, Stroke::new(1.0_f32, border));
    p.text(rect.center(), Align2::CENTER_CENTER, text, font, fg);
    resp
}

/// "Tekan kombinasi… · Esc batal" chip shown while recording.
fn rec_chip(ui: &mut Ui, pal: &Palette) {
    egui::Frame::none()
        .fill(pal.accent_soft)
        .stroke(Stroke::new(2.0_f32, pal.accent))
        .rounding(6.0)
        .inner_margin(Margin::symmetric(10.0, 3.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, pal.danger);
                ui.label(RichText::new("Tekan kombinasi… · Esc batal").size(tokens::TEXT_SMALL).strong().color(pal.accent));
            });
        });
}

fn card<R>(ui: &mut Ui, pal: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::none()
        .fill(pal.bg)
        .stroke(Stroke::new(1.0_f32, pal.line))
        .rounding(tokens::RADIUS_CONTROL + 2.0)
        .inner_margin(Margin::same(12.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn tgl(ui: &mut Ui, pal: &Palette, v: &mut bool, label: &str) {
    widgets::toggle(ui, pal, v, label, None);
}

// ---------- helpers ----------

fn path_edit(ui: &mut Ui, pal: &Palette, p: &mut PathBuf, folder: bool) {
    let mut s = p.to_string_lossy().into_owned();
    if ui.add(egui::TextEdit::singleline(&mut s).desired_width(360.0)).changed() {
        *p = PathBuf::from(s);
    }
    if cbtn(ui, pal, "Pilih...") {
        let d = rfd::FileDialog::new();
        let picked = if folder {
            d.set_directory(&*p).pick_folder()
        } else {
            d.add_filter("Program", &["exe", "bat", "cmd", "com"]).pick_file()
        };
        if let Some(x) = picked {
            *p = x;
        }
    }
    if !p.as_os_str().is_empty() && !p.exists() {
        ui.label(RichText::new("tidak ada").color(pal.warn_text));
    }
}

fn opt_num<T: egui::emath::Numeric>(
    ui: &mut Ui,
    pal: &Palette,
    label: &str,
    v: &mut Option<T>,
    default: T,
    range: std::ops::RangeInclusive<T>,
) {
    ui.horizontal(|ui| {
        let mut on = v.is_some();
        if widgets::toggle(ui, pal, &mut on, label, None).changed() {
            *v = on.then_some(default);
        }
        if let Some(x) = v {
            ui.add(egui::DragValue::new(x).range(range));
        }
    });
}

fn labeled(ui: &mut Ui, pal: &Palette, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(pal.muted));
        add(ui);
    });
}

// ---------- tabs ----------

fn general_tab(ui: &mut Ui, pal: &Palette, cfg: &mut Config) {
    section(ui, pal, "Tampilan");
    labeled(ui, pal, "Tema:", |ui| {
        let mut idx = match cfg.general.theme {
            ThemeMode::System => 0,
            ThemeMode::Light => 1,
            ThemeMode::Dark => 2,
        };
        if widgets::segmented(ui, pal, &mut idx, &["Ikuti Windows", "Terang", "Gelap"]).changed() {
            cfg.general.theme = match idx {
                1 => ThemeMode::Light,
                2 => ThemeMode::Dark,
                _ => ThemeMode::System,
            };
        }
    });
    let g = &mut cfg.general;
    section(ui, pal, "Penyimpanan");
    labeled(ui, pal, "Folder output:", |ui| path_edit(ui, pal, &mut g.output_dir, true));
    labeled(ui, pal, "Pola nama file:", |ui| {
        ui.text_edit_singleline(&mut g.filename_pattern);
        muted(ui, pal, ".ext otomatis; placeholder: {datetime} {date} {time}");
    });
    tgl(ui, pal, &mut g.monthly_subfolders, "Subfolder per bulan (yyyy-mm)");
    labeled(ui, pal, "Format gambar:", |ui| {
        let mut idx = usize::from(g.image_format == ImageFormat::Jpeg);
        if widgets::segmented(ui, pal, &mut idx, &["PNG", "JPEG"]).changed() {
            g.image_format = if idx == 1 { ImageFormat::Jpeg } else { ImageFormat::Png };
        }
    });
    if g.image_format == ImageFormat::Jpeg {
        labeled(ui, pal, "Kualitas JPEG:", |ui| {
            ui.add(egui::Slider::new(&mut g.jpeg_quality, 1..=100));
        });
    }
    section(ui, pal, "Perilaku");
    tgl(ui, pal, &mut g.play_sound, "Bunyi saat capture");
    tgl(ui, pal, &mut g.start_with_windows, "Jalankan saat Windows start");
    editor_section(ui, pal, cfg);
}

fn editor_section(ui: &mut Ui, pal: &Palette, cfg: &mut Config) {
    section(ui, pal, "Editor");
    let e = &mut cfg.editor;
    tgl(ui, pal, &mut e.show_shortcut_hints, "Tampilkan hint shortcut di tooltip dan menu");
    labeled(ui, pal, "Tool awal:", |ui| {
        let cur = EDITOR_SHORTCUTS
            .iter()
            .find(|s| s.id == e.default_tool)
            .map(|s| s.label)
            .unwrap_or(e.default_tool.as_str())
            .to_string();
        egui::ComboBox::from_id_salt("default_tool").selected_text(cur).show_ui(ui, |ui| {
            for d in EDITOR_SHORTCUTS.iter().filter(|d| d.id.starts_with("tool.")) {
                ui.selectable_value(&mut e.default_tool, d.id.to_string(), d.label);
            }
        });
    });
    muted(ui, pal, "Palet warna (tombol 1-9):");
    ui.horizontal_wrapped(|ui| {
        for (i, hex) in e.palette.iter_mut().enumerate() {
            let rgba = model::parse_hex_color(hex).unwrap_or([128, 128, 128, 255]);
            let mut c = egui::Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3]);
            ui.vertical(|ui| {
                ui.label(format!("{}", i + 1));
                if ui.color_edit_button_srgba(&mut c).changed() {
                    *hex = model::format_hex_color(c.to_srgba_unmultiplied());
                }
            });
        }
    });
}

fn history_tab(ui: &mut Ui, pal: &Palette, cfg: &mut Config) {
    let h = &mut cfg.history;
    tgl(ui, pal, &mut h.enabled, "Catat history capture");
    section(ui, pal, "Cleanup otomatis");
    let c = &mut h.cleanup;
    tgl(ui, pal, &mut c.enabled, "Aktifkan cleanup");
    ui.add_enabled_ui(c.enabled, |ui| {
        opt_num(ui, pal, "Hapus lebih tua dari (hari):", &mut c.max_age_days, 30, 1..=36500);
        opt_num(ui, pal, "Batas total ukuran (MB):", &mut c.max_total_mb, 5000, 1..=10_000_000);
        tgl(ui, pal, &mut c.keep_favorites, "Jangan hapus favorit");
        tgl(ui, pal, &mut c.use_recycle_bin, "Pindahkan ke Recycle Bin (bukan hapus permanen)");
        tgl(ui, pal, &mut c.run_on_startup, "Jalankan saat Jepret start");
    });
}

fn capture_tab(ui: &mut Ui, pal: &Palette, cfg: &mut Config) {
    section(ui, pal, "Capture");
    let c = &mut cfg.capture;
    tgl(ui, pal, &mut c.show_cursor, "Sertakan kursor");
    tgl(ui, pal, &mut c.hdr_tonemap, "Tone-map monitor HDR ke SDR");
    opt_num(ui, pal, "Override SDR white level (nits):", &mut c.sdr_white_nits, 200.0_f32, 80.0..=1000.0);
    muted(ui, pal, "Kosongkan untuk memakai pengaturan Windows.");
    section(ui, pal, "Overlay region");
    tgl(ui, pal, &mut c.overlay_magnifier, "Tampilkan magnifier");
    tgl(ui, pal, &mut c.overlay_snap_to_windows, "Snap ke window saat hover");
    section(ui, pal, "Scroll capture");
    let s = &mut cfg.scroll;
    egui::Grid::new("scroll").num_columns(2).show(ui, |ui| {
        ui.label("Maks. frame:");
        ui.add(egui::DragValue::new(&mut s.max_frames).range(1..=1000));
        ui.end_row();
        ui.label("Jeda antar scroll (ms):");
        ui.add(egui::DragValue::new(&mut s.scroll_delay_ms).range(0..=5000));
        ui.end_row();
        ui.label("Wheel notch per langkah:");
        ui.add(egui::DragValue::new(&mut s.wheel_notches).range(1..=50));
        ui.end_row();
    });
}

fn action_summary(actions: &[Action]) -> String {
    actions
        .iter()
        .map(|a| match a {
            Action::OpenWith(_) => "open_with".to_string(),
            Action::RunCommand(_) => "run_command".to_string(),
            o => o.label(),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

impl SettingsApp {
    fn recording_tab(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let r = &mut self.cfg.recording;
        section(ui, &pal, "Video");
        egui::Grid::new("rec").num_columns(2).show(ui, |ui| {
            ui.label("FPS:");
            ui.add(egui::DragValue::new(&mut r.fps).range(1..=120));
            ui.end_row();
            ui.label("Bitrate (kbps):");
            ui.add(egui::DragValue::new(&mut r.bitrate_kbps).range(100..=200_000));
            ui.end_row();
        });
        tgl(ui, &pal, &mut r.show_cursor, "Sertakan kursor");
        tgl(ui, &pal, &mut r.system_audio, "Rekam audio sistem");
        tgl(ui, &pal, &mut r.microphone, "Rekam mikrofon");
        if r.microphone {
            labeled(ui, &pal, "ID mikrofon (kosong = default):", |ui| {
                let mut s = r.mic_device.clone().unwrap_or_default();
                if ui.text_edit_singleline(&mut s).changed() {
                    r.mic_device = (!s.is_empty()).then_some(s);
                }
            });
        }
        let mut custom = r.output_dir.is_some();
        if widgets::toggle(ui, &pal, &mut custom, "Folder output video khusus", None).changed() {
            r.output_dir = custom.then(|| self.cfg.general.output_dir.clone());
        }
        if let Some(d) = &mut r.output_dir {
            ui.horizontal(|ui| path_edit(ui, &pal, d, true));
        }
        section(ui, &pal, "Setelah rekaman selesai");
        let keys = app_keys(&self.cfg);
        let r = &mut self.cfg.recording;
        if actions_editor(ui, &pal, "rec", &mut r.actions, &keys, &mut self.bufs, true) {
            self.bufs.retain(|k, _| !k.starts_with("rec:"));
        }
    }

    fn hotkeys_tab(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let keys = app_keys(&self.cfg);
        let dups: Vec<usize> = {
            let ks: Vec<&str> = self.cfg.hotkeys.iter().map(|h| h.keys.as_str()).collect();
            hotkey::find_duplicates(&ks).into_iter().flat_map(|(a, b)| [a, b]).collect()
        };
        let mut remove = None;
        let mut start: Option<usize> = None;
        let mut stop = false;
        let mut swap: Option<(usize, usize)> = None;
        let n = self.cfg.hotkeys.len();
        for i in 0..n {
            let mut structural = false;
            let recording_this = self.recording == Some(RecTarget::Global(i));
            let b = &mut self.cfg.hotkeys[i];
            card(ui, &pal, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("#{}", i + 1)).color(pal.muted));
                    let bad = hotkey::normalize(&b.keys).is_err() || dups.contains(&i);
                    let mut te = egui::TextEdit::singleline(&mut b.keys).desired_width(180.0);
                    if bad {
                        te = te.text_color(pal.danger);
                    }
                    ui.add_enabled(!recording_this, te);
                    if recording_this {
                        if cbtn(ui, &pal, "Batal rekam") {
                            stop = true;
                        }
                        rec_chip(ui, &pal);
                    } else if cbtn(ui, &pal, "Rekam") {
                        start = Some(i);
                    }
                    egui::ComboBox::from_id_salt(("kind", i)).selected_text(kind_label(b.capture)).show_ui(ui, |ui| {
                        for (k, l) in CAPTURE_KINDS {
                            ui.selectable_value(&mut b.capture, k, l);
                        }
                    });
                    if sbtn_en(ui, &pal, i > 0, "^") {
                        swap = Some((i, i - 1));
                    }
                    if sbtn_en(ui, &pal, i + 1 < n, "v") {
                        swap = Some((i, i + 1));
                    }
                    if cbtn(ui, &pal, "Hapus") {
                        remove = Some(i);
                    }
                });
                if recording_this {
                    if let Some(m) = &self.record_msg {
                        ui.label(RichText::new(m).color(pal.danger));
                    }
                }
                muted(ui, &pal, "Actions (berurutan):");
                structural |= actions_editor(ui, &pal, &format!("hk{i}"), &mut b.actions, &keys, &mut self.bufs, false);
            });
            if structural {
                self.bufs.retain(|k, _| !k.starts_with(&format!("hk{i}:")));
            }
        }
        if stop {
            self.stop_recording();
        }
        if let Some(i) = start {
            self.start_recording(RecTarget::Global(i));
        }
        if let Some((a, b)) = swap {
            self.cfg.hotkeys.swap(a, b);
            self.bufs.clear();
            self.stop_recording();
        }
        if let Some(i) = remove {
            self.cfg.hotkeys.remove(i);
            self.bufs.clear();
            self.stop_recording();
        }
        if widgets::button(ui, &pal, Some(Icon::Plus), "Tambah hotkey", None).clicked() {
            self.cfg.hotkeys.push(HotkeyBinding {
                keys: String::new(),
                capture: CaptureKind::Region,
                actions: vec![Action::Save, Action::Clipboard],
                delay_secs: 0,
            });
        }
        muted(ui, &pal, "Format kanonik: ctrl+shift+printscreen, alt+f5, ctrl+plus ... (lihat juga tab Shortcut).");
    }

    fn shortcuts_tab(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        // Header: title + search + reset all.
        let mut reset_all = false;
        ui.horizontal(|ui| {
            title(ui, "Shortcut");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                reset_all = cbtn(ui, &pal, "Reset semua");
                egui::Frame::none()
                    .stroke(Stroke::new(1.0_f32, pal.line_strong))
                    .rounding(tokens::RADIUS_CONTROL)
                    .inner_margin(Margin::symmetric(10.0, 4.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                            icons::paint(ui.painter(), r, Icon::Search, pal.muted, 2.0);
                            ui.add(
                                egui::TextEdit::singleline(&mut self.search)
                                    .frame(false)
                                    .desired_width(240.0)
                                    .hint_text("Cari aksi atau tombol (mis. PrtSc)"),
                            );
                        });
                    });
            });
        });
        if reset_all {
            model::reset_all_editor(&mut self.cfg);
        }

        if widgets::banner(
            ui,
            &pal,
            BannerKind::Warn,
            "Windows 11 memakai PrtSc untuk Snipping Tool. Matikan di Settings › Accessibility › Keyboard agar \
             hotkey Jepret jalan.",
            Some("Buka setting Windows"),
        ) {
            if let Err(e) = std::process::Command::new("explorer.exe").arg("ms-settings:easeofaccess-keyboard").spawn() {
                self.status = Some((false, format!("Gagal membuka pengaturan: {e}")));
            }
        }

        let q = self.search.trim().to_lowercase();
        let matches = |texts: &[&str]| q.is_empty() || texts.iter().any(|t| t.to_lowercase().contains(&q));

        let mut start: Option<RecTarget> = None;
        let mut stop = false;
        let mut goto_workflow = false;
        let rec = self.recording.clone();
        let rec_msg = self.record_msg.clone();

        // ---- global hotkeys ----
        ui.add_space(6.0);
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            h2(ui, "Hotkey global");
            muted(ui, &pal, "Aktif di mana saja selama Jepret berjalan di tray.");
            ui.add_space(4.0);
        });
        let dups: Vec<usize> = {
            let ks: Vec<&str> = self.cfg.hotkeys.iter().map(|h| h.keys.as_str()).collect();
            hotkey::find_duplicates(&ks).into_iter().flat_map(|(a, b)| [a, b]).collect()
        };
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for (i, b) in self.cfg.hotkeys.iter().enumerate() {
                let pretty = hotkey::parse(&b.keys).map(|c| c.pretty()).unwrap_or_else(|_| b.keys.clone());
                if !matches(&[kind_label(b.capture), &pretty]) {
                    continue;
                }
                let err = hotkey::normalize(&b.keys).err();
                let dup = dups.contains(&i);
                let bad = err.is_some() || dup;
                let recording_this = rec == Some(RecTarget::Global(i));
                let (rect, cols) = row_rects(ui, 40.0);
                let bg = if bad {
                    Some(pal.danger_soft)
                } else if recording_this {
                    Some(pal.accent_soft.gamma_multiply(0.6))
                } else {
                    None
                };
                if let Some(bg) = bg {
                    ui.painter().rect_filled(rect, tokens::RADIUS_CONTROL, bg);
                }
                cell(ui, cols[0], false, |ui| {
                    ui.add(egui::Label::new(RichText::new(kind_label(b.capture)).strong()).truncate());
                });
                cell(ui, cols[1], false, |ui| {
                    if recording_this {
                        rec_chip(ui, &pal);
                        return;
                    }
                    let shown = if pretty.is_empty() { "—" } else { pretty.as_str() };
                    kbd_chip(ui, &pal, shown, bad, false);
                    let (note, col) = if let Some(e) = &err {
                        (e.clone(), pal.danger)
                    } else if dup {
                        ("Bentrok: kombinasi sama dengan hotkey lain".to_string(), pal.danger)
                    } else if matches!(b.capture, CaptureKind::RecordRegion | CaptureKind::RecordMonitor) {
                        ("tekan lagi = stop".to_string(), pal.muted)
                    } else {
                        (action_summary(&b.actions), pal.muted)
                    };
                    ui.add(egui::Label::new(RichText::new(note).size(tokens::TEXT_SMALL).color(col)).truncate());
                });
                cell(ui, cols[2], true, |ui| {
                    if chip_button(ui, &pal, "Workflow ›", 28.0, Chip::Ghost).clicked() {
                        goto_workflow = true;
                    }
                    if recording_this {
                        if cbtn(ui, &pal, "Batal") {
                            stop = true;
                        }
                    } else if cbtn(ui, &pal, "Rekam") {
                        start = Some(RecTarget::Global(i));
                    }
                });
                if recording_this {
                    if let Some(m) = &rec_msg {
                        ui.label(RichText::new(m).size(tokens::TEXT_SMALL).color(pal.danger));
                    }
                }
            }
        });

        // ---- editor shortcuts ----
        ui.add_space(14.0);
        let changed_n = EDITOR_SHORTCUTS.iter().filter(|d| model::editor_is_overridden(&self.cfg, d.id)).count();
        ui.horizontal(|ui| {
            h2(ui, "Editor");
            muted(
                ui,
                &pal,
                &format!("{} aksi · {} diubah · klik kombinasi untuk menghapus", EDITOR_SHORTCUTS.len(), changed_n),
            );
        });
        let mut groups: Vec<&str> = Vec::new();
        for d in EDITOR_SHORTCUTS {
            if !groups.contains(&d.group) {
                groups.push(d.group);
            }
        }
        self.group = self.group.min(groups.len().saturating_sub(1));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (gi, g) in groups.iter().enumerate() {
                if pill(ui, &pal, g, gi == self.group && q.is_empty()).clicked() {
                    self.group = gi;
                }
            }
        });
        if let Some(m) = &rec_msg {
            if matches!(rec, Some(RecTarget::EditorAdd(_))) {
                ui.label(RichText::new(m).size(tokens::TEXT_SMALL).color(pal.danger));
            }
        }
        let conflict_set: std::collections::HashSet<String> =
            model::editor_conflicts(&self.cfg).into_iter().map(|(c, _)| c.to_string()).collect();
        let cur_group = groups.get(self.group).copied();
        let mut edits: Vec<(&'static str, Vec<String>)> = Vec::new();
        let mut resets: Vec<&'static str> = Vec::new();
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for d in EDITOR_SHORTCUTS {
                let eff = model::editor_effective(&self.cfg, d.id);
                let in_view = if q.is_empty() {
                    Some(d.group) == cur_group
                } else {
                    matches(&[d.label, d.id, d.group, &eff.join(" ")])
                };
                if !in_view {
                    continue;
                }
                let overridden = model::editor_is_overridden(&self.cfg, d.id);
                let (rect, cols) = row_rects(ui, 36.0);
                ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0_f32, pal.line));
                cell(ui, cols[0], false, |ui| {
                    ui.add(egui::Label::new(d.label).truncate());
                    if overridden {
                        egui::Frame::none()
                            .fill(pal.accent_soft)
                            .rounding(tokens::RADIUS_KBD)
                            .inner_margin(Margin::symmetric(6.0, 1.0))
                            .show(ui, |ui| {
                                ui.label(RichText::new("diubah").size(tokens::TEXT_KBD).strong().color(pal.accent));
                            });
                    }
                });
                cell(ui, cols[1], false, |ui| {
                    for (ci, c) in eff.iter().enumerate() {
                        let pretty = KeyCombo::parse(c).map(|k| k.pretty()).unwrap_or_else(|_| c.clone());
                        if kbd_chip(ui, &pal, &pretty, conflict_set.contains(c), true)
                            .on_hover_text("Klik untuk hapus")
                            .clicked()
                        {
                            let mut n = eff.clone();
                            n.remove(ci);
                            edits.push((d.id, n));
                        }
                    }
                    if eff.is_empty() {
                        muted(ui, &pal, "(nonaktif)");
                    }
                    let this = RecTarget::EditorAdd(d.id.to_string());
                    if rec.as_ref() == Some(&this) {
                        rec_chip(ui, &pal);
                        if chip_button(ui, &pal, "Batal", 24.0, Chip::Ghost).clicked() {
                            stop = true;
                        }
                    } else if chip_button(ui, &pal, "+ Rekam", 24.0, Chip::Dashed).clicked() {
                        start = Some(this);
                    }
                });
                cell(ui, cols[2], true, |ui| {
                    if overridden {
                        let defs: Vec<String> = model::editor_defaults(d.id)
                            .iter()
                            .map(|s| KeyCombo::parse(s).map(|k| k.pretty()).unwrap_or_else(|_| s.clone()))
                            .collect();
                        let to = if defs.is_empty() { "nonaktif".to_string() } else { defs.join(", ") };
                        if chip_button(ui, &pal, &format!("Reset ke {to}"), 28.0, Chip::GhostAccent).clicked() {
                            resets.push(d.id);
                        }
                    }
                });
            }
        });
        for (id, list) in edits {
            model::set_editor_combos(&mut self.cfg, id, &list);
        }
        for id in resets {
            self.cfg.editor.shortcuts.remove(id);
        }
        if stop {
            self.stop_recording();
        }
        if let Some(t) = start {
            self.start_recording(t);
        }
        if goto_workflow {
            self.tab = Tab::Hotkeys;
            self.stop_recording();
        }
        ui.add_space(6.0);
        muted(
            ui,
            &pal,
            "Gestur tetap: Shift = batasi sudut/proporsi, Alt = dari tengah, Space+drag = geser kanvas, \
             Ctrl+scroll = zoom. Override hanya disimpan bila berbeda dari default.",
        );
    }

    fn apps_tab(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        muted(ui, &pal, "Aplikasi yang bisa dipakai action \"open_with\".");
        ui.horizontal(|ui| {
            if widgets::button(ui, &pal, Some(Icon::Plus), "Pilih .exe...", None).clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("Program", &["exe"]).pick_file() {
                    model::add_app(&mut self.cfg, p, None);
                }
            }
            if widgets::button(ui, &pal, None, "Dari aplikasi terpasang (Open with)", None).clicked() {
                self.candidates = Some(match jepret_workflow::open_with_candidates() {
                    Ok(v) => (v, None),
                    Err(e) => (Vec::new(), Some(e.to_string())),
                });
            }
        });
        if let Some((list, err)) = &self.candidates {
            let mut add = None;
            let mut close = false;
            card(ui, &pal, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Aplikasi terpasang").strong());
                    if sbtn(ui, &pal, "Tutup") {
                        close = true;
                    }
                });
                if let Some(e) = err {
                    ui.label(RichText::new(format!("Tidak bisa membaca daftar: {e}")).color(pal.warn_text));
                }
                if list.is_empty() {
                    muted(ui, &pal, "Daftar kosong. Gunakan tombol \"Pilih .exe...\".");
                }
                for c in list {
                    ui.horizontal(|ui| {
                        if sbtn(ui, &pal, "Tambah") {
                            add = Some((c.path.clone(), c.name.clone()));
                        }
                        ui.label(&c.name);
                        muted(ui, &pal, &c.path.display().to_string());
                    });
                }
            });
            if let Some((p, n)) = add {
                model::add_app(&mut self.cfg, p, Some(n));
            }
            if close {
                self.candidates = None;
            }
        }
        let mut remove = None;
        let keys: Vec<String> = self.cfg.apps.keys().cloned().collect();
        for k in keys {
            let Some(app) = self.cfg.apps.get_mut(&k) else { continue };
            card(ui, &pal, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&k).monospace());
                    let mut name = app.name.clone().unwrap_or_default();
                    ui.label(RichText::new("Nama:").color(pal.muted));
                    if ui.add(egui::TextEdit::singleline(&mut name).desired_width(160.0)).changed() {
                        app.name = (!name.is_empty()).then_some(name);
                    }
                    if cbtn(ui, &pal, "Hapus") {
                        remove = Some(k.clone());
                    }
                });
                labeled(ui, &pal, "Path:", |ui| path_edit(ui, &pal, &mut app.path, false));
                labeled(ui, &pal, "Argumen:", |ui| {
                    let buf = self.bufs.entry(format!("app:{k}")).or_insert_with(|| model::join_args(&app.args));
                    if ui.add(egui::TextEdit::singleline(buf).desired_width(360.0)).changed() {
                        app.args = model::split_args(buf);
                    }
                    muted(ui, &pal, "{file}, {dir}, ...");
                });
            });
        }
        if let Some(k) = remove {
            self.cfg.apps.remove(&k);
            self.bufs.remove(&format!("app:{k}"));
        }
    }
}

fn app_keys(cfg: &Config) -> Vec<String> {
    cfg.apps.keys().cloned().collect()
}

/// Ordered action list editor. Returns true when the list structure changed
/// (callers then drop cached text buffers for it).
fn actions_editor(
    ui: &mut Ui,
    pal: &Palette,
    id: &str,
    actions: &mut Vec<Action>,
    app_keys: &[String],
    bufs: &mut HashMap<String, String>,
    file_only: bool,
) -> bool {
    let mut changed = false;
    let mut remove = None;
    let mut swap = None;
    let n = actions.len();
    for (j, a) in actions.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if sbtn_en(ui, pal, j > 0, "^") {
                swap = Some((j, j - 1));
            }
            if sbtn_en(ui, pal, j + 1 < n, "v") {
                swap = Some((j, j + 1));
            }
            if sbtn(ui, pal, "x") {
                remove = Some(j);
            }
            match a {
                Action::OpenWith(key) => {
                    ui.label("open_with");
                    egui::ComboBox::from_id_salt((id, j)).selected_text(key.as_str()).show_ui(ui, |ui| {
                        for k in app_keys {
                            ui.selectable_value(key, k.clone(), k);
                        }
                    });
                }
                Action::RunCommand(c) => {
                    ui.label("run_command");
                    path_edit(ui, pal, &mut c.path, false);
                    tgl(ui, pal, &mut c.hidden, "sembunyikan");
                    let buf = bufs.entry(format!("{id}:{j}")).or_insert_with(|| model::join_args(&c.args));
                    if ui.add(egui::TextEdit::singleline(buf).desired_width(160.0).hint_text("argumen")).changed() {
                        c.args = model::split_args(buf);
                    }
                }
                other => {
                    ui.label(other.label());
                }
            }
        });
    }
    ui.horizontal(|ui| {
        let mut add: Option<Action> = None;
        egui::ComboBox::from_id_salt((id, "add")).selected_text("+ Tambah action").show_ui(ui, |ui| {
            let simple = [
                Action::Save,
                Action::Clipboard,
                Action::CopyPath,
                Action::OpenEditor,
                Action::OpenFolder,
                Action::Notify,
            ];
            for a in simple {
                if (!file_only || a.needs_file()) && ui.selectable_label(false, a.label()).clicked() {
                    add = Some(a);
                }
            }
            if ui.selectable_label(false, "open_with").clicked() {
                add = Some(Action::OpenWith(app_keys.first().cloned().unwrap_or_default()));
            }
            if ui.selectable_label(false, "run_command").clicked() {
                add = Some(Action::RunCommand(CommandSpec {
                    path: PathBuf::new(),
                    args: vec!["{file}".into()],
                    hidden: false,
                }));
            }
        });
        if let Some(a) = add {
            actions.push(a);
            changed = true;
        }
    });
    if let Some((a, b)) = swap {
        actions.swap(a, b);
        changed = true;
    }
    if let Some(j) = remove {
        actions.remove(j);
        changed = true;
    }
    changed
}
