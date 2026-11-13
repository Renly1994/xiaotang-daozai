//! 额度卡片的釉面绘制：绿→黄→红配色、高光同心环、彩虹 Gemini 星。
//!
//! 这里只负责像素：调用方传百分比与几何，函数自己处理渐变、内侧亮边、
//! 外圈柔光和端头平切（egui 的描边画不出渐变，所以环用自建 Mesh 画）。

use crate::paint::{mix, Card};
use eframe::egui::{self, Color32, Shape, Vec2};
use std::f32::consts::{FRAC_PI_2, PI, TAU};

/// 卡片底色（径向渐变的中心）。
/// 卡片底色（径向渐变的外缘）。
/// 星的四尖配色：上红、右蓝、下绿、左黄。
const TIPS: [(f32, [u8; 3]); 4] = [
    (-FRAC_PI_2, [0xFF, 0x4D, 0x4D]),
    (0.0, [0x3B, 0x82, 0xF6]),
    (FRAC_PI_2, [0x22, 0xC5, 0x5E]),
    (PI, [0xFA, 0xCC, 0x15]),
];

/// 百分比色标：100% 绿、75% 黄绿、50% 黄、35% 橙、20% 红、0% 深红。
const STOPS: [(f32, [u8; 3]); 6] = [
    (100.0, [0x00, 0xE5, 0xA0]),
    (75.0, [0xA8, 0xE0, 0x5A]),
    (50.0, [0xFF, 0xD1, 0x66]),
    (35.0, [0xFF, 0xB0, 0x20]),
    (20.0, [0xFF, 0x4D, 0x3D]),
    (0.0, [0xE8, 0x32, 0x1E]),
];

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round().clamp(0.0, 255.0) as u8
}

fn lerp_rgb(a: [u8; 3], b: [u8; 3], t: f32) -> Color32 {
    Color32::from_rgb(lerp_u8(a[0], b[0], t), lerp_u8(a[1], b[1], t), lerp_u8(a[2], b[2], t))
}

/// 剩余百分比 → 颜色。越接近用完越红，且中途不平滑到黑。
pub fn quota_color(pct: f32) -> Color32 {
    let pct = if pct.is_finite() { pct.clamp(0.0, 100.0) } else { 0.0 };
    for pair in STOPS.windows(2) {
        let (hi, high) = pair[0];
        let (lo, low) = pair[1];
        if pct <= hi && pct >= lo {
            let t = if (hi - lo).abs() < f32::EPSILON { 0.0 } else { (pct - lo) / (hi - lo) };
            return lerp_rgb(low, high, t);
        }
    }
    lerp_rgb(STOPS[5].1, STOPS[5].1, 0.0)
}

/// 重置倒计时：不到一天用「时:分:秒」，一天以上用「X天 X小时 X分钟」。
pub fn countdown_text(reset: Option<i64>, now: i64) -> Option<String> {
    let reset = reset?;
    let left = reset - now;
    if left <= 0 {
        return Some("已重置".into());
    }
    Some(if left < 86_400 {
        let (hours, rest) = (left / 3_600, left % 3_600);
        format!("{hours:02}:{:02}:{:02}", rest / 60, rest % 60)
    } else {
        let (days, rest) = (left / 86_400, left % 86_400);
        format!("{days}天 {}小时 {}分钟", rest / 3_600, (rest % 3_600) / 60)
    })
}

/// 卡片底色：先铺 #23272C 圆角底，再从中心往外压暗到 #0A0B0D。
/// 同心环的半径表：size 是外径，count 决定环数与环宽。
pub fn ring_layout(size: f32, count: usize, micro: bool) -> (f32, f32, Vec<f32>) {
    let count = count.max(1);
    let (width, gap) = match (micro, count) {
        (true, 1) => (3.0, 0.0),
        (true, 2) => (2.2, 1.5),
        (true, _) => (1.7, 0.8),
        (false, 1) => ((size * 0.102).clamp(3.0, 6.5), 0.0),
        (false, 2) => (5.0, 3.6),
        (false, _) => (4.2, 3.0),
    };
    let outer = size / 2.0 - width / 2.0 - 1.0;
    let radii = (0..count).map(|i| outer - i as f32 * (width + gap)).collect();
    (width, gap, radii)
}

