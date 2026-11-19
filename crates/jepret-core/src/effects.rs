//! Image effects applied after capture (workflow action `effects`) or from
//! the editor: rounded corners, border, drop shadow, torn edges, watermark.
//! Integer fields only so [`crate::Action`] stays `Eq`.
//!
//! TOML form:
//! ```toml
//! actions = [{ effects = { corner_radius = 8, shadow_blur = 16,
//!              watermark = { text = "© {date}", position = "bottom_right" } } }, "save"]
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectSpec {
    /// Rounded-corner radius in px (0 = square).
    pub corner_radius: u32,
    /// Border width in px (0 = none), drawn inside the rounded outline.
    pub border_width: u32,
    /// `#rrggbb` or `#rrggbbaa`.
    pub border_color: String,
    /// Torn ("ripped paper") edge per side.
    pub torn: TornEdges,
    /// Tooth height of torn edges in px.
    pub torn_size: u32,
    /// Drop-shadow blur radius in px (0 = no shadow). The output grows by
    /// the blur + offset so the shadow fits (transparent margin).
    pub shadow_blur: u32,
    pub shadow_offset_x: i32,
    pub shadow_offset_y: i32,
    /// `#rrggbb` or `#rrggbbaa`.
    pub shadow_color: String,
    pub watermark: Option<Watermark>,
}

impl Default for EffectSpec {
    fn default() -> Self {
        Self {
            corner_radius: 0,
            border_width: 0,
            border_color: "#000000".into(),
            torn: TornEdges::default(),
            torn_size: 12,
            shadow_blur: 0,
            shadow_offset_x: 0,
            shadow_offset_y: 6,
            shadow_color: "#00000066".into(),
            watermark: None,
        }
    }
}

impl EffectSpec {
    /// True when applying would change nothing.
    pub fn is_noop(&self) -> bool {
        self.corner_radius == 0
            && self.border_width == 0
            && !self.torn.any()
            && self.shadow_blur == 0
            && self.watermark.as_ref().is_none_or(|w| w.text.trim().is_empty())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TornEdges {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl TornEdges {
    pub fn any(&self) -> bool {
        self.top || self.right || self.bottom || self.left
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Watermark {
    /// Supports template placeholders (`{date}`, `{time}`, …); the caller
    /// expands them before rendering.
    pub text: String,
    pub position: Anchor,
    /// Font size in px.
    pub size: u32,
    /// `#rrggbb` or `#rrggbbaa`.
    pub color: String,
    /// 0..=100.
    pub opacity: u8,
    /// Distance from the image edge in px.
    pub margin: u32,
}

impl Default for Watermark {
    fn default() -> Self {
        Self {
            text: String::new(),
            position: Anchor::BottomRight,
            size: 18,
            color: "#ffffff".into(),
            opacity: 70,
            margin: 12,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    #[default]
    BottomRight,
}

/// Parse `#rrggbb` / `#rrggbbaa` (case-insensitive) into RGBA.
pub fn parse_hex_color(s: &str) -> Option<[u8; 4]> {
    let h = s.trim().strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    match h.len() {
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_noop_and_round_trips() {
        let s = EffectSpec::default();
        assert!(s.is_noop());
        let t = toml::to_string(&s).unwrap();
        assert_eq!(toml::from_str::<EffectSpec>(&t).unwrap(), s);
    }

    #[test]
    fn partial_toml() {
        let s: EffectSpec = toml::from_str("corner_radius = 8\n[watermark]\ntext = \"hi\"\nposition = \"top_left\"").unwrap();
        assert_eq!(s.corner_radius, 8);
        assert_eq!(s.watermark.as_ref().unwrap().position, Anchor::TopLeft);
        assert_eq!(s.watermark.unwrap().opacity, 70);
        assert!(!EffectSpec { torn: TornEdges { bottom: true, ..Default::default() }, ..Default::default() }.is_noop());
    }

    #[test]
    fn hex_colors() {
        assert_eq!(parse_hex_color("#FF8000"), Some([255, 128, 0, 255]));
        assert_eq!(parse_hex_color("#00000066"), Some([0, 0, 0, 0x66]));
        assert_eq!(parse_hex_color("123456"), None);
        assert_eq!(parse_hex_color("#12345"), None);
    }
}
