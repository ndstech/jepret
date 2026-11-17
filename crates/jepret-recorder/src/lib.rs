//! Screen recorder. Owner: **T09**. See PRD F6.
//!
//! Video: Windows.Graphics.Capture frames (D3D11 textures, no CPU copy if
//! possible) → Media Foundation Sink Writer, H.264 hardware MFT, fallback
//! software. Audio: WASAPI loopback + mic, mixed, AAC 48 kHz stereo.
//! Runs on its own threads; `start` returns immediately.

use std::{path::PathBuf, time::Duration};

use jepret_core::{Rect, Result};

#[derive(Debug, Clone)]
pub struct RecordOptions {
    pub output: PathBuf,
    /// Virtual-desktop px. Width/height are rounded down to even numbers.
    pub region: Rect,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub system_audio: bool,
    /// `None` = no mic. `Some("")` = default mic. Else device id.
    pub mic_device: Option<String>,
    pub show_cursor: bool,
}

pub struct Recorder {
    _private: (),
}

impl Recorder {
    pub fn start(_opts: RecordOptions) -> Result<Self> {
        anyhow::bail!("TODO(T09): Recorder::start")
    }

    /// Finalize the MP4 and return its path.
    pub fn stop(self) -> Result<PathBuf> {
        anyhow::bail!("TODO(T09): Recorder::stop")
    }

    pub fn elapsed(&self) -> Duration {
        Duration::ZERO
    }
}

#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

pub fn list_microphones() -> Result<Vec<AudioDevice>> {
    anyhow::bail!("TODO(T09): list_microphones")
}
