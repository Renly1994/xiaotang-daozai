//! Migration stub for the egui 0.35 + winit + egui-directx11 (D3D11) stack.
//!
//! The original `gpu_glass.rs` is a WGSL/eframe-wgpu 0.31 paint callback. In the
//! new stack the liquid-glass optics are drawn by the HLSL layer in the D3D11
//! render loop (see egui-directx11 shaders/glass.hlsl), so this module keeps only
//! the data-shaped API that the rest of the app references, with inert bodies.
//! This lets the UI code compile against egui 0.35 before the render layer is
//! fully wired up.

use eframe::egui::Rect;

pub const MAX_CTL: usize = 13;

#[derive(Clone, Copy)]
pub struct Ctl {
    pub rect: Rect,
    pub radius: f32,
    pub mag: f32,
    pub refract: f32,
    /// 材质样式: 0=普通, 1=分栏容器(暗), 2=分栏指示器(亮)。传给 shader 做层次区分。
    pub style: u8,
}
impl Ctl {
    const fn none() -> Self { Self { rect: Rect::ZERO, radius: 0.0, mag: 1.0, refract: 1.0, style: 0 } }
}
impl Default for Ctl {
    fn default() -> Self { Self::none() }
}

/// 融球水滴搜索的 shader 参数：xy=中心(capture px,顶原点), zw=半宽半高；
/// k=融合半径, anchor_y=岛体上沿锚点 y, on=开关。app 每帧写，runtime 读进 uniform。
static METABALL: std::sync::Mutex<([f32; 4], [f32; 4])> =
    std::sync::Mutex::new(([0.0; 4], [0.0; 4]));

pub fn set_metaball(cx: f32, cy: f32, hw: f32, hh: f32, k: f32, anchor_y: f32, on: bool) {
    if let Ok(mut g) = METABALL.lock() {
        g.0 = [cx, cy, hw, hh];
        g.1 = [k, anchor_y, if on { 1.0 } else { 0.0 }, 0.0];
    }
}

pub fn metaball_snapshot() -> ([f32; 4], [f32; 4]) {
    METABALL.lock().map(|g| *g).unwrap_or(([0.0; 4], [0.0; 4]))
}

/// 本帧的岛内控件玻璃列表，由 app::paint_gpu_glass 每帧写入，
/// winit_runner::build_glass_params 读出转成 shader 的 crect/cprm。
/// 用固定数组避免每帧分配；MAX_CTL = shader cbuffer 的槽位数。
static CONTROLS: std::sync::Mutex<([Ctl; MAX_CTL], usize)> =
    std::sync::Mutex::new(([Ctl::none(); MAX_CTL], 0));

/// app 每帧发布控件玻璃几何（egui 逻辑点坐标，相对窗口）。
pub fn set_controls(ctls: &[Ctl]) {
    if let Ok(mut guard) = CONTROLS.lock() {
        let n = ctls.len().min(MAX_CTL);
        guard.0[..n].copy_from_slice(&ctls[..n]);
        guard.1 = n;
    }
}

/// 渲染循环读取本帧控件玻璃列表（egui 逻辑点坐标）。
pub fn controls_snapshot() -> ([Ctl; MAX_CTL], usize) {
    CONTROLS.lock().map(|g| *g).unwrap_or(([Ctl::none(); MAX_CTL], 0))
}

/// Opaque frame descriptor; the real HLSL path builds its own uniform buffer.
#[derive(Clone, Copy)]
pub struct GlassFrame {
    pub body: Rect,
    pub capture: Rect,
}
impl Default for GlassFrame {
    fn default() -> Self { Self { body: Rect::ZERO, capture: Rect::ZERO } }
}

/// Kept for API compatibility; no egui-wgpu callback in the new stack.
pub struct GlassCallback {
    pub frame: GlassFrame,
    pub format: u32, // placeholder for the old wgpu::TextureFormat
}

