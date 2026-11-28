//! 系统性能：展开页、常驻徽标、侧边停靠与设置。
//!
//! 展开页先是总览：处理器、显卡、内存三张「玻璃液位」卡，磁盘和网络两张霓虹曲线卡；点哪张进哪类硬件的详情。
//! 底部程序区左右两栏并排「CPU 最高」和「内存最多」，只看不点。
//! 详情页是五个孪生版式：顶部带迷你曲线的页签条，左侧大环与三行读数，右上主视图，右下三行列表。
//! 质感取自玻璃格：染色的暗底、霓虹描边、底边发亮，格子里的细颗粒随负载闪烁。
use crate::app::resident_navigation::Destination;
use crate::{
    app::{App, PERFORMANCE_PAGE},
    paint::{alpha, mix, Card, H},
    performance::{BarStyle, Metric, Process, Reading, View},
    theme::W,
};
use eframe::egui::{self, Color32, Pos2};
use std::collections::VecDeque;
use std::f32::consts::{FRAC_PI_2, TAU};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------- 配色：每类硬件一种主色，次级读数（上传、写入）取主色的浅色 ----------
pub(crate) const CPU_COLOR: Color32 = Color32::from_rgb(99, 218, 207);
const GPU: Color32 = Color32::from_rgb(171, 156, 245);
const RAM: Color32 = Color32::from_rgb(233, 189, 117);
const NET: Color32 = Color32::from_rgb(110, 178, 250);
const DISK: Color32 = Color32::from_rgb(240, 146, 170);
const INK: Color32 = Color32::from_rgb(236, 240, 244);
const MUTED: Color32 = Color32::from_rgb(150, 158, 168);
const FAINT: Color32 = Color32::from_rgb(98, 104, 114);
const GRID: Color32 = Color32::from_rgb(44, 48, 55);
const SURFACE: Color32 = Color32::from_rgb(24, 26, 30);
const RAISED: Color32 = Color32::from_rgb(36, 39, 45);
const HOT: Color32 = Color32::from_rgb(255, 128, 102);
const KERNEL: Color32 = Color32::from_rgb(246, 138, 108);
const COLORS: [Color32; 3] = [CPU_COLOR, GPU, RAM];
const LETTERS: [&str; 3] = ["C", "G", "M"];

/// 展开页高度：总览要像一块小屏，比其它页高。
pub(crate) const PAGE_H: f32 = 360.0;
/// 正文左右边界（导航箭头之间）。
const LEFT: f32 = 37.0;
const RIGHT: f32 = 381.0;
// 总览：三张硬件卡、两张吞吐卡、一张程序排行
const CARD_GAP: f32 = 7.0;
const TOP_Y: f32 = 36.0;
const TOP_H: f32 = 134.0;
const IO_Y: f32 = TOP_Y + TOP_H + CARD_GAP;
const IO_H: f32 = 46.0;
const APP_Y: f32 = IO_Y + IO_H + CARD_GAP;
const APP_H: f32 = 114.0;
/// 总览底部的程序表。
const APP_CARD: [f32; 4] = [LEFT, APP_Y, RIGHT - LEFT, APP_H];
// 详情页：页签条
const TAB_Y: f32 = 36.0;
const TAB_H: f32 = 44.0;
const TAB_GAP: f32 = 5.0;
// 详情页：左列大环与读数，右列主视图与列表
const HERO_X: f32 = 89.0;
const HERO_Y: f32 = 156.0;
const HERO_R: f32 = 40.0;
const SIDE_R: f32 = 141.0;
const COL_X: f32 = 161.0;
const COL_W: f32 = RIGHT - COL_X;
const VIZ_Y: f32 = 104.0;
const VIZ_H: f32 = 96.0;
const LIST_Y: f32 = 234.0;
/// 列表行距。
const ROW: f32 = 18.0;
const BODY_BOTTOM: f32 = APP_Y + APP_H;
const GB: f64 = 1073741824.0;
/// 颗粒闪烁的刷新间隔：只在展开页可见时按这个节奏重绘，不跑满帧。
const SHIMMER: Duration = Duration::from_millis(90);

fn tab_w() -> f32 {
    (RIGHT - LEFT - TAB_GAP * 4.) / 5.
}
fn top_card(i: usize) -> [f32; 4] {
    let w = (RIGHT - LEFT - CARD_GAP * 2.) / 3.;
    [LEFT + i as f32 * (w + CARD_GAP), TOP_Y, w, TOP_H]
}
fn io_card(i: usize) -> [f32; 4] {
    let w = (RIGHT - LEFT - CARD_GAP) / 2.;
    [LEFT + i as f32 * (w + CARD_GAP), IO_Y, w, IO_H]
}
/// 总览里通向详情页的卡片位置（处理器、显卡、内存、磁盘、网络）。
fn overview_card(view: View) -> [f32; 4] {
    match view {
        View::Cpu | View::Overview => top_card(0),
        View::Gpu => top_card(1),
        View::Memory => top_card(2),
        View::Disk => io_card(0),
        View::Network => io_card(1),
    }
}
/// 测试用：总览卡片中心、详情页签中心、返回按钮中心。
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn overview_card_center(view: View) -> (f32, f32) {
    let [x, y, w, h] = overview_card(view);
    (x + w / 2., y + h / 2.)
}
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn view_tab_center(i: usize) -> (f32, f32) {
    (LEFT + i as f32 * (tab_w() + TAB_GAP) + tab_w() / 2., TAB_Y + TAB_H / 2.)
}
pub(crate) const BACK: [f32; 4] = [81.0, 10.0, 26.0, 18.0];

