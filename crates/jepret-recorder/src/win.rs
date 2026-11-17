//! Recording session orchestration (Windows).

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use jepret_core::Rect;
use windows::Win32::Media::MediaFoundation::{MFShutdown, MFStartup, MFSTARTUP_FULL, MF_VERSION};

use crate::{
    audio::{self, Kind, Source, CHANNELS, SAMPLE_RATE},
    mix,
    mux::Mux,
    video::{self, VideoCapture},
    RecordOptions,
};

pub struct Session {
    stop: Arc<AtomicBool>,
    controller: Option<JoinHandle<Result<()>>>,
    start: Instant,
}

impl Session {
    pub fn start(opts: RecordOptions) -> Result<Self> {
        if opts.fps == 0 || opts.fps > 120 {
            return Err(anyhow!("fps must be 1..=120"));
        }
        if let Some(dir) = opts.output.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel::<Result<Instant>>();
        let stop2 = stop.clone();
        let controller = std::thread::Builder::new()
            .name("jepret-recorder".into())
            .spawn(move || run(opts, stop2, tx))?;
        let start = rx.recv().context("recorder thread died during start")??;
        Ok(Self {
            stop,
            controller: Some(controller),
            start,
        })
    }

    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    pub fn stop(mut self) -> Result<()> {
        self.finish()
    }

    fn finish(&mut self) -> Result<()> {
        self.stop.store(true, Ordering::Relaxed);
        match self.controller.take() {
            Some(h) => h.join().map_err(|_| anyhow!("recorder thread panicked"))?,
            None => Ok(()),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Dropping without stop(): still finalize so the file stays valid.
        if let Err(e) = self.finish() {
            tracing::warn!("finalizing on drop failed: {e:#}");
        }
    }
}

struct MfGuard;
impl MfGuard {
    fn new() -> Result<Self> {
        // SAFETY: balanced with MFShutdown in Drop.
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL)? };
        Ok(Self)
    }
}
impl Drop for MfGuard {
    fn drop(&mut self) {
        // SAFETY: matches successful MFStartup.
        unsafe {
            let _ = MFShutdown();
        }
    }
}

type Setup = (
    VideoCapture,
    Arc<Mutex<Mux>>,
    Vec<(Arc<Source>, JoinHandle<()>)>,
);

fn setup(opts: &RecordOptions, stop: &Arc<AtomicBool>) -> Result<Setup> {
    let mons = video::monitors()?;
    let rects: Vec<Rect> = mons.iter().map(|m| m.1).collect();
    let idx = mix::best_monitor(opts.region, &rects)
        .ok_or_else(|| anyhow!("region {:?} is outside every monitor", opts.region))?;
    let (hmon, mrect) = mons[idx];
    let r = mix::fit_region(opts.region, mrect).ok_or_else(|| anyhow!("region too small"))?;
    let crop = Rect::new(r.x - mrect.x, r.y - mrect.y, r.width, r.height);

    let want_audio = opts.system_audio || opts.mic_device.is_some();
    let mut sources = Vec::new();
    let mut kinds = Vec::new();
    if opts.system_audio {
        kinds.push(Kind::Loopback);
    }
    if let Some(m) = &opts.mic_device {
        kinds.push(Kind::Mic(m.clone()));
    }
    for k in kinds {
        let src = Arc::new(Source::default());
        match audio::spawn(k.clone(), src.clone(), stop.clone()) {
            Ok(h) => sources.push((src, h)),
            // Audio is best-effort: keep recording video if a device fails.
            Err(e) => tracing::warn!("audio source {k:?} unavailable: {e:#}"),
        }
    }

    let cleanup = |sources: Vec<(Arc<Source>, JoinHandle<()>)>| {
        stop.store(true, Ordering::Relaxed);
        for (_, h) in sources {
            let _ = h.join();
        }
        stop.store(false, Ordering::Relaxed);
    };

    let cap = match VideoCapture::start(hmon, crop, opts.show_cursor) {
        Ok(c) => c,
        Err(e) => {
            cleanup(sources);
            return Err(e.context("start screen capture"));
        }
    };
    let mux = match Mux::new(
        &opts.output,
        cap.width,
        cap.height,
        opts.fps,
        opts.bitrate_kbps.max(500),
        want_audio && !sources.is_empty(),
    ) {
        Ok(m) => m,
        Err(e) => {
            cleanup(sources);
            return Err(e.context("create MP4 writer"));
        }
    };
    Ok((cap, Arc::new(Mutex::new(mux)), sources))
}

