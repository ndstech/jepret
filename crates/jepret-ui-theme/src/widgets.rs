//! Reusable widgets from the design. They work as-is; **T15a** may refine
//! the painting but must keep these signatures.

use egui::{Align2, Color32, FontFamily, FontId, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::icons::{self, Icon};
use crate::tokens::*;

fn text_size(ui: &Ui, text: &str, font: &FontId) -> Vec2 {
    ui.fonts(|f| f.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE).size())
}

fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

fn body() -> FontId {
    FontId::new(TEXT_BODY, FontFamily::Proportional)
}

/// Keyboard chip, e.g. `Ctrl+PrtSc`. Pass `KeyCombo::pretty()` output.
pub fn kbd(ui: &mut Ui, pal: &Palette, text: &str) -> Response {
    let font = mono(TEXT_KBD);
    let size = text_size(ui, text, &font) + Vec2::new(12.0, 6.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    // 2 px bottom border look: darker rect offset under the chip.
    p.rect_filled(rect.translate(Vec2::new(0.0, 1.0)), RADIUS_KBD, pal.line_strong);
    p.rect(rect, RADIUS_KBD, pal.kbd_bg, Stroke::new(1.0_f32, pal.line_strong));
    p.text(rect.center(), Align2::CENTER_CENTER, text, font, pal.text);
    resp
}

/// Small muted shortcut text inside buttons/menus (no chip).
pub fn shortcut_hint(ui: &mut Ui, pal: &Palette, text: &str) -> Response {
    ui.label(egui::RichText::new(text).font(mono(10.0)).color(pal.muted))
}

fn button_impl(ui: &mut Ui, pal: &Palette, label: &str, shortcut: Option<&str>, primary: bool, icon: Option<Icon>) -> Response {
    let font = body();
    let label_sz = text_size(ui, label, &font);
    let sc_font = mono(10.0);
    let sc_sz = shortcut.map(|s| text_size(ui, s, &sc_font)).unwrap_or(Vec2::ZERO);
    let icon_w = if icon.is_some() { 16.0 + SP_2 } else { 0.0 };
    let gap = if shortcut.is_some() { SP_2 } else { 0.0 };
    let w = 12.0 + icon_w + label_sz.x + gap + sc_sz.x + 12.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, CONTROL_H), Sense::click());
    let hovered = resp.hovered();
    let (fill, stroke, fg, sc_col) = if primary {
        let fill = if hovered { pal.accent.gamma_multiply(0.9) } else { pal.accent };
        (fill, Stroke::NONE, pal.on_accent, pal.on_accent.gamma_multiply(0.8))
    } else {
        let fill = if hovered { pal.raised } else { pal.surface };
        (fill, Stroke::new(1.0_f32, pal.line_strong), pal.text, pal.muted)
    };
    let p = ui.painter();
    p.rect(rect, RADIUS_CONTROL, fill, stroke);
    let mut x = rect.left() + 12.0;
    if let Some(ic) = icon {
        let r = Rect::from_center_size(egui::pos2(x + 8.0, rect.center().y), Vec2::splat(16.0));
        icons::paint(p, r, ic, fg, 1.75);
        x += icon_w;
    }
    p.text(egui::pos2(x, rect.center().y), Align2::LEFT_CENTER, label, font, fg);
    if let Some(s) = shortcut {
        p.text(egui::pos2(rect.right() - 12.0, rect.center().y), Align2::RIGHT_CENTER, s, sc_font, sc_col);
    }
    if resp.has_focus() {
        p.rect_stroke(rect.expand(2.0), RADIUS_CONTROL + 2.0, Stroke::new(2.0_f32, pal.accent));
    }
    resp
}

/// Filled accent button (one per view: the main action).
pub fn primary_button(ui: &mut Ui, pal: &Palette, label: &str, shortcut: Option<&str>) -> Response {
    button_impl(ui, pal, label, shortcut, true, None)
}

/// Outlined button, optional leading icon and trailing shortcut.
pub fn button(ui: &mut Ui, pal: &Palette, icon: Option<Icon>, label: &str, shortcut: Option<&str>) -> Response {
    button_impl(ui, pal, label, shortcut, false, icon)
}

/// Square icon button (toolbar / tool palette).
///
/// `selected` = accent-soft fill and accent icon. The hover tooltip shows
/// `tooltip` plus a shortcut chip; `corner_key` also draws the shortcut
/// letter in the corner (tool palette).
///
/// ```ignore
/// IconButton::new(Icon::Arrow, "Panah").size(TOOL_BTN).selected(true)
///     .shortcut(Some("A")).corner_key(true).show(ui, &pal);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct IconButton<'a> {
    pub icon: Icon,
    pub tooltip: &'a str,
    pub size: f32,
    pub selected: bool,
    pub shortcut: Option<&'a str>,
    pub corner_key: bool,
}

impl<'a> IconButton<'a> {
    pub fn new(icon: Icon, tooltip: &'a str) -> Self {
        Self { icon, tooltip, size: TOOLBAR_BTN, selected: false, shortcut: None, corner_key: false }
    }
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
    pub fn shortcut(mut self, shortcut: Option<&'a str>) -> Self {
        self.shortcut = shortcut;
        self
    }
    pub fn corner_key(mut self, corner_key: bool) -> Self {
        self.corner_key = corner_key;
        self
    }
    pub fn show(self, ui: &mut Ui, pal: &Palette) -> Response {
        icon_button_impl(ui, pal, self)
    }
}

