//! Small runtime state persisted between runs (`state.toml`).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{geom::Rect, monitor::MonitorId};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppState {
    pub last_region: Option<LastRegion>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LastRegion {
    pub rect: Rect,
    pub monitor: Option<MonitorId>,
}

impl AppState {
    /// Missing or corrupt file → default state (never fails the app).
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> crate::Result<()> {
        if let Some(dir) = path.parent() {
            crate::paths::ensure_dir(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}
