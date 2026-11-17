//! Entry point. Owner: **T12**. See PRD F11.
//!
//! Modes:
//!   jepret                       → daemon (tray + hotkeys)
//!   jepret capture <kind>        → ask running daemon to capture (or run once)
//!   jepret --editor <file>       → editor process
//!   jepret --gallery             → gallery process
//!   jepret --settings            → settings process

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod command;
mod daemon;
mod dispatch;
mod hotkey;
mod raster;

use std::path::PathBuf;

use jepret_core::{paths, Config};

fn main() -> anyhow::Result<()> {
    // The embedded manifest (build.rs) already declares PerMonitorV2; this call
    // covers `cargo run` setups where the manifest is missing. Failure = already set.
    // SAFETY: plain Win32 call, no pointers.
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }
    init_logging();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config_path = paths::config_file();

    match args.first().map(String::as_str) {
        Some("--editor") => {
            let file = PathBuf::from(args.get(1).ok_or_else(|| anyhow::anyhow!("--editor <file>"))?);
            let config = Config::load(&config_path)?;
            jepret_editor::run_editor(&file, &config)
        }
        Some("--gallery") => {
            let config = Config::load(&config_path)?;
            jepret_gallery::run_gallery(&config)
        }
        Some("--settings") => jepret_settings::run_settings(&config_path),
        Some("capture") => {
            let kind = args.get(1).ok_or_else(|| anyhow::anyhow!("capture <kind>"))?;
            daemon::send_or_run_capture(kind)
        }
        None => daemon::run(),
        Some(other) => anyhow::bail!("unknown argument: {other}"),
    }
}

/// Daily-rolling log file in `paths::logs_dir()` (last 7 files). Failure to
/// set up logging is never fatal.
fn init_logging() {
    let dir = paths::logs_dir();
    if paths::ensure_dir(&dir).is_err() {
        return;
    }
    let Ok(appender) = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("jepret")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&dir)
    else {
        return;
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("JEPRET_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(appender))
        .with_ansi(false)
        .with_env_filter(filter)
        .try_init();
    std::panic::set_hook(Box::new(|info| tracing::error!("panic: {info}")));
}
