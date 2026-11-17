//! Capture pipeline glue. Owner: **T12**.
//!
//! kind → capture (jepret-capture / overlay / scroll / recorder)
//!      → workflow::run → history.insert(saved_path) if saved_permanently
//!      → update state.last_region for Region/Scroll/RecordRegion.
//!
//! Runs on a worker thread (never the daemon's message-loop thread), because
//! the overlay and the scroll driver block. Every failure is returned as an
//! `Err` (or reported through [`Ctx::notify`]) — nothing here may panic.

use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::Context as _;
use jepret_capture::CaptureOptions;
use jepret_core::{
    paths,
    state::{AppState, LastRegion},
    Action, CaptureKind, CaptureResult, Config, Image, MonitorId, MonitorInfo, Point, Rect,
};
use jepret_history::{EntryKind, HistoryDb, NewEntry};
use jepret_overlay::{OverlayOptions, RegionBorder};
use jepret_recorder::{RecordOptions, Recorder};
use jepret_scroll::{ScrollOptions, StitchOptions};
use jepret_workflow::WorkflowContext;
use tracing::{debug, info, warn};
use windows::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
    System::Threading::{CreateEventW, SetEvent, INFINITE},
    UI::WindowsAndMessaging::{
        DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MSG,
        PM_REMOVE, QS_ALLINPUT,
    },
};

/// Show a user-visible notification: `(title, body)`.
pub type NotifyFn = fn(&str, &str);

pub struct Ctx<'a> {
    pub config: &'a Config,
    pub notify: NotifyFn,
}

/// Called after recording starts/stops so the daemon can refresh the tray.
pub static STATE_HOOK: OnceLock<fn()> = OnceLock::new();

fn state_changed() {
    if let Some(f) = STATE_HOOK.get() {
        f();
    }
}

// ---------------------------------------------------------------- recording state

struct RecState {
    /// Manual-reset event (raw HANDLE) the recording thread waits on.
    stop_event: isize,
    started: Instant,
}

static RECORDING: Mutex<Option<RecState>> = Mutex::new(None);

fn lock_rec() -> std::sync::MutexGuard<'static, Option<RecState>> {
    RECORDING.lock().unwrap_or_else(|p| p.into_inner())
}

pub fn is_recording() -> bool {
    lock_rec().is_some()
}

pub fn recording_elapsed() -> Option<std::time::Duration> {
    lock_rec().as_ref().map(|r| r.started.elapsed())
}

/// Ask the recording thread (if any) to finish. Returns whether one existed.
pub fn stop_recording() -> bool {
    let guard = lock_rec();
    match guard.as_ref() {
        Some(r) => {
            // SAFETY: the handle stays valid until the recording thread clears
            // RECORDING (under this same lock) and only then closes it.
            let _ = unsafe { SetEvent(HANDLE(r.stop_event as *mut _)) };
            true
        }
        None => false,
    }
}

// ---------------------------------------------------------------- entry point

pub fn handle(ctx: &Ctx<'_>, kind: CaptureKind, actions: &[Action]) -> anyhow::Result<()> {
    if kind.is_recording() {
        return record(ctx, kind, actions);
    }
    let t0 = Instant::now();
    let Some(result) = acquire(ctx, kind)? else {
        debug!(?kind, "capture cancelled");
        return Ok(());
    };
    debug!(?kind, ms = t0.elapsed().as_millis() as u64, "captured {}x{}", result.image.width, result.image.height);
    finish_image(ctx, &result, actions)
}

fn opts(ctx: &Ctx<'_>) -> CaptureOptions {
    CaptureOptions::from(&ctx.config.capture)
}

