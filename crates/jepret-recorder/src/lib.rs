//! Screen recorder. Owner: **T09**. See PRD F6.
//!
//! Video: Windows.Graphics.Capture frames (D3D11 textures, cropped on the
//! GPU) → Media Foundation Sink Writer, H.264 hardware MFT when available,
//! software fallback. Audio: WASAPI loopback + mic, mixed, AAC 48 kHz stereo.
//! Runs on its own threads; `start` returns immediately.
//!
//! Video and audio share one `Instant` (QPC) as time zero. Video frames are
//! emitted at a constant rate from the latest captured frame (so a static
//! screen still produces frames); the audio timeline is generated from the
//! wall clock with silence padding (loopback emits nothing while silent).

use std::{path::PathBuf, time::Duration};

use jepret_core::{Rect, Result};

pub mod mix;

#[cfg(windows)]
mod audio;
#[cfg(windows)]
mod mux;
#[cfg(windows)]
mod video;
#[cfg(windows)]
mod win;

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
    #[cfg(windows)]
    inner: Option<win::Session>,
    output: PathBuf,
}

impl Recorder {
    /// Start recording on background threads. Fails (without leaving a
    /// half-written file) if the region, encoder or audio device is unusable.
    pub fn start(opts: RecordOptions) -> Result<Self> {
        #[cfg(windows)]
        {
            let output = opts.output.clone();
            Ok(Self {
                inner: Some(win::Session::start(opts)?),
                output,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = opts;
            anyhow::bail!("recording is only supported on Windows")
        }
    }

    /// Finalize the MP4 and return its path.
    pub fn stop(mut self) -> Result<PathBuf> {
        #[cfg(windows)]
        if let Some(s) = self.inner.take() {
            s.stop()?;
        }
        Ok(self.output.clone())
    }

    pub fn elapsed(&self) -> Duration {
        #[cfg(windows)]
        if let Some(s) = &self.inner {
            return s.elapsed();
        }
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
    #[cfg(windows)]
    {
        audio::list_microphones()
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

/// RAII COM (MTA) initialisation for worker threads.
#[cfg(windows)]
pub(crate) struct ComGuard(bool);

#[cfg(windows)]
impl ComGuard {
    pub(crate) fn new() -> Self {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
        // SAFETY: balanced with CoUninitialize in Drop only when it succeeded.
        let ok = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() };
        Self(ok)
    }
}

#[cfg(windows)]
impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: matches the successful CoInitializeEx in `new`.
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}
