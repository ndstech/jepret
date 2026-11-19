//! OCR via `Windows.Media.Ocr` (offline, uses the user's installed OCR
//! languages).

use jepret_core::{Image, Result};

/// Recognised text, one line per OCR line, joined with `\n`.
pub fn recognize(image: &Image) -> Result<String> {
    // TODO(ocr): implement
    let _ = image;
    anyhow::bail!("OCR belum tersedia")
}
