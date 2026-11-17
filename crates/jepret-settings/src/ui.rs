//! egui front-end. All state lives in a draft `Config`; logic is in `model` / `hotkey`.

use std::{collections::HashMap, path::PathBuf};

use egui::{Color32, RichText};
use jepret_core::{
    config::{HotkeyBinding, ImageFormat},
    Action, CaptureKind, CommandSpec, Config,
};
use jepret_workflow::OpenWithCandidate;

use crate::{
    hotkey::{self, Hotkey},
    model::{self, Issue, Severity},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Hotkeys,
    Apps,
    History,
    Recording,
    Capture,
}

const TABS: [(Tab, &str); 6] = [
    (Tab::General, "Umum"),
    (Tab::Hotkeys, "Hotkey & Workflow"),
    (Tab::Apps, "Aplikasi eksternal"),
    (Tab::History, "History & Cleanup"),
    (Tab::Recording, "Recording"),
    (Tab::Capture, "Capture"),
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

pub struct SettingsApp {
    path: PathBuf,
    cfg: Config,
    saved: Config,
    tab: Tab,
    status: Option<(bool, String)>,
    /// Hotkey index currently capturing a key press.
    recording: Option<usize>,
    record_msg: Option<String>,
    /// Text buffers for args lines, keyed by a stable-ish id; cleared on structural edits.
    bufs: HashMap<String, String>,
    candidates: Option<(Vec<OpenWithCandidate>, Option<String>)>,
}

impl SettingsApp {
    pub fn new(path: PathBuf, cfg: Config) -> Self {
        Self {
            path,
            saved: cfg.clone(),
            cfg,
            tab: Tab::General,
            status: None,
            recording: None,
            record_msg: None,
            bufs: HashMap::new(),
            candidates: None,
        }
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

    fn poll_hotkey_recording(&mut self, ctx: &egui::Context) {
        let Some(idx) = self.recording else { return };
        let events = ctx.input(|i| i.events.clone());
        for ev in events {
            let egui::Event::Key { key, pressed: true, modifiers, repeat: false, .. } = ev else { continue };
            if key == egui::Key::Escape && !modifiers.any() {
                self.recording = None;
                self.record_msg = None;
                return;
            }
            let name = format!("{key:?}");
            match Hotkey::from_parts(modifiers.ctrl, modifiers.alt, modifiers.shift, false, &name) {
                Ok(h) => {
                    if let Some(b) = self.cfg.hotkeys.get_mut(idx) {
                        b.keys = h.to_string();
                    }
                    self.recording = None;
                    self.record_msg = None;
                }
                Err(e) => self.record_msg = Some(e),
            }
            return;
        }
    }
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_hotkey_recording(ctx);
        let mut preview = self.cfg.clone();
        model::normalize_config(&mut preview);
        let issues = model::validate(&preview);
        let dirty = self.cfg != self.saved;

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (t, label) in TABS {
                    ui.selectable_value(&mut self.tab, t, label);
                }
            });
        });

        egui::TopBottomPanel::bottom("bar").show(ctx, |ui| {
            ui.add_space(4.0);
            if !issues.is_empty() {
                egui::ScrollArea::vertical().max_height(80.0).id_salt("issues").show(ui, |ui| {
                    for i in &issues {
                        draw_issue(ui, i);
                    }
                });
                ui.separator();
            }
            ui.horizontal(|ui| {
                let blocked = model::has_errors(&issues);
                if ui.add_enabled(dirty && !blocked, egui::Button::new("Simpan")).clicked() {
                    self.save();
                }
                if ui.add_enabled(dirty, egui::Button::new("Batalkan perubahan")).clicked() {
                    self.cfg = self.saved.clone();
                    self.bufs.clear();
                    self.recording = None;
                }
                if dirty {
                    ui.colored_label(Color32::from_rgb(220, 160, 0), "belum disimpan");
                }
                if let Some((ok, msg)) = &self.status {
                    let c = if *ok { Color32::from_rgb(60, 170, 80) } else { Color32::from_rgb(220, 70, 60) };
                    ui.colored_label(c, msg);
                }
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.tab {
                Tab::General => general_tab(ui, &mut self.cfg),
                Tab::Hotkeys => self.hotkeys_tab(ui),
                Tab::Apps => self.apps_tab(ui),
                Tab::History => history_tab(ui, &mut self.cfg),
                Tab::Recording => self.recording_tab(ui),
                Tab::Capture => capture_tab(ui, &mut self.cfg),
            });
        });
    }
}

