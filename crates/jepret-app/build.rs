//! Embed the application manifest (Per-Monitor-V2 DPI awareness) and the app
//! icon into jepret.exe. Uses the MSVC linker (`/MANIFESTINPUT` + a generated
//! `.res`) so no resource-compiler crate is needed. The icon is rasterised
//! deterministically from the SVG path by `src/raster.rs`.

#[path = "src/raster.rs"]
mod raster;

fn main() {
    println!("cargo:rerun-if-changed=jepret.manifest");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/raster.rs");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{dir}/jepret.manifest");

        if let Ok(out) = std::env::var("OUT_DIR") {
            let images = raster::icon_images();
            let res = std::path::Path::new(&out).join("jepret-icon.res");
            let ico = std::path::Path::new(&out).join("jepret.ico");
            if std::fs::write(&res, raster::res_file(&images)).is_ok() {
                let _ = std::fs::write(&ico, raster::ico_file(&images));
                println!("cargo:rustc-link-arg-bins={}", res.display());
            }
        }
    }
}
