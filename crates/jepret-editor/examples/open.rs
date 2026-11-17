//! `cargo run -p jepret-editor --example open -- <file.png>`

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "jepret_editor=debug".into()),
        )
        .init();
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: open <image file>"))?;
    jepret_editor::run_editor(std::path::Path::new(&path), &jepret_core::Config::default())
}
