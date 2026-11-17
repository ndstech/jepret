//! Pure helpers: region fitting and audio mixing (unit-tested, no Win32).

use jepret_core::Rect;

/// Clamp `region` to `bounds` (the monitor) and round width/height down to
/// even numbers (H.264/NV12 requirement). `None` if nothing is left.
pub fn fit_region(region: Rect, bounds: Rect) -> Option<Rect> {
    let r = region.intersect(&bounds)?;
    let w = r.width & !1;
    let h = r.height & !1;
    if w < 2 || h < 2 {
        return None;
    }
    Some(Rect::new(r.x, r.y, w, h))
}

/// Index of the monitor whose bounds overlap `region` the most.
pub fn best_monitor(region: Rect, monitors: &[Rect]) -> Option<usize> {
    monitors
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            region
                .intersect(m)
                .map(|r| (i, r.width as u64 * r.height as u64))
        })
        .filter(|&(_, a)| a > 0)
        .max_by_key(|&(_, a)| a)
        .map(|(i, _)| i)
}

/// Sum all sources sample-wise, clamp to [-1, 1] and convert to i16.
/// Shorter sources are treated as zero-padded to `len`.
pub fn mix_to_i16(sources: &[Vec<f32>], len: usize) -> Vec<i16> {
    (0..len)
        .map(|i| {
            let s: f32 = sources
                .iter()
                .map(|v| v.get(i).copied().unwrap_or(0.0))
                .sum();
            (s.clamp(-1.0, 1.0) * 32767.0).round() as i16
        })
        .collect()
}

/// Next frame index to emit given elapsed time (100 ns units); never goes
/// backwards or repeats `last`.
pub fn next_frame_index(elapsed_100ns: i64, frame_100ns: i64, last: Option<i64>) -> i64 {
    let idx = (elapsed_100ns + frame_100ns / 2) / frame_100ns;
    match last {
        Some(l) if idx <= l => l + 1,
        _ => idx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_is_clamped_and_even() {
        let mon = Rect::new(0, 0, 1920, 1080);
        let r = fit_region(Rect::new(10, 10, 101, 51), mon).unwrap();
        assert_eq!((r.width, r.height), (100, 50));
        let r = fit_region(Rect::new(1900, 1000, 500, 500), mon).unwrap();
        assert_eq!((r.x, r.y, r.width, r.height), (1900, 1000, 20, 80));
        assert!(fit_region(Rect::new(3000, 0, 10, 10), mon).is_none());
        assert!(fit_region(Rect::new(0, 0, 1, 1), mon).is_none());
    }

    #[test]
    fn region_negative_coords() {
        let mon = Rect::new(-1920, 0, 1920, 1080);
        let r = fit_region(Rect::new(-100, 5, 200, 100), mon).unwrap();
        assert_eq!((r.x, r.width), (-100, 100));
    }

    #[test]
    fn picks_monitor_with_most_overlap() {
        let mons = [Rect::new(0, 0, 1920, 1080), Rect::new(1920, 0, 1920, 1080)];
        assert_eq!(best_monitor(Rect::new(1800, 0, 400, 100), &mons), Some(1));
        assert_eq!(best_monitor(Rect::new(0, 0, 10, 10), &mons), Some(0));
        assert_eq!(best_monitor(Rect::new(-500, 0, 10, 10), &mons), None);
    }

    #[test]
    fn mix_sums_clamps_pads() {
        let a = vec![0.5, 0.5, 0.5];
        let b = vec![0.75, -1.0];
        let out = mix_to_i16(&[a, b], 4);
        assert_eq!(out[0], 32767);
        assert_eq!(out[1], (-0.5f32 * 32767.0).round() as i16);
        assert_eq!(out[2], (0.5f32 * 32767.0).round() as i16);
        assert_eq!(out[3], 0);
        assert!(mix_to_i16(&[], 2).iter().all(|&s| s == 0));
    }

    #[test]
    fn frame_index_monotonic() {
        let f = 333_333;
        assert_eq!(next_frame_index(0, f, None), 0);
        assert_eq!(next_frame_index(f * 3 + 10, f, Some(2)), 3);
        assert_eq!(next_frame_index(f * 3, f, Some(3)), 4);
    }
}