fn view_color(view: View) -> Color32 {
    match view {
        View::Cpu | View::Overview => CPU_COLOR,
        View::Gpu => GPU,
        View::Memory => RAM,
        View::Disk => DISK,
        View::Network => NET,
    }
}
fn secondary(color: Color32) -> Color32 {
    mix(color, INK, 0.5)
}
fn percent(value: Option<f32>) -> String {
    value.map_or_else(|| "—".into(), |n| format!("{n:.0}%"))
}
fn speed(value: Option<f32>) -> String {
    let Some(n) = value else {
        return "—".into();
    };
    for (unit, factor) in [("GB/s", 1e9), ("MB/s", 1e6), ("KB/s", 1e3)] {
        if n >= factor {
            let v = n / factor;
            return if v >= 100. { format!("{v:.0} {unit}") } else { format!("{v:.1} {unit}") };
        }
    }
    format!("{n:.0} B/s")
}
/// 速率拆成数字和单位，大环中心分两行排。
fn speed_parts(value: Option<f32>) -> (String, String) {
    let text = speed(value);
    match text.split_once(' ') {
        Some((n, unit)) => (n.into(), unit.into()),
        None => (text, String::new()),
    }
}
/// 字节数，按 Windows 的习惯用 1024 进制、写作 GB / MB。
fn bytes(n: u64) -> String {
    let v = n as f64;
    if v >= 1024. * GB {
        format!("{:.1} TB", v / 1024. / GB)
    } else if v >= GB {
        if v >= 100. * GB { format!("{:.0} GB", v / GB) } else { format!("{:.1} GB", v / GB) }
    } else if v >= 1048576. {
        format!("{:.0} MB", v / 1048576.)
    } else {
        format!("{:.0} KB", v / 1024.)
    }
}
fn capacity(value: Option<(u64, u64)>) -> String {
    value.map_or_else(|| "—".into(), |(used, total)| {
        let (u, t) = (used as f64 / GB, total as f64 / GB);
        if t >= 100. { format!("{u:.0} / {t:.0} GB") } else { format!("{u:.1} / {t:.1} GB") }
    })
}
fn link(bits: u64) -> String {
    if bits >= 1_000_000_000 { format!("{:.1} Gbps", bits as f64 / 1e9) } else { format!("{:.0} Mbps", bits as f64 / 1e6) }
}
/// 内存构成：程序（已用减去内核常驻）、内核、缓存（可随时回收，在可用里），以及剩下的空闲。
fn memory_parts(r: &Reading) -> ([(&'static str, u64, Color32); 3], u64) {
    let (used, total) = r.memory.unwrap_or((0, 0));
    let kernel = r.memory_kernel.unwrap_or(0).min(used);
    // 可用 = 备用缓存 + 空闲。空闲直接读系统计数器，缓存取差值，三处数字始终对得上；
    // 读不到计数器时才退回系统缓存的大小（它统计口径不同，可能比可用还大，所以要截住）。
    let available = total.saturating_sub(used);
    let (cache, free) = match r.memory_free {
        Some(free) => (available - free.min(available), free.min(available)),
        None => {
            let cache = r.memory_cache.unwrap_or(0).min(available);
            (cache, available - cache)
        }
    };
    // 缓存属于「可用」：颜色压暗一半，在条上和真正被占用的程序、内核明显区分开。
    ([("程序", used - kernel, RAM), ("内核", kernel, KERNEL), ("缓存", cache, mix(mix(RAM, NET, 0.75), SURFACE, 0.5))], free)
}
fn share(value: Option<(u64, u64)>) -> Option<f32> {
    value.filter(|&(_, total)| total > 0).map(|(used, total)| used as f32 / total as f32)
}
fn temperature_color(value: Option<f32>) -> Color32 {
    match value {
        Some(t) if t >= 85. => HOT,
        Some(t) if t >= 75. => RAM,
        _ => INK,
    }
}
fn metric(r: &Reading, i: usize) -> Option<f32> {
    [r.cpu, r.gpu, r.ram()][i]
}
fn sum2(values: [Option<f32>; 2]) -> Option<f32> {
    match values {
        [None, None] => None,
        [a, b] => Some(a.unwrap_or(0.) + b.unwrap_or(0.)),
    }
}
fn resident_value(m: Metric, r: &Reading) -> String {
    match m {
        Metric::Cpu => percent(r.cpu),
        Metric::Gpu => percent(r.gpu),
        Metric::Ram => percent(r.ram()),
        Metric::Vram => r.video_memory.map_or_else(|| "—".into(), |(used, _)| format!("{:.1} GB", used as f64 / GB)),
        Metric::GpuTemp => r.gpu_temp.map_or_else(|| "—".into(), |t| format!("{t:.0}°C")),
        Metric::Download => speed(r.network[0]),
        Metric::Upload => speed(r.network[1]),
        Metric::Read => speed(r.disk[0]),
        Metric::Write => speed(r.disk[1]),
    }
}
fn metric_color(m: Metric) -> Color32 {
    match m {
        Metric::Cpu => CPU_COLOR,
        Metric::Gpu | Metric::Vram | Metric::GpuTemp => GPU,
        Metric::Ram => RAM,
        Metric::Download => NET,
        Metric::Upload => secondary(NET),
        Metric::Read => DISK,
        Metric::Write => secondary(DISK),
    }
}

fn paint_rate_column(c: &Card, left: f32, rates: [Option<Metric>; 2], r: &Reading, divider: bool) {
    if divider {
        c.seg((left - 4., 8.), (left - 4., 22.), 1., GRID);
    }
    for (i, m) in rates.into_iter().enumerate() {
        let Some(m) = m else { continue };
        let y = 9.5 + i as f32 * 11.5;
        c.centered_text(left, y, m.rate_label().unwrap(), 8.5, W::Bold, metric_color(m), H::Left);
        c.centered_text(left + 60., y, &resident_value(m, r), 9.5, W::Bold, INK, H::Right);
    }
}

/// 整数哈希到 0..1，颗粒的位置、相位都由它决定，同一格每帧一致。
fn hash(a: u32, b: u32) -> f32 {
    let mut x = a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA77).rotate_left(13);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.
}

/// 环形占用：从正上方顺时针走，两端圆头，进度弧始终落在轨道里；
/// `glow` 时弧中间再走一道细亮芯，末端一颗亮点，光都收在轨道宽度以内，不往外溢。
fn gauge(c: &Card, x: f32, y: f32, r: f32, width: f32, value: Option<f32>, color: Color32, glow: bool) {
    c.track(x, y, r, width, alpha(color, 0.12));
    let Some(v) = value.map(|v| v.clamp(0., 100.) / 100.).filter(|v| *v > 0.004) else {
        return;
    };
    let (a0, a1) = (-FRAC_PI_2, -FRAC_PI_2 + TAU * v);
    c.arc(x, y, r, a0, a1, width, color);
    for a in [a0, a1] {
        c.circle(x + r * a.cos(), y + r * a.sin(), width / 2., color);
    }
    if glow {
        c.arc(x, y, r, a0, a1, width * 0.3, alpha(mix(color, Color32::WHITE, 0.45), 0.55));
        let (ex, ey) = (x + r * a1.cos(), y + r * a1.sin());
        c.glow(ex, ey, width * 0.5, alpha(mix(color, Color32::WHITE, 0.4), 0.6));
        c.circle(ex, ey, width * 0.2, mix(color, Color32::WHITE, 0.75));
    }
}

/// 点是否落在圆角矩形内（留 `pad` 的边距），颗粒不会漏出圆角。
fn inside_round(rect: [f32; 4], r: f32, px: f32, py: f32, pad: f32) -> bool {
    let [x, y, w, h] = rect;
    let (l, t, rr, b) = (x + pad, y + pad, x + w - pad, y + h - pad);
    if px < l || px > rr || py < t || py > b {
        return false;
    }
    let r = (r - pad).max(0.);
    let cx = px.clamp(l + r, rr - r);
    let cy = py.clamp(t + r, b - r);
    (px - cx).powi(2) + (py - cy).powi(2) <= r * r
}

/// 颗粒网格：3 点间距、1.6 点见方的小方块。
const PITCH: f32 = 3.0;
const DOT: f32 = 1.6;

/// 玻璃体：染色的暗底，上淡下浓。
fn glass_body(c: &Card, rect: [f32; 4], r: f32, color: Color32, k: f32) {
    let [x, y, w, h] = rect;
    c.vgrad(x, y, w, h, r, alpha(color, 0.04 * k), alpha(color, 0.20 * k));
}

/// 玻璃的边：霓虹细边、底边一道亮线、顶边一丝高光。
fn glass_rim(c: &Card, rect: [f32; 4], r: f32, color: Color32, k: f32) {
    let [x, y, w, h] = rect;
    c.stroke(x + 0.5, y + 0.5, w - 1., h - 1., r, 1., alpha(mix(color, Color32::WHITE, 0.1), 0.85 * k));
    let inset = r.max(1.5);
    if w > inset * 2. {
        c.seg((x + inset, y + h - 0.9), (x + w - inset, y + h - 0.9), 1.4, alpha(mix(color, Color32::WHITE, 0.4), k));
        c.seg((x + inset, y + 1.1), (x + w - inset, y + 1.1), 0.8, alpha(Color32::WHITE, 0.10 * k));
    }
}

/// 颗粒：网格按整块玻璃 `shape` 对齐（分段条跨段连续），只画 `area` 里、横向落在 `span` 内的格。
/// `lit` 时越靠底越密、各自闪烁，`energy`（0..1）越高越密、闪得越快；没亮的格留一点底纹。
#[allow(clippy::too_many_arguments)]
fn cells(c: &Card, shape: [f32; 4], r: f32, area: [f32; 4], span: (f32, f32), color: Color32, energy: f32, t: f64, seed: u32, k: f32, lit: bool) {
    let [sx, sy, sw, sh] = shape;
    let [ax, ay, aw, ah] = area;
    let cols = ((sw - 1.4) / PITCH).floor().max(0.) as u32;
    let rows = ((sh - 1.4) / PITCH).floor().max(0.) as u32;
    let ox = sx + (sw - cols as f32 * PITCH) / 2. + (PITCH - DOT) / 2.;
    let oy = sy + (sh - rows as f32 * PITCH) / 2. + (PITCH - DOT) / 2.;
    let energy = energy.clamp(0., 1.);
    let speed = 1.2 + 3.2 * energy;
    let (top, bottom) = (ay, ay + ah);
    for i in 0..cols {
        let px = ox + i as f32 * PITCH;
        if px + DOT / 2. < span.0.max(ax) || px + DOT / 2. > span.1.min(ax + aw) {
            continue;
        }
        for j in 0..rows {
            let py = oy + j as f32 * PITCH;
            if py + DOT / 2. < top || py + DOT / 2. > bottom || !inside_round(shape, r, px + DOT / 2., py + DOT / 2., 0.9) {
                continue;
            }
            if !lit {
                c.fill(px, py, DOT, DOT, 0.3, alpha(color, 0.05 * k));
                continue;
            }
            let depth = ((py + DOT - top) / ah.max(1.)).clamp(0., 1.);
            let chance = (0.16 + 0.6 * energy) * (0.12 + 0.88 * depth.powf(1.6));
            if hash(seed.wrapping_add(i), j) > chance {
                c.fill(px, py, DOT, DOT, 0.3, alpha(color, 0.07 * k));
                continue;
            }
            let phase = hash(j.wrapping_mul(31).wrapping_add(seed), i);
            let twinkle = (0.5 + 0.5 * (t * speed as f64 * (0.6 + phase as f64) + (phase * TAU) as f64).sin()) as f32;
            let tint = mix(color, Color32::WHITE, 0.2 + 0.6 * twinkle * hash(i, j.wrapping_add(seed)));
            c.fill(px, py, DOT, DOT, 0.3, alpha(tint, (0.4 + 0.6 * twinkle) * k));
        }
    }
}

/// 竖直读数柱：一根带颗粒底纹的小圆角玻璃柱，占用从底部点亮（同一外形裁出来，不另贴内胆），顶端一道平直的亮线。
fn tank(c: &Card, rect: [f32; 4], value: Option<f32>, color: Color32, t: f64, seed: u32) {
    let [x, y, w, h] = rect;
    let r = (w * 0.2).min(3.);
    c.fill(x, y, w, h, r, alpha(color, 0.04));
    c.stroke(x + 0.5, y + 0.5, w - 1., h - 1., r, 1., alpha(color, 0.20));
    cells(c, rect, r, rect, (x, x + w), color, 0., t, seed, 1., false);
    let Some(f) = value.map(|v| (v / 100.).clamp(0., 1.)).filter(|f| *f > 0.004) else {
        return;
    };
    let top = y + h - (h * f).max(1.5);
    c.glow(x + w / 2., y + h, w * 1.6, alpha(color, 0.08 + 0.12 * f));
    let lit = c.clipped(x - 2., top, w + 4., y + h - top + 3.);
    glass_body(&lit, rect, r, color, 1.);
    cells(&lit, rect, r, [x, top, w, y + h - top], (x, x + w), color, f, t, seed, 1., true);
    glass_rim(&lit, rect, r, color, 1.);
    if f < 0.995 {
        c.glow(x + w / 2., top + 1., w * 0.8, alpha(color, 0.30));
        c.seg((x + 1.2, top + 0.7), (x + w - 1.2, top + 0.7), 1.4, mix(color, Color32::WHITE, 0.55));
    }
}

/// 横向构成条（像推文里的存储条）：整根条是一块小圆角玻璃，各段按横向范围从同一外形裁出来，
/// 描边、底边亮线和颗粒网格都跨段连续；段与段之间一道淡淡的隔断，两侧颜色互相晕开一点。
/// 已用部分的末端有一道亮边；`focus` 之外的段压暗。
#[allow(clippy::too_many_arguments)]
fn glass_bar(c: &Card, rect: [f32; 4], parts: &[(f32, Color32)], focus: Option<usize>, energy: f32, t: f64, seed: u32, key: &str) {
    let [x, y, w, h] = rect;
    let r = (h * 0.2).min(3.);
    c.fill(x, y, w, h, r, Color32::from_rgb(26, 28, 33));
    c.stroke(x + 0.5, y + 0.5, w - 1., h - 1., r, 1., Color32::from_rgb(46, 49, 57));
    cells(c, rect, r, rect, (x, x + w), Color32::from_gray(150), 0., t, seed, 1., false);
    let ctx = c.ui.ctx().clone();
    let mut left = x;
    let mut cuts: Vec<(f32, Color32, Color32, f32)> = Vec::new();
    let mut last: Option<(Color32, f32)> = None;
    for (i, (part, color)) in parts.iter().enumerate() {
        // 每段宽度平滑过渡，数据变化时条会「长」过去。
        let target = part.clamp(0., 1.) * w;
        let width = if c.live { ctx.animate_value_with_time(egui::Id::new(("performance-glass", key, i)), target, 0.5) } else { target };
        let width = width.min(x + w - left).max(0.);
        if width < 0.5 {
            continue;
        }
        let dim = focus.is_some_and(|f| f != i);
        let k = if dim { 0.3 } else { 1. };
        let seg = c.clipped(left, y - 3., width, h + 6.);
        glass_body(&seg, rect, r, *color, k);
        cells(&seg, rect, r, rect, (left, left + width), *color, energy, t, seed.wrapping_add(i as u32 * 97), k, true);
        glass_rim(&seg, rect, r, *color, k);
        if !dim && focus.is_some() {
            c.glow(left + width / 2., y + h, width.max(h), alpha(*color, 0.25));
        }
        if let Some((prev, pk)) = last {
            cuts.push((left, prev, *color, pk.min(k)));
        }
        last = Some((*color, k));
        left += width;
    }
    // 段间隔断：两侧颜色互相晕开，中间一道细细的亮线。
    for (cx, a, b, k) in cuts {
        let blend = (h * 0.3).clamp(2., 5.);
        let mut mesh = egui::Mesh::default();
        let (top, bottom) = (y + 1.5, y + h - 1.5);
        for (dx, col) in [(-blend, alpha(b, 0.)), (0., alpha(mix(a, b, 0.5), 0.22 * k)), (blend, alpha(a, 0.))] {
            mesh.colored_vertex(c.pos(cx + dx, top), col);
            mesh.colored_vertex(c.pos(cx + dx, bottom), col);
        }
        for q in 0..2u32 {
            let i = q * 2;
            mesh.add_triangle(i, i + 1, i + 2);
            mesh.add_triangle(i + 1, i + 3, i + 2);
        }
        c.painter.add(egui::Shape::mesh(mesh));
        // 一道暗缝把两段分开，缝两侧各一条本段颜色的亮边，一眼看得出是两段。
        c.seg((cx, y + 1.), (cx, y + h - 1.), 2.4, alpha(Color32::from_rgb(12, 13, 16), 0.9 * k.max(0.6)));
        c.seg((cx - 1.7, y + 2.), (cx - 1.7, y + h - 2.), 1., alpha(mix(a, Color32::WHITE, 0.5), 0.8 * k));
        c.seg((cx + 1.7, y + 2.), (cx + 1.7, y + h - 2.), 1., alpha(mix(b, Color32::WHITE, 0.5), 0.8 * k));
    }
    // 已用部分的末端：一道带光的亮边。
    if let Some((color, k)) = last.filter(|_| left < x + w - r.max(1.5)) {
        if left > x + r {
            c.glow(left, y + h / 2., h * 0.7, alpha(color, 0.22 * k));
            c.seg((left, y + 1.5), (left, y + h - 1.5), 1.4, alpha(mix(color, Color32::WHITE, 0.55), k));
        }
    }
}

/// 在压暗的构成条上点亮一小段（从 `from` 到 `to`，按整条的比例），段的两端各一道亮边。
#[allow(clippy::too_many_arguments)]
fn glass_span(c: &Card, rect: [f32; 4], from: f32, to: f32, color: Color32, energy: f32, t: f64, seed: u32) {
    let [x, y, w, h] = rect;
    let r = (h * 0.2).min(3.);
    let (a, b) = (x + w * from.clamp(0., 1.), x + w * to.clamp(0., 1.));
    if b - a < 0.5 {
        return;
    }
    let seg = c.clipped(a, y - 3., b - a, h + 6.);
    glass_body(&seg, rect, r, color, 1.);
    cells(&seg, rect, r, rect, (a, b), color, energy.max(0.6), t, seed, 1., true);
    glass_rim(&seg, rect, r, color, 1.);
    c.glow((a + b) / 2., y + h, (b - a).max(h), alpha(color, 0.3));
    for edge in [a, b] {
        c.seg((edge, y + 1.5), (edge, y + h - 1.5), 1.4, mix(color, Color32::WHITE, 0.6));
    }
}

/// 鼠标落在右列哪一行程序上（行从 `y + 20` 起、行距 `ROW`；可见 `rows` 行，已滚过 `offset` 行，共 `total` 行）。
/// 整个列表区连成一片按最近的行算，行与行之间没有空隙，快速扫过时不会短暂落空。返回在整个列表里的名次。
fn row_hover(c: &Card, y: f32, rows: usize, offset: f32, total: usize) -> Option<usize> {
    let rows = rows.min(total);
    if !c.live || rows == 0 {
        return None;
    }
    let p = c.ui.ctx().pointer_hover_pos().map(|p| c.to_local(p))?;
    let top = y + 20. - 13.5;
    if !(COL_X - 5.0..RIGHT + 5.).contains(&p.x) || !(top..top + rows as f32 * ROW).contains(&p.y) {
        return None;
    }
    Some((((p.y - top) / ROW + offset).max(0.) as usize).min(total - 1))
}

/// CPU、内存详情的程序列表一屏几行；更多的名次滚动查看。
const LIST_ROWS: usize = 5;

/// 可滚动程序列表占的区域（页面坐标）：滚轮落在这里只滚列表，不翻页。
pub(crate) fn list_area() -> egui::Rect {
    egui::Rect::from_min_size(Pos2::new(COL_X - 6., LIST_Y + 7.), egui::vec2(RIGHT - COL_X + 18., LIST_ROWS as f32 * ROW))
}

const PROGRAM_ICON_CACHE_LIMIT: usize = 64;

#[derive(Default)]
struct ProgramIconCache {
    recent: VecDeque<(String, egui::TextureHandle, u64)>,
    drawn: Vec<egui::TextureHandle>,
}

fn program_icon_texture(ctx: &egui::Context, name: &str, image: &egui::ColorImage) -> egui::TextureId {
    let mut first = false;
    let cache = ctx.data_mut(|data| {
        data.get_temp_mut_or_insert_with(egui::Id::new("performance-icon-cache"), || {
            first = true;
            Arc::new(Mutex::new(ProgramIconCache::default()))
        }).clone()
    });
    if first {
        let cache = Arc::downgrade(&cache);
        ctx.on_begin_pass("performance-icon-cache", Arc::new(move |ctx| {
            if ctx.viewport_id() == egui::ViewportId::ROOT {
                if let Some(cache) = cache.upgrade() {
                    cache.lock().unwrap().drawn.clear();
                }
            }
        }));
    }

    let key = name.to_lowercase();
    let pass = ctx.cumulative_pass_nr_for(egui::ViewportId::ROOT);
    let mut cache = cache.lock().unwrap();
    let texture = if let Some(index) = cache.recent.iter().position(|(name, _, _)| name == &key) {
        cache.recent.remove(index).unwrap().1
    } else {
        if cache.recent.len() == PROGRAM_ICON_CACHE_LIMIT {
            let (_, oldest, last_pass) = cache.recent.pop_front().unwrap();
            if last_pass == pass {
                // 这一帧前面画过的图标仍在绘制列表里，等下一次主窗口绘制再释放。
                cache.drawn.push(oldest);
            }
        }
        ctx.load_texture(format!("performance-icon-{key}"), image.clone(), egui::TextureOptions::LINEAR)
    };
    let id = texture.id();
    cache.recent.push_back((key, texture, pass));
    id
}

/// 程序图标：有系统图标就画图标，没有就画一个带首字母的小方块。
fn program_icon(c: &Card, x: f32, y: f32, size: f32, program: &Process, color: Color32) {
    if let Some(image) = &program.icon {
        let texture = program_icon_texture(c.ui.ctx(), &program.name, image);
        let uv = egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.));
        c.painter.image(texture, c.rect(x, y, size, size), uv, Color32::WHITE);
        return;
    }
    c.fill(x, y, size, size, 3., alpha(color, 0.22));
    let letter: String = program.name.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
    c.centered_text(x + size / 2., y + size / 2., &letter, size * 0.62, W::Bold, color, H::Center);
}

