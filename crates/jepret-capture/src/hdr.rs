//! HDR → SDR tone-mapping. Owner: **T02**. Pure functions, no Win32.
//!
//! Input is what WGC/DXGI give for an HDR monitor in
//! `DXGI_FORMAT_R16G16B16A16_FLOAT`: **scRGB linear**, where 1.0 = 80 nits,
//! values may exceed 1.0 or be negative (wide gamut).
//!
//! Output: [`Image`] RGBA8 sRGB, alpha = 255.
//!
//! # Algorithm
//! 1. `scale = 80 / sdr_white_nits`, so the user's SDR white level (what
//!    Windows shows as "white" in the desktop) maps to linear 1.0. Everything
//!    at or below SDR white is therefore reproduced 1:1 (identity), which is
//!    what keeps a screenshot looking like the screen in an SDR viewer
//!    (UI/text is neither faded nor over-bright).
//! 2. Negative components (wide-gamut, out of Rec.709) are clamped to 0;
//!    NaN is treated as 0 and +inf as a very large finite value.
//! 3. Highlights above SDR white: **hue-preserving soft roll-off**. With
//!    `m = max(r, g, b) > 1` the pixel is scaled by `1/m` (so no channel
//!    clips independently and hue is kept) and then blended toward white by
//!    `t = 1 - 1/m`, a Reinhard-style desaturation: the brighter the
//!    highlight, the closer to white, like a film shoulder. Continuous and
//!    identity at `m = 1`. A bright coloured highlight (e.g. 5, 1, 1) does
//!    not become a hard-clipped saturated red; its G/B rise monotonically
//!    with brightness. Neutral highlights necessarily end at 255 because
//!    SDR white must stay exactly 255 (no headroom exists in 8-bit sRGB).
//!    A luminance-only extended Reinhard was rejected: it would darken SDR
//!    white (Y=1 → ~0.53) unless a knee is added, and still clips per channel.
//! 4. sRGB OETF via a 16K-entry lookup table, quantized to u8.
//!
//! Speed: f16 → linear is a 64K-entry table (scale + clamp folded in), f16 →
//! final u8 another table for pixels within SDR range (fast path), the OETF a
//! table, rows are processed on `std::thread::scope` workers.

use jepret_core::Image;

/// Resolution of the OETF lookup table (linear 0..1 → u8).
const OETF_LUT_SIZE: usize = 16384;

