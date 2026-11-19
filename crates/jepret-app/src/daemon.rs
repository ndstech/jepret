//! Daemon: single instance, tray, global hotkeys, message loop. Owner: **T12**.
//!
//! Architecture (everything is plain Win32 — no tray/hotkey crates, to keep
//! the resident footprint small):
//!
//! * **Main thread** owns a hidden top-level window (class `Jepret.Daemon.Wnd`)
//!   and runs the message loop. It handles `WM_HOTKEY` (RegisterHotKey), the
//!   tray icon + context menu (Shell_NotifyIcon), `WM_COPYDATA` from second
//!   instances, and a directory change notification on the config dir. The
//!   loop blocks in `MsgWaitForMultipleObjects` — no polling, no timers while idle.
//! * **Worker threads** (`jepret-capture`, one at a time via [`BUSY`]) run
//!   `dispatch::handle` under `catch_unwind`. They talk back to the main
//!   thread only with `PostMessage` (balloon notifications, tray refresh).
//! * **Recording thread** (`jepret-recorder`) owns the border + recorder while
//!   recording and is stopped through an event handle.
//! * **Single instance**: named mutex `Local\Jepret.Daemon`. A second
//!   `jepret capture <kind>` finds the first one's window and sends the
//!   command as `WM_COPYDATA` (UTF-8 text, see [`crate::command::Command`]).
//! * Config reload: a debounced (400 ms) reload when `config.toml` changes;
//!   unchanged content (e.g. `state.toml` writes) is ignored.

use std::{
    process::Child,
    sync::{
        atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use anyhow::Context as _;
use jepret_core::{paths, Action, CaptureKind, Config};
use jepret_ui_theme::Icon;
use tracing::{debug, error, info, warn};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{
            CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, LRESULT, POINT,
            WAIT_OBJECT_0, WPARAM,
        },
        Graphics::Gdi::{
            CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
            HGDIOBJ,
        },
        Storage::FileSystem::{
            FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification,
            FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
        },
        System::{
            DataExchange::COPYDATASTRUCT,
            LibraryLoader::GetModuleHandleW,
            Registry::{
                RegCloseKey, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY,
                HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_DWORD,
                RRF_RT_REG_SZ,
            },
            Threading::{CreateMutexW, INFINITE},
        },
        UI::{
            HiDpi::{GetDpiForSystem, GetDpiForWindow, GetSystemMetricsForDpi},
            Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT},
            Shell::{
                ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_ERROR,
                NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_SELECT,
                NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
            },
            WindowsAndMessaging::*,
        },
    },
};

use crate::{
    command::{self, Command},
    dispatch::{self, Ctx},
    hotkey, raster,
};

const CLASS_NAME: PCWSTR = w!("Jepret.Daemon.Wnd");
const MUTEX_NAME: PCWSTR = w!("Local\\Jepret.Daemon");
const COPYDATA_MAGIC: usize = 0x4A50_5254; // "JPRT"

const WM_TRAY: u32 = WM_APP + 1;
const WM_USER_NOTIFY: u32 = WM_APP + 2;
const WM_REC_STATE: u32 = WM_APP + 3;

const TIMER_RELOAD: usize = 1;
const TIMER_REC: usize = 2;
const TIMER_IDLE: usize = 3;
const IDLE_RELEASE_MS: u32 = 60_000;
const WM_CAPTURE_DONE: u32 = WM_APP + 5;

const ID_GALLERY: usize = 200;
const ID_SETTINGS: usize = 201;
const ID_OPEN_FOLDER: usize = 202;
const ID_QUIT: usize = 203;
const ID_STOP_REC: usize = 204;
const ID_OCR: usize = 205;
const ID_KIND_BASE: usize = 100;

static HWND_RAW: AtomicIsize = AtomicIsize::new(0);
static BUSY: AtomicBool = AtomicBool::new(false);
/// Set while the last balloon shown was the "PrtSc is used by Snipping Tool"
/// hint, so a click on it opens the keyboard settings page.
static SNIP_HINT_ACTIVE: AtomicBool = AtomicBool::new(false);
const NIN_BALLOONUSERCLICK_EV: u32 = WM_USER + 5;
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

struct Daemon {
    config: Arc<Config>,
    /// `RegisterHotKey` id → index into `config.hotkeys`.
    registered: Vec<i32>,
    icon_idle: isize,
    icon_rec: isize,
}

static STATE: Mutex<Option<Daemon>> = Mutex::new(None);
static CHILDREN: Mutex<[Option<Child>; 2]> = Mutex::new([None, None]);

fn state() -> std::sync::MutexGuard<'static, Option<Daemon>> {
    STATE.lock().unwrap_or_else(|p| p.into_inner())
}

