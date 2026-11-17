//! DXGI Desktop Duplication grab of one monitor → CPU image.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use jepret_core::{Image, MonitorInfo, Result};
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

use crate::pixels::bgra_to_rgba;

/// How to read the monitor.
#[derive(Clone, Copy)]
pub(crate) enum Mode {
    /// 8-bit BGRA (DWM already converted HDR→SDR if the monitor is HDR).
    Sdr,
    /// FP16 scRGB, tone-mapped with this SDR white level (nits).
    Hdr { sdr_white_nits: f32 },
}

fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

fn find_output(device_name: &str) -> Result<(IDXGIAdapter1, IDXGIOutput)> {
    // SAFETY: COM enumeration; all out values are owned interface pointers.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut ai = 0;
        while let Ok(adapter) = factory.EnumAdapters1(ai) {
            let mut oi = 0;
            while let Ok(output) = adapter.EnumOutputs(oi) {
                let desc = output.GetDesc()?;
                if wide_to_string(&desc.DeviceName) == device_name {
                    return Ok((adapter, output));
                }
                oi += 1;
            }
            ai += 1;
        }
    }
    anyhow::bail!("DXGI output {device_name} not found")
}

/// Cached duplication session for one output. D3D11 device creation alone
/// costs 250 ms+ on hybrid-GPU laptops, so it is kept between calls.
struct Session {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    /// Last desktop image (GPU side), reused when the screen has not changed
    /// since the previous acquire (AcquireNextFrame would just time out).
    last: Option<ID3D11Texture2D>,
    staging: Option<ID3D11Texture2D>,
}

type Slot = Arc<Mutex<Option<Session>>>;
type Cache = Mutex<HashMap<(String, bool), Slot>>;

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Drop every cached device/duplication (frees GPU memory). The next capture
/// re-creates them.
pub(crate) fn release_cache() {
    if let Ok(mut c) = cache().lock() {
        c.clear();
    }
}

fn open_session(mon: &MonitorInfo, mode: Mode) -> Result<Session> {
    let (adapter, output) = find_output(&mon.id.0)?;
    // SAFETY: standard D3D11/DXGI COM usage; all pointers are live locals.
    unsafe {
        let mut device: Option<ID3D11Device> = None;
        let mut ctx: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            &adapter.cast::<IDXGIAdapter>()?,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut ctx),
        )?;
        let device = device.ok_or_else(|| anyhow::anyhow!("no D3D11 device"))?;
        let ctx = ctx.ok_or_else(|| anyhow::anyhow!("no D3D11 context"))?;
        let dup = match mode {
            Mode::Hdr { .. } => output
                .cast::<IDXGIOutput5>()?
                .DuplicateOutput1(&device, 0, &[DXGI_FORMAT_R16G16B16A16_FLOAT])?,
            Mode::Sdr => output.cast::<IDXGIOutput1>()?.DuplicateOutput(&device)?,
        };
        let dd = dup.GetDesc();
        anyhow::ensure!(
            matches!(
                dd.Rotation,
                DXGI_MODE_ROTATION_IDENTITY | DXGI_MODE_ROTATION_UNSPECIFIED
            ),
            "rotated output not supported by duplication path"
        );
        Ok(Session { device, ctx, dup, last: None, staging: None })
    }
}

