//! Premultiplied DirectComposition presentation from the currently bound WGL context.
use eframe::glow::{self, HasContext};
use std::{
    ffi::{c_void, CStr, CString},
    rc::Rc,
    sync::Arc,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
type Resolver<'a> = dyn Fn(&CStr) -> *const c_void + 'a;
use windows::{
    core::Interface,
    Foundation::Numerics::Matrix3x2,
    Win32::{
        Foundation::HWND,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::*,
            DirectComposition::*,
            Dxgi::{Common::*, *},
        },
    },
};

// Some older WGL implementations use these reserved sentinel addresses for missing functions.
fn valid_wgl_proc(p: *const c_void) -> bool {
    !matches!(p as isize, -1..=3)
}

type Handle = *mut c_void;
type Open = unsafe extern "system" fn(Handle) -> Handle;
type Close = unsafe extern "system" fn(Handle) -> i32;
type Register = unsafe extern "system" fn(Handle, Handle, u32, u32, u32) -> Handle;
type Unregister = unsafe extern "system" fn(Handle, Handle) -> i32;
type Access = unsafe extern "system" fn(Handle, i32, *const Handle) -> i32;

struct Bridge {
    handle: Handle,
    close: Close,
    register: Register,
    unregister: Unregister,
    lock: Access,
    unlock: Access,
}

impl Bridge {
    unsafe fn open(resolve: &Resolver<'_>, device: &ID3D11Device, dc: Handle) -> Result<Self> {
        macro_rules! proc_address {
            ($name:literal, $ty:ty) => {{
                let addr = resolve(CString::new($name)?.as_c_str());
                if !valid_wgl_proc(addr) {
                    return Err(concat!("WGL entry point missing: ", $name).into());
                }
                std::mem::transmute::<*const c_void, $ty>(addr)
            }};
        }
        let get_extensions = proc_address!(
            "wglGetExtensionsStringARB",
            unsafe extern "system" fn(Handle) -> *const i8
        );
        let list = get_extensions(dc);
        if list.is_null()
            || !CStr::from_ptr(list)
                .to_bytes()
                .split(|&b| b == b' ')
                .any(|s| s == b"WGL_NV_DX_interop2")
        {
            return Err("WGL_NV_DX_interop2 unavailable on this adapter".into());
        }
        let open = proc_address!("wglDXOpenDeviceNV", Open);
        let close = proc_address!("wglDXCloseDeviceNV", Close);
        let register = proc_address!("wglDXRegisterObjectNV", Register);
        let unregister = proc_address!("wglDXUnregisterObjectNV", Unregister);
        let lock = proc_address!("wglDXLockObjectsNV", Access);
        let unlock = proc_address!("wglDXUnlockObjectsNV", Access);
        let handle = open(device.as_raw());
        if handle.is_null() {
            return Err(format!(
                "WGL could not share the D3D11 device: {}",
                std::io::Error::last_os_error()
            )
            .into());
        }
        Ok(Self {
            handle,
            close,
            register,
            unregister,
            lock,
            unlock,
        })
    }
}

// On a hybrid machine WGL need not use DXGI adapter zero. Ask the WGL driver
// to open the actual D3D11 device on each hardware adapter before choosing one.
unsafe fn share_device(
    resolve: &Resolver<'_>,
    dc: Handle,
) -> Result<(ID3D11Device, ID3D11DeviceContext, Rc<Bridge>)> {
    let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
    let mut failures = Vec::new();
    for index in 0.. {
        let adapter = match factory.EnumAdapters1(index) {
            Ok(a) => a,
            Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(e) => return Err(e.into()),
        };
        let info = adapter.GetDesc1()?;
        if info.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let attempt = || -> Result<_> {
            let mut device = None;
            let mut immediate = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut immediate),
            )?;
            let device = device.ok_or("missing D3D11 device")?;
            let bridge = Rc::new(Bridge::open(resolve, &device, dc)?);
            Ok((device, immediate.ok_or("missing D3D11 context")?, bridge))
        };
        match attempt() {
            Ok(shared) => return Ok(shared),
            Err(e) => failures.push(format!(
                "{}: {e}",
                String::from_utf16_lossy(&info.Description).trim_end_matches('\0')
            )),
        }
    }
    Err(format!(
        "no shareable WGL/D3D11 hardware adapter ({})",
        failures.join(", ")
    )
    .into())
}

impl Drop for Bridge {
    fn drop(&mut self) {
        unsafe {
            (self.close)(self.handle);
        }
    }
}

struct Target {
    gl: Arc<glow::Context>,
    bridge: Rc<Bridge>,
    fbo: glow::Framebuffer,
    rb: glow::Renderbuffer,
    registration: Handle,
    shared: ID3D11Texture2D,
    capacity: [u32; 2],
}