fn hwnd() -> HWND {
    HWND(HWND_RAW.load(Ordering::Acquire) as *mut _)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---------------------------------------------------------------- public entry points

pub fn run() -> anyhow::Result<()> {
    // SAFETY: named mutex; handle intentionally kept open for the process lifetime.
    let _mutex = unsafe { CreateMutexW(None, true, MUTEX_NAME) }.context("CreateMutexW")?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        info!("another Jepret daemon is already running; exiting");
        return Ok(());
    }

    let config_path = paths::config_file();
    let (config, config_err) = match Config::load(&config_path) {
        Ok(c) => (c, None),
        Err(e) => (Config::default(), Some(format!("{e:#}"))),
    };
    let config = Arc::new(config);
    *state() = Some(Daemon {
        config: config.clone(),
        registered: Vec::new(),
        icon_idle: 0,
        icon_rec: 0,
    });

    let _ = dispatch::STATE_HOOK.set(|| {
        let h = hwnd();
        // SAFETY: posting to our own window from any thread is allowed.
        unsafe {
            let _ = PostMessageW(h, WM_REC_STATE, WPARAM(0), LPARAM(0));
        }
    });

    let window = create_window()?;
    HWND_RAW.store(window.0 as isize, Ordering::Release);
    // Icons depend on the window's DPI and the taskbar theme, so build them now.
    let (idle, rec) = make_tray_icons();
    if let Some(d) = state().as_mut() {
        d.icon_idle = idle;
        d.icon_rec = rec;
    }
    add_tray_icon(window, false);

    if let Some(e) = config_err {
        warn!("config load failed: {e}");
        notify_balloon("Jepret: config.toml tidak valid", &format!("Memakai pengaturan default. {e}"), true);
    }
    register_hotkeys(window);
    check_snipping_tool_conflict(window);
    apply_autostart(&config);

    // Startup cleanup on a background thread; never blocks the tray.
    if config.history.cleanup.run_on_startup {
        let cfg = config.clone();
        let _ = std::thread::Builder::new().name("jepret-cleanup".into()).spawn(move || {
            if let Err(e) = dispatch::run_cleanup(&cfg) {
                warn!("startup cleanup failed: {e:#}");
            }
        });
    }

    let watch = watch_config_dir();
    info!("daemon ready");
    message_loop(window, watch);

    if let Some(h) = watch {
        // SAFETY: handle came from FindFirstChangeNotificationW.
        let _ = unsafe { FindCloseChangeNotification(h) };
    }
    Ok(())
}

/// Forward `capture <kind>` to a running daemon, or run it in-process once.
pub fn send_or_run_capture(kind: &str) -> anyhow::Result<()> {
    let kind = command::parse_kind(kind)?;
    let cmd = Command::Capture(kind);

    // Is a daemon alive? (mutex exists)
    // SAFETY: opening/creating a named mutex; we release it immediately if we got it.
    let probe = unsafe { CreateMutexW(None, true, MUTEX_NAME) }.context("CreateMutexW")?;
    let daemon_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    // SAFETY: closing the handle we just created.
    let _ = unsafe { CloseHandle(probe) };

    if daemon_running {
        return forward(cmd);
    }

    // No daemon: run once in-process.
    if kind.is_recording() {
        anyhow::bail!("recording needs the Jepret daemon; start jepret.exe first");
    }
    let config = Config::load(&paths::config_file()).unwrap_or_default();
    let actions = command::actions_for(&config, kind);
    let ctx = Ctx { config: &config, notify };
    dispatch::handle(&ctx, kind, &actions)
}