fn icon_button_impl(ui: &mut Ui, pal: &Palette, b: IconButton<'_>) -> Response {
    let IconButton { icon, tooltip, size, selected, shortcut, corner_key } = b;
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let p = ui.painter();
    let fill = if selected {
        pal.accent_soft
    } else if resp.hovered() {
        pal.raised
    } else {
        Color32::TRANSPARENT
    };
    p.rect_filled(rect, RADIUS_CONTROL, fill);
    let fg = if selected { pal.accent } else { pal.text };
    let ic = Rect::from_center_size(rect.center(), Vec2::splat(size * 0.5));
    icons::paint(p, ic, icon, fg, 1.75);
    if corner_key {
        if let Some(k) = shortcut {
            p.text(rect.right_bottom() - Vec2::new(3.0, 2.0), Align2::RIGHT_BOTTOM, k, mono(8.0), pal.muted);
        }
    }
    let tip = tooltip.to_owned();
    let sc = shortcut.map(str::to_owned);
    let pal2 = *pal;
    resp.on_hover_ui(move |ui| {
        ui.horizontal(|ui| {
            ui.label(&tip);
            if let Some(s) = &sc {
                kbd(ui, &pal2, s);
            }
        });
    })
}

/// Pill switch with label and optional shortcut on the right.
pub fn toggle(ui: &mut Ui, pal: &Palette, on: &mut bool, label: &str, shortcut: Option<&str>) -> Response {
    let mut resp = ui
        .horizontal(|ui| {
            let (rect, r) = ui.allocate_exact_size(Vec2::new(34.0, 18.0), Sense::click());
            let t = ui.ctx().animate_bool(r.id, *on);
            let track = if *on { pal.accent } else { pal.line_strong };
            ui.painter().rect_filled(rect, 9.0, track);
            let x = egui::lerp(rect.left() + 9.0..=rect.right() - 9.0, t);
            ui.painter().circle_filled(egui::pos2(x, rect.center().y), 6.0, Color32::WHITE);
            let l = ui.add(egui::Label::new(label).sense(Sense::click()));
            if let Some(s) = shortcut {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| shortcut_hint(ui, pal, s));
            }
            r | l
        })
        .inner;
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp
}

/// Segmented control (e.g. Semua / Gambar / Video). Returns the response of
/// the whole control; `selected` changes on click.
pub fn segmented(ui: &mut Ui, pal: &Palette, selected: &mut usize, options: &[&str]) -> Response {
    let font = body();
    let pad = 12.0;
    let widths: Vec<f32> = options.iter().map(|o| text_size(ui, o, &font).x + pad * 2.0).collect();
    let total = widths.iter().sum::<f32>() + 4.0;
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(total, CONTROL_H), Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(rect, RADIUS_CONTROL, pal.raised);
    let mut x = rect.left() + 2.0;
    for (i, (opt, w)) in options.iter().zip(&widths).enumerate() {
        let seg = Rect::from_min_size(egui::pos2(x, rect.top() + 2.0), Vec2::new(*w, CONTROL_H - 4.0));
        let r = ui.interact(seg, resp.id.with(i), Sense::click());
        if r.clicked() && *selected != i {
            *selected = i;
            resp.mark_changed();
        }
        let on = *selected == i;
        if on {
            p.rect(seg, RADIUS_CONTROL - 2.0, pal.surface, Stroke::new(1.0_f32, pal.line));
        }
        let col = if on { pal.text } else { pal.muted };
        p.text(seg.center(), Align2::CENTER_CENTER, *opt, font.clone(), col);
        x += w;
    }
    resp
}

/// Upper-case muted section label (12/600).
pub fn section_header(ui: &mut Ui, pal: &Palette, text: &str) -> Response {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .size(TEXT_SMALL)
            .strong()
            .color(pal.muted),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerKind {
    Info,
    Warn,
    Danger,
}

/// Full-width banner with icon, text and an optional action button.
/// Returns true when the action was clicked.
pub fn banner(ui: &mut Ui, pal: &Palette, kind: BannerKind, text: &str, action: Option<&str>) -> bool {
    let (bg, border, fg, icon) = match kind {
        BannerKind::Info => (pal.accent_soft, pal.accent, pal.text, Icon::Check),
        BannerKind::Warn => (pal.warn_bg, pal.warn_border, pal.warn_text, Icon::Warning),
        BannerKind::Danger => (pal.danger_soft, pal.danger, pal.text, Icon::Warning),
    };
    let mut clicked = false;
    egui::Frame::none()
        .fill(bg)
        .stroke(Stroke::new(1.0_f32, border))
        .rounding(RADIUS_CONTROL + 2.0)
        .inner_margin(egui::Margin::symmetric(14.0, 10.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                icons::paint(ui.painter(), r, icon, fg, 2.0);
                ui.add(egui::Label::new(egui::RichText::new(text).color(fg)).wrap());
                if let Some(a) = action {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        clicked = button(ui, pal, None, a, None).clicked();
                    });
                }
            });
        });
    clicked
}
