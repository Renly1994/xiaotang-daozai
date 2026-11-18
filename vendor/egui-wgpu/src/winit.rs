#![allow(clippy::missing_errors_doc)]
#![allow(clippy::undocumented_unsafe_blocks)]

use crate::capture::{capture_channel, CaptureReceiver, CaptureSender, CaptureState};
use crate::{renderer, RenderState, SurfaceErrorAction, WgpuConfiguration};
use egui::{Context, Event, UserData, ViewportId, ViewportIdMap, ViewportIdSet};
use std::{num::NonZeroU32, sync::Arc};

struct SurfaceState {
    surface: wgpu::Surface<'static>,
    alpha_mode: wgpu::CompositeAlphaMode,
    width: u32,
    height: u32,
    /// 这个 surface 是否挂在 DirectComposition visual 上。
    /// 必须是逐 surface 的：`DCOMP_ACTIVE` 是全局的，拿它判断会把设置窗口的
    /// 普通 HWND 交换链也配成预乘 alpha，DXGI 直接拒绝 → `Surface::configure` 报致命错并崩溃。
    dcomp: bool,
}

/// 岛仔定制：DirectComposition 的 device / visual / target 必须比 surface 活得久。
#[cfg(windows)]
struct DcompKeepAlive {
    /// 这套 visual 挂在哪个窗口上。surface 重建时要复用同一套，
    /// 因为一个 HWND 只能有一个 DComp target。
    hwnd: isize,
    _device: windows::Win32::Graphics::DirectComposition::IDCompositionDevice,
    _visual: windows::Win32::Graphics::DirectComposition::IDCompositionVisual,
    _target: windows::Win32::Graphics::DirectComposition::IDCompositionTarget,
}

#[cfg(windows)]
thread_local! {
    static DCOMP_KEEPALIVE: std::cell::RefCell<Option<DcompKeepAlive>> =
        const { std::cell::RefCell::new(None) };
}

/// 跨线程可见的「当前是否在用 DComp」。native 的窗口更新和 surface 创建不一定同线程，
/// 所以不能用 thread_local 判断，否则会继续套 SetWindowRgn，把经典皮肤切出锯齿。
static DCOMP_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 认领要跑 DComp 的那个窗口（存 HWND，0 = 未认领）。
/// 不能用「首个 surface」的一次性布尔：窗口重建时 surface 会被重创，
/// 一次性标志已经为 true，就会掉回 HWND 交换链，而窗口又没有 GDI 表面 → 整窗空白。
/// 按 HWND 认领可以覆盖重建，也不会把设置窗口误认成主窗口。
#[cfg(windows)]
static DCOMP_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
/// DComp surface 真的建失败过。之后所有窗口一律走 HWND 交换链，不再反复试。
#[cfg(windows)]
static DCOMP_FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// surface 创建阶段失败后调用：放弃 DComp，让上层回到「轮廓裁剪 + 普通交换链」。
/// 不能只清 `DCOMP_HWND`——那样下一个窗口会重新认领、再失败一次。
#[cfg(windows)]
fn mark_dcomp_failed() {
    DCOMP_FAILED.store(true, std::sync::atomic::Ordering::SeqCst);
    DCOMP_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// 判断这个窗口是否该走 DComp（顺带完成认领）。
#[cfg(windows)]
fn claim_dcomp_window(window: &winit::window::Window) -> bool {
    use std::sync::atomic::Ordering::SeqCst;
    use winit::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

    if std::env::var_os("DAOZAI_DCOMP").is_none()
        || DCOMP_FAILED.load(std::sync::atomic::Ordering::Relaxed)
    {
        return false;
    }
    let Ok(handle) = window.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return false;
    };
    let hwnd = win32.hwnd.get();
    match DCOMP_HWND.compare_exchange(0, hwnd, SeqCst, SeqCst) {
        Ok(_) => true,
        Err(claimed) => claimed == hwnd,
    }
}