fn forward(cmd: Command) -> anyhow::Result<()> {
    // The daemon may still be creating its window right after start-up.
    let mut target = HWND::default();
    for _ in 0..30 {
        // SAFETY: plain FindWindowW lookup.
        if let Ok(h) = unsafe { FindWindowW(CLASS_NAME, PCWSTR::null()) } {
            if !h.0.is_null() {
                target = h;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    anyhow::ensure!(!target.0.is_null(), "Jepret daemon window not found");

    let text = cmd.encode();
    let bytes = text.as_bytes();
    let cds = COPYDATASTRUCT {
        dwData: COPYDATA_MAGIC,
        cbData: bytes.len() as u32,
        lpData: bytes.as_ptr() as *mut _,
    };
    let mut result = 0usize;
    // SAFETY: `cds` and `bytes` outlive the synchronous SendMessageTimeout call.
    let r = unsafe {
        SendMessageTimeoutW(
            target,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&cds as *const _ as isize),
            SMTO_ABORTIFHUNG,
            5000,
            Some(&mut result),
        )
    };
    anyhow::ensure!(r.0 != 0, "daemon did not accept the command");
    Ok(())
}

// ---------------------------------------------------------------- notifications

/// User-visible notification from any thread. Balloon via the daemon tray
/// icon, or a message box when running one-shot without a daemon.
pub fn notify(title: &str, body: &str) {
    info!("notify: {title}: {body}");
    let h = hwnd();
    if h.0.is_null() {
        // SAFETY: modal message box in the one-shot CLI mode.
        unsafe {
            let _ = MessageBoxW(None, PCWSTR(wide(body).as_ptr()), PCWSTR(wide(title).as_ptr()), MB_OK | MB_ICONINFORMATION);
        }
        return;
    }
    let boxed = Box::into_raw(Box::new((title.to_string(), body.to_string())));
    // SAFETY: ownership of `boxed` moves to the window proc on success.
    unsafe {
        if PostMessageW(h, WM_USER_NOTIFY, WPARAM(0), LPARAM(boxed as isize)).is_err() {
            drop(Box::from_raw(boxed));
        }
    }
}

fn copy_utf16<const N: usize>(dst: &mut [u16; N], s: &str) {
    let mut n = 0;
    for u in s.encode_utf16().take(N - 1) {
        dst[n] = u;
        n += 1;
    }
    dst[n] = 0;
}

fn base_nid(window: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: 1,
        ..Default::default()
    }
}

fn notify_balloon(title: &str, body: &str, is_error: bool) {
    SNIP_HINT_ACTIVE.store(false, Ordering::Relaxed);
    let mut nid = base_nid(hwnd());
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = if is_error { NIIF_ERROR } else { NIIF_INFO };
    copy_utf16(&mut nid.szInfoTitle, title);
    copy_utf16(&mut nid.szInfo, body);
    // SAFETY: nid is fully initialised and valid for the call.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

fn add_tray_icon(window: HWND, recording: bool) {
    let icon = {
        let st = state();
        st.as_ref().map(|d| if recording { d.icon_rec } else { d.icon_idle }).unwrap_or(0)
    };
    let mut nid = base_nid(window);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = HICON(icon as *mut _);
    copy_utf16(&mut nid.szTip, "Jepret");
    // SAFETY: nid valid for the calls.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
    }
}

/// Replace only the tray tooltip (safe to call from any thread).
fn set_tray_tip(tip: &str) {
    let mut nid = base_nid(hwnd());
    nid.uFlags = NIF_TIP;
    copy_utf16(&mut nid.szTip, tip);
    // SAFETY: nid valid for the call; the shell serialises icon updates.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

fn update_tray(recording: bool) {
    let (icon, tip) = {
        let st = state();
        let icon = st.as_ref().map(|d| if recording { d.icon_rec } else { d.icon_idle }).unwrap_or(0);
        let tip = match dispatch::recording_elapsed() {
            Some(d) if recording => {
                let s = d.as_secs();
                format!("Jepret - merekam {:02}:{:02}", s / 60, s % 60)
            }
            _ => "Jepret".to_string(),
        };
        (icon, tip)
    };
    let mut nid = base_nid(hwnd());
    nid.uFlags = NIF_ICON | NIF_TIP;
    nid.hIcon = HICON(icon as *mut _);
    copy_utf16(&mut nid.szTip, &tip);
    // SAFETY: nid valid for the call.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// Tray icon edge in pixels for the DPI of the window the tray belongs to.
fn tray_icon_size() -> u32 {
    // SAFETY: plain metric queries; the window handle may be null (then system DPI).
    let px = unsafe {
        let h = hwnd();
        let dpi = if h.0.is_null() { GetDpiForSystem() } else { GetDpiForWindow(h) };
        GetSystemMetricsForDpi(SM_CXSMICON, if dpi == 0 { 96 } else { dpi })
    };
    if px > 0 {
        px as u32
    } else {
        16
    }
}

/// `SystemUsesLightTheme` (taskbar/tray theme); defaults to dark when unreadable.
fn taskbar_is_light() -> bool {
    let mut data = 0u32;
    let mut size = 4u32;
    // SAFETY: reads a DWORD into a properly sized local buffer.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    r.is_ok() && data == 1
}

/// Build an `HBITMAP` from straight RGBA (`premultiply` for menu bitmaps).
fn bitmap_from_rgba(rgba: &[u8], size: u32, premultiply: bool) -> Option<HBITMAP> {
    let bgra = raster::rgba_to_bgra(rgba, premultiply);
    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: size as i32,
        biHeight: -(size as i32), // top-down
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: CreateDIBSection returns a pointer to `size*size*4` bytes we own
    // for the bitmap's lifetime; we copy exactly that many bytes into it.
    unsafe {
        let bmp = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        if bits.is_null() || bgra.len() != (size * size * 4) as usize {
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            return None;
        }
        std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());
        Some(bmp)
    }
}

fn make_icon(recording: bool, size: u32, light: bool) -> isize {
    let px = raster::tray_icon_rgba(size, recording, light);
    let mask = vec![0u8; (size.div_ceil(16) * 2 * size) as usize];
    let Some(color) = bitmap_from_rgba(&px, size, false) else { return 0 };
    // SAFETY: the mask buffer outlives CreateBitmap (which copies it); both
    // bitmaps are deleted after CreateIconIndirect copied them.
    unsafe {
        let mask_bmp = CreateBitmap(size as i32, size as i32, 1, 1, Some(mask.as_ptr() as *const _));
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask_bmp, hbmColor: color };
        let icon = CreateIconIndirect(&info).map(|i| i.0 as isize).unwrap_or(0);
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask_bmp.0));
        icon
    }
}

