//! Synthetic scroll-capture tests for `Stitcher` (seeded, deterministic).

use jepret_core::Image;
use jepret_scroll::{PushResult, StitchOptions, Stitcher};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + (self.next() % (hi - lo + 1) as u64) as usize
    }
    fn color(&mut self) -> [u8; 4] {
        let v = self.next();
        [v as u8, (v >> 8) as u8, (v >> 16) as u8, 255]
    }
}

/// Tall page of blank / text-noise / partial-box blocks. Text blocks alternate with blank/box blocks.
fn gen_page(rng: &mut Rng, w: usize, h: usize) -> Image {
    let mut px = Vec::with_capacity(w * h * 4);
    let bg = [250, 250, 250, 255];
    let mut y = 0;
    let mut text_next = true;
    while y < h {
        // Alternate text blocks with blank/box blocks so any 48-row window has text.
        let kind = if text_next { 1 } else { rng.range(0, 1) * 2 };
        let len = if text_next { rng.range(20, 40) } else { rng.range(8, 30) }.min(h - y);
        text_next = !text_next;
        let fill = rng.color();
        let (x0, x1) = (rng.range(0, w / 2), rng.range(w / 2 + 1, w));
        for _ in 0..len {
            for x in 0..w {
                let p = match kind {
                    0 => bg,
                    1 => {
                        if rng.range(0, 3) == 0 {
                            rng.color()
                        } else {
                            bg
                        }
                    }
                    _ => {
                        if (x0..x1).contains(&x) {
                            fill
                        } else {
                            bg
                        }
                    }
                };
                px.extend_from_slice(&p);
            }
        }
        y += len;
    }
    Image::from_raw(w as u32, h as u32, px).unwrap()
}

fn rows(img: &Image, from: usize, to: usize) -> &[u8] {
    let s = img.width as usize * 4;
    &img.pixels[from * s..to * s]
}

fn noise_band(rng: &mut Rng, w: usize, h: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(w * h * 4);
    for _ in 0..w * h {
        v.extend_from_slice(&rng.color());
    }
    v
}

fn frame(w: usize, header: &[u8], body: &[u8], footer: &[u8], h: usize) -> Image {
    let mut px = Vec::new();
    px.extend_from_slice(header);
    px.extend_from_slice(body);
    px.extend_from_slice(footer);
    Image::from_raw(w as u32, h as u32, px).unwrap()
}

#[test]
fn random_pages_reconstruct_exactly() {
    for seed in 0..60u64 {
        let mut rng = Rng(seed * 7919 + 1);
        let w = rng.range(100, 220);
        let ht = if seed % 3 == 0 { 0 } else { rng.range(8, 40) };
        let fb = if seed % 4 == 0 { 0 } else { rng.range(8, 40) };
        let bh = rng.range(140, 260);
        let h = ht + bh + fb;
        let nframes = rng.range(3, 9);
        let page_h = bh + (nframes - 1) * (bh - 48) + 10;
        let page = gen_page(&mut rng, w, page_h);
        let header = noise_band(&mut rng, w, ht);
        let footer = noise_band(&mut rng, w, fb);

        let mut st = Stitcher::new(StitchOptions::default());
        let mut s = 0;
        let mut last_s = 0;
        for k in 0..nframes {
            if k > 0 {
                s = (s + rng.range(10, bh - 48)).min(page_h - bh);
            }
            let f = frame(w, &header, rows(&page, s, s + bh), &footer, h);
            let r = st.push(&f);
            if k > 0 && s == last_s {
                assert_eq!(r, PushResult::NoChange, "seed {seed} frame {k}");
            } else if k > 0 {
                assert_eq!(r, PushResult::Appended { rows: (s - last_s) as u32 }, "seed {seed} frame {k}");
            }
            last_s = s;
            // Occasionally repeat the frame: must be NoChange.
            if rng.range(0, 3) == 0 {
                assert_eq!(st.push(&f), PushResult::NoChange, "seed {seed} dup");
            }
        }
        let out = st.finish();
        let mut expect = header.clone();
        expect.extend_from_slice(rows(&page, 0, last_s + bh));
        expect.extend_from_slice(&footer);
        assert_eq!(out.width as usize, w, "seed {seed}");
        assert_eq!(out.height as usize, ht + last_s + bh + fb, "seed {seed}");
        assert!(out.pixels == expect, "seed {seed}: pixels differ");
    }
}

