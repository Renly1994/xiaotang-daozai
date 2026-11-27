//! D3D11 自绘渲染运行时：单 device 上 WGC 抓屏 → HLSL 玻璃 → egui widgets →
//! Present → DComp Commit。从 egui-directx11 的 glass_spike.rs 移植到 windows 0.58。
//!
//! 透明路线：CreateSwapChainForComposition + IDCompositionVisual（逐像素预乘 alpha），
//! 窗口用 WS_EX_NOREDIRECTIONBITMAP，玻璃外区域完全透明显示桌面。

use super::renderer::Renderer;
use super::wgc_capture::WgcCapture;
use std::cell::RefCell;
use std::ptr;
use std::rc::Rc;
use windows::core::Interface;
use windows::Win32::{
    Foundation::{BOOL, HMODULE, HWND, RECT},
    Graphics::{
        Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST},
        Direct3D11::*,
        DirectComposition::{DCompositionCreateDevice2, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual},
        Dxgi::{Common::*, *},
    },
};


/// egui 0.31 没有 `Context::run_ui`（0.37 才有）；这里用 `Context::run` + 手动构造
/// 覆盖全屏的 root Ui 来等价替代，调用方拿到的 `&mut Ui` 与 0.37 一致。
fn run_ui_031(
    ctx: &egui::Context,
    input: egui::RawInput,
    mut run_ui: impl FnMut(&mut egui::Ui),
) -> egui::FullOutput {
    let screen_rect = input.screen_rect.unwrap_or_else(|| egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::Vec2::new(512.0, 210.0),
    ));
    ctx.run(input, |ctx| {
        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::new(("daozai", "root-ui")),
            egui::UiBuilder::new()
                .layer_id(egui::LayerId::background())
                .max_rect(screen_rect),
        );
        run_ui(&mut ui);
    })
}

pub const MAX_CTL: usize = 13;

/// 玻璃层 uniform，布局必须与 shaders/glass.hlsl 的 `cbuffer U` 完全一致（11 x float4）。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct GlassParams {
    /// [x, y, w, h] 岛体玻璃在窗口内的矩形（物理像素）
    pub bounds: [f32; 4],
    /// [chroma, -, morph, material]
    pub params: [f32; 4],
    /// [radius, texW, texH, -]
    pub extra: [f32; 4],
    /// [r, g, b, a] 玻璃染色
    pub tint: [f32; 4],
    /// [flipY, origin_x, origin_y, -] WGC 全屏帧采样用窗口屏幕原点
    pub quad: [f32; 4],
    /// [pointer_x, pointer_y, refract, flat]
    pub misc: [f32; 4],
    /// 控件玻璃矩形（最多 MAX_CTL 个）
    pub crect: [[f32; 4]; MAX_CTL],
    /// 控件玻璃参数 [radius, enabled, magnify, refract]
    pub cprm: [[f32; 4]; MAX_CTL],
    /// 控件材质样式 [style, -, -, -]（0普通 1分栏容器暗 2分栏指示器亮）
    pub cstyle: [[f32; 4]; MAX_CTL],
    /// [blur, band, edge_width, edge_opacity]
    pub look: [f32; 4],
    /// [-, -, -, isDComp=1] 逐像素 alpha
    pub notch: [f32; 4],
    /// 光源配置
    pub lightcfg: [f32; 4],
    /// 融球水滴: xy=中心(capture px,顶原点), zw=半宽半高
    pub meta: [f32; 4],
    /// 融球: x=k, y=锚点y, z=开关, w=保留
    pub meta2: [f32; 4],
}

/// app 的 `paint_gpu_glass` 在 `run_ui` 内写当帧的岛体几何/刘海形状/染色快照，
/// 而 `build_glass_params` 在 `run_ui` 之前读，拿到的是上一帧。收起成超宽岛体时
/// body_rect 与肩部 flare 逐帧变化，差一帧就会让玻璃边缘扫过常驻宠物和电量图标，
/// 表现为图标闪烁/抽搐。这里在 run_ui 之后按快照重算，和控件透镜的刷新同理。
fn refresh_glass_uniforms(u: &mut GlassParams) {
    let snap = crate::gpu_glass::uniforms();
    if !snap.active {
        u.bounds = [-10000.0, -10000.0, 1.0, 1.0];
        return;
    }
    // ppp / 画布尺寸 / 停靠方向由 build_glass_params 冻结在 u 里，这里不再从设置反推。
    let ppp = u.params[1].max(0.1);
    let h = u.extra[2];
    let b = snap.body_rect;
    let (bx, by, bw, bh) = (b[0] * ppp, b[1] * ppp, b[2] * ppp, b[3] * ppp);
    u.bounds = [bx, h - by - bh, bw, bh];
    u.extra[0] = snap.radius * ppp;
    u.params = [snap.chroma * ppp, ppp, snap.morph, snap.material];
    u.misc = [snap.pointer[0] * ppp, h - snap.pointer[1] * ppp, snap.refract * ppp, 0.0];
    u.look = [snap.blur * ppp, snap.band * ppp, snap.edge_w * ppp, snap.edge_op];
    u.lightcfg = [snap.light_follow, 0.0, snap.shadow, 0.0];
    u.tint = if snap.material > 0.5 { [0.96, 0.97, 1.0, 0.10] } else { [0.55, 0.72, 0.90, 0.10] };
    let shape = crate::gpu_glass::shape_params();
    u.notch = [shape[0] * ppp, shape[1] * ppp, shape[2], 1.0];
}

/// 把 app 本帧写入的控件玻璃快照（egui 逻辑点、y 从顶）同步到 shader uniform。
/// 必须在 `ctx.run_ui` 之后调用——app 的 paint_gpu_glass 在 run_ui 内发布控件，
/// 提前读会拿到上一帧，展开/收起动画时残留透镜白边会闪一条竖线。
///
/// 高度基准用 `u.extra[2]`（= build_glass_params 写入、shader 全屏三角形实际使用的
/// 画布物理高）。**不要**用 `window.inner_size().height`：那是窗口的物理像素高，
/// 高 DPI 下与 canvas_h 差一截——控件 y 翻转用错基准会让透镜偏移到面板外。
///
/// ppp 由 `params.y` 显式传入，**不要**再从 canvas_h 反推：过去用 `canvas_h / WIN_H`
/// 在左右停靠时错了——那时画布高是 SIDE_H(600)*ppp 而非 WIN_H(270)*ppp，反推出 2.2 倍
/// 的 ppp，控件透镜全部飞到面板外，表现为「左右停靠时控件没有玻璃效果」。
fn refresh_control_uniforms(u: &mut GlassParams) {
    let canvas_h = u.extra[2];
    let ppp = if u.params[1] > 0.0 { u.params[1] } else { (canvas_h / crate::app::WIN_H).max(0.1) };
    let (ctls, n) = crate::gpu_glass::controls_snapshot();
    let slots = u.cprm.len();
    let n = n.min(slots);
    for (slot, ctl) in ctls.iter().take(n).enumerate() {
        let r = &ctl.rect;
        let rx = r.min.x * ppp;
        let ry = r.min.y * ppp;
        let rw = r.width() * ppp;
        let rh = r.height() * ppp;
        // y 从 egui 顶原点翻到 D3D11 底原点，画布高用 shader 的 extra[2]（=岛体 bounds 同基准）。
        u.crect[slot] = [rx, canvas_h - ry - rh, rw, rh];
        u.cprm[slot] = [ctl.radius * ppp, 1.0, ctl.mag.max(1.0), ctl.refract.max(0.05)];
        u.cstyle[slot] = [ctl.style as f32, 0.0, 0.0, 0.0];
    }
    // 关键：本帧不用的槽必须清掉启用位，否则沿用上一帧的 cprm.y=1 会画出幽灵透镜
    // （收起后残留的竖白线正是这个）。
    for slot in n..slots {
        u.cprm[slot] = [0.0, 0.0, 1.0, 1.0];
        u.cstyle[slot] = [0.0; 4];
    }
}