fn make_tray_icons() -> (isize, isize) {
    let (size, light) = (tray_icon_size(), taskbar_is_light());
    (make_icon(false, size, light), make_icon(true, size, light))
}

/// Rebuild tray icons after a DPI / taskbar-theme change and push them to the shell.
fn refresh_tray_icons() {
    let (idle, rec) = make_tray_icons();
    if idle == 0 || rec == 0 {
        return;
    }
    let old = {
        let mut st = state();
        st.as_mut().map(|d| (std::mem::replace(&mut d.icon_idle, idle), std::mem::replace(&mut d.icon_rec, rec)))
    };
    update_tray(dispatch::is_recording());
    if let Some((a, b)) = old {
        // SAFETY: the shell already switched to the new icon handles above.
        unsafe {
            let _ = DestroyIcon(HICON(a as *mut _));
            let _ = DestroyIcon(HICON(b as *mut _));
        }
    }
}

// ---------------------------------------------------------------- window + loop

fn create_window() -> anyhow::Result<HWND> {
    // SAFETY: standard window-class registration and creation on this thread.
    unsafe {
        let inst = GetModuleHandleW(None).context("GetModuleHandleW")?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: inst.into(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            anyhow::bail!("RegisterClassW failed");
        }
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            CLASS_NAME,
            w!("Jepret"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            inst,
            None,
        )
        .context("CreateWindowExW")
    }
}

fn watch_config_dir() -> Option<HANDLE> {
    let dir = paths::config_dir();
    if let Err(e) = paths::ensure_dir(&dir) {
        warn!("cannot create config dir: {e:#}");
        return None;
    }
    let wdir = wide(&dir.to_string_lossy());
    // SAFETY: wdir is NUL-terminated and outlives the call.
    unsafe {
        FindFirstChangeNotificationW(
            PCWSTR(wdir.as_ptr()),
            false,
            FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_SIZE,
        )
    }
    .map_err(|e| warn!("config watch unavailable: {e}"))
    .ok()
}

