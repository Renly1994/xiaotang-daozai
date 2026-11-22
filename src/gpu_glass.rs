//! 液态玻璃 GPU 通道（试验）：在 egui 的同一个 wgpu render pass 里画真光学折射。
//!
//! 光学部分借鉴 [ziuus/liquid-glass](https://github.com/ziuus/liquid-glass)（MIT）的
//! `glass-core/src/renderer/glass.wgsl`：Snell 折射、Cauchy 七波长色散、Schlick Fresnel、
//! C¹ 零斜率透镜剖面、只朝左上的镜面高光与只朝右下的接触阴影。
//! 这里按岛仔的 wgpu 24 / egui-wgpu 0.31 重写了管线与 WGSL，**没有引入任何新依赖**。
//!
//! 前提：岛仔默认跑 glow（OpenGL），只有设置里的「兼容渲染（Direct3D 12）」开启时
//! `RenderState` 才存在。没有 wgpu 设备时 `App` 会回退到 CPU 顶点色皮肤。
//!
//! 背景纹理由 GDI 抓「岛体外扩矩形」得到；岛体矩形内用上边缘像素填充，
//! 避免折射到已经画好的自己（反馈循环）。

use eframe::egui_wgpu::{self, wgpu};
use std::sync::Mutex;

/// 没有封面时的中性淡彩。
pub const NEUTRAL_TINT: [f32; 3] = [0.62, 0.66, 0.74];
/// 背景纹理固定尺寸：展开动画里 capture 每帧都变，重建纹理会闪烁并拖慢合成。
/// shader 里用同样的常量把 uv 映射到纹理左上角的有效子区。
const TEX_W: u32 = 1280;
const TEX_H: u32 = 512;

/// 光学参数与几何，全部按物理像素给。
#[derive(Clone, Copy)]
pub struct GlassFrame {
    /// 抓取矩形在屏幕上的物理坐标 x,y,w,h。
    pub capture: [f32; 4],
    /// 岛体在抓取矩形内的 x,y,w,h。
    pub bounds: [f32; 4],
    /// 圆角半径（像素）、展开程度 0..1、光标局部坐标（像素）。
    pub shape: [f32; 4],
    /// 封面主色淡彩与强度。
    pub tint: [f32; 4],
    /// 时间（秒）、折射强度、色散、亮/暗模式。
    pub optics: [f32; 4],
    /// 抓取矩形的 clip-space 坐标 x0,y0,x1,y1。
    pub clip: [f32; 4],
    /// 岛内控件玻璃：每块的 x,y,w,h（capture 内物理像素）。最多 MAX_CTL 块。
    pub controls: [[f32; 4]; MAX_CTL],
    /// 每块控件的 (圆角半径 px, 是否启用, 放大倍数, 折射倍数)。
    pub control_params: [[f32; 4]; MAX_CTL],
    /// 外观四件套：(内部模糊, 折射带宽, 白边宽度 —— 都是设备像素, 白边不透明度)。
    pub look: [f32; 4],
    /// 刘海形状：(flare px, flare_h px, 是否刘海, _)。非刘海时第 3 项为 0。
    pub notch: [f32; 4],
    /// 逻辑点 → 物理像素的比例。主题采样区域要按它换算成固定的设备像素尺寸。
    pub pixels_per_point: f32,
}

/// 一次 pass 最多画几块控件玻璃（= uniform 里 crect/cprm 的数组长度）。
pub const MAX_CTL: usize = 13;

/// 一块控件玻璃。
///
/// `mag` 是放大镜倍数（1 = 不放大）；
/// `refract` 是这块控件的折射倍数，由调用方按**短边**缩放 —— 27px 的圆片照
/// 78px 卡片的位移去弯，整个圆片采到的都是几十像素外的壁纸，看着像挖了个洞。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ctl {
    pub rect: eframe::egui::Rect,
    pub radius: f32,
    pub mag: f32,
    pub refract: f32,
}

/// 每帧挂到 egui painter 上的回调。
pub struct GlassCallback {
    pub frame: GlassFrame,
    pub format: wgpu::TextureFormat,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Uniforms {
    bounds: [f32; 4],
    params: [f32; 4],   // ior, dispersion, time, light
    extra: [f32; 4],    // radius, tex_w, tex_h, expand
    tint: [f32; 4],
    quad: [f32; 4],
    misc: [f32; 4],     // pointer_x, pointer_y, refraction, _
    crect: [[f32; 4]; MAX_CTL],
    cprm: [[f32; 4]; MAX_CTL],
    look: [f32; 4],     // blur, band, edge_w, edge_opacity
    // 刘海形状：x=flare（顶部外扩宽度，px）、y=flare_h（外扩高度，px）、
    // z=是否刘海（>0.5 生效）、w=保留。非刘海形状时 z=0，走普通圆角矩形。
    notch: [f32; 4],
}

impl Uniforms {
    fn bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                self as *const Self as *const u8,
                std::mem::size_of::<Self>(),
            )
        }
    }
}

struct GlassGpu {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    sampler: wgpu::Sampler,
    texture: Option<wgpu::Texture>,
    bind_group: Option<wgpu::BindGroup>,
    size: (u32, u32),
}

static GPU: Mutex<Option<GlassGpu>> = Mutex::new(None);
static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static GRAB_N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// 上一次成功抓到的矩形尺寸 (w<<32 | h)。
/// WGC 报「这一帧没有新画面」时，如果当前需要的矩形尺寸和它不一致，就**不能**
/// 复用旧纹理：视口已经被 egui 设成新的 capture 矩形，而 uniform 里的 tex 和
/// bounds 还是旧的，于是整块玻璃会按旧几何被画进更大的视口里 ——
/// 表现就是展开动画中「一个缩小/错位的岛仔被套在大轮廓里面」。
/// 只有尺寸一致时复用纹理才是安全的。
static LAST_CAP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// 抓到的桌面平均亮度（0..1）的 f32 位模式，供 App 决定深/浅材质。
static BG_LUMA: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f59_999a); // 0.85，未抓屏前默认浅色
/// DWM 捕获排除是否已经尝试过 / 成功设置了几个窗口。
static EXCLUSION_TRIED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static EXCLUSION_TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// 冻结诊断模式的预热帧数：前 24 帧保持排除以收集干净壁纸，之后取消排除、冻结。
static FREEZE_TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static EXCLUDED_N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// 桌面平均亮度。非 Windows / 未抓屏时给一个偏浅的默认值。
pub fn background_luma() -> f32 {
    let forced = std::env::var("DAOZAI_LIQUID_FORCE_LUMA").ok()
        .and_then(|v| v.parse::<f32>().ok()).map(|v| v.clamp(0.0, 1.0));
    if let Some(v) = forced { return v; }
    f32::from_bits(BG_LUMA.load(std::sync::atomic::Ordering::Relaxed))
}


/// GPU 通道已经成功跑起来：此时兜底底色可以几乎透明，让折射层自己说话。
pub fn gpu_ready() -> bool {
    !FAILED.load(std::sync::atomic::Ordering::Relaxed)
        && TICK.load(std::sync::atomic::Ordering::Relaxed) > 0
}

fn diag(message: &str) {
    use std::io::Write;
    let dir = crate::model::config_dir().join("diagnostics");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("liquid-glass.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 256 * 1024) { let _ = std::fs::write(&path, b""); }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{}", message);
    }
}

const SHADER: &str = r#"
struct U {
    bounds : vec4<f32>,
    params : vec4<f32>,
    extra  : vec4<f32>,
    tint   : vec4<f32>,
    quad   : vec4<f32>,
    misc   : vec4<f32>,
    // 岛内控件玻璃：crect = (x, y, w, h)（capture 内物理像素），
    // cprm = (圆角半径, 是否启用, 放大倍数, 折射倍数)。
    crect  : array<vec4<f32>, 13>,
    cprm   : array<vec4<f32>, 13>,
    // 外观四件套：(内部模糊 px, 折射带宽 px, 白边宽度 px, 白边不透明度)。
    look   : vec4<f32>,
    // 刘海形状：(flare px, flare_h px, 是否刘海, _)。
    notch  : vec4<f32>,
}
@group(0) @binding(0) var<uniform> u : U;
@group(0) @binding(1) var bg : texture_2d<f32>;
@group(0) @binding(2) var samp : sampler;

struct VOut {
    @builtin(position) pos : vec4<f32>,
    @location(0) uv : vec2<f32>,
}

@vertex
fn vs(@builtin(vertex_index) i : u32) -> VOut {
    var xy = array<vec2<f32>, 6>(
        vec2<f32>(u.quad.x, u.quad.y), vec2<f32>(u.quad.z, u.quad.y), vec2<f32>(u.quad.x, u.quad.w),
        vec2<f32>(u.quad.x, u.quad.w), vec2<f32>(u.quad.z, u.quad.y), vec2<f32>(u.quad.z, u.quad.w)
    );
    var uv = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0)
    );
    var o = VOut(vec4<f32>(0.0), vec2<f32>(0.0));
    o.pos = vec4<f32>(xy[i], 0.0, 1.0);
    o.uv = uv[i];
    return o;
}

fn rr_sdf(p : vec2<f32>, b : vec2<f32>, r : f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

// 刘海轮廓 SDF：与 shape::sections 的几何一致 —— 顶部 flare_h 高度内，半宽按
// flare*(1-t)^2.4 向外柔和外扩（两侧"肩部"），下方是普通圆角矩形（只有底部两角
// 圆角，顶部是直角，因为肩部已经从顶边外扩出去了）。
fn notch_sdf(p : vec2<f32>, b : vec2<f32>, r : f32, flare : f32, fh : f32) -> f32 {
    // t: 0 在顶边、1 在 flare 带底部。
    let t = clamp((p.y + b.y) / max(fh, 1.0), 0.0, 1.0);
    let wing = flare * pow(1.0 - t, 2.4);
    let hx = b.x + wing;
    // 底部圆角带（只有下半部分需要圆角）。
    if (p.y > b.y - r) {
        let qx = max(abs(p.x) - (b.x - r), 0.0);
        let qy = abs(p.y) - (b.y - r);
        return length(vec2<f32>(qx, max(qy, 0.0))) - r;
    }
    // 其余部分：水平半宽 hx(y)（含肩部），垂直边界 ±b.y。
    return max(abs(p.x) - hx, abs(p.y) - b.y);
}

// 逐像素伪随机（0..1）。只用于抖动，不需要高质量分布。
fn hash21(p : vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(0.1031, 0.1030));
    q = q + vec2<f32>(dot(q, q.yx + vec2<f32>(33.33)));
    return fract((q.x + q.y) * q.x);
}

// 纹理固定 1280x512，只写左上角 (tw,th)；px 是 capture 内的物理像素坐标。
fn sample_px(px : vec2<f32>) -> vec3<f32> {
    // FLAT=2：用棋盘格当背景，折射位移、色散、越界都会直接显形。
    if (u.misc.w > 1.5) {
        // 20px 网格 + 1.6px 抗锯齿细线：硬棋盘本身的格子边缘也是一级级像素，
        // 很容易被当成「玻璃锯齿」，换成带 AA 的网格线就不会混淆。
        let q = px / 20.0;
        let dl = min(abs(fract(q.x) - 0.5), abs(fract(q.y) - 0.5)) * 20.0;
        let ln = 1.0 - smoothstep(0.6, 1.8, dl);
        return mix(vec3<f32>(0.80, 0.84, 0.90), vec3<f32>(0.18, 0.22, 0.30), ln);
    }
    let tex = vec2<f32>(u.extra.y, u.extra.z);
    let scale = vec2<f32>(u.extra.y / 1280.0, u.extra.z / 512.0);
    let uv = clamp(px / tex, vec2<f32>(0.001), vec2<f32>(0.999)) * scale;
    return textureSampleLevel(bg, samp, uv, 0.0).rgb;
}

