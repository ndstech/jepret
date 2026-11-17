//! Design tokens. Values are the design canvas's, verbatim.

use egui::{Color32, Rounding, Stroke, Style, TextStyle, FontFamily, FontId, Vec2};
use jepret_core::config::ThemeMode;

/// Radius (px).
pub const RADIUS_KBD: f32 = 4.0;
pub const RADIUS_CONTROL: f32 = 8.0;
pub const RADIUS_PANEL: f32 = 12.0;

/// Sizes (px).
pub const CONTROL_H: f32 = 32.0;
pub const TOOLBAR_BTN: f32 = 36.0;
pub const TOOL_BTN: f32 = 40.0;
pub const ROW_H: f32 = 36.0;

/// Spacing scale (4 px grid).
pub const SP_1: f32 = 4.0;
pub const SP_2: f32 = 8.0;
pub const SP_3: f32 = 12.0;
pub const SP_4: f32 = 16.0;
pub const SP_6: f32 = 24.0;
pub const SP_8: f32 = 32.0;

/// Text sizes (px).
pub const TEXT_TITLE: f32 = 20.0;
pub const TEXT_SECTION: f32 = 15.0;
pub const TEXT_BODY: f32 = 13.0;
pub const TEXT_SMALL: f32 = 12.0;
pub const TEXT_KBD: f32 = 11.0;

/// Annotation palette defaults (editor colours 1–9). The user's
/// `config.editor.palette` overrides these.
pub const ANNOTATION: [Color32; 9] = [
    Color32::from_rgb(0xE5, 0x39, 0x35),
    Color32::from_rgb(0xFB, 0x8C, 0x00),
    Color32::from_rgb(0xFD, 0xD8, 0x35),
    Color32::from_rgb(0x43, 0xA0, 0x47),
    Color32::from_rgb(0x1E, 0x88, 0xE5),
    Color32::from_rgb(0x8E, 0x24, 0xAA),
    Color32::from_rgb(0x00, 0x00, 0x00),
    Color32::from_rgb(0xFF, 0xFF, 0xFF),
    Color32::from_rgb(0x75, 0x75, 0x75),
];

/// Gold used for the favourite star.
pub const FAVORITE: Color32 = Color32::from_rgb(0xE8, 0xA3, 0x17);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// Window background.
    pub bg: Color32,
    /// Panels, toolbars, cards.
    pub surface: Color32,
    /// Menus, tooltips-on-dark, hovered rows.
    pub raised: Color32,
    pub line: Color32,
    /// Input borders, kbd borders.
    pub line_strong: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub on_accent: Color32,
    /// Recording / destructive.
    pub danger: Color32,
    pub danger_soft: Color32,
    pub warn_bg: Color32,
    pub warn_border: Color32,
    pub warn_text: Color32,
    /// Editor canvas backdrop behind the image.
    pub canvas: Color32,
    /// Tooltip fill and text (inverted).
    pub tooltip: Color32,
    pub tooltip_text: Color32,
    /// kbd chip fill.
    pub kbd_bg: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

impl Palette {
    pub const LIGHT: Palette = Palette {
        dark: false,
        bg: hex(0xF6F7F9),
        surface: hex(0xFFFFFF),
        raised: hex(0xF0F2F5),
        line: hex(0xE3E6EB),
        line_strong: hex(0xD5D9DF),
        text: hex(0x1B1F24),
        muted: hex(0x5B6470),
        accent: hex(0x0E7C86),
        accent_soft: hex(0xE6F3F4),
        on_accent: hex(0xFFFFFF),
        danger: hex(0xC62828),
        danger_soft: hex(0xFDECEA),
        warn_bg: hex(0xFFF6E5),
        warn_border: hex(0xF3D9A4),
        warn_text: hex(0x5C3D00),
        canvas: hex(0xE9ECEF),
        tooltip: hex(0x1B1F24),
        tooltip_text: hex(0xFFFFFF),
        kbd_bg: hex(0xF6F7F9),
    };

