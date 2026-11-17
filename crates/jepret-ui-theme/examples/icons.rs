//! Visual check: every icon at 16/20/24/48 px (light + dark) and every widget.
//! `cargo run -p jepret-ui-theme --example icons`

use eframe::egui::{self, Rect, Vec2};
use jepret_core::config::ThemeMode;
use jepret_ui_theme::widgets::{self, BannerKind, IconButton};
use jepret_ui_theme::{icons, tokens, Icon, Palette};

struct App {
    dark: bool,
    toggle_a: bool,
    toggle_b: bool,
    seg: usize,
    stroke: f32,
}

impl App {
    fn panel(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        egui::Frame::none().fill(pal.bg).inner_margin(12.0).show(ui, |ui| {
            ui.visuals_mut().override_text_color = Some(pal.text);
            widgets::section_header(ui, pal, if pal.dark { "Gelap" } else { "Terang" });
            for size in [16.0_f32, 20.0, 24.0, 48.0] {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::splat(6.0);
                    ui.label(format!("{size}px"));
                    for &icon in Icon::ALL {
                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
                        icons::paint(ui.painter(), r, icon, pal.text, self.stroke);
                        resp.on_hover_text(format!("{icon:?}"));
                    }
                });
            }
            ui.horizontal(|ui| {
                ui.label("filled:");
                for icon in [Icon::Star, Icon::Play, Icon::Record, Icon::Rect] {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(32.0), egui::Sense::hover());
                    icons::paint_filled(ui.painter(), r, icon, tokens::FAVORITE);
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                for (i, (icon, key)) in Icon::TOOLS.iter().enumerate() {
                    IconButton::new(*icon, key)
                        .size(tokens::TOOL_BTN)
                        .selected(i == 2)
                        .shortcut(Some("A"))
                        .corner_key(true)
                        .show(ui, pal);
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                widgets::primary_button(ui, pal, "Simpan", Some("Ctrl+S"));
                widgets::button(ui, pal, Some(Icon::Copy), "Salin", Some("Ctrl+C"));
                widgets::button(ui, pal, None, "Batal", None);
                widgets::kbd(ui, pal, "Ctrl+PrtSc");
                widgets::kbd(ui, pal, "A");
                widgets::shortcut_hint(ui, pal, "Ctrl+Z");
            });
            ui.add_space(8.0);
            widgets::toggle(ui, pal, &mut self.toggle_a, "Salin otomatis", Some("Ctrl+K"));
            widgets::toggle(ui, pal, &mut self.toggle_b, "Bunyi rana", None);
            widgets::segmented(ui, pal, &mut self.seg, &["Semua", "Gambar", "Video"]);
            ui.add_space(8.0);
            widgets::banner(ui, pal, BannerKind::Info, "Tersimpan ke Pictures\\Jepret", Some("Buka"));
            widgets::banner(ui, pal, BannerKind::Warn, "Hotkey bentrok dengan aplikasi lain", Some("Ubah"));
            widgets::banner(ui, pal, BannerKind::Danger, "Perekaman gagal", None);
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Stroke (24-unit):");
                ui.add(egui::Slider::new(&mut self.stroke, 1.0..=3.0));
                ui.checkbox(&mut self.dark, "UI gelap");
            });
        });
        let (light, dark) = (Palette::LIGHT, Palette::DARK);
        egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| {
            ui.columns(2, |cols| {
                for (col, pal) in cols.iter_mut().zip([light, dark]) {
                    jepret_ui_theme::apply(ctx, &pal);
                    let r = col.max_rect();
                    col.painter().rect_filled(Rect::from_min_max(r.min, r.max), 0.0, pal.bg);
                    egui::ScrollArea::vertical().id_salt(pal.dark).show(col, |ui| self.panel(ui, &pal));
                }
            });
        });
        jepret_ui_theme::apply(ctx, if self.dark { &dark } else { &light });
    }
}

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "Jepret icons",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([1400.0, 900.0]),
            ..Default::default()
        },
        Box::new(|cc| {
            jepret_ui_theme::install(&cc.egui_ctx, ThemeMode::Light);
            Ok(Box::new(App { dark: false, toggle_a: true, toggle_b: false, seg: 0, stroke: 1.75 }))
        }),
    )
}