/// 运行期兜底第一层：**建窗口之前**先探一次 DirectComposition 能不能用。
///
/// 为什么必须在建窗口前探：DComp 要求窗口没有 GDI 重定向表面（`WS_EX_NOREDIRECTIONBITMAP`），
/// 而这个样式只能在 `CreateWindowEx` 时给，建完之后无法补加。所以一旦“窗口已经去掉表面、
/// 之后才发现 DComp 不可用”，就只能整进程重开（退到 Vulkan 或 OpenGL）——那是以前的行为。
/// 先探一次，探不通就**别**去掉表面，直接走老 HWND 交换链，进程都不用重开。
#[cfg(windows)]
pub fn direct_composition_available() -> bool {
    use std::sync::atomic::{AtomicU8, Ordering};
    static CACHED: AtomicU8 = AtomicU8::new(0); // 0=未测 1=可用 2=不可用
    match CACHED.load(Ordering::Relaxed) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    let ok = probe_direct_composition();
    if !ok {
        log::warn!("DirectComposition 不可用，保留 GDI 表面并走普通 HWND 交换链");
    }
    CACHED.store(if ok { 1 } else { 2 }, Ordering::Relaxed);
    ok
}

#[cfg(not(windows))]
pub fn direct_composition_available() -> bool {
    false
}

/// 只建 D3D11 设备 + DComp device + visual，不碰窗口。
/// 走的是和真正建 surface 完全相同的入口（D3D11 的 IDXGIDevice → DCompositionCreateDevice2），
/// 所以这里能过、那边基本也能过；反过来这里失败就说明这台机器确实不该走 DComp。
#[cfg(windows)]
fn probe_direct_composition() -> bool {
    use windows::core::Interface as _;
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, D3D11_SDK_VERSION,
    };
    use windows::Win32::Graphics::DirectComposition::{
        DCompositionCreateDevice2, IDCompositionDevice,
    };
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;

    for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        let mut device: Option<ID3D11Device> = None;
        let created = unsafe {
            D3D11CreateDevice(
                None,
                driver,
                None,
                Default::default(),
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
        };
        let Some(device) = device.filter(|_| created.is_ok()) else {
            continue;
        };
        let Ok(dxgi_device) = device.cast::<IDXGIDevice>() else {
            continue;
        };
        let dcomp: IDCompositionDevice = match unsafe { DCompositionCreateDevice2(&dxgi_device) } {
            Ok(dcomp) => dcomp,
            Err(_) => continue,
        };
        if unsafe { dcomp.CreateVisual() }.is_ok() {
            return true;
        }
    }
    false
}

