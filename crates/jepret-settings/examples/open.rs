//! Opens the settings window on a config in a temp dir.
//! `cargo run -p jepret-settings --example open`

fn main() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join("jepret-settings-demo");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("config.toml");
    println!("config: {}", path.display());
    jepret_settings::run_settings(&path)?;
    println!("exists after close: {}", path.exists());
    Ok(())
}
