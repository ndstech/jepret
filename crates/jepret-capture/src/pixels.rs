//! Pure pixel helpers (no Win32): BGRA→RGBA, blitting, composition.

// `as_chunks` needs Rust 1.88; the workspace MSRV is 1.80.
#![allow(clippy::chunks_exact_to_as_chunks)]

use jepret_core::{Image, Rect};

/// Convert a (possibly row-padded) BGRA8 buffer to an opaque RGBA [`Image`].
/// `row_pitch` is in bytes. Alpha is forced to 255 (desktop is opaque).
pub fn bgra_to_rgba(src: &[u8], width: u32, height: u32, row_pitch: usize) -> Image {
    let w = width as usize;
    let mut pixels = vec![0u8; w * height as usize * 4];
    for y in 0..height as usize {
        let s = &src[y * row_pitch..y * row_pitch + w * 4];
        let d = &mut pixels[y * w * 4..(y + 1) * w * 4];
        for (sp, dp) in s.chunks_exact(4).zip(d.chunks_exact_mut(4)) {
            dp[0] = sp[2];
            dp[1] = sp[1];
            dp[2] = sp[0];
            dp[3] = 255;
        }
    }
    Image { width, height, pixels }
}

/// Copy `src` into `dst` with its top-left at (`x`,`y`) in `dst`
/// coordinates; parts outside `dst` are clipped.
pub fn blit(dst: &mut Image, src: &Image, x: i32, y: i32) {
    let bounds = Rect::new(0, 0, dst.width, dst.height);
    let placed = Rect::new(x, y, src.width, src.height);
    let Some(r) = bounds.intersect(&placed) else { return };
    let row_bytes = r.width as usize * 4;
    for dy in r.y..r.bottom() {
        let sy = (dy - y) as usize;
        let sx = (r.x - x) as usize;
        let s = sy * src.stride() + sx * 4;
        let d = dy as usize * dst.stride() + r.x as usize * 4;
        dst.pixels[d..d + row_bytes].copy_from_slice(&src.pixels[s..s + row_bytes]);
    }
}

/// Compose per-monitor images (each with its virtual-desktop rect) into one
/// image covering `area`. Uncovered pixels stay transparent black.
pub fn compose(area: Rect, parts: &[(Image, Rect)]) -> Image {
    let mut out = Image::new(area.width, area.height);
    for (img, rect) in parts {
        blit(&mut out, img, rect.x - area.x, rect.y - area.y);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_swizzle_and_pitch() {
        // 2x2, pitch 12 bytes (4 bytes padding per row)
        let src = [
            1, 2, 3, 0, 4, 5, 6, 0, 99, 99, 99, 99, //
            7, 8, 9, 0, 10, 11, 12, 0, 99, 99, 99, 99,
        ];
        let img = bgra_to_rgba(&src, 2, 2, 12);
        assert_eq!(img.pixel(0, 0), [3, 2, 1, 255]);
        assert_eq!(img.pixel(1, 0), [6, 5, 4, 255]);
        assert_eq!(img.pixel(0, 1), [9, 8, 7, 255]);
        assert_eq!(img.pixel(1, 1), [12, 11, 10, 255]);
    }

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Image {
        let mut i = Image::new(w, h);
        for p in i.pixels.chunks_exact_mut(4) {
            p.copy_from_slice(&c);
        }
        i
    }

    #[test]
    fn compose_negative_coords_and_gap() {
        let left = (solid(2, 2, [255, 0, 0, 255]), Rect::new(-2, 0, 2, 2));
        let right = (solid(2, 1, [0, 255, 0, 255]), Rect::new(1, 0, 2, 1));
        let area = Rect::new(-2, 0, 5, 2);
        let out = compose(area, &[left, right]);
        assert_eq!((out.width, out.height), (5, 2));
        assert_eq!(out.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(out.pixel(1, 1), [255, 0, 0, 255]);
        assert_eq!(out.pixel(2, 0), [0, 0, 0, 0]); // gap x=0
        assert_eq!(out.pixel(3, 0), [0, 255, 0, 255]); // x=1
        assert_eq!(out.pixel(3, 1), [0, 0, 0, 0]); // right is only 1 tall
    }

    #[test]
    fn blit_clips() {
        let mut dst = Image::new(3, 3);
        let src = solid(3, 3, [1, 1, 1, 255]);
        blit(&mut dst, &src, -2, 2);
        assert_eq!(dst.pixel(0, 2), [1, 1, 1, 255]);
        assert_eq!(dst.pixel(1, 2), [0, 0, 0, 0]);
        blit(&mut dst, &src, 10, 10); // fully outside: no panic
    }
}