// 分通道 5 点十字平均：R 采一侧、B 采另一侧，颜色差就是色散彩边。
fn tap5(px : vec2<f32>, ca : vec2<f32>, blur : f32) -> vec3<f32> {
    var off = array<vec2<f32>, 5>(
        vec2<f32>(0.0, 0.0), vec2<f32>(blur, 0.0), vec2<f32>(-blur, 0.0),
        vec2<f32>(0.0, blur), vec2<f32>(0.0, -blur)
    );
    var s = vec3<f32>(0.0);
    for (var i : i32 = 0; i < 5; i = i + 1) {
        let q = px + off[i];
        s = s + vec3<f32>(sample_px(q + ca).r, sample_px(q).g, sample_px(q - ca).b);
    }
    return s / 5.0;
}

// 岛内控件玻璃：岛体那块玻璃的「小尺寸版本」。
//
// 关键前提：控件底下没有别的纹理可采 —— 抓屏抓到的是**桌面**，不是已经画好的面板。
// 所以控件玻璃折射的仍然是桌面，只是透镜更小更紧，读起来就像「贴在玻璃上的一小块
// 更厚的玻璃」。材质、色散、倒角、导光全都沿用岛体那一套，只是：
//   · 折射带按**短边**缩放（岛体按高度，控件的高度往往很小，不能照搬）
//   · 材质照抄岛体，只多 1% 提亮 —— 层次感交给边缘
//   · 白边更细更亮（岛体 3px，控件 1.4px）
fn ctrl_glass(p : vec2<f32>, local : vec2<f32>, hs : vec2<f32>, radius : f32, d : f32, mat : f32, Ld : vec2<f32>, edge_bg : vec3<f32>, mag : f32, refr : f32) -> vec4<f32> {
    let e = 1.0;
    let gx = rr_sdf(local + vec2<f32>(e, 0.0), hs, radius) - rr_sdf(local - vec2<f32>(e, 0.0), hs, radius);
    let gy = rr_sdf(local + vec2<f32>(0.0, e), hs, radius) - rr_sdf(local - vec2<f32>(0.0, e), hs, radius);
    let g = normalize(vec2<f32>(gx, gy) + vec2<f32>(1e-6));

    let short = max(min(hs.x, hs.y), 1.0);
    // 控件自带带宽（按短边缩放）：色散带的深浅不能跟岛体那 130px 走，
    // 否则一块 78px 高的卡片整块都在折射带里。外观页的「折射带宽」只管岛体。
    let band = clamp(short * 1.30, 10.0, 72.0);
    let t = clamp(-d / band, 0.0, 1.0);
    let k = pow(1.0 - pow(t, 0.30), 2.0);
    // 和岛体共用同一对设置项（折射 / 色散），但控件小得多 —— 同样的像素位移在
    // 它上面占比更大，所以先打 0.55 折，再乘每块控件自己的 `refr`
    // （27px 的圆片不能按 78px 卡片的位移弯，否则整片采到的都是几十像素外的壁纸）。
    let strength = max(u.misc.z, 0.0) * 0.55 * max(refr, 0.05);
    let cxn = clamp((abs(local.x) - (hs.x - radius)) / max(radius, 1.0), 0.0, 1.0);
    let cyn = clamp((abs(local.y) - (hs.y - radius)) / max(radius, 1.0), 0.0, 1.0);
    let chroma = max(u.params.x, 0.0) * 0.55 * max(refr, 0.05) * mix(0.60, 1.0, cxn * cyn);
    // 放大镜：采样点朝控件中心压缩，背景在玻璃里被放大一点点。
    // 指示器类给 1.12、小开关珠 1.25；我们用 1.10 左右，小圆片才有「透镜感」。
    let m = max(mag, 1.0);
    let q = p - local * (1.0 - 1.0 / m);
    let bp = q - g * k * strength;

    // 边缘颗粒修复：折射带最深处会把壁纸上的小亮斑（气泡、星点）拉进边里，
    // 看起来像边缘发毛。采样步长随折射深度加大，把拉进来的特征糊开。
    var col = tap5(bp, -g * k * chroma, u.look.x * 0.80 + k * 1.2);
    // 材质必须和岛体**完全一致**，否则控件会明显比面板亮一档 —— 实测漏掉下面
    // 这两步时，浅色壁纸下卡片内部 240、面板 197，看起来是一块白纸而不是玻璃。
    // （亮暗反相补偿 + 逐像素 scrim，都照抄 fs 里的岛体版本。）
    let lum_c = dot(col, vec3<f32>(0.299, 0.587, 0.114));
    col = col * (1.0 + mix(0.045, -0.035, smoothstep(0.42, 0.88, lum_c)));
    let bg_l = dot(edge_bg, vec3<f32>(0.299, 0.587, 0.114));
    col = mix(col, vec3<f32>(0.30), smoothstep(0.48, 0.96, bg_l) * 0.16 * mat);
    // 抬起感：比岛体略亮一点点。幅度必须小 —— 大了就变成「贴了一块浅色纸」。
    col = col * 1.010 + vec3<f32>(0.004, 0.006, 0.010);
    // 深色材质的压暗系数和岛体**一模一样**（0.58 / 0.70）。第一版给控件用了
    // 更轻的 0.72 / 0.55 想让它们「浮起来」，结果是卡片比面板亮 20%，像贴了一块
    // 灰纸。玻璃叠玻璃的层次应该只由**边缘**（白线 + 倒角暗带 + 导光）表达。
    col = mix(col, col * 0.58 + vec3<f32>(0.010, 0.014, 0.020), vec3<f32>((1.0 - mat) * 0.70));
    // 顶部冷光：玻璃厚度。
    let topk = 1.0 - smoothstep(0.0, max(hs.y * 2.0, 1.0), local.y + hs.y);
    col = col + vec3<f32>(0.06, 0.08, 0.11) * topk * 0.16;
    // 倒角暗带（比岛体更贴近边缘、更窄）。
    let tq = (d + 1.7) / 0.80;
    let bevel = exp(-tq * tq);
    col = col * (1.0 - bevel * 0.10);
    // 向内导光：衰减长度取控件短边的一半以内，小控件才有一圈"透亮"。
    let t_in = max(-d, 0.0);
    let bleed = exp(-t_in / clamp(short * 0.45, 5.0, 22.0)) * (1.0 - bevel * 0.5);
    let lobe = pow(max(dot(g, Ld), 0.0), 1.5);
    col = col + vec3<f32>(0.62, 0.86, 0.96) * bleed * (lobe * 0.15 + 0.055);
    // 白边：峰值在内侧，宽度取「白边宽度」设置项（σ = 宽度×0.42，1.5px → 0.63，
    // 和 1.5px 线性边的观感等价，但带抗锯齿）。
    let sigma_c = max(u.look.z, 0.5) * 0.42;
    let lq = (d + sigma_c * 0.65) / sigma_c;
    let line = exp(-lq * lq);
    let dirw = mix(0.55, 1.0, pow(max(dot(g, Ld), 0.0), 1.3));
    let line_v = line * dirw;
    let edge_a = clamp(u.look.w, 0.0, 1.0);
    col = mix(col, vec3<f32>(1.0), clamp(line_v * edge_a, 0.0, 1.0));
    let aa = 1.0 - smoothstep(-1.0, 1.0, d);
    return vec4<f32>(col, clamp(max(aa, line_v * edge_a), 0.0, 1.0));
}

// 选中的控件玻璃：SDF 距离、半尺寸、圆角、下标（>=13 = 没命中）、放大与折射倍数。
struct CtlSel { d : f32, hs : vec2<f32>, r : f32, idx : u32, mag : f32, refr : f32 }

// 命中测试：**后一个覆盖前一个**（音量浮层必须压在歌词卡上面），
// 所以是「命中就覆盖」，不是取 SDF 最小值。
fn pick_ctl(p : vec2<f32>) -> CtlSel {
    var out = CtlSel(0.0, vec2<f32>(1.0), 0.0, 13u, 1.0, 1.0);
    for (var ci = 0u; ci < 13u; ci = ci + 1u) {
        if (u.cprm[ci].y < 0.5) { continue; }
        let cr = u.crect[ci];
        // 控件不需要岛体那个 3px 内缩（那是为了 SetWindowRgn 的硬裁剪），
        // 只留半个像素让抗锯齿完全落在矩形内，边缘位置就和 egui 给的矩形重合。
        let hs = max(cr.zw * 0.5 - vec2<f32>(0.5), vec2<f32>(1.0));
        let rr = max(min(u.cprm[ci].x - 0.5, min(hs.x, hs.y)), 0.0);
        let dd = rr_sdf(p - cr.xy - cr.zw * 0.5, hs, rr);
        if (dd < 0.0) { out = CtlSel(dd, hs, rr, ci, u.cprm[ci].z, u.cprm[ci].w); }
    }
    return out;
}