/// 霓虹折线：宽而淡的光、中等的光、亮芯三层叠出来。
fn neon(c: &Card, pts: &[(f32, f32)], width: f32, color: Color32) {
    c.path(pts, width * 4.5, alpha(color, 0.06));
    c.path(pts, width * 2.2, alpha(color, 0.16));
    c.path(pts, width, mix(color, Color32::WHITE, 0.18));
}

impl App {
    pub(crate) fn performance_on_bar(&self) -> bool {
        self.module_on(PERFORMANCE_PAGE) && self.settings.show_performance && !self.effective_minimal()
            && (self.settings.performance_bar != BarStyle::Custom || self.settings.performance_metrics != 0)
    }
    /// 岛内当前显示的页面：先看岛内的临时选择，没有就用设置里的。
    fn performance_view(&self) -> View {
        self.performance.view.unwrap_or(self.settings.performance_view)
    }

    /// 当前详情页的程序列表还能往下滚几行；不是 CPU、内存详情或列表不满一屏时为 0。
    pub(crate) fn performance_list_max(&self) -> usize {
        let r = &self.performance.latest;
        let len = match self.performance_view() {
            View::Cpu => r.by_cpu.len(),
            View::Memory => r.by_memory.len(),
            _ => return 0,
        };
        len.saturating_sub(LIST_ROWS)
    }

    /// 列表滚过的行数，按名次一行一行地滑过去。各详情页各记各的动画，换页时不会从别的页的位置滑过来。
    fn list_offset(&self, c: &Card, view: View, len: usize) -> f32 {
        let first = self.performance.list_first.min(len.saturating_sub(LIST_ROWS)) as f32;
        c.ui.ctx().animate_value_with_time(egui::Id::new(("performance-list-offset", view.key())), first, 0.16)
    }

    // ---------- 采样历史 ----------

    /// 最近 `seconds` 秒内某项读数的峰值。
    fn peak(&self, value: impl Fn(&Reading) -> Option<f32>, seconds: f32) -> Option<f32> {
        let now = Instant::now();
        self.performance
            .history
            .iter()
            .filter(|(at, _)| now.saturating_duration_since(*at).as_secs_f32() <= seconds)
            .filter_map(|(_, r)| value(r))
            .reduce(f32::max)
    }

    /// 把最近 `seconds` 秒的读数投到矩形里；采样断档超过 2.5 秒就断开成几段。
    /// `down` 时自上沿向下长（镜像图的下半）。
    fn runs(&self, value: impl Fn(&Reading) -> Option<f32>, rect: [f32; 4], seconds: f32, max: f32, down: bool) -> Vec<Vec<(f32, f32)>> {
        let [x, y, w, h] = rect;
        let now = Instant::now();
        let mut runs: Vec<Vec<(f32, f32)>> = Vec::new();
        let mut last: Option<Instant> = None;
        for (at, sample) in &self.performance.history {
            let age = now.saturating_duration_since(*at).as_secs_f32();
            if age > seconds {
                continue;
            }
            let Some(v) = value(sample) else {
                last = None;
                continue;
            };
            let f = (v / max).clamp(0., 1.);
            let p = (x + w * (1. - age / seconds), if down { y + h * f } else { y + h * (1. - f) });
            match last.filter(|t| at.saturating_duration_since(*t).as_secs_f32() < 2.5) {
                Some(_) => runs.last_mut().unwrap().push(p),
                None => runs.push(vec![p]),
            }
            last = Some(*at);
        }
        runs
    }

    /// 折线 + 向基线渐隐的面积填充。`fill` 为 0 时只画线；`glow` 时线是霓虹三层。返回「现在」那一点。
    #[allow(clippy::too_many_arguments)]
    fn plot(
        &self,
        c: &Card,
        value: impl Fn(&Reading) -> Option<f32>,
        rect: [f32; 4],
        seconds: f32,
        max: f32,
        color: Color32,
        width: f32,
        fill: f32,
        down: bool,
        glow: bool,
    ) -> Option<(f32, f32)> {
        let base = if down { rect[1] } else { rect[1] + rect[3] };
        let runs = self.runs(value, rect, seconds, max, down);
        for run in &runs {
            if fill > 0. && run.len() > 1 {
                let mut mesh = egui::Mesh::default();
                let top = alpha(color, fill);
                for &(px, py) in run {
                    mesh.colored_vertex(c.pos(px, py), top);
                    mesh.colored_vertex(c.pos(px, base), Color32::TRANSPARENT);
                }
                for i in 0..run.len() as u32 - 1 {
                    let k = i * 2;
                    mesh.add_triangle(k, k + 1, k + 2);
                    mesh.add_triangle(k + 1, k + 3, k + 2);
                }
                c.painter.add(egui::Shape::mesh(mesh));
            }
            if run.len() > 1 {
                if glow {
                    neon(c, run, width, color);
                } else {
                    c.path(run, width, color);
                }
            } else {
                c.circle(run[0].0, run[0].1, width * 0.6, color);
            }
        }
        runs.last()
            .and_then(|run| run.last().copied())
            .filter(|p| p.0 >= rect[0] + rect[2] * (1. - 2.5 / seconds))
    }

    /// 没有上限的读数（网速、磁盘）按最近一分钟的峰值定刻度，并留下限，空闲时曲线贴底。
    fn throughput_scale(&self, value: impl Fn(&Reading) -> [Option<f32>; 2], floor: f32) -> f32 {
        let a = self.peak(|r| value(r)[0], 60.).unwrap_or(0.);
        let b = self.peak(|r| value(r)[1], 60.).unwrap_or(0.);
        (a.max(b) * 1.15).max(floor)
    }

    /// 逐核：每个逻辑处理器一根柱。`rich` 时每根是一只小玻璃液位，否则是简洁的渐变柱（常驻用）。
    /// 核数太多时相邻的核合并成一根（取平均）。
    fn core_bars(&self, c: &Card, rect: [f32; 4], max_cols: usize, rich: Option<f64>) {
        let [x, y, w, h] = rect;
        let loads = &self.performance.latest.cores;
        let group = loads.len().div_ceil(max_cols).max(1);
        let cols: Vec<f32> = if loads.is_empty() {
            vec![0.; 8]
        } else {
            loads.chunks(group).map(|g| g.iter().sum::<f32>() / g.len() as f32).collect()
        };
        let slot = w / cols.len() as f32;
        let bar = (slot * 0.62).max(1.5);
        let ctx = c.ui.ctx().clone();
        // 每根柱最近 10 秒的峰值，画成柱里的一道短横线，看得出刚才冲到过多高。
        let peaks: Vec<f32> = if rich.is_some() {
            let now = Instant::now();
            let mut peaks = vec![0f32; cols.len()];
            for (at, sample) in &self.performance.history {
                if now.saturating_duration_since(*at).as_secs_f32() > 10. || sample.cores.len() != loads.len() {
                    continue;
                }
                for (i, g) in sample.cores.chunks(group).enumerate().take(peaks.len()) {
                    peaks[i] = peaks[i].max(g.iter().sum::<f32>() / g.len() as f32);
                }
            }
            peaks
        } else {
            Vec::new()
        };
        for (i, load) in cols.iter().enumerate() {
            let bx = x + slot * i as f32 + (slot - bar) / 2.;
            match rich {
                Some(t) => {
                    let shown = if c.live { ctx.animate_value_with_time(egui::Id::new(("performance-core", i)), *load, 0.4) } else { *load };
                    let color = if shown >= 85. { mix(CPU_COLOR, HOT, ((shown - 85.) / 15.).clamp(0., 1.)) } else { CPU_COLOR };
                    tank(c, [bx, y, bar, h], Some(shown), color, t, 11 + i as u32 * 7);
                    let peak = peaks.get(i).copied().unwrap_or(0.).max(shown);
                    if peak - shown > 3. && h > 12. {
                        let py = y + h - (h * peak / 100.).clamp(1.5, h - 1.5);
                        c.seg((bx + 1.2, py), (bx + bar - 1.2, py), 1.2, alpha(mix(color, Color32::WHITE, 0.5), 0.75));
                    }
                }
                None => {
                    let f = (load / 100.).clamp(0., 1.);
                    c.fill(bx, y, bar, h, bar / 2., Color32::from_rgb(30, 38, 40));
                    if f > 0.01 {
                        let lh = (h * f).max(bar);
                        c.vgrad(bx, y + h - lh, bar, lh, bar / 2., mix(CPU_COLOR, Color32::WHITE, 0.35), alpha(CPU_COLOR, 0.55));
                    }
                }
            }
        }
    }

