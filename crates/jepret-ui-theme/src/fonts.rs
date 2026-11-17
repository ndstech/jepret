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

/// Put the system fonts first in each family, keeping egui's defaults as
/// fallbacks (they cover symbols such as arrows and ✕).
pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    for (family, candidates) in [
        (FontFamily::Proportional, UI_CANDIDATES),
        (FontFamily::Monospace, MONO_CANDIDATES),
    ] {
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
