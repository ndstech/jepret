//! Media Foundation Sink Writer: H.264 (hardware MFT when available) + AAC
//! into MP4.

use std::path::Path;

use anyhow::{Context, Result};
use windows::{core::HSTRING, Win32::Media::MediaFoundation::*};

pub struct Mux {
    writer: IMFSinkWriter,
    video: u32,
    audio: Option<u32>,
}

// SAFETY: IMFSinkWriter is free-threaded; all access goes through a Mutex.
unsafe impl Send for Mux {}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

fn video_types(
    w: u32,
    h: u32,
    fps: u32,
    bitrate_kbps: u32,
) -> Result<(IMFMediaType, IMFMediaType)> {
    // SAFETY: plain attribute setters on freshly created media types.
    unsafe {
        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate_kbps.saturating_mul(1000))?;
        out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        out.SetUINT32(&MF_MT_MPEG2_PROFILE, 100)?; // High
        out.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
        out.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
        out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;

        let inp = MFCreateMediaType()?;
        inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        inp.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        inp.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        inp.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
        inp.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
        inp.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
        // Positive stride = top-down rows (our buffers are top-down).
        inp.SetUINT32(&MF_MT_DEFAULT_STRIDE, w * 4)?;
        Ok((out, inp))
    }
}

fn audio_types() -> Result<(IMFMediaType, IMFMediaType)> {
    let rate = crate::audio::SAMPLE_RATE;
    // SAFETY: attribute setters on freshly created media types.
    unsafe {
        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)?;
        out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)?; // 192 kbps
        out.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 1)?;
        out.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
        out.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29)?;

        let inp = MFCreateMediaType()?;
        inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        inp.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        inp.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        inp.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)?;
        inp.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        inp.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
        inp.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, rate * 4)?;
        Ok((out, inp))
    }
}

fn create_writer(path: &Path, hardware: bool) -> Result<IMFSinkWriter> {
    // SAFETY: attribute store and writer creation; `url` outlives the call.
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 3)?;
        let attrs: IMFAttributes = attrs.context("MFCreateAttributes")?;
        attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, hardware as u32)?;
        attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
        let url = HSTRING::from(path.as_os_str());
        Ok(MFCreateSinkWriterFromURL(&url, None, &attrs)?)
    }
}

impl Mux {
    pub fn new(
        path: &Path,
        w: u32,
        h: u32,
        fps: u32,
        bitrate_kbps: u32,
        audio: bool,
    ) -> Result<Self> {
        // Try hardware encoders first, then fall back to software.
        match Self::build(path, w, h, fps, bitrate_kbps, audio, true) {
            Ok(m) => Ok(m),
            Err(e) => {
                tracing::warn!("hardware H.264 encoder unavailable ({e:#}); using software");
                Self::build(path, w, h, fps, bitrate_kbps, audio, false)
            }
        }
    }

    fn build(
        path: &Path,
        w: u32,
        h: u32,
        fps: u32,
        bitrate_kbps: u32,
        audio: bool,
        hardware: bool,
    ) -> Result<Self> {
        let writer = create_writer(path, hardware)?;
        let (vout, vin) = video_types(w, h, fps, bitrate_kbps)?;
        // SAFETY: configuring the writer before BeginWriting.
        unsafe {
            let video = writer.AddStream(&vout).context("AddStream(video)")?;
            writer
                .SetInputMediaType(video, &vin, None)
                .context("SetInputMediaType(video)")?;
            let audio = if audio {
                let (aout, ain) = audio_types()?;
                let a = writer.AddStream(&aout).context("AddStream(audio)")?;
                writer
                    .SetInputMediaType(a, &ain, None)
                    .context("SetInputMediaType(audio)")?;
                Some(a)
            } else {
                None
            };
            writer.BeginWriting().context("BeginWriting")?;
            tracing::info!(
                "sink writer ready ({} H.264 encoder)",
                if hardware {
                    "hardware-capable"
                } else {
                    "software"
                }
            );
            Ok(Self {
                writer,
                video,
                audio,
            })
        }
    }

    fn write(&self, stream: u32, data: &[u8], ts: i64, dur: i64) -> Result<()> {
        // SAFETY: buffer is locked only while copying `data` into it.
        unsafe {
            let buf = MFCreateMemoryBuffer(data.len() as u32)?;
            let mut p = std::ptr::null_mut();
            buf.Lock(&mut p, None, None)?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
            buf.Unlock()?;
            buf.SetCurrentLength(data.len() as u32)?;
            let s = MFCreateSample()?;
            s.AddBuffer(&buf)?;
            s.SetSampleTime(ts)?;
            s.SetSampleDuration(dur)?;
            self.writer.WriteSample(stream, &s)?;
        }
        Ok(())
    }

    /// `bgra`: tightly packed top-down. `ts`/`dur` in 100 ns.
    pub fn write_video(&self, bgra: &[u8], ts: i64, dur: i64) -> Result<()> {
        self.write(self.video, bgra, ts, dur)
    }

    pub fn write_audio(&self, pcm: &[i16], ts: i64, dur: i64) -> Result<()> {
        let Some(a) = self.audio else { return Ok(()) };
        // SAFETY: reinterpret i16 slice as bytes (no padding, same lifetime).
        let bytes = unsafe { std::slice::from_raw_parts(pcm.as_ptr() as *const u8, pcm.len() * 2) };
        self.write(a, bytes, ts, dur)
    }

    pub fn finalize(&self) -> Result<()> {
        // SAFETY: finalizing the writer writes the MP4 moov atom.
        unsafe { Ok(self.writer.Finalize()?) }
    }
}