/// 主窗口「岛体大小」缩放：把 egui 输出的 IME 位置从放大后的逻辑点换回原生逻辑点。
/// 与 winit_runner::scale_root_input 成对，子窗口不参与。
fn scale_root_platform_output(output: &mut egui::PlatformOutput, scale: f32) {
    if !scale.is_finite() || (scale - 1.0).abs() < 1e-4 {
        return;
    }
    let rect = |r: egui::Rect| {
        egui::Rect::from_min_max(
            egui::pos2(r.min.x * scale, r.min.y * scale),
            egui::pos2(r.max.x * scale, r.max.y * scale),
        )
    };
    if let Some(ime) = output.ime.as_mut() {
        ime.rect = rect(ime.rect);
        ime.cursor_rect = rect(ime.cursor_rect);
    }
}

/// D3D11 暂存纹理是「自下而上 + BGRA」，egui 要「自上而下 + RGBA（预乘）」。
fn bgra_bottom_up_to_rgba_top_down(src: &[u8], row_pitch: usize, w: usize, h: usize) -> Vec<u8> {
    let mut rgba = vec![0u8; w * h * 4];
    for row in 0..h {
        let srow = &src[row * row_pitch..row * row_pitch + w * 4];
        let dst = &mut rgba[(h - 1 - row) * w * 4..];
        for x in 0..w {
            let s = &srow[x * 4..x * 4 + 4];
            let d = &mut dst[x * 4..x * 4 + 4];
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = s[3];
        }
    }
    rgba
}