/// 当前窗口是否已经在用 DirectComposition 透明交换链。
/// 用了它就不再需要 `SetWindowRgn` 硬裁轮廓——那会把非液态皮肤的圆角切出锯齿。
pub fn direct_composition_active() -> bool {
    #[cfg(windows)]
    {
        DCOMP_ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// wgpu-hal 只对 visual 调 SetContent，不负责 Commit；surface configure 之后必须补一次，
/// 否则 DComp 树不提交，窗口就是空的。
pub fn commit_direct_composition() {
    #[cfg(windows)]
    {
        DCOMP_KEEPALIVE.with(|slot| {
            if let Some(keep) = slot.borrow().as_ref() {
                let _ = unsafe { keep._device.Commit() };
            }
        });
    }
}

/// 用 wgpu 设备自己的 ID3D12Device 创建 DirectComposition visual，
/// 再把该 visual 交给 wgpu 建 surface。不要用独立 D3D 设备，否则 SetContent 会失败。
#[cfg(windows)]
fn create_dcomp_surface(
    instance: &wgpu::Instance,
    window: &winit::window::Window,
    adapter: &wgpu::Adapter,
) -> Result<wgpu::Surface<'static>, crate::WgpuError> {
    use windows::core::Interface as _;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, D3D11_SDK_VERSION,
    };
    use windows::Win32::Graphics::DirectComposition::{
        DCompositionCreateDevice2, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
    };
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    use winit::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

    let handle = window.window_handle()?.as_raw();
    let RawWindowHandle::Win32(win32) = handle else {
        return Err(crate::WgpuError::DirectComposition("不是 Win32 窗口".into()));
    };
    let hwnd = HWND(win32.hwnd.get() as *mut _);

    let hwnd_key = win32.hwnd.get();

    // 同一个 HWND 只能有一个 DComp target：第二次 CreateTargetForHwnd 会报
    // DCOMPOSITION_ERROR_WINDOW_ALREADY_COMPOSED。窗口重建 / 换缩放 / 切屏都会重建 surface，
    // 所以这里必须复用第一次建好的 visual，而不是重来一遍——
    // 否则整条透明链路会掉进兜底、退回 SetWindowRgn 硬裁（圆角又变锯齿）。
    let existing = DCOMP_KEEPALIVE.with(|slot| {
        let slot = slot.borrow();
        slot.as_ref()
            .filter(|keep| keep.hwnd == hwnd_key)
            .map(|keep| keep._visual.as_raw())
    });

    let visual_ptr = match existing {
        Some(ptr) => ptr,
        None => {
            // AMD 的 ID3D12Device 不能 QI 到 IDXGIDevice；用同一个 DXGI adapter 建 D3D11 设备，
            // 再从 D3D11 设备拿 IDXGIDevice 给 DComp。visual 仍由 wgpu 的 D3D12 交换链 SetContent。
            let dxgi_adapter = unsafe {
                adapter.as_hal::<wgpu::hal::api::Dx12, _, _>(|hal| {
                    hal.map(|a| a.raw_dxgi_adapter().clone())
                })
            }
            .ok_or_else(|| crate::WgpuError::DirectComposition("当前适配器不是 DX12".into()))?;

            let mut d3d11: Option<ID3D11Device> = None;
            unsafe {
                D3D11CreateDevice(
                    &dxgi_adapter,
                    D3D_DRIVER_TYPE_UNKNOWN,
                    None,
                    Default::default(),
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut d3d11),
                    None,
                    None,
                )
            }
            .map_err(|e| crate::WgpuError::DirectComposition(format!("D3D11CreateDevice: {e}")))?;
            let d3d11 = d3d11.ok_or_else(|| {
                crate::WgpuError::DirectComposition("D3D11CreateDevice 没返回设备".into())
            })?;
            let dxgi_device: IDXGIDevice = d3d11.cast().map_err(|e| {
                crate::WgpuError::DirectComposition(format!("ID3D11Device→IDXGIDevice: {e}"))
            })?;

            let dcomp: IDCompositionDevice = unsafe { DCompositionCreateDevice2(&dxgi_device) }
                .map_err(|e| crate::WgpuError::DirectComposition(e.to_string()))?;
            let visual: IDCompositionVisual = unsafe { dcomp.CreateVisual() }
                .map_err(|e| crate::WgpuError::DirectComposition(e.to_string()))?;
            let target: IDCompositionTarget = unsafe { dcomp.CreateTargetForHwnd(hwnd, true) }
                .map_err(|e| crate::WgpuError::DirectComposition(e.to_string()))?;
            unsafe { target.SetRoot(&visual) }
                .map_err(|e| crate::WgpuError::DirectComposition(e.to_string()))?;
            unsafe { dcomp.Commit() }
                .map_err(|e| crate::WgpuError::DirectComposition(e.to_string()))?;

            let ptr = visual.as_raw();
            DCOMP_KEEPALIVE.with(|slot| {
                *slot.borrow_mut() = Some(DcompKeepAlive {
                    hwnd: hwnd_key,
                    _device: dcomp,
                    _visual: visual,
                    _target: target,
                });
            });
            ptr
        }
    };

    let surface = unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(visual_ptr))?
    };
    DCOMP_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(surface)
}

