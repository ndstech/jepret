//! Scrolling capture. Owner: **T07** (`stitch.rs`, pure) and **T08** (`driver.rs`, Win32).

pub mod driver;
pub mod stitch;

pub use driver::{capture_scrolling, ScrollOptions};
pub use stitch::{PushResult, StitchOptions, Stitcher};