    pub const DARK: Palette = Palette {
        dark: true,
        bg: hex(0x17191C),
        surface: hex(0x1F2226),
        raised: hex(0x272B30),
        line: hex(0x33383E),
        line_strong: hex(0x4A5058),
        text: hex(0xE8EAED),
        muted: hex(0xA3ABB5),
        accent: hex(0x4FC3CF),
        accent_soft: hex(0x173B40),
        on_accent: hex(0x0B2A2E),
        danger: hex(0xFF6B5E),
        danger_soft: hex(0x3A1F1D),
        warn_bg: hex(0x3A2E14),
        warn_border: hex(0x6B5420),
        warn_text: hex(0xF3D9A4),
        canvas: hex(0x121417),
        tooltip: hex(0xE8EAED),
        tooltip_text: hex(0x17191C),
        kbd_bg: hex(0x272B30),
    };

    pub fn for_mode(mode: ThemeMode) -> Palette {
        let dark = match mode {
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
            ThemeMode::System => crate::system_prefers_dark(),
        };
        if dark {
            Self::DARK
        } else {
            Self::LIGHT
        }
    }
}

/// Full egui style for a palette.
pub fn style(p: &Palette) -> Style {
    let mut s = Style::default();
    let v = &mut s.visuals;
    *v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.override_text_color = Some(p.text);
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0_f32, p.line);
    v.window_rounding = Rounding::same(RADIUS_PANEL);
    v.menu_rounding = Rounding::same(RADIUS_CONTROL + 2.0);
    v.extreme_bg_color = p.surface;
    v.faint_bg_color = p.raised;
    v.code_bg_color = p.kbd_bg;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warn_text;
    v.error_fg_color = p.danger;
    v.selection.bg_fill = p.accent_soft;
    v.selection.stroke = Stroke::new(1.0_f32, p.accent);
    v.window_shadow = egui::epaint::Shadow {
        offset: Vec2::new(0.0, 8.0),
        blur: 24.0,
        spread: 0.0,
        color: Color32::from_black_alpha(if p.dark { 110 } else { 40 }),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: Vec2::new(0.0, 6.0),
        blur: 20.0,
        spread: 0.0,
        color: Color32::from_black_alpha(if p.dark { 100 } else { 36 }),
    };

    let r = Rounding::same(RADIUS_CONTROL);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.line);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.noninteractive.rounding = r;

    w.inactive.bg_fill = p.surface;
    w.inactive.weak_bg_fill = p.surface;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, p.line_strong);
    w.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.inactive.rounding = r;

    w.hovered.bg_fill = p.raised;
    w.hovered.weak_bg_fill = p.raised;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, p.line_strong);
    w.hovered.fg_stroke = Stroke::new(1.5_f32, p.text);
    w.hovered.rounding = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.accent_soft;
    w.active.weak_bg_fill = p.accent_soft;
    w.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
    w.active.fg_stroke = Stroke::new(1.5_f32, p.text);
    w.active.rounding = r;
    w.active.expansion = 0.0;

    w.open = w.active;

    let sp = &mut s.spacing;
    sp.item_spacing = Vec2::new(SP_2, SP_2);
    sp.button_padding = Vec2::new(10.0, 6.0);
    sp.interact_size = Vec2::new(40.0, CONTROL_H);
    sp.window_margin = egui::Margin::same(SP_4);
    sp.menu_margin = egui::Margin::same(6.0);
    sp.indent = 18.0;

    s.text_styles = [
        (TextStyle::Heading, FontId::new(TEXT_TITLE, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(TEXT_BODY, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(TEXT_BODY, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(TEXT_SMALL, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(TEXT_SMALL, FontFamily::Monospace)),
    ]
    .into();
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance contrast.
    fn contrast(a: Color32, b: Color32) -> f32 {
        fn lum(c: Color32) -> f32 {
            let f = |v: u8| {
                let s = v as f32 / 255.0;
                if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
        }
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn text_contrast_meets_aa() {
        for p in [Palette::LIGHT, Palette::DARK] {
            for bg in [p.bg, p.surface, p.raised] {
                assert!(contrast(p.text, bg) >= 4.5, "text on {bg:?}");
                assert!(contrast(p.muted, bg) >= 4.5, "muted on {bg:?} dark={}", p.dark);
            }
            assert!(contrast(p.on_accent, p.accent) >= 4.5, "on_accent dark={}", p.dark);
            assert!(contrast(p.warn_text, p.warn_bg) >= 4.5);
            assert!(contrast(p.tooltip_text, p.tooltip) >= 4.5);
        }
    }
}