impl Target {
    unsafe fn new(
        device: &ID3D11Device,
        gl: Arc<glow::Context>,
        bridge: Rc<Bridge>,
        size: [u32; 2],
    ) -> Result<Self> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: size[0],
            Height: size[1],
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            ..Default::default()
        };
        let mut shared = None;
        device.CreateTexture2D(&desc, None, Some(&mut shared))?;
        let shared = shared.ok_or("missing interop texture")?;
        let rb = gl.create_renderbuffer()?;
        let fbo = match gl.create_framebuffer() {
            Ok(fbo) => fbo,
            Err(error) => {
                gl.delete_renderbuffer(rb);
                return Err(error.into());
            }
        };
        let registration = (bridge.register)(
            bridge.handle,
            shared.as_raw(),
            rb.0.get(),
            glow::RENDERBUFFER,
            1,
        );
        if registration.is_null() {
            let error = std::io::Error::last_os_error();
            gl.delete_framebuffer(fbo);
            gl.delete_renderbuffer(rb);
            return Err(format!("wglDXRegisterObjectNV: {error}").into());
        }
        let target = Self {
            gl,
            bridge,
            fbo,
            rb,
            registration,
            shared,
            capacity: size,
        };
        target.with_gl(|| {
            target.gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::RENDERBUFFER,
                Some(rb),
            );
            let status = target.gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if status != glow::FRAMEBUFFER_COMPLETE {
                return Err(format!("incomplete shared FBO: {status:#x}").into());
            }
            Ok(())
        })?;
        Ok(target)
    }

    unsafe fn with_gl(&self, draw: impl FnOnce() -> Result<()>) -> Result<()> {
        if (self.bridge.lock)(self.bridge.handle, 1, &self.registration) == 0 {
            return Err(format!("WGL lock failed: {}", std::io::Error::last_os_error()).into());
        }
        self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
        let result = draw();
        self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        if (self.bridge.unlock)(self.bridge.handle, 1, &self.registration) == 0 {
            return Err(format!("WGL unlock failed: {}", std::io::Error::last_os_error()).into());
        }
        result
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_framebuffer(self.fbo);
            if (self.bridge.unregister)(self.bridge.handle, self.registration) == 0 {
                eprintln!(
                    "wglDXUnregisterObjectNV: {}",
                    std::io::Error::last_os_error()
                );
            }
            self.gl.delete_renderbuffer(self.rb);
        }
    }
}

pub struct Presenter {
    // Drop shared GL resources before their device, current context and hidden window.
    target: Target,
    buffer: Option<ID3D11Texture2D>,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    swap: IDXGISwapChain1,
    visual: IDCompositionVisual,
    _tree: IDCompositionTarget,
    comp: IDCompositionDevice,
    size: [u32; 2],
}

impl Presenter {
    pub unsafe fn new(
        hwnd: HWND,
        gl: Arc<glow::Context>,
        resolve: &Resolver<'_>,
        size: [u32; 2],
    ) -> Result<Self> {
        let get_dc_addr = resolve(c"wglGetCurrentDC");
        if !valid_wgl_proc(get_dc_addr) {
            return Err("wglGetCurrentDC unavailable".into());
        }
        let get_dc: unsafe extern "system" fn() -> Handle = std::mem::transmute(get_dc_addr);
        let dc = get_dc();
        if dc.is_null() {
            return Err("OpenGL is not using a current WGL drawable".into());
        }
        let (device, immediate, bridge) = share_device(resolve, dc)?;
        let dx: IDXGIDevice = device.cast()?;
        let adapter = dx.GetAdapter()?;

        let factory: IDXGIFactory2 = adapter.GetParent()?;
        let spec = DXGI_SWAP_CHAIN_DESC1 {
            Width: size[0],
            Height: size[1],
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            ..Default::default()
        };
        let swap = factory.CreateSwapChainForComposition(&device, &spec, None)?;
        let comp: IDCompositionDevice = DCompositionCreateDevice(&dx)?;
        let tree = comp.CreateTargetForHwnd(hwnd, true)?;
        let visual = comp.CreateVisual()?;
        visual.SetContent(&swap)?;
        tree.SetRoot(&visual)?;
        let target = Target::new(&device, gl.clone(), bridge, size)?;
        let mut p = Self {
            target,
            buffer: None,
            device,
            context: immediate,
            swap,
            visual,
            _tree: tree,
            comp,
            size,
        };
        p.attach(size)?;
        Ok(p)
    }

