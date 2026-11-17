use serde::{Deserialize, Serialize};

use crate::geom::Rect;

/// Stable-ish monitor identifier: the GDI device name (e.g. `\\.\DISPLAY1`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MonitorId(pub String);

#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub id: MonitorId,
    /// Friendly name if available (e.g. "DELL U2720Q"), else the device name.
    pub name: String,
    /// Full monitor bounds, physical px, virtual desktop coords.
    pub rect: Rect,
    /// Work area (without taskbar), physical px, virtual desktop coords.
    pub work_area: Rect,
    /// DPI scale factor (1.0 = 96 DPI).
    pub scale: f32,
    pub primary: bool,
    /// Advanced color / HDR currently enabled on this monitor.
    pub hdr: bool,
    /// SDR white level in nits as configured in Windows (80.0 if unknown).
    pub sdr_white_nits: f32,
}