    // ---------- 常驻：顶部岛 ----------

    pub(crate) fn paint_performance_bar(&self, c: &Card, x: f32) {
        self.paint_performance_style(c, x, self.settings.performance_bar);
    }
    fn paint_performance_style(&self, c: &Card, x: f32, style: BarStyle) {
        let r = &self.performance.latest;
        match style {
            BarStyle::Custom => {
                let mask = self.settings.performance_metrics;
                let stacked = mask.count_ones() > 3;
                let start = x - BarStyle::Custom.width(mask) / 2.;
                let mut left = start;
                for (first, second, width) in BarStyle::value_columns(mask) {
                    if stacked {
                        // 两行一列；单数时最后一列只占上一行，字号与其它列一致。
                        for (m, y) in std::iter::once((first, 9.5)).chain(second.map(|m| (m, 21.))) {
                            c.page_link(left,if y<15. {0.}else{15.},width,15.,("performance-metric",m as u8),Destination::Performance(m.view()));
                            c.centered_text(left + 2., y, m.label(), 8., W::Semibold, metric_color(m), H::Left);
                            c.centered_text(left + width - 4., y, &resident_value(m, r), 9., W::Bold, INK, H::Right);
                        }
                    } else {
                        c.page_link(left,0.,width,30.,("performance-metric",first as u8),Destination::Performance(first.view()));
                        let cx = left + width / 2.;
                        c.centered_text(cx, 8.5, first.label(), 8., W::Semibold, metric_color(first), H::Center);
                        c.centered_text(cx, 20.5, &resident_value(first, r), 12., W::Bold, INK, H::Center);
                    }
                    left += width;
                }
                for rates in BarStyle::rate_columns(mask) {
                    c.page_link(left,0.,BarStyle::RATE_WIDTH,30.,("performance-rate",rates[0].or(rates[1]).unwrap() as u8),Destination::Performance(rates[0].or(rates[1]).unwrap().view()));
                    paint_rate_column(c, left + 3., rates, r, left > start);
                    left += BarStyle::RATE_WIDTH;
                }
            }
            BarStyle::Gauges | BarStyle::Dashboard => {
                let dashboard = style == BarStyle::Dashboard;
                let left_edge = x - style.width(self.settings.performance_metrics) / 2.;
                if dashboard {
                    c.page_link(left_edge,0.,30.,30.,"performance-dashboard-cpu",Destination::Performance(View::Cpu));
                    self.plot(c, |s| s.cpu, [left_edge, 8., 26., 14.], 16., 100., CPU_COLOR, 1.4, 0.35, false, false);
                    for (j, rates) in Metric::RATE_PAIRS.into_iter().enumerate() {
                        let left = left_edge + 178. + j as f32 * BarStyle::RATE_WIDTH;
                        c.page_link(left,0.,BarStyle::RATE_WIDTH,30.,("performance-dashboard-rate",j),Destination::Performance(rates[0].view()));
                        paint_rate_column(c, left, rates.map(Some), r, true);
                    }
                }
                for i in 0..3 {
                    let left = left_edge + if dashboard { 30. } else { 1. } + i as f32 * 49.;
                    c.page_link(left,0.,49.,30.,("performance-gauge",i),Destination::Performance(Metric::ALL[i].view()));
                    let value = metric(r, i);
                    gauge(c, left + 8., 15., 7., 2.2, value, COLORS[i], false);
                    c.centered_text(left + 8., 15., LETTERS[i], 7.5, W::Bold, COLORS[i], H::Center);
                    c.centered_text(left + 19., 15., &percent(value), 11., W::Bold, INK, H::Left);
                }
            }
            BarStyle::Cores => {
                c.page_link(x-52.,0.,104.,30.,"performance-cores",Destination::Performance(View::Cpu));
                let left = x - BarStyle::Cores.width(0) / 2.;
                c.centered_text(left + 2., 8.5, "CPU", 8., W::Semibold, CPU_COLOR, H::Left);
                c.centered_text(left + 2., 20.5, &percent(r.cpu), 12., W::Bold, INK, H::Left);
                self.core_bars(c, [left + 40., 6., 64., 18.], 16, None);
            }
            BarStyle::Pulse => {
                c.page_link(x-39.,0.,78.,30.,"performance-pulse",Destination::Performance(View::Cpu));
                c.centered_text(x - 38., 8.5, "CPU", 8., W::Semibold, CPU_COLOR, H::Left);
                c.centered_text(x - 38., 20.5, &percent(r.cpu), 12., W::Bold, INK, H::Left);
                c.seg((x - 3., 22.5), (x + 37., 22.5), 1., GRID);
                if let Some(p) = self.plot(c, |s| s.cpu, [x - 3., 7., 40., 15.], 16., 100., CPU_COLOR, 1.5, 0.4, false, false) {
                    c.circle(p.0, p.1, 1.8, CPU_COLOR);
                }
            }
            BarStyle::Network => {
                c.page_link(x-44.,0.,88.,30.,"performance-network",Destination::Performance(View::Network));
                for (i, (arrow, col)) in [("↓", NET), ("↑", secondary(NET))].into_iter().enumerate() {
                    let cy = 9.5 + i as f32 * 11.5;
                    c.centered_text(x - 40., cy, arrow, 11., W::Bold, col, H::Left);
                    c.centered_text(x + 40., cy, &speed(r.network[i]), 10.5, W::Bold, INK, H::Right);
                }
            }
        }
    }
    pub(crate) fn performance_bar(&mut self, c: &Card, x: f32) {
        self.paint_performance_bar(c, x);

    }

    // ---------- 常驻：侧边停靠 ----------

    pub(crate) fn side_performance(&mut self, c: &Card, x: f32, y: f32, height: f32) {
        // 和横条用同一套元素：带字母的小圆环、CPU 标签加读数、负载曲线、彩色箭头加白色读数，只是竖着排。
        let r = self.performance.latest.clone();
        let top = y - height / 2.;
        // 一只小圆环：环里是 C / G / M，下面是百分比。
        let ring = |c: &Card, cy: f32, i: usize| {
            c.page_link(x-16.,cy-9.,32.,30.,("side-performance-gauge",i),Destination::Performance(Metric::ALL[i].view()));
            let value = metric(&r, i);
            gauge(c, x, cy, 7., 2.2, value, COLORS[i], false);
            c.centered_text(x, cy, LETTERS[i], 7.5, W::Bold, COLORS[i], H::Center);
            c.centered_text(x, cy + 14., &percent(value), 9., W::Bold, INK, H::Center);
        };
        // 两行读数：左边彩色标签，右边白色数值，数值放不下时缩小字号。
        let pair = |c: &Card, cy: f32, rates: [Option<Metric>; 2]| {
            for (i, m) in rates.into_iter().enumerate() {
                let Some(m) = m else { continue };
                let row = cy + i as f32 * 11.;
                c.page_link(x-16.,row-5.5,32.,11.,("side-performance-rate",m as u8),Destination::Performance(m.view()));
                let value = resident_value(m, &r).replace("B/s", "").replace(' ', "");
                let size = 8.5 * (22. / c.text_w(&value, 8.5, W::Bold)).min(1.);
                c.centered_text(x - 15., row, m.rate_label().unwrap(), 7.5, W::Bold, metric_color(m), H::Left);
                c.centered_text(x + 16., row, &value, size, W::Bold, INK, H::Right);
            }
        };
        let cpu_label = |c: &Card, top: f32| {
            c.page_link(x-16.,top,32.,46.,"side-performance-cpu",Destination::Performance(View::Cpu));
            c.centered_text(x, top + 6., "CPU", 7.5, W::Semibold, CPU_COLOR, H::Center);
            c.centered_text(x, top + 17., &percent(r.cpu), 10., W::Bold, INK, H::Center);
        };
        match self.settings.performance_bar {
            BarStyle::Custom => {
                let mask = self.settings.performance_metrics;
                let mut top = top;
                for m in Metric::ALL.into_iter().filter(|m| mask & m.bit() != 0 && m.rate_label().is_none()) {
                    c.page_link(x-16.,top,32.,23.,("side-performance-custom",m as u8),Destination::Performance(m.view()));
                    let value = resident_value(m, &r).replace(' ', "");
                    let size = 10. * (30. / c.text_w(&value, 10., W::Bold)).min(1.);
                    c.centered_text(x, top + 6., m.label(), 7.5, W::Semibold, metric_color(m), H::Center);
                    c.centered_text(x, top + 16., &value, size, W::Bold, INK, H::Center);
                    top += 23.;
                }
                for rates in BarStyle::rate_columns(mask) {
                    pair(c, top + 6., rates);
                    top += 23.;
                }
            }
            BarStyle::Gauges => {
                for i in 0..3 {
                    ring(c, top + 9. + i as f32 * 30., i);
                }
            }
            BarStyle::Dashboard => {
                c.page_link(x-16.,top,32.,22.,"side-performance-dashboard-cpu",Destination::Performance(View::Cpu));
                self.plot(c, |s| s.cpu, [x - 14., top + 3., 28., 12.], 16., 100., CPU_COLOR, 1.4, 0.35, false, false);
                c.seg((x - 12., top + 19.), (x + 12., top + 19.), 1., GRID);
                for i in 0..3 {
                    ring(c, top + 31. + i as f32 * 30., i);
                }
                c.seg((x - 12., top + 112.), (x + 12., top + 112.), 1., GRID);
                pair(c, top + 120., Metric::RATE_PAIRS[0].map(Some));
                pair(c, top + 145., Metric::RATE_PAIRS[1].map(Some));
            }
            BarStyle::Cores => {
                cpu_label(c, top);
                self.core_bars(c, [x - 13., top + 27., 26., 16.], 6, None);
            }
            BarStyle::Pulse => {
                cpu_label(c, top);
                c.seg((x - 14., top + 43.), (x + 14., top + 43.), 1., GRID);
                if let Some(p) = self.plot(c, |s| s.cpu, [x - 14., top + 27., 28., 15.], 16., 100., CPU_COLOR, 1.5, 0.4, false, false) {
                    c.circle(p.0, p.1, 1.8, CPU_COLOR);
                }
            }
            BarStyle::Network => pair(c, top + 11., Metric::RATE_PAIRS[0].map(Some)),
        }

    }

    // ---------- 展开页 ----------

    pub(crate) fn page_performance(&mut self, c: &Card) {
        let ctx = c.ui.ctx().clone();
        let t = ctx.input(|i| i.time);
        // 颗粒闪烁按固定节奏重绘，只在页面真正显示时。
        if c.live {
            ctx.request_repaint_after(SHIMMER);
        }
        let view = self.performance_view();
        // 换到别的详情页时程序列表回到顶部；同一页收起再展开还停在原处。
        if view != self.performance.list_view {
            self.performance.list_view = view;
            self.performance.list_first = 0;
        }
        self.performance.list_first = self.performance.list_first.min(self.performance_list_max());
        // 总览 ↔ 详情切换时整页交叉淡入，并有一点缩放，像推进去 / 退出来。
        // 动画状态不区分可交互与否：展开到一半（只画不交互）和展开完成共用一份，重新展开时不会从旧值再跳一次。
        let depth = ctx.animate_bool_with_time(egui::Id::new("performance-depth"), view != View::Overview, 0.26);
        let r = self.performance.latest.clone();
        if depth < 1. {
            let k = 1. - depth;
            let layer = c.faded(k * k).scaled(209.5, PAGE_H / 2., 1. + 0.04 * depth);
            let layer = if view == View::Overview { layer } else { layer.ghost() };
            self.performance_overview(&layer, &r, t);
        }
        if depth > 0. {
            let layer = c.faded(depth * depth).scaled(209.5, PAGE_H / 2., 0.96 + 0.04 * depth);
            let layer = if view == View::Overview { layer.ghost() } else { layer };
            // 退回总览的途中仍然画上一个详情页。
            let shown = if view == View::Overview { self.performance.last_detail } else { view };
            self.performance_detail(&layer, shown, &r, t);
        }
        if view != View::Overview {
            self.performance.last_detail = view;
        }
    }