@fragment
fn fs(i : VOut) -> @location(0) vec4<f32> {
    let tex = vec2<f32>(u.extra.y, u.extra.z);
    let p = i.uv * tex;
    // 形状整体内缩 1.2px：SetWindowRgn 的裁剪是每像素硬边、没有抗锯齿，
    // 玻璃边缘正好压在区域边界上时，半透明像素会被切掉 → 阶梯锯齿。
    // 内缩后抗锯齿完整落在窗口区域内，区域只裁到已经完全透明的部分。
    // inset 必须大于「白边半宽(1.6) + aa 半宽(0.8)」，否则白边外半部分被区域裁掉，
    // 边缘会变成 1px 硬跳变（实测 y:43 → 181，跨 138）—— 这就是看到的锯齿。
    let inset = 3.0;
    let half_size = max(u.bounds.zw * 0.5 - vec2<f32>(inset), vec2<f32>(2.0));
    let local = p - u.bounds.xy - u.bounds.zw * 0.5;
    let radius = max(min(u.extra.x - inset, min(half_size.x, half_size.y)), 0.0);
    // 刘海形状用 notch_sdf（顶部两侧肩部外扩），其余形状仍是圆角矩形。
    let d = select(rr_sdf(local, half_size, radius),
                   notch_sdf(local, half_size, radius, u.notch.x, u.notch.y),
                   u.notch.z > 0.5);
    let mat = clamp(u.params.w, 0.0, 1.0);

    // 岛体外：SetWindowRgn 的窗口区域比岛体大一圈（展开时左右各约 38px），
    // 而兼容渲染的窗口底是不透明黑。这里必须把**背后的桌面**画回去，
    // 否则这圈余量会变成明显的黑边/黑色背景 —— 之前涂近黑就是这么来的。
    if (d > inset - 0.25) {
        // DComp 有真实 alpha：岛体外直接透明，让桌面自己透出来。
        if (u.notch.w > 0.5) { return vec4<f32>(0.0); }
        let bg = sample_px(p);
        // 紧贴边缘留一点极淡的接触阴影，玻璃才不像凭空浮着。
        let sh = (1.0 - smoothstep(0.0, 7.0, d - 1.0)) * mix(0.06, 0.02, mat);
        let col = bg * (1.0 - sh) + vec3<f32>(0.05, 0.06, 0.07) * sh;
        return vec4<f32>(col, 1.0);
    }

    // 1.2px 的抗锯齿足够平滑；再宽会在实时模式下露出滞后 1~2 帧的桌面色，
    // 边缘反而显得毛躁。
    let aa = 1.0 - smoothstep(-1.2, 1.2, d);

    // 光源方向跟着指针走。桌面没有陀螺仪，指针是唯一能表达「用户正在动」的输入：
    // Apple 的镜面高光随设备倾斜移动，这里把指针当成虚拟光源 —— 指针在哪一侧，
    // 哪一侧的边缘光就亮。指针离岛心很近时退回默认的左上光，避免方向在中心乱跳。
    // 限制在上半圈：指针只控制**水平摆动**，垂直方向永远保持「光从上方来」。
    // 原因是岛仔贴在屏幕顶部，鼠标绝大多数时间在它下面 —— 真跟随的话静止状态
    // 会一直是「从下往上打光」，观感很别扭。人类对光源在上方有本能预期。
    // 放在这里而不是玻璃光学那一段，是因为诊断模式（FLAT）也要用它画控件玻璃。
    let center = u.bounds.xy + u.bounds.zw * 0.5;
    let to_ptr = u.misc.xy - center;
    let plen = length(to_ptr);
    let swung = to_ptr / max(plen, 1e-3);
    let up_bias = vec2<f32>(swung.x, min(swung.y, -0.18));
    let Ld = normalize(mix(normalize(vec2<f32>(-0.45, -0.65)), up_bias, smoothstep(3.0, 48.0, plen)) + vec2<f32>(1e-6));

    // 背后未折射的桌面色：控件玻璃的材质补偿、局部 scrim 和最后的边缘混合都要用。
    let edge_bg = sample_px(p);

    // 对齐诊断：DAOZAI_LIQUID_FLAT=1 时玻璃内原样输出抓到的背景。
    // 如果它与岛体外的壁纸无缝衔接，说明抓屏坐标与屏幕一致；错位就说明坐标有偏。
    // 控件玻璃这里也要画 —— 棋盘格（FLAT=2）是唯一能直接看出「控件边界对不对、
    // 折射带有多宽」的手段。
    if (u.misc.w > 0.5) {
        var flat_col = sample_px(p);
        let cs = pick_ctl(p);
        if (cs.idx < 10u) {
            let cr = u.crect[cs.idx];
            let ce = ctrl_glass(p, p - cr.xy - cr.zw * 0.5, cs.hs, cs.r, cs.d, mat, Ld, edge_bg, cs.mag, cs.refr);
            flat_col = mix(flat_col, ce.rgb, ce.a);
        }
        let aa = 1.0 - smoothstep(-1.2, 1.2, d);
        let a = select(1.0, aa, u.notch.w > 0.5);
        return vec4<f32>(flat_col * a, a);
    }

    // SDF 梯度指向边缘外侧。
    let e = 1.0;
    let gx = rr_sdf(local + vec2<f32>(e, 0.0), half_size, radius) - rr_sdf(local - vec2<f32>(e, 0.0), half_size, radius);
    let gy = rr_sdf(local + vec2<f32>(0.0, e), half_size, radius) - rr_sdf(local - vec2<f32>(0.0, e), half_size, radius);
    let g = normalize(vec2<f32>(gx, gy) + vec2<f32>(1e-6));

    // 折射带：边缘弯得最狠，往里 t 增大、k 迅速衰减。
    // band 上限原来卡在 72 —— 实测展开态面板高 210px 时只有外侧 72px 参与折射，
    // 内侧 138px（66% 的高度）k≈0，等于 1:1 复制。这就是「小胶囊像玻璃、
    // 大面板像半透明贴纸」的根因。上限提到 130，strength 同步放宽到 78。
    // 展开/折叠过渡：形变过程中折射带略微加宽、色散随之增强，玻璃看起来像被
    // 「拉」开再稳住。morph 是「展开进度的变化速度」（0..1，带衰减），
    // 静止时恒为 0，所以折叠态和展开态完全不受影响。
    // 注意系数不能大：band 决定折射**影响多深**，对 210px 高的展开面板来说
    // ×1.55 会让折射带从 130 涨到 200，几乎覆盖整块面板 —— 背景被整体吸走，
    // 表现为「过渡中面板内部突然变白、脑袋往下滑然后被盖住」。
    // 所以 band 只给一点点（0.08），主要加在 strength（边缘弯得更狠但不扩散）。
    let ms = max(u.params.z, 0.0);
    let morph = select(clamp(u.extra.w, 0.0, 1.0) * ms, ms, u.params.y > 0.5);
    // 折射带宽：设置项直接给（设备像素）。原来 `clamp(bounds.w*0.80, 30, 130)` 在
    // 展开面板上饱和到 130 —— 87 CSS px @1.5x 屏，所以默认值就是那个观感。
    let band = max(u.look.y, 1.0) * (1.0 + morph * 0.08);
    let t = clamp(-d / band, 0.0, 1.0);
    let k = pow(1.0 - pow(t, 0.30), 2.0);

    // 关键修复：折射方向朝玻璃**内部**（-g）。之前朝外，边缘会把岛体外的
    // 壁纸折进来，玻璃里就出现「不属于该区域的壁纸像素块」，贴屏顶时还会
    // 越界 clamp 成一条拉伸色带。
    //
    // 折射强度：直接取设置项（设备像素），形变期间再乘一点增益。以前是
    // `clamp(bounds.w*0.90, 26, 78)` —— 那个自适应在展开面板上饱和到 78，
    // 和 48 CSS px（面板 1.5x 屏 = 72 设备 px）本来就是一个量级，
    // 改成绝对值只是把「拧不动」变成「拧得动」。
    let strength = max(u.misc.z, 0.0) * (1.0 + morph * 0.15);
    // 色散：默认 chroma=12 CSS px（设备 px 下 R/B 相距 48px），而且**整圈边缘一视同仁**。
    // 我们原来把直边收到 0.22 倍（只剩 0.57px，肉眼等于没有），现在只收到 0.60：
    // 直边留一点彩边、圆角保持完整色散，观感更稳。
    let cxn = clamp((abs(local.x) - (half_size.x - radius)) / max(radius, 1.0), 0.0, 1.0);
    let cyn = clamp((abs(local.y) - (half_size.y - radius)) / max(radius, 1.0), 0.0, 1.0);
    let corner = cxn * cyn;
    let chroma = max(u.params.x, 0.0) * mix(0.60, 1.0, corner);
    let refr = -g * k * strength;
    let ca = -g * k * chroma;
    let bp = p + refr;

    var col = tap5(bp, ca, max(u.look.x, 0.0) + k * 1.6);
    // 玻璃比空气「多一层」，但这点提亮必须极轻。以前这里是四层白色叠加：
    // +0.004 冷色、10% 白雾、顶部冷光、外加一圈白边，浅色壁纸下整体被推成
    // 一片发白的雾 —— 玻璃感就是这么没的（实测岛内亮度只比壁纸高 8，却全糊了）。
    col = col * 1.004 + vec3<f32>(0.002, 0.003, 0.006);

    // 顶部一点冷色提亮，做出玻璃厚度（幅度从 0.18 砍到 0.10）。
    let topk = 1.0 - smoothstep(0.0, max(u.bounds.w, 1.0), p.y - u.bounds.y);
    col = col + vec3<f32>(0.05, 0.07, 0.10) * topk * 0.10;

    // 材质：不再无脑「加白」。亮背景下加白会把对比彻底压平 —— 实测背景亮度
    // 250 时白色高光只剩 +5 的余量，玻璃等于不存在（背景 60 时有 +124）。
    // 改成按背景亮度**反相补偿**：
    //   暗背景(<0.42) → 轻微提亮，材质才有存在感
    //   亮背景(>0.88) → 轻微压暗，把高光的对比空间腾出来
    let lum_m = dot(col, vec3<f32>(0.299, 0.587, 0.114));
    let lift = mix(0.045, -0.035, smoothstep(0.42, 0.88, lum_m));
    col = col * (1.0 + lift);

    // 深色材质：保留背景明暗层次，但必须明显比背景沉。0.70 太轻（实测只暗 5.9，
    // 几乎看不出来），0.58 才能让深色玻璃成为「第二层材质」而不是一层蓝雾。
    col = mix(col, col * 0.58 + vec3<f32>(0.010, 0.014, 0.020), vec3<f32>((1.0 - mat) * 0.70));
    // 极淡冷白雾，只用来统一色相，不再承担材质感
    col = mix(col, vec3<f32>(0.96, 0.97, 1.0), vec3<f32>(mat * 0.02));

    // 局部 scrim：决定文字可读性的不是「整块背景的平均亮度」，而是**每个像素
    // 背后**的亮度。这里按像素取未折射的背景亮度，亮的地方多压、暗的地方几乎
    // 不动 —— 把文字背后的明暗落差压小，而不是把整块玻璃统一调暗（统一调暗会
    // 让深色区一起变灰，反而更闷）。只对浅色材质生效：深色材质本来就是白字。
    let bg_l = dot(edge_bg, vec3<f32>(0.299, 0.587, 0.114));
    let scrim = smoothstep(0.48, 0.96, bg_l) * 0.16 * mat;
    col = mix(col, vec3<f32>(0.30), scrim);

    // 白雾会洗掉饱和度，补回来一点，玻璃才有「透过一层介质」的颜色感。
    let lum0 = dot(col, vec3<f32>(0.299, 0.587, 0.114));
    col = mix(vec3<f32>(lum0), col, 1.06);

    // 倒角暗带：光线在倒角处被折走，紧贴高光内侧**必然**有一条很窄的暗线 ——
    // 这是厚玻璃的固有特征，不是瑕疵。位置贴着倒角（2.2px）、σ≈0.9（约 2px 宽），
    // 只压 8%：再深就变成「给高光画一条截止线」，那是上一版的问题。
    let tq = (d + 2.2) / 0.9;
    let bevel = exp(-tq * tq);
    col = col * (1.0 - bevel * 0.08);

    // 向内延伸的柔光（导光）：光从倒角进入玻璃后被全内反射困住，再逐步泄漏出来。
    // 两个物理修正：
    //   ① 衰减长度取决于**玻璃厚度**，跟岛体多高无关 —— 上一版用
    //      clamp(h*0.35, 8, 26) 是视觉补偿，物理上不成立，改回固定 16px。
    //   ② 它必须被倒角暗带抑制，而不是简单叠加：同一套机制（那里的光被折走了），
    //      所以乘 (1 - bevel*0.7)，光在倒角处让位、过暗带后再铺开。
    let t_in = max(-d, 0.0);
    let bleed = exp(-t_in / 16.0) * (1.0 - bevel * 0.55);
    // Beer-Lambert 吸收：视线穿过玻璃的路径长度在**倒角处最长**（越靠近边缘越
    // 斜着穿过玻璃），所以边缘会比中间更「滤色」—— 真实厚玻璃那圈青绿边就是
    // 这么来的。
    // 关键：这一层是**乘**在透过玻璃的背景上，不是往导光里加色。
    // 之前只给导光加色偏，等于把颜色加在光最暗的地方，实测只有 1~2 级、看不见；
    // 乘在背景上则亮暗背景都成立，边缘 R 通道最多被压掉 7%。
    let path = exp(-t_in / 7.0);
    col = col * mix(vec3<f32>(1.0), vec3<f32>(0.90, 1.0, 0.96), path * 0.65);

    // 导光本身也带一点吸收色（路径更长、更青），但它是加光，深度衰减后
    // 基本看不见 —— 保留它是为了物理自洽，不是为了观感。
    let absorb = 1.0 - exp(-t_in / 10.0);
    let lcol = mix(vec3<f32>(0.88, 0.93, 1.0), vec3<f32>(0.52, 0.90, 0.80), absorb);

    // 三个分量叠加，而不是只留迎光侧：
    //   ① 迎光侧：光直接照进来，最亮
    //   ② 背光侧：光从迎光面进入玻璃后被内反射「绕」到对面再漏出 ——
    //      这就是真实玻璃背光边也有一圈暗光的来源。指数放软（1.2）让它更散。
    //   ③ 环境项：桌面周围漫射进玻璃的光，与方向无关。
    // 只做 ① 的时候，鼠标在下方就是「上半截有光、下半截死平」，中间一条硬分界。
    let lobe_a = pow(max(dot(g, Ld), 0.0), 1.6);
    let lobe_b = pow(max(dot(g, -Ld), 0.0), 1.2);
    col = col + lcol * bleed * (lobe_a * 0.16 + lobe_b * 0.085 + 0.050);
    col = col + u.tint.rgb * u.tint.a * 0.05 * bleed;

    // 高光：玻璃质感的命门。必须是**一条锐利的亮线**，不是一圈雾。
    // 之前 exp(-d²/3.61)（σ=1.9，有效宽约 8px）把高光摊成了雾：实测 45px 高的
    // 胶囊上边缘有整整 10px 是白的（255,255,255,252,244,231,220,213…），
    // 上下各 10px 就吃掉 44% 的高度，观感自然从「玻璃」变成「磨砂塑料」。
    // 现在峰值落在边缘内侧 0.7px、σ≈1.15（有效宽约 3px）；整条线都在形状
    // 内部，所以不影响外缘覆盖率，外缘依旧交给 aa，不会退回 1px 硬跳。
    // 白边宽度 / 亮度都来自设置项：1.5px →
    // σ0.82，与旧写死值一致；0.9 不透明度 → 0.62(暗背景)~0.92(亮背景) 也一致。
    let sigma = max(u.look.z, 0.3) * 0.365;
    let lq = (d + sigma * 0.55) / sigma;
    let line = exp(-lq * lq);
    let gq = (d + 1.0) / max(3.2 * u.look.z / 1.5, 0.8);
    let glow = exp(-gq * gq) * 0.04;
    // 高光**沿边缘方向调制**：迎光侧最亮，背光侧只留一条淡淡的边。
    // 这一步才是「液态感」的关键 —— 把 line 均匀铺满整圈，看起来只是一圈描边；
    // 让它的强度随光源方向变化，才像一块会反光的玻璃。
    // 实测（指针左/右/下三帧）：只加方向性的 rim 时侧边亮度差只有 3.6，
    // 调制 line 之后同一对比变成 19.6，肉眼才看得出光在动。
    let dirw = mix(0.40, 1.0, pow(max(dot(g, Ld), 0.0), 1.5));
    let line_v = line * dirw;
    // 高光权重也要跟背景走：暗背景下 0.62 就够亮（184 vs 背景 60，很抢眼），
    // 但亮背景下材质被压到 231、背景 250，权重不够高光反而比背景还暗，
    // 会看成一条「暗边」。亮背景拉到 0.92 并用纯白，才顶得到 253 附近。
    // 设置项 edgeOpacity 乘在这条曲线上（0.9 = 旧默认，0 = 整条白边关掉）。
    let edge_a = clamp(u.look.w, 0.0, 1.0);
    let line_w = edge_a * mix(0.69, 1.02, smoothstep(0.60, 0.92, lum_m));
    col = mix(col, vec3<f32>(1.0), clamp(line_v * line_w + glow, 0.0, 1.0));

    // ---------- 岛内控件玻璃 ----------
    // 一次抓屏、一次 pass 画多块玻璃：所有控件与岛体共用同一张背景纹理，各自再跑
    // 一遍小尺寸透镜。这样控件边缘也有真实折射与白边，而不是贴一层半透明色块。
    let cs = pick_ctl(p);
    if (cs.idx < 10u) {
        let cr = u.crect[cs.idx];
        let ce = ctrl_glass(p, p - cr.xy - cr.zw * 0.5, cs.hs, cs.r, cs.d, mat, Ld, edge_bg, cs.mag, cs.refr);
        col = mix(col, ce.rgb, ce.a);
    }

    // 关键：边缘必须和「背后的桌面」混合。兼容渲染的窗口不透明，
    // 输出预乘 col*alpha 在 aa→0 时会变成纯黑，岛体轮廓外就是一圈黑线
    // （圆角处最明显）。这里直接 mix 桌面色，边缘永远不会有黑。
    // 覆盖率取 max(aa, line*0.9)：高光自带覆盖率，不能被 aa 归零，
    // 否则白边的外半部分会被 mix(edge_bg, col, 0) 整个丢弃，边缘只剩 1px 硬跳。
    let cov = clamp(max(aa, line_v * edge_a), 0.0, 1.0);
    // 抖动：8-bit 输出在平滑渐变上必然出色带（深色壁纸尤其明显）。加 ±1 级噪声
    // 把量化台阶打散，幅度压在 1/255 量级 —— 只让渐变更顺，看不到噪点。
    let dth = (hash21(p) - 0.5) * (1.7 / 255.0);
    let out = mix(edge_bg, col, vec3<f32>(cov)) + vec3<f32>(dth);
    if (u.notch.w > 0.5) {
        return vec4<f32>(out * cov, cov);
    }
    return vec4<f32>(out, 1.0);
}
"#;