/// 从刚画完的 D3D11 渲染目标读回 RGBA 帧（顶原点、预乘 alpha）。
/// 语义对齐 eframe 0.35 `egui_glow::Painter::read_screen_rgba`：
/// egui 的 Screenshot 事件和兼容渲染分层窗口都期待「顶原点 RGBA 预乘」。
/// 只在 egui 显式请求截图时调用，避免每帧 GPU stall。
fn read_back_rt_rgba(
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    rt: &ID3D11RenderTargetView,
) -> Option<egui::ColorImage> {
    let tex: ID3D11Texture2D = unsafe { rt.GetResource().ok()?.cast().ok()? };
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { tex.GetDesc(&mut desc) };
    let (w, h) = (desc.Width as usize, desc.Height as usize);
    if w == 0 || h == 0 { return None; }
    let staging_desc = D3D11_TEXTURE2D_DESC {
        Width: w as u32, Height: h as u32, MipLevels: 1, ArraySize: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING, BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut staging = None;
    if unsafe { device.CreateTexture2D(&staging_desc, None, Some(&mut staging)) }.is_err() { return None; }
    let staging = staging?;
    unsafe { ctx.CopyResource(&staging, &tex); }
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    if unsafe { ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.is_err() { return None; }
    // D3D11 纹理行序自下而上、通道 BGRA；egui 要自上而下 RGBA。
    let pitch = mapped.RowPitch as usize;
    let mapped_bytes = unsafe {
        std::slice::from_raw_parts(mapped.pData as *const u8, pitch * h)
    };
    let rgba = bgra_bottom_up_to_rgba_top_down(mapped_bytes, pitch, w, h);
    unsafe { ctx.Unmap(&staging, 0); }
    Some(egui::ColorImage::from_rgba_premultiplied([w, h], &rgba))
}

/// 照搬 eframe 0.35 `glow_integration.rs:715-747` 的 ActionRequested 回灌：
/// `process_viewport_commands` 只把 Screenshot/Cut/Copy/Paste 记进 actions，
/// 真正的像素读回与事件注入必须由集成层完成，否则这些功能全是死数据。
fn drain_actions(
    egui_winit: &mut egui_winit::State,
    actions: &mut egui::ahash::HashSet<egui_winit::ActionRequested>,
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    rt: Option<&ID3D11RenderTargetView>,
    _viewport_id: egui::ViewportId,
) {
    if actions.is_empty() { return; }
    for action in actions.drain() {
        match action {
            egui_winit::ActionRequested::Screenshot(user_data) => {
                // screenshot 必须在 Present 之前读回（FLIP_DISCARD 交换链 Present 后后缓冲内容失效）。
                // egui 0.31 用 Event::Screenshot 回灌（0.37 改成一次性 ScreenshotCallback）。
                if let Some(image) = rt.and_then(|rt| read_back_rt_rgba(device, ctx, rt)) {
                    egui_winit.egui_input_mut().events.push(egui::Event::Screenshot {
                        viewport_id: _viewport_id,
                        user_data,
                        image: std::sync::Arc::new(image),
                    });
                }
            }
            egui_winit::ActionRequested::Cut => {
                egui_winit.egui_input_mut().events.push(egui::Event::Cut);
            }
            egui_winit::ActionRequested::Copy => {
                egui_winit.egui_input_mut().events.push(egui::Event::Copy);
            }
            egui_winit::ActionRequested::Paste => {
                if let Some(contents) = egui_winit.clipboard_text() {
                    let contents = contents.replace("\r\n", "\n");
                    if !contents.is_empty() {
                        egui_winit.egui_input_mut().events.push(egui::Event::Paste(contents));
                    }
                }
            }
        }
    }
}

struct GlassPipeline {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    cbuf: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    blend: ID3D11BlendState,
    /// b1：材质插件设置值（8 x float4），内置玻璃不用，仍每帧更新。
    plugin_cbuf: ID3D11Buffer,
    bg_srv: ID3D11ShaderResourceView,
    bg_tex: ID3D11Texture2D,
    bg_w: u32,
    bg_h: u32,
}

struct DcompState {
    _device: IDCompositionDevice,
    _visual: IDCompositionVisual,
    _target: IDCompositionTarget,
}

/// 一帧的渲染统计（供 UI 状态行显示）。
#[derive(Clone, Default)]
pub struct FrameStats {
    pub fps: f32,
    pub wgc_live: bool,
    /// 主窗口 run_ui 产出的子 viewport 窗口命令（Visible/OuterPosition/InnerSize 等），
    /// 由 winit_runner 应用到对应子窗口。immediate viewport 的 UI 在主窗口跑，
    /// 命令也在主窗口 output 里，必须传出去否则设置页永远 invisible。
    pub viewport_commands: Vec<(egui::ViewportId, Vec<egui::ViewportCommand>)>,
    /// 本帧仍活跃的 viewport id（app 不再 show_viewport 的窗口要销毁，否则关不掉）。
    pub active_viewports: Vec<egui::ViewportId>,
    /// Present 返回 DXGI_ERROR_DEVICE_REMOVED / DEVICE_RESET：D3D 设备已丢失，
    /// 集成层必须重建 device/swapchain（并重建共享 Renderer 与子窗口），否则黑屏到退出。
    pub device_lost: bool,
}

/// D3D11 抓屏栈诊断：同时输出到 stderr 和 liquid-glass.log。
/// 之前只用 eprintln，日志里看不到重建/失败/watchdog，概率性冻结无从诊断。
fn dlog(msg: &str) {
    eprintln!("[d3d11] {}", msg);
    crate::wgc::log(&format!("d3d11: {}", msg));
}

/// 渲染运行时。持有窗口生命周期内不能重建的 D3D/DComp 状态。
pub struct D3d11Runtime {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    swap: IDXGISwapChain1,
    rt: Option<ID3D11RenderTargetView>,
    dcomp: DcompState,
    glass: GlassPipeline,
    /// 当前激活材质包编译出的像素着色器；None 时用内置玻璃 PS。
    #[cfg(feature="sdk-js")]
    material_ps: Option<ID3D11PixelShader>,
    #[cfg(feature="sdk-js")]
    material_gen: u64,
    wgc: Option<WgcCapture>,
    /// 当前 WGC 会话所抓显示器在虚拟桌面中的原点；窗口换屏后与当前屏不同则重建。
    wgc_monitor_origin: Option<(i32, i32)>,
    wgc_ever_frame: bool,
    /// 创建当前 WGC 会话时的桌面拓扑代数。与 `wgc::topo_epoch()` 不等说明
    /// 换过分辨率/显示模式：会话绑定的主屏/帧尺寸已失效，必须重建（社区标准做法：
    /// item size 变化时对 frame pool 做 Recreate；这里整个会话重建，等价且更简单）。
    wgc_epoch: u64,
    /// 重建失败后的退避重试时刻。多屏切换中间态 WgcCapture::new 常失败，
    /// 不能一次失败就永久放弃（否则玻璃定格不自愈）；按 0.2/0.4/0.8...s 退避重试。
    wgc_retry_at: Option<std::time::Instant>,
    wgc_retry_backoff_ms: u64,
    /// 最近一次成功抓到帧的时刻。会话可能在稳定期中间建立、之后因 DWM 重组
    /// 永久不送帧；此时拿不到帧 → 尺寸校验也无从触发 → 必须靠 watchdog 重建。
    /// 社区 streaming 项目「重建后 confirm 新帧到达」同款。
    wgc_last_frame_at: Option<std::time::Instant>,
    pub egui_ctx: egui::Context,
    /// 所有 viewport 共用同一个 Renderer/TexturePool：egui 字体图集是 Context 级单例，
    /// 各窗口各自 Renderer 会导致 partial 纹理更新 pos 套到旧图集上越界（文字错乱+闪退）。
    egui_renderer: Rc<RefCell<Renderer>>,
    egui_winit: Option<egui_winit::State>,
    frames: u64,
    last_report: std::time::Instant,
    fps: f32,
    /// 上一帧 egui 请求的重绘延迟（about_to_wait 用）
    pub repaint_delay: std::time::Duration,
    /// 主窗口 viewport 信息（窗口命令处理用）
    viewport_info: egui::ViewportInfo,
    actions: egui::ahash::HashSet<egui_winit::ActionRequested>,
}

impl D3d11Runtime {
    /// 子 viewport 必须复用主窗口的 device/context/Renderer：egui 字体图集是
    /// Context 级单例，各窗口独立 Renderer 会让 partial 纹理更新越界闪退。
    pub fn device(&self) -> ID3D11Device { self.device.clone() }
    pub fn device_context(&self) -> ID3D11DeviceContext { self.ctx.clone() }
    pub fn shared_renderer(&self) -> Rc<RefCell<Renderer>> { self.egui_renderer.clone() }
    /// 第一次启动：新建 egui Context。恢复路径请用 `new_with_ctx` 保留原 Context，
    /// 否则 Shell / 后台线程手里的旧 Context 会和新渲染栈脱节（repaint 与字体图集全断）。
    pub fn new(window: &winit::window::Window, prefer_dgpu: bool) -> windows::core::Result<Self> {
        Self::new_with_ctx(window, egui::Context::default(), prefer_dgpu)
    }

    /// 用既有 egui Context 重建 D3D11 栈（suspended/resume、设备丢失恢复共用）。
    pub fn new_with_ctx(window: &winit::window::Window, egui_ctx: egui::Context, prefer_dgpu: bool) -> windows::core::Result<Self> {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let RawWindowHandle::Win32(h) = window.window_handle().unwrap().as_raw() else {
            return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL));
        };
        let size = window.inner_size();
        // WGC 抓整屏合成帧；必须把岛仔自己排除出合成树，否则玻璃会采样到
        // 它自己的窗口（递归自采样）。原版 gpu_glass.rs 在 wgpu 栈做同样的事，
        // 但迁移后 gpu_glass 被重定向到 stub，这里由新渲染栈接手。
        {
            use windows::Win32::UI::WindowsAndMessaging::{
                SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
            };
            let hwnd = HWND(h.hwnd.get() as _);
            if let Err(e) = unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) } {
                eprintln!("[d3d11] WDA_EXCLUDEFROMCAPTURE failed: {:?}", e);
            }
        }
        eprintln!("[d3d11] create_device_swap {}x{} hwnd={:?}", size.width, size.height, h.hwnd.get());
        let (device, ctx, swap, dcomp) = match Self::create_device_swap(HWND(h.hwnd.get() as _), size.width, size.height, prefer_dgpu) {
            Ok(v) => { eprintln!("[d3d11] device/swap/dcomp OK"); v }
            Err(e) => { eprintln!("[d3d11] device/swap FAILED: {:?}", e); return Err(e); }
        };
        let rt = Self::create_rt(&device, &swap)?;

        let egui_renderer = Rc::new(RefCell::new(Renderer::new(&device)?));
        let egui_winit = egui_winit::State::new(
            egui_ctx.clone(), egui_ctx.viewport_id(), window, None, None, None);

        // WGC 与主 device 同一份，CopyResource 纯 GPU、零回读。失败时背景保持黑色，
        // 玻璃仍可显示（HLSL 采样黑底），不影响 UI 交互。
        let hwnd = HWND(h.hwnd.get() as _);
        let wgc = match WgcCapture::new(&device, hwnd) {
            Ok(c) => { eprintln!("[d3d11] WGC OK {}x{} origin={:?}", c.size().0, c.size().1, c.capture_origin()); Some(c) }
            Err(e) => { eprintln!("[d3d11] WGC unavailable: {:?}", e); None }
        };
        let wgc_monitor_origin = wgc.as_ref().map(|c| c.capture_origin());
        let (bg_w, bg_h) = wgc.as_ref().map(|c| c.size()).unwrap_or((1920, 1080));
        eprintln!("[d3d11] creating glass pipeline bg={}x{}", bg_w, bg_h);
        let glass = match Self::create_glass_pipeline(&device, bg_w, bg_h) {
            Ok(g) => { eprintln!("[d3d11] glass pipeline OK"); g }
            Err(e) => { eprintln!("[d3d11] glass pipeline FAILED: {:?}", e); return Err(e); }
        };

        Ok(Self {
            device, ctx, swap, rt: Some(rt), dcomp, glass, wgc, wgc_monitor_origin,
            #[cfg(feature="sdk-js")]
            material_ps: None,
            #[cfg(feature="sdk-js")]
            material_gen: 0,
            wgc_ever_frame: false, wgc_epoch: crate::wgc::topo_epoch(),
            wgc_retry_at: None, wgc_retry_backoff_ms: 200, wgc_last_frame_at: None,
            egui_ctx, egui_renderer, egui_winit: Some(egui_winit),
            frames: 0, last_report: std::time::Instant::now(), fps: 0.0,
            repaint_delay: std::time::Duration::ZERO,
            viewport_info: egui::ViewportInfo::default(), actions: Default::default(),
        })
    }

    /// 根据启动期快照选择 D3D11 适配器。
    ///
    /// 关闭独显偏好时保持原来的行为：直接用 DXGI 的默认（第 0 个）适配器。
    /// 开启时枚举适配器，跳过软件适配器和 Microsoft Basic Render Driver，
    /// 优先独立显存更大的卡；没有独显则退回默认适配器。
    fn choose_adapter(factory: &IDXGIFactory2, prefer_dgpu: bool) -> windows::core::Result<IDXGIAdapter> {
        if !prefer_dgpu {
            return unsafe { factory.EnumAdapters(0) };
        }

        let factory1: IDXGIFactory1 = factory.cast()?;
        let mut best_discrete: Option<(u64, String, IDXGIAdapter1)> = None;
        for index in 0..16u32 {
            let adapter = match unsafe { factory1.EnumAdapters1(index) } {
                Ok(adapter) => adapter,
                Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(error) => return Err(error),
            };
            let desc = unsafe { adapter.GetDesc1()? };
            if desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 || desc.VendorId == 0x1414 {
                continue;
            }
            let score = Self::adapter_score(&desc);
            let name = String::from_utf16_lossy(
                &desc.Description[..desc.Description.iter().position(|&c| c == 0).unwrap_or(128)],
            );
            if score & (1u64 << 62) != 0
                && best_discrete.as_ref().map_or(true, |(s, _, _)| score > *s)
            {
                best_discrete = Some((score, name, adapter));
            }
        }

        if let Some((score, name, adapter)) = best_discrete {
            eprintln!("[d3d11] adapter selected: {name} (score {score})");
            return adapter.cast();
        }
        eprintln!("[d3d11] no discrete adapter found, falling back to default");
        unsafe { factory.EnumAdapters(0) }
    }

    fn adapter_score(desc: &DXGI_ADAPTER_DESC1) -> u64 {
        // NVIDIA / AMD 基本都是独显；Intel 只有在独立显存达到 1 GiB 时才按独显排序
        // （新版 Arc 独显满足，核显通常不满足）。
        let discrete = matches!(desc.VendorId, 0x10DE | 0x1002)
            || (desc.VendorId == 0x8086 && desc.DedicatedVideoMemory >= 1024 * 1024 * 1024);
        let mut score = (desc.DedicatedVideoMemory.min(u64::MAX as usize) as u64).min(u64::MAX - 1);
        if discrete {
            score = score.saturating_add(1u64 << 62);
        }
        score
    }

    fn create_device_swap(window: HWND, w: u32, h: u32, prefer_dgpu: bool)
        -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext, IDXGISwapChain1, DcompState)> {
        let factory: IDXGIFactory2 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS::default()) }?;
        let adapter = Self::choose_adapter(&factory, prefer_dgpu)?;
        let mut device = None;
        let mut ctx = None;
        unsafe {
            // windows 0.58: 第 3 参 software 是 Param<HMODULE>，与 0.62 直接 HMODULE 不同。
            D3D11CreateDevice(
                &adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE(ptr::null_mut()),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut ctx),
            )?;
        }
        let device = device.unwrap();
        let ctx = ctx.unwrap();

        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: w, Height: h,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            Stereo: BOOL(0),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };
        let swap = unsafe { factory.CreateSwapChainForComposition(&device, &desc, None)? };

        let dxgi_device: IDXGIDevice = device.cast()?;
        let dcomp: IDCompositionDevice = unsafe { DCompositionCreateDevice2(&dxgi_device)? };
        let visual: IDCompositionVisual = unsafe { dcomp.CreateVisual()? };
        let target: IDCompositionTarget = unsafe { dcomp.CreateTargetForHwnd(window, true)? };
        unsafe {
            visual.SetContent(&swap)?;
            target.SetRoot(&visual)?;
            dcomp.Commit()?;
        }
        let dcomp_state = DcompState { _device: dcomp, _visual: visual, _target: target };
        Ok((device, ctx, swap, dcomp_state))
    }

    fn create_rt(device: &ID3D11Device, swap: &IDXGISwapChain1) -> windows::core::Result<ID3D11RenderTargetView> {
        let tex = unsafe { swap.GetBuffer::<ID3D11Texture2D>(0)? };
        let mut rt = None;
        unsafe { device.CreateRenderTargetView(&tex, None, Some(&mut rt))? };
        Ok(rt.unwrap())
    }

    fn create_glass_pipeline(device: &ID3D11Device, bg_w: u32, bg_h: u32) -> windows::core::Result<GlassPipeline> {
        let vs_blob = include_bytes!("../../shaders/glass_vs.bin");
        let ps_blob = include_bytes!("../../shaders/glass_ps.bin");
        unsafe {
            let mut vs = None;
            device.CreateVertexShader(vs_blob, None, Some(&mut vs))?;
            let mut ps = None;
            device.CreatePixelShader(ps_blob, None, Some(&mut ps))?;
            let mut cbuf = None;
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: std::mem::size_of::<GlassParams>() as u32,
                    Usage: D3D11_USAGE_DYNAMIC,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                    MiscFlags: 0, StructureByteStride: 0,
                }, None, Some(&mut cbuf))?;
            // b1: 32 个材质插件参数（8 x float4）。内置玻璃不读它，但材质 pass 需要。
            let mut plugin_cbuf = None;
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 32 * std::mem::size_of::<f32>() as u32,
                    Usage: D3D11_USAGE_DYNAMIC,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                    MiscFlags: 0, StructureByteStride: 0,
                }, None, Some(&mut plugin_cbuf))?;
            let mut sampler = None;
            device.CreateSamplerState(
                &D3D11_SAMPLER_DESC {
                    Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    ComparisonFunc: D3D11_COMPARISON_ALWAYS,
                    BorderColor: [0.0; 4], MaxAnisotropy: 1,
                    MinLOD: 0.0, MaxLOD: f32::MAX, MipLODBias: 0.0,
                }, Some(&mut sampler))?;
            let mut blend = None;
            device.CreateBlendState(
                &D3D11_BLEND_DESC {
                    AlphaToCoverageEnable: BOOL(0), IndependentBlendEnable: BOOL(0),
                    RenderTarget: [D3D11_RENDER_TARGET_BLEND_DESC {
                        BlendEnable: BOOL(1),
                        SrcBlend: D3D11_BLEND_ONE, DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                        BlendOp: D3D11_BLEND_OP_ADD,
                        SrcBlendAlpha: D3D11_BLEND_ONE, DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                        BlendOpAlpha: D3D11_BLEND_OP_ADD,
                        RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
                    }; 8],
                }, Some(&mut blend))?;

            // 背景纹理：BGRA，尺寸匹配 WGC 帧。初始全黑（WGC 首帧后由 CopyResource 覆盖）。
            let desc = D3D11_TEXTURE2D_DESC {
                Width: bg_w, Height: bg_h, MipLevels: 1, ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0, MiscFlags: 0,
            };
            let mut bg_tex = None;
            device.CreateTexture2D(&desc, None, Some(&mut bg_tex))?;
            let bg_tex = bg_tex.unwrap();
            let mut bg_srv = None;
            device.CreateShaderResourceView(&bg_tex, None, Some(&mut bg_srv))?;

            Ok(GlassPipeline {
                vs: vs.unwrap(), ps: ps.unwrap(), cbuf: cbuf.unwrap(),
                plugin_cbuf: plugin_cbuf.unwrap(),
                sampler: sampler.unwrap(), blend: blend.unwrap(),
                bg_srv: bg_srv.unwrap(), bg_tex, bg_w, bg_h,
            })
        }
    }

    /// 换分辨率/显示模式后重建 WGC 抓屏会话 + 背景纹理。
    ///
    /// 微软官方（screen-capture）与主线 `recreate_after_device_lost()` 的标准做法：
    /// item 尺寸变化时对 frame pool 做 Recreate、丢弃旧帧。这里直接 drop 旧会话再
    /// `WgcCapture::new`：new 内部重新取当前主屏 HMONITOR + item.Size() 建 pool/session，
    /// 覆盖「换分辨率」和「主屏互换/扩展复制」两种情形，且旧帧一并丢弃。
    /// bg_tex 尺寸必须跟着新帧走——D3D11 CopyResource 要求 src/dst 尺寸一致，
    /// 否则静默失败、一直拿旧纹理画 = 玻璃永久定格（用户实测的故障）。
    fn rebuild_wgc(&mut self, hwnd: HWND) {
        let epoch = crate::wgc::topo_epoch();
        // 关键：new 失败（多屏切换中间态主屏 item 暂不可用）时**不设 None、不更新 epoch**，
        // 保留旧会话 + 退避重试。否则 epoch 被标记为已处理 → 永不重建 → 玻璃永久定格。
        // 这正是社区 streaming 项目反复强调的「reinit 失败不能 latch 成永久状态」。
        let fresh = match WgcCapture::new(&self.device, hwnd) {
            Ok(c) => c,
            Err(e) => {
                let backoff = self.wgc_retry_backoff_ms.min(2000);
                self.wgc_retry_at = Some(std::time::Instant::now() + std::time::Duration::from_millis(backoff));
                self.wgc_retry_backoff_ms = (backoff * 2).min(2000);
                dlog(&format!("topology epoch={} rebuild WGC failed ({:?}), retry in {}ms",
                              epoch, e, backoff));
                return;
            }
        };
        let (bw, bh) = fresh.size();
        dlog(&format!("topology epoch={} rebuild WGC {}x{} (was {}x{})",
                      epoch, bw, bh, self.glass.bg_w, self.glass.bg_h));
        // 尺寸没变也重建会话（主屏 monitor 可能换了、item 失效），但纹理可复用。
        if bw != self.glass.bg_w || bh != self.glass.bg_h {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: bw, Height: bh, MipLevels: 1, ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0, MiscFlags: 0,
            };
            let built = unsafe {
                let mut tex = None;
                if self.device.CreateTexture2D(&desc, None, Some(&mut tex)).is_err() { None } else {
                    let tex = tex.unwrap();
                    let mut srv = None;
                    if self.device.CreateShaderResourceView(&tex, None, Some(&mut srv)).is_err() { None }
                    else { Some((tex, srv.unwrap())) }
                }
            };
            if let Some((tex, srv)) = built {
                self.glass.bg_tex = tex;
                self.glass.bg_srv = srv;
                self.glass.bg_w = bw;
                self.glass.bg_h = bh;
            } else {
                // 新纹理创建失败：保留旧会话/纹理与 epoch，退避后再试（自愈）。
                let backoff = self.wgc_retry_backoff_ms.min(2000);
                self.wgc_retry_at = Some(std::time::Instant::now() + std::time::Duration::from_millis(backoff));
                self.wgc_retry_backoff_ms = (backoff * 2).min(2000);
                dlog(&format!("rebuild bg texture failed, retry in {}ms", backoff));
                return;
            }
        }
        // 全部成功：换上新会话、标记 epoch 已处理、清退避（下次拓扑变化从 200ms 重新起）。
        self.wgc_monitor_origin = Some(fresh.capture_origin());
        self.wgc = Some(fresh);
        self.wgc_ever_frame = false;
        self.wgc_epoch = epoch;
        self.wgc_retry_at = None;
        self.wgc_retry_backoff_ms = 200;
        // watchdog 从重建时刻起算：若 1.5s 内没有新帧到达，说明这个会话是「活的但不送帧」，
        // 需要再重建（稳定期中间建立的会话常这样）。
        self.wgc_last_frame_at = Some(std::time::Instant::now());
    }

    /// 窗口尺寸变化：重建 RT（swap 由 DComp 持有，ResizeBuffers 即可）。
    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 { return; }
        self.rt.take();
        unsafe { let _ = self.swap.ResizeBuffers(2, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SWAP_CHAIN_FLAG(0)); }
        self.rt = Self::create_rt(&self.device, &self.swap).ok();
    }

    /// 取 egui-winit 输入（由事件循环在 Redraw 时调用）。
    pub fn take_input(&mut self, window: &winit::window::Window) -> egui::RawInput {
        self.egui_winit.as_mut().unwrap().take_egui_input(window)
    }

    pub fn on_window_event(&mut self, window: &winit::window::Window, event: &winit::event::WindowEvent) -> bool {
        let resp = self.egui_winit.as_mut().unwrap().on_window_event(window, event);
        if !resp.consumed {
            if let winit::event::WindowEvent::Resized(s) = event {
                self.resize(s.width, s.height);
            }
        }
        resp.repaint
    }

    /// 渲染一帧：clear 透明 → 抓屏 → HLSL 玻璃 → egui → Present → Commit。
    /// `run_ui` 回调里调用岛仔的 Shell 绘制逻辑。`root_scale` 只作用于主窗口的
    /// 输入/输出换算（设置里的「岛体大小」），不碰全局 zoom。
    pub fn render(&mut self, window: &winit::window::Window, origin: (f32, f32),
                  params: &GlassParams, input: egui::RawInput, root_scale: f32,
                  mut run_ui: impl FnMut(&egui::Context, &mut egui::Ui)) -> FrameStats {
        // 重建触发条件（社区「持续自愈状态机」）：
        //   ① 拓扑 epoch 变化；② 上次重建失败后的退避到期；③ 帧尺寸与 bg 纹理不匹配。
        // 三者任一即重建，且失败会继续退避重试，不再一次失败就永久 latch。
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let hwnd = match window.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(h)) => HWND(h.hwnd.get() as _),
            _ => HWND(std::ptr::null_mut()),
        };
        // 窗口所在屏变了（跨屏拖动）：WGC 会话必须重建，否则还抓旧屏。
        let current_origin = {
            use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST, GetMonitorInfoW, MONITORINFO};
            let m = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if unsafe { GetMonitorInfoW(m, &mut mi).as_bool() } { Some((mi.rcMonitor.left, mi.rcMonitor.top)) } else { None }
        };
        let monitor_changed = current_origin.is_some() && current_origin != self.wgc_monitor_origin;
        let epoch_changed = self.wgc_epoch != crate::wgc::topo_epoch() || monitor_changed;
        let retry_due = self.wgc_retry_at.is_some_and(|t| std::time::Instant::now() >= t);
        // watchdog：稳定期外、会话已建立，但超过 1.5s 没抓到任何新帧 → 会话是
        // 「活着但不送帧」（稳定期中间建立的会话常这样），尺寸校验也无从触发，
        // 必须主动重建。社区 streaming 项目「重建后 confirm 新帧到达」同款。
        let now = std::time::Instant::now();
        let frame_stalled = !crate::wgc::settling()
            && self.wgc.is_some()
            && self.wgc_last_frame_at.is_some_and(|t| now.duration_since(t) > std::time::Duration::from_millis(1500));
        if epoch_changed || retry_due || frame_stalled {
            if frame_stalled && !epoch_changed && !retry_due {
                dlog("WGC no frame >1.5s, watchdog rebuild");
            }
            self.rebuild_wgc(hwnd);
        }
        // WGC 抓屏（GPU 侧 CopyResource）。稳定期内不抓过渡帧，沿用旧纹理；
        // app 层 paint_gpu_glass 同期回退纯色，看不到旧拓扑画面。
        // grab_into 每帧校验 src/dst 尺寸，不匹配会置 size_mismatch 并返回 false。
        let mut size_mismatch = false;
        if !crate::wgc::settling() {
            if let Some(wgc) = self.wgc.as_ref() {
                if wgc.grab_into(&self.ctx, &self.glass.bg_tex) {
                    self.wgc_ever_frame = true;
                    self.wgc_last_frame_at = Some(std::time::Instant::now());
                }
                size_mismatch = wgc.take_size_mismatch();
            }
        }
        // 帧尺寸不匹配 → 重建（不等拓扑事件，社区 streaming 项目每帧校验同款）。
        // 走 retry_at 退避：中间态反复变尺寸时不会每帧重建（避免重建风暴）。
        if size_mismatch && self.wgc_retry_at.is_none() {
            let backoff = self.wgc_retry_backoff_ms.min(2000);
            self.wgc_retry_at = Some(std::time::Instant::now() + std::time::Duration::from_millis(backoff));
            self.wgc_retry_backoff_ms = (backoff * 2).min(2000);
        }
        // 主题亮度：从刚抓到的全屏帧里取「玻璃背后那块桌面」的实时亮度，
        // 500ms 节流读回。旧 wgpu 栈每帧抓屏时算，迁移后 D3D11 栈需在这里接上，
        // 否则 material 会被钉死在启动时的壁纸平均值（偏蓝、不透）。
        // params.bounds 是窗口内物理像素(y 从底)，换算成屏幕顶原点坐标作为采样框。
        if self.wgc_ever_frame && params.bounds[2] > 4.0 && params.bounds[3] > 4.0 {
            let win_h = window.inner_size().height as f32;
            // WGC 纹理只含所抓屏，主题采样框也要用相对该屏的局部坐标。
            let (cap_x, cap_y) = self.wgc_monitor_origin.unwrap_or((0, 0));
            let bx = origin.0 - cap_x as f32 + params.bounds[0];
            let by = origin.1 - cap_y as f32 + (win_h - params.bounds[1] - params.bounds[3]);
            let bw = params.bounds[2];
            let bh = params.bounds[3];
            // 采样框比玻璃略窄、全部夹在全屏帧内。分两段：上缘 35% 和内容区 65%。
            // 展开态下半部分才是文字/数据，上缘一小条亮区不该代表整块背景——
            // 两区亮度差超过阈值时由 effective_luma 让内容区优先。
            let sx = (bx + bw * 0.05) as i32;
            let sy = by.max(0.0) as i32;
            let ex = (bx + bw * 0.95) as i32;
            let top_h = (bh * 0.35).max(8.0).min(bh.max(8.0));
            let top_y = (by + top_h) as i32;
            let bottom_y = (by + bh.max(8.0)) as i32;
            let top = (sx, sy, ex.max(sx + 1), top_y.max(sy + 1));
            let content = (sx, top_y, ex.max(sx + 1), bottom_y.max(top_y + 1));
            crate::wgc::set_theme_rects(Some(top), Some(content));
            crate::wgc::sample_theme_luma_d3d11(
                &self.device, &self.ctx, &self.glass.bg_tex,
                0, 0, self.glass.bg_w as i32, self.glass.bg_h as i32,
            );
        }
        // HLSL uniform：补上窗口原点与 flipY（WGC top-down）
        let mut u = *params;
        // WGC 帧与 D3D11 纹理同为 top-down（v=0 顶部），不要额外翻转 uv.y；
        // 窗口局部 y 已在 shader 的 sample_px 里按 extra.z 翻正。
        u.quad[0] = 0.0;
        // WGC 纹理只含所抓显示器；采样坐标必须是相对该屏左上角的局部坐标，
        // 用虚拟桌面绝对坐标跨到副屏会超出纹理被 saturate 夹边 → 背景错乱。
        let (cap_x, cap_y) = self.wgc_monitor_origin.unwrap_or((0, 0));
        u.quad[1] = origin.0 - cap_x as f32;
        u.quad[2] = origin.1 - cap_y as f32;
        u.notch[3] = 1.0; // DComp 逐像素 alpha

        // 融球水滴：app 写窗口内逻辑点（顶原点），这里换成画布 device px。
        // 关键：shader 的 p.y 从底原点算（bounds 也做了 h-by 翻转），必须把 y 翻过来：
        //   dev_y = extra.z(画布物理高) - logic_y * ppp
        // 之前漏翻导致融球被画到窗口下方，屏幕上完全看不到。
        let (mb, mb2) = crate::gpu_glass_stub::metaball_snapshot();
        let mb_ppp = if u.params[1] > 0.0 { u.params[1] } else { 1.0 };
        let canvas_h = u.extra[2];
        u.meta = [mb[0] * mb_ppp, canvas_h - mb[1] * mb_ppp,
                  mb[2] * mb_ppp, mb[3] * mb_ppp];
        u.meta2 = [mb2[0] * mb_ppp, canvas_h - mb2[1] * mb_ppp, mb2[2], mb2[3]];

        // 更新窗口信息（位置/尺寸/DPI），供 egui 生成正确的 ViewportCommand。
        egui_winit::update_viewport_info(&mut self.viewport_info, &self.egui_ctx, window, false);
        // egui pass（ctx 先克隆，避免闭包内同时借用 self 与移动 run_ui）
        let ctx = self.egui_ctx.clone();
        let egui_output = run_ui_031(&ctx, input, |ui| run_ui(&ctx, ui));
        // run_ui 之后统一刷新本帧快照：先岛体/形状/染色（收起动画时逐帧变的 body、
        // 肩部），再控件透镜。两者在 render 之前填都会读到上一帧，表现分别是玻璃边缘
        // 扫过常驻图标、以及宠物与时间之间残留一条透镜白边。
        refresh_glass_uniforms(&mut u);
        refresh_control_uniforms(&mut u);
        let platform = {
            let (ro, mut po, viewports) = super::renderer::split_output(egui_output);
            // egui 输出在放大后的逻辑点里；egui_winit 期待原生逻辑点，换回去。
            scale_root_platform_output(&mut po, root_scale);
            // 记录 egui 请求的重绘间隔（岛仔隐藏态 1fps、常态按设置 fps）
            let mut d = std::time::Duration::ZERO;
            for (_, vo) in viewports.iter() { d = d.max(vo.repaint_delay); }
            self.repaint_delay = d;
            // 处理主窗口命令（OuterPosition/Visible/SetTheme 等）——egui_winit 的
            // handle_platform_output 只处理光标/剪贴板，窗口命令必须走这里。
            if let Some(vo) = viewports.get(&egui::ViewportId::ROOT) {
                egui_winit::process_viewport_commands(&ctx, &mut self.viewport_info,
                    vo.commands.clone(), window, &mut self.actions);
            }
            // 子 viewport 的窗口命令传出给 winit_runner 应用到子窗口（设置页 Visible(true)）。
            let child_cmds = viewports.iter()
                .filter(|(id, _)| **id != egui::ViewportId::ROOT)
                .map(|(id, vo)| (*id, vo.commands.clone()))
                .collect();
            let active = viewports.keys().copied().collect();
            self.egui_winit.as_mut().unwrap().handle_platform_output(window, po);
            (ro, child_cmds, active)
        };
        // 帧率统计
        self.frames += 1;
        let elapsed = self.last_report.elapsed().as_secs_f32();
        if elapsed >= 1.0 {
            self.fps = self.frames as f32 / elapsed;
            self.frames = 0;
            self.last_report = std::time::Instant::now();
        }
        let size = window.inner_size();
        let mut device_lost = false;

        if let Some(rt) = self.rt.clone() {
            unsafe {
                self.ctx.ClearRenderTargetView(&rt, &[0.0, 0.0, 0.0, 0.0]);
                self.ctx.OMSetRenderTargets(Some(&[Some(rt.clone())]), None);
            }
            self.draw_glass(size.width, size.height, &u);
            let _ = self.egui_renderer.borrow_mut().render(&self.ctx, &rt, &self.egui_ctx, platform.0);
            // Present 之前消费截图/剪贴板动作：FLIP_DISCARD 后缓冲内容失效。
            drain_actions(self.egui_winit.as_mut().unwrap(), &mut self.actions,
                &self.device, &self.ctx, Some(&rt), egui::ViewportId::ROOT);
            let present = unsafe { self.swap.Present(1, DXGI_PRESENT(0)) };
            // 显卡重置 / 远程桌面断开时 Present 会返回 DEVICE_REMOVED/RESET。
            // 旧代码把错误丢弃，结果是永久黑屏只能重启；这里上报让事件循环重建整条 D3D 栈。
            device_lost = present == DXGI_ERROR_DEVICE_REMOVED || present == DXGI_ERROR_DEVICE_RESET;
            if device_lost { dlog(&format!("Present failed: {present:?}; device lost")); }
            unsafe { let _ = self.dcomp._device.Commit(); }
        }
        FrameStats { fps: self.fps, wgc_live: self.wgc_ever_frame,
                     viewport_commands: platform.1, active_viewports: platform.2, device_lost }
    }

    /// 材质包 DXBC 变化时重建像素着色器；编译失败回退内置玻璃 pass。
    #[cfg(feature="sdk-js")]
    fn sync_material_shader(&mut self) {
        let Some((generation, dxbc, _)) = crate::local_plugins::material_d3d11::snapshot() else {
            self.material_ps = None;
            self.material_gen = 0;
            return;
        };
        if generation == self.material_gen { return; }
        self.material_gen = generation;
        unsafe {
            let mut ps = None;
            match self.device.CreatePixelShader(&dxbc, None, Some(&mut ps)) {
                Ok(()) => self.material_ps = ps,
                Err(error) => {
                    dlog(&format!("material pixel shader FAILED: {error:?}"));
                    self.material_ps = None;
                }
            }
        }
    }

    fn draw_glass(&mut self, win_w: u32, win_h: u32, u: &GlassParams) {
        #[cfg(feature="sdk-js")]
        self.sync_material_shader();
        unsafe {
            let mut mapped = std::mem::zeroed::<D3D11_MAPPED_SUBRESOURCE>();
            if self.ctx.Map(&self.glass.cbuf, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped)).is_ok() {
                ptr::copy_nonoverlapping(u as *const GlassParams as *const u8,
                    mapped.pData as *mut u8, std::mem::size_of::<GlassParams>());
                self.ctx.Unmap(&self.glass.cbuf, 0);
            }
            #[cfg(feature="sdk-js")]
            {
                let values = crate::local_plugins::material_d3d11::active_params();
                let mut mapped = std::mem::zeroed::<D3D11_MAPPED_SUBRESOURCE>();
                if self.ctx.Map(&self.glass.plugin_cbuf, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped)).is_ok() {
                    ptr::copy_nonoverlapping(values.as_ptr() as *const u8,
                        mapped.pData as *mut u8, values.len() * std::mem::size_of::<f32>());
                    self.ctx.Unmap(&self.glass.plugin_cbuf, 0);
                }
            }
            let ctx = &self.ctx;
            ctx.VSSetShader(&self.glass.vs, None);
            #[cfg(feature="sdk-js")]
            let ps = self.material_ps.as_ref().unwrap_or(&self.glass.ps);
            #[cfg(not(feature="sdk-js"))]
            let ps = &self.glass.ps;
            ctx.PSSetShader(ps, None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.glass.cbuf.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.glass.cbuf.clone())]));
            ctx.PSSetConstantBuffers(1, Some(&[Some(self.glass.plugin_cbuf.clone())]));
            ctx.PSSetShaderResources(0, Some(&[Some(self.glass.bg_srv.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.glass.sampler.clone())]));
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0, TopLeftY: 0.0, Width: win_w as f32, Height: win_h as f32,
                MinDepth: 0.0, MaxDepth: 1.0,
            }]));
            // egui renderer 每画一个 mesh 都会 RSSetScissorRects 到该 mesh 的 clip_rect，
            // 且这个状态跨 pass / 跨帧持续有效。glass pass 画全屏三角形前必须把 scissor
            // 重置成整个 RT，否则会被上一帧 egui 最后一个 mesh 的小 clip 裁掉——表现为
            // 某些页面（最后一个 mesh 的 clip 偏小，如新建日程表单）玻璃整层消失。
            ctx.RSSetScissorRects(Some(&[RECT {
                left: 0, top: 0, right: win_w as i32, bottom: win_h as i32,
            }]));
            ctx.OMSetBlendState(&self.glass.blend, Some(&[0.0; 4]), u32::MAX);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.Draw(3, 0);
        }
    }
}

