//! Embed the application manifest (Per-Monitor-V2 DPI awareness) into jepret.exe.
//! Uses the MSVC linker's /MANIFESTINPUT so no resource-compiler crate is needed.

fn main() {
    println!("cargo:rerun-if-changed=jepret.manifest");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{dir}/jepret.manifest");
    }
}