fn make_gpu(device: &wgpu::Device, format: wgpu::TextureFormat) -> Option<GlassGpu> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| make_gpu_inner(device, format))).ok()
}

fn make_gpu_inner(device: &wgpu::Device, format: wgpu::TextureFormat) -> GlassGpu {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("daozai-liquid-glass"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("daozai-glass-layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("daozai-glass-pipeline-layout"),
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    // 与 egui-wgpu 的预乘 alpha 混合保持一致。
    let blend = wgpu::BlendState {
        color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
        alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
    };
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("daozai-glass-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
        fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs"), targets: &[Some(wgpu::ColorTargetState { format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("daozai-glass-uniform"),
        size: std::mem::size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    GlassGpu { pipeline, layout, uniform, sampler, texture: None, bind_group: None, size: (0, 0) }
}

/// DAOZAI_LIQUID_FLAT=1：玻璃内原样输出背景，用于检查抓屏与屏幕是否对齐。
fn flat_flag() -> f32 {
    static FLAT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let v = FLAT.get_or_init(|| std::env::var("DAOZAI_LIQUID_FLAT").unwrap_or_default());
    return match v.as_str() { "2" => 2.0, "1" => 1.0, _ => 0.0 };
}

/// 形变增强倍数，由设置项 `liquid_morph` 每帧写入（默认 1.0）。
/// 用一个原子存 f32 的位模式，避免为了一个标量去改 frame_for 的签名。
static MORPH_SCALE: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0x3F80_0000); // = 1.0f32

pub fn set_morph_scale(v: f32) {
    MORPH_SCALE.store(v.clamp(0.0, 3.0).to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// DAOZAI_LIQUID_CAPTURE_EVERY：抓屏节流倍数（默认 1 = 每帧都抓）。
/// 设为 2 时玻璃背景按 30fps 更新，岛体自身动画仍是 60fps。
fn capture_every() -> u64 {
    static V: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("DAOZAI_LIQUID_CAPTURE_EVERY").ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(|v| v.clamp(1, 4))
            .unwrap_or(1)
    })
}

fn morph_scale() -> f32 {
    f32::from_bits(MORPH_SCALE.load(std::sync::atomic::Ordering::Relaxed))
}

/// 折射 / 色散强度，由设置项每帧写入（单位 CSS px，量级 48 / 12）。
///
/// 这两个数原来是我按实测硬编码在着色器里的（strength 按面板高度缩放、chroma 收到
/// 2.6px 还乘了 0.22 的角部权重），调一次要改代码重编三分钟 —— 而「色散够不够」
/// 是纯观感问题，必须能边看边拧。
static OPTICS: [std::sync::atomic::AtomicU32; 2] = [
    std::sync::atomic::AtomicU32::new(0x4240_0000), // refract = 48.0
    std::sync::atomic::AtomicU32::new(0x4100_0000), // chroma  = 8.0
];

pub fn set_liquid_optics(refract_px: f32, chroma_px: f32) {
    use std::sync::atomic::Ordering::Relaxed;
    OPTICS[0].store(refract_px.clamp(0.0, 120.0).to_bits(), Relaxed);
    OPTICS[1].store(chroma_px.clamp(0.0, 30.0).to_bits(), Relaxed);
}

fn strength_scale() -> f32 {
    f32::from_bits(OPTICS[0].load(std::sync::atomic::Ordering::Relaxed))
}

fn chroma_scale() -> f32 {
    f32::from_bits(OPTICS[1].load(std::sync::atomic::Ordering::Relaxed))
}

/// 外观四件套（外观页最下面那组滑块剩下的四项）：
/// 内部模糊 px、折射带宽 px、白边宽度 px、白边不透明度。
/// 前三个单位是 CSS px，进 uniform 前乘 ppp；不透明度无量纲。
static LOOK: [std::sync::atomic::AtomicU32; 4] = [
    std::sync::atomic::AtomicU32::new(0x3FC0_0000), // blur   = 1.5
    std::sync::atomic::AtomicU32::new(0x42AE_0000), // band   = 87.0（≈ 旧代码写死的 130 设备 px）
    std::sync::atomic::AtomicU32::new(0x3FC0_0000), // edge_w = 1.5
    std::sync::atomic::AtomicU32::new(0x3F66_6666), // edge_op= 0.90
];

pub fn set_liquid_look(blur_px: f32, band_px: f32, edge_px: f32, edge_op: f32) {
    use std::sync::atomic::Ordering::Relaxed;
    LOOK[0].store(blur_px.clamp(0.0, 4.0).to_bits(), Relaxed);
    LOOK[1].store(band_px.clamp(10.0, 140.0).to_bits(), Relaxed);
    LOOK[2].store(edge_px.clamp(0.0, 4.0).to_bits(), Relaxed);
    LOOK[3].store(edge_op.clamp(0.0, 1.0).to_bits(), Relaxed);
}

/// DirectComposition 路径下窗口已有真实逐像素 alpha：WGSL 必须输出覆盖率 alpha，
/// 而不是把桌面画成 alpha=1 的矩形。
static DCOMP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_direct_composition(active: bool) {
    DCOMP.store(active, std::sync::atomic::Ordering::Relaxed);
}

fn direct_composition() -> bool {
    DCOMP.load(std::sync::atomic::Ordering::Relaxed)
}

fn look() -> [f32; 4] {
    use std::sync::atomic::Ordering::Relaxed;
    [0, 1, 2, 3].map(|i| f32::from_bits(LOOK[i].load(Relaxed)))
}

/// DAOZAI_LIQUID_MORPH_FORCE=1：把形变强度钉成常数，用于确定性验证
/// （动画只有约 100ms，抓帧时序对不齐，不钉住就没法做 A/B）。
fn morph_force() -> f32 {
    static V: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *V.get_or_init(|| if std::env::var("DAOZAI_LIQUID_MORPH_FORCE").is_ok() { 1.0 } else { 0.0 })
}

fn uniforms(f: &GlassFrame, tex: (u32, u32)) -> Uniforms {
    Uniforms {
        bounds: f.bounds,
        // params.z 原来是 time，shader 里一直没人用，这里改放形变倍数；
        // params.y 原来也是空的，改放「强制」开关；
        // params.x 原来是 ior（1.52，shader 里从没用过），改放色散倍数。
        params: [f.optics[2], morph_force(), morph_scale(), f.optics[3]],
        extra: [f.shape[0], tex.0 as f32, tex.1 as f32, f.shape[1]],
        tint: f.tint,
        quad: f.clip,
        // misc.z = 折射倍数（shader 里直接乘在 strength 上）。
        misc: [f.shape[2], f.shape[3], f.optics[1], flat_flag()],
        crect: f.controls,
        cprm: f.control_params,
        look: f.look,
        notch: f.notch,
    }
}

/// DWM 捕获排除：把自己从「所有屏幕捕获」里摘掉。
///
/// WGC / DWM 缩略图 / GDI BitBlt 抓的都是 DWM 合成后的画面，默认会把岛仔自己
/// 也抓进去。WDA_EXCLUDEFROMCAPTURE 让窗口照常显示在屏幕上，但在捕获路径里
/// 完全消失 —— 这样每帧抓到的就是干净的、实时的桌面，不再需要静态底片。
/// 副作用：第三方截图/录屏工具也会看不到岛仔，可用 DAOZAI_LIQUID_NO_EXCLUDE=1 关闭。
#[cfg(windows)]
fn enable_capture_exclusion() {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::*;
    if std::env::var("DAOZAI_LIQUID_NO_EXCLUDE").is_ok() {
        if !EXCLUSION_TRIED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            diag("exclude: 被 DAOZAI_LIQUID_NO_EXCLUDE 关闭，回退静态底片");
        }
        return;
    }
    // 实机证明：SetWindowRgn / 窗口样式变更会把 display affinity 重置，
    // 只设一次的话后面 WGC 帧又会抓到岛仔自己（dump 第 2 帧干净、第 7 帧有残影）。
    // 所以「每帧必须检查」这条不能破。
    //
    // 但「每帧检查」不等于「每帧 EnumWindows + 无条件重设」：
    //   · 本进程的窗口集合几乎不变 → 句柄缓存起来，只在失效/定期时才重扫；
    //   · affinity 没丢时 SetWindowDisplayAffinity 是纯浪费 → 先 GetWindowDisplayAffinity，
    //     确认丢了才重设。
    // 从「枚举全部顶层窗口 + N 次重设」降到「N 次查询」，N 通常只有 1~2。
    static HOSTS: std::sync::Mutex<Vec<isize>> = std::sync::Mutex::new(Vec::new());
    let tick = EXCLUSION_TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut hosts = HOSTS.lock().unwrap_or_else(|e| e.into_inner());

    // 句柄失效（窗口销毁）、启动初期、定期重扫，三者任一成立就重新枚举。
    // 「启动初期每帧重扫」是必须的：岛仔窗口在第一帧可能还没可见，缓存里只填到
    // 别的窗口，就会把「岛仔还没被排除」这个状态一直维持到下次定期重扫（2 秒），
    // 期间 WGC 抓到岛仔自己 → 玻璃里出现正反馈条纹。
    // 前 240 帧（约 4 秒）每帧枚举，之后退回缓存 + 每 120 帧重扫。
    let mut rescan = hosts.is_empty() || tick < 240 || tick % 120 == 0;
    if !rescan {
        for h in hosts.iter() {
            let mut aff = 0u32;
            if unsafe { GetWindowDisplayAffinity(HWND(*h as *mut _), &mut aff) }.is_err() {
                rescan = true;
                break;
            }
        }
    }
    if rescan {
        hosts.clear();
        unsafe extern "system" fn visit(hwnd: HWND, l: LPARAM) -> BOOL {
            use windows::Win32::System::Threading::GetCurrentProcessId;
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == GetCurrentProcessId() && IsWindowVisible(hwnd).as_bool() {
                (*(l.0 as *mut Vec<isize>)).push(hwnd.0 as isize);
            }
            BOOL(1)
        }
        unsafe {
            let _ = EnumWindows(Some(visit), LPARAM(&mut *hosts as *mut Vec<isize> as isize));
        }
    }

    let mut fixed = 0u32;
    for h in hosts.iter() {
        let hwnd = HWND(*h as *mut _);
        let mut aff = 0u32;
        if unsafe { GetWindowDisplayAffinity(hwnd, &mut aff) }.is_ok()
            && aff == WDA_EXCLUDEFROMCAPTURE.0
        {
            continue;
        }
        if unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) }.is_ok() {
            fixed += 1;
        }
    }
    EXCLUDED_N.store(fixed, std::sync::atomic::Ordering::Relaxed);
    if tick == 0 || tick % 1800 == 0 {
        diag(&format!("exclude: 检查 #{} 窗口 {} 个，重设 {} 个", tick, hosts.len(), fixed));
    }
}