#[test]
fn manual_sticky_without_autodetect() {
    let mut rng = Rng(42);
    let (w, ht, fb, bh) = (150, 20, 16, 200);
    let page = gen_page(&mut rng, w, 700);
    let header = noise_band(&mut rng, w, ht);
    let footer = noise_band(&mut rng, w, fb);
    let opts = StitchOptions { ignore_top: ht as u32, ignore_bottom: fb as u32, auto_detect_sticky: false, ..Default::default() };
    let mut st = Stitcher::new(opts);
    for s in [0usize, 90, 200, 300] {
        let f = frame(w, &header, rows(&page, s, s + bh), &footer, ht + bh + fb);
        assert_ne!(st.push(&f), PushResult::NoMatch);
    }
    let out = st.finish();
    let mut expect = header;
    expect.extend_from_slice(rows(&page, 0, 500));
    expect.extend_from_slice(&footer);
    assert!(out.pixels == expect);
}

#[test]
fn no_change_and_no_match() {
    let mut rng = Rng(7);
    let w = 120;
    let page = gen_page(&mut rng, w, 600);
    let mut st = Stitcher::new(StitchOptions::default());
    let f0 = Image::from_raw(w as u32, 200, rows(&page, 0, 200).to_vec()).unwrap();
    assert_eq!(st.push(&f0), PushResult::Appended { rows: 200 });
    assert_eq!(st.push(&f0), PushResult::NoChange);
    assert_eq!(st.height(), 200);
    // Unrelated content.
    let other = gen_page(&mut Rng(999), w, 200);
    assert_eq!(st.push(&other), PushResult::NoMatch);
    // Scrolled past the previous frame (no overlap).
    let far = Image::from_raw(w as u32, 200, rows(&page, 350, 550).to_vec()).unwrap();
    assert_eq!(st.push(&far), PushResult::NoMatch);
    assert_eq!(st.height(), 200);
    // Wrong size is rejected, not a panic.
    assert_eq!(st.push(&Image::new(10, 10)), PushResult::NoMatch);
    // State is intact: a valid scroll still stitches.
    let ok = Image::from_raw(w as u32, 200, rows(&page, 100, 300).to_vec()).unwrap();
    assert_eq!(st.push(&ok), PushResult::Appended { rows: 100 });
    assert_eq!(st.finish().pixels, rows(&page, 0, 300));
}

#[test]
fn empty_stitcher_finishes_empty() {
    let st = Stitcher::new(StitchOptions::default());
    assert_eq!(st.height(), 0);
    let img = st.finish();
    assert_eq!((img.width, img.height), (0, 0));
}

#[test]
fn tolerates_antialiasing_noise() {
    let mut rng = Rng(5);
    let w = 160;
    let page = gen_page(&mut rng, w, 800);
    let mut st = Stitcher::new(StitchOptions::default());
    for s in [0usize, 120, 260, 400] {
        let mut px = rows(&page, s, s + 300).to_vec();
        if s > 0 {
            // +-3 jitter on ~1 in 200 channels (not on the first frame).
            for i in (0..px.len()).step_by(197) {
                if (i / (w * 4)) % 4 == 0 {
                    px[i] = px[i].saturating_add(3);
                }
            }
        }
        let f = Image::from_raw(w as u32, 300, px).unwrap();
        assert_ne!(st.push(&f), PushResult::NoMatch, "s={s}");
    }
    assert_eq!(st.height(), 700);
}

#[test]
fn timing_1080p_push() {
    let (w, h, step) = (1920usize, 1080usize, 300usize);
    let mut rng = Rng(2024);
    let n = 8;
    let page = gen_page(&mut rng, w, h + step * n);
    let mut st = Stitcher::new(StitchOptions::default());
    let mut times = Vec::new();
    for k in 0..=n {
        let f = Image::from_raw(w as u32, h as u32, rows(&page, k * step, k * step + h).to_vec()).unwrap();
        let t = std::time::Instant::now();
        let r = st.push(&f);
        times.push(t.elapsed());
        assert_ne!(r, PushResult::NoMatch);
    }
    assert_eq!(st.height() as usize, h + step * n);
    eprintln!("per push: {times:?}");
    times.sort();
    let median = times[times.len() / 2];
    let max = times[times.len() - 1];
    eprintln!("1080p push: median {median:?}, max {max:?}");
    if !cfg!(debug_assertions) {
        assert!(max.as_millis() <= 20, "push too slow: {max:?}");
    }
}
