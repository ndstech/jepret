//! The pin window. Idle = zero repaint; repaints only follow input, resizes
//! and the short label fade.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use egui::{Color32, Event, Key, PointerButton, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2, ViewportCommand};
use jepret_core::{Config, Result};
use jepret_ui_theme::Palette;

use crate::logic;
use crate::sys;

/// Largest texture side we try to upload (typical GL minimum guarantee).
const MAX_TEX_SIDE: u32 = 16_384;
/// Scroll accumulation (egui points) per zoom / opacity step.
const WHEEL_STEP: f32 = 40.0;
/// Hover label: fully visible for this long, then fades over `FADE`.
const LABEL_HOLD: Duration = Duration::from_millis(1400);
const FADE: Duration = Duration::from_millis(400);
/// Frames after a window command during which we trust our own geometry
/// instead of the (lagging) viewport info.
const PENDING_FRAMES: u8 = 4;
const MIN_SIDE_PT: f32 = 16.0;

pub fn run(file: &Path, config_path: &Path) -> Result<()> {
    sys::init_dpi_awareness();
    let config = Config::load(config_path).unwrap_or_else(|e| {
        tracing::warn!("config tidak terbaca ({e:#}); pakai default");
        Config::default()
    });
    let rgba = load_rgba(file)?;
    let (iw, ih) = rgba.dimensions();

    // Initial geometry in physical px, computed before the window exists.
    let env = sys::query();
    let (work, cursor, scale) = match env {
        Some(e) => (e.work, e.cursor, e.scale),
        None => ((0.0, 0.0, 1920.0, 1080.0), (960.0, 540.0), 1.0),
    };
    let zoom = logic::fit_zoom(iw, ih, work.2 as u32, work.3 as u32);
    let size = logic::zoomed_size(iw, ih, zoom);
    let pos = logic::place_near(cursor, size, work);
    tracing::debug!(?work, ?cursor, scale, zoom, "pin initial geometry");

    let viewport = egui::ViewportBuilder::default()
        .with_title("Jepret - Pin")
        .with_icon(std::sync::Arc::new(jepret_ui_theme::app_icon()))
        .with_decorations(false)
        .with_transparent(true)
        .with_taskbar(false)
        .with_resizable(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop)
        .with_inner_size([size.0 / scale, size.1 / scale])
        .with_position([pos.0 / scale, pos.1 / scale]);
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let file = file.to_path_buf();
    let theme = config.general.theme;
    eframe::run_native(
        "Jepret - Pin",
        options,
        Box::new(move |cc| {
            let pal = jepret_ui_theme::install(&cc.egui_ctx, theme);
            cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
            let tex = cc.egui_ctx.load_texture("pin", to_color_image(&rgba), TextureOptions::LINEAR);
            Ok(Box::new(PinApp::new(file, rgba, tex, pal, zoom, size, pos)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("pin window failed: {e}"))
}

fn load_rgba(file: &Path) -> Result<image::RgbaImage> {
    let img = image::open(file)
        .with_context(|| format!("buka gambar {}", file.display()))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    anyhow::ensure!(w > 0 && h > 0, "gambar kosong");
    anyhow::ensure!(w <= MAX_TEX_SIDE && h <= MAX_TEX_SIDE, "gambar terlalu besar ({w}x{h})");
    Ok(img)
}

fn to_color_image(img: &image::RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Copy,
    SaveAs,
    OpenEditor,
    Zoom100,
    Opacity(u8),
    Close,
}

struct PinApp {
    file: PathBuf,
    img_w: u32,
    img_h: u32,
    linear: TextureHandle,
    nearest: Option<TextureHandle>,
    pal: Palette,

    zoom: f32,
    opacity: u8,
    /// Window geometry in points (screen coords), our best knowledge.
    pos: Pos2,
    size: Vec2,
    pending: u8,
    /// Initial physical geometry, applied once ppp is known.
    init_phys: ((f32, f32), (f32, f32)),
    placed: bool,

    scroll_acc: f32,
    hovered: bool,
    label: String,
    label_at: Instant,
}

impl PinApp {
    fn new(
        file: PathBuf,
        rgba: image::RgbaImage,
        linear: TextureHandle,
        pal: Palette,
        zoom: f32,
        size_phys: (f32, f32),
        pos_phys: (f32, f32),
    ) -> Self {
        Self {
            file,
            img_w: rgba.width(),
            img_h: rgba.height(),
            linear,
            nearest: None,
            pal,
            zoom,
            opacity: 100,
            pos: Pos2::new(pos_phys.0, pos_phys.1),
            size: Vec2::new(size_phys.0, size_phys.1),
            pending: 0,
            init_phys: (size_phys, pos_phys),
            placed: false,
            scroll_acc: 0.0,
            hovered: false,
            label: String::new(),
            label_at: Instant::now(),
        }
    }

    fn size_for(&self, zoom: f32, ppp: f32) -> Vec2 {
        let (w, h) = logic::zoomed_size(self.img_w, self.img_h, zoom);
        Vec2::new((w / ppp).max(MIN_SIDE_PT), (h / ppp).max(MIN_SIDE_PT))
    }

    fn show_label(&mut self, text: String) {
        self.label = text;
        self.label_at = Instant::now();
    }

    fn zoom_label(&self) -> String {
        format!("{}%", (self.zoom * 100.0).round() as i32)
    }

    /// Resize to `zoom`, keeping the image point under `anchor` (screen
    /// points) fixed.
    fn set_zoom(&mut self, ctx: &egui::Context, zoom: f32, anchor: Pos2) {
        let zoom = logic::clamp_zoom(zoom);
        let ppp = ctx.pixels_per_point();
        let new_size = self.size_for(zoom, ppp);
        let np = logic::anchored_pos(
            (self.pos.x, self.pos.y),
            (self.size.x, self.size.y),
            (new_size.x, new_size.y),
            (anchor.x, anchor.y),
        );
        self.zoom = zoom;
        self.size = new_size;
        self.pos = Pos2::new(np.0, np.1);
        self.pending = PENDING_FRAMES;
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(new_size));
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(self.pos));
        self.show_label(self.zoom_label());
    }

    fn set_opacity(&mut self, v: u8) {
        self.opacity = v.clamp(logic::MIN_OPACITY, logic::MAX_OPACITY);
        self.show_label(format!("Opasitas {}%", self.opacity));
    }

    fn texture(&mut self, ctx: &egui::Context) -> egui::TextureId {
        if self.zoom >= 2.0 {
            if self.nearest.is_none() {
                // Re-decode rather than keeping a second copy around.
                match load_rgba(&self.file) {
                    Ok(img) => {
                        self.nearest = Some(ctx.load_texture("pin-nearest", to_color_image(&img), TextureOptions::NEAREST));
                    }
                    Err(e) => tracing::warn!("tekstur nearest gagal: {e:#}"),
                }
            }
            if let Some(t) = &self.nearest {
                return t.id();
            }
        }
        self.linear.id()
    }

    fn do_copy(&mut self) {
        let res = load_rgba(&self.file).and_then(|img| {
            let mut cb = arboard::Clipboard::new().context("buka clipboard")?;
            cb.set_image(arboard::ImageData {
                width: img.width() as usize,
                height: img.height() as usize,
                bytes: std::borrow::Cow::Borrowed(img.as_raw()),
            })
            .context("isi clipboard")?;
            Ok(())
        });
        match res {
            Ok(()) => self.show_label("Disalin".into()),
            Err(e) => {
                tracing::warn!("salin gagal: {e:#}");
                self.show_label("Gagal menyalin".into());
            }
        }
    }

    fn do_save_as(&mut self) {
        let stem = self.file.file_stem().and_then(|s| s.to_str()).unwrap_or("pin");
        let mut dlg = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .add_filter("JPEG", &["jpg", "jpeg"])
            .set_file_name(format!("{stem}.png"));
        if let Some(dir) = self.file.parent() {
            dlg = dlg.set_directory(dir);
        }
        let Some(target) = dlg.save_file() else { return };
        let res = load_rgba(&self.file).and_then(|img| save_image(&img, &target));
        match res {
            Ok(()) => self.show_label("Disimpan".into()),
            Err(e) => {
                tracing::warn!("simpan gagal: {e:#}");
                self.show_label("Gagal menyimpan".into());
            }
        }
    }

    fn open_editor(&mut self) {
        match spawn_editor(&self.file) {
            Ok(()) => {}
            Err(e) => {
                tracing::warn!("buka editor gagal: {e:#}");
                self.show_label("Gagal membuka editor".into());
            }
        }
    }

    fn perform(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Copy => self.do_copy(),
            Action::SaveAs => self.do_save_as(),
            Action::OpenEditor => self.open_editor(),
            Action::Zoom100 => {
                let anchor = self.pos + self.size / 2.0;
                self.set_zoom(ctx, 1.0, anchor);
            }
            Action::Opacity(v) => self.set_opacity(v),
            Action::Close => ctx.send_viewport_cmd(ViewportCommand::Close),
        }
    }
}

fn save_image(img: &image::RgbaImage, target: &Path) -> Result<()> {
    let is_jpeg = target
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"))
        .unwrap_or(false);
    if is_jpeg {
        image::DynamicImage::ImageRgba8(img.clone()).to_rgb8().save(target)
    } else {
        img.save(target)
    }
    .with_context(|| format!("simpan {}", target.display()))
}

fn spawn_editor(file: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("current_exe")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--editor").arg(file);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: outlives this pin.
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    cmd.spawn().context("spawn editor")?;
    Ok(())
}

impl eframe::App for PinApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let ppp = ctx.pixels_per_point();

        // First frame: ppp is now real; (re)apply the physical-px plan.
        if !self.placed {
            self.placed = true;
            let ((sw, sh), (px, py)) = self.init_phys;
            self.size = Vec2::new((sw / ppp).max(MIN_SIDE_PT), (sh / ppp).max(MIN_SIDE_PT));
            self.pos = Pos2::new(px / ppp, py / ppp);
            self.pending = PENDING_FRAMES;
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(self.size));
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(self.pos));
            ctx.request_repaint();
        }

        // Track real geometry when we have no command in flight.
        if self.pending > 0 {
            self.pending -= 1;
            ctx.request_repaint();
        } else if let Some(r) = ctx.input(|i| i.viewport().inner_rect) {
            self.pos = r.min;
            self.size = r.size();
        }

        let menu_open = ctx.memory(|m| m.any_popup_open());
        let mut action: Option<Action> = None;

        let (scroll_y, ctrl, esc, copy, hover_pos) = ctx.input(|i| {
            let copy = i.events.iter().any(|e| matches!(e, Event::Copy))
                || (i.modifiers.command && i.key_pressed(Key::C));
            (i.raw_scroll_delta.y, i.modifiers.ctrl, i.key_pressed(Key::Escape), copy, i.pointer.hover_pos())
        });
        if esc && !menu_open {
            action = Some(Action::Close);
        }
        if copy {
            action = Some(Action::Copy);
        }

        // Wheel: zoom around the cursor, Ctrl+wheel: opacity.
        if scroll_y != 0.0 && !menu_open {
            self.scroll_acc += scroll_y;
            while self.scroll_acc.abs() >= WHEEL_STEP {
                let up = self.scroll_acc > 0.0;
                self.scroll_acc -= WHEEL_STEP.copysign(self.scroll_acc);
                if ctrl {
                    self.set_opacity(if up { logic::opacity_up(self.opacity) } else { logic::opacity_down(self.opacity) });
                } else {
                    let anchor = hover_pos.map(|p| self.pos + p.to_vec2()).unwrap_or(self.pos + self.size / 2.0);
                    let z = if up { logic::zoom_in(self.zoom) } else { logic::zoom_out(self.zoom) };
                    self.set_zoom(ctx, z, anchor);
                }
            }
        } else if scroll_y == 0.0 {
            self.scroll_acc = 0.0;
        }

        let hovered_now = hover_pos.is_some();
        if hovered_now && !self.hovered {
            self.label = self.zoom_label();
            self.label_at = Instant::now();
        }
        self.hovered = hovered_now;

        let tex = self.texture(ctx);
        let tint = Color32::from_white_alpha((self.opacity as f32 / 100.0 * 255.0).round() as u8);
        let accent = self.pal.accent;

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                let resp = ui.allocate_rect(rect, Sense::click_and_drag());
                ui.painter().image(
                    tex,
                    rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    tint,
                );

                if resp.drag_started_by(PointerButton::Primary) {
                    ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                }
                if resp.double_clicked_by(PointerButton::Primary) {
                    action = Some(Action::Close);
                }

                resp.context_menu(|ui| {
                    if ui.button("Salin\tCtrl+C").clicked() {
                        action = Some(Action::Copy);
                        ui.close_menu();
                    }
                    if ui.button("Simpan sebagai…").clicked() {
                        action = Some(Action::SaveAs);
                        ui.close_menu();
                    }
                    if ui.button("Buka di editor").clicked() {
                        action = Some(Action::OpenEditor);
                        ui.close_menu();
                    }
                    if ui.button("Zoom 100%").clicked() {
                        action = Some(Action::Zoom100);
                        ui.close_menu();
                    }
                    ui.menu_button("Opasitas", |ui| {
                        for v in (logic::MIN_OPACITY..=logic::MAX_OPACITY).step_by(logic::OPACITY_STEP as usize) {
                            if ui.selectable_label(self.opacity == v, format!("{v}%")).clicked() {
                                action = Some(Action::Opacity(v));
                                ui.close_menu();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("Tutup\tEsc").clicked() {
                        action = Some(Action::Close);
                        ui.close_menu();
                    }
                });

                // Hover chrome: 1 px accent border + fading label top-right.
                if self.hovered || menu_open {
                    let px = 1.0 / ppp;
                    ui.painter()
                        .rect_stroke(rect.shrink(px / 2.0), 0.0, Stroke::new(px, accent));
                    let age = self.label_at.elapsed();
                    let alpha = if age <= LABEL_HOLD {
                        1.0
                    } else {
                        1.0 - (age - LABEL_HOLD).as_secs_f32() / FADE.as_secs_f32()
                    };
                    if alpha > 0.0 && !self.label.is_empty() {
                        let font = egui::FontId::proportional(12.0);
                        let galley = ui.painter().layout_no_wrap(self.label.clone(), font, Color32::WHITE);
                        let pad = Vec2::new(6.0, 3.0);
                        let size = galley.size() + pad * 2.0;
                        let min = Pos2::new(rect.right() - size.x - 6.0, rect.top() + 6.0);
                        let r = Rect::from_min_size(min, size);
                        let a = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
                        ui.painter().rect_filled(r, 4.0, Color32::from_black_alpha((a as f32 * 0.7) as u8));
                        ui.painter().galley(
                            r.min + pad,
                            galley,
                            Color32::from_white_alpha(a),
                        );
                        if alpha < 1.0 {
                            ctx.request_repaint_after(Duration::from_millis(33));
                        } else {
                            ctx.request_repaint_after(LABEL_HOLD.saturating_sub(age) + Duration::from_millis(5));
                        }
                    }
                }
            });

        if let Some(a) = action {
            self.perform(ctx, a);
            ctx.request_repaint();
        }
    }
}