/// 诊断：把某一帧抓到的原始屏幕写成 PPM，直接看 WDA 排除到底给了什么。
/// 只有设置 DAOZAI_LIQUID_DUMP=1 时启用，最多写 8 帧。
#[cfg(windows)]
static DUMP_N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg(windows)]
fn dump_capture(buf: &[u8], w: u32, h: u32, tag: &str) {
    if std::env::var("DAOZAI_LIQUID_DUMP").is_err() { return; }
    let n = DUMP_N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if n >= 8 { return; }
    let dir = crate::model::config_dir().join("diagnostics");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("liquid-cap-{}-{}.ppm", n, tag));
    let mut out = Vec::with_capacity((w * h * 3) as usize + 32);
    out.extend_from_slice(format!("P6\n{} {}\n255\n", w, h).as_bytes());
    for i in (0..buf.len()).step_by(4) {
        out.push(buf[i]); out.push(buf[i + 1]); out.push(buf[i + 2]);
    }
    let _ = std::fs::write(path, out);
}

/// 取消捕获排除（冻结诊断模式预热结束后调用），让第三方截图重新能看到岛仔。
#[cfg(windows)]
fn disable_capture_exclusion() {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe extern "system" fn visit(hwnd: HWND, _: LPARAM) -> BOOL {
        use windows::Win32::System::Threading::GetCurrentProcessId;
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == GetCurrentProcessId() && IsWindowVisible(hwnd).as_bool() {
            let _ = SetWindowDisplayAffinity(hwnd, WDA_NONE);
        }
        BOOL(1)
    }
    unsafe { let _ = EnumWindows(Some(visit), LPARAM(0)); }
    diag("exclude: 冻结模式预热完成，已取消排除（截图可见）");
}

/// 主题采样框的尺寸（CSS 像素）。宽度取「展开面板宽 + 两侧各留一圈」，
/// 高度只取岛体上部的一条带 —— 够代表玻璃背后的明暗，又不至于被页面高度带跑。
#[cfg(windows)]
const THEME_W_CSS: f32 = 520.0;
#[cfg(windows)]
const THEME_H_CSS: f32 = 120.0;

/// 主题判定（深/浅材质）的采样框：以**岛体上缘中心**为锚点、固定尺寸。
///
/// 以前这里用的是「抓屏矩形减去岛体」那一圈像素。问题是那一圈的形状完全跟着
/// 岛体尺寸走，而各页的展开高度不同（音乐 140 / 视频 176 / 日历 216 / 天气 232），
/// 一翻页圈里就换了一批壁纸 → 明暗被重新判定 → 背景明明没动，控件和白边却
/// 黑白跳一次。改成固定尺寸、上缘锚定后，同一块屏幕位置永远是同一个采样框：
/// 翻页、换页、展开高度变化都不会再动它，只有岛体自己挪位置（拖动/换屏）才跟着走。
#[cfg(windows)]
fn theme_rect(body: [f32; 4], bw: i32, bh: i32, ppp: f32) -> (i32, i32, i32, i32) {
    let ppp = ppp.max(0.1);
    // 夹在抓屏尺寸内：抓屏只有几十像素时（展开动画第一帧）也不能越过缓冲边界。
    let (bw, bh) = (bw.max(1), bh.max(1));
    let sw = ((THEME_W_CSS * ppp).round() as i32).min(bw).max(1);
    let sh = ((THEME_H_CSS * ppp).round() as i32).min(bh).max(1);
    let cx = (body[0] + body[2] * 0.5).round() as i32;
    let x0 = (cx - sw / 2).clamp(0, (bw - sw).max(0));
    // 上缘锚定：岛体顶端（含刘海肩上那几像素）就是锚点，往下取固定高度。
    let y0 = ((body[1] - 6.0 * ppp).round() as i32).clamp(0, (bh - sh).max(0));
    (x0, y0, x0 + sw, y0 + sh)
}

/// 排除未生效时的兜底：首帧抓的「干净桌面底片」。
/// 每帧只把岛体以外的像素写回底片，岛体内部一直用这张底片，避免自我鬼影。
///
/// 宽度必须容得下「左移锚点 + 展开态抓屏宽 + 拖动余量」，否则 need_new
/// （ox+bw > PLATE_W）会每帧触发。冻结模式第 24 帧取消了 WDA 排除，此时
/// 重建底片走 GDI grab_raw 会把岛仔自己（专辑封面/歌词）抓进去 → 玻璃出现
/// "玻璃显示玻璃"的彩条重影。实测：岛居中物理 x≈1280，展开抓屏宽 957px，
/// 抓屏矩形 [802,1758]，拖动 ±100 逻辑点（±125 物理）。取 1280 + 锚点左移 256：
/// 底片 [546,1826] 覆盖展开和拖动范围，且整体仍在 2560 屏内（GDI 抓屏要求
/// 矩形完全在屏内，越界会 BitBlt 失败 —— 所以不能贪心取 2048）。
#[cfg(windows)]
const PLATE_W: u32 = 1280;
#[cfg(windows)]
const PLATE_H: u32 = 512;
/// 底片起点比抓屏矩形左移 256px：容纳展开时抓屏矩形向左变宽 + 向左拖动。
#[cfg(windows)]
const PLATE_ANCHOR: i32 = 256;

#[cfg(windows)]
/// 底片可疑时的重试计数（连续丢弃次数）。
static PLATE_RETRY: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// 底片刷新窗口与上次重建时刻。桌面源换代（切显示模式 / 换屏）后，动态壁纸软件
/// 往往比系统动画晚 1~2s 才画好；底片一旦存下就不会自己更新，于是残留一小段旧画面。
/// 这里在换代后 5s 内每 500ms 强制重建一次底片，壁纸一画好就会被替换掉。
static PLATE_REFRESH_UNTIL: Mutex<Option<std::time::Instant>> = Mutex::new(None);
static PLATE_BUILT_AT: Mutex<Option<std::time::Instant>> = Mutex::new(None);
static PLATE_EPOCH_SEEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 桌面源换代后需要连续重建底片的窗口（毫秒）。
const PLATE_REFRESH_MS: u64 = 5000;
/// 刷新窗口内两次重建的最小间隔（毫秒）。
const PLATE_REFRESH_STEP_MS: u64 = 500;

