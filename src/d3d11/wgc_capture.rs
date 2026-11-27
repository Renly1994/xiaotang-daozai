//! Minimal Windows Graphics Capture wrapper for the dao zai D3D11 spike.
//!
//! Uses polling (TryGetNextFrame) — no async event handler needed.
//! One D3D11 device drives both the swapchain and the capture frame pool, so
//! CopyResource stays on the GPU: no cross-API sharing, no CPU readback.

use windows::Win32::{
    Foundation::HWND,
    Graphics::{
        Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D},
        Dxgi::IDXGIDevice,
    },
};
use windows::core::Interface;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::factory;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct WgcCapture {
    pool: Direct3D11CaptureFramePool,
    _session: GraphicsCaptureSession, // keep alive; dropping stops capture
    width: u32,
    height: u32,
    diag_done: std::cell::Cell<bool>,
    size_checked: std::cell::Cell<bool>,
    size_warned: AtomicBool,
    /// 帧尺寸与 bg_tex 不一致（取走即清）。上层据此重建会话/纹理。
    size_mismatch: AtomicBool,
    /// 所抓显示器在虚拟桌面中的原点（物理像素），把窗口屏幕坐标换成纹理局部坐标用。
    capture_origin: (i32, i32),
}

impl WgcCapture {
    /// Capture the monitor that owns `hwnd` (not always the primary one).
    /// `device` should be the same device that drives the swapchain so
    /// CopyResource is a pure GPU-side operation.
    pub fn new(device: &ID3D11Device, hwnd: HWND) -> windows::core::Result<Self> {
        // WGC/WinRT requires a COM apartment. The app main thread may not have
        // one; initialize MTA (idempotent — RPC_E_CHANGED_MODE is harmless).
        let _ = unsafe { windows::Win32::System::WinRT::RoInitialize(
            windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
        ) };

        // WinRT needs an IDirect3DDevice wrapping our DXGI device (same GPU).
        let dxgi: IDXGIDevice = device.cast()?;
        let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }?;
        let d3d_device: IDirect3DDevice = inspectable.cast()?;

        // Create a capture item for the primary monitor via the interop factory.
        let interop: IGraphicsCaptureItemInterop =
            factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        // 跨屏拖动后岛体可能在副屏：必须抓窗口所在屏，否则玻璃采样坐标对不上。
        let monitor = unsafe { windows::Win32::Graphics::Gdi::MonitorFromWindow(
            hwnd, windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST,
        ) };
        use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFO};
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let origin = if unsafe { GetMonitorInfoW(monitor, &mut mi).as_bool() } {
            (mi.rcMonitor.left, mi.rcMonitor.top)
        } else { (0, 0) };
        let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(monitor)? };

        // Use item.Size() (not monitor rect) per Microsoft guidance.
        let size = item.Size()?;
        let width = size.Width.max(1) as u32;
        let height = size.Height.max(1) as u32;

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &d3d_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            SizeInt32 { Width: width as i32, Height: height as i32 },
        )?;
        let session = pool.CreateCaptureSession(&item)?;
        session.StartCapture()?;
        // Best-effort: hide the capture border (Windows 11 yellow frame).
        let _ = session.SetIsBorderRequired(false);

        Ok(Self { pool, _session: session, width, height,
                  diag_done: std::cell::Cell::new(false),
                  size_checked: std::cell::Cell::new(false),
                  size_warned: AtomicBool::new(false),
                  size_mismatch: AtomicBool::new(false),
                  capture_origin: origin })
    }

    pub fn size(&self) -> (u32, u32) { (self.width, self.height) }
    /// 所抓显示器在虚拟桌面中的原点（物理像素）。
    pub fn capture_origin(&self) -> (i32, i32) { self.capture_origin }

    /// 取走「帧尺寸与背景纹理不匹配」信号（取后清零，只报一次）。
    pub fn take_size_mismatch(&self) -> bool {
        self.size_mismatch.swap(false, Ordering::Relaxed)
    }

    /// Poll the newest frame and copy it into `dst` (GPU-side, format must be
    /// B8G8R8A8_UNORM). Returns true if a fresh frame was copied.
    pub fn grab_into(
        &self,
        ctx: &ID3D11DeviceContext,
        dst: &ID3D11Texture2D,
    ) -> bool {
        loop {
            let frame = match self.pool.TryGetNextFrame() {
                Ok(f) => f,
                Err(e) => {
                    if !self.diag_done.get() {
                        self.diag_done.set(true);
                        eprintln!("[wgc] TryGetNextFrame error first time: {:?}", e);
                    }
                    return false; // no new frame
                }
            };
            if !self.diag_done.get() {
                self.diag_done.set(true);
                eprintln!("[wgc] got first frame OK");
            }
            let surface = match frame.Surface() {
                Ok(s) => s,
                Err(e) => { if !self.diag_done.get() { self.diag_done.set(true); eprintln!("[wgc] Surface() failed: {:?}", e); } return false; }
            };
            // IDirect3DSurface is NOT directly castable to ID3D11Texture2D;
            // must go through IDirect3DDxgiInterfaceAccess::GetInterface().
            let access: IDirect3DDxgiInterfaceAccess = match surface.cast() {
                Ok(a) => a,
                Err(e) => { if !self.diag_done.get() { self.diag_done.set(true); eprintln!("[wgc] cast access failed: {:?}", e); } return false; }
            };
            let src: ID3D11Texture2D = match unsafe { access.GetInterface() } {
                Ok(t) => t,
                Err(e) => { if !self.diag_done.get() { self.diag_done.set(true); eprintln!("[wgc] GetInterface failed: {:?}", e); } return false; }
            };
            // 每帧校验 src 尺寸与 dst（bg_tex）是否一致。D3D11 CopyResource 在
            // 尺寸不匹配时**静默失败**（不报错也不拷贝），则会一直拿旧纹理画 =
            // 玻璃错乱/拉伸。社区 streaming 项目（Vibepollo 等）都是每帧校验
            // 描述符、不匹配就 reinit，绝不带着错位尺寸继续跑。
            use windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC;
            let mut sd = unsafe { std::mem::zeroed::<D3D11_TEXTURE2D_DESC>() };
            let mut dd = unsafe { std::mem::zeroed::<D3D11_TEXTURE2D_DESC>() };
            unsafe { src.GetDesc(&mut sd); dst.GetDesc(&mut dd); }
            if !self.size_checked.get() {
                self.size_checked.set(true);
                eprintln!("[wgc] first frame src {}x{} dst {}x{}", sd.Width, sd.Height, dd.Width, dd.Height);
            }
            if sd.Width != dd.Width || sd.Height != dd.Height {
                self.size_mismatch.store(true, Ordering::Relaxed);
                if !self.size_warned.swap(true, Ordering::Relaxed) {
                    let m = format!("frame size {}x{} != bg {}x{}, signal rebuild",
                                    sd.Width, sd.Height, dd.Width, dd.Height);
                    eprintln!("[wgc] {}", m);
                    crate::wgc::log(&format!("wgc: {}", m));
                }
                return false; // 上层据 size_mismatch() 触发重建
            }
            self.size_warned.store(false, Ordering::Relaxed);
            unsafe { ctx.CopyResource(dst, &src) };
            return true;
        }
    }
}
