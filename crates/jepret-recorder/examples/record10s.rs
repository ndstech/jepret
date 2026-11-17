//! Record the primary monitor (or a region) for N seconds.
//!
//! `cargo run -p jepret-recorder --example record10s --release -- [seconds] [out.mp4] [x y w h] [--mic] [--no-sys]`

use std::time::Duration;

use jepret_core::Rect;
use jepret_recorder::{list_microphones, RecordOptions, Recorder};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,jepret_recorder=debug")
        .init();
    #[cfg(windows)]
    // SAFETY: plain process-wide DPI opt-in so coordinates are physical px.
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SetProcessDPIAware();
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mic = args.iter().any(|a| a == "--mic");
    let sys = !args.iter().any(|a| a == "--no-sys");
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let secs: u64 = pos.first().and_then(|s| s.parse().ok()).unwrap_or(10);
    let out = pos
        .get(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("jepret_record10s.mp4"));
    let region = if pos.len() >= 6 {
        let n: Vec<i32> = pos[2..6].iter().map(|s| s.parse().unwrap_or(0)).collect();
        Rect::new(n[0], n[1], n[2] as u32, n[3] as u32)
    } else {
        Rect::new(0, 0, 1920, 1080)
    };
    println!("microphones: {:?}", list_microphones()?);
    let rec = Recorder::start(RecordOptions {
        output: out.clone(),
        region,
        fps: 30,
        bitrate_kbps: 8000,
        system_audio: sys,
        mic_device: mic.then(String::new),
        show_cursor: true,
    })?;
    println!("recording {secs}s -> {}", out.display());
    std::thread::sleep(Duration::from_secs(secs));
    println!("elapsed {:?}", rec.elapsed());
    let path = rec.stop()?;
    println!(
        "done: {} ({} bytes)",
        path.display(),
        std::fs::metadata(&path)?.len()
    );
    Ok(())
}
