//! `cargo run -p jepret-pin --example pin -- <file.png>`
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("jepret_pin=debug").init();
    let file: PathBuf = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("pemakaian: pin <file.png>"))?;
    let cfg = std::env::temp_dir().join("jepret-pin-example-config.toml");
    jepret_pin::run_pin(&file, &cfg)
}
