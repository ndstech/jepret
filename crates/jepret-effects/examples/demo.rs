//! `cargo run --release -p jepret-effects --example demo -- input.png` -> out.png
use jepret_core::effects::{Anchor, TornEdges, Watermark};
use jepret_core::{EffectSpec, Image};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: demo <input.png>")?;
    let rgba = image::open(&path)?.to_rgba8();
    let img = Image::from_raw(rgba.width(), rgba.height(), rgba.into_raw())?;
    let spec = EffectSpec {
        corner_radius: 16,
        border_width: 3,
        border_color: "#3b82f6".into(),
        torn: TornEdges { bottom: true, ..Default::default() },
        torn_size: 10,
        shadow_blur: 24,
        shadow_offset_x: 0,
        shadow_offset_y: 8,
        watermark: Some(Watermark { text: "Jepret\ndemo".into(), position: Anchor::BottomRight, ..Default::default() }),
        ..Default::default()
    };
    let t = std::time::Instant::now();
    let out = jepret_effects::apply(&img, &spec);
    println!("apply: {:?} -> {}x{}", t.elapsed(), out.width, out.height);
    image::save_buffer("out.png", &out.pixels, out.width, out.height, image::ColorType::Rgba8)?;
    Ok(())
}