    // ---------- 总览 ----------

    fn performance_overview(&mut self, c: &Card, r: &Reading, t: f64) {
        let ctx = c.ui.ctx().clone();
        // ---- CPU、显卡、内存：大数字 + 读数柱 + 一分钟曲线，底部一条各自的「构成」 ----
        let subs = [
            r.cpu_mhz.map(|m| format!("{:.2} GHz", m / 1000.)).unwrap_or_default(),
            r.gpu_temp.map(|v| format!("{v:.0} °C")).unwrap_or_else(|| r.gpu_name.replace("(R)", "").replace("Intel ", "").replace("NVIDIA ", "").replace("AMD ", "")),
            capacity(r.memory),
        ];
        let (parts, _) = memory_parts(r);
        let (used, total) = r.memory.unwrap_or((0, 0));
        // 核显几乎没有专用显存（几十到一两百 MB），这时总览改看共享显存。
        let integrated = r.shared_memory.is_some() && r.video_memory.is_none_or(|(_, total)| total < 512 * 1048576);
        let vram = if integrated { r.shared_memory } else { r.video_memory };
        let rows = [
            ("逻辑处理器", if r.cores.is_empty() { "—".to_string() } else { r.cores.len().to_string() }),
            (if integrated { "共享显存" } else { "专用显存" }, capacity(vram)),
            ("可用", r.memory.map_or_else(|| "—".into(), |_| bytes(total.saturating_sub(used)))),
        ];
        for (i, view) in [View::Cpu, View::Gpu, View::Memory].into_iter().enumerate() {
            let [x, y, w, _] = top_card(i);
            let color = COLORS[i];
            let value = metric(r, i);
            let hover = self.overview_card_frame(c, overview_card(view), view, view.key(), color);
            c.circle(x + 12., y + 15., 2.4, color);
            c.centered_text(x + 19., y + 15., view.label(), 10., W::Regular, mix(MUTED, INK, hover), H::Left);
            if let Some(p) = self.peak(|s| metric(s, i), 60.) {
                c.centered_text(x + w - 10., y + 15., &format!("峰值 {p:.0}%"), 8.5, W::Regular, FAINT, H::Right);
            }
            let shown = value.map(|v| if c.live { ctx.animate_value_with_time(egui::Id::new(("performance-top", i)), v, 0.45) } else { v });
            match shown {
                Some(v) => {
                    let nw = c.text(x + 11., y + 54., &format!("{v:.0}"), 26., W::Bold, INK, H::Left);
                    c.text(x + 12.5 + nw, y + 54., "%", 11., W::Semibold, MUTED, H::Left);
                }
                None => {
                    c.text(x + 11., y + 54., "—", 26., W::Bold, FAINT, H::Left);
                }
            }
            let sub_color = if i == 1 && r.gpu_temp.is_some() { temperature_color(r.gpu_temp) } else { MUTED };
            let sub_color = if sub_color == INK { MUTED } else { sub_color };
            c.text(x + 11., y + 69., &c.fit(&subs[i], 9.5, W::Regular, w - 50.), 9.5, W::Regular, sub_color, H::Left);
            self.plot(c, |s| metric(s, i), [x + 10., y + 74., w - 50., 16.], 60., 100., color, 1.1, 0.22, false, true);
            tank(c, [x + w - 29., y + 25., 19., 65.], shown, color, t, 3 + i as u32 * 131);
            let (label, amount) = &rows[i];
            c.text(x + 11., y + 104., label, 8.5, W::Regular, MUTED, H::Left);
            c.text(x + w - 10., y + 104., &c.fit(amount, 8.5, W::Bold, w - 60.), 8.5, W::Bold, INK, H::Right);
            let strip = [x + 10., y + 109., w - 20., 16.];
            match i {
                0 => self.core_bars(c, strip, 16, Some(t)),
                1 => glass_bar(c, strip, &[(share(vram).unwrap_or(0.), color)], None, r.gpu.unwrap_or(0.) / 100., t, 5, "overview-vram"),
                _ => {
                    let whole = total.max(1) as f32;
                    glass_bar(c, strip, &parts.map(|(_, n, col)| (n as f32 / whole, col)), None, r.ram().unwrap_or(0.) / 100., t, 17, "overview-memory");
                }
            }
        }
        // ---- 磁盘、网络：窄条，两项读数一行，背后垫一层淡淡的一分钟曲线 ----
        for (j, view) in [View::Disk, View::Network].into_iter().enumerate() {
            let [x, y, w, h] = io_card(j);
            let color = view_color(view);
            let hover = self.overview_card_frame(c, overview_card(view), view, view.key(), color);
            let read = move |s: &Reading| if j == 0 { s.disk } else { s.network };
            let (labels, note, floor) = if j == 0 {
                (["读", "写"], r.disk_active.map(|a| format!("活动 {a:.0}%")), 1e6)
            } else {
                (["↓", "↑"], r.link_speed.map(link), 64e3)
            };
            let max = self.throughput_scale(read, floor);
            let chart = c.clipped(x + 1., y + 1., w - 2., h - 2.);
            self.plot(&chart, move |s| sum2(read(s)), [x + 1., y + 18., w - 2., h - 19.], 60., max * 2., alpha(color, 0.35 + 0.3 * hover), 1., 0.12, false, false);
            c.circle(x + 12., y + 14., 2.4, color);
            c.centered_text(x + 19., y + 14., view.label(), 10., W::Regular, mix(MUTED, INK, hover), H::Left);
            if let Some(note) = note {
                c.centered_text(x + w - 10., y + 14., &note, 8.5, W::Regular, FAINT, H::Right);
            }
            let values = read(r);
            for k in 0..2 {
                let left = x + 11. + k as f32 * (w - 18.) / 2.;
                let tint = if k == 0 { color } else { secondary(color) };
                let lw = c.text(left, y + 36., labels[k], 9.5, W::Bold, tint, H::Left);
                c.text(left + lw + 5., y + 36., &speed(values[k]), 11., W::Bold, if values[k].is_some() { INK } else { FAINT }, H::Left);
            }
        }
        // ---- 程序：左右两栏，一栏看谁最占 CPU、一栏看谁最占内存，各自只有一个读数 ----
        self.programs_table(c, r);
    }

    /// 总览底部的程序区：「CPU 最高」和「内存最多」并排，每栏前四名。
    /// 每行背后一条淡淡的液位条，长度是它占本栏第一名的比例；不需要排序，也不跳转。
    fn programs_table(&self, c: &Card, r: &Reading) {
        let [x, y, w, h] = APP_CARD;
        c.fill(x, y, w, h, 11., SURFACE);
        c.stroke(x + 0.5, y + 0.5, w - 1., h - 1., 10.5, 1., alpha(INK, 0.06));
        let half = (w - 1.) / 2.;
        c.seg((x + half + 0.5, y + 12.), (x + half + 0.5, y + h - 12.), 0.8, GRID);
        let columns: [(&str, View, &[Process], Box<dyn Fn(&Process) -> (f32, String)>); 2] = [
            ("CPU 最高", View::Cpu, &r.by_cpu, Box::new(|p: &Process| (p.cpu, format!("{:.1}%", p.cpu)))),
            ("内存最多", View::Memory, &r.by_memory, Box::new(|p: &Process| (p.memory as f32, bytes(p.memory)))),
        ];
        for (k, (title, view, list, value)) in columns.into_iter().enumerate() {
            let color = view_color(view);
            let left = x + k as f32 * (half + 1.) + 10.;
            let right = left + half - 20.;
            c.circle(left + 2.5, y + 13., 2.4, color);
            c.centered_text(left + 9., y + 13., title, 9., W::Regular, MUTED, H::Left);
            if list.is_empty() {
                c.text(left, y + 44., "正在统计…", 9.5, W::Regular, FAINT, H::Left);
                continue;
            }
            let top = value(&list[0]).0.max(0.001);
            for (i, program) in list.iter().take(4).enumerate() {
                let row_y = y + 24. + i as f32 * 21.;
                let (amount, text) = value(program);
                let share = (amount / top).clamp(0.04, 1.);
                // 液位条：从左边长出来，越靠后的名次越淡。
                let k = 1. - i as f32 * 0.16;
                let lw = (right - left + 6.) * share;
                c.hgrad(left - 3., row_y, lw, 18., 5., alpha(color, 0.05 * k), alpha(color, 0.20 * k));
                c.seg((left - 3. + lw - 0.7, row_y + 4.), (left - 3. + lw - 0.7, row_y + 14.), 1.2, alpha(mix(color, Color32::WHITE, 0.4), 0.8 * k));
                let base = row_y + 13.;
                program_icon(c, left, base - 10.5, 13., program, color);
                let tw = c.text_w(&text, 9.5, W::Bold);
                c.text(left + 18., base, &c.fit(&program.name, 9.5, W::Regular, right - left - 24. - tw), 9.5, W::Regular, INK, H::Left);
                c.text(right, base, &text, 9.5, W::Bold, INK, H::Right);
            }
        }
    }

    /// 总览卡片的底板与交互：悬停时浮起、描上主色并在底边亮起一道光；点击进入对应详情。返回悬停程度。
    fn overview_card_frame(&mut self, c: &Card, rect: [f32; 4], view: View, key: &str, color: Color32) -> f32 {
        let [x, y, w, h] = rect;
        let response = c.hit(x, y, w, h, ("performance-card", key)).on_hover_cursor(egui::CursorIcon::PointingHand);
        let (hover, press) = c.hover_t(&response);
        c.fill(x, y, w, h, 11., mix(SURFACE, RAISED, hover * 0.8));
        c.vgrad(x, y, w, h, 11., alpha(color, 0.), alpha(color, 0.035 + 0.03 * hover));
        c.stroke(x + 0.5, y + 0.5, w - 1., h - 1., 10.5, 1., alpha(color, 0.12 + 0.45 * hover));
        if hover > 0. {
            c.glow(x + w / 2., y + h, w * 0.55, alpha(color, 0.18 * hover));
            c.seg((x + 14., y + h - 0.8), (x + w - 14., y + h - 0.8), 1.2, alpha(color, 0.7 * hover));
        }
        if press > 0. {
            c.fill(x, y, w, h, 11., Color32::from_white_alpha((press * 10.) as u8));
        }
        if response.clicked() {
            self.performance.view = Some(view);
        }
        hover
    }

    // ---------- 详情 ----------

    fn performance_detail(&mut self, c: &Card, view: View, r: &Reading, t: f64) {
        let fade = self.performance_tabs(c, view);
        self.performance_back(c);
        let body = c.faded(fade);
        let color = view_color(view);
        body.seg((151., 92.), (151., BODY_BOTTOM), 0.8, GRID);
        match view {
            View::Cpu | View::Overview => self.performance_cpu(&body, r, color, t),
            View::Gpu => self.performance_gpu(&body, r, color, t),
            View::Memory => self.performance_memory(&body, r, color, t),
            View::Disk => self.performance_disk(&body, r, color, t),
            View::Network => self.performance_network(&body, r, color),
        }
    }

    /// 标题后的返回胶囊：回到总览。
    fn performance_back(&mut self, c: &Card) {
        let [x, y, w, h] = BACK;
        let response = c.hit(x, y - 3., w, h + 6., "performance-back").on_hover_text("返回总览");
        let (hover, _) = c.hover_t(&response);
        c.fill(x, y, w, h, h / 2., mix(Color32::from_rgb(30, 30, 33), RAISED, hover));
        let ink = mix(Color32::from_gray(150), INK, hover);
        let (cx, cy) = (x + w / 2. - 0.5, y + h / 2.);
        c.path(&[(cx + 2., cy - 4.), (cx - 2., cy), (cx + 2., cy + 4.)], 1.6, ink);
        if response.clicked() {
            self.performance.view = Some(View::Overview);
        }
    }

