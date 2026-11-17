//! Manual demo / smoke test for the overlay, using a synthetic gradient image.
//!
//! ```powershell
//! cargo run -p jepret-overlay --example demo
//! cargo run -p jepret-overlay --example demo -- --selftest   # scripted, no manual input
//! ```
//!
//! Flags: `--no-magnifier`, `--initial` (preselect a rect), `--border` is
//! always shown for 2 s after a successful selection.

use std::time::{Duration, Instant};

use jepret_core::{Image, MonitorId, MonitorInfo, Rect};
use jepret_overlay::{select_region, OverlayOptions, RegionBorder};
use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW};
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT,
};
use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;

fn to_rect(r: RECT) -> Rect {
    Rect::new(r.left, r.top, (r.right - r.left) as u32, (r.bottom - r.top) as u32)
}

unsafe extern "system" fn enum_cb(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let out = &mut *(data.0 as *mut Vec<MonitorInfo>);
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(hmon, &mut mi as *mut MONITORINFOEXW as *mut _).as_bool() {
        let len = mi.szDevice.iter().position(|&c| c == 0).unwrap_or(mi.szDevice.len());
        let name = String::from_utf16_lossy(&mi.szDevice[..len]);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        out.push(MonitorInfo {
            id: MonitorId(name.clone()),
            name,
            rect: to_rect(mi.monitorInfo.rcMonitor),
            work_area: to_rect(mi.monitorInfo.rcWork),
            scale: dx as f32 / 96.0,
            primary: mi.monitorInfo.dwFlags & 1 != 0,
            hdr: false,
            sdr_white_nits: 80.0,
        });
    }
    BOOL(1)
}

fn monitors() -> Vec<MonitorInfo> {
    let mut v: Vec<MonitorInfo> = Vec::new();
    // SAFETY: callback only touches the Vec behind `data` for the call's duration.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(enum_cb), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

/// Gradient + 100 px grid + 1 px red corner markers, so selection accuracy
/// and the magnifier are easy to eyeball. Each monitor gets a distinct tint.
fn synthetic(origin: Rect, mons: &[MonitorInfo]) -> Image {
    let (w, h) = (origin.width, origin.height);
    let mut px = vec![255u8; w as usize * h as usize * 4];
    for y in 0..h {
        for x in 0..w {
            let (gx, gy) = (origin.x + x as i32, origin.y + y as i32);
            let mi = mons.iter().position(|m| m.rect.contains(jepret_core::Point::new(gx, gy))).unwrap_or(0);
            let tint = [[1.0, 0.7, 0.7], [0.7, 1.0, 0.7], [0.7, 0.7, 1.0]][mi % 3];
            let base = [
                (x as f32 / w as f32 * 200.0) as u8,
                (y as f32 / h as f32 * 200.0) as u8,
                (255.0 - x as f32 / w as f32 * 150.0) as u8,
            ];
            let mut c = [
                (base[0] as f32 * tint[0]) as u8,
                (base[1] as f32 * tint[1]) as u8,
                (base[2] as f32 * tint[2]) as u8,
            ];
            if gx.rem_euclid(100) == 0 || gy.rem_euclid(100) == 0 {
                c = [255, 255, 255];
            } else if gx.rem_euclid(10) == 0 || gy.rem_euclid(10) == 0 {
                c = [c[0] / 2 + 60, c[1] / 2 + 60, c[2] / 2 + 60];
            }
            let i = (y as usize * w as usize + x as usize) * 4;
            px[i..i + 3].copy_from_slice(&c);
        }
    }
    Image { width: w, height: h, pixels: px }
}

fn send_mouse(flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    };
    // SAFETY: one valid INPUT.
    unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
}

/// Drives a real drag with synthetic input and returns the expected rect.
fn spawn_selftest(primary: Rect) -> (std::thread::JoinHandle<()>, Rect) {
    let (x0, y0, x1, y1) = (primary.x + 200, primary.y + 150, primary.x + 499, primary.y + 349);
    let expected = Rect::new(x0, y0, 300, 200);
    let h = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1200));
        // SAFETY: plain Win32 calls.
        unsafe {
            let _ = SetCursorPos(x0, y0);
            std::thread::sleep(Duration::from_millis(150));
            send_mouse(MOUSEEVENTF_LEFTDOWN);
            for i in 1..=10 {
                let _ = SetCursorPos(x0 + (x1 - x0) * i / 10, y0 + (y1 - y0) * i / 10);
                std::thread::sleep(Duration::from_millis(30));
            }
            // releasing the button after a drag confirms the selection.
            send_mouse(MOUSEEVENTF_LEFTUP);
        }
    });
    (h, expected)
}

fn main() -> anyhow::Result<()> {
    // Physical-pixel coordinates (overlay also forces this on its own thread).
    // SAFETY: plain Win32 call.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let args: Vec<String> = std::env::args().collect();
    let selftest = args.iter().any(|a| a == "--selftest");

    let mons = monitors();
    anyhow::ensure!(!mons.is_empty(), "no monitors found");
    let origin = mons.iter().skip(1).fold(mons[0].rect, |a, m| a.union(&m.rect));
    println!("virtual desktop: {origin:?}");
    for m in &mons {
        println!("  {} {:?} scale {}", m.name, m.rect, m.scale);
    }

    let t = Instant::now();
    let img = synthetic(origin, &mons);
    println!("synthetic image: {} ms", t.elapsed().as_millis());

    // A few fake "windows" (topmost first) for hover-snap, relative to each monitor.
    let mut window_rects = Vec::new();
    for m in &mons {
        let r = m.rect;
        window_rects.push(Rect::new(r.x + 100, r.y + 100, r.width / 3, r.height / 3));
        window_rects.push(Rect::new(r.x + 40, r.y + 40, r.width / 2, r.height / 2));
    }

    let primary = mons.iter().find(|m| m.primary).unwrap_or(&mons[0]).rect;
    let opts = OverlayOptions {
        initial: args.iter().any(|a| a == "--initial").then(|| Rect::new(primary.x + 300, primary.y + 300, 400, 250)),
        magnifier: !args.iter().any(|a| a == "--no-magnifier"),
        window_rects: if selftest { vec![] } else { window_rects },
        hint: Some("Jepret demo: drag / click window / arrows / Enter / Esc".into()),
    };

    let st = selftest.then(|| spawn_selftest(primary));
    let t = Instant::now();
    let res = select_region(&img, origin, &mons, &opts)?;
    println!("select_region returned after {} ms: {res:?}", t.elapsed().as_millis());

    if let Some((h, expected)) = st {
        let _ = h.join();
        anyhow::ensure!(res == Some(expected), "selftest: expected {expected:?}, got {res:?}");
        println!("selftest OK: selection is pixel-exact");
    }
    if let Some(r) = res {
        println!("showing RegionBorder around {r:?} for 2 s");
        let _b = RegionBorder::show(r)?;
        std::thread::sleep(Duration::from_secs(2));
    }
    Ok(())
}