/// Linear (0..1) → sRGB encoded (0..1). Input is clamped to 0..1; NaN → 0.
pub fn srgb_oetf(linear: f32) -> f32 {
    // `!(x > 0.0)` also catches NaN.
    if linear.is_nan() || linear <= 0.0 {
        return 0.0;
    }
    if linear >= 1.0 {
        return 1.0;
    }
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

fn build_oetf_lut() -> Vec<u8> {
    (0..=OETF_LUT_SIZE)
        .map(|i| {
            let l = i as f32 / OETF_LUT_SIZE as f32;
            (srgb_oetf(l) * 255.0 + 0.5) as u8
        })
        .collect()
}

/// f16 bits → scaled, non-negative, finite linear value.
fn build_decode_lut(scale: f32) -> Vec<f32> {
    (0..=u16::MAX)
        .map(|bits| {
            let v = half::f16::from_bits(bits).to_f32();
            if v.is_nan() || v <= 0.0 {
                0.0
            } else {
                (v * scale).min(1.0e9)
            }
        })
        .collect()
}

/// Bit set in a `direct` LUT entry when the value is above SDR white.
const OVER: u16 = 0x100;

/// f16 bits → encoded u8 (low byte) for values within SDR range; entries above
/// SDR white carry [`OVER`] and need the roll-off path.
fn build_direct_lut(decode: &[f32], oetf: &[u8]) -> Vec<u16> {
    decode
        .iter()
        .map(|&v| {
            if v > 1.0 {
                OVER | 255
            } else {
                u16::from(oetf[(v * OETF_LUT_SIZE as f32 + 0.5) as usize])
            }
        })
        .collect()
}

/// Tone-map one row. `row` has `width*4` u16, `out` has `width*4` u8.
#[inline]
fn tonemap_row(row: &[u16], out: &mut [u8], decode: &[f32], direct: &[u16], oetf: &[u8]) {
    let lut_max = OETF_LUT_SIZE as f32;
    let enc = |v: f32| -> u8 { oetf[(v * lut_max + 0.5) as usize] };
    let (rows4, _) = row.as_chunks::<4>();
    let (outs4, _) = out.as_chunks_mut::<4>();
    for (px, o) in rows4.iter().zip(outs4.iter_mut()) {
        // Fast path: nothing above SDR white (the vast majority of desktop pixels).
        let (dr, dg, db) = (
            direct[px[0] as usize],
            direct[px[1] as usize],
            direct[px[2] as usize],
        );
        if (dr | dg | db) & OVER == 0 {
            o[0] = dr as u8;
            o[1] = dg as u8;
            o[2] = db as u8;
            o[3] = 255;
            continue;
        }
        let mut r = decode[px[0] as usize];
        let mut g = decode[px[1] as usize];
        let mut b = decode[px[2] as usize];
        let m = r.max(g).max(b);
        if m > 1.0 {
            let inv = 1.0 / m;
            let t = 1.0 - inv; // desaturation toward white
            r = r * inv * (1.0 - t) + t;
            g = g * inv * (1.0 - t) + t;
            b = b * inv * (1.0 - t) + t;
        }
        o[0] = enc(r.min(1.0));
        o[1] = enc(g.min(1.0));
        o[2] = enc(b.min(1.0));
        o[3] = 255;
    }
}

/// `src` holds raw f16 bits (`half::f16::to_bits`), 4 channels per pixel.
/// `row_pitch` is in **u16 elements** (≥ width*4) to allow GPU padding.
///
/// Never panics on bad input: a `row_pitch` smaller than `width*4` is raised
/// to `width*4`, a too-short `src` leaves the missing rows opaque black, and a
/// non-positive/NaN `sdr_white_nits` falls back to 80 nits (scale 1.0).
pub fn tonemap_scrgb_f16(
    src: &[u16],
    width: u32,
    height: u32,
    row_pitch: usize,
    sdr_white_nits: f32,
) -> Image {
    let w = width as usize;
    let h = height as usize;
    let mut img = Image::new(width, height);
    if w == 0 || h == 0 {
        return img;
    }
    let nits = if sdr_white_nits.is_finite() && sdr_white_nits > 0.0 {
        sdr_white_nits
    } else {
        80.0
    };
    let pitch = row_pitch.max(w * 4);
    let decode = build_decode_lut(80.0 / nits);
    let oetf = build_oetf_lut();
    let direct = build_direct_lut(&decode, &oetf);

    // Rows fully present in `src` (the last row only needs w*4 elements).
    let avail_rows = if src.len() < w * 4 {
        0
    } else {
        ((src.len() - w * 4) / pitch + 1).min(h)
    };
    // Rows without source data = opaque black (valid rows write their own alpha).
    let (tail, _) = img.pixels[avail_rows * w * 4..].as_chunks_mut::<4>();
    for px in tail {
        px[3] = 255;
    }
    if avail_rows == 0 {
        return img;
    }

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(8)
        .min(avail_rows.div_ceil(64).max(1));
    let rows_per = avail_rows.div_ceil(threads);

    let (decode, direct, oetf) = (&decode[..], &direct[..], &oetf[..]);
    let dst = &mut img.pixels[..avail_rows * w * 4];
    std::thread::scope(|s| {
        for (ci, chunk) in dst.chunks_mut(rows_per * w * 4).enumerate() {
            let first_row = ci * rows_per;
            s.spawn(move || {
                for (ri, out) in chunk.chunks_exact_mut(w * 4).enumerate() {
                    let start = (first_row + ri) * pitch;
                    tonemap_row(&src[start..start + w * 4], out, decode, direct, oetf);
                }
            });
        }
    });
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use half::f16;

    fn bits(v: f32) -> u16 {
        f16::from_f32(v).to_bits()
    }

    fn one(rgb: [f32; 3], nits: f32) -> [u8; 4] {
        let src = [bits(rgb[0]), bits(rgb[1]), bits(rgb[2]), bits(1.0)];
        let img = tonemap_scrgb_f16(&src, 1, 1, 4, nits);
        [img.pixels[0], img.pixels[1], img.pixels[2], img.pixels[3]]
    }

    #[test]
    fn a_sdr_white_is_255() {
        for nits in [80.0_f32, 200.0, 240.0, 480.0] {
            let v = nits / 80.0;
            assert_eq!(one([v, v, v], nits), [255, 255, 255, 255], "nits={nits}");
        }
    }

    #[test]
    fn b_zero_is_zero() {
        assert_eq!(one([0.0, 0.0, 0.0], 80.0), [0, 0, 0, 255]);
    }

    #[test]
    fn c_negative_nan_inf_do_not_panic() {
        assert_eq!(one([-0.5, -100.0, -0.0], 80.0), [0, 0, 0, 255]);
        let p = one([f32::NAN, f32::INFINITY, f32::NEG_INFINITY], 80.0);
        assert_eq!(p[3], 255);
        // bad white level falls back instead of panicking
        let _ = one([1.0, 1.0, 1.0], 0.0);
        let _ = one([1.0, 1.0, 1.0], f32::NAN);
    }

    #[test]
    fn d_highlights_do_not_hard_clip() {
        // Gray ramp: monotonic non-decreasing.
        let mut prev = 0u8;
        for k in [0.1_f32, 0.5, 1.0, 2.0, 4.0, 10.0, 100.0, 60000.0] {
            let p = one([k, k, k], 80.0)[0];
            assert!(p >= prev, "gray {k}: {p} < {prev}");
            prev = p;
        }
        // Coloured highlight: G/B strictly rise (roll to white) instead of
        // the saturated channel clipping alone.
        let mut prev_g = 0u8;
        for k in [1.0_f32, 2.0, 4.0, 10.0, 50.0] {
            let p = one([k, 0.5, 0.5], 80.0);
            assert_eq!(p[0], 255);
            assert!(p[1] > prev_g || k == 1.0, "k={k} g={} prev={prev_g}", p[1]);
            assert_eq!(p[1], p[2]);
            prev_g = p[1];
        }
        let p = one([8.0, 2.0, 0.5], 80.0);
        assert!(p[0] >= p[1] && p[1] >= p[2]);
    }

    #[test]
    fn e_row_pitch_larger_than_width() {
        let (w, h, pitch) = (3usize, 2usize, 3 * 4 + 5);
        let mut src = vec![bits(0.5); pitch * h]; // padding = junk 0.5
        let white = [bits(1.0), bits(1.0), bits(1.0), bits(1.0)];
        let black = [bits(0.0), bits(0.0), bits(0.0), bits(1.0)];
        for y in 0..h {
            for x in 0..w {
                let px = if (x + y) % 2 == 0 { white } else { black };
                src[y * pitch + x * 4..y * pitch + x * 4 + 4].copy_from_slice(&px);
            }
        }
        let img = tonemap_scrgb_f16(&src, w as u32, h as u32, pitch, 80.0);
        assert_eq!(img.pixels.len(), w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let want = if (x + y) % 2 == 0 { 255 } else { 0 };
                let i = (y * w + x) * 4;
                assert_eq!(&img.pixels[i..i + 4], &[want, want, want, 255], "({x},{y})");
            }
        }
    }

    #[test]
    fn mid_gray_matches_srgb_oetf() {
        // 0.5 linear → sRGB ≈ 188
        let p = one([0.5, 0.5, 0.5], 80.0)[0];
        assert_eq!(p, (srgb_oetf(0.5) * 255.0 + 0.5) as u8);
        assert!((187..=189).contains(&p));
    }

    #[test]
    fn short_or_empty_input_does_not_panic() {
        let img = tonemap_scrgb_f16(&[], 4, 4, 16, 80.0);
        assert_eq!(img.pixels.len(), 64);
        assert_eq!(&img.pixels[..4], &[0, 0, 0, 255]);
        let img = tonemap_scrgb_f16(&[], 0, 0, 0, 80.0);
        assert!(img.pixels.is_empty());
        // pitch < width*4 is tolerated
        let src = vec![bits(1.0); 8];
        let _ = tonemap_scrgb_f16(&src, 2, 1, 1, 80.0);
    }

    #[test]
    fn oetf_endpoints() {
        assert_eq!(srgb_oetf(0.0), 0.0);
        assert_eq!(srgb_oetf(1.0), 1.0);
        assert_eq!(srgb_oetf(-1.0), 0.0);
        assert_eq!(srgb_oetf(5.0), 1.0);
        assert!(srgb_oetf(0.2) < srgb_oetf(0.3));
    }

    /// 4K timing; run: `cargo test -p jepret-capture --release -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_4k() {
        let (w, h) = (3840usize, 2160usize);
        let src: Vec<u16> = (0..w * h * 4)
            .map(|i| bits(((i * 7919) % 1000) as f32 / 250.0))
            .collect();
        let _ = tonemap_scrgb_f16(&src, w as u32, h as u32, w * 4, 240.0);
        // Best of N: other processes on a shared machine only ever add time.
        let mut best = f64::MAX;
        for _ in 0..20 {
            let t = std::time::Instant::now();
            std::hint::black_box(tonemap_scrgb_f16(&src, w as u32, h as u32, w * 4, 240.0));
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        }
        let ms = best;
        println!("4K tonemap: {ms:.2} ms/frame (best of 20)");
        assert!(ms <= 30.0, "too slow: {ms} ms");
    }
}