    unsafe fn attach(&mut self, size: [u32; 2]) -> Result<()> {
        self.buffer = Some(self.swap.GetBuffer(0)?);
        // OpenGL's first texel row is the bottom of the island; D3D's is the top.
        self.visual.SetTransform2(&Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: -1.0,
            M31: 0.0,
            M32: size[1] as f32,
        })?;
        self.comp.Commit()?;
        self.size = size;
        Ok(())
    }

    pub fn adapter(&self) -> windows::core::Result<IDXGIAdapter> {
        unsafe { self.device.cast::<IDXGIDevice>()?.GetAdapter() }
    }

    pub unsafe fn resize(&mut self, size: [u32; 2]) -> Result<()> {
        if self.size == size {
            return Ok(());
        }
        if size[0] > self.target.capacity[0] || size[1] > self.target.capacity[1] {
            // Keep a high-water allocation across monitor, scale and dock changes.
            let limit = self
                .target
                .gl
                .get_parameter_i32(glow::MAX_RENDERBUFFER_SIZE) as u32;
            if size.iter().any(|&n| n > limit) {
                return Err("frame exceeds the GL renderbuffer limit".into());
            }
            let capacity =
                [0, 1].map(|i| self.target.capacity[i].max(size[i].next_power_of_two().min(limit)));
            self.target = Target::new(
                &self.device,
                self.target.gl.clone(),
                self.target.bridge.clone(),
                capacity,
            )?;
        }
        self.buffer.take();
        self.swap.ResizeBuffers(
            0,
            size[0],
            size[1],
            DXGI_FORMAT_UNKNOWN,
            DXGI_SWAP_CHAIN_FLAG(0),
        )?;
        self.attach(size)
    }

    pub unsafe fn present(&mut self, size: [u32; 2], draw: impl FnOnce()) -> Result<()> {
        // Minimized windows can still receive texture updates for other viewports.
        if size.contains(&0) {
            return self.target.with_gl(|| {
                draw();
                Ok(())
            });
        }
        self.resize(size)?;
        let buffer = self.buffer.as_ref().ok_or("no swap-chain buffer")?;
        self.target.with_gl(|| {
            draw();
            Ok(())
        })?;
        let frame = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: self.size[0],
            bottom: self.size[1],
            back: 1,
        };
        self.context.CopySubresourceRegion(
            buffer,
            0,
            0,
            0,
            0,
            &self.target.shared,
            0,
            Some(&frame),
        );
        self.swap.Present(1, DXGI_PRESENT(0)).ok()?;
        Ok(())
    }
}

/// The visual covers the full canvas. Only its winit input sibling gets a hit region.
pub struct IslandSurface {
    presenter: Presenter,
    window: VisualWindow,
    input: HWND,
}
struct VisualWindow(HWND);
impl Drop for VisualWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0);
        }
    }
}
impl IslandSurface {
    pub unsafe fn new(
        input: HWND,
        gl: Arc<glow::Context>,
        resolve: &Resolver<'_>,
        size: [u32; 2],
    ) -> Result<Self> {
        let window = VisualWindow(crate::composition::create_visual_window(
            windows::core::w!("DaozaiAlphaSurface"),
            true,
        )?);
        let presenter = Presenter::new(window.0, gl, resolve, size)?;
        Ok(Self {
            presenter,
            window,
            input,
        })
    }

    unsafe fn follow_input(&self) -> windows::core::Result<()> {
        use windows::Win32::{
            Foundation::RECT,
            Graphics::Gdi::{GetWindowRgnBox, NULLREGION},
            UI::WindowsAndMessaging::*,
        };
        let mut area = RECT::default();
        let shown = IsWindowVisible(self.input).as_bool()
            && GetWindowRgnBox(self.input, &mut area) != NULLREGION;
        let visual = self.window.0;
        let visible = IsWindowVisible(visual).as_bool();
        if !shown {
            if visible {
                let _ = ShowWindow(visual, SW_HIDE);
            }
            return Ok(());
        }
        let mut desired = RECT::default();
        GetWindowRect(self.input, &mut desired)?;
        let mut current = RECT::default();
        GetWindowRect(visual, &mut current)?;
        let above = GetWindow(self.input, GW_HWNDPREV).ok();
        if !visible || current != desired || above != Some(visual) {
            let mut flags = SWP_NOACTIVATE | SWP_SHOWWINDOW;
            if above == Some(visual) {
                flags |= SWP_NOZORDER;
            }
            SetWindowPos(
                visual,
                above.unwrap_or(HWND_TOPMOST),
                desired.left,
                desired.top,
                desired.right - desired.left,
                desired.bottom - desired.top,
                flags,
            )?;
        }
        Ok(())
    }
}
impl eframe::RootGlowPresenter for IslandSurface {
    fn present(
        &mut self,
        size: [u32; 2],
        draw: &mut dyn FnMut(),
    ) -> std::result::Result<(), String> {
        unsafe {
            self.presenter
                .present(size, draw)
                .map_err(|e| e.to_string())?;
            self.follow_input().map_err(|e| e.to_string())
        }
    }
}