fn acquire(ctx: &Ctx<'_>, kind: CaptureKind) -> anyhow::Result<Option<CaptureResult>> {
    let copts = opts(ctx);
    let now = SystemTime::now();
    match kind {
        CaptureKind::Fullscreen => {
            let (image, region) = jepret_capture::capture_virtual_screen(&copts).context("fullscreen capture")?;
            Ok(Some(CaptureResult { image, kind, region, monitor: None, captured_at: now }))
        }
        CaptureKind::ActiveMonitor => {
            let m = active_monitor()?;
            let (image, region) = jepret_capture::capture_monitor(&m.id, &copts).context("monitor capture")?;
            Ok(Some(CaptureResult { image, kind, region, monitor: Some(m.id), captured_at: now }))
        }
        CaptureKind::ActiveWindow => {
            let (image, region) = jepret_capture::capture_foreground_window(&copts).context("window capture")?;
            let monitor = jepret_capture::monitor_at(center(region)).ok().map(|m| m.id);
            Ok(Some(CaptureResult { image, kind, region, monitor, captured_at: now }))
        }
        CaptureKind::Region => region_capture(ctx, None, now),
        CaptureKind::LastRegion => match resolve_last_region(ctx)? {
            LastResolution::Use(rect, monitor) => {
                let image = jepret_capture::capture_rect(rect, &copts).context("last-region capture")?;
                Ok(Some(CaptureResult { image, kind, region: rect, monitor, captured_at: now }))
            }
            LastResolution::Fallback(msg) => {
                if let Some(m) = msg {
                    (ctx.notify)("Jepret", m);
                }
                region_capture(ctx, None, now)
            }
        },
        CaptureKind::Scroll => {
            let Some((rect, ..)) = pick_region(ctx, None, Some("Scroll capture: pilih area"))? else {
                return Ok(None);
            };
            let image = run_scroll(ctx, rect)?;
            let monitor = jepret_capture::monitor_at(center(rect)).ok().map(|m| m.id);
            save_last_region(rect, monitor.clone());
            Ok(Some(CaptureResult { image, kind, region: rect, monitor, captured_at: now }))
        }
        CaptureKind::RecordRegion | CaptureKind::RecordMonitor => {
            anyhow::bail!("recording kinds are handled by record()")
        }
    }
}

fn active_monitor() -> anyhow::Result<MonitorInfo> {
    let p = jepret_capture::cursor_pos().context("cursor position")?;
    jepret_capture::monitor_at(p).context("monitor at cursor")
}

fn region_capture(
    ctx: &Ctx<'_>,
    initial: Option<Rect>,
    now: SystemTime,
) -> anyhow::Result<Option<CaptureResult>> {
    let Some((rect, frozen, origin)) = pick_region(ctx, initial, None)? else {
        return Ok(None);
    };
    let image = frozen
        .crop(rect.relative_to(origin.origin()))
        .context("crop selected region")?;
    let region = Rect::new(rect.x, rect.y, image.width, image.height);
    let monitor = jepret_capture::monitor_at(center(region)).ok().map(|m| m.id);
    save_last_region(region, monitor.clone());
    Ok(Some(CaptureResult { image, kind: CaptureKind::Region, region, monitor, captured_at: now }))
}

/// Freeze the desktop and let the user pick a rect.
/// Returns `(selection, frozen image, origin rect of the frozen image)`.
fn pick_region(
    ctx: &Ctx<'_>,
    initial: Option<Rect>,
    hint: Option<&str>,
) -> anyhow::Result<Option<(Rect, Image, Rect)>> {
    let (frozen, origin) = jepret_capture::capture_virtual_screen(&opts(ctx)).context("freeze screen")?;
    let monitors = jepret_capture::monitors().context("enumerate monitors")?;
    let window_rects = if ctx.config.capture.overlay_snap_to_windows {
        jepret_capture::visible_window_rects().unwrap_or_else(|e| {
            warn!("visible_window_rects failed: {e:#}");
            Vec::new()
        })
    } else {
        Vec::new()
    };
    let oo = OverlayOptions {
        initial,
        magnifier: ctx.config.capture.overlay_magnifier,
        window_rects,
        hint: hint.map(str::to_owned),
    };
    let sel = jepret_overlay::select_region(&frozen, origin, &monitors, &oo).context("region overlay")?;
    Ok(sel.filter(|r| !r.is_empty()).map(|r| (r, frozen, origin)))
}

