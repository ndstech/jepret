//! Visual constants and DirectWrite helpers shared by the overlay and the
//! region border. Colours are copied from the design canvas (the overlay does
//! not use egui, so `jepret-ui-theme` is intentionally not a dependency).

use windows::core::PCWSTR;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat, IDWriteTextLayout, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_TEXT_METRICS,
};

/// `0xRRGGBB` + alpha -> Direct2D colour (non-premultiplied).
pub const fn rgba(hex: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a,
    }
}

pub const fn rgb(hex: u32) -> D2D1_COLOR_F {
    rgba(hex, 1.0)
}

/// Dim outside the selection: rgba(10,12,15,0.55).
pub const DIM: D2D1_COLOR_F = rgba(0x0A0C0F, 0.55);
pub const ACCENT: D2D1_COLOR_F = rgb(0x4FC3CF);
pub const SNAP: D2D1_COLOR_F = rgb(0x8ADBE3);
pub const WHITE: D2D1_COLOR_F = rgb(0xFFFFFF);
pub const PANEL_BG: D2D1_COLOR_F = rgb(0x17191C);
pub const PANEL_BORDER: D2D1_COLOR_F = rgb(0x33383E);
/// Hint bar / snap label fill: rgba(23,25,28,0.92) / 0.9.
pub const HINT_BG: D2D1_COLOR_F = rgba(0x17191C, 0.92);
pub const SNAP_LABEL_BG: D2D1_COLOR_F = rgba(0x17191C, 0.9);
pub const TEXT: D2D1_COLOR_F = rgb(0xE8EAED);
pub const TEXT_HINT: D2D1_COLOR_F = rgb(0xC3C9D1);
pub const MUTED: D2D1_COLOR_F = rgb(0xA3ABB5);
pub const KBD_BORDER: D2D1_COLOR_F = rgb(0x4A5058);
pub const KBD_BG: D2D1_COLOR_F = rgb(0x272B30);
/// Recording indicator (danger) and the dark text on top of it.
pub const DANGER: D2D1_COLOR_F = rgb(0xFF6B5E);
pub const ON_DANGER: D2D1_COLOR_F = rgb(0x2A0B08);
pub const TIMER_BG: D2D1_COLOR_F = rgb(0x1F2226);

pub const UI_FONTS: [&str; 2] = ["Segoe UI Variable", "Segoe UI"];
pub const MONO_FONTS: [&str; 2] = ["Cascadia Mono", "Consolas"];

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Whether the font collection has a family called `name`.
///
/// # Safety
/// Plain COM calls on a live collection.
unsafe fn has_family(coll: &IDWriteFontCollection, name: &str) -> bool {
    let w = wide(name);
    let (mut idx, mut exists) = (0u32, BOOL(0));
    coll.FindFamilyName(PCWSTR(w.as_ptr()), &mut idx, &mut exists).is_ok() && exists.as_bool()
}

/// First installed family out of `families` (the last one if none is found).
///
/// # Safety
/// `dwrite` must be a live factory.
pub unsafe fn pick_family(dwrite: &IDWriteFactory, families: &[&'static str]) -> &'static str {
    let mut coll = None;
    if dwrite.GetSystemFontCollection(&mut coll, false).is_ok() {
        if let Some(coll) = coll {
            for f in families {
                if has_family(&coll, f) {
                    return f;
                }
            }
        }
    }
    families[families.len() - 1]
}

/// Text format for `family` at `size` px.
///
/// # Safety
/// `dwrite` must be a live factory.
pub unsafe fn text_format(
    dwrite: &IDWriteFactory,
    family: &str,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
) -> windows::core::Result<IDWriteTextFormat> {
    let fam = wide(family);
    dwrite.CreateTextFormat(
        PCWSTR(fam.as_ptr()),
        None,
        weight,
        DWRITE_FONT_STYLE_NORMAL,
        DWRITE_FONT_STRETCH_NORMAL,
        size,
        windows::core::w!("en-us"),
    )
}

/// Layout of `text` in `format` plus its (width, height) in px.
///
/// # Safety
/// `dwrite` must be a live factory.
pub unsafe fn text_layout(
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    text: &str,
) -> Option<(IDWriteTextLayout, f32, f32)> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let layout = dwrite.CreateTextLayout(&wide, format, 2000.0, 200.0).ok()?;
    let mut m = DWRITE_TEXT_METRICS::default();
    layout.GetMetrics(&mut m).ok()?;
    Some((layout, m.width, m.height))
}

/// `mm:ss` (minutes keep counting past 59, like a stopwatch).
pub fn format_timer(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_format() {
        assert_eq!(format_timer(0), "00:00");
        assert_eq!(format_timer(134), "02:14");
        assert_eq!(format_timer(3600 + 5), "60:05");
    }

    #[test]
    fn hex_to_color() {
        let c = rgb(0x4FC3CF);
        assert!((c.r - 79.0 / 255.0).abs() < 1e-6 && (c.b - 207.0 / 255.0).abs() < 1e-6 && c.a == 1.0);
    }
}
