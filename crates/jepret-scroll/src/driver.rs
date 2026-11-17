//! Scroll driver. Owner: **T08**.
//!
//! Loop: capture region (`jepret_capture::capture_rect`) → push to
//! [`Stitcher`](crate::Stitcher) → send mouse wheel to the window under the
//! region center (SendInput with cursor moved there, restore cursor after)
//! → wait `scroll_delay_ms` → repeat. Stop on 2× `NoChange`, `max_frames`,
//! or Esc pressed (GetAsyncKeyState polling between frames is fine here).

use jepret_core::{Image, Rect, Result};

use crate::{PushResult, Stitcher};

#[derive(Debug, Clone)]
pub struct ScrollOptions {
    pub max_frames: u32,
    pub scroll_delay_ms: u32,
    pub wheel_notches: u32,
    pub capture: jepret_capture::CaptureOptions,
    pub stitch: crate::StitchOptions,
}

/// Blocks until done. Returns the stitched image.
pub fn capture_scrolling(region: Rect, opts: &ScrollOptions) -> Result<Image> {
    anyhow::ensure!(region.width > 0 && region.height > 0, "scroll region is empty");
    let center = (
        region.x + (region.width / 2) as i32,
        region.y + (region.height / 2) as i32,
    );
    let saved = win::cursor_pos();
    // Restore the cursor on every exit path (success, error, panic).
    let _restore = RestoreGuard::new(move || {
        if let Some((x, y)) = saved {
            win::set_cursor_pos(x, y);
        }
    });

    let delay = std::time::Duration::from_millis(u64::from(opts.scroll_delay_ms));
    let notches = opts.wheel_notches.max(1);
    let image = run_scroll_loop(
        opts.max_frames,
        || jepret_capture::capture_rect(region, &opts.capture),
        || {
            win::set_cursor_pos(center.0, center.1);
            win::send_wheel_down(notches)?;
            std::thread::sleep(delay);
            Ok(())
        },
        win::escape_down,
        Stitcher::new(opts.stitch.clone()),
    )?;
    tracing::debug!(width = image.width, height = image.height, "scroll capture done");
    Ok(image)
}

/// Minimal surface of [`Stitcher`] used by the loop (lets tests use a fake).
pub(crate) trait FrameSink {
    fn push(&mut self, frame: &Image) -> PushResult;
    fn finish(self) -> Image;
}

impl FrameSink for Stitcher {
    fn push(&mut self, frame: &Image) -> PushResult {
        Stitcher::push(self, frame)
    }
    fn finish(self) -> Image {
        Stitcher::finish(self)
    }
}

/// Runs a closure on drop.
struct RestoreGuard<F: FnMut()>(F);

impl<F: FnMut()> RestoreGuard<F> {
    fn new(f: F) -> Self {
        Self(f)
    }
}

impl<F: FnMut()> Drop for RestoreGuard<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

/// Core loop, independent of Win32. `scroll` is expected to include the
/// post-scroll delay. Stops on 2 consecutive `NoChange`, `max_frames` captured
/// frames, or `cancelled()` (checked before every capture and scroll, so
/// cancel takes effect within one frame; the result so far is returned).
pub(crate) fn run_scroll_loop<S: FrameSink>(
    max_frames: u32,
    mut capture: impl FnMut() -> Result<Image>,
    mut scroll: impl FnMut() -> Result<()>,
    mut cancelled: impl FnMut() -> bool,
    mut stitcher: S,
) -> Result<Image> {
    let max_frames = max_frames.max(1);
    let mut frames = 0u32;
    let mut no_change = 0u32;
    while frames < max_frames {
        if cancelled() {
            break;
        }
        let frame = capture()?;
        frames += 1;
        match stitcher.push(&frame) {
            PushResult::NoChange => no_change += 1,
            PushResult::Appended { .. } | PushResult::NoMatch => no_change = 0,
        }
        if no_change >= 2 || frames >= max_frames || cancelled() {
            break;
        }
        scroll()?;
    }
    anyhow::ensure!(frames > 0, "scroll capture cancelled before the first frame");
    Ok(stitcher.finish())
}