fn message_loop(window: HWND, watch: Option<HANDLE>) {
    let handles: Vec<HANDLE> = watch.into_iter().collect();
    loop {
        // SAFETY: blocks until config-dir change or any window message — no polling.
        let r = unsafe { MsgWaitForMultipleObjects(Some(&handles), false, INFINITE, QS_ALLINPUT) };
        if r == WAIT_OBJECT_0 && !handles.is_empty() {
            // SAFETY: re-arm the change notification; debounce the reload.
            unsafe {
                let _ = FindNextChangeNotification(handles[0]);
                SetTimer(window, TIMER_RELOAD, 400, None);
            }
        }
        let mut msg = MSG::default();
        // SAFETY: standard message pump.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // The body is wrapped so that a bug can never unwind across the FFI boundary.
    let r = std::panic::catch_unwind(|| handle_message(h, msg, wp, lp));
    match r {
        Ok(Some(l)) => l,
        Ok(None) => DefWindowProcW(h, msg, wp, lp),
        Err(p) => {
            error!("panic in window proc: {}", dispatch::panic_message(&*p));
            DefWindowProcW(h, msg, wp, lp)
        }
    }
}

fn handle_message(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_HOTKEY => {
            on_hotkey(wp.0 as i32);
            Some(LRESULT(0))
        }
        WM_TRAY => {
            // NOTIFYICON_VERSION_4: LOWORD(lparam) is the event.
            let ev = (lp.0 as u32) & 0xFFFF;
            if ev == WM_CONTEXTMENU || ev == NIN_SELECT {
                show_menu(h);
            } else if ev == NIN_BALLOONUSERCLICK_EV && SNIP_HINT_ACTIVE.swap(false, Ordering::Relaxed) {
                // SAFETY: opens the system settings page; we never change the setting ourselves.
                unsafe {
                    ShellExecuteW(h, w!("open"), w!("ms-settings:easeofaccess-keyboard"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
                }
            }
            Some(LRESULT(0))
        }
        WM_COMMAND => {
            on_menu_command(h, wp.0 & 0xFFFF);
            Some(LRESULT(0))
        }
        WM_USER_NOTIFY => {
            // SAFETY: pointer produced by `notify` via Box::into_raw.
            let b = unsafe { Box::from_raw(lp.0 as *mut (String, String)) };
            notify_balloon(&b.0, &b.1, false);
            Some(LRESULT(0))
        }
        WM_REC_STATE => {
            let rec = dispatch::is_recording();
            update_tray(rec);
            // SAFETY: timer on our own window; only runs while recording.
            unsafe {
                if rec {
                    SetTimer(h, TIMER_REC, 1000, None);
                } else {
                    let _ = KillTimer(h, TIMER_REC);
                }
            }
            Some(LRESULT(0))
        }
        WM_CAPTURE_DONE => {
            // One-shot idle timer, re-armed after every capture (no polling).
            // SAFETY: timer on our own window.
            unsafe {
                SetTimer(h, TIMER_IDLE, IDLE_RELEASE_MS, None);
            }
            Some(LRESULT(0))
        }
        WM_TIMER => {
            match wp.0 {
                TIMER_IDLE => {
                    // SAFETY: our own timer.
                    unsafe {
                        let _ = KillTimer(h, TIMER_IDLE);
                    }
                    if BUSY.load(Ordering::Acquire) || dispatch::is_recording() {
                        // SAFETY: re-arm; try again after another idle period.
                        unsafe {
                            SetTimer(h, TIMER_IDLE, IDLE_RELEASE_MS, None);
                        }
                    } else {
                        release_capture_cache();
                    }
                }
                TIMER_RELOAD => {
                    // SAFETY: our own timer.
                    unsafe {
                        let _ = KillTimer(h, TIMER_RELOAD);
                    }
                    reload_config(h);
                }
                TIMER_REC => update_tray(dispatch::is_recording()),
                _ => {}
            }
            Some(LRESULT(0))
        }
        WM_COPYDATA => {
            // SAFETY: lparam points to a COPYDATASTRUCT valid for this call.
            let cds = unsafe { &*(lp.0 as *const COPYDATASTRUCT) };
            if cds.dwData != COPYDATA_MAGIC || cds.lpData.is_null() || cds.cbData > 4096 {
                return Some(LRESULT(0));
            }
            // SAFETY: the sender guarantees cbData readable bytes at lpData.
            let bytes = unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) };
            match std::str::from_utf8(bytes).map_err(anyhow::Error::from).and_then(Command::decode) {
                Ok(c) => {
                    run_command(h, c);
                    Some(LRESULT(1))
                }
                Err(e) => {
                    warn!("bad IPC command: {e:#}");
                    Some(LRESULT(0))
                }
            }
        }
        WM_DESTROY => {
            shutdown(h);
            // SAFETY: ends the message loop.
            unsafe { PostQuitMessage(0) };
            Some(LRESULT(0))
        }
        WM_DPICHANGED | WM_DISPLAYCHANGE => {
            refresh_tray_icons();
            None
        }
        WM_SETTINGCHANGE => {
            // "ImmersiveColorSet" = light/dark switch.
            // SAFETY: lParam is a NUL-terminated wide string or null per the message contract.
            let name = unsafe { PCWSTR(lp.0 as *const u16).to_string().unwrap_or_default() };
            if name == "ImmersiveColorSet" {
                refresh_tray_icons();
            }
            None
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            add_tray_icon(h, dispatch::is_recording());
            Some(LRESULT(0))
        }
        _ => None,
    }
}

/// Free cached D3D11 devices / duplications after ~60 s without a capture.
fn release_capture_cache() {
    debug!("idle: releasing capture cache");
    jepret_capture::release_cache();
}