    /// 页签条：五个页签，每个带名称、读数和一分钟迷你曲线；选中底板随页签滑动。
    fn performance_tabs(&mut self, c: &Card, view: View) -> f32 {
        let ctx = c.ui.ctx().clone();
        let current = View::DETAILS.iter().position(|v| *v == view).unwrap_or(0);
        let slide = ctx.animate_value_with_time(egui::Id::new("performance-tab-slide"), current as f32, 0.22);
        let r = self.performance.latest.clone();
        let w = tab_w();
        let sx = LEFT + slide * (w + TAB_GAP);
        let accent = view_color(view);
        c.fill(sx, TAB_Y, w, TAB_H, 9., RAISED);
        c.stroke(sx + 0.5, TAB_Y + 0.5, w - 1., TAB_H - 1., 8.5, 1., alpha(accent, 0.5));
        // 选中页签只在自己底边亮一道线，不往下面的大环上漏光。
        c.seg((sx + 12., TAB_Y + TAB_H - 1.), (sx + w - 12., TAB_Y + TAB_H - 1.), 1.2, alpha(accent, 0.7));
        for (i, tab) in View::DETAILS.into_iter().enumerate() {
            let x = LEFT + i as f32 * (w + TAB_GAP);
            let color = view_color(tab);
            let response = c.hit(x, TAB_Y, w, TAB_H, ("performance-view", i));
            let (hover, _) = c.hover_t(&response);
            let on = (1. - (slide - i as f32).abs()).clamp(0., 1.);
            if on < 0.99 {
                c.fill(x, TAB_Y, w, TAB_H, 9., alpha(mix(SURFACE, RAISED, hover * 0.6), 1. - on));
            }
            c.circle(x + 9., TAB_Y + 11., 2.2, color);
            c.centered_text(x + 15., TAB_Y + 11., tab.label(), 9., W::Regular, mix(MUTED, INK, on), H::Left);
            let (value, max, series): (String, f32, Box<dyn Fn(&Reading) -> Option<f32>>) = match tab {
                View::Gpu => (percent(r.gpu), 100., Box::new(|s: &Reading| s.gpu)),
                View::Memory => (percent(r.ram()), 100., Box::new(|s: &Reading| s.ram())),
                View::Disk => (speed(sum2(r.disk)), self.throughput_scale(|s| [sum2(s.disk), None], 1e6), Box::new(|s: &Reading| sum2(s.disk))),
                View::Network => (speed(r.network[0]), self.throughput_scale(|s| s.network, 64e3), Box::new(|s: &Reading| s.network[0])),
                View::Cpu | View::Overview => (percent(r.cpu), 100., Box::new(|s: &Reading| s.cpu)),
            };
            let size = if value.ends_with("/s") { 10.5 } else { 12.5 };
            c.text(x + 8., TAB_Y + 29., &c.fit(&value, size, W::Bold, w - 12.), size, W::Bold, INK, H::Left);
            self.plot(c, series, [x + 7., TAB_Y + 33., w - 14., 7.], 60., max, alpha(color, 0.5 + 0.5 * on), 1., 0.3 * on, false, false);
            if response.clicked() && i != current {
                self.performance.view = Some(tab);
            }
        }
        let fade = 1. - (slide - current as f32).abs().min(1.);
        if c.live && fade < 1. {
            ctx.request_repaint();
        }
        0.15 + 0.85 * fade
    }

    /// 左列大环：环表示占用，中心是数字和一行小字；数值变化时环平滑过渡。
    fn hero(&self, c: &Card, key: &str, value: Option<f32>, big: &str, unit: &str, color: Color32) {
        let ctx = c.ui.ctx().clone();
        let shown = value.map(|v| if c.live { ctx.animate_value_with_time(egui::Id::new(("performance-hero", key)), v, 0.45) } else { v });
        gauge(c, HERO_X, HERO_Y, HERO_R, 7., shown, color, true);
        let size = if big.chars().count() > 4 { 17. } else { 22. };
        c.centered_text(HERO_X, HERO_Y - 4., big, size, W::Bold, if value.is_some() { INK } else { FAINT }, H::Center);
        c.centered_text(HERO_X, HERO_Y + 15., unit, 9., W::Regular, MUTED, H::Center);
    }
    /// 左列大环下面的四行读数。
    fn hero_rows(&self, c: &Card, rows: [(&str, String, Color32); 4]) {
        for (i, (label, value, ink)) in rows.into_iter().enumerate() {
            let base = 238. + i as f32 * 24.;
            c.text(LEFT, base, label, 9.5, W::Regular, MUTED, H::Left);
            c.text(SIDE_R, base, &c.fit(&value, 10.5, W::Bold, 70.), 10.5, W::Bold, ink, H::Right);
        }
    }
    /// 右列小节标题：左侧名称，右侧补充说明。
    fn section(&self, c: &Card, y: f32, title: &str, note: &str, note_color: Color32) {
        c.text(COL_X, y, title, 9., W::Semibold, MUTED, H::Left);
        if !note.is_empty() {
            c.text(RIGHT, y, &c.fit(note, 9., W::Regular, COL_W - c.text_w(title, 9., W::Semibold) - 12.), 9., W::Regular, note_color, H::Right);
        }
    }
    /// 程序排行：图标、名称、发光细条和数值；悬停的行有一层底色。还没有数据时显示一行灰字。
    #[allow(clippy::too_many_arguments)]
    /// `scroll` 为 `Some(滚过的行数)` 时整张列表都画出来、裁在 `limit` 行的窗口里，右边一条细滚动条；否则只画前 `limit` 名。
    fn program_rows(&self, c: &Card, y: f32, limit: usize, programs: &[Process], scroll: Option<f32>, color: Color32, value: impl Fn(&Process) -> (f32, String)) {
        if programs.is_empty() {
            c.text(COL_X, y + 20., "正在统计…", 10., W::Regular, FAINT, H::Left);
            return;
        }
        let offset = scroll.unwrap_or(0.);
        let total = if scroll.is_some() { programs.len() } else { programs.len().min(limit) };
        let hovered = row_hover(c, y, limit, offset, total);
        let top = programs.first().map_or(1., |p| value(p).0.max(0.001));
        let outer = c;
        let window = c.clipped(COL_X - 6., y + 7., RIGHT - COL_X + 12., limit as f32 * ROW);
        let c = &window;
        for (i, program) in programs.iter().take(total).enumerate() {
            let base = y + 20. + (i as f32 - offset) * ROW;
            if base < y + 20. - ROW || base > y + 20. + limit as f32 * ROW {
                continue;
            }
            if hovered == Some(i) {
                c.fill(COL_X - 5., base - 13., COL_W + 10., 17., 5., RAISED);
            }
            let (amount, text) = value(program);
            let tint = mix(color, SURFACE, (i as f32 * 0.14).min(0.56));
            program_icon(c, COL_X, base - 10.5, 13., program, color);
            c.text(COL_X + 19., base, &c.fit(&program.name, 10., W::Regular, 94.), 10., W::Regular, INK, H::Left);
            let bar_x = COL_X + 118.;
            let len = (50. * amount / top).clamp(2., 50.);
            c.fill(bar_x, base - 5., 50., 3., 1.5, alpha(color, 0.12));
            c.fill(bar_x, base - 5.5, len, 4., 2., alpha(tint, 0.25));
            c.fill(bar_x, base - 5., len, 3., 1.5, tint);
            c.circle(bar_x + len - 1.5, base - 3.5, 1.4, mix(tint, Color32::WHITE, 0.5));
            c.text(RIGHT, base, &text, 10., W::Bold, INK, H::Right);
        }
        // 细滚动条：轨道是窗口的高度，滑块长度是可见行占全部的比例。
        if scroll.is_some() && total > limit {
            let (track_y, track_h) = (y + 9., limit as f32 * ROW - 4.);
            let thumb = (track_h * limit as f32 / total as f32).max(12.);
            let at = track_y + (track_h - thumb) * (offset / (total - limit) as f32).clamp(0., 1.);
            let k = if hovered.is_some() { 0.75 } else { 0.4 };
            outer.fill(RIGHT + 7., track_y, 2., track_h, 1., alpha(INK, 0.06));
            outer.fill(RIGHT + 7., at, 2., thumb, 1., alpha(color, k));
        }
    }
    /// 60 秒霓虹面积图，带 50% 参考线和「现在」光点。
    fn trend(&self, c: &Card, value: impl Fn(&Reading) -> Option<f32>, max: f32, color: Color32, rect: [f32; 4]) {
        let [x, y, w, h] = rect;
        c.seg((x, y + h / 2.), (x + w, y + h / 2.), 0.6, GRID);
        c.seg((x, y + h), (x + w, y + h), 1., GRID);
        if let Some((x, y)) = self.plot(c, value, rect, 60., max, color, 1.5, 0.3, false, true) {
            c.glow(x, y, 9., alpha(color, 0.5));
            c.circle(x, y, 2.6, mix(color, Color32::WHITE, 0.3));
        }
    }
    /// 上下镜像图：上半是主读数（下载 / 读取），下半是次读数（上传 / 写入），共用一个刻度。
    fn mirror(&self, c: &Card, value: impl Fn(&Reading) -> [Option<f32>; 2] + Copy, max: f32, color: Color32, height: f32) {
        let half = height / 2.;
        let mid = VIZ_Y + half;
        self.plot(c, move |s| value(s)[0], [COL_X, VIZ_Y, COL_W, half - 1.], 60., max, color, 1.3, 0.32, false, true);
        self.plot(c, move |s| value(s)[1], [COL_X, mid + 1., COL_W, half - 1.], 60., max, secondary(color), 1.1, 0.22, true, true);
        c.seg((COL_X, mid), (RIGHT, mid), 1., GRID);
    }

    fn performance_cpu(&self, c: &Card, r: &Reading, color: Color32, t: f64) {
        self.hero(c, "cpu", r.cpu, &percent(r.cpu), "占用", color);
        let user = r.cpu.zip(r.cpu_kernel).map(|(all, kernel)| (all - kernel).max(0.));
        self.hero_rows(c, [
            ("频率", r.cpu_mhz.map_or_else(|| "—".into(), |m| format!("{:.2} GHz", m / 1000.)), INK),
            ("用户态", percent(user), INK),
            ("内核态", percent(r.cpu_kernel), INK),
            ("一分钟峰值", percent(self.peak(|s| s.cpu, 60.)), INK),
        ]);
        let user_peak = self.peak(|s| s.cpu, 60.).map(|p| format!("峰值 {p:.0}%")).unwrap_or_default();
        self.section(c, VIZ_Y - 7., "最近一分钟", &user_peak, FAINT);
        self.trend(c, |s| s.cpu, 100., color, [COL_X, VIZ_Y, COL_W, 56.]);
        // 逐核只是一条窄窄的参考，不抢主图。
        let busiest = r.cores.iter().copied().reduce(f32::max).map(|v| format!("最忙的核 {v:.0}%")).unwrap_or_default();
        let title = if r.cores.is_empty() { "逻辑处理器".to_string() } else { format!("逻辑处理器 · {}", r.cores.len()) };
        self.section(c, 180., &title, &busiest, FAINT);
        self.core_bars(c, [COL_X, 186., COL_W, 26.], 32, Some(t));
        self.section(c, LIST_Y, "占用最高的程序", "", FAINT);
        let offset = self.list_offset(c, View::Cpu, r.by_cpu.len());
        self.program_rows(c, LIST_Y, LIST_ROWS, &r.by_cpu, Some(offset), color, |p| (p.cpu, format!("{:.1}%", p.cpu)));
    }