/// 轻量子窗口渲染器：普通 egui 窗口（设置页/邮件阅读器等独立 viewport），
/// 无 WGC 玻璃，用不透明 clear（或按 builder.transparent 决定）。
pub struct PlainWindow {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    swap: IDXGISwapChain1,
    rt: Option<ID3D11RenderTargetView>,
    pub egui_winit: egui_winit::State,
    /// 与主窗口共享（字体图集单例）；每窗口独立 Renderer 会导致纹理越界闪退。
    renderer: Rc<RefCell<Renderer>>,
    pub transparent: bool,
    pub viewport_info: egui::ViewportInfo,
    pub actions: egui::ahash::HashSet<egui_winit::ActionRequested>,
}

impl PlainWindow {
    pub fn new(window: &winit::window::Window, egui_ctx: &egui::Context, transparent: bool,
               device: ID3D11Device, ctx: ID3D11DeviceContext,
               renderer: Rc<RefCell<Renderer>>) -> windows::core::Result<Self> {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let RawWindowHandle::Win32(h) = window.window_handle().unwrap().as_raw() else {
            return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL));
        };
        let size = window.inner_size();
        // 从共享 device 取它所属的 factory，保证 swapchain 与主窗口同 adapter。
        let dxgi_device: IDXGIDevice = device.cast()?;
        let adapter: IDXGIAdapter = unsafe { dxgi_device.GetAdapter()? };
        let factory: IDXGIFactory2 = unsafe { adapter.GetParent()? };
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: size.width, Height: size.height,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM, Stereo: BOOL(0),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT, BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH, SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: if transparent { DXGI_ALPHA_MODE_PREMULTIPLIED } else { DXGI_ALPHA_MODE_IGNORE },
            Flags: 0,
        };
        // 普通子窗口用 Hwnd swapchain（不需 DComp）；透明用 no_redirection + premultiplied。
        let swap: IDXGISwapChain1 = unsafe { factory.CreateSwapChainForHwnd(
            &device, HWND(h.hwnd.get() as _), &desc, None, None)? };
        let tex = unsafe { swap.GetBuffer::<ID3D11Texture2D>(0)? };
        let mut rt = None;
        unsafe { device.CreateRenderTargetView(&tex, None, Some(&mut rt))? };
        let egui_winit = egui_winit::State::new(egui_ctx.clone(), egui_ctx.viewport_id(), window, None, None, None);
        Ok(Self { device, ctx, swap, rt, egui_winit, renderer, transparent, viewport_info: egui::ViewportInfo::default(), actions: Default::default() })
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 { return; }
        self.rt.take();
        unsafe { let _ = self.swap.ResizeBuffers(2, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SWAP_CHAIN_FLAG(0)); }
        let tex = unsafe { self.swap.GetBuffer::<ID3D11Texture2D>(0) };
        if let Ok(tex) = tex {
            let mut rt = None;
            unsafe { let _ = self.device.CreateRenderTargetView(&tex, None, Some(&mut rt)); }
            self.rt = rt;
        }
    }

    pub fn on_event(&mut self, window: &winit::window::Window, event: &winit::event::WindowEvent) -> bool {
        let resp = self.egui_winit.on_window_event(window, event);
        if !resp.consumed {
            if let winit::event::WindowEvent::Resized(s) = event { self.resize(s.width, s.height); }
        }
        resp.repaint
    }

    /// immediate viewport：主窗口 run_ui 时已同步跑过子窗口 UI（借 app），
    /// 这里只负责把拿到的 FullOutput 画出来 + 处理窗口命令。
    pub fn paint_output(&mut self, window: &winit::window::Window, egui_ctx: &egui::Context,
                        viewport_id: egui::ViewportId, output: egui::FullOutput) {
        egui_winit::update_viewport_info(&mut self.viewport_info, egui_ctx, window, false);
        let (ro, po, viewports) = super::renderer::split_output(output);
        if let Some(vo) = viewports.get(&viewport_id) {
            egui_winit::process_viewport_commands(egui_ctx, &mut self.viewport_info,
                vo.commands.clone(), window, &mut self.actions);
        }
        self.egui_winit.handle_platform_output(window, po);
        if let Some(rt) = self.rt.clone() {
            unsafe {
                self.ctx.OMSetRenderTargets(Some(&[Some(rt.clone())]), None);
                let clear = if self.transparent { [0.0, 0.0, 0.0, 0.0] } else { [0.1, 0.1, 0.12, 1.0] };
                self.ctx.ClearRenderTargetView(&rt, &clear);
            }
            let _ = self.renderer.borrow_mut().render(&self.ctx, &rt, egui_ctx, ro);
            drain_actions(&mut self.egui_winit, &mut self.actions, &self.device,
                &self.ctx, Some(&rt), viewport_id);
            unsafe { let _ = self.swap.Present(1, DXGI_PRESENT(0)); }
        }
    }

    /// 渲染子 viewport 的 UI。
    pub fn render(&mut self, window: &winit::window::Window, egui_ctx: &egui::Context,
                  mut input: egui::RawInput, viewport_id: egui::ViewportId,
                  mut run_ui: impl FnMut(&mut egui::Ui)) {
        // 子 viewport 的输入必须带自己的 viewport_id，否则 egui 当成 ROOT。
        input.viewport_id = viewport_id;
        egui_winit::update_viewport_info(&mut self.viewport_info, egui_ctx, window, false);
        let output = run_ui_031(egui_ctx, input, run_ui);
        let (ro, po, viewports) = super::renderer::split_output(output);
        if let Some(vo) = viewports.get(&viewport_id) {
            egui_winit::process_viewport_commands(egui_ctx, &mut self.viewport_info,
                vo.commands.clone(), window, &mut self.actions);
        }
        self.egui_winit.handle_platform_output(window, po);
        let size = window.inner_size();
        if let Some(rt) = self.rt.clone() {
            unsafe {
                self.ctx.OMSetRenderTargets(Some(&[Some(rt.clone())]), None);
                let clear = if self.transparent { [0.0, 0.0, 0.0, 0.0] } else { [0.1, 0.1, 0.12, 1.0] };
                self.ctx.ClearRenderTargetView(&rt, &clear);
            }
            let _ = self.renderer.borrow_mut().render(&self.ctx, &rt, egui_ctx, ro);
            drain_actions(&mut self.egui_winit, &mut self.actions, &self.device,
                &self.ctx, Some(&rt), viewport_id);
            unsafe { let _ = self.swap.Present(1, DXGI_PRESENT(0)); }
        }
        let _ = size;
    }
}