fn draw_issue(ui: &mut egui::Ui, i: &Issue) {
    match i.severity {
        Severity::Error => ui.colored_label(Color32::from_rgb(220, 70, 60), format!("Error: {}", i.message)),
        Severity::Warning => ui.colored_label(Color32::from_rgb(220, 160, 0), format!("Peringatan: {}", i.message)),
    };
}

// ---------- helpers ----------

fn path_edit(ui: &mut egui::Ui, p: &mut PathBuf, folder: bool) {
    let mut s = p.to_string_lossy().into_owned();
    if ui.add(egui::TextEdit::singleline(&mut s).desired_width(360.0)).changed() {
        *p = PathBuf::from(s);
    }
    if ui.button("Pilih...").clicked() {
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
        ui.colored_label(Color32::from_rgb(220, 160, 0), "tidak ada");
    }
}

fn opt_num<T: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    label: &str,
    v: &mut Option<T>,
    default: T,
    range: std::ops::RangeInclusive<T>,
) {
    ui.horizontal(|ui| {
        let mut on = v.is_some();
        if ui.checkbox(&mut on, label).changed() {
            *v = on.then_some(default);
        }
        if let Some(x) = v {
            ui.add(egui::DragValue::new(x).range(range));
        }
    });
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong());
}

// ---------- tabs ----------

fn general_tab(ui: &mut egui::Ui, cfg: &mut Config) {
    let g = &mut cfg.general;
    section(ui, "Penyimpanan");
    ui.horizontal(|ui| {
        ui.label("Folder output:");
        path_edit(ui, &mut g.output_dir, true);
    });
    ui.horizontal(|ui| {
        ui.label("Pola nama file:");
        ui.text_edit_singleline(&mut g.filename_pattern);
        ui.weak(".ext otomatis; placeholder: {datetime} {date} {time}");
    });
    ui.checkbox(&mut g.monthly_subfolders, "Subfolder per bulan (yyyy-mm)");
    ui.horizontal(|ui| {
        ui.label("Format gambar:");
        ui.selectable_value(&mut g.image_format, ImageFormat::Png, "PNG");
        ui.selectable_value(&mut g.image_format, ImageFormat::Jpeg, "JPEG");
    });
    if g.image_format == ImageFormat::Jpeg {
        ui.horizontal(|ui| {
            ui.label("Kualitas JPEG:");
            ui.add(egui::Slider::new(&mut g.jpeg_quality, 1..=100));
        });
    }
    section(ui, "Perilaku");
    ui.checkbox(&mut g.play_sound, "Bunyi saat capture");
    ui.checkbox(&mut g.start_with_windows, "Jalankan saat Windows start");
}

fn history_tab(ui: &mut egui::Ui, cfg: &mut Config) {
    let h = &mut cfg.history;
    ui.checkbox(&mut h.enabled, "Catat history capture");
    section(ui, "Cleanup otomatis");
    let c = &mut h.cleanup;
    ui.checkbox(&mut c.enabled, "Aktifkan cleanup");
    ui.add_enabled_ui(c.enabled, |ui| {
        opt_num(ui, "Hapus lebih tua dari (hari):", &mut c.max_age_days, 30, 1..=36500);
        opt_num(ui, "Batas total ukuran (MB):", &mut c.max_total_mb, 5000, 1..=10_000_000);
        ui.checkbox(&mut c.keep_favorites, "Jangan hapus favorit");
        ui.checkbox(&mut c.use_recycle_bin, "Pindahkan ke Recycle Bin (bukan hapus permanen)");
        ui.checkbox(&mut c.run_on_startup, "Jalankan saat Jepret start");
    });
}

