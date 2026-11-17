//! Jepret's shared look — the code form of the design canvas
//! (https://claude.ai/artifact/QJGaMEX5iCXhnkXdVajf1c, artboard "Token desain & ikon").
//!
//! * [`tokens`] — colours (light/dark), radii, spacing, sizes. **Never hard-code
//!   a colour in a UI crate; take it from [`Palette`].**
//! * [`fonts`] — Segoe UI Variable + Cascadia Mono from `C:\Windows\Fonts`
//!   (stand-ins for the mockups' Inter / JetBrains Mono; zero bytes added to
//!   the exe), egui's bundled fonts as fallback.
//! * [`icons`] — every icon in the design as an SVG path, painted with egui.
//! * [`widgets`] — kbd chip, buttons, icon button, toggle, segmented control,
//!   section header, info/warn banner.
//!
//! Usage in an eframe app:
//! ```ignore
//! let pal = jepret_ui_theme::install(&cc.egui_ctx, config.general.theme);
//! // keep `pal` in your app state; pass `&pal` to widgets
//! ```

pub mod fonts;
pub mod icons;
pub mod tokens;
pub mod widgets;

pub use icons::Icon;
pub use tokens::Palette;

use jepret_core::config::ThemeMode;

/// Resolve the mode, load fonts, apply visuals + spacing. Returns the palette
/// the caller should keep and use for custom painting.
pub fn install(ctx: &egui::Context, mode: ThemeMode) -> Palette {
    fonts::install(ctx);
    let pal = Palette::for_mode(mode);
    apply(ctx, &pal);
    pal
}

/// Apply a palette to egui's global style (visuals, spacing, text sizes).
pub fn apply(ctx: &egui::Context, pal: &Palette) {
    ctx.set_style(tokens::style(pal));
}

/// Whether Windows "app mode" is dark (`AppsUseLightTheme == 0`).
/// Defaults to light when it cannot be read.
pub fn system_prefers_dark() -> bool {
    #[cfg(windows)]
    {
        use windows::core::w;
        use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
        let mut value: u32 = 1;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: valid out pointers sized for a DWORD.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
                w!("AppsUseLightTheme"),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut value as *mut u32 as *mut _),
                Some(&mut size),
            )
        };
        r.is_ok() && value == 0
    }
    #[cfg(not(windows))]
    false
}