#[cfg(test)]
mod readback_tests {
    use super::*;

    #[test]
    fn screenshot_readback_flips_rows_and_swaps_bgra_to_rgba() {
        // 两行、每行 2 像素。D3D11 内自下而上存：第 0 行是屏幕底部。
        // 行距 12 字节（8 有效 + 4 padding），模拟 D3D11_MAPPED_SUBRESOURCE.RowPitch。
        let src = [
            // row 0（底行）：红(BGRA) / 绿(BGRA)，再 4 字节 padding
            0x00, 0x00, 0xff, 0xff, 0x00, 0xff, 0x00, 0x80, 0xee, 0xee, 0xee, 0xee,
            // row 1（顶行）：蓝(BGRA) / 白(BGRA)，再 4 字节 padding
            0xff, 0x00, 0x00, 0x40, 0xff, 0xff, 0xff, 0xff, 0xdd, 0xdd, 0xdd, 0xdd,
        ];
        let out = bgra_bottom_up_to_rgba_top_down(&src, 12, 2, 2);
        assert_eq!(out, vec![
            0x00, 0x00, 0xff, 0x40, 0xff, 0xff, 0xff, 0xff, // 顶行：蓝、白
            0xff, 0x00, 0x00, 0xff, 0x00, 0xff, 0x00, 0x80, // 底行：红、绿
        ]);
    }

    #[test]
    fn adapter_score_prefers_discrete_over_integrated() {
        let mut igpu = DXGI_ADAPTER_DESC1::default();
        igpu.VendorId = 0x8086;
        igpu.DedicatedVideoMemory = 128 * 1024 * 1024;
        let mut dgpu = DXGI_ADAPTER_DESC1::default();
        dgpu.VendorId = 0x10DE;
        dgpu.DedicatedVideoMemory = 4 * 1024 * 1024 * 1024;
        assert!(D3d11Runtime::adapter_score(&dgpu) > D3d11Runtime::adapter_score(&igpu));

        let mut intel_arc = DXGI_ADAPTER_DESC1::default();
        intel_arc.VendorId = 0x8086;
        intel_arc.DedicatedVideoMemory = 8 * 1024 * 1024 * 1024;
        assert!(D3d11Runtime::adapter_score(&intel_arc) > D3d11Runtime::adapter_score(&igpu));
    }
}
