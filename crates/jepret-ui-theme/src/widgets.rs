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
    // Mockup: padding 1px 6px, border 1px with a 2px bottom border, radius 4.
    let tsz = text_size(ui, text, &font);
    let size = Vec2::new(tsz.x + 12.0 + 2.0, tsz.y + 2.0 + 3.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    // Border colour fills the whole chip; the face is inset 1 px (sides/top)
    // and 2 px (bottom), which yields the thicker bottom edge.
    p.rect_filled(rect, RADIUS_KBD, pal.line_strong);
    let face = Rect::from_min_max(rect.min + Vec2::new(1.0, 1.0), rect.max - Vec2::new(1.0, 2.0));
    p.rect_filled(face, RADIUS_KBD - 1.0, pal.kbd_bg);
    p.text(face.center(), Align2::CENTER_CENTER, text, font, pal.text);
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
    // Mockup: height 32, padding 0 14 (icon buttons 0 12), gap 8, radius 8.
    let pad = if icon.is_some() { 12.0 } else { 14.0 };
    let w = pad + icon_w + label_sz.x + gap + sc_sz.x + pad;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, CONTROL_H), Sense::click());
    let hovered = resp.hovered();
    let pressed = resp.is_pointer_button_down_on();
    let (fill, stroke, fg, sc_col) = if primary {
        let fill = if pressed {
            pal.accent.gamma_multiply(0.8)
        } else if hovered {
            pal.accent.gamma_multiply(0.9)
        } else {
            pal.accent
        };
        (fill, Stroke::NONE, pal.on_accent, pal.on_accent.gamma_multiply(0.8))
    } else {
        let fill = if pressed {
            pal.accent_soft
        } else if hovered {
            pal.raised
        } else {
            pal.surface
        };
        (fill, Stroke::new(1.0_f32, pal.line_strong), pal.text, pal.muted)
    };
    let p = ui.painter();
    p.rect(rect, RADIUS_CONTROL, fill, stroke);
    let mut x = rect.left() + pad;
    if let Some(ic) = icon {
        let r = Rect::from_center_size(egui::pos2(x + 8.0, rect.center().y), Vec2::splat(16.0));
        icons::paint(p, r, ic, fg, 2.0);
        x += icon_w;
    }
    p.text(egui::pos2(x, rect.center().y), Align2::LEFT_CENTER, label, font, fg);
    if let Some(s) = shortcut {
        p.text(egui::pos2(rect.right() - pad, rect.center().y), Align2::RIGHT_CENTER, s, sc_font, sc_col);
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

fn danger_button_impl(ui: &mut Ui, pal: &Palette, label: &str, shortcut: Option<&str>, solid: bool) -> Response {
    let font = body();
    let label_sz = text_size(ui, label, &font);
    let sc_font = mono(10.0);
    let sc_sz = shortcut.map(|s| text_size(ui, s, &sc_font)).unwrap_or(Vec2::ZERO);
    let gap = if shortcut.is_some() { SP_2 } else { 0.0 };
    let pad = 14.0;
    let w = pad + label_sz.x + gap + sc_sz.x + pad;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, CONTROL_H), Sense::click());
    let hovered = resp.hovered();
    let pressed = resp.is_pointer_button_down_on();
    let (fill, stroke, fg, sc_col) = if solid {
        let fill = if pressed {
            pal.danger.gamma_multiply(0.8)
        } else if hovered {
            pal.danger.gamma_multiply(0.9)
        } else {
            pal.danger
        };
        let fg = if pal.dark { pal.bg } else { pal.on_accent };
        (fill, Stroke::NONE, fg, fg.gamma_multiply(0.8))
    } else {
        let fill = if pressed {
            pal.danger_soft.gamma_multiply(if pal.dark { 1.25 } else { 0.92 })
        } else if hovered {
            pal.danger_soft
        } else {
            pal.surface
        };
        let border = if hovered || pressed { pal.danger } else { pal.line_strong };
        (fill, Stroke::new(1.0_f32, border), pal.danger, pal.danger.gamma_multiply(0.8))
    };
    let p = ui.painter();
    p.rect(rect, RADIUS_CONTROL, fill, stroke);
    p.text(egui::pos2(rect.left() + pad, rect.center().y), Align2::LEFT_CENTER, label, font, fg);
    if let Some(s) = shortcut {
        p.text(egui::pos2(rect.right() - pad, rect.center().y), Align2::RIGHT_CENTER, s, sc_font, sc_col);
    }
    if resp.has_focus() {
        p.rect_stroke(rect.expand(2.0), RADIUS_CONTROL + 2.0, Stroke::new(2.0_f32, pal.danger));
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Solid danger button (primary destructive action, e.g. "Hapus").
pub fn danger_button(ui: &mut Ui, pal: &Palette, label: &str, shortcut: Option<&str>) -> Response {
    danger_button_impl(ui, pal, label, shortcut, true)
}

/// Outlined / soft-tinted danger button (secondary destructive action, e.g. "Buang").
pub fn danger_soft_button(ui: &mut Ui, pal: &Palette, label: &str, shortcut: Option<&str>) -> Response {
    danger_button_impl(ui, pal, label, shortcut, false)
}

/// Tone for a modal dialog's header icon badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalTone {
    Accent,
    Warn,
    Danger,
}

