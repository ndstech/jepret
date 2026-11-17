//! HDR → SDR tone-mapping. Owner: **T02**. Pure functions, no Win32.
//!
//! Input is what WGC/DXGI give for an HDR monitor in
//! `DXGI_FORMAT_R16G16B16A16_FLOAT`: **scRGB linear**, where 1.0 = 80 nits,
//! values may exceed 1.0 or be negative (wide gamut).
//!
//! Output: [`Image`] RGBA8 sRGB, alpha = 255.
//!
//! Expected algorithm (T02 may improve, keep the signature):
//! 1. scale = 80 / sdr_white_nits  → SDR white maps to 1.0
//! 2. clamp negatives to 0 (or gamut-map Rec.2020→Rec.709 if wanted)
//! 3. soft-clip highlights above 1.0 (e.g. extended Reinhard on luminance)
//! 4. sRGB OETF, quantize to u8

use jepret_core::Image;

/// `src` holds raw f16 bits (`half::f16::to_bits`), 4 channels per pixel.
/// `row_pitch` is in **u16 elements** (≥ width*4) to allow GPU padding.
pub fn tonemap_scrgb_f16(
    _src: &[u16],
    _width: u32,
    _height: u32,
    _row_pitch: usize,
    _sdr_white_nits: f32,
) -> Image {
    todo!("T02: tonemap_scrgb_f16")
}

/// Linear (0..1) → sRGB encoded (0..1).
pub fn srgb_oetf(_linear: f32) -> f32 {
    todo!("T02: srgb_oetf")
}
