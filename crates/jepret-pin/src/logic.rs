//! Pure pin-window math (no egui / Win32). Units are documented per function.

pub const MIN_ZOOM: f32 = 0.10;
pub const MAX_ZOOM: f32 = 4.00;
pub const ZOOM_STEPS: [f32; 10] = [0.10, 0.25, 0.50, 0.75, 1.00, 1.25, 1.50, 2.00, 3.00, 4.00];

pub const MIN_OPACITY: u8 = 20;
pub const MAX_OPACITY: u8 = 100;
pub const OPACITY_STEP: u8 = 10;

const EPS: f32 = 1e-3;

/// Next larger preset zoom (clamped to [`MAX_ZOOM`]).
pub fn zoom_in(cur: f32) -> f32 {
    ZOOM_STEPS.iter().copied().find(|&s| s > cur + EPS).unwrap_or(MAX_ZOOM)
}

/// Next smaller preset zoom (clamped to [`MIN_ZOOM`]).
pub fn zoom_out(cur: f32) -> f32 {
    ZOOM_STEPS.iter().rev().copied().find(|&s| s < cur - EPS).unwrap_or(MIN_ZOOM)
}

pub fn clamp_zoom(z: f32) -> f32 {
    if z.is_finite() {
        z.clamp(MIN_ZOOM, MAX_ZOOM)
    } else {
        1.0
    }
}

/// Initial zoom: 100% unless the image does not fit in 80% of the work area.
/// All args physical px.
pub fn fit_zoom(img_w: u32, img_h: u32, work_w: u32, work_h: u32) -> f32 {
    if img_w == 0 || img_h == 0 || work_w == 0 || work_h == 0 {
        return 1.0;
    }
    let zx = work_w as f32 * 0.8 / img_w as f32;
    let zy = work_h as f32 * 0.8 / img_h as f32;
    clamp_zoom(zx.min(zy).min(1.0))
}

/// Window size in physical px for an image at `zoom`.
pub fn zoomed_size(img_w: u32, img_h: u32, zoom: f32) -> (f32, f32) {
    (img_w as f32 * zoom, img_h as f32 * zoom)
}

pub fn opacity_up(cur: u8) -> u8 {
    cur.saturating_add(OPACITY_STEP).clamp(MIN_OPACITY, MAX_OPACITY)
}

pub fn opacity_down(cur: u8) -> u8 {
    cur.saturating_sub(OPACITY_STEP).clamp(MIN_OPACITY, MAX_OPACITY)
}

/// Top-left for a window of `size` centred on `cursor`, kept inside `work`
/// (x, y, w, h). If larger than the work area, aligns to its top-left.
pub fn place_near(cursor: (f32, f32), size: (f32, f32), work: (f32, f32, f32, f32)) -> (f32, f32) {
    let axis = |c: f32, s: f32, lo: f32, len: f32| {
        let max = lo + len - s;
        if max < lo {
            lo
        } else {
            (c - s / 2.0).clamp(lo, max)
        }
    };
    (axis(cursor.0, size.0, work.0, work.2), axis(cursor.1, size.1, work.1, work.3))
}

/// New window top-left after resizing `old_size` -> `new_size` so the image
/// point under `cursor` (screen coords) stays under the cursor.
pub fn anchored_pos(pos: (f32, f32), old_size: (f32, f32), new_size: (f32, f32), cursor: (f32, f32)) -> (f32, f32) {
    let ratio = |new: f32, old: f32| if old > 0.0 { new / old } else { 1.0 };
    (
        cursor.0 - (cursor.0 - pos.0) * ratio(new_size.0, old_size.0),
        cursor.1 - (cursor.1 - pos.1) * ratio(new_size.1, old_size.1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_steps_walk() {
        assert_eq!(zoom_in(1.0), 1.25);
        assert_eq!(zoom_out(1.0), 0.75);
        assert_eq!(zoom_in(4.0), 4.0);
        assert_eq!(zoom_out(0.10), 0.10);
        assert_eq!(zoom_in(2.0), 3.0);
    }

    #[test]
    fn zoom_from_arbitrary_value() {
        assert_eq!(zoom_in(0.63), 0.75);
        assert_eq!(zoom_out(0.63), 0.50);
        assert_eq!(zoom_in(0.05), 0.10);
        assert_eq!(zoom_out(9.0), 4.0);
    }

    #[test]
    fn clamp_handles_nan() {
        assert_eq!(clamp_zoom(f32::NAN), 1.0);
        assert_eq!(clamp_zoom(100.0), MAX_ZOOM);
        assert_eq!(clamp_zoom(0.0), MIN_ZOOM);
    }

    #[test]
    fn fit_small_image_is_100() {
        assert_eq!(fit_zoom(400, 300, 1920, 1040), 1.0);
    }

    #[test]
    fn fit_large_image_scaled_to_80_percent() {
        let z = fit_zoom(3840, 2160, 1920, 1040);
        let (w, h) = zoomed_size(3840, 2160, z);
        assert!(w <= 1920.0 * 0.8 + 0.5 && h <= 1040.0 * 0.8 + 0.5);
        assert!((h - 832.0).abs() < 0.5, "height-limited, got {h}");
    }

    #[test]
    fn fit_degenerate() {
        assert_eq!(fit_zoom(0, 10, 100, 100), 1.0);
        assert_eq!(fit_zoom(10, 10, 0, 100), 1.0);
        assert_eq!(fit_zoom(100_000, 10, 100, 100), MIN_ZOOM);
    }

    #[test]
    fn opacity_steps_clamp() {
        assert_eq!(opacity_down(100), 90);
        assert_eq!(opacity_down(20), 20);
        assert_eq!(opacity_up(100), 100);
        assert_eq!(opacity_up(95), 100);
        assert_eq!(opacity_down(25), 20);
    }

    #[test]
    fn place_centres_on_cursor() {
        let p = place_near((500.0, 400.0), (200.0, 100.0), (0.0, 0.0, 1920.0, 1040.0));
        assert_eq!(p, (400.0, 350.0));
    }

    #[test]
    fn place_clamps_to_work_area() {
        let p = place_near((5.0, 1035.0), (200.0, 100.0), (0.0, 0.0, 1920.0, 1040.0));
        assert_eq!(p, (0.0, 940.0));
        let q = place_near((500.0, 500.0), (3000.0, 100.0), (10.0, 20.0, 1920.0, 1040.0));
        assert_eq!(q.0, 10.0);
    }

    #[test]
    fn anchor_keeps_point_under_cursor() {
        let pos = (100.0, 100.0);
        let old = (400.0, 200.0);
        let new = (800.0, 400.0);
        let cursor = (300.0, 150.0);
        let np = anchored_pos(pos, old, new, cursor);
        let fx0 = (cursor.0 - pos.0) / old.0;
        let fx1 = (cursor.0 - np.0) / new.0;
        let fy0 = (cursor.1 - pos.1) / old.1;
        let fy1 = (cursor.1 - np.1) / new.1;
        assert!((fx0 - fx1).abs() < 1e-5 && (fy0 - fy1).abs() < 1e-5);
    }

    #[test]
    fn anchor_at_top_left_corner_keeps_pos() {
        let np = anchored_pos((50.0, 60.0), (100.0, 100.0), (200.0, 200.0), (50.0, 60.0));
        assert_eq!(np, (50.0, 60.0));
    }

    #[test]
    fn anchor_zero_old_size_is_safe() {
        let np = anchored_pos((0.0, 0.0), (0.0, 0.0), (10.0, 10.0), (5.0, 5.0));
        assert_eq!(np, (0.0, 0.0));
    }
}
