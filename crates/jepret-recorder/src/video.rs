//! Windows.Graphics.Capture of one monitor, cropped on the GPU into a
//! "latest frame" texture; the pacing loop reads it back at a fixed fps.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use anyhow::{anyhow, Context, Result};
use jepret_core::Rect;
use windows::{
    core::{factory, IInspectable, Interface},
    Foundation::TypedEventHandler,
    Graphics::{
        Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession},
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::{BOOL, HMODULE, LPARAM, RECT, TRUE},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::{
                D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
                ID3D11Texture2D, D3D11_BOX, D3D11_CPU_ACCESS_READ,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
                D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
            },
            Dxgi::{
                Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
                IDXGIDevice,
            },
            Gdi::{EnumDisplayMonitors, HDC, HMONITOR},
        },
        System::WinRT::{
            Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
            Graphics::Capture::IGraphicsCaptureItemInterop,
        },
    },
};

/// Monitor rectangles in virtual-desktop pixels.
pub fn monitors() -> Result<Vec<(HMONITOR, Rect)>> {
    unsafe extern "system" fn cb(m: HMONITOR, _: HDC, r: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the &mut Vec we pass below; `r` is valid per API.
        let v = &mut *(data.0 as *mut Vec<(HMONITOR, Rect)>);
        let r = &*r;
        v.push((
            m,
            Rect::new(
                r.left,
                r.top,
                (r.right - r.left) as u32,
                (r.bottom - r.top) as u32,
            ),
        ));
        TRUE
    }
    let mut v: Vec<(HMONITOR, Rect)> = Vec::new();
    // SAFETY: callback only runs during this call.
    unsafe {
        let ok = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut _ as isize));
        if !ok.as_bool() {
            return Err(anyhow!("EnumDisplayMonitors failed"));
        }
    }
    Ok(v)
}

pub struct VideoCapture {
    _device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    latest: ID3D11Texture2D,
    staging: ID3D11Texture2D,
    have_frame: Arc<AtomicBool>,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    pub width: u32,
    pub height: u32,
}

impl VideoCapture {
    /// `crop` is relative to the monitor's top-left, even-sized.
    pub fn start(hmon: HMONITOR, crop: Rect, show_cursor: bool) -> Result<Self> {
        // SAFETY: standard D3D11/WGC setup; all out-pointers are valid locals.
        unsafe {
            let mut device = None;
            let mut ctx = None;
            D3D11CreateDevice(
                None::<&windows::Win32::Graphics::Dxgi::IDXGIAdapter>,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )
            .context("D3D11CreateDevice")?;
            let device: ID3D11Device = device.context("no D3D11 device")?;
            let ctx: ID3D11DeviceContext = ctx.context("no D3D11 context")?;
            // WGC callbacks and our pacing thread share the immediate context.
            if let Ok(mt) = ctx.cast::<ID3D11Multithread>() {
                let _ = mt.SetMultithreadProtected(true);
            }

            let dxgi: IDXGIDevice = device.cast()?;
            let d3d: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;

            let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
            let item: GraphicsCaptureItem =
                interop.CreateForMonitor(hmon).context("CreateForMonitor")?;

            let mut desc = D3D11_TEXTURE2D_DESC {
                Width: crop.width,
                Height: crop.height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                ..Default::default()
            };
            let mut latest = None;
            device.CreateTexture2D(&desc, None, Some(&mut latest))?;
            let latest = latest.context("latest texture")?;
            desc.Usage = D3D11_USAGE_STAGING;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            device.CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging = staging.context("staging texture")?;

            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                &d3d,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                item.Size()?,
            )?;

            let have_frame = Arc::new(AtomicBool::new(false));
            {
                let ctx = ctx.clone();
                let latest = latest.clone();
                let have = have_frame.clone();
                let bx = D3D11_BOX {
                    left: crop.x as u32,
                    top: crop.y as u32,
                    front: 0,
                    right: crop.x as u32 + crop.width,
                    bottom: crop.y as u32 + crop.height,
                    back: 1,
                };
                pool.FrameArrived(
                    &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(
                        move |p, _| {
                            let Some(p) = p.as_ref() else { return Ok(()) };
                            let frame = p.TryGetNextFrame()?;
                            let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
                            let tex: ID3D11Texture2D = access.GetInterface()?;
                            let mut d = D3D11_TEXTURE2D_DESC::default();
                            tex.GetDesc(&mut d);
                            // Skip frames smaller than the crop (resolution change).
                            if d.Width >= bx.right && d.Height >= bx.bottom {
                                ctx.CopySubresourceRegion(&latest, 0, 0, 0, 0, &tex, 0, Some(&bx));
                                have.store(true, Ordering::Release);
                            }
                            Ok(())
                        },
                    ),
                )?;
            }

            let session = pool.CreateCaptureSession(&item)?;
            let _ = session.SetIsCursorCaptureEnabled(show_cursor);
            // Win11: hide the yellow capture border (may be unsupported).
            let _ = session.SetIsBorderRequired(false);
            session.StartCapture()?;

            Ok(Self {
                _device: device,
                ctx,
                latest,
                staging,
                have_frame,
                pool,
                session,
                width: crop.width,
                height: crop.height,
            })
        }
    }

    /// Copy the latest frame into `out` as tightly-packed BGRA. Returns
    /// `false` while no frame has arrived yet.
    pub fn read_frame(&self, out: &mut Vec<u8>) -> Result<bool> {
        if !self.have_frame.load(Ordering::Acquire) {
            return Ok(false);
        }
        let row = self.width as usize * 4;
        out.resize(row * self.height as usize, 0);
        // SAFETY: Map gives a pointer valid for RowPitch*height bytes until Unmap.
        unsafe {
            self.ctx.CopyResource(&self.staging, &self.latest);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            let src = m.pData as *const u8;
            for y in 0..self.height as usize {
                std::ptr::copy_nonoverlapping(
                    src.add(y * m.RowPitch as usize),
                    out.as_mut_ptr().add(y * row),
                    row,
                );
            }
            self.ctx.Unmap(&self.staging, 0);
        }
        Ok(true)
    }
}

impl Drop for VideoCapture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