/// Centered modal dialog with a dimmed backdrop scrim, elevated card, tinted
/// header icon badge, title, and close button (`×`).
///
/// Returns `true` when the user dismissed the modal by clicking outside the
/// card or clicking the close button.
pub fn modal_dialog(
    ctx: &egui::Context,
    pal: &Palette,
    id: &str,
    width: f32,
    icon: Option<(Icon, ModalTone)>,
    title: &str,
    body: impl FnOnce(&mut Ui),
) -> bool {
    let screen = ctx.screen_rect();
    let scrim_clicked = egui::Area::new(egui::Id::new(id).with("scrim"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
            ui.painter()
                .rect_filled(rect, 0.0, Color32::from_black_alpha(if pal.dark { 153 } else { 110 }));
            resp.clicked()
        })
        .inner;

    let card_w = width.min((screen.width() - SP_8).max(280.0));
    let pad_x = SP_6;
    let pad_y = 20.0;
    let mut close_clicked = false;

    egui::Area::new(egui::Id::new(id).with("card"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(pal.surface)
                .stroke(Stroke::new(1.0_f32, pal.line))
                .rounding(RADIUS_PANEL)
                .shadow(egui::epaint::Shadow {
                    offset: Vec2::new(0.0, 16.0),
                    blur: 48.0,
                    spread: 0.0,
                    color: Color32::from_black_alpha(if pal.dark { 128 } else { 60 }),
                })
                .inner_margin(egui::Margin::symmetric(pad_x, pad_y))
                .show(ui, |ui| {
                    ui.set_width(card_w - pad_x * 2.0);
                    ui.spacing_mut().item_spacing = Vec2::new(SP_2, SP_2);

                    ui.horizontal(|ui| {
                        if let Some((ic, tone)) = icon {
                            let (bg, border, fg) = match tone {
                                ModalTone::Accent => (pal.accent_soft, pal.accent.gamma_multiply(0.35), pal.accent),
                                ModalTone::Warn => (pal.warn_bg, pal.warn_border, pal.warn_text),
                                ModalTone::Danger => (pal.danger_soft, pal.danger.gamma_multiply(0.35), pal.danger),
                            };
                            let (badge, _) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::hover());
                            ui.painter().rect(badge, RADIUS_CONTROL + 1.0, bg, Stroke::new(1.0_f32, border));
                            icons::paint(
                                ui.painter(),
                                Rect::from_center_size(badge.center(), Vec2::splat(18.0)),
                                ic,
                                fg,
                                1.9,
                            );
                            ui.add_space(SP_1);
                        }
                        ui.label(egui::RichText::new(title).size(16.0).strong().color(pal.text));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let (r, x) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
                            if x.hovered() {
                                ui.painter().rect_filled(r, RADIUS_CONTROL, pal.raised);
                            }
                            let fg = if x.hovered() { pal.text } else { pal.muted };
                            icons::paint(ui.painter(), Rect::from_center_size(r.center(), Vec2::splat(13.0)), Icon::Close, fg, 1.75);
                            if x.on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text("Tutup (Esc)")
                                .clicked()
                            {
                                close_clicked = true;
                            }
                        });
                    });

                    ui.add_space(SP_2);
                    body(ui);
                });
        });

    scrim_clicked || close_clicked
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
            // White pill with a faint drop shadow (mockup: 0 1px 2px / 8%).
            p.rect_filled(seg.translate(Vec2::new(0.0, 1.0)), 6.0, Color32::from_black_alpha(if pal.dark { 60 } else { 20 }));
            p.rect_filled(seg, 6.0, pal.surface);
        } else if r.hovered() {
            p.rect_filled(seg, 6.0, pal.line.gamma_multiply(0.5));
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
