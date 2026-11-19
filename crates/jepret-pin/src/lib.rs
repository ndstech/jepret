//! `jepret --pin <file>`: show an image in a borderless always-on-top
//! window.

use std::path::Path;

use jepret_core::Result;

/// Run the pin window for `file` until closed (blocks).
pub fn run_pin(file: &Path, config_path: &Path) -> Result<()> {
    // TODO(pin): implement
    let _ = (file, config_path);
    anyhow::bail!("pin belum tersedia")
}