struct CleanPlate {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    data: Vec<u8>,
    /// 抓取时的抓屏源代次。换显示器后 WGC 会话重建、代次递增，底片里的旧屏桌面
    /// 内容就失效了，必须重抓 —— 否则玻璃会透出上一块屏的画面。
    epoch: u64,
}

#[cfg(windows)]
static PLATE: Mutex<Option<CleanPlate>> = Mutex::new(None);

#[cfg(windows)]
fn row_mean(buf: &[u8], w: u32, h: u32, x0: i32, x1: i32, y: i32) -> [f32; 3] {
    let yy = y.clamp(0, h as i32 - 1) as u32;
    let a = x0.max(0) as u32;
    let b = (x1.min(w as i32)).max(a as i32) as u32;
    let (mut r, mut g, mut bl, mut n) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for x in a..b {
        let i = ((yy * w + x) * 4) as usize;
        r += buf[i] as f32; g += buf[i + 1] as f32; bl += buf[i + 2] as f32; n += 1.0;
    }
    if n > 0.0 { [r / n, g / n, bl / n] } else { [238.0, 238.0, 238.0] }
}

#[cfg(windows)]
fn fill_gradient(buf: &mut [u8], w: u32, x0: i32, y0: i32, x1: i32, y1: i32, top: [f32; 3], bot: [f32; 3]) {
    if x1 <= x0 || y1 <= y0 { return; }
    let hh = (y1 - y0) as f32;
    for yy in y0..y1 {
        let k = (yy - y0) as f32 / hh;
        for xx in x0..x1 {
            let i = ((yy as u32 * w + xx as u32) * 4) as usize;
            buf[i] = (top[0] + (bot[0] - top[0]) * k) as u8;
            buf[i + 1] = (top[1] + (bot[1] - top[1]) * k) as u8;
            buf[i + 2] = (top[2] + (bot[2] - top[2]) * k) as u8;
            buf[i + 3] = 255;
        }
    }
}

/// 纯 GDI 抓屏，返回自上而下的 RGBA；不做任何岛体处理。
#[cfg(windows)]
fn grab_raw(x: i32, y: i32, w: i32, h: i32) -> Option<(Vec<u8>, u32, u32)> {
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::Graphics::Gdi::HGDIOBJ;
    if w <= 0 || h <= 0 || w > 2048 || h > 1024 { return None; }
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() { return None; }
        let mem = CreateCompatibleDC(screen);
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = w;
        info.bmiHeader.biHeight = -h; // 负数 = 自上而下
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB.0;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(mem, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let old_bmp = SelectObject(mem, HGDIOBJ(bmp.0));
        let ok = BitBlt(mem, 0, 0, w, h, screen, x, y, SRCCOPY).is_ok();
        let (bw, bh) = (w as u32, h as u32);
        let mut rgba = vec![0u8; (bw * bh * 4) as usize];
        if ok && !bits.is_null() {
            // DIB 是 BGRA，纹理格式也用 Bgra8Unorm，整块拷贝即可（省掉每帧 4 次/像素的转换）。
            let src = std::slice::from_raw_parts(bits as *const u8, rgba.len());
            rgba.copy_from_slice(src);
        }
        SelectObject(mem, old_bmp);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if ok { Some((rgba, bw, bh)) } else { None }
    }
}

/// 区域平均亮度（0..255）。
#[cfg(windows)]
fn mean_luma(buf: &[u8], w: u32, h: u32, x0: i32, y0: i32, x1: i32, y1: i32) -> f32 {
    let (mut s, mut n) = (0.0f32, 0.0f32);
    for y in (y0.max(0)..y1.min(h as i32)).step_by(4) {
        for x in (x0.max(0)..x1.min(w as i32)).step_by(4) {
            let i = ((y as u32 * w + x as u32) * 4) as usize;
            s += (buf[i] as f32 + buf[i + 1] as f32 + buf[i + 2] as f32) / 3.0;
            n += 1.0;
        }
    }
    if n > 0.0 { s / n } else { 0.0 }
}

/// 岛体以外（capture 外围环带）的平均亮度，作为「干净桌面」的参照。
#[cfg(windows)]
fn mean_luma_outside(buf: &[u8], w: u32, h: u32,
                     ex0: i32, ey0: i32, ex1: i32, ey1: i32) -> f32 {
    let (mut s, mut n) = (0.0f32, 0.0f32);
    for y in (0..h as i32).step_by(4) {
        for x in (0..w as i32).step_by(4) {
            if x >= ex0 && x < ex1 && y >= ey0 && y < ey1 { continue; }
            let i = ((y as u32 * w + x as u32) * 4) as usize;
            s += (buf[i] as f32 + buf[i + 1] as f32 + buf[i + 2] as f32) / 3.0;
            n += 1.0;
        }
    }
    if n > 0.0 { s / n } else { 0.0 }
}

/// 滚动干净底片：干净帧把整块（含岛体）写进底片，污染帧只更新岛体以外。
/// 岛体内部因此最多滞后到最近一次干净帧，而不再是启动时的一张静态快照。
#[cfg(windows)]
fn compose_with_plate(raw: &[u8], bw: u32, bh: u32, x: i32, y: i32,
                      body: [f32; 4], clean_frame: bool, frozen: bool) -> Option<Vec<u8>> {
    let (cx0, cy0, cx1, cy1) = (
        (body[0] - 10.0).max(0.0) as i32,
        (body[1] - 10.0).max(0.0) as i32,
        ((body[0] + body[2] + 10.0).min(bw as f32)) as i32,
        ((body[1] + body[3] + 10.0).min(bh as f32)) as i32,
    );
    let mut guard = PLATE.lock().unwrap_or_else(|e| e.into_inner());
    let epoch = crate::wgc::epoch();
    // 桌面源换代（切显示模式/换屏）：开启底片刷新窗口，让晚到的动态壁纸能顶上去。
    if PLATE_EPOCH_SEEN.swap(epoch, std::sync::atomic::Ordering::Relaxed) != epoch {
        *PLATE_REFRESH_UNTIL.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(PLATE_REFRESH_MS));
        PLATE_RETRY.store(0, std::sync::atomic::Ordering::Relaxed);
    }
    let now = std::time::Instant::now();
    let refreshing = PLATE_REFRESH_UNTIL
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some_and(|until| now < until)
        && PLATE_BUILT_AT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map_or(true, |t| now.duration_since(t).as_millis() as u64 >= PLATE_REFRESH_STEP_MS);
    let need_new = refreshing || match guard.as_ref() {
        None => true,
        Some(p) => {
            if p.epoch != epoch { true } else {
                let (ox, oy) = (x - p.x, y - p.y);
                ox < 0 || oy < 0 || ox + bw as i32 > p.w as i32 || oy + bh as i32 > p.h as i32
            }
        }
    };
    if need_new {
        let ax = x - PLATE_ANCHOR;
        let big = grab_raw(ax, y, PLATE_W as i32, PLATE_H as i32);
        let (mut data, pw, ph, fpx, fpy) = if let Some((d, pw, ph)) = big {
            (d, pw, ph, ax, y)
        } else {
            (raw.to_vec(), bw, bh, x, y)
        };
        let (sx, sy) = (x - fpx, y - fpy);
        let (bx0, by0, bx1, by1) = (
            (body[0] + sx as f32 - 10.0).max(0.0) as i32,
            (body[1] + sy as f32 - 10.0).max(0.0) as i32,
            ((body[0] + sx as f32 + body[2] + 10.0).min(pw as f32)) as i32,
            ((body[1] + sy as f32 + body[3] + 10.0).min(ph as f32)) as i32,
        );
        // 岛体区域在底片里是黑的时候（WDA 排除的竞态残留）才抹成上下桌面色的
        // 渐变。之前是「只要不是冻结模式就无条件抹」，于是**干净帧也被抹掉了**：
        // 底片里岛体位置永远是一条竖直渐变，玻璃取样到它就永远是平的，
        // 冻结模式截图诊断看到的全是假背景 —— 玻璃感被判死刑主要就是这条。
        let top = row_mean(&data, pw, ph, bx0, bx1, by0 - 6);
        let bot = row_mean(&data, pw, ph, bx0, bx1, by1 + 6);
        let inner = mean_luma(&data, pw, ph, bx0, by0, bx1, by1);
        let ring = (top[0] + top[1] + top[2] + bot[0] + bot[1] + bot[2]) / 6.0;
        // 底片里的岛体区域明显比周围暗 = 抓到的是切显示模式/启动瞬间的过渡态（黑边、
        // 收拢动画、或还没画完的桌面）。这种帧一旦当底片存下来，玻璃会一直显示这块暗区，
        // 表现就是「花屏且不自愈」。旧阈值要求 inner<45 太严：实测过渡态 inner=72
        // （正常壁纸约 190），刚好漏过去。改成相对阈值：低于周围 62% 就认为不可信。
        // 全黑底片（inner≈0）单独兜住：切换显示模式的瞬间整块抓屏都是黑的，
        // 这时 ring 也是 0，相对阈值判断不出来（实测 inner=0 / ring=0）。
        let blank = !frozen && inner < 8.0;
        let polluted = !frozen && (blank || (ring > 100.0 && inner < ring * 0.62));
        // 宁可晚几帧也不要存一张脏底片：判定可疑时先丢掉这次抓取重试（60 帧 ≈1s）。
        // 注意**不要**靠加大这个窗口来等动态壁纸画好 —— 那只会让纯色期更长。壁纸比
        // 系统动画晚 1~2s 是常态，交给下面的「底片定期刷新」处理。
        if polluted && PLATE_RETRY.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 60 {
            diag(&format!("plate: 底片可疑 inner={:.0} ring={:.0}，丢弃重试", inner, ring));
            return None;
        }
        PLATE_RETRY.store(0, std::sync::atomic::Ordering::Relaxed);
        if polluted || (inner < 45.0 && ring - inner > 80.0) {
            fill_gradient(&mut data, pw, bx0, by0, bx1, by1, top, bot);
            if polluted {
                diag(&format!("plate: 底片不可信（{}）inner={:.0} ring={:.0}，已填渐变兜底",
                              if blank {"全黑"} else {"过渡态"}, inner, ring));
            }
        }
        diag(&format!("plate: new {}x{} at ({},{}) frozen={} inner={:.0}",
                      pw, ph, fpx, fpy, frozen, inner));
        *guard = Some(CleanPlate { x: fpx, y: fpy, w: pw, h: ph, data, epoch });
        *PLATE_BUILT_AT.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now());
        // 干净底片建成 → 告诉 wgc 这台机器实际需要多久稳定，供下次切换起步。
        if !polluted && !blank {
            crate::wgc::note_plate_ready();
        }
    }
    let p = guard.as_mut().unwrap();
    let (ox, oy) = (x - p.x, y - p.y);
    let mut out = vec![0u8; (bw * bh * 4) as usize];
    // 快速路径：干净帧且底片完全覆盖当前矩形时，输出就是 raw 本身，
    // 只要整块刷进底片。省掉每帧 90k+ 次逐像素判断（这是抓屏路径里最重的一段）。
    if clean_frame && ox >= 0 && oy >= 0 && ox + bw as i32 <= p.w as i32 && oy + bh as i32 <= p.h as i32 {
        out.copy_from_slice(raw);
        let row = (bw * 4) as usize;
        for yy in 0..bh as usize {
            let d = ((oy as usize + yy) * p.w as usize + ox as usize) * 4;
            p.data[d..d + row].copy_from_slice(&raw[yy * row..yy * row + row]);
        }
        return Some(out);
    }
    for yy in 0..bh as i32 {
        for xx in 0..bw as i32 {
            let d = ((yy as u32 * bw + xx as u32) * 4) as usize;
            let (qx, qy) = (ox + xx, oy + yy);
            let in_plate = qx >= 0 && qy >= 0 && qx < p.w as i32 && qy < p.h as i32;
            let excluded = xx >= cx0 && xx < cx1 && yy >= cy0 && yy < cy1;
            // 冻结态：底片锁定为启动时的干净壁纸，绝不能用 raw。冻结第 24 帧已取消
            // WDA 排除，raw 每帧都抓到岛仔自己（专辑封面/文字），拖动时抓屏矩形超出
            // 底片的那一带若用 raw 填充，就会把自身像素画进玻璃 → 残留重影。
            // 超界像素改为 clamp 到底片最近边缘（壁纸的自然延续），肉眼无缝。
            let use_raw = if frozen { false } else { clean_frame || !excluded || !in_plate };
            if use_raw {
                out[d..d + 4].copy_from_slice(&raw[d..d + 4]);
                if in_plate {
                    let s = ((qy as u32 * p.w + qx as u32) * 4) as usize;
                    p.data[s..s + 4].copy_from_slice(&raw[d..d + 4]);
                }
            } else {
                let (cqx, cqy) = (qx.clamp(0, p.w as i32 - 1), qy.clamp(0, p.h as i32 - 1));
                let s = ((cqy as u32 * p.w + cqx as u32) * 4) as usize;
                out[d..d + 4].copy_from_slice(&p.data[s..s + 4]);
            }
        }
    }
    Some(out)
}

