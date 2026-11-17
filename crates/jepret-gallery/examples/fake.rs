//! Opens the gallery with 10,000 fake entries: `cargo run -p jepret-gallery --example fake`.

use std::sync::Arc;

use jepret_core::Config;
use jepret_gallery::FakeSource;

fn main() -> anyhow::Result<()> {
    jepret_gallery::run_gallery_with(Arc::new(FakeSource::new(10_000)), &Config::default())
}
