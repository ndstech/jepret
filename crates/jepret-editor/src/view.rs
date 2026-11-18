//! Viewport math for the editor canvas (pure, no egui): pan limits,
//! scrollbar geometry and navigator mapping. Everything works per axis in
//! screen points; `pan` is the offset of the image's top-left corner from
//! the viewport's top-left corner.

/// Free space kept around the image when it is larger than the viewport,
/// so its edge (and the drop shadow) stays reachable.
pub const MARGIN: f32 = 24.0;
/// Shortest scrollbar thumb, in points.
pub const MIN_THUMB: f32 = 24.0;

/// True when `content` (scaled image length) does not fit in `view`.
pub fn scrollable(view: f32, content: f32) -> bool {
    content > view + 0.5
}

/// Clamp `pan` on one axis: an image that fits is centred; a larger one
/// may scroll until its edge is `MARGIN` inside the viewport.
pub fn clamp_pan(pan: f32, view: f32, content: f32) -> f32 {
    if !scrollable(view, content) {
        return ((view - content) / 2.0).round();
    }
    pan.clamp(view - content - MARGIN, MARGIN)
}

/// Scrollbar thumb `(start, len)` inside a track of length `track`.
/// Only meaningful when [`scrollable`].
pub fn thumb(track: f32, view: f32, content: f32, pan: f32) -> (f32, f32) {
    let total = content + 2.0 * MARGIN;
    let len = (track * view / total).clamp(MIN_THUMB.min(track), track);
    let range = (total - view).max(1.0);
    let offset = (MARGIN - pan).clamp(0.0, range);
    ((track - len) * offset / range, len)
}

/// Inverse of [`thumb`]: the pan that puts the thumb at `start`.
pub fn pan_for_thumb(track: f32, view: f32, content: f32, start: f32) -> f32 {
    let total = content + 2.0 * MARGIN;
    let len = (track * view / total).clamp(MIN_THUMB.min(track), track);
    let range = (total - view).max(1.0);
    let free = (track - len).max(1.0);
    let offset = start.clamp(0.0, free) / free * range;
    MARGIN - offset
}

/// Pan that centres image coordinate `p` (image pixels) in the viewport.
pub fn pan_centering(p: f32, view: f32, zoom: f32) -> f32 {
    view / 2.0 - p * zoom
}

/// Size of the navigator thumbnail for an `iw`×`ih` image, fitting in
/// `max_w`×`max_h` and keeping the aspect ratio.
pub fn navigator_size(iw: f32, ih: f32, max_w: f32, max_h: f32) -> (f32, f32) {
    if iw <= 0.0 || ih <= 0.0 {
        return (0.0, 0.0);
    }
    let s = (max_w / iw).min(max_h / ih);
    ((iw * s).round().max(1.0), (ih * s).round().max(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_image_is_centred() {
        assert!(!scrollable(800.0, 400.0));
        assert_eq!(clamp_pan(-500.0, 800.0, 400.0), 200.0);
        assert_eq!(clamp_pan(900.0, 800.0, 400.0), 200.0);
    }

    #[test]
    fn large_image_pan_is_bounded_by_margin() {
        assert!(scrollable(800.0, 2000.0));
        assert_eq!(clamp_pan(100.0, 800.0, 2000.0), MARGIN);
        assert_eq!(clamp_pan(-5000.0, 800.0, 2000.0), 800.0 - 2000.0 - MARGIN);
        assert_eq!(clamp_pan(-300.0, 800.0, 2000.0), -300.0);
    }

    #[test]
    fn clamp_is_continuous_at_the_threshold() {
        let view = 800.0;
        // At the threshold the centred pan (0) lies inside the scroll range.
        assert_eq!(clamp_pan(0.0, view, view), 0.0);
        assert_eq!(clamp_pan(0.0, view, view + 1.0), 0.0);
    }

    #[test]
    fn thumb_spans_track_proportionally() {
        let (track, view, content) = (800.0, 800.0, 2000.0 - 2.0 * MARGIN);
        // total = 2000 -> thumb is 40% of the track.
        let (s, l) = thumb(track, view, content, MARGIN);
        assert_eq!(s, 0.0);
        assert!((l - 320.0).abs() < 0.01);
        let end = view - content - MARGIN;
        let (s, l) = thumb(track, view, content, end);
        assert!((s + l - track).abs() < 0.01);
    }

    #[test]
    fn thumb_round_trips() {
        let (track, view, content) = (600.0, 700.0, 3000.0);
        for pan in [MARGIN, -100.0, -1234.5, view - content - MARGIN] {
            let (s, _) = thumb(track, view, content, pan);
            assert!((pan_for_thumb(track, view, content, s) - pan).abs() < 0.01, "pan {pan}");
        }
    }

    #[test]
    fn thumb_has_minimum_length() {
        let (_, l) = thumb(300.0, 300.0, 1_000_000.0, 0.0);
        assert_eq!(l, MIN_THUMB);
    }

    #[test]
    fn centering_puts_point_mid_view() {
        let pan = pan_centering(500.0, 800.0, 2.0);
        assert_eq!(pan + 500.0 * 2.0, 400.0);
    }

    #[test]
    fn navigator_keeps_aspect() {
        assert_eq!(navigator_size(4000.0, 1000.0, 200.0, 140.0), (200.0, 50.0));
        assert_eq!(navigator_size(1000.0, 2000.0, 200.0, 140.0), (70.0, 140.0));
        assert_eq!(navigator_size(0.0, 10.0, 200.0, 140.0), (0.0, 0.0));
    }
}