/// 一圈釉面进度环：白底轨 + 柔光 + 渐变主体 + 内侧亮边 + 顶部高光。
///
/// 弧从 12 点开始顺时针，端头平切（Mesh 自己就是平头，不用圆帽）。
#[allow(clippy::too_many_arguments)]
pub fn glossy_ring(c: &Card, x: f32, y: f32, r: f32, width: f32, pct: f32, base: Color32, micro: bool, cuts: &[(f32, f32)]) {
    let pct = if pct.is_finite() { pct.clamp(0.0, 100.0) } else { 0.0 };
    // 27px 下底轨会被抗锯齿糊成一圈亮灰，小环干脆不画；卡片上留一条很淡的。\n    if !micro { c.ring(x, y, r, width, Color32::from_white_alpha(14)); }
    let sweep = TAU * pct / 100.0;
    if sweep <= 0.001 {
        return;
    }
    let start = -FRAC_PI_2;
    // 27px 下柔光会把环糊成一圈灰白，小环不画。
    if !micro { c.arc(x, y, r, start, start + sweep, width, base.gamma_multiply(0.10)); }
    // 27px 的环只有 2px 宽：Mesh 的三角形在这里是硬边，直接给锯齿；
    // egui 的 Shape::line 自带羽化，小尺寸改用描边，颜色照样按剩余量走。
    if micro {
        // 星尖压在环上的地方留缺口：只画没被星盖住的那几段。
        for (a0, a1) in arc_segments(start, start + sweep, cuts) {
            c.arc(x, y, r, a0, a1, width, base);
            c.arc(x, y, r - width * 0.22, a0, a1, width * 0.34, mix(base, Color32::WHITE, 0.34));
        }
        return;
    }    let n = ((sweep / 1.5f32.to_radians()).ceil() as usize).clamp(12, 240);
    let inner_r = (r - width / 2.0).max(0.5);
    let outer_r = r + width / 2.0;
    let light = mix(base, Color32::WHITE, 0.12);
    let dark = mix(base, Color32::BLACK, 0.10);
    // 内外各加一圈 alpha 0 的羽化边：Mesh 的三角形是硬边，不加这一圈在 1x 上就是锯齿。
    let feather = 0.75;
    let clear = Color32::TRANSPARENT;
    let mut mesh = egui::Mesh::default();
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let a = start + sweep * t;
        // 沿弧走一圈明度：起点最亮、末尾略暗，像光从上面扫下来。
        let along = if t < 0.5 { mix(light, base, t * 2.0) } else { mix(base, dark, (t - 0.5) * 2.0) };
        let (sin, cos) = a.sin_cos();
        let at = |radius: f32| c.pos(x + radius * cos, y + radius * sin);
        mesh.colored_vertex(at(outer_r + feather), clear);
        mesh.colored_vertex(at(outer_r), mix(along, Color32::BLACK, 0.12));
        mesh.colored_vertex(at(inner_r), mix(along, Color32::WHITE, 0.18));
        mesh.colored_vertex(at((inner_r - feather).max(0.2)), clear);
    }
    for i in 0..n as u32 {
        let b = i * 4;
        for band in 0..3u32 {
            let (a0, a1) = (b + band, b + band + 4);
            mesh.add_triangle(a0, a1, a0 + 1);
            mesh.add_triangle(a0 + 1, a1, a1 + 1);
        }
    }
    c.painter.add(Shape::mesh(mesh));
    c.arc(x, y, r - width * 0.26, start, start + sweep, width * 0.10, Color32::from_rgba_unmultiplied(255, 255, 255, 26));
}