fn run_scroll(ctx: &Ctx<'_>, rect: Rect) -> anyhow::Result<Image> {
    let so = ScrollOptions {
        max_frames: ctx.config.scroll.max_frames,
        scroll_delay_ms: ctx.config.scroll.scroll_delay_ms,
        wheel_notches: ctx.config.scroll.wheel_notches,
        capture: opts(ctx),
        stitch: StitchOptions::default(),
    };
    // The scroll crate's stitcher may panic (todo! stubs). In dev/test builds
    // the panic is caught here; release uses panic=abort (see report).
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| jepret_scroll::capture_scrolling(rect, &so))) {
        Ok(r) => r.context("scroll capture"),
        Err(p) => anyhow::bail!("scroll capture panicked: {}", panic_message(&*p)),
    }
}

pub fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

// ---------------------------------------------------------------- last region (F4)

#[derive(Debug, PartialEq)]
enum LastResolution {
    Use(Rect, Option<MonitorId>),
    /// Fall back to interactive region mode, optionally with a notification.
    Fallback(Option<&'static str>),
}

fn resolve_last_region(ctx: &Ctx<'_>) -> anyhow::Result<LastResolution> {
    let state = AppState::load(&paths::state_file());
    let _ = ctx;
    let monitors = jepret_capture::monitors().unwrap_or_default();
    Ok(decide_last_region(state.last_region.as_ref(), &monitors))
}

fn decide_last_region(last: Option<&LastRegion>, monitors: &[MonitorInfo]) -> LastResolution {
    let Some(last) = last else {
        return LastResolution::Fallback(None);
    };
    let monitor_ok = match &last.monitor {
        Some(id) => monitors.iter().any(|m| &m.id == id),
        None => monitors.iter().any(|m| m.rect.intersect(&last.rect).is_some()),
    };
    if !monitor_ok || last.rect.is_empty() {
        return LastResolution::Fallback(Some("Monitor region terakhir sudah tidak ada. Pilih region baru."));
    }
    LastResolution::Use(last.rect, last.monitor.clone())
}

fn save_last_region(rect: Rect, monitor: Option<MonitorId>) {
    let st = AppState { last_region: Some(LastRegion { rect, monitor }) };
    if let Err(e) = st.save(&paths::state_file()) {
        warn!("save last_region failed: {e:#}");
    }
}

fn center(r: Rect) -> Point {
    Point::new(r.x + (r.width / 2) as i32, r.y + (r.height / 2) as i32)
}

// ---------------------------------------------------------------- after-capture

fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("jepret.exe"))
}

fn finish_image(ctx: &Ctx<'_>, result: &CaptureResult, actions: &[Action]) -> anyhow::Result<()> {
    let wctx = WorkflowContext { config: ctx.config, exe_path: exe_path() };
    let outcome = jepret_workflow::run(&wctx, result, actions).context("workflow")?;
    report_errors(ctx, &outcome.errors);
    show_notification(ctx, outcome.notification.clone());
    if outcome.saved_permanently {
        if let Some(path) = &outcome.saved_path {
            record_history(
                ctx,
                path,
                EntryKind::Image,
                result.image.width,
                result.image.height,
                crate::command::kind_name(result.kind),
            );
        }
    }
    Ok(())
}

/// Show the workflow's `notify` text (if any) as a tray balloon.
fn show_notification(ctx: &Ctx<'_>, text: Option<String>) {
    if let Some(t) = text {
        (ctx.notify)("Jepret", &t);
    }
}

fn report_errors(ctx: &Ctx<'_>, errors: &[(String, String)]) {
    if errors.is_empty() {
        return;
    }
    for (action, err) in errors {
        warn!("action {action} failed: {err}");
    }
    let body = errors
        .iter()
        .map(|(a, e)| format!("{a}: {e}"))
        .collect::<Vec<_>>()
        .join("\n");
    (ctx.notify)("Jepret: aksi gagal", &body);
}