/// 抓当前岛体外扩矩形。WGC 帧干净时整块刷新滚动底片，污染时只更新外围；
/// WGC 不可用则回退 GDI，同样走滚动底片（岛体内部保持最近一次干净画面）。
#[cfg(windows)]
fn capture(x: i32, y: i32, w: i32, h: i32, body: [f32; 4], ppp: f32) -> Option<(Vec<u8>, u32, u32)> {
    if w <= 0 || h <= 0 || w > 2048 || h > 1024 { return None; }
    // 显示器拓扑刚变过（切「扩展/复制/仅主屏」）：桌面还在播重建动画，这一秒内
    // 抓到的都是收拢/黑边的中间态。先检测（顺带处理稳定期内的再次切换），
    // 再决定是否暂停出帧，让玻璃保持上一帧，别污染底片。
    crate::wgc::poll_topology();
    if crate::wgc::settling() { return None; }
    // 截图诊断模式：不排除自身（截图能看到岛仔），背景冻结成启动时的纯壁纸，
    // 这样既能截到玻璃，又能看到真实的折射/色散细节。
    let frozen_env = std::env::var("DAOZAI_LIQUID_FREEZE_BG").is_ok();
    let warm_n = if frozen_env { FREEZE_TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed) } else { u64::MAX };
    let warm = frozen_env && warm_n < 24;
    let frozen = frozen_env && warm_n >= 24;
    if !frozen_env || warm {
        enable_capture_exclusion();
    } else if warm_n == 24 {
        disable_capture_exclusion();
    }
    let want = ((w as u64) << 32) | (h as u64);
    let (raw, bw, bh, from_wgc) = match crate::wgc::grab(x, y, w, h) {
        crate::wgc::Grab::Frame(p, tw, th) => (p, tw, th, true),
        // WGC 这一帧还没有新画面：复用上一帧纹理。
        // **绝对不能退回 GDI**：GDI BitBlt 读的是桌面 DC，WDA_EXCLUDEFROMCAPTURE
        // 在那里不生效（只会把窗口变黑、或异步失效后抓到窗口自己），
        // 一旦抓到岛仔自己就会形成「玻璃里显示玻璃」的正反馈条纹。
        // 复用旧纹理最多让玻璃的几何错一帧，退回 GDI 会让整块玻璃糊成条纹。
        crate::wgc::Grab::NoFrame => return None,
        crate::wgc::Grab::Unavailable => {
            let (p, tw, th) = grab_raw(x, y, w, h)?;
            (p, tw, th, false)
        }
    };
    LAST_CAP.store(want, std::sync::atomic::Ordering::Relaxed);
    let (cx0, cy0, cx1, cy1) = (
        (body[0] - 10.0).max(0.0) as i32,
        (body[1] - 10.0).max(0.0) as i32,
        ((body[0] + body[2] + 10.0).min(bw as f32)) as i32,
        ((body[1] + body[3] + 10.0).min(bh as f32)) as i32,
    );
    dump_capture(&raw, bw, bh, if from_wgc { "wgc" } else { "gdi" });
    let inner = mean_luma(&raw, bw, bh, cx0, cy0, cx1, cy1);
    let outer = mean_luma_outside(&raw, bw, bh, cx0, cy0, cx1, cy1);
    // 只丢弃「岛体几乎全黑、且明显比外围暗」的帧 —— 那是 WDA 排除的竞态残留。
    // 之前用「暗像素比例」判断，深色壁纸会把正常帧全判成污染（实测 inner=0.97），
    // 导致底片永远不更新、换壁纸也不生效。这里改用平均亮度对比，深色壁纸不受影响。
    let black_island = inner < 45.0 && outer - inner > 80.0;
    if black_island {
        if GRAB_N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 60 == 0 {
            diag(&format!("capture: 丢弃黑岛形帧 inner={:.0} outer={:.0}", inner, outer));
        }
        return None;
    }
    // 主题明暗：用**固定尺寸**的采样框（见 `theme_rect`），只认 WGC 抓到的干净帧。
    // 放在污染判定之后：黑岛形帧、GDI 兜底帧（岛仔自己会被抓进去）都不许污染主题，
    // 冻结截图模式下取消过捕获排除，同样跳过 —— 宁可沿用上一次的值。
    if from_wgc && !frozen {
        let (tx0, ty0, tx1, ty1) = theme_rect(body, bw as i32, bh as i32, ppp);
        let luma = (mean_luma(&raw, bw, bh, tx0, ty0, tx1, ty1) / 255.0).clamp(0.0, 1.0);
        // 明显变化才落盘：换壁纸、岛仔换位置会看到这条，翻页/换页不该看到。
        let prev = f32::from_bits(BG_LUMA.load(std::sync::atomic::Ordering::Relaxed));
        if (luma - prev).abs() > 0.06 {
            diag(&format!("theme: luma={:.2}→{:.2} 框=[{},{} {}x{}] 岛体高={:.0}",
                          prev, luma, tx0, ty0, tx1 - tx0, ty1 - ty0, body[3]));
        }
        BG_LUMA.store(luma.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }
    if GRAB_N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 120 == 0 {
        diag(&format!("capture: src={} inner={:.0} outer={:.0} cap={}x{}",
                      if from_wgc { "wgc" } else { "gdi" }, inner, outer, bw, bh));
    }
    // 默认信任 WGC 帧并整块刷新底片：换壁纸/窗口移动都能立即反映。
    let out = compose_with_plate(&raw, bw, bh, x, y, body,
                                 if frozen { false } else { true }, frozen)?;
    if frozen && warm_n == 24 { diag("freeze: 背景已冻结为启动时的干净壁纸"); }
    Some((out, bw, bh))
}

#[cfg(not(windows))]
fn capture(_x: i32, _y: i32, w: i32, h: i32, _body: [f32; 4], _ppp: f32) -> Option<(Vec<u8>, u32, u32)> {
    // 非 Windows 只给一张渐变，保证模块能编译；真机效果以 Windows 为准。
    let (w, h) = (w.max(1) as u32, h.max(1) as u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            rgba[i] = (40.0 + x as f32 / w as f32 * 90.0) as u8;
            rgba[i + 1] = (70.0 + y as f32 / h as f32 * 60.0) as u8;
            rgba[i + 2] = 150;
            rgba[i + 3] = 255;
        }
    }
    Some((rgba, w, h))
}

impl egui_wgpu::CallbackTrait for GlassCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let mut guard = GPU.lock().unwrap_or_else(|e| e.into_inner());
        let tick = TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // 帧率探针：tick 每帧 +1，定期落盘就能算出实际 fps（日志里其它行只在
        // 抓到新帧时才写，静态背景根本测不出帧率）。
        if tick % 600 == 0 { diag(&format!("perf: tick={tick}")); }
        if FAILED.load(std::sync::atomic::Ordering::Relaxed) { return Vec::new(); }
        if guard.is_none() {
            match make_gpu(device, self.format) {
                Some(gpu) => { *guard = Some(gpu); diag("make_gpu: OK"); }
                None => {
                    FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
                    diag("make_gpu: FAILED (shader/pipeline 创建抛错)，回退纯色");
                    return Vec::new();
                }
            }
        }
        let gpu = guard.as_mut().unwrap();
        let f = &self.frame;
        let (cw, ch) = (f.capture[2].max(1.0) as i32, f.capture[3].max(1.0) as i32);
        let tex_size;

        // 抓屏节流：每帧回读一次屏幕是这条链路里最贵的一段（WGC Map 是 GPU→CPU
        // 同步等待，再加两次 1MB 级拷贝）。几何没变时可以隔帧抓，纹理和 uniform
        // 沿用上一帧 —— 玻璃背景降频到 30fps，但岛体自身的动画仍是 60fps。
        // 几何在变（展开/折叠）时绝不节流，否则又会错位。
        static LAST_BOUNDS: std::sync::Mutex<[f32; 4]> = std::sync::Mutex::new([0.0; 4]);
        let every = capture_every();
        if every > 1 && tick % every as u64 != 0 {
            let same = *LAST_BOUNDS.lock().unwrap_or_else(|e| e.into_inner()) == f.bounds;
            if same { return Vec::new(); }
        }
        *LAST_BOUNDS.lock().unwrap_or_else(|e| e.into_inner()) = f.bounds;

        // 每帧都抓：节流会让「纹理内容」与「当前 bounds」错一帧，
        // 展开动画里几何被拉伸，表现为抖动/错位。
        {
            let Some((rgba, tw, th)) = capture(f.capture[0] as i32, f.capture[1] as i32, cw, ch, f.bounds, f.pixels_per_point) else {
                if tick == 0 { diag("capture: GDI 抓屏失败"); }
                return Vec::new();
            };
            if tick % 120 == 0 {
                let mut sum = 0u64;
                let mut n = 0u64;
                let mut i = 0usize;
                while i + 4 <= rgba.len() { sum += rgba[i] as u64; n += 1; i += 4004; }
                let mean = if n > 0 { sum / n } else { 0 };
                diag(&format!("tick={tick} cap={tw}x{th} mean_r={mean} tex={:?} clip={:?} bounds={:?}", gpu.size, f.clip, f.bounds));
            }
            if gpu.texture.is_none() {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("daozai-glass-backdrop"),
                    size: wgpu::Extent3d { width: TEX_W, height: TEX_H, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                gpu.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("daozai-glass-bind"),
                    layout: &gpu.layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: gpu.uniform.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&gpu.sampler) },
                    ],
                }));
                gpu.texture = Some(texture);
            }
            gpu.size = (tw, th);
            // wgpu 要求 bytes_per_row 是 256 的倍数；不对齐会触发 validation error
            // （纹理保持未初始化 → 整块黑），所以每行补 padding 再上传。
            if tw > TEX_W || th > TEX_H { return Vec::new(); }
            let bpr = (tw * 4 + 255) / 256 * 256;
            let mut padded = vec![0u8; bpr as usize * th as usize];
            let row = (tw * 4) as usize;
            for y in 0..th as usize {
                let (s, d) = (y * row, y * bpr as usize);
                padded[d..d + row].copy_from_slice(&rgba[s..s + row]);
            }
            if tick == 0 { diag(&format!("upload: bpr={bpr} (raw={}) rows={th}", tw * 4)); }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: gpu.texture.as_ref().unwrap(), mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &padded,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(th) },
                wgpu::Extent3d { width: tw, height: th, depth_or_array_layers: 1 },
            );
            tex_size = (tw, th);
        }
        let u = uniforms(f, tex_size);
        queue.write_buffer(&gpu.uniform, 0, u.bytes());
        Vec::new()
    }

    fn paint(
        &self,
        _info: eframe::egui::epaint::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        let guard = GPU.lock().unwrap_or_else(|e| e.into_inner());
        let Some(gpu) = guard.as_ref() else { return; };
        let Some(bind) = gpu.bind_group.as_ref() else { return; };
        let tick = TICK.load(std::sync::atomic::Ordering::Relaxed);
        if tick == 1 { diag("paint: pipeline/bind_group 已就绪"); }
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..6, 0..1);
    }
}