fn shutdown(h: HWND) {
    if dispatch::stop_recording() {
        for _ in 0..100 {
            if !dispatch::is_recording() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    unregister_hotkeys(h);
    let nid = base_nid(h);
    // SAFETY: removing our own tray icon.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

// ---------------------------------------------------------------- hotkeys & commands

fn unregister_hotkeys(h: HWND) {
    let mut st = state();
    if let Some(d) = st.as_mut() {
        for id in d.registered.drain(..) {
            // SAFETY: ids were registered by us on this window.
            unsafe {
                let _ = UnregisterHotKey(h, id);
            }
        }
    }
}

fn register_hotkeys(h: HWND) {
    unregister_hotkeys(h);
    let mut st = state();
    let Some(d) = st.as_mut() else { return };
    let mut problems = Vec::new();
    for (i, b) in d.config.hotkeys.iter().enumerate() {
        match hotkey::parse(&b.keys) {
            Err(e) => problems.push(format!(
                "{}: kombinasi tidak valid ({e}). Ganti di Settings > Hotkey.",
                b.keys
            )),
            Ok(hk) => {
                let id = i as i32 + 1;
                // SAFETY: registering a hotkey on our own window.
                let r = unsafe { RegisterHotKey(h, id, HOT_KEY_MODIFIERS(hk.modifiers) | MOD_NOREPEAT, hk.vk) };
                match r {
                    Ok(()) => d.registered.push(id),
                    Err(e) => {
                        let pretty = jepret_core::KeyCombo::parse(&b.keys).map(|k| k.pretty()).unwrap_or_else(|_| b.keys.clone());
                        problems.push(format!(
                            "{pretty} gagal didaftarkan (mungkin dipakai aplikasi lain): {e}. Ganti kombinasinya di Settings > Hotkey."
                        ));
                    }
                }
            }
        }
    }
    drop(st);
    if !problems.is_empty() {
        for p in &problems {
            warn!("hotkey problem: {p}");
        }
        notify_balloon("Jepret: hotkey bermasalah", &problems.join("\n"), true);
    }
}

fn on_hotkey(id: i32) {
    let binding = {
        let st = state();
        st.as_ref().and_then(|d| d.config.hotkeys.get((id - 1) as usize).cloned())
    };
    if let Some(b) = binding {
        debug!(keys = %b.keys, "hotkey");
        let actions = if b.actions.is_empty() { None } else { Some(b.actions) };
        spawn_capture_with(b.capture, actions, b.delay_secs);
    }
}

fn run_command(h: HWND, c: Command) {
    match c {
        Command::Capture(kind) => spawn_capture_with(kind, None, 0),
        Command::Reload => reload_config(h),
    }
}

/// `WM_SPAWN` helper (kind index) — kept so menu commands can be posted too.
fn spawn_capture(kind_index: u32, actions: Option<Vec<Action>>) {
    if let Some((_, k)) = command::KIND_NAMES.get(kind_index as usize) {
        spawn_capture_with(*k, actions, 0);
    }
}

/// `delay_secs` (from `HotkeyBinding::delay_secs`) is slept on the capture
/// worker thread before acquiring; recording kinds ignore it.
fn spawn_capture_with(kind: CaptureKind, actions: Option<Vec<Action>>, delay_secs: u32) {
    // Pressing the record hotkey again while recording = stop.
    if kind.is_recording() && dispatch::stop_recording() {
        return;
    }
    if BUSY.swap(true, Ordering::AcqRel) {
        debug!("capture already in progress; ignoring {kind:?}");
        return;
    }
    let config = match state().as_ref() {
        Some(d) => d.config.clone(),
        None => {
            BUSY.store(false, Ordering::Release);
            return;
        }
    };
    let actions = actions.unwrap_or_else(|| command::actions_for(&config, kind));
    let delay = if kind.is_recording() { None } else { dispatch::delay_duration(delay_secs) };
    let spawned = std::thread::Builder::new().name("jepret-capture".into()).spawn(move || {
        if let Some(d) = delay {
            // Worker thread, never the message loop: a plain sleep is fine here.
            set_tray_tip(&format!("Jepret - capture dalam {} s", d.as_secs()));
            std::thread::sleep(d);
            set_tray_tip("Jepret");
        }
        let ctx = Ctx { config: &config, notify };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dispatch::handle(&ctx, kind, &actions)));
        match r {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                error!(?kind, "capture failed: {e:#}");
                notify("Jepret: capture gagal", &format!("{e:#}"));
            }
            Err(p) => {
                let m = dispatch::panic_message(&*p);
                error!(?kind, "capture panicked: {m}");
                notify("Jepret: capture gagal", &format!("Kesalahan internal: {m}"));
            }
        }
        BUSY.store(false, Ordering::Release);
        // SAFETY: posting to our own window; failure just skips the idle release.
        unsafe {
            let _ = PostMessageW(hwnd(), WM_CAPTURE_DONE, WPARAM(0), LPARAM(0));
        }
    });
    if let Err(e) = spawned {
        BUSY.store(false, Ordering::Release);
        error!("cannot spawn capture thread: {e}");
    }
}

fn reload_config(h: HWND) {
    let new = match Config::load(&paths::config_file()) {
        Ok(c) => c,
        Err(e) => {
            warn!("config reload failed: {e:#}");
            notify_balloon("Jepret: config.toml tidak valid", &format!("{e:#}"), true);
            return;
        }
    };
    {
        let mut st = state();
        let Some(d) = st.as_mut() else { return };
        if *d.config == new {
            return; // e.g. state.toml was written
        }
        d.config = Arc::new(new);
    }
    info!("config reloaded");
    register_hotkeys(h);
    if let Some(d) = state().as_ref() {
        apply_autostart(&d.config);
    }
}

// ---------------------------------------------------------------- menu

