//! Saves a PNG per monitor + the whole virtual desktop to a temp folder and
//! prints capture timings. `cargo run -p jepret-capture --example capture_all`
//! (use `--release` for representative timings; `-- --cursor` includes cursor).

use std::path::PathBuf;
use std::time::Instant;

use jepret_capture::{
    capture_foreground_window, capture_monitor, capture_virtual_screen, cursor_pos, monitors,
    visible_window_rects, CaptureOptions,
};
use jepret_core::Image;

fn save(img: &Image, path: &PathBuf) -> anyhow::Result<()> {
    image::save_buffer(path, &img.pixels, img.width, img.height, image::ColorType::Rgba8)?;
    println!("  saved {} ({}x{})", path.display(), img.width, img.height);
    Ok(())
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("jepret_capture=debug").init();
    let opts = CaptureOptions {
        show_cursor: std::env::args().any(|a| a == "--cursor"),
        hdr_tonemap: true,
        sdr_white_nits: None,
    };
    let dir = std::env::temp_dir().join("jepret-capture-example");
    std::fs::create_dir_all(&dir)?;

    let mons = monitors()?;
    println!("{} monitor(s), cursor at {:?}", mons.len(), cursor_pos()?);
    for m in &mons {
        println!(
            "  {} \"{}\" {:?} scale {} primary {} hdr {} sdr_white {} nits",
            m.id.0, m.name, m.rect, m.scale, m.primary, m.hdr, m.sdr_white_nits
        );
    }

    // warm-up
    let _ = capture_virtual_screen(&opts)?;

    for (i, m) in mons.iter().enumerate() {
        let t = Instant::now();
        let (img, rect) = capture_monitor(&m.id, &opts)?;
        println!("monitor {} {:?}: {:.1} ms", m.id.0, rect, t.elapsed().as_secs_f64() * 1000.0);
        save(&img, &dir.join(format!("monitor{i}.png")))?;
    }

    let mut times = Vec::new();
    let mut last = None;
    for _ in 0..5 {
        let t = Instant::now();
        last = Some(capture_virtual_screen(&opts)?);
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    println!(
        "virtual screen x5: {}",
        times.iter().map(|t| format!("{t:.1}")).collect::<Vec<_>>().join(" / ")
    );
    if let Some((img, rect)) = last {
        println!("virtual rect {rect:?}");
        save(&img, &dir.join("virtual.png"))?;
    }

    let t = Instant::now();
    match capture_foreground_window(&opts) {
        Ok((img, rect)) => {
            println!("foreground window {rect:?}: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
            save(&img, &dir.join("foreground.png"))?;
        }
        Err(e) => println!("foreground window: {e:#}"),
    }

    let t = Instant::now();
    let wins = visible_window_rects()?;
    println!("{} visible windows in {:.1} ms; top 5:", wins.len(), t.elapsed().as_secs_f64() * 1000.0);
    for r in wins.iter().take(5) {
        println!("  {r:?}");
    }
    Ok(())
}