    fn performance_gpu(&self, c: &Card, r: &Reading, color: Color32, t: f64) {
        self.hero(c, "gpu", r.gpu, &percent(r.gpu), "占用", color);
        self.hero_rows(c, [
            ("温度", r.gpu_temp.map_or_else(|| "—".into(), |v| format!("{v:.0} °C")), temperature_color(r.gpu_temp)),
            ("专用显存", r.video_memory.map_or_else(|| "—".into(), |(used, _)| bytes(used)), INK),
            ("共享显存", r.shared_memory.map_or_else(|| "—".into(), |(used, _)| bytes(used)), INK),
            ("一分钟峰值", percent(self.peak(|s| s.gpu, 60.)), INK),
        ]);
        let peak = self.peak(|s| s.gpu, 60.).map(|p| format!("峰值 {p:.0}%")).unwrap_or_default();
        self.section(c, VIZ_Y - 7., "最近一分钟", &peak, FAINT);
        self.trend(c, |s| s.gpu, 100., color, [COL_X, VIZ_Y, COL_W, 56.]);
        // 引擎：3D、视频解码、视频编码、复制……各一行，同类取最忙的那个。
        let bar_x = COL_X + 50.;
        self.section(c, 180., "引擎", "", FAINT);
        let engines: Vec<_> = r.gpu_engines.iter().take(4).collect();
        if engines.is_empty() {
            c.text(COL_X, 198., "显卡驱动没有提供引擎信息", 9.5, W::Regular, FAINT, H::Left);
        }
        for (i, (label, load)) in engines.iter().enumerate() {
            let base = 198. + i as f32 * ROW;
            c.text(COL_X, base, label, 9.5, W::Regular, MUTED, H::Left);
            glass_bar(c, [bar_x, base - 10., RIGHT - 40. - bar_x, 12.], &[(load / 100., color)], None, load / 100., t, 41 + i as u32, &format!("gpu-engine-{label}"));
            c.text(RIGHT, base, &percent(Some(*load)), 9.5, W::Bold, INK, H::Right);
        }
        // 显存：专用（显卡自带）和共享（借用系统内存，核显主要用它）。
        let top = 198. + engines.len().max(1) as f32 * ROW + 8.;
        let name = if r.gpu_name.is_empty() { "显卡型号未知" } else { &r.gpu_name };
        c.text(COL_X, top, &c.fit(name, 9., W::Semibold, COL_W - 40.), 9., W::Semibold, color, H::Left);
        c.text(RIGHT, top, "显存", 9., W::Regular, FAINT, H::Right);
        for (i, (label, memory)) in [("专用", r.video_memory), ("共享", r.shared_memory)].into_iter().enumerate() {
            let base = top + 20. + i as f32 * ROW;
            c.text(COL_X, base, label, 9.5, W::Regular, MUTED, H::Left);
            glass_bar(c, [bar_x, base - 10., RIGHT - 78. - bar_x, 12.], &[(share(memory).unwrap_or(0.), color)], None, r.gpu.unwrap_or(0.) / 100., t, 5 + i as u32, ["vram", "shared-vram"][i]);
            c.text(RIGHT, base, &capacity(memory), 9.5, W::Bold, INK, H::Right);
        }
    }

    fn performance_memory(&self, c: &Card, r: &Reading, color: Color32, t: f64) {
        self.hero(c, "memory", r.ram(), &percent(r.ram()), "已用", color);
        let (used, total) = r.memory.unwrap_or((0, 0));
        self.hero_rows(c, [
            ("已用", r.memory.map_or_else(|| "—".into(), |_| bytes(used)), INK),
            ("可用", r.memory.map_or_else(|| "—".into(), |_| bytes(total.saturating_sub(used))), INK),
            ("缓存", r.memory.map_or_else(|| "—".into(), |_| bytes(memory_parts(r).0[2].1)), INK),
            ("总量", r.memory.map_or_else(|| "—".into(), |_| bytes(total)), INK),
        ]);
        let (parts, free) = memory_parts(r);
        let whole = total.max(1) as f32;
        let bar = [COL_X, VIZ_Y - 1., COL_W, 28.];
        let legend_y = VIZ_Y + 44.;
        let legend_w = COL_W / 2.;
        let legend = |i: usize| (COL_X + (i % 2) as f32 * legend_w, legend_y + (i / 2) as f32 * 16.);
        // 只有悬停构成条本身时突出对应的一段，其余压暗，标题右侧换成这一段的大小；图例只跟着变暗，自己不响应。
        let pointer = c.ui.ctx().pointer_hover_pos().map(|p| c.to_local(p));
        let mut focus = None;
        // 「可用」= 缓存 + 空闲（缓存随时能让给程序用），在条上是已用之后的整段；悬停这一段时整段一起亮。
        let available = total.saturating_sub(used);
        let used_share = if c.live { c.ui.ctx().animate_value_with_time(egui::Id::new("performance-memory-available"), used as f32 / whole, 0.5) } else { used as f32 / whole };
        let mut available_hover = false;
        if let Some(p) = pointer.filter(|_| c.live) {
            if (bar[1]..bar[1] + bar[3]).contains(&p.y) && (bar[0]..bar[0] + bar[2]).contains(&p.x) {
                let at = (p.x - bar[0]) / bar[2];
                if at >= used_share {
                    available_hover = true;
                    focus = Some(2);
                } else {
                    let mut acc = 0.;
                    focus = parts.iter().position(|(_, n, _)| {
                        acc += *n as f32 / whole;
                        at < acc
                    });
                }
            }
        }
        // 悬停下面的某个程序：整条压暗，只点亮「程序」段里属于它的那一小截（按排行顺序从左往右排）。
        // 在列表里上下移动时亮块滑到下一个程序，不会先全亮再亮起。
        let programs = &r.by_memory;
        let offset = self.list_offset(c, View::Memory, programs.len());
        let hovered = row_hover(c, LIST_Y, LIST_ROWS, offset, programs.len());
        let ctx = c.ui.ctx().clone();
        // 刚移进列表时亮块直接出现在当前行，之后在行间移动才滑过去。
        let was = ctx.data_mut(|d| std::mem::replace(d.get_temp_mut_or_default::<bool>(egui::Id::new("performance-memory-span-on")), hovered.is_some()));
        let glide = if was { 0.14 } else { 0. };
        let span = hovered.map(|i| {
            let limit = parts[0].1 as f32 / whole;
            let before: u64 = programs[..i].iter().map(|p| p.memory).sum();
            let from = (before as f32 / whole).min(limit);
            let to = (from + programs[i].memory as f32 / whole).min(limit);
            let ease = |id: &str, v: f32| ctx.animate_value_with_time(egui::Id::new(("performance-memory-span", id)), v, glide);
            (ease("from", from), ease("to", to))
        });
        let note = match (hovered, focus) {
            (Some(i), _) => format!("{} {}", programs[i].name, bytes(programs[i].memory)),
            (None, _) if available_hover => format!("可用 {}", bytes(available)),
            (None, Some(i)) => format!("{} {}", parts[i].0, bytes(parts[i].1)),
            (None, None) => format!("可用 {}", bytes(available)),
        };
        let focus = if hovered.is_some() { Some(0) } else { focus };
        self.section(c, VIZ_Y - 7., "内存构成", &note, if focus.is_some() { INK } else { FAINT });
        let energy = r.ram().unwrap_or(0.) / 100.;
        // 悬停「可用」时条本身不点亮任何一段：可用是余量，只用括号和一圈淡描边圈出来，和点亮某一段区分开。
        let bar_focus = if span.is_some() { Some(usize::MAX) } else if available_hover { None } else { focus };
        glass_bar(c, bar, &parts.map(|(_, n, col)| (n as f32 / whole, col)), bar_focus, energy, t, 17, "memory");
        if let Some((from, to)) = span {
            glass_span(c, bar, from, to, color, energy, t, 17);
        }
        // 条下方一道细括号标出「可用」那一整段（缓存 + 空闲）。悬停时已用部分轻轻压暗，
        // 可用段只描一圈淡淡的边、括号稍微提亮，不发光，比悬停程序、内核那种点亮要安静。
        let [bx, by, bw, bh] = bar;
        let a = bx + bw * used_share.clamp(0., 1.);
        if bx + bw - a > 6. {
            let cache = mix(RAM, NET, 0.75);
            let ink = alpha(cache, if available_hover { 0.85 } else { 0.55 });
            let y = by + bh + 5.;
            c.seg((a + 1., y), (bx + bw - 1., y), 1., ink);
            for x in [a + 1., bx + bw - 1.] {
                c.seg((x, y - 3.), (x, y + 0.5), 1., ink);
            }
            if available_hover {
                c.fill(bx + 1., by + 1., (a - bx - 1.).max(0.), bh - 2., 2., alpha(Color32::BLACK, 0.35));
                c.stroke(a + 0.5, by + 0.5, (bx + bw - a - 1.).max(0.), bh - 1., 2., 1., alpha(cache, 0.4));
            }
        }
        let rows = parts.iter().copied().chain([("空闲", free, Color32::from_rgb(58, 62, 70))]);
        for (i, (label, amount, col)) in rows.enumerate() {
            let (x, y) = legend(i);
            let dim = focus.is_some_and(|f| f != i) && !(available_hover && i == 3);
            c.circle(x + 3., y - 3.5, 2.5, if dim { alpha(col, 0.35) } else { col });
            c.text(x + 10., y, label, 9., W::Regular, if dim { FAINT } else { MUTED }, H::Left);
            c.text(x + legend_w - 10., y, &bytes(amount), 9., W::Bold, if dim { FAINT } else { INK }, H::Right);
        }
        let peak = self.peak(|s| s.ram(), 60.).map(|p| format!("峰值 {p:.0}%")).unwrap_or_default();
        self.section(c, 182., "最近一分钟", &peak, FAINT);
        self.trend(c, |s| s.ram(), 100., color, [COL_X, 188., COL_W, 28.]);
        self.section(c, LIST_Y, "占用内存最多的程序", "", FAINT);
        self.program_rows(c, LIST_Y, LIST_ROWS, &r.by_memory, Some(offset), color, |p| (p.memory as f32, bytes(p.memory)));
    }

    fn performance_disk(&self, c: &Card, r: &Reading, color: Color32, t: f64) {
        self.hero(c, "disk", r.disk_active, &percent(r.disk_active), "活动时间", color);
        self.hero_rows(c, [
            ("读取", speed(r.disk[0]), INK),
            ("写入", speed(r.disk[1]), INK),
            ("合计", speed(sum2(r.disk)), INK),
            ("一分钟峰值", speed(self.peak(|s| sum2(s.disk), 60.)), INK),
        ]);
        let max = self.throughput_scale(|s| s.disk, 1e6);
        self.section(c, VIZ_Y - 7., "读取 ↑   写入 ↓", &format!("刻度 {}", speed(Some(max))), FAINT);
        self.mirror(c, |s| s.disk, max, color, 56.);
        let more = r.volumes.len().saturating_sub(3);
        self.section(c, 180., "本地分区", &if more > 0 { format!("另有 {more} 个分区") } else { String::new() }, FAINT);
        if r.volumes.is_empty() {
            c.text(COL_X, 198., "正在统计…", 10., W::Regular, FAINT, H::Left);
        }
        let energy = r.disk_active.unwrap_or(0.) / 100.;
        for (i, volume) in r.volumes.iter().take(3).enumerate() {
            let base = 198. + i as f32 * ROW;
            let used = volume.used as f32 / volume.total.max(1) as f32;
            let tint = if used >= 0.9 { HOT } else { color };
            c.text(COL_X, base, &format!("{}:", volume.letter), 10., W::Bold, INK, H::Left);
            glass_bar(c, [COL_X + 20., base - 11., 112., 14.], &[(used, tint)], None, energy, t, 23 + i as u32, &format!("volume-{}", volume.letter));
            c.text(RIGHT, base, &capacity(Some((volume.used, volume.total))), 10., W::Bold, INK, H::Right);
        }
        // 读写最多的程序：按进程的读写字节数（含文件、管道等全部读写），和任务管理器的「磁盘」列口径不完全一样。
        let list_y = 198. + r.volumes.len().clamp(1, 3) as f32 * ROW + 6.;
        let limit = (((BODY_BOTTOM - 10. - (list_y + 20.)) / ROW).floor() as usize + 1).min(5);
        self.section(c, list_y, "读写最多的程序", "", FAINT);
        self.program_rows(c, list_y, limit, &r.by_io, None, color, |p| (p.io, speed(Some(p.io))));
    }