/// 背景亮度：供 liquid_k 决定深/浅材质（深材质压暗+冷蓝，浅材质通透）。
/// 优先读 wgc::THEME_LUMA——D3D11 渲染循环每 500ms 从玻璃背后的真实桌面帧采样，
/// 与旧 wgpu 栈同机制（theme_luma_commit 过滤过渡帧）；未采样到时用启动时壁纸锚值兜底。
/// 迁移初期这里只读静态壁纸平均值，把 material 钉在半深不浅 → 玻璃偏蓝不透，已修。
pub fn background_luma() -> f32 {
    if let Some(v) = std::env::var("DAOZAI_LIQUID_FORCE_LUMA").ok().and_then(|s| s.parse::<f32>().ok()) {
        return v.clamp(0.0, 1.0);
    }
    // wgc::theme_luma() 启动时被壁纸锚值 seed，之后由 D3D11 采样实时更新。
    crate::wgc::theme_luma()
}
pub fn seed_theme_luma(v: f32) {
    // 播种到 wgc 的实时亮度（app::new 里也直接调 wgc::seed_theme_luma，幂等）。
    crate::wgc::seed_theme_luma(v);
}
pub fn set_morph_scale(v: f32) {
    if let Ok(mut u) = UNIFORMS.lock() { u.morph = v; }
}
pub fn set_liquid_optics(refract_px: f32, chroma_px: f32) {
    if let Ok(mut u) = UNIFORMS.lock() { u.refract = refract_px; u.chroma = chroma_px; }
}
pub fn set_liquid_look(blur: f32, band: f32, edge: f32, op: f32) {
    if let Ok(mut u) = UNIFORMS.lock() { u.blur = blur; u.band = band; u.edge_w = edge; u.edge_op = op; }
}
/// 高光跟随鼠标强度（0~1）→ HLSL `lightcfg.x`。
///
/// 迁移到 D3D11 时漏接了这条线：`GlassUniforms.light_follow` 一直是初始的 0，
/// 于是 shader 里 `follow = saturate(lightcfg.x) = 0`，`Ld` 恒为固定左上光
/// `normalize(-0.45,-0.65)`——设置里的「高光跟随鼠标」滑杆怎么拖都没反应。
/// app 每帧从 `settings.liquid_light_follow` 同步到这里。
pub fn set_light_follow(v: f32) {
    if let Ok(mut u) = UNIFORMS.lock() { u.light_follow = v.clamp(0.0, 1.0); }
}
/// 岛体投影开关 → HLSL lightcfg.z。投影由 shader 在玻璃 pass 里画（egui 侧画会盖住玻璃）。
pub fn set_shadow(enabled: bool) {
    if let Ok(mut u) = UNIFORMS.lock() { u.shadow = if enabled { 1.0 } else { 0.0 }; }
}

pub fn frame_for(
    body: Rect,
    capture: Rect,
    _screen_px: (u32, u32),
    _ppp: f32,
    _radius: f32,
    _morph: f32,
    _tint: [f32; 3],
    _time: f32,
    _pointer: (f32, f32),
    _origin: (f32, f32),
    _lightness: f32,
    _light_follow: f32,
    _controls: &[Ctl],
    _notch: Option<(f32, f32)>,
) -> GlassFrame {
    GlassFrame { body, capture }
}

// ---------------------------------------------------------------------------
// D3D11 HLSL 桥接：app 每帧把岛体几何/材质参数写到这里，渲染循环读取生成 uniform。
// ---------------------------------------------------------------------------

/// 从 egui 点坐标转到 HLSL 需要的物理像素 uniform。
#[derive(Clone, Copy, Default)]
pub struct GlassUniforms {
    /// 岛体 rect（egui 点，相对窗口左上）
    pub body_rect: [f32; 4],
    pub radius: f32,
    pub chroma: f32,
    pub refract: f32,
    pub morph: f32,
    pub material: f32,
    pub blur: f32,
    pub band: f32,
    pub edge_w: f32,
    pub edge_op: f32,
    pub pointer: [f32; 2],
    pub light_follow: f32,
    /// 岛体投影开关（设置「岛体投影」）→ shader lightcfg.z。投影由 HLSL 画在玻璃 pass。
    pub shadow: f32,
    pub active: bool,
}

static UNIFORMS: std::sync::Mutex<GlassUniforms> = std::sync::Mutex::new(GlassUniforms {
    body_rect: [0.0; 4], radius: 0.0, chroma: 0.0, refract: 0.0, morph: 0.0, material: 0.0,
    blur: 0.0, band: 0.0, edge_w: 0.0, edge_op: 0.0, pointer: [0.0; 2],
    light_follow: 0.0, shadow: 1.0, active: false,
});

pub fn uniforms() -> GlassUniforms {
    UNIFORMS.lock().map(|u| *u).unwrap_or_default()
}

