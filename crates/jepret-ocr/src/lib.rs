//! OCR via `Windows.Media.Ocr` (offline, uses the user's installed OCR
//! languages).
//!
//! `recognize` blocks; call it from a worker thread.

use jepret_core::{Image, Result};

/// Images whose longest side is below this are upscaled 2x before OCR.
const UPSCALE_BELOW: u32 = 800;

/// Fallback when the engine reports no max dimension (it normally is 10000).
#[cfg(windows)]
const DEFAULT_MAX_DIMENSION: u32 = 10_000;

/// What to do with an image before handing it to the OCR engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalePlan {
    Keep,
    Resize { width: u32, height: u32 },
}

/// Decide scaling: shrink when larger than `max_dim`, 2x upscale when tiny
/// (as long as the result still fits in `max_dim`).
pub fn plan_scale(width: u32, height: u32, max_dim: u32) -> ScalePlan {
    if width == 0 || height == 0 || max_dim == 0 {
        return ScalePlan::Keep;
    }
    let longest = width.max(height);
    if longest > max_dim {
        let f = max_dim as f64 / longest as f64;
        let w = ((width as f64 * f).round() as u32).clamp(1, max_dim);
        let h = ((height as f64 * f).round() as u32).clamp(1, max_dim);
        return ScalePlan::Resize { width: w, height: h };
    }
    if longest < UPSCALE_BELOW && longest * 2 <= max_dim {
        return ScalePlan::Resize { width: width * 2, height: height * 2 };
    }
    ScalePlan::Keep
}

/// RGBA straight alpha -> BGRA premultiplied alpha.
pub fn rgba_to_bgra_premul(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for p in rgba.as_chunks::<4>().0 {
        let a = p[3] as u32;
        let m = |c: u8| ((c as u32 * a + 127) / 255) as u8;
        out.extend_from_slice(&[m(p[2]), m(p[1]), m(p[0]), p[3]]);
    }
    out
}

