//! Image effects (pure, no Win32/egui): rounded corners, border, drop
//! shadow, torn edges and a text watermark. Used by the workflow action
//! `effects` and by the editor's effect panel.

use jepret_core::{EffectSpec, Image};

/// Apply `spec` to `image` and return the result. Watermark text must be
/// already expanded (placeholders) by the caller. The output may be larger
/// than the input (shadow margin) and has transparent pixels outside the
/// shape (corners, torn edges, shadow area).
pub fn apply(image: &Image, spec: &EffectSpec) -> Image {
    // TODO(effects): implement
    let _ = spec;
    image.clone()
}

/// Size of [`apply`]'s output for an input of `w`×`h`, and the offset of
/// the original image's top-left inside it (for previews).
pub fn output_layout(w: u32, h: u32, spec: &EffectSpec) -> (u32, u32, i32, i32) {
    // TODO(effects): implement
    let _ = spec;
    (w, h, 0, 0)
}