/// One tray-menu row.
enum Row {
    Sep,
    /// `(id, label, shortcut kind, icon)`
    Item(usize, &'static str, Option<CaptureKind>, MenuGlyph),
}

#[derive(Clone, Copy)]
enum MenuGlyph {
    Path(Icon),
    Stop,
}

fn kind_id(kind: CaptureKind) -> usize {
    ID_KIND_BASE + command::KIND_NAMES.iter().position(|(_, k)| *k == kind).unwrap_or(0)
}

/// Menu order of the design ("Tray" artboard); the record row flips to "Stop rekaman".
fn menu_rows(recording: bool) -> Vec<Row> {
    use CaptureKind as K;
    let rec = if recording {
        Row::Item(ID_STOP_REC, "Stop rekaman", Some(K::RecordRegion), MenuGlyph::Stop)
    } else {
        Row::Item(kind_id(K::RecordRegion), "Rekam region", Some(K::RecordRegion), MenuGlyph::Path(Icon::Record))
    };
    vec![
        Row::Item(kind_id(K::Region), "Region", Some(K::Region), MenuGlyph::Path(Icon::Frame)),
        Row::Item(kind_id(K::Fullscreen), "Fullscreen", Some(K::Fullscreen), MenuGlyph::Path(Icon::Monitor)),
        Row::Item(kind_id(K::ActiveWindow), "Window aktif", Some(K::ActiveWindow), MenuGlyph::Path(Icon::Window)),
        Row::Item(kind_id(K::LastRegion), "Last region", Some(K::LastRegion), MenuGlyph::Path(Icon::History)),
        Row::Item(kind_id(K::Scroll), "Scroll capture", Some(K::Scroll), MenuGlyph::Path(Icon::Scroll)),
        Row::Item(ID_OCR, "Ambil teks (OCR)", None, MenuGlyph::Path(Icon::Frame)),
        Row::Sep,
        rec,
        Row::Sep,
        Row::Item(ID_GALLERY, "Galeri", None, MenuGlyph::Path(Icon::Gallery)),
        Row::Item(ID_SETTINGS, "Settings", None, MenuGlyph::Path(Icon::Settings)),
        Row::Item(ID_OPEN_FOLDER, "Buka folder output", None, MenuGlyph::Path(Icon::Folder)),
        Row::Sep,
        Row::Item(ID_QUIT, "Keluar", None, MenuGlyph::Path(Icon::Exit)),
    ]
}

fn show_menu(h: HWND) {
    let recording = dispatch::is_recording();
    let cfg = state().as_ref().map(|d| d.config.clone());
    let icon_px = tray_icon_size();
    let mut bitmaps: Vec<HBITMAP> = Vec::new();
    // SAFETY: menu creation/tracking on the window's own thread; the bitmaps
    // are deleted only after the menu that references them is destroyed.
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        for row in menu_rows(recording) {
            let Row::Item(id, label, kind, glyph) = row else {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                continue;
            };
            let text = match kind.and_then(|k| cfg.as_deref().and_then(|c| shortcut_label(c, k))) {
                Some(sc) => format!("{label}\t{sc}"),
                None => label.to_string(),
            };
            let _ = AppendMenuW(menu, MF_STRING, id, PCWSTR(wide(&text).as_ptr()));
            let red = id == ID_STOP_REC || id == kind_id(CaptureKind::RecordRegion);
            let color = if red { [0xD9, 0x3A, 0x2B] } else { raster::TRAY_DARK_ON_LIGHT };
            let rgba = match glyph {
                MenuGlyph::Path(i) => raster::menu_icon_rgba(icon_px, i.path(), color),
                MenuGlyph::Stop => raster::stop_icon_rgba(icon_px, color),
            };
            if let Some(bmp) = bitmap_from_rgba(&rgba, icon_px, true) {
                let mii = MENUITEMINFOW {
                    cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_BITMAP,
                    hbmpItem: bmp,
                    ..Default::default()
                };
                let _ = SetMenuItemInfoW(menu, id as u32, false, &mii);
                bitmaps.push(bmp);
            }
        }

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(h);
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, 0, h, None);
        let _ = PostMessageW(h, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        for b in bitmaps {
            let _ = DeleteObject(HGDIOBJ(b.0));
        }
    }
}

/// Human-readable shortcut of the first binding for `kind` (e.g. `Ctrl+PrtSc`).
fn shortcut_label(config: &Config, kind: CaptureKind) -> Option<String> {
    config
        .hotkeys
        .iter()
        .find(|h| h.capture == kind)
        .and_then(|h| jepret_core::KeyCombo::parse(&h.keys).ok())
        .map(|k| k.pretty())
}

