//! Entry point. Owner: **T12**. See PRD F11.
//!
//! Modes:
//!   jepret                       → daemon (tray + hotkeys)
//!   jepret capture <kind>        → ask running daemon to capture (or run once)
//!   jepret --editor <file>       → editor process
//!   jepret --gallery             → gallery process
//!   jepret --settings            → settings process

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod daemon;
mod dispatch;

use std::path::PathBuf;

use jepret_core::{paths, Config};

fn main() -> anyhow::Result<()> {
    // TODO(T12): SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2) first,
    // then logging (tracing-appender to paths::logs_dir()).
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