mod win {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_WHEEL, MOUSEINPUT,
        VK_ESCAPE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};

    const WHEEL_DELTA: i32 = 120;

    pub fn cursor_pos() -> Option<(i32, i32)> {
        let mut p = POINT::default();
        // SAFETY: `p` is a valid, writable POINT for the duration of the call.
        unsafe { GetCursorPos(&mut p) }.ok().map(|_| (p.x, p.y))
    }

    pub fn set_cursor_pos(x: i32, y: i32) {
        // SAFETY: plain value arguments, no pointers.
        if let Err(e) = unsafe { SetCursorPos(x, y) } {
            tracing::warn!("SetCursorPos failed: {e}");
        }
    }

    /// Scroll down by `notches` wheel notches at the current cursor position.
    pub fn send_wheel_down(notches: u32) -> anyhow::Result<()> {
        let delta = -WHEEL_DELTA.saturating_mul(notches.min(1000) as i32);
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    // Wheel delta is a signed value stored in a DWORD.
                    mouseData: delta as u32,
                    dwFlags: MOUSEEVENTF_WHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        // SAFETY: `input` is a valid INPUT array of length 1 and the size matches.
        let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
        anyhow::ensure!(sent == 1, "SendInput injected {sent} of 1 events");
        Ok(())
    }

    /// True while Esc is physically held down.
    pub fn escape_down() -> bool {
        // SAFETY: plain value argument, no pointers.
        let state = unsafe { GetAsyncKeyState(i32::from(VK_ESCAPE.0)) };
        (state as u16) & 0x8000 != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn img(tag: u8) -> Image {
        Image { width: 1, height: 1, pixels: vec![tag, 0, 0, 255] }
    }

    struct FakeSink {
        results: Vec<PushResult>,
        pushed: Vec<u8>,
    }

    impl FakeSink {
        fn new(mut results: Vec<PushResult>) -> Self {
            results.reverse();
            Self { results, pushed: vec![] }
        }
    }

    impl FrameSink for FakeSink {
        fn push(&mut self, frame: &Image) -> PushResult {
            self.pushed.push(frame.pixels[0]);
            self.results.pop().unwrap_or(PushResult::Appended { rows: 1 })
        }
        // Encode number of pushed frames in the output width.
        fn finish(self) -> Image {
            Image { width: self.pushed.len() as u32, height: 1, pixels: vec![] }
        }
    }

    fn run(
        max: u32,
        results: Vec<PushResult>,
        cancel_after_polls: Option<u32>,
    ) -> (Result<Image>, u32, u32) {
        let captures = Cell::new(0u32);
        let scrolls = Cell::new(0u32);
        let polls = Cell::new(0u32);
        let r = run_scroll_loop(
            max,
            || {
                captures.set(captures.get() + 1);
                Ok(img(captures.get() as u8))
            },
            || {
                scrolls.set(scrolls.get() + 1);
                Ok(())
            },
            || {
                polls.set(polls.get() + 1);
                cancel_after_polls.is_some_and(|n| polls.get() > n)
            },
            FakeSink::new(results),
        );
        (r, captures.get(), scrolls.get())
    }

    #[test]
    fn stops_after_two_consecutive_no_change() {
        use PushResult::*;
        let (r, caps, scrolls) =
            run(50, vec![Appended { rows: 1 }, Appended { rows: 1 }, NoChange, NoChange], None);
        assert_eq!(r.unwrap().width, 4);
        assert_eq!(caps, 4);
        assert_eq!(scrolls, 3);
    }

    #[test]
    fn single_no_change_does_not_stop() {
        use PushResult::*;
        let (r, caps, _) =
            run(5, vec![Appended { rows: 1 }, NoChange, Appended { rows: 1 }, NoChange], None);
        assert_eq!(caps, 5);
        assert_eq!(r.unwrap().width, 5);
    }

    #[test]
    fn stops_at_max_frames() {
        let (r, caps, scrolls) = run(3, vec![], None);
        assert_eq!(r.unwrap().width, 3);
        assert_eq!(caps, 3);
        assert_eq!(scrolls, 2); // no useless scroll after the last frame
    }

    #[test]
    fn cancel_returns_result_so_far() {
        // polls: 1 (pre-capture f1), 2 (post-push f1), 3 (pre-capture f2) ok;
        // 4 (post-push f2) -> cancelled
        let (r, caps, _) = run(50, vec![], Some(3));
        assert_eq!(caps, 2);
        assert_eq!(r.unwrap().width, 2);
    }

    #[test]
    fn cancel_before_first_frame_is_error() {
        let (r, caps, _) = run(50, vec![], Some(0));
        assert!(r.is_err());
        assert_eq!(caps, 0);
    }

    #[test]
    fn capture_error_propagates() {
        let r = run_scroll_loop(
            5,
            || anyhow::bail!("boom"),
            || Ok(()),
            || false,
            FakeSink::new(vec![]),
        );
        assert!(r.is_err());
    }

    #[test]
    fn restore_guard_runs_on_success_and_error() {
        let restored = RefCell::new(0);
        {
            let _g = RestoreGuard::new(|| *restored.borrow_mut() += 1);
        }
        assert_eq!(*restored.borrow(), 1);
        let res: Result<()> = (|| {
            let _g = RestoreGuard::new(|| *restored.borrow_mut() += 1);
            anyhow::bail!("fail")
        })();
        assert!(res.is_err());
        assert_eq!(*restored.borrow(), 2);
    }
}
