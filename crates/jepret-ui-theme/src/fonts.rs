//! System fonts: Segoe UI Variable (UI) and Cascadia Mono (kbd / numbers),
//! read from `%WINDIR%\Fonts` at startup. Missing files fall back to the
//! older Segoe UI / Consolas, then to egui's bundled fonts — never an error.

use egui::{FontData, FontDefinitions, FontFamily};

const UI_CANDIDATES: &[&str] = &["SegUIVar.ttf", "segoeui.ttf"];
const MONO_CANDIDATES: &[&str] = &["CascadiaMono.ttf", "consola.ttf"];

fn fonts_dir() -> std::path::PathBuf {
    std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into())
        .join("Fonts")
}

fn load_first(names: &[&str]) -> Option<(String, Vec<u8>)> {
    let dir = fonts_dir();
    names.iter().find_map(|n| {
        std::fs::read(dir.join(n))
            .ok()
            .map(|bytes| (n.to_string(), bytes))
    })
}

/// The UI font file (Segoe UI Variable / Segoe UI), read once and shared, so
/// CPU text rendering (editor save) matches what egui shows.
pub fn ui_font() -> Option<(&'static str, &'static [u8])> {
    use std::sync::OnceLock;
    static UI: OnceLock<Option<(String, Vec<u8>)>> = OnceLock::new();
    UI.get_or_init(|| load_first(UI_CANDIDATES))
        .as_ref()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
}

/// Put the system fonts first in each family, keeping egui's defaults as
/// fallbacks (they cover symbols such as arrows and ✕).
pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    match ui_font() {
        Some((name, bytes)) => {
            defs.font_data.insert(name.to_string(), FontData::from_static(bytes));
            defs.families.entry(FontFamily::Proportional).or_default().insert(0, name.to_string());
        }
        None => tracing::warn!("no system font found among {UI_CANDIDATES:?}; using egui default"),
    }
    for (family, candidates) in [(FontFamily::Monospace, MONO_CANDIDATES)] {
        match load_first(candidates) {
            Some((name, bytes)) => {
                defs.font_data.insert(name.clone(), FontData::from_owned(bytes));
                defs.families.entry(family).or_default().insert(0, name);
            }
            None => tracing::warn!("no system font found among {candidates:?}; using egui default"),
        }
    }
    ctx.set_fonts(defs);
}
