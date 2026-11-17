use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{geom::Rect, image::Image, monitor::MonitorId};

/// What a hotkey captures. Serialized as snake_case strings in config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureKind {
    /// Whole virtual desktop (all monitors).
    Fullscreen,
    /// Monitor under the cursor.
    ActiveMonitor,
    /// Foreground window.
    ActiveWindow,
    /// Interactive region selection (overlay).
    Region,
    /// Re-capture the last selected region without overlay.
    LastRegion,
    /// Scrolling capture of a selected region.
    Scroll,
    /// Toggle recording of a selected region.
    RecordRegion,
    /// Toggle recording of the monitor under the cursor.
    RecordMonitor,
}

impl CaptureKind {
    pub fn is_recording(self) -> bool {
        matches!(self, Self::RecordRegion | Self::RecordMonitor)
    }
}

/// Result of an image capture, handed to the workflow runner.
#[derive(Debug, Clone)]
pub struct CaptureResult {
    pub image: Image,
    pub kind: CaptureKind,
    /// Captured area in virtual-desktop physical px.
    pub region: Rect,
    /// Monitor the region (mostly) lies on, if any.
    pub monitor: Option<MonitorId>,
    pub captured_at: SystemTime,
}