fn record_history(ctx: &Ctx<'_>, path: &Path, kind: EntryKind, width: u32, height: u32, source: &str) {
    if !ctx.config.history.enabled {
        return;
    }
    let res = (|| -> anyhow::Result<()> {
        paths::ensure_dir(&paths::data_dir())?;
        let db = HistoryDb::open(&paths::history_db())?;
        let created_at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
        let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        db.insert(&NewEntry {
            path: path.to_path_buf(),
            kind,
            created_at,
            width,
            height,
            bytes,
            source: source.to_string(),
        })?;
        Ok(())
    })();
    if let Err(e) = res {
        // A history failure must not look like a failed capture.
        warn!("history insert failed: {e:#}");
        (ctx.notify)("Jepret", &format!("Riwayat gagal disimpan: {e:#}"));
    }
}

/// Startup cleanup (`history.cleanup.run_on_startup`). Intended for a background thread.
pub fn run_cleanup(config: &Config) -> anyhow::Result<()> {
    let rules = &config.history.cleanup;
    if !config.history.enabled || !rules.enabled || !rules.run_on_startup {
        return Ok(());
    }
    paths::ensure_dir(&paths::data_dir())?;
    let db = HistoryDb::open(&paths::history_db())?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    let rep = db.cleanup(rules, now)?;
    info!(
        "cleanup: {} files deleted, {} entries removed, {} bytes freed, {} errors",
        rep.files_deleted,
        rep.entries_removed,
        rep.bytes_freed,
        rep.errors.len()
    );
    Ok(())
}

// ---------------------------------------------------------------- recording

fn record(ctx: &Ctx<'_>, kind: CaptureKind, actions: &[Action]) -> anyhow::Result<()> {
    // Same hotkey toggles: second press stops.
    if stop_recording() {
        return Ok(());
    }
    let rect = if kind == CaptureKind::RecordMonitor {
        active_monitor()?.rect
    } else {
        let initial = None;
        match pick_region(ctx, initial, Some("Rekam: pilih area"))? {
            Some((r, ..)) => r,
            None => return Ok(()),
        }
    };

    let mut cfg = ctx.config.clone();
    if let Some(dir) = &cfg.recording.output_dir {
        cfg.general.output_dir = dir.clone();
    }
    let output = jepret_workflow::output_path(&cfg, "mp4").context("recording output path")?;
    if let Some(dir) = output.parent() {
        paths::ensure_dir(dir)?;
    }
    let ro = RecordOptions {
        output,
        region: rect,
        fps: cfg.recording.fps,
        bitrate_kbps: cfg.recording.bitrate_kbps,
        system_audio: cfg.recording.system_audio,
        mic_device: cfg.recording.microphone.then(|| cfg.recording.mic_device.clone().unwrap_or_default()),
        show_cursor: cfg.recording.show_cursor,
    };
    let actions: Vec<Action> = if actions.is_empty() { cfg.recording.actions.clone() } else { actions.to_vec() };
    let notify = ctx.notify;
    if kind == CaptureKind::RecordRegion {
        save_last_region(rect, jepret_capture::monitor_at(center(rect)).ok().map(|m| m.id));
    }

    // SAFETY: plain event creation; handle is closed by the recording thread.
    let event = unsafe { CreateEventW(None, true, false, None) }.context("CreateEventW")?;
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let event_raw = event.0 as isize;
    std::thread::Builder::new()
        .name("jepret-recorder".into())
        .spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                recording_thread(cfg, ro, actions, notify, event_raw, kind, tx)
            }));
            if let Err(p) = r {
                warn!("recording thread panicked: {}", panic_message(&*p));
                *lock_rec() = None;
                state_changed();
                notify("Jepret", "Perekaman berhenti karena kesalahan internal.");
            }
            // SAFETY: closing the handle we created, after RECORDING was cleared.
            let _ = unsafe { CloseHandle(HANDLE(event_raw as *mut _)) };
        })
        .context("spawn recording thread")?;
    match rx.recv() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => anyhow::bail!("{e}"),
        Err(_) => anyhow::bail!("recording thread ended before starting"),
    }
}

