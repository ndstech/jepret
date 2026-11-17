//! Image file IO for the editor (decode / encode / clipboard). No egui.

use std::path::Path;

use anyhow::Context;
use jepret_core::config::ImageFormat;
use jepret_core::{Image, Result};

/// Decode a PNG/JPEG file into an RGBA8 [`Image`].
pub fn load_image(path: &Path) -> Result<Image> {
    let dynimg = image::ImageReader::open(path)
        .with_context(|| format!("open {}", path.display()))?
        .with_guessed_format()?
        .decode()
        .with_context(|| format!("decode {}", path.display()))?;
    let rgba = dynimg.into_rgba8();
    let (w, h) = rgba.dimensions();
    Image::from_raw(w, h, rgba.into_raw())
}

/// Format to use for `path`: by extension, falling back to `fallback`.
pub fn format_for(path: &Path, fallback: ImageFormat) -> ImageFormat {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()) {
        Some(e) if e == "png" => ImageFormat::Png,
        Some(e) if e == "jpg" || e == "jpeg" => ImageFormat::Jpeg,
        _ => fallback,
    }
}

/// Encode `img` to `path` (PNG fast-compression, or JPEG at `jpeg_quality`).
pub fn save_image(img: &Image, path: &Path, fallback: ImageFormat, jpeg_quality: u8) -> Result<()> {
    use image::codecs::{jpeg::JpegEncoder, png::{CompressionType, FilterType, PngEncoder}};
    use image::ExtendedColorType;
    use std::io::BufWriter;

    let file = std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
    let mut out = BufWriter::new(file);
    match format_for(path, fallback) {
        ImageFormat::Png => {
            use image::ImageEncoder;
            PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive)
                .write_image(&img.pixels, img.width, img.height, ExtendedColorType::Rgba8)?;
        }
        ImageFormat::Jpeg => {
            use image::ImageEncoder;
            let mut rgb = Vec::with_capacity(img.width as usize * img.height as usize * 3);
            for p in img.pixels.chunks_exact(4) {
                rgb.extend_from_slice(&p[..3]);
            }
            JpegEncoder::new_with_quality(&mut out, jpeg_quality.clamp(1, 100))
                .write_image(&rgb, img.width, img.height, ExtendedColorType::Rgb8)?;
        }
    }
    use std::io::Write;
    out.flush()?;
    Ok(())
}

/// Put `img` on the system clipboard.
pub fn copy_to_clipboard(img: &Image) -> Result<()> {
    let mut cb = arboard::Clipboard::new().context("open clipboard")?;
    cb.set_image(arboard::ImageData {
        width: img.width as usize,
        height: img.height as usize,
        bytes: std::borrow::Cow::Borrowed(&img.pixels),
    })
    .context("set clipboard image")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_roundtrip() {
        let mut img = Image::new(8, 6);
        for (i, p) in img.pixels.chunks_exact_mut(4).enumerate() {
            p.copy_from_slice(&[i as u8, 255 - i as u8, 7, 255]);
        }
        let dir = std::env::temp_dir().join(format!("jepret-editor-io-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.png");
        save_image(&img, &p, ImageFormat::Png, 90).unwrap();
        let back = load_image(&p).unwrap();
        assert_eq!(back.pixels, img.pixels);
        let j = dir.join("t.jpg");
        save_image(&img, &j, ImageFormat::Png, 90).unwrap();
        let back = load_image(&j).unwrap();
        assert_eq!((back.width, back.height), (8, 6));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn format_by_extension() {
        assert_eq!(format_for(Path::new("a.JPG"), ImageFormat::Png), ImageFormat::Jpeg);
        assert_eq!(format_for(Path::new("a.png"), ImageFormat::Jpeg), ImageFormat::Png);
        assert_eq!(format_for(Path::new("a"), ImageFormat::Jpeg), ImageFormat::Jpeg);
    }

    /// Release-only timing for the T06 AC (open 4K PNG <= 300 ms):
    /// `cargo test -p jepret-editor --release -- --ignored --nocapture load_4k`
    #[test]
    #[ignore]
    fn load_4k_timing() {
        let (w, h) = (3840u32, 2160u32);
        let mut img = Image::new(w, h);
        // screenshot-like content: gradients + noise-ish text-like pattern
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize * 4;
                let n = ((x * 31 + y * 17) ^ (x >> 3)) as u8;
                let v = if (x / 40 + y / 24) % 5 == 0 { n } else { (x / 16) as u8 };
                img.pixels[i..i + 4].copy_from_slice(&[v, (y / 9) as u8, v ^ 0x55, 255]);
            }
        }
        let p = std::env::temp_dir().join(format!("jepret-4k-{}.png", std::process::id()));
        save_image(&img, &p, ImageFormat::Png, 90).unwrap();
        let size = std::fs::metadata(&p).unwrap().len();
        let mut best = u128::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            let back = load_image(&p).unwrap();
            best = best.min(t.elapsed().as_millis());
            assert_eq!((back.width, back.height), (w, h));
        }
        let t = std::time::Instant::now();
        let c = crate::flatten(&img, &crate::Document::new());
        let flat = t.elapsed().as_millis();
        let _ = std::fs::remove_file(&p);
        println!("4K PNG ({} KB): best decode = {best} ms; flatten(no shapes) = {flat} ms", size / 1024);
        assert_eq!(c.width, w);
    }
}