/// Everything you need to paint egui with [`wgpu`] on [`winit`].
///
/// Alternatively you can use [`crate::Renderer`] directly.
///
/// NOTE: all egui viewports share the same painter.
pub struct Painter {
    context: Context,
    configuration: WgpuConfiguration,
    msaa_samples: u32,
    support_transparent_backbuffer: bool,
    dithering: bool,
    depth_format: Option<wgpu::TextureFormat>,
    screen_capture_state: Option<CaptureState>,

    instance: wgpu::Instance,
    render_state: Option<RenderState>,

    // Per viewport/window:
    depth_texture_view: ViewportIdMap<wgpu::TextureView>,
    msaa_texture_view: ViewportIdMap<wgpu::TextureView>,
    surfaces: ViewportIdMap<SurfaceState>,
    capture_tx: CaptureSender,
    capture_rx: CaptureReceiver,
}

impl Painter {
    /// Manages [`wgpu`] state, including surface state, required to render egui.
    ///
    /// Only the [`wgpu::Instance`] is initialized here. Device selection and the initialization
    /// of render + surface state is deferred until the painter is given its first window target
    /// via [`set_window()`](Self::set_window). (Ensuring that a device that's compatible with the
    /// native window is chosen)
    ///
    /// Before calling [`paint_and_update_textures()`](Self::paint_and_update_textures) a
    /// [`wgpu::Surface`] must be initialized (and corresponding render state) by calling
    /// [`set_window()`](Self::set_window) once you have
    /// a [`winit::window::Window`] with a valid `.raw_window_handle()`
    /// associated.
    pub async fn new(
        context: Context,
        configuration: WgpuConfiguration,
        msaa_samples: u32,
        depth_format: Option<wgpu::TextureFormat>,
        support_transparent_backbuffer: bool,
        dithering: bool,
    ) -> Self {
        let (capture_tx, capture_rx) = capture_channel();
        let instance = configuration.wgpu_setup.new_instance().await;

        Self {
            context,
            configuration,
            msaa_samples,
            support_transparent_backbuffer,
            dithering,
            depth_format,
            screen_capture_state: None,

            instance,
            render_state: None,

            depth_texture_view: Default::default(),
            surfaces: Default::default(),
            msaa_texture_view: Default::default(),

            capture_tx,
            capture_rx,
        }
    }

    /// Get the [`RenderState`].
    ///
    /// Will return [`None`] if the render state has not been initialized yet.
    pub fn render_state(&self) -> Option<RenderState> {
        self.render_state.clone()
    }

    fn configure_surface(
        surface_state: &SurfaceState,
        render_state: &RenderState,
        config: &WgpuConfiguration,
    ) {
        profiling::function_scope!();

        let width = surface_state.width;
        let height = surface_state.height;

        let mut surf_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: render_state.target_format,
            present_mode: config.present_mode,
            alpha_mode: surface_state.alpha_mode,
            view_formats: vec![render_state.target_format],
            ..surface_state
                .surface
                .get_default_config(&render_state.adapter, width, height)
                .expect("The surface isn't supported by this adapter")
        };

        if let Some(desired_maximum_frame_latency) = config.desired_maximum_frame_latency {
            surf_config.desired_maximum_frame_latency = desired_maximum_frame_latency;
        }

