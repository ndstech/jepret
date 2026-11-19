//! Usage: cargo run -p jepret-ocr --example ocr -- <image.png>

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("pakai: ocr <gambar.png>"))?;
    let rgba = image::open(&path)?.to_rgba8();
    let (w, h) = rgba.dimensions();
    let img = jepret_core::Image::from_raw(w, h, rgba.into_raw())?;
    let t = std::time::Instant::now();
    let text = jepret_ocr::recognize(&img)?;
    eprintln!("({w}x{h}, {:?})", t.elapsed());
    println!("{text}");
    Ok(())
}