/// 四尖的角度：上、右、下、左。
pub const STAR_TIPS: [f32; 4] = [-FRAC_PI_2, 0.0, FRAC_PI_2, std::f32::consts::PI];

/// 沿一个扇区采样星的外轮廓，返回 (相对尖角的偏角, 半径)。形状与 gemini_star 完全一致。
fn star_outline(r: f32) -> Vec<(f32, f32)> {
    let waist = 0.22f32;
    let per = 144usize;
    let a0 = -FRAC_PI_2;
    let a1 = 0.0f32;
    let mid = (a0 + a1) / 2.0;
    let (cx, cy) = (r * waist * mid.cos(), r * waist * mid.sin());
    let (p0x, p0y) = (r * a0.cos(), r * a0.sin());
    let (p1x, p1y) = (r * a1.cos(), r * a1.sin());
    let mut out = Vec::with_capacity(per);
    for step in 0..per {
        let u = step as f32 / per as f32;
        let v = 1.0 - u;
        let px = v * v * p0x + 2.0 * v * u * cx + u * u * p1x;
        let py = v * v * p0y + 2.0 * v * u * cy + u * u * p1y;
        out.push((py.atan2(px) - a0, (px * px + py * py).sqrt()));
    }
    out
}

/// 半径 ring_r 这一圈被星身盖住的角度半宽：0 表示星没碰到这一圈。
pub fn star_cover_half_angle(star_r: f32, ring_r: f32) -> f32 {
    if ring_r >= star_r || star_r <= 0.0 {
        return 0.0;
    }
    for (off, rad) in star_outline(star_r) {
        if rad <= ring_r {
            return off;
        }
    }
    0.0
}

/// 把一段弧按缺口切成若干段：缺口里什么都不画，底色自然透出来。
fn arc_segments(a0: f32, a1: f32, cuts: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if cuts.is_empty() || a1 <= a0 {
        return vec![(a0, a1)];
    }
    let mut pieces = vec![(a0, a1)];
    for (center, half) in cuts {
        if *half <= 0.0 {
            continue;
        }
        let mut next: Vec<(f32, f32)> = Vec::new();
        for (s, e) in pieces {
            let mut hit: Option<(f32, f32)> = None;
            for shift in [0.0f32, TAU, -TAU] {
                let cs = center + shift - half;
                let ce = center + shift + half;
                if ce > s && cs < e {
                    hit = Some((cs.max(s), ce.min(e)));
                    break;
                }
            }
            match hit {
                None => next.push((s, e)),
                Some((cs, ce)) => {
                    if cs > s + 1e-4 {
                        next.push((s, cs));
                    }
                    if ce < e - 1e-4 {
                        next.push((ce, e));
                    }
                }
            }
        }
        pieces = next;
    }
    pieces
}
/// 角度 → 星的颜色：在相邻两尖之间按角度插值，尖上正好是那一尖的纯色。
fn star_color(angle: f32) -> Color32 {
    let mut best = (f32::MAX, TIPS[0].1, TIPS[0].1, 0.0f32);
    for i in 0..TIPS.len() {
        let (a0, c0) = TIPS[i];
        let (a1, c1) = TIPS[(i + 1) % TIPS.len()];
        // 让 a0 < a1，并保证角度落在这段扇区里：四段覆盖 360°。
        let (a1, c1) = if a1 <= a0 { (a1 + TAU, c1) } else { (a1, c1) };
        for shift in [0.0f32, TAU, -TAU] {
            let a = angle + shift;
            if a >= a0 - 1e-4 && a <= a1 + 1e-4 {
                let t = ((a - a0) / (a1 - a0)).clamp(0.0, 1.0);
                let d = (a - (a0 + a1) / 2.0).abs();
                if d < best.0 {
                    best = (d, c0, c1, t);
                }
            }
        }
    }
    lerp_rgb(best.1, best.2, best.3)
}