fn run(
    opts: RecordOptions,
    stop: Arc<AtomicBool>,
    ready: std::sync::mpsc::Sender<Result<Instant>>,
) -> Result<()> {
    let _com = crate::ComGuard::new();
    let _mf = match MfGuard::new() {
        Ok(m) => m,
        Err(e) => {
            let _ = ready.send(Err(anyhow!("{e:#}")));
            return Err(e);
        }
    };
    let (cap, mux, sources) = match setup(&opts, &stop) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(anyhow!("{e:#}")));
            return Err(e);
        }
    };
    // Time zero for audio and video: after all setup, so no leading offset.
    let start = Instant::now();
    let _ = ready.send(Ok(start));

    let (srcs, handles): (Vec<_>, Vec<_>) = sources.into_iter().unzip();
    let mixer = if srcs.is_empty() {
        None
    } else {
        let (mux, stop) = (mux.clone(), stop.clone());
        Some(std::thread::spawn(move || {
            mixer_loop(mux, srcs, stop, start)
        }))
    };

    let res = video_loop(&cap, &mux, opts.fps, &stop, start);

    stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }
    if let Some(m) = mixer {
        let _ = m.join();
    }
    drop(cap);
    let fin = match mux.lock() {
        Ok(m) => m.finalize(),
        Err(_) => Err(anyhow!("mux poisoned")),
    };
    res.and(fin.context("finalize MP4"))
}

fn video_loop(
    cap: &VideoCapture,
    mux: &Arc<Mutex<Mux>>,
    fps: u32,
    stop: &AtomicBool,
    start: Instant,
) -> Result<()> {
    let frame = 10_000_000i64 / fps as i64;
    let mut last: Option<i64> = None;
    let mut buf = Vec::new();
    let mut written = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let elapsed = (start.elapsed().as_nanos() / 100) as i64;
        let idx = mix::next_frame_index(elapsed, frame, last);
        let due = idx * frame;
        if due > elapsed {
            std::thread::sleep(Duration::from_nanos(((due - elapsed) * 100) as u64));
            if stop.load(Ordering::Relaxed) {
                break;
            }
        }
        if !cap.read_frame(&mut buf)? {
            // No frame from WGC yet; try again shortly.
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        mux.lock()
            .map_err(|_| anyhow!("mux poisoned"))?
            .write_video(&buf, due, frame)?;
        last = Some(idx);
        written += 1;
    }
    tracing::debug!("video frames written: {written}");
    Ok(())
}

fn mixer_loop(
    mux: Arc<Mutex<Mux>>,
    sources: Vec<Arc<Source>>,
    stop: Arc<AtomicBool>,
    start: Instant,
) {
    const CHUNK: u64 = 480; // 10 ms
    let mut written: u64 = 0;
    let mut finishing = false;
    loop {
        let target = (start.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as u64;
        while written + CHUNK <= target {
            let n = CHUNK as usize * CHANNELS;
            let bufs: Vec<Vec<f32>> = sources.iter().map(|s| s.take(n)).collect();
            let pcm = mix::mix_to_i16(&bufs, n);
            let ts = (written * 10_000_000 / SAMPLE_RATE as u64) as i64;
            let dur = (CHUNK * 10_000_000 / SAMPLE_RATE as u64) as i64;
            let r = mux
                .lock()
                .map_err(|_| anyhow!("mux poisoned"))
                .and_then(|m| m.write_audio(&pcm, ts, dur));
            if let Err(e) = r {
                tracing::warn!("audio write failed: {e:#}");
                return;
            }
            written += CHUNK;
        }
        if finishing {
            return;
        }
        finishing = stop.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(10));
    }
}