/// 把逻辑点矩形换成屏幕物理像素与 clip-space 坐标。
pub fn frame_for(
    body: eframe::egui::Rect,
    capture: eframe::egui::Rect,
    screen_px: (u32, u32),
    ppp: f32,
    radius: f32,
    // 形变强度 0..1（展开进度的变化速度，带衰减），驱动过渡期的光学增强。
    morph: f32,
    tint: [f32; 3],
    time: f32,
    pointer: (f32, f32),
    origin: (f32, f32),
    lightness: f32,
    // 岛内控件玻璃（屏幕坐标的逻辑点矩形 + 圆角 + 放大/折射倍数），**按绘制顺序**给：
    // 靠后的压在前面的上面。
    controls: &[Ctl],
    // 刘海形状的肩部参数 (flare, flare_h)，逻辑点；`None` = 普通圆角矩形（胶囊）。
    notch: Option<(f32, f32)>,
) -> GlassFrame {
    // 抓屏用屏幕绝对物理坐标，clip-space / bounds 用窗口内坐标。
    let c = ((capture.min.x + origin.0) * ppp, (capture.min.y + origin.1) * ppp);
    let cw = capture.width() * ppp;
    let ch = capture.height() * ppp;
    let bounds = [
        (body.min.x - capture.min.x) * ppp,
        (body.min.y - capture.min.y) * ppp,
        body.width() * ppp,
        body.height() * ppp,
    ];
    // 控件矩形换算到「capture 内物理像素」，着色器里直接用。
    // 只接受完整落在抓屏矩形内的控件：越界时着色器会 clamp 到边缘像素，
    // 表现为一条被拉长的色带。
    let mut crect = [[0.0f32; 4]; MAX_CTL];
    let mut cprm = [[0.0f32; 4]; MAX_CTL];
    let mut slot = 0usize;
    for ctl in controls.iter() {
        if slot == MAX_CTL { break; }
        let r = ctl.rect;
        if r.width() < 6.0 || r.height() < 6.0 || !capture.contains_rect(r) { continue; }
        crect[slot] = [
            (r.min.x - capture.min.x) * ppp,
            (r.min.y - capture.min.y) * ppp,
            r.width() * ppp,
            r.height() * ppp,
        ];
        cprm[slot] = [ctl.radius * ppp, 1.0, ctl.mag.max(1.0), ctl.refract.max(0.05)];
        slot += 1;
    }
    // egui-wgpu 在调用回调前已把 render pass 的 viewport 设成 callback.rect，
    // 所以 clip-space 只需铺满 [-1,1]；uv 0..1 正好对应抓到的那块纹理。
    let _ = screen_px;
    GlassFrame {
        capture: [c.0, c.1, cw, ch],
        bounds,
        shape: [radius * ppp, morph, (pointer.0 - capture.min.x) * ppp, (pointer.1 - capture.min.y) * ppp],
        tint: [tint[0], tint[1], tint[2], 1.0],
        // optics = (time, 折射 CSS px, 色散 CSS px, 亮/暗)。着色器全程按设备像素算，
        // 所以这里乘 ppp —— 着色器全程按设备像素算。
        optics: [time, strength_scale() * ppp, chroma_scale() * ppp, lightness],
        // 外观四件套：前三个同样 CSS px → 设备像素，最后一个是不透明度。
        look: {
            let l = look();
            [l[0] * ppp, l[1] * ppp, l[2] * ppp, l[3]]
        },
        clip: [-1.0, 1.0, 1.0, -1.0],
        controls: crect,
        control_params: cprm,
        // 刘海：flare/flare_h 同样 CSS px → 设备像素；非刘海时标志位 0。
        notch: match notch {
            Some((flare, flare_h)) => [flare * ppp, flare_h * ppp, 1.0, if direct_composition() {1.0} else {0.0}],
            None => [0.0, 0.0, 0.0, if direct_composition() {1.0} else {0.0}],
        },
        pixels_per_point: ppp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 主题采样框只跟「岛体上缘中心」走：翻页（页面高度不同）不能挪动它，
    /// 否则背景没变、深/浅材质却被重新判定，控件白边会黑白跳一次。
    #[test]
    fn theme_rect_is_immune_to_page_height() {
        let ppp = 1.75;
        // 同一块屏、同一个岛体中心，音乐页 140 高 vs 天气页 232 高。
        let music = [112.0, 10.5, 733.0, 245.0];
        let weather = [112.0, 10.5, 733.0, 406.0];
        let a = theme_rect(music, 957, 339, ppp);
        let b = theme_rect(weather, 957, 500, ppp);
        assert_eq!(a, b, "换页只改高度，采样框必须一模一样：{a:?} vs {b:?}");
        // 收起态抓屏矮很多，框会被裁到抓屏内，但仍然贴着岛体上缘。
        let c = theme_rect([112.0, 10.5, 362.0, 52.5], 586, 147, ppp);
        assert!(c.1 >= 0 && c.3 <= 147, "收到抓屏内：{c:?}");
        assert_eq!(c.1, 0, "上缘锚定，靠近屏幕顶时从 0 开始");
        // 岛体整体右移，框跟着走。
        let moved = theme_rect([312.0, 10.5, 733.0, 245.0], 1157, 339, ppp);
        assert!(moved.0 > a.0, "拖动岛仔时采样框跟着平移：{a:?} → {moved:?}");
        // 退化尺寸（展开动画第一帧抓屏只有几像素）不能越界、更不能 panic。
        for (w, h) in [(1, 1), (3, 2), (4, 9), (9, 4)] {
            let r = theme_rect([2.0, 1.0, 1.0, 1.0], w, h, ppp);
            assert!(r.0 >= 0 && r.1 >= 0 && r.2 <= w && r.3 <= h, "{w}x{h} → {r:?}");
        }
    }

    #[test]
    fn frame_maps_rects_to_pixels_and_clip_space() {
        use eframe::egui::{Pos2, Rect};
        let body = Rect::from_min_size(Pos2::new(100.0, 20.0), eframe::egui::vec2(200.0, 30.0));
        let capture = Rect::from_min_size(Pos2::new(60.0, 0.0), eframe::egui::vec2(280.0, 90.0));
        let f = frame_for(body, capture, (1920, 1080), 1.0, 15.0, 0.0, [0.6, 0.7, 0.8], 1.0, (150.0, 30.0), (0.0, 0.0), 1.0, &[], None);
        assert_eq!(f.bounds, [40.0, 20.0, 200.0, 30.0]);
        assert_eq!(f.clip, [-1.0, 1.0, 1.0, -1.0], "viewport 已由 egui 设为 callback 矩形");
        // 折射 / 色散走原子量 → optics → uniform 的两个不同槽位：
        // 折射进 misc.z（乘在 strength 上），色散进 params.x（乘在 chroma 上）。
        // 设置项单位是 CSS px，进 uniform 前要乘 ppp（着色器全程设备像素）。
        set_liquid_optics(60.0, 15.0);
        let u = uniforms(&frame_for(body, capture, (1920, 1080), 2.0, 15.0, 0.0, [0.6, 0.7, 0.8], 1.0,
                                   (150.0, 30.0), (0.0, 0.0), 1.0, &[], None), (280, 90));
        assert_eq!(u.params[0], 30.0, "色散进 params.x，并按 ppp 换算到设备像素");
        assert_eq!(u.misc[2], 120.0, "折射进 misc.z，并按 ppp 换算到设备像素");
        set_liquid_optics(48.0, 8.0);
    }

    #[test]
    fn control_rects_map_into_capture_pixels() {
        use eframe::egui::{Pos2, Rect};
        let body = Rect::from_min_size(Pos2::new(100.0, 20.0), eframe::egui::vec2(200.0, 30.0));
        let capture = Rect::from_min_size(Pos2::new(60.0, 0.0), eframe::egui::vec2(280.0, 90.0));
        let lyric = Rect::from_min_size(Pos2::new(80.0, 25.0), eframe::egui::vec2(50.0, 20.0));
        // 越界的控件必须被丢掉，否则着色器会 clamp 到边缘像素拉出色带。
        let outside = Rect::from_min_size(Pos2::new(50.0, 25.0), eframe::egui::vec2(50.0, 20.0));
        let ctl = |rect, radius| Ctl { rect, radius, mag: 1.10, refract: 0.35 };
        let f = frame_for(body, capture, (1920, 1080), 2.0, 15.0, 0.0, [0.6, 0.7, 0.8], 1.0, (150.0, 30.0), (0.0, 0.0), 1.0,
                          &[ctl(outside, 6.0), ctl(lyric, 6.0)], None);
        assert_eq!(f.controls[0], [40.0, 50.0, 100.0, 40.0], "控件按 2x 换算到 capture 内物理像素");
        assert_eq!(f.control_params[0], [12.0, 1.0, 1.10, 0.35], "圆角/启用/放大/折射一起进 uniform");
        assert_eq!(f.control_params[1][1], 0.0, "越界的控件不参与绘制");
    }

    #[test]
    fn uniforms_are_packed_without_padding() {
        // 8 个 vec4（bounds/params/extra/tint/quad/misc/look/notch）+ crect[13] + cprm[13]
        // 全是 16 字节对齐，544 = 34×16，没有任何隐式填充。加字段时同步改这里和 WGSL 的 `U`。
        assert_eq!(std::mem::size_of::<Uniforms>(), 544);
        assert_eq!(std::mem::size_of::<Uniforms>(), 8 * 16 + MAX_CTL * 32);
    }
}
