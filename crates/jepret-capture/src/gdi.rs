//! GDI helpers: SDR fallback grab (when Desktop Duplication is unavailable)
//! and software cursor compositing.

use jepret_core::{Image, Point, Rect, Result};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::pixels::bgra_to_rgba;

/// Top-down 32bpp DIB section selected into a memory DC.
struct Dib {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    width: u32,
    height: u32,
}

impl Dib {
    fn new(width: u32, height: u32) -> Result<Dib> {
        // SAFETY: standard GDI object creation; failure paths release what was made.
        unsafe {
            let dc = CreateCompatibleDC(None);
            anyhow::ensure!(!dc.is_invalid(), "CreateCompatibleDC failed");
            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = width as i32;
            bmi.bmiHeader.biHeight = -(height as i32); // top-down
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB.0;
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let bmp = match CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0) {
                Ok(b) if !bits.is_null() => b,
                Ok(b) => {
                    let _ = DeleteObject(b);
                    let _ = DeleteDC(dc);
                    anyhow::bail!("CreateDIBSection returned no bits");
                }
                Err(e) => {
                    let _ = DeleteDC(dc);
                    return Err(e.into());
                }
            };
            let old = SelectObject(dc, bmp);
            Ok(Dib { dc, bmp, old, bits: bits as *mut u8, width, height })
        }
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: DIB section memory is width*height*4 bytes and lives as long as `self`.
        unsafe { std::slice::from_raw_parts(self.bits, self.width as usize * self.height as usize * 4) }
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: as above, with exclusive access through &mut self.
        unsafe { std::slice::from_raw_parts_mut(self.bits, self.width as usize * self.height as usize * 4) }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        // SAFETY: objects were created in `new`; deselect before deleting.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp);
            let _ = DeleteDC(self.dc);
        }
    }
}

/// BitBlt a virtual-desktop rect from the screen DC (SDR only).
pub(crate) fn grab_rect(rect: Rect) -> Result<Image> {
    anyhow::ensure!(!rect.is_empty(), "empty rect");
    let dib = Dib::new(rect.width, rect.height)?;
    // SAFETY: screen DC acquired and released here; dib.dc is valid.
    unsafe {
        let screen = GetDC(None);
        anyhow::ensure!(!screen.is_invalid(), "GetDC(NULL) failed");
        let r = BitBlt(
            dib.dc,
            0,
            0,
            rect.width as i32,
            rect.height as i32,
            screen,
            rect.x,
            rect.y,
            SRCCOPY | CAPTUREBLT,
        );
        ReleaseDC(None, screen);
        r?;
    }
    Ok(bgra_to_rgba(dib.bytes(), rect.width, rect.height, rect.width as usize * 4))
}

/// Draw the current system cursor onto `img`, whose (0,0) is `origin` in
/// virtual-desktop coordinates. Best effort; silently does nothing if the
/// cursor is hidden or unavailable.
pub(crate) fn draw_cursor(img: &mut Image, origin: Point) {
    const BOX: i32 = 128;
    // SAFETY: GDI calls on locally owned handles; icon bitmaps from
    // GetIconInfo are deleted; DIB memory is only touched via safe slices.
    unsafe {
        let mut ci = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
        if GetCursorInfo(&mut ci).is_err() || ci.flags.0 & CURSOR_SHOWING.0 == 0 {
            return;
        }
        let mut ii = ICONINFO::default();
        if GetIconInfo(HICON(ci.hCursor.0), &mut ii).is_err() {
            return;
        }
        if !ii.hbmMask.is_invalid() {
            let _ = DeleteObject(ii.hbmMask);
        }
        if !ii.hbmColor.is_invalid() {
            let _ = DeleteObject(ii.hbmColor);
        }
        let cx = ci.ptScreenPos.x - ii.xHotspot as i32 - origin.x;
        let cy = ci.ptScreenPos.y - ii.yHotspot as i32 - origin.y;
        let bounds = Rect::new(0, 0, img.width, img.height);
        let Some(r) = bounds.intersect(&Rect::new(cx, cy, BOX as u32, BOX as u32)) else { return };

        let Ok(mut dib) = Dib::new(BOX as u32, BOX as u32) else { return };
        // Fill the box with the underlying pixels (RGBA -> BGRA), black outside.
        {
            let dst = dib.bytes_mut();
            dst.fill(0);
            for y in 0..r.height as usize {
                for x in 0..r.width as usize {
                    let s = (r.y as usize + y) * img.stride() + (r.x as usize + x) * 4;
                    let bx = (r.x - cx) as usize + x;
                    let by = (r.y - cy) as usize + y;
                    let d = (by * BOX as usize + bx) * 4;
                    dst[d] = img.pixels[s + 2];
                    dst[d + 1] = img.pixels[s + 1];
                    dst[d + 2] = img.pixels[s];
                    dst[d + 3] = 255;
                }
            }
        }
        if DrawIconEx(dib.dc, 0, 0, HICON(ci.hCursor.0), 0, 0, 0, HBRUSH::default(), DI_NORMAL).is_err() {
            return;
        }
        let src = dib.bytes();
        for y in 0..r.height as usize {
            for x in 0..r.width as usize {
                let d = (r.y as usize + y) * img.stride() + (r.x as usize + x) * 4;
                let bx = (r.x - cx) as usize + x;
                let by = (r.y - cy) as usize + y;
                let s = (by * BOX as usize + bx) * 4;
                img.pixels[d] = src[s + 2];
                img.pixels[d + 1] = src[s + 1];
                img.pixels[d + 2] = src[s];
                img.pixels[d + 3] = 255;
            }
        }
    }
}