/// Windows binds PrtSc to Snipping Tool when this per-user DWORD is 1. On
/// Windows 11 the value is absent until the user touches the setting, and
/// the absent default is *on*.
fn print_screen_opens_snipping_tool() -> bool {
    let mut data = 0u32;
    let mut size = 4u32;
    // SAFETY: reads a DWORD into a properly sized local buffer.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Control Panel\Keyboard"),
            w!("PrintScreenKeyForSnippingEnabled"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    snipping_from_registry(r.is_ok().then_some(data), windows_build())
}

/// Pure decision: explicit value wins; absent = on for Windows 11 (build
/// 22000+), off for Windows 10.
fn snipping_from_registry(value: Option<u32>, build: u32) -> bool {
    match value {
        Some(v) => v == 1,
        None => build >= 22000,
    }
}

/// `CurrentBuildNumber` from the registry (0 if unreadable).
fn windows_build() -> u32 {
    let mut buf = [0u16; 16];
    let mut size = std::mem::size_of_val(&buf) as u32;
    // SAFETY: reads a short REG_SZ into a properly sized local UTF-16 buffer.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"),
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if r.is_err() {
        return 0;
    }
    let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n]).trim().parse().unwrap_or(0)
}

fn check_snipping_tool_conflict(_h: HWND) {
    let uses_prtsc = state().as_ref().is_some_and(|d| {
        d.config
            .hotkeys
            .iter()
            .any(|b| jepret_core::KeyCombo::parse(&b.keys).is_ok_and(|k| k.key == "printscreen"))
    });
    if uses_prtsc && print_screen_opens_snipping_tool() {
        notify_balloon(
            "Jepret: PrtSc dipakai Snipping Tool",
            "Windows membuka Snipping Tool saat PrtSc ditekan. Klik di sini untuk membuka pengaturan keyboard lalu matikan \"Gunakan tombol Print screen untuk membuka tangkapan layar\".",
            false,
        );
        SNIP_HINT_ACTIVE.store(true, Ordering::Relaxed);
    }
}

fn on_menu_command(h: HWND, id: usize) {
    match id {
        ID_GALLERY => spawn_child(0, "--gallery"),
        ID_SETTINGS => spawn_child(1, "--settings"),
        ID_OPEN_FOLDER => open_output_folder(h),
        ID_STOP_REC => {
            dispatch::stop_recording();
        }
        ID_OCR => spawn_capture_with(CaptureKind::Region, Some(vec![Action::Ocr]), 0),
        ID_QUIT => {
            // SAFETY: destroys our own window → WM_DESTROY → shutdown.
            unsafe {
                let _ = DestroyWindow(h);
            }
        }
        i if (ID_KIND_BASE..ID_KIND_BASE + command::KIND_NAMES.len()).contains(&i) => {
            spawn_capture((i - ID_KIND_BASE) as u32, None);
        }
        _ => {}
    }
}

fn spawn_child(slot: usize, arg: &str) {
    let mut ch = CHILDREN.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(c) = ch[slot].as_mut() {
        if matches!(c.try_wait(), Ok(None)) {
            return; // already open
        }
    }
    match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg(arg).spawn()) {
        Ok(c) => ch[slot] = Some(c),
        Err(e) => {
            drop(ch);
            notify("Jepret", &format!("Gagal membuka {arg}: {e}"));
        }
    }
}

fn open_output_folder(h: HWND) {
    let dir = state().as_ref().map(|d| d.config.general.output_dir.clone());
    let Some(dir) = dir else { return };
    if let Err(e) = paths::ensure_dir(&dir) {
        notify("Jepret", &format!("Folder output tidak bisa dibuat: {e}"));
        return;
    }
    let wd = wide(&dir.to_string_lossy());
    // SAFETY: wd is NUL-terminated and outlives the call.
    unsafe {
        ShellExecuteW(h, w!("open"), PCWSTR(wd.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

// ---------------------------------------------------------------- start with Windows

fn apply_autostart(config: &Config) {
    let want = config.general.start_with_windows;
    let Ok(exe) = std::env::current_exe() else { return };
    // SAFETY: registry calls with NUL-terminated buffers that outlive them.
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            0,
            KEY_SET_VALUE,
            &mut key,
        )
        .is_err()
        {
            return;
        }
        if want {
            let val = wide(&format!("\"{}\"", exe.display()));
            let bytes = std::slice::from_raw_parts(val.as_ptr() as *const u8, val.len() * 2);
            let _ = RegSetValueExW(key, w!("Jepret"), 0, REG_SZ, Some(bytes));
        } else {
            let _ = RegDeleteValueW(key, w!("Jepret"));
        }
        let _ = RegCloseKey(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snipping_default_depends_on_windows_version() {
        assert!(snipping_from_registry(None, 26200)); // Win11, never touched
        assert!(!snipping_from_registry(None, 19045)); // Win10
        assert!(!snipping_from_registry(Some(0), 26200)); // user turned it off
        assert!(snipping_from_registry(Some(1), 19045));
    }

    #[test]
    fn reads_a_real_build_number() {
        assert!(windows_build() >= 10240);
    }

    #[test]
    fn copy_utf16_truncates_and_terminates() {
        let mut buf = [1u16; 4];
        copy_utf16(&mut buf, "abcdef");
        assert_eq!(buf, [b'a' as u16, b'b' as u16, b'c' as u16, 0]);
    }
}