/// app 的 paint_gpu_glass 每帧写入岛体真实几何/指针/材质。
/// 液态玻璃配置弹窗开关（UI 状态，无需落盘）。
static CONFIG_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn toggle_config() { CONFIG_OPEN.fetch_xor(true, std::sync::atomic::Ordering::Relaxed); }
pub fn config_open() -> bool { CONFIG_OPEN.load(std::sync::atomic::Ordering::Relaxed) }
pub fn set_config_open(v: bool) { CONFIG_OPEN.store(v, std::sync::atomic::Ordering::Relaxed); }

/// 深/浅主题平滑值（1=浅底深字，0=深底白字），app tick 写入、paint_body 读取。
static THEME_LEVEL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub fn set_theme_level(v: f32) { THEME_LEVEL.store(v.to_bits(), std::sync::atomic::Ordering::Relaxed); }
pub fn theme_level() -> f32 { f32::from_bits(THEME_LEVEL.load(std::sync::atomic::Ordering::Relaxed)) }

pub fn set_body(rect: Rect, radius: f32, pointer: (f32, f32), material: f32) {
    if let Ok(mut u) = UNIFORMS.lock() {
        u.body_rect = [rect.left(), rect.top(), rect.width(), rect.height()];
        u.radius = radius;
        u.pointer = [pointer.0, pointer.1];
        u.material = material;
        u.active = true;
    }
}

/// 非液态皮肤每帧关掉玻璃 pass。
///
/// `set_body` 只在 `Skin::Liquid` 的绘制路径里调用；如果切到悬浮云/毛玻璃后
/// 不主动清掉 `active`，D3D11 每帧仍会拿最后一次（往往是收起态）的 body_rect
/// 继续画折射，表现为其他皮肤背景里残留一块不会随展开放大的液态玻璃。
pub fn set_active(active: bool) {
    if let Ok(mut u) = UNIFORMS.lock() {
        u.active = active;
    }
}

/// 顶栏/刘海形状参数：`[flare, flare_h, is_notch]`，第三项为 0 时画胶囊圆角矩形。
static SHAPE_PARAMS: std::sync::Mutex<[f32; 3]> = std::sync::Mutex::new([0.0; 3]);
pub fn set_shape(flare: f32, flare_h: f32, is_notch: bool) {
    if let Ok(mut s) = SHAPE_PARAMS.lock() {
        *s = [flare, flare_h, if is_notch { 1.0 } else { 0.0 }];
    }
}
pub fn shape_params() -> [f32; 3] {
    SHAPE_PARAMS.lock().map(|s| *s).unwrap_or([0.0; 3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liquid_geometry_can_be_deactivated() {
        set_body(Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(180.0, 48.0)),
                 24.0, (100.0, 44.0), 1.0);
        assert!(uniforms().active);
        assert_eq!(uniforms().body_rect, [10.0, 20.0, 180.0, 48.0]);

        set_active(false);
        assert!(!uniforms().active);
        // 关掉后几何保留但不再被渲染循环使用；再开时由下一帧 set_body 覆盖。
        assert_eq!(uniforms().body_rect, [10.0, 20.0, 180.0, 48.0]);

        set_body(Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(320.0, 260.0)),
                 12.0, (160.0, 130.0), 0.0);
        assert!(uniforms().active);
        assert_eq!(uniforms().body_rect, [0.0, 0.0, 320.0, 260.0]);
        set_active(false);
    }

    #[test]
    fn shape_params_carry_notch_flag() {
        set_shape(17.6, 16.0, true);
        assert_eq!(shape_params(), [17.6, 16.0, 1.0]);
        set_shape(0.0, 8.0, false);
        assert_eq!(shape_params(), [0.0, 8.0, 0.0]);
    }

    #[test]
    fn light_follow_is_wired_and_clamped() {
        // 回归锁：迁移期漏接这条线曾导致「高光跟随鼠标」滑杆完全无效（恒 0）。
        set_light_follow(0.0);
        assert_eq!(uniforms().light_follow, 0.0);
        set_light_follow(0.65);
        assert!((uniforms().light_follow - 0.65).abs() < 1e-6);
        // 越界值要夹到 0..=1，否则 shader 里 saturate 之外的地方可能算出怪方向。
        set_light_follow(3.0);
        assert_eq!(uniforms().light_follow, 1.0);
        set_light_follow(-1.0);
        assert_eq!(uniforms().light_follow, 0.0);
        set_light_follow(1.0);
    }
}