fn capture_tab(ui: &mut egui::Ui, cfg: &mut Config) {
    section(ui, "Capture");
    let c = &mut cfg.capture;
    ui.checkbox(&mut c.show_cursor, "Sertakan kursor");
    ui.checkbox(&mut c.hdr_tonemap, "Tone-map monitor HDR ke SDR");
    opt_num(ui, "Override SDR white level (nits):", &mut c.sdr_white_nits, 200.0_f32, 80.0..=1000.0);
    ui.weak("Kosongkan untuk memakai pengaturan Windows.");
    section(ui, "Overlay region");
    ui.checkbox(&mut c.overlay_magnifier, "Tampilkan magnifier");
    ui.checkbox(&mut c.overlay_snap_to_windows, "Snap ke window saat hover");
    section(ui, "Scroll capture");
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

impl SettingsApp {
    fn recording_tab(&mut self, ui: &mut egui::Ui) {
        let r = &mut self.cfg.recording;
        section(ui, "Video");
        egui::Grid::new("rec").num_columns(2).show(ui, |ui| {
            ui.label("FPS:");
            ui.add(egui::DragValue::new(&mut r.fps).range(1..=120));
            ui.end_row();
            ui.label("Bitrate (kbps):");
            ui.add(egui::DragValue::new(&mut r.bitrate_kbps).range(100..=200_000));
            ui.end_row();
        });
        ui.checkbox(&mut r.show_cursor, "Sertakan kursor");
        ui.checkbox(&mut r.system_audio, "Rekam audio sistem");
        ui.checkbox(&mut r.microphone, "Rekam mikrofon");
        if r.microphone {
            ui.horizontal(|ui| {
                ui.label("ID mikrofon (kosong = default):");
                let mut s = r.mic_device.clone().unwrap_or_default();
                if ui.text_edit_singleline(&mut s).changed() {
                    r.mic_device = (!s.is_empty()).then_some(s);
                }
            });
        }
        let mut custom = r.output_dir.is_some();
        if ui.checkbox(&mut custom, "Folder output video khusus").changed() {
            r.output_dir = custom.then(|| self.cfg.general.output_dir.clone());
        }
        if let Some(d) = &mut r.output_dir {
            ui.horizontal(|ui| path_edit(ui, d, true));
        }
        section(ui, "Setelah rekaman selesai");
        let keys = app_keys(&self.cfg);
        let r = &mut self.cfg.recording;
        if actions_editor(ui, "rec", &mut r.actions, &keys, &mut self.bufs, true) {
            self.bufs.retain(|k, _| !k.starts_with("rec:"));
        }
    }

    fn hotkeys_tab(&mut self, ui: &mut egui::Ui) {
        let keys = app_keys(&self.cfg);
        let dups: Vec<usize> = {
            let ks: Vec<&str> = self.cfg.hotkeys.iter().map(|h| h.keys.as_str()).collect();
            hotkey::find_duplicates(&ks).into_iter().flat_map(|(a, b)| [a, b]).collect()
        };
        let mut remove = None;
        let mut swap: Option<(usize, usize)> = None;
        let n = self.cfg.hotkeys.len();
        for i in 0..n {
            let mut structural = false;
            let recording_this = self.recording == Some(i);
            let b = &mut self.cfg.hotkeys[i];
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("#{}", i + 1));
                    let bad = hotkey::normalize(&b.keys).is_err() || dups.contains(&i);
                    let mut te = egui::TextEdit::singleline(&mut b.keys).desired_width(180.0);
                    if bad {
                        te = te.text_color(Color32::from_rgb(220, 70, 60));
                    }
                    ui.add_enabled(!recording_this, te);
                    if recording_this {
                        if ui.button("Batal rekam").clicked() {
                            self.recording = None;
                        }
                        ui.colored_label(Color32::from_rgb(220, 160, 0), "tekan kombinasi tombol... (Esc batal)");
                    } else if ui.button("Rekam").clicked() {
                        self.recording = Some(i);
                        self.record_msg = None;
                    }
                    egui::ComboBox::from_id_salt(("kind", i)).selected_text(kind_label(b.capture)).show_ui(ui, |ui| {
                        for (k, l) in CAPTURE_KINDS {
                            ui.selectable_value(&mut b.capture, k, l);
                        }
                    });
                    if ui.add_enabled(i > 0, egui::Button::new("^")).clicked() {
                        swap = Some((i, i - 1));
                    }
                    if ui.add_enabled(i + 1 < n, egui::Button::new("v")).clicked() {
                        swap = Some((i, i + 1));
                    }
                    if ui.button("Hapus").clicked() {
                        remove = Some(i);
                    }
                });
                if recording_this {
                    if let Some(m) = &self.record_msg {
                        ui.colored_label(Color32::from_rgb(220, 70, 60), m);
                    }
                    ui.weak("Tombol Win tidak bisa direkam; ketik manual, mis. win+shift+s.");
                }
                ui.label("Actions (berurutan):");
                structural |= actions_editor(ui, &format!("hk{i}"), &mut b.actions, &keys, &mut self.bufs, false);
            });
            if structural {
                self.bufs.retain(|k, _| !k.starts_with(&format!("hk{i}:")));
            }
        }
        if let Some((a, b)) = swap {
            self.cfg.hotkeys.swap(a, b);
            self.bufs.clear();
            self.recording = None;
        }
        if let Some(i) = remove {
            self.cfg.hotkeys.remove(i);
            self.bufs.clear();
            self.recording = None;
        }
        if ui.button("+ Tambah hotkey").clicked() {
            self.cfg.hotkeys.push(HotkeyBinding {
                keys: String::new(),
                capture: CaptureKind::Region,
                actions: vec![Action::Save, Action::Clipboard],
            });
        }
        ui.add_space(4.0);
        ui.weak("Format: ctrl+shift+4, printscreen, alt+printscreen, ctrl+alt+f5 (huruf kecil, modifier dulu).");
    }

    fn apps_tab(&mut self, ui: &mut egui::Ui) {
        ui.label("Aplikasi yang bisa dipakai action \"open_with\".");
        ui.horizontal(|ui| {
            if ui.button("+ Pilih .exe...").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("Program", &["exe"]).pick_file() {
                    model::add_app(&mut self.cfg, p, None);
                }
            }
            if ui.button("Dari aplikasi terpasang (Open with)").clicked() {
                self.candidates = Some(match jepret_workflow::open_with_candidates() {
                    Ok(v) => (v, None),
                    Err(e) => (Vec::new(), Some(e.to_string())),
                });
            }
        });
        if let Some((list, err)) = &self.candidates {
            let mut add = None;
            let mut close = false;
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Aplikasi terpasang").strong());
                    if ui.small_button("Tutup").clicked() {
                        close = true;
                    }
                });
                if let Some(e) = err {
                    ui.colored_label(Color32::from_rgb(220, 160, 0), format!("Tidak bisa membaca daftar: {e}"));
                }
                if list.is_empty() {
                    ui.weak("Daftar kosong. Gunakan tombol \"Pilih .exe...\".");
                }
                for c in list {
                    ui.horizontal(|ui| {
                        if ui.small_button("Tambah").clicked() {
                            add = Some((c.path.clone(), c.name.clone()));
                        }
                        ui.label(&c.name);
                        ui.weak(c.path.display().to_string());
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
        ui.separator();
        let mut remove = None;
        let keys: Vec<String> = self.cfg.apps.keys().cloned().collect();
        for k in keys {
            let Some(app) = self.cfg.apps.get_mut(&k) else { continue };
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&k).monospace());
                    let mut name = app.name.clone().unwrap_or_default();
                    ui.label("Nama:");
                    if ui.add(egui::TextEdit::singleline(&mut name).desired_width(160.0)).changed() {
                        app.name = (!name.is_empty()).then_some(name);
                    }
                    if ui.button("Hapus").clicked() {
                        remove = Some(k.clone());
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Path:");
                    path_edit(ui, &mut app.path, false);
                });
                ui.horizontal(|ui| {
                    ui.label("Argumen:");
                    let buf = self.bufs.entry(format!("app:{k}")).or_insert_with(|| model::join_args(&app.args));
                    if ui.add(egui::TextEdit::singleline(buf).desired_width(360.0)).changed() {
                        app.args = model::split_args(buf);
                    }
                    ui.weak("{file}, {dir}, ...");
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
    ui: &mut egui::Ui,
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
            if ui.add_enabled(j > 0, egui::Button::new("^").small()).clicked() {
                swap = Some((j, j - 1));
            }
            if ui.add_enabled(j + 1 < n, egui::Button::new("v").small()).clicked() {
                swap = Some((j, j + 1));
            }
            if ui.add(egui::Button::new("x").small()).clicked() {
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
                    path_edit(ui, &mut c.path, false);
                    ui.checkbox(&mut c.hidden, "sembunyikan");
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