/// Bilinear resize of an RGBA8 buffer.
pub fn resize_rgba(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let (sw, sh, dw, dh) = (sw as usize, sh as usize, dw as usize, dh as usize);
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || src.len() < sw * sh * 4 {
        return vec![0; dw * dh * 4];
    }
    let mut out = vec![0u8; dw * dh * 4];
    let xr = sw as f32 / dw as f32;
    let yr = sh as f32 / dh as f32;
    for y in 0..dh {
        let fy = ((y as f32 + 0.5) * yr - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let wy = fy - y0 as f32;
        for x in 0..dw {
            let fx = ((x as f32 + 0.5) * xr - 0.5).clamp(0.0, (sw - 1) as f32);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let wx = fx - x0 as f32;
            for c in 0..4 {
                let p = |xx: usize, yy: usize| src[(yy * sw + xx) * 4 + c] as f32;
                let top = p(x0, y0) * (1.0 - wx) + p(x1, y0) * wx;
                let bot = p(x0, y1) * (1.0 - wx) + p(x1, y1) * wx;
                out[(y * dw + x) * 4 + c] = (top * (1.0 - wy) + bot * wy + 0.5) as u8;
            }
        }
    }
    out
}

/// Join OCR lines with `\n`, dropping trailing whitespace and empty lines.
pub fn join_lines<I, S>(lines: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    lines
        .into_iter()
        .map(|l| l.as_ref().trim_end().to_string())
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Recognised text, one line per OCR line, joined with `\n`.
pub fn recognize(image: &Image) -> Result<String> {
    #[cfg(windows)]
    {
        win::recognize(image)
    }
    #[cfg(not(windows))]
    {
        let _ = image;
        anyhow::bail!("OCR hanya tersedia di Windows")
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use anyhow::{anyhow, Context};
    use tracing::{debug, warn};
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

    const NO_LANG: &str =
        "Bahasa OCR belum terpasang di Windows (Settings › Time & language › Language)";

    fn init_apartment() {
        // SAFETY: plain WinRT init for this thread; failure (e.g. the thread
        // is already in another apartment) is tolerated.
        if let Err(e) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            debug!("RoInitialize: {e} (diabaikan)");
        }
    }

    fn create_engine() -> Result<OcrEngine> {
        if let Ok(e) = OcrEngine::TryCreateFromUserProfileLanguages() {
            return Ok(e);
        }
        warn!("OCR: bahasa profil pengguna tidak didukung, coba bahasa pertama yang tersedia");
        let langs = OcrEngine::AvailableRecognizerLanguages().map_err(|_| anyhow!(NO_LANG))?;
        for lang in &langs {
            if let Ok(e) = OcrEngine::TryCreateFromLanguage(&lang) {
                return Ok(e);
            }
        }
        Err(anyhow!(NO_LANG))
    }

    pub fn recognize(image: &Image) -> Result<String> {
        anyhow::ensure!(image.width > 0 && image.height > 0, "gambar kosong");
        init_apartment();
        let engine = create_engine()?;

        let max = OcrEngine::MaxImageDimension().unwrap_or(DEFAULT_MAX_DIMENSION);
        let (w, h, bgra) = match plan_scale(image.width, image.height, max) {
            ScalePlan::Keep => (image.width, image.height, rgba_to_bgra_premul(&image.pixels)),
            ScalePlan::Resize { width, height } => {
                debug!("OCR: resize {}x{} -> {width}x{height}", image.width, image.height);
                let r = resize_rgba(&image.pixels, image.width, image.height, width, height);
                (width, height, rgba_to_bgra_premul(&r))
            }
        };

        let writer = DataWriter::new().context("DataWriter")?;
        writer.WriteBytes(&bgra).context("WriteBytes")?;
        let buffer = writer.DetachBuffer().context("DetachBuffer")?;
        let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            w as i32,
            h as i32,
            BitmapAlphaMode::Premultiplied,
        )
        .context("SoftwareBitmap")?;

        let t = std::time::Instant::now();
        let result = engine
            .RecognizeAsync(&bitmap)
            .context("RecognizeAsync")?
            .get()
            .context("OCR gagal")?;
        let mut lines = Vec::new();
        for line in result.Lines().context("Lines")? {
            lines.push(line.Text().context("Text")?.to_string());
        }
        debug!("OCR: {} baris dalam {:?}", lines.len(), t.elapsed());
        Ok(join_lines(lines))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_keep_midsize() {
        assert_eq!(plan_scale(1920, 1080, 10_000), ScalePlan::Keep);
    }

    #[test]
    fn scale_up_tiny() {
        assert_eq!(plan_scale(300, 40, 10_000), ScalePlan::Resize { width: 600, height: 80 });
    }

    #[test]
    fn scale_down_huge() {
        assert_eq!(
            plan_scale(20_000, 10_000, 10_000),
            ScalePlan::Resize { width: 10_000, height: 5_000 }
        );
    }

    #[test]
    fn scale_no_upscale_over_max() {
        assert_eq!(plan_scale(700, 10, 1000), ScalePlan::Keep);
    }

    #[test]
    fn scale_degenerate() {
        assert_eq!(plan_scale(0, 10, 100), ScalePlan::Keep);
    }

    #[test]
    fn premul_basic() {
        let out = rgba_to_bgra_premul(&[255, 128, 0, 255, 200, 100, 50, 0, 100, 100, 100, 128]);
        assert_eq!(&out[0..4], &[0, 128, 255, 255]);
        assert_eq!(&out[4..8], &[0, 0, 0, 0]);
        assert_eq!(&out[8..12], &[50, 50, 50, 128]);
    }

    #[test]
    fn resize_dims_and_flat_color() {
        let src = [10u8, 20, 30, 255].repeat(4);
        let out = resize_rgba(&src, 2, 2, 4, 4);
        assert_eq!(out.len(), 64);
        assert!(out.as_chunks::<4>().0.iter().all(|p| *p == [10, 20, 30, 255]));
    }

    #[test]
    fn join() {
        assert_eq!(join_lines(["a b ", "", "  ", "c"]), "a b\nc");
        assert_eq!(join_lines(Vec::<String>::new()), "");
    }
}
