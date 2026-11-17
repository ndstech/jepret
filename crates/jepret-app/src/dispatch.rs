//! Capture pipeline glue. Owner: **T12**.
//!
//! kind → capture (jepret-capture / overlay / scroll / recorder)
//!      → workflow::run → history.insert(saved_path) if saved_permanently
//!      → update state.last_region for Region/Scroll/RecordRegion.

use jepret_core::{Action, CaptureKind, Config};

#[allow(dead_code)] // used once T12 lands
pub struct Ctx<'a> {
    pub config: &'a Config,
}

pub fn handle(_ctx: &Ctx<'_>, _kind: CaptureKind, _actions: &[Action]) -> anyhow::Result<()> {
    anyhow::bail!("TODO(T12): dispatch::handle")
}