/// Capture one monitor via Desktop Duplication.
pub(crate) fn grab(mon: &MonitorInfo, mode: Mode) -> Result<Image> {
    let t0 = Instant::now();
    let key = (mon.id.0.clone(), matches!(mode, Mode::Hdr { .. }));
    let slot: Slot = {
        let mut c = cache().lock().map_err(|_| anyhow::anyhow!("capture cache poisoned"))?;
        c.entry(key).or_default().clone()
    };
    let mut guard = slot.lock().map_err(|_| anyhow::anyhow!("capture session poisoned"))?;

    let mut last_err = None;
    for _ in 0..2 {
        let fresh = guard.is_none();
        if fresh {
            *guard = Some(open_session(mon, mode)?);
        }
        let t_open = t0.elapsed();
        let Some(sess) = guard.as_mut() else { anyhow::bail!("no session") };
        match grab_with(sess, mode) {
            Ok(img) => {
                tracing::debug!(
                    monitor = %mon.id.0,
                    fresh,
                    open_ms = t_open.as_secs_f32() * 1000.0,
                    total_ms = t0.elapsed().as_secs_f32() * 1000.0,
                    "dxgi grab"
                );
                return Ok(img);
            }
            Err(e) => {
                // Invalidate and retry once with a new session (access lost
                // after mode change / UAC desktop / sleep). A brand-new
                // session that fails is a real failure.
                *guard = None;
                if fresh {
                    return Err(e);
                }
                tracing::debug!("duplication session reset: {e:#}");
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("duplication failed")))
}

fn grab_with(sess: &mut Session, mode: Mode) -> Result<Image> {
    // SAFETY: COM calls on live objects owned by `sess`; mapped memory is only
    // read while mapped; an acquired frame is always released.
    unsafe {
        // Short wait when we already hold an image: a pending frame is normally
        // there; on a static screen this times out and the retained copy is used.
        let have_last = sess.last.is_some();
        let deadline = Instant::now() + Duration::from_millis(1000);
        loop {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            let wait = if have_last { 25 } else { 100 };
            match sess.dup.AcquireNextFrame(wait, &mut info, &mut res) {
                Ok(()) => {
                    let copied = copy_acquired(sess, &info, res);
                    let _ = sess.dup.ReleaseFrame();
                    if copied? || have_last {
                        break;
                    }
                }
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                    if have_last {
                        break;
                    }
                }
                Err(e) => return Err(e.into()),
            }
            anyhow::ensure!(Instant::now() < deadline, "timed out waiting for desktop frame");
        }

        let last = sess.last.as_ref().ok_or_else(|| anyhow::anyhow!("no desktop frame"))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        last.GetDesc(&mut desc);
        if sess.staging.is_none() {
            let mut sd = desc;
            sd.Usage = D3D11_USAGE_STAGING;
            sd.BindFlags = 0;
            sd.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            sd.MiscFlags = 0;
            let mut st: Option<ID3D11Texture2D> = None;
            sess.device.CreateTexture2D(&sd, None, Some(&mut st))?;
            sess.staging = st;
        }
        let staging = sess.staging.as_ref().ok_or_else(|| anyhow::anyhow!("no staging texture"))?;
        sess.ctx.CopyResource(staging, last);
        read_staging(&sess.ctx, staging, &desc, mode)
    }
}

/// Copy an acquired frame into the retained GPU texture. Returns whether the
/// frame carried a desktop image (cursor-only updates do not).
unsafe fn copy_acquired(
    sess: &mut Session,
    info: &DXGI_OUTDUPL_FRAME_INFO,
    res: Option<IDXGIResource>,
) -> Result<bool> {
    if info.LastPresentTime == 0 {
        return Ok(false);
    }
    let Some(r) = res else { return Ok(false) };
    let tex: ID3D11Texture2D = r.cast()?;
    if sess.last.is_none() {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut desc);
        desc.Usage = D3D11_USAGE_DEFAULT;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = 0;
        desc.MiscFlags = 0;
        let mut t: Option<ID3D11Texture2D> = None;
        sess.device.CreateTexture2D(&desc, None, Some(&mut t))?;
        sess.last = t;
    }
    match &sess.last {
        Some(last) => {
            sess.ctx.CopyResource(last, &tex);
            Ok(true)
        }
        None => Ok(false),
    }
}

unsafe fn read_staging(
    ctx: &ID3D11DeviceContext,
    staging: &ID3D11Texture2D,
    desc: &D3D11_TEXTURE2D_DESC,
    mode: Mode,
) -> Result<Image> {
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
    let (w, h) = (desc.Width, desc.Height);
    let pitch = mapped.RowPitch as usize;
    let img = match (mode, desc.Format) {
        (Mode::Hdr { sdr_white_nits }, DXGI_FORMAT_R16G16B16A16_FLOAT) => {
            // SAFETY: mapped region is RowPitch*h bytes, 8-byte aligned by D3D.
            let src = std::slice::from_raw_parts(mapped.pData as *const u16, pitch / 2 * h as usize);
            // Never unwind while mapped (T02's stub may still be `todo!()`).
            std::panic::catch_unwind(|| crate::hdr::tonemap_scrgb_f16(src, w, h, pitch / 2, sdr_white_nits))
                .map_err(|_| anyhow::anyhow!("HDR tone-map panicked"))
        }
        (_, DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB) => {
            // SAFETY: as above, RowPitch*h bytes are mapped.
            let src = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * h as usize);
            Ok(bgra_to_rgba(src, w, h, pitch))
        }
        (_, f) => Err(anyhow::anyhow!("unsupported duplication format {f:?}")),
    };
    ctx.Unmap(staging, 0);
    img
}