/// 官方 Gemini 四角星：四尖朝上下左右、边向内凹，颜色按角度取彩虹。
pub fn gemini_star(c: &Card, x: f32, y: f32, r: f32, micro: bool) {
    // 官方那颗四角星：四个尖角朝上下左右，边向内收成凹弧，颜色按角度转。
    // 用径向曲线 r(θ) 采样——尖角处半径最大、45° 处收到 waist，形状干净也好羽化。
    let n = 288usize;
    let waist = 0.22f32;
    let tip_at: [f32; 4] = [-FRAC_PI_2, 0.0, FRAC_PI_2, std::f32::consts::PI];
    let average = {
        let mut sum = [0u32; 3];
        for (_, rgb) in TIPS { for (slot, v) in sum.iter_mut().zip(rgb) { *slot += v as u32; } }
        let sink = [
            (sum[0] / TIPS.len() as u32) as u8,
            (sum[1] / TIPS.len() as u32) as u8,
            (sum[2] / TIPS.len() as u32) as u8,
        ];
        lerp_rgb(sink, sink, 0.0)
    };
    let centre = c.pos(x, y);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(centre, average);
    // 每段用二次贝塞尔从一尖连到下一尖，控制点压在 r*waist 的 45° 方向：
    // 边自然内凹、尖角保持锐利，正是官方那颗星的形状。
    let per = n / 4;
    let mut ring: Vec<egui::Pos2> = Vec::with_capacity(n);
    let mut outline: Vec<(f32, f32)> = Vec::with_capacity(n);
    for q in 0..4usize {
        let a0 = tip_at[q];
        let a1 = if q == 3 { tip_at[0] + TAU } else { tip_at[q + 1] };
        let mid = (a0 + a1) / 2.0;
        let ctrl = egui::pos2(x + r * waist * mid.cos(), y + r * waist * mid.sin());
        let p0 = egui::pos2(x + r * a0.cos(), y + r * a0.sin());
        let p1 = egui::pos2(x + r * a1.cos(), y + r * a1.sin());
        for step in 0..per {
            let u = step as f32 / per as f32;
            let v = 1.0 - u;
            let px = v * v * p0.x + 2.0 * v * u * ctrl.x + u * u * p1.x;
            let py = v * v * p0.y + 2.0 * v * u * ctrl.y + u * u * p1.y;
            let pos = c.pos(px, py);
            mesh.colored_vertex(pos, star_color((py - y).atan2(px - x)));
            ring.push(pos);
            outline.push((px, py));
        }
    }
    for k in 0..n as u32 {
        mesh.add_triangle(0, 1 + k, 1 + (k + 1) % n as u32);
    }
    // 边缘描一圈淡白：27 像素上四个尖全靠这道边才立得住，不然会糊成菱形。
    let mut rim = outline.clone();
    if let Some(first) = outline.first() {
        rim.push(*first);
    }
    c.path(&rim, if micro { 0.4 } else { 0.9 }, Color32::from_white_alpha(if micro { 30 } else { 52 }));
    // 中心提一点亮：釉面星在暗底上要有反光才不会看成一块色斑。
    let core = if micro { r * 0.52 } else { r * 0.58 };
    c.glow(x, y, core, Color32::from_white_alpha(if micro { 16 } else { 22 }));
    // 边缘羽化：向外再铺一圈透明顶点，尖角在小尺寸上就不会露硬边。
    let feather = if r >= 9.0 { 0.7 } else { 0.0 };
    if feather > 0.0 {
        let base = mesh.vertices.len() as u32;
        for pos in &ring {
            let dx = pos.x - centre.x;
            let dy = pos.y - centre.y;
            let len = (dx * dx + dy * dy).sqrt().max(0.01);
            let k = (len + feather) / len;
            mesh.colored_vertex(egui::pos2(centre.x + dx * k, centre.y + dy * k), Color32::TRANSPARENT);
        }
        for k in 0..n as u32 {
            let (a0, a1) = (1 + k, 1 + (k + 1) % n as u32);
            mesh.add_triangle(a0, a1, base + k);
            mesh.add_triangle(a1, base + k, base + (k + 1) % n as u32);
        }
    }
    c.painter.add(Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_gap_grows_as_the_ring_gets_closer_to_the_centre() {
        let star = 9.6f32;
        assert_eq!(star_cover_half_angle(star, 12.65), 0.0, "外圈在星尖之外，不该有缺口");
        let mid = star_cover_half_angle(star, 10.15 - 0.85 - 0.35);
        let inner = star_cover_half_angle(star, 7.65 - 0.85 - 0.35);
        assert!(mid > 0.0 && inner > mid, "越靠内的环，被星吃掉的角度越大：{mid} / {inner}");
        assert!(inner < std::f32::consts::FRAC_PI_2, "单个缺口不应超过 90°：{inner}");
    }

    #[test]
    fn arc_segments_leave_the_gap_empty() {
        let a0 = -FRAC_PI_2;
        let a1 = a0 + TAU * 0.75;
        let half = 0.20f32;
        let pieces = arc_segments(a0, a1, &[(a0, half)]);
        let drawn: f32 = pieces.iter().map(|(s, e)| e - s).sum();
        assert!((drawn - (a1 - a0 - half)).abs() < 1e-3, "缺口要整段跳过：{drawn}");
        assert!(pieces.iter().all(|(s, e)| e > s), "不能留下空段");
    }

    #[test]
    fn quota_colors_hit_the_spec_stops() {
        assert_eq!(quota_color(100.0), Color32::from_rgb(0x00, 0xE5, 0xA0));
        assert_eq!(quota_color(50.0), Color32::from_rgb(0xFF, 0xD1, 0x66));
        assert_eq!(quota_color(20.0), Color32::from_rgb(0xFF, 0x4D, 0x3D));
        assert_eq!(quota_color(0.0), Color32::from_rgb(0xE8, 0x32, 0x1E));
        assert_eq!(quota_color(75.0), Color32::from_rgb(0xA8, 0xE0, 0x5A));
    }

    #[test]
    fn quota_color_goes_green_to_red_without_jumping() {
        let mut last = quota_color(100.0);
        for step in 0..=100 {
            let pct = 100.0 - step as f32;
            let color = quota_color(pct);
            // 绿通道一路降；红通道在 100%→20% 一路升，20% 以下按规格转深红，允许变暗。
            assert!(color.g() <= last.g() + 1, "绿通道在 {pct}% 反而变亮了");
            if pct >= 20.0 {
                assert!(color.r() >= last.r().saturating_sub(1), "红通道在 {pct}% 反而变暗了");
            }
            last = color;
        }
        assert!(quota_color(100.0).g() > 200);
        assert!(quota_color(0.0).r() > 200);
    }

    #[test]
    fn countdown_uses_clock_under_a_day_and_words_above() {
        let now = 1_000_000;
        assert_eq!(countdown_text(Some(now + 15_153), now).as_deref(), Some("04:12:33"));
        assert_eq!(countdown_text(Some(now + 86_400 * 3 + 3_600 * 10 + 60 * 22), now).as_deref(), Some("3天 10小时 22分钟"));
        assert_eq!(countdown_text(Some(now + 1), now).as_deref(), Some("00:00:01"));
        assert_eq!(countdown_text(Some(now + 86_400), now).as_deref(), Some("1天 0小时 0分钟"));
    }

    #[test]
    fn countdown_handles_missing_and_past_resets() {
        assert_eq!(countdown_text(None, 1_000_000), None);
        assert_eq!(countdown_text(Some(999_000), 1_000_000).as_deref(), Some("已重置"));
    }
}
