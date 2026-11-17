//! Shared contracts for every Jepret crate.
//!
//! **This crate is the API contract.** Other crates are developed in parallel
//! against these types; changing a public item here needs a note in
//! `TASKS.md` ("Contract changes") so other workers can follow.

pub mod action;
pub mod capture;
pub mod config;
pub mod geom;
pub mod image;
pub mod monitor;
pub mod paths;
pub mod state;
pub mod template;

pub use action::{Action, CommandSpec};
pub use capture::{CaptureKind, CaptureResult};
pub use config::Config;
pub use geom::{Point, Rect};
pub use image::Image;
pub use monitor::{MonitorId, MonitorInfo};
pub use state::AppState;

pub type Result<T> = anyhow::Result<T>;

pub const APP_NAME: &str = "Jepret";