/// Owns the border window and the recorder (both may be thread-affine) and
/// pumps messages until the stop event fires.
fn recording_thread(
    cfg: Config,
    ro: RecordOptions,
    actions: Vec<Action>,
    notify: NotifyFn,
    event_raw: isize,
    kind: CaptureKind,
    started: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let (width, height) = (ro.region.width, ro.region.height);
    let border = match RegionBorder::show(ro.region) {
        Ok(b) => Some(b),
        Err(e) => {
            warn!("recording border failed: {e:#}");
            None
        }
    };
    let recorder = match Recorder::start(ro) {
        Ok(r) => r,
        Err(e) => {
            let _ = started.send(Err(format!("{e:#}")));
            return;
        }
    };
    *lock_rec() = Some(RecState { stop_event: event_raw, started: Instant::now() });
    state_changed();
    let _ = started.send(Ok(()));
    info!("recording started");

    let event = HANDLE(event_raw as *mut _);
    loop {
        // SAFETY: waits on our own event plus this thread's message queue.
        let r = unsafe { MsgWaitForMultipleObjects(Some(&[event]), false, INFINITE, QS_ALLINPUT) };
        if r == WAIT_OBJECT_0 {
            break;
        }
        let mut msg = MSG::default();
        // SAFETY: standard message pump for the border window on this thread.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    #[allow(clippy::drop_non_drop)] // RegionBorder gets a Drop impl in T03
    std::mem::drop(border);
    let stopped = recorder.stop();
    *lock_rec() = None;
    state_changed();

    let ctx = Ctx { config: &cfg, notify };
    match stopped {
        Ok(path) => {
            info!("recording saved: {}", path.display());
            let wctx = WorkflowContext { config: &cfg, exe_path: exe_path() };
            match jepret_workflow::run_for_file(&wctx, &path, &actions) {
                Ok(outcome) => {
                    report_errors(&ctx, &outcome.errors);
                    if let Some(t) = outcome.notification {
                        notify("Jepret", &t);
                    }
                }
                Err(e) => {
                    warn!("recording workflow failed: {e:#}");
                    notify("Jepret", &format!("Aksi setelah rekaman gagal: {e:#}"));
                }
            }
            record_history(&ctx, &path, EntryKind::Video, width, height, crate::command::kind_name(kind));
        }
        Err(e) => {
            warn!("recorder.stop failed: {e:#}");
            notify("Jepret", &format!("Gagal menyimpan rekaman: {e:#}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(id: &str, rect: Rect) -> MonitorInfo {
        MonitorInfo {
            id: MonitorId(id.into()),
            name: id.into(),
            rect,
            work_area: rect,
            scale: 1.0,
            primary: false,
            hdr: false,
            sdr_white_nits: 80.0,
        }
    }

    #[test]
    fn last_region_none_falls_back_silently() {
        assert_eq!(decide_last_region(None, &[]), LastResolution::Fallback(None));
    }

    #[test]
    fn last_region_monitor_gone_notifies() {
        let last = LastRegion { rect: Rect::new(0, 0, 10, 10), monitor: Some(MonitorId("B".into())) };
        let ms = [mon("A", Rect::new(0, 0, 1920, 1080))];
        assert!(matches!(decide_last_region(Some(&last), &ms), LastResolution::Fallback(Some(_))));
    }

    #[test]
    fn last_region_ok() {
        let last = LastRegion { rect: Rect::new(-100, 5, 50, 50), monitor: Some(MonitorId("A".into())) };
        let ms = [mon("A", Rect::new(-1920, 0, 1920, 1080))];
        assert_eq!(
            decide_last_region(Some(&last), &ms),
            LastResolution::Use(last.rect, Some(MonitorId("A".into())))
        );
    }

    #[test]
    fn last_region_without_monitor_checks_intersection() {
        let last = LastRegion { rect: Rect::new(5000, 5000, 50, 50), monitor: None };
        let ms = [mon("A", Rect::new(0, 0, 1920, 1080))];
        assert!(matches!(decide_last_region(Some(&last), &ms), LastResolution::Fallback(Some(_))));
    }

    #[test]
    fn center_of_rect() {
        assert_eq!(center(Rect::new(-100, 10, 50, 20)), Point::new(-75, 20));
    }

    #[test]
    fn panic_messages() {
        let p = std::panic::catch_unwind(|| panic!("boom")).unwrap_err();
        assert_eq!(panic_message(&*p), "boom");
    }
}
