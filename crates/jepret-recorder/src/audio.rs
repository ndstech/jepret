//! WASAPI capture (loopback + microphone) into ring buffers of 48 kHz f32
//! stereo, plus device enumeration.

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use anyhow::{Context, Result};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Media::Audio::{
            eCapture, eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDevice,
            IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_LOOPBACK,
            AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
        },
        System::Com::{
            CoCreateInstance, CoTaskMemFree,
            StructuredStorage::{PropVariantClear, PropVariantToStringAlloc},
            CLSCTX_ALL, STGM_READ,
        },
    },
};

use crate::AudioDevice;

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
/// Cap of buffered audio per source (seconds); older data is dropped.
const MAX_BUFFERED_SECS: usize = 1;

/// Ring buffer of interleaved stereo f32 samples fed by a capture thread.
#[derive(Default)]
pub struct Source {
    buf: Mutex<VecDeque<f32>>,
}

impl Source {
    fn push(&self, samples: impl Iterator<Item = f32>) {
        let Ok(mut b) = self.buf.lock() else { return };
        b.extend(samples);
        let cap = SAMPLE_RATE as usize * CHANNELS * MAX_BUFFERED_SECS;
        if b.len() > cap {
            let drop = b.len() - cap;
            b.drain(..drop);
        }
    }

    /// Take up to `n` samples (fewer if not enough are buffered).
    pub fn take(&self, n: usize) -> Vec<f32> {
        let Ok(mut b) = self.buf.lock() else {
            return Vec::new();
        };
        let n = n.min(b.len());
        b.drain(..n).collect()
    }
}

fn enumerator() -> Result<IMMDeviceEnumerator> {
    // SAFETY: plain COM activation; COM is initialised by the calling thread.
    unsafe { Ok(CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?) }
}

fn device_id(dev: &IMMDevice) -> Result<String> {
    // SAFETY: GetId returns a CoTaskMem string that we free after copying.
    unsafe {
        let p: PWSTR = dev.GetId()?;
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as _));
        Ok(s)
    }
}

fn device_name(dev: &IMMDevice) -> String {
    // SAFETY: the PROPVARIANT is read only when it holds a VT_LPWSTR and is
    // cleared afterwards.
    unsafe {
        let Ok(store) = dev.OpenPropertyStore(STGM_READ) else {
            return String::new();
        };
        let Ok(mut pv) = store.GetValue(&PKEY_Device_FriendlyName) else {
            return String::new();
        };
        let name = match PropVariantToStringAlloc(&pv) {
            Ok(p) => {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as _));
                s
            }
            Err(_) => String::new(),
        };
        let _ = PropVariantClear(&mut pv);
        name
    }
}

pub fn list_microphones() -> Result<Vec<AudioDevice>> {
    let _com = crate::ComGuard::new();
    let en = enumerator()?;
    // SAFETY: COM calls on valid interfaces.
    unsafe {
        let default_id = en
            .GetDefaultAudioEndpoint(eCapture, eConsole)
            .ok()
            .and_then(|d| device_id(&d).ok());
        let coll = en.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
        let mut out = Vec::new();
        for i in 0..coll.GetCount()? {
            let dev = coll.Item(i)?;
            let id = device_id(&dev)?;
            out.push(AudioDevice {
                name: device_name(&dev),
                is_default: default_id.as_deref() == Some(id.as_str()),
                id,
            });
        }
        Ok(out)
    }
}

#[derive(Debug, Clone)]
pub enum Kind {
    Loopback,
    /// Empty id = default microphone.
    Mic(String),
}

struct Wasapi {
    client: IAudioClient,
    capture: IAudioCaptureClient,
}

fn open(kind: &Kind) -> Result<Wasapi> {
    let en = enumerator()?;
    // SAFETY: COM calls; the WAVEFORMATEX outlives Initialize.
    unsafe {
        let (dev, mut flags) = match kind {
            Kind::Loopback => (
                en.GetDefaultAudioEndpoint(eRender, eConsole)
                    .context("no default render device")?,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
            ),
            Kind::Mic(id) if id.is_empty() => (
                en.GetDefaultAudioEndpoint(eCapture, eConsole)
                    .context("no default microphone")?,
                0,
            ),
            Kind::Mic(id) => {
                let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
                (
                    en.GetDevice(PCWSTR(wide.as_ptr()))
                        .context("microphone not found")?,
                    0,
                )
            }
        };
        flags |= AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
        let fmt = WAVEFORMATEX {
            wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
            nChannels: CHANNELS as u16,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * CHANNELS as u32 * 4,
            nBlockAlign: CHANNELS as u16 * 4,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, &fmt, None)?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        Ok(Wasapi { client, capture })
    }
}

/// Start a capture thread that fills `src` until `stop` is set. Returns after
/// the device was opened (or failed to open).
pub fn spawn(
    kind: Kind,
    src: Arc<Source>,
    stop: Arc<AtomicBool>,
) -> Result<std::thread::JoinHandle<()>> {
    let (tx, rx) = std::sync::mpsc::channel::<Result<()>>();
    let handle = std::thread::Builder::new()
        .name(format!("jepret-audio-{kind:?}"))
        .spawn(move || {
            let _com = crate::ComGuard::new();
            let w = match open(&kind) {
                Ok(w) => {
                    let _ = tx.send(Ok(()));
                    w
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            while !stop.load(Ordering::Relaxed) {
                if let Err(e) = drain(&w, &src) {
                    tracing::warn!("audio capture {kind:?} stopped: {e}");
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            // SAFETY: valid started client.
            unsafe {
                let _ = w.client.Stop();
            }
        })?;
    rx.recv().context("audio thread died")??;
    Ok(handle)
}

fn drain(w: &Wasapi, src: &Source) -> Result<()> {
    // SAFETY: GetBuffer yields a pointer valid for `frames` frames until
    // ReleaseBuffer; we copy before releasing.
    unsafe {
        loop {
            if w.capture.GetNextPacketSize()? == 0 {
                return Ok(());
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            w.capture
                .GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
            let n = frames as usize * CHANNELS;
            if flags & 0x2 != 0 || data.is_null() {
                // AUDCLNT_BUFFERFLAGS_SILENT
                src.push((0..n).map(|_| 0.0));
            } else {
                let s = std::slice::from_raw_parts(data as *const f32, n);
                src.push(s.iter().copied());
            }
            w.capture.ReleaseBuffer(frames)?;
        }
    }
}