    fn performance_network(&self, c: &Card, r: &Reading, color: Color32) {
        // 环是对数刻度的速度表（1 KB/s 到 1 GB/s），平常的几十 KB/s 也看得出起伏；中心直接写下载速度。
        let dial = r.network[0].map(|rate| (rate.max(1e3) / 1e3).log10() / 6. * 100.);
        let busy = r
            .link_speed
            .zip(r.network[0].into_iter().chain(r.network[1]).reduce(f32::max))
            .map(|(link, rate)| (rate * 800. / link as f32).min(100.));
        let (big, unit) = speed_parts(r.network[0]);
        self.hero(c, "network", dial, &big, &format!("{unit} 下载"), color);
        self.hero_rows(c, [
            ("上传", speed(r.network[1]), INK),
            ("链路", r.link_speed.map_or_else(|| "—".into(), link), INK),
            ("链路占用", percent(busy), INK),
            ("一分钟峰值", speed(self.peak(|s| s.network[0], 60.)), INK),
        ]);
        let max = self.throughput_scale(|s| s.network, 64e3);
        self.section(c, VIZ_Y - 7., "下载 ↑   上传 ↓", &format!("刻度 {}", speed(Some(max))), FAINT);
        self.mirror(c, |s| s.network, max, color, VIZ_H);
        self.section(c, LIST_Y, "网卡启用以来", "", FAINT);
        let both = r.net_total.map(|t| t[0] + t[1]);
        for (i, (label, amount, tint)) in [("已接收", r.net_total.map(|t| t[0]), color), ("已发送", r.net_total.map(|t| t[1]), secondary(color)), ("合计", both, FAINT)]
            .into_iter()
            .enumerate()
        {
            let base = LIST_Y + 20. + i as f32 * ROW;
            c.fill(COL_X, base - 8., 3., 9., 1.5, tint);
            c.text(COL_X + 9., base, label, 10., W::Regular, INK, H::Left);
            c.text(RIGHT, base, &amount.map_or_else(|| "—".into(), bytes), 10., W::Bold, INK, H::Right);
        }
    }

    // ---------- 设置页 ----------

    /// 设置里的样式缩略：一段岛身，里面直接画常驻样式本身，宽的样式整体缩小。
    fn performance_style_preview(&self, ui: &egui::Ui, style: BarStyle, rect: egui::Rect) {
        let width = style.width(self.settings.performance_metrics).max(40.);
        let body_w = width + 18.;
        let scale = (rect.width() / body_w).min(1.);
        let body = egui::Rect::from_center_size(rect.center(), egui::vec2(body_w * scale, 30. * scale));
        ui.painter().rect_filled(body, 15. * scale, Color32::BLACK);
        if style == BarStyle::Custom && self.settings.performance_metrics == 0 {
            ui.painter().circle_stroke(body.center(), 5., egui::Stroke::new(1.4, FAINT));
            return;
        }
        let mut card = Card::new(ui, Pos2::new(body.left(), body.top()), body);
        card.s = scale;
        self.paint_performance_style(&card.ghost(), body_w / 2., style);
    }

    pub(crate) fn performance_settings(&mut self, ui: &mut egui::Ui) {
        use crate::extras::{setting_row, settings_card, settings_label, settings_segmented};
        use crate::settings_theme::color as theme_color;
        let ctx = ui.ctx().clone();
        settings_label(ui, "常驻显示");
        settings_card(ui, |ui| {
            setting_row(ui, "显示在岛上", "点击指标可展开系统性能", &mut self.settings.show_performance);
            ui.separator();
            ui.add_enabled_ui(self.settings.show_performance, |ui| {
                ui.spacing_mut().item_spacing.y = 6.;
                for style in BarStyle::ALL {
                    let selected = self.settings.performance_bar == style;
                    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 46.), egui::Sense::click());
                    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                    let fill = if selected {
                        theme_color(&ctx, Color32::from_rgb(46, 52, 62), Color32::from_rgb(232, 241, 252))
                    } else if response.hovered() {
                        theme_color(&ctx, Color32::from_rgb(44, 44, 48), Color32::from_rgb(242, 244, 247))
                    } else {
                        theme_color(&ctx, Color32::from_rgb(29, 29, 32), Color32::from_rgb(246, 247, 249))
                    };
                    ui.painter().rect_filled(rect, 9., fill);
                    if selected {
                        ui.painter().rect_stroke(
                            rect,
                            9.,
                            egui::Stroke::new(1.2, theme_color(&ctx, Color32::from_rgb(92, 160, 240), Color32::from_rgb(62, 132, 230))),
                            egui::StrokeKind::Inside,
                        );
                    }
                    let ink = theme_color(&ctx, Color32::WHITE, Color32::from_rgb(30, 34, 41));
                    let weak = theme_color(&ctx, Color32::from_gray(160), Color32::from_gray(96));
                    ui.painter().text(rect.left_top() + egui::vec2(12., 15.), egui::Align2::LEFT_CENTER, style.label(), egui::FontId::proportional(13.), ink);
                    ui.painter().text(rect.left_top() + egui::vec2(12., 32.), egui::Align2::LEFT_CENTER, style.summary(), egui::FontId::proportional(11.), weak);
                    let preview = egui::Rect::from_min_max(egui::pos2(rect.left() + 128., rect.top() + 6.), egui::pos2(rect.right() - 10., rect.bottom() - 6.));
                    if preview.width() > 40. {
                        self.performance_style_preview(ui, style, preview);
                    }
                    if response.clicked() && !selected {
                        self.settings.performance_bar = style;
                    }
                    // 自由搭配的指标开关紧跟在它自己的卡片下面。
                    if style == BarStyle::Custom && self.settings.performance_bar == BarStyle::Custom {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(6., 6.);
                            ui.add_space(4.);
                            for m in Metric::ALL {
                                let checked = self.settings.performance_metrics & m.bit() != 0;
                                if metric_chip(ui, m.label(), metric_color(m), checked).clicked() {
                                    self.settings.performance_metrics ^= m.bit();
                                }
                            }
                        });
                        ui.add_space(4.);
                    }
                }
                let hint = match self.settings.performance_bar {
                    BarStyle::Custom => {
                        if self.settings.performance_metrics == 0 { "未选择指标，常驻区已收起。" } else { "仅在岛上显示勾选的指标。" }
                    }
                    BarStyle::Gauges => "依次显示 CPU、显卡和内存占用率。",
                    BarStyle::Cores => "显示处理器占用率，每个逻辑处理器一根柱，按负载升高；核心较多时相邻的合成一根。",
                    BarStyle::Pulse => "显示 CPU 占用率和最近 16 秒的负载曲线。",
                    BarStyle::Dashboard => "横向并排显示负载曲线、三项占用率、网速和磁盘读写；侧边停靠时同样的内容竖着排。",
                    BarStyle::Network => "分别显示物理网卡的下载与上传速度。",
                };
                crate::extras::settings_hint(ui, hint);
            });
        });
        settings_label(ui, "展开页");
        settings_card(ui, |ui| {
            settings_segmented(ui, &mut self.settings.performance_view, &View::ALL.map(|v| (v, v.label())), 44.);
            crate::extras::settings_hint(ui, "展开时先显示的页面。总览里点卡片进入详情，标题旁的返回键回到总览；移开鼠标收起后再展开，仍停在刚才的页面，点常驻指标或切到其它页面后从这里开始。硬件未提供的读数显示为 —。");
            crate::extras::settings_hint(ui, "程序排行和分区容量只在展开页打开时统计，收起后停止。");
        });
    }
}

/// 自由搭配的指标开关：胶囊按钮，选中时染上指标色。
fn metric_chip(ui: &mut egui::Ui, label: &str, color: Color32, on: bool) -> egui::Response {
    use crate::settings_theme::color as theme_color;
    let ctx = ui.ctx().clone();
    let font = egui::FontId::proportional(12.);
    let text_w = ctx.fonts(|f| f.layout_no_wrap(label.into(), font.clone(), Color32::WHITE).size().x);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(text_w + 30., 26.), egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let dark = ui.visuals().dark_mode;
    let base = theme_color(&ctx, Color32::from_rgb(29, 29, 32), Color32::from_rgb(242, 244, 247));
    let fill = if on {
        mix(base, color, if dark { 0.22 } else { 0.28 })
    } else if response.hovered() {
        theme_color(&ctx, Color32::from_rgb(44, 44, 48), Color32::from_rgb(232, 235, 240))
    } else {
        base
    };
    ui.painter().rect_filled(rect, 13., fill);
    let edge = if on { alpha(color, 0.8) } else { theme_color(&ctx, Color32::from_rgb(58, 58, 63), Color32::from_rgb(214, 218, 224)) };
    ui.painter().rect_stroke(rect, 13., egui::Stroke::new(1., edge), egui::StrokeKind::Inside);
    let dot = rect.left_center() + egui::vec2(12., 0.);
    if on {
        ui.painter().circle_filled(dot, 3., color);
    } else {
        ui.painter().circle_stroke(dot, 2.6, egui::Stroke::new(1.1, theme_color(&ctx, Color32::from_gray(120), Color32::from_gray(150))));
    }
    let ink = theme_color(
        &ctx,
        if on { Color32::WHITE } else { Color32::from_gray(200) },
        if on { Color32::from_rgb(30, 34, 41) } else { Color32::from_gray(80) },
    );
    ui.painter().text(rect.left_center() + egui::vec2(20., 0.), egui::Align2::LEFT_CENTER, label, font, ink);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw_program_icons(ctx: &egui::Context, names: &[usize]) -> egui::FullOutput {
        let image = std::sync::Arc::new(egui::ColorImage::new([1, 1], Color32::WHITE));
        ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let card = Card::new(ui, Pos2::ZERO, ui.max_rect());
                for name in names {
                    let process = Process {
                        name: format!("App-{name}"),
                        icon: Some(image.clone()),
                        ..Default::default()
                    };
                    program_icon(&card, 0., 0., 13., &process, INK);
                }
            });
        })
    }

    fn program_icon_textures(ctx: &egui::Context) -> std::collections::HashMap<String, egui::TextureId> {
        ctx.tex_manager()
            .read()
            .allocated()
            .filter_map(|(&id, meta)| meta.name.strip_prefix("performance-icon-").map(|name| (name.to_owned(), id)))
            .collect()
    }

    #[test]
    fn performance_program_icons_evict_least_recently_used_textures() {
        let ctx = egui::Context::default();
        for index in 0..64 {
            draw_program_icons(&ctx, &[index]);
        }
        let original = program_icon_textures(&ctx);
        assert_eq!(original.len(), 64);
        let output = draw_program_icons(&ctx, &[0, 64]);
        let current = program_icon_textures(&ctx);
        assert_eq!(current.len(), 64);
        assert_eq!(current["app-0"], original["app-0"]);
        assert!(!current.contains_key("app-1"));
        assert!(output.textures_delta.free.contains(&original["app-1"]));
        assert!(current.contains_key("app-64"));
    }

    #[test]
    fn performance_program_icons_hold_evicted_drawn_textures_until_next_pass() {
        let ctx = egui::Context::default();
        let names: Vec<_> = (0..65).collect();
        let first = draw_program_icons(&ctx, &names);
        let used = program_icon_textures(&ctx);
        assert_eq!(used.len(), names.len());
        assert!(!first.textures_delta.free.contains(&used["app-0"]));
        let second = draw_program_icons(&ctx, &[]);
        assert!(second.textures_delta.free.contains(&used["app-0"]));
        assert_eq!(program_icon_textures(&ctx).len(), 64);
    }

    /// 可用 = 缓存 + 空闲，悬停、图例和左栏三处用的是同一组数，任何读数下都对得上。
    #[test]
    fn memory_cache_and_free_always_add_up_to_available() {
        let gb = 1u64 << 30;
        for (free, cache) in [(Some(gb), Some(9 * gb)), (Some(20 * gb), None), (None, Some(9 * gb)), (None, Some(gb)), (Some(0), None)] {
            let r = Reading { memory: Some((12 * gb, 16 * gb)), memory_free: free, memory_cache: cache, memory_kernel: Some(gb), ..Default::default() };
            let (parts, free) = memory_parts(&r);
            assert_eq!(parts[2].1 + free, 4 * gb, "缓存 {} + 空闲 {free} 应等于可用", parts[2].1);
            assert_eq!(parts[0].1 + parts[1].1, 12 * gb);
        }
    }
}