        surface_state
            .surface
            .configure(&render_state.device, &surf_config);
    }

    /// Updates (or clears) the [`winit::window::Window`] associated with the [`Painter`]
    ///
    /// This creates a [`wgpu::Surface`] for the given Window (as well as initializing render
    /// state if needed) that is used for egui rendering.
    ///
    /// This must be called before trying to render via
    /// [`paint_and_update_textures`](Self::paint_and_update_textures)
    ///
    /// # Portability
    ///
    /// _In particular it's important to note that on Android a it's only possible to create
    /// a window surface between `Resumed` and `Paused` lifecycle events, and Winit will panic on
    /// attempts to query the raw window handle while paused._
    ///
    /// On Android [`set_window`](Self::set_window) should be called with `Some(window)` for each
    /// `Resumed` event and `None` for each `Paused` event. Currently, on all other platforms
    /// [`set_window`](Self::set_window) may be called with `Some(window)` as soon as you have a
    /// valid [`winit::window::Window`].
    ///
    /// # Errors
    /// If the provided wgpu configuration does not match an available device.
    pub async fn set_window(
        &mut self,
        viewport_id: ViewportId,
        window: Option<Arc<winit::window::Window>>,
    ) -> Result<(), crate::WgpuError> {
        profiling::scope!("Painter::set_window"); // profile_function gives bad names for async functions

        if let Some(window) = window {
            let size = window.inner_size();
            if !self.surfaces.contains_key(&viewport_id) {
                #[cfg(windows)]
                let mut dcomp = claim_dcomp_window(&window);
                #[cfg(not(windows))]
                let mut dcomp = false;

                #[cfg(windows)]
                let surface = if dcomp {
                    if self.render_state.is_none() {
                        self.render_state = Some(
                            RenderState::create(
                                &self.configuration,
                                &self.instance,
                                None,
                                self.depth_format,
                                self.msaa_samples,
                                self.dithering,
                            )
                            .await?,
                        );
                    }
                    let adapter = self
                        .render_state
                        .as_ref()
                        .expect("RenderState 刚创建")
                        .adapter
                        .clone();
                    // 预检过了这里仍可能失败（显卡驱动/远程桌面等）。就地退回普通交换链，
                    // 让上方按轮廓裁剪兜住形状，不要因为一层透明合成把整个启动打成 Err。
                    match create_dcomp_surface(&self.instance, &window, &adapter) {
                        Ok(surface) => surface,
                        Err(error) => {
                            log::warn!("DirectComposition surface 创建失败，回退普通交换链：{error}");
                            mark_dcomp_failed();
                            dcomp = false;
                            self.instance.create_surface(window)?
                        }
                    }
                } else {
                    self.instance.create_surface(window)?
                };
                #[cfg(not(windows))]
                let surface = self.instance.create_surface(window)?;
                self.add_surface(surface, viewport_id, size, dcomp).await?;
                commit_direct_composition();
            }
        } else {
            log::warn!("No window - clearing all surfaces");
            self.surfaces.clear();
        }
        Ok(())
    }

    /// Updates (or clears) the [`winit::window::Window`] associated with the [`Painter`] without taking ownership of the window.
    ///
    /// Like [`set_window`](Self::set_window) except:
    ///
    /// # Safety
    /// The user is responsible for ensuring that the window is alive for as long as it is set.
    pub async unsafe fn set_window_unsafe(
        &mut self,
        viewport_id: ViewportId,
        window: Option<&winit::window::Window>,
    ) -> Result<(), crate::WgpuError> {
        profiling::scope!("Painter::set_window_unsafe"); // profile_function gives bad names for async functions

        if let Some(window) = window {
            let size = window.inner_size();
            if !self.surfaces.contains_key(&viewport_id) {
                #[cfg(windows)]
                let mut dcomp = claim_dcomp_window(&window);
                #[cfg(not(windows))]
                let mut dcomp = false;

                #[cfg(windows)]
                let surface = if dcomp {
                    // DComp visual 需要 ID3D12Device，而设备又是随 surface 懒创建的。
                    // 先不带 surface 建 RenderState，再拿设备建 visual，最后建 surface。
                    if self.render_state.is_none() {
                        self.render_state = Some(
                            RenderState::create(
                                &self.configuration,
                                &self.instance,
                                None,
                                self.depth_format,
                                self.msaa_samples,
                                self.dithering,
                            )
                            .await?,
                        );
                    }
                    let adapter = self
                        .render_state
                        .as_ref()
                        .expect("RenderState 刚创建")
                        .adapter
                        .clone();
                    match create_dcomp_surface(&self.instance, window, &adapter) {
                        Ok(surface) => surface,
                        Err(error) => {
                            log::warn!("DirectComposition surface 创建失败，回退普通交换链：{error}");
                            mark_dcomp_failed();
                            dcomp = false;
                            unsafe {
                                self.instance
                                    .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::from_window(&window)?)?
                            }
                        }
                    }
                } else {
                    unsafe {
                        self.instance
                            .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::from_window(&window)?)?
                    }
                };
                #[cfg(not(windows))]
                let surface = unsafe {
                    self.instance
                        .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::from_window(&window)?)?
                };
                self.add_surface(surface, viewport_id, size, dcomp).await?;
                commit_direct_composition();
            }
        } else {
            log::warn!("No window - clearing all surfaces");
            self.surfaces.clear();
        }
        Ok(())
    }

    async fn add_surface(
        &mut self,
        surface: wgpu::Surface<'static>,
        viewport_id: ViewportId,
        size: winit::dpi::PhysicalSize<u32>,
        dcomp: bool,
    ) -> Result<(), crate::WgpuError> {
        let render_state = if let Some(render_state) = &self.render_state {
            render_state
        } else {
            let render_state = RenderState::create(
                &self.configuration,
                &self.instance,
                Some(&surface),
                self.depth_format,
                self.msaa_samples,
                self.dithering,
            )
            .await?;
            self.render_state.get_or_insert(render_state)
        };
        let alpha_mode = if dcomp {
            // DComp visual 的交换链必须预乘 alpha；Auto 在 DX12 上会按不透明处理，整窗变黑矩形。
            wgpu::CompositeAlphaMode::PreMultiplied
        } else if self.support_transparent_backbuffer {
            let supported_alpha_modes = surface.get_capabilities(&render_state.adapter).alpha_modes;

            // Prefer pre multiplied over post multiplied!
            if supported_alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
                wgpu::CompositeAlphaMode::PreMultiplied
            } else if supported_alpha_modes.contains(&wgpu::CompositeAlphaMode::PostMultiplied) {
                wgpu::CompositeAlphaMode::PostMultiplied
            } else {
                log::warn!("Transparent window was requested, but the active wgpu surface does not support a `CompositeAlphaMode` with transparency.");
                wgpu::CompositeAlphaMode::Auto
            }
        } else {
            wgpu::CompositeAlphaMode::Auto
        };
        self.surfaces.insert(
            viewport_id,
            SurfaceState {
                surface,
                width: size.width,
                height: size.height,
                alpha_mode,
                dcomp,
            },
        );
        let Some(width) = NonZeroU32::new(size.width) else {
            log::debug!("The window width was zero; skipping generate textures");
            return Ok(());
        };
        let Some(height) = NonZeroU32::new(size.height) else {
            log::debug!("The window height was zero; skipping generate textures");
            return Ok(());
        };
        self.resize_and_generate_depth_texture_view_and_msaa_view(viewport_id, width, height);
        Ok(())
    }

    /// Returns the maximum texture dimension supported if known
    ///
    /// This API will only return a known dimension after `set_window()` has been called
    /// at least once, since the underlying device and render state are initialized lazily
    /// once we have a window (that may determine the choice of adapter/device).
    pub fn max_texture_side(&self) -> Option<usize> {
        self.render_state
            .as_ref()
            .map(|rs| rs.device.limits().max_texture_dimension_2d as usize)
    }

    fn resize_and_generate_depth_texture_view_and_msaa_view(
        &mut self,
        viewport_id: ViewportId,
        width_in_pixels: NonZeroU32,
        height_in_pixels: NonZeroU32,
    ) {
        profiling::function_scope!();

        let width = width_in_pixels.get();
        let height = height_in_pixels.get();

        let render_state = self.render_state.as_ref().unwrap();
        let surface_state = self.surfaces.get_mut(&viewport_id).unwrap();

        surface_state.width = width;
        surface_state.height = height;

        Self::configure_surface(surface_state, render_state, &self.configuration);

        if let Some(depth_format) = self.depth_format {
            self.depth_texture_view.insert(
                viewport_id,
                render_state
                    .device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("egui_depth_texture"),
                        size: wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: self.msaa_samples,
                        dimension: wgpu::TextureDimension::D2,
                        format: depth_format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[depth_format],
                    })
                    .create_view(&wgpu::TextureViewDescriptor::default()),
            );
        }

        if let Some(render_state) = (self.msaa_samples > 1)
            .then_some(self.render_state.as_ref())
            .flatten()
        {
            let texture_format = render_state.target_format;
            self.msaa_texture_view.insert(
                viewport_id,
                render_state
                    .device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("egui_msaa_texture"),
                        size: wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: self.msaa_samples,
                        dimension: wgpu::TextureDimension::D2,
                        format: texture_format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[texture_format],
                    })
                    .create_view(&wgpu::TextureViewDescriptor::default()),
            );
        };
    }

    pub fn on_window_resized(
        &mut self,
        viewport_id: ViewportId,
        width_in_pixels: NonZeroU32,
        height_in_pixels: NonZeroU32,
    ) {
        profiling::function_scope!();

        if self.surfaces.contains_key(&viewport_id) {
            self.resize_and_generate_depth_texture_view_and_msaa_view(
                viewport_id,
                width_in_pixels,
                height_in_pixels,
            );
        } else {
            log::warn!("Ignoring window resize notification with no surface created via Painter::set_window()");
        }
    }

    /// Returns two things:
    ///
    /// The approximate number of seconds spent on vsync-waiting (if any),
    /// and the captures captured screenshot if it was requested.
    ///
    /// If `capture_data` isn't empty, a screenshot will be captured.
    pub fn paint_and_update_textures(
        &mut self,
        viewport_id: ViewportId,
        pixels_per_point: f32,
        clear_color: [f32; 4],
        clipped_primitives: &[epaint::ClippedPrimitive],
        textures_delta: &epaint::textures::TexturesDelta,
        capture_data: Vec<UserData>,
    ) -> f32 {
        profiling::function_scope!();

        let capture = !capture_data.is_empty();
        let mut vsync_sec = 0.0;

        let Some(render_state) = self.render_state.as_mut() else {
            return vsync_sec;
        };
        let Some(surface_state) = self.surfaces.get(&viewport_id) else {
            return vsync_sec;
        };

        let mut encoder =
            render_state
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("encoder"),
                });

        // Upload all resources for the GPU.
        let screen_descriptor = renderer::ScreenDescriptor {
            size_in_pixels: [surface_state.width, surface_state.height],
            pixels_per_point,
        };

        let user_cmd_bufs = {
            let mut renderer = render_state.renderer.write();
            for (id, image_delta) in &textures_delta.set {
                renderer.update_texture(
                    &render_state.device,
                    &render_state.queue,
                    *id,
                    image_delta,
                );
            }

            renderer.update_buffers(
                &render_state.device,
                &render_state.queue,
                &mut encoder,
                clipped_primitives,
                &screen_descriptor,
            )
        };

        let output_frame = {
            profiling::scope!("get_current_texture");
            // This is what vsync-waiting happens on my Mac.
            let start = web_time::Instant::now();
            let output_frame = surface_state.surface.get_current_texture();
            vsync_sec += start.elapsed().as_secs_f32();
            output_frame
        };

        let output_frame = match output_frame {
            Ok(frame) => frame,
            Err(err) => match (*self.configuration.on_surface_error)(err) {
                SurfaceErrorAction::RecreateSurface => {
                    Self::configure_surface(surface_state, render_state, &self.configuration);
                    return vsync_sec;
                }
                SurfaceErrorAction::SkipFrame => {
                    return vsync_sec;
                }
            },
        };

        let mut capture_buffer = None;
        {
            let renderer = render_state.renderer.read();

            let target_texture = if capture {
                let capture_state = self.screen_capture_state.get_or_insert_with(|| {
                    CaptureState::new(&render_state.device, &output_frame.texture)
                });
                capture_state.update(&render_state.device, &output_frame.texture);

                &capture_state.texture
            } else {
                &output_frame.texture
            };
            let target_view = target_texture.create_view(&wgpu::TextureViewDescriptor::default());

            let (view, resolve_target) = (self.msaa_samples > 1)
                .then_some(self.msaa_texture_view.get(&viewport_id))
                .flatten()
                .map_or((&target_view, None), |texture_view| {
                    (texture_view, Some(&target_view))
                });

            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui_render"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear_color[0] as f64,
                            g: clear_color[1] as f64,
                            b: clear_color[2] as f64,
                            a: clear_color[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: self.depth_texture_view.get(&viewport_id).map(|view| {
                    wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            // It is very unlikely that the depth buffer is needed after egui finished rendering
                            // so no need to store it. (this can improve performance on tiling GPUs like mobile chips or Apple Silicon)
                            store: wgpu::StoreOp::Discard,
                        }),
                        stencil_ops: None,
                    }
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Forgetting the pass' lifetime means that we are no longer compile-time protected from
            // runtime errors caused by accessing the parent encoder before the render pass is dropped.
            // Since we don't pass it on to the renderer, we should be perfectly safe against this mistake here!
            renderer.render(
                &mut render_pass.forget_lifetime(),
                clipped_primitives,
                &screen_descriptor,
            );

            if capture {
                if let Some(capture_state) = &mut self.screen_capture_state {
                    capture_buffer = Some(capture_state.copy_textures(
                        &render_state.device,
                        &output_frame,
                        &mut encoder,
                    ));
                }
            }
        }

        let encoded = {
            profiling::scope!("CommandEncoder::finish");
            encoder.finish()
        };

        // Submit the commands: both the main buffer and user-defined ones.
        {
            profiling::scope!("Queue::submit");
            // wgpu doesn't document where vsync can happen. Maybe here?
            let start = web_time::Instant::now();
            render_state
                .queue
                .submit(user_cmd_bufs.into_iter().chain([encoded]));
            vsync_sec += start.elapsed().as_secs_f32();
        };

        // Free textures marked for destruction **after** queue submit since they might still be used in the current frame.
        // Calling `wgpu::Texture::destroy` on a texture that is still in use would invalidate the command buffer(s) it is used in.
        // However, once we called `wgpu::Queue::submit`, it is up for wgpu to determine how long the underlying gpu resource has to live.
        {
            let mut renderer = render_state.renderer.write();
            for id in &textures_delta.free {
                renderer.free_texture(id);
            }
        }

        if let Some(capture_buffer) = capture_buffer {
            if let Some(screen_capture_state) = &mut self.screen_capture_state {
                screen_capture_state.read_screen_rgba(
                    self.context.clone(),
                    capture_buffer,
                    capture_data,
                    self.capture_tx.clone(),
                    viewport_id,
                );
            }
        }

        {
            profiling::scope!("present");
            // wgpu doesn't document where vsync can happen. Maybe here?
            let start = web_time::Instant::now();
            output_frame.present();
            vsync_sec += start.elapsed().as_secs_f32();
        }

        vsync_sec
    }

    /// Call this at the beginning of each frame to receive the requested screenshots.
    pub fn handle_screenshots(&self, events: &mut Vec<Event>) {
        for (viewport_id, user_data, screenshot) in self.capture_rx.try_iter() {
            let screenshot = Arc::new(screenshot);
            for data in user_data {
                events.push(Event::Screenshot {
                    viewport_id,
                    user_data: data,
                    image: screenshot.clone(),
                });
            }
        }
    }

    pub fn gc_viewports(&mut self, active_viewports: &ViewportIdSet) {
        self.surfaces.retain(|id, _| active_viewports.contains(id));
        self.depth_texture_view
            .retain(|id, _| active_viewports.contains(id));
        self.msaa_texture_view
            .retain(|id, _| active_viewports.contains(id));
    }

    #[allow(clippy::needless_pass_by_ref_mut, clippy::unused_self)]
    pub fn destroy(&mut self) {
        // TODO(emilk): something here?
    }
}
