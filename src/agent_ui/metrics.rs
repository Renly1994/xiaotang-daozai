//! 排版度量：所有尺寸 token 与真实文本测量都集中在这里。
//!
//! 关键的工程约束：`measure_*`（决定卡片/画布高度）与 `draw_*`（真正绘制）必须用
//! **同一次排版结果**。这里用 egui 的 LayoutJob（带 `line_height`）排版并返回 Galley，
//! 高度取 `galley.size().y`，不再按字符数估算。

use crate::paint::Card;
use crate::theme::{self, W};
use eframe::egui::{self, Color32, FontId, Galley, Pos2, Stroke};
use std::sync::Arc;

// ---------- 尺寸 token（对齐 island-hud，按 419 宽面板等比收敛） ----------
/// 卡片左右内边距。
pub(crate) const PAD: f32 = 18.0;
/// 头部顶端留白。
pub(crate) const HEAD_TOP: f32 = 13.0;
/// 来源图标直径。
pub(crate) const ICON: f32 = 24.0;
/// 标题字号 / 副标题字号 / 状态行字号。
pub(crate) const TITLE: f32 = 13.0;
pub(crate) const SUBTITLE: f32 = 10.5;
pub(crate) const STATUS: f32 = 10.0;
pub(crate) const LABEL: f32 = 10.0;
pub(crate) const BODY: f32 = 11.5;
pub(crate) const CODE: f32 = 10.0;
pub(crate) const META: f32 = 9.5;
/// 行高倍数：正文 1.6、元信息 1.5。
pub(crate) const LINE_BODY: f32 = 1.55;
pub(crate) const LINE_CODE: f32 = 1.55;
/// 按钮高度与按钮间距。
pub(crate) const BTN_H: f32 = 30.0;
pub(crate) const BTN_GAP: f32 = 8.0;
/// 卡片底部留白。
pub(crate) const FOOT_PAD: f32 = 16.0;
/// 区块间距。
pub(crate) const GAP: f32 = 10.0;
/// 头部行高（图标 + 两行文字）。
pub(crate) const HEAD_H: f32 = 42.0;
/// 状态行高度。
pub(crate) const STATUS_H: f32 = 18.0;
/// 正文与头部/状态行之间的间距。
pub(crate) const BODY_TOP_GAP: f32 = 8.0;

/// 卡片宽度上限（与展开面板 PW 对齐）。
pub(crate) const CARD_W: f32 = 419.0;
/// 卡片内容宽度。
pub(crate) const CONTENT_W: f32 = CARD_W - PAD * 2.0;
/// 卡片基础控件高度（供 mod.rs 复用）。
pub(crate) const BAR_H: f32 = 30.0;

/// 代码（行内与代码块）共用的字色：比正文亮一档，和等宽字体一起把代码从散文里分出来。
pub(crate) const CODE_INK: Color32 = Color32::from_rgb(224, 217, 207);

/// 行内片段的显示样式（受限 markdown 的可见部分：正文 / 行内代码 / 粗体 / 链接）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Span {
    /// 普通正文：颜色由绘制时的兜底色决定，跟随段落类型（正文/引用/标题）。
    #[default]
    Text,
    /// 行内 `code`：等宽字体 + 亮一档的字色。
    Code,
    /// `**粗体**`。
    Strong,
    /// `[文字](链接)`：文字用可点击色，链接本身不加载、不执行。
    Link,
}

/// 无窗口上下文时的测量器。
#[derive(Clone, Copy)]
pub(crate) struct M<'a> {
    pub ctx: &'a egui::Context,
}

impl<'a> M<'a> {
    pub fn new(ctx: &'a egui::Context) -> Self {
        Self { ctx }
    }

    /// 按给定字体与行高排版一段**已经做过 RTL 显示转换**的文字。
    fn job(&self, shown: &str, font: FontId, color: Color32, width: f32, line_height: Option<f32>, break_anywhere: bool) -> Arc<Galley> {
        let mut job = egui::text::LayoutJob::simple(shown.to_owned(), font, color, width.max(1.0));
        job.wrap.break_anywhere = break_anywhere;
        if let Some(h) = line_height {
            for section in &mut job.sections {
                section.format.line_height = Some(h);
            }
        }
        self.ctx.fonts(|f| f.layout_job(job))
    }

    fn layout(&self, text: &str, font: FontId, color: Color32, width: f32, line_height: Option<f32>, break_anywhere: bool) -> Arc<Galley> {
        let shown = crate::rtl_text::display(text);
        self.job(&shown, font, color, width, line_height, break_anywhere)
    }

    /// 正文/标题排版：按宽度换行，行高按 `size * LINE_BODY` 拉松。
    pub fn text(&self, text: &str, size: f32, weight: W, width: f32) -> Arc<Galley> {
        self.layout(text, theme::font(size, weight), theme::WHITE, width, Some(size * LINE_BODY), true)
    }

    /// 与 `text` 同一套度量，但调用方已经做过 RTL 显示转换（避免转换两次）。
    pub fn text_shown(&self, shown: &str, size: f32, weight: W, width: f32) -> Arc<Galley> {
        self.job(shown, theme::font(size, weight), theme::WHITE, width, Some(size * LINE_BODY), true)
    }

    /// 行内分段排版（`code` / **粗体** / 链接）：与 `text` 同行高、同断行策略，
    /// 保证 measure 与 draw 用同一份结果。传入的文字必须已经做过 RTL 显示转换。
    pub fn spans(&self, spans: &[(Span, String)], size: f32, weight: W, width: f32) -> Arc<Galley> {
        let line_height = size * LINE_BODY;
        let mut job = egui::text::LayoutJob::default();
        job.wrap.break_anywhere = true;
        job.wrap.max_width = width.max(1.0);
        for (style, text) in spans {
            if text.is_empty() {
                continue;
            }
            let (font, color) = match style {
                Span::Text => (theme::font(size, weight), theme::WHITE),
                Span::Strong => (theme::font(size, W::Semibold), theme::WHITE),
                // 行内代码略小一点：等宽字体的字面更宽，同字号会显得比正文重。
                Span::Code => (theme::mono(size * 0.94, W::Regular), CODE_INK),
                Span::Link => (theme::font(size, weight), theme::AGENT_WORK),
            };
            job.append(text, 0.0, egui::text::TextFormat { font_id: font, color, line_height: Some(line_height), ..Default::default() });
        }
        if job.sections.is_empty() {
            return self.text_shown("", size, weight, width);
        }
        self.ctx.fonts(|f| f.layout_job(job))
    }

    /// 等宽/代码排版。
    pub fn mono(&self, text: &str, size: f32, weight: W, width: f32) -> Arc<Galley> {
        // 代码/命令按字符断行（等价 CSS 的 overflow-wrap:anywhere）：超长单行命令不会被裁掉。
        self.layout(text, theme::mono(size, weight), theme::WHITE, width, Some(size * LINE_CODE), true)
    }

    /// 单行（不换行）排版，用于标题、倒计时等需要 fit 的地方。
    pub fn line(&self, text: &str, size: f32, weight: W) -> Arc<Galley> {
        self.layout(text, theme::font(size, weight), theme::WHITE, f32::INFINITY, None, false)
    }

    /// 单行等宽排版。
    pub fn line_mono(&self, text: &str, size: f32, weight: W) -> Arc<Galley> {
        self.layout(text, theme::mono(size, weight), theme::WHITE, f32::INFINITY, None, false)
    }

    pub fn height(&self, g: &Galley) -> f32 {
        g.size().y
    }
    pub fn width(&self, g: &Galley) -> f32 {
        g.size().x
    }
    /// 按最大宽度截断成单行，超出加省略号。
    pub fn fit(&self, text: &str, size: f32, weight: W, max_w: f32) -> Arc<Galley> {
        let full = self.line(text, size, weight);
        if full.size().x <= max_w {
            return full;
        }
        let chars: Vec<char> = crate::rtl_text::display(text).chars().collect();
        let mut low = 0usize;
        let mut high = chars.len().saturating_sub(1);
        while low < high {
            let n = low + (high - low + 1) / 2;
            let candidate: String = chars[..n].iter().collect::<String>() + "…";
            if self.line(&candidate, size, weight).size().x <= max_w {
                low = n;
            } else {
                high = n - 1;
            }
        }
        self.line(&(chars[..low].iter().collect::<String>() + "…"), size, weight)
    }
}

/// 在卡片画布上绘制一份 galley（`top` 为行盒顶部）。
pub(crate) fn galley(c: &Card, x: f32, top: f32, g: &Arc<Galley>, color: Color32) {
    c.painter.galley(c.pos(x, top), g.clone(), color);
}

/// 圆角内嵌块（代码块/选项/输入框）的底与描边。
pub(crate) fn inset(c: &Card, x: f32, y: f32, w: f32, h: f32, fill: Color32, edge: Color32) {
    c.fill(x, y, w, h, 9.0, fill);
    c.stroke(x, y, w, h, 9.0, 1.0, edge);
}

/// 一条 1pt 分隔线（只在需要的地方用，卡片头部与正文之间不再画）。
pub(crate) fn hairline(c: &Card, x: f32, y: f32, w: f32, alpha: f32) {
    c.seg((x, y), (x + w, y), 1.0, Color32::from_white_alpha((255.0 * alpha) as u8));
}

/// 细滚动条：`ratio` 是可视比例（0..1），`offset` 是 0..1 的滚动进度。
pub(crate) fn scrollbar(c: &Card, x: f32, top: f32, view_h: f32, ratio: f32, offset: f32, alpha: f32) {
    if ratio >= 0.999 {
        return;
    }
    let track = view_h.max(8.0);
    let thumb = (track * ratio).clamp(18.0, track);
    let travel = (track - thumb).max(0.0);
    let y = top + travel * offset.clamp(0.0, 1.0);
    c.fill(x, top, 2.0, track, 1.0, Color32::from_white_alpha((8.0 * alpha) as u8));
    c.fill(x, y, 2.0, thumb, 1.0, Color32::from_white_alpha((56.0 * alpha) as u8));
}

/// 圆角矩形按钮：主按钮反白、次按钮深灰、危险操作与次按钮同色（不加红）。
pub(crate) fn button(
    c: &Card,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    id: &str,
    label: &str,
    primary: bool,
    enabled: bool,
) -> bool {
    // UI Map（调试，`DAOZAI_UI_MAP`）：所有卡片的 footer 按钮都从这里画，
    // 在这里登记能保证「地图上的矩形」与「点下去真正命中的矩形」完全一致。
    if crate::ui_map::enabled() {
        crate::ui_map::ensure(c.ui.ctx());
        if let Some(node_id) = crate::ui_map::footer_node_id(id) {
            crate::ui_map::node(node_id, c.rect(x, y, w, h), label, enabled);
        }
    }
    let r = c.hit(x, y, w, h, id);
    let (hover, press) = c.hover_t(&r);
    let base = if primary { theme::AGENT_PRIMARY } else { theme::AGENT_SECONDARY };
    let hovered = if primary { theme::AGENT_PRIMARY_HOVER } else { theme::AGENT_SECONDARY_HOVER };
    // 禁用态不用整体乘 alpha：浅色主按钮乘完会变成「灰底灰字」，几乎看不清。
    // 统一改成深灰底 + 中等亮度文字，既弱化又保持可读（参考项目的 opacity .42 在深色岛体上同理）。
    let (fill, ink) = if enabled {
        (crate::paint::mix(base, hovered, hover), if primary { Color32::from_rgb(8, 8, 8) } else { Color32::from_rgb(238, 238, 238) })
    } else {
        (Color32::from_rgb(44, 44, 47), Color32::from_rgb(130, 130, 136))
    };
    let scale = if enabled { 1.0 - 0.03 * press } else { 1.0 };
    let inner = c.scaled(x + w / 2.0, y + h / 2.0, scale);
    inner.fill(x, y, w, h, 8.0, fill);
    inner.centered_text(x + w / 2.0, y + h / 2.0, label, 11.0, W::Semibold, ink, crate::paint::H::Center);
    enabled && r.clicked()
}

/// 顶部小胶囊（跳转/收起一类的轻操作）。
pub(crate) fn chip(c: &Card, x: f32, y: f32, w: f32, h: f32, id: &str, label: &str, accent: Color32) -> bool {
    // UI Map（调试）：跳转 chip；完成卡轻提示上的「查看 / 关闭」也走这里。
    if crate::ui_map::enabled() {
        crate::ui_map::ensure(c.ui.ctx());
        let node_id = match id {
            "agent-jump-chip" => Some("chip.jump"),
            other => crate::ui_map::footer_node_id(other),
        };
        if let Some(node_id) = node_id {
            crate::ui_map::node(node_id, c.rect(x, y, w, h), label, true);
        }
    }
    let r = c.hit(x, y, w, h, id);
    let (hover, press) = c.hover_t(&r);
    // 按下立刻提亮压缩：跳转本身可能没有可见变化（客户端已在前台），至少让用户确认点到了。
    let fill = Color32::from_white_alpha((12.0 + 14.0 * hover + 20.0 * press) as u8);
    let c = c.scaled(x + w / 2.0, y + h / 2.0, 1.0 - 0.02 * press);
    c.fill(x, y, w, h, h / 2.0, fill);
    c.stroke(x, y, w, h, h / 2.0, 1.0, Color32::from_white_alpha((26.0 + 30.0 * press) as u8));
    c.centered_text(x + w / 2.0, y + h / 2.0, label, 9.5, W::Semibold, crate::paint::mix(theme::AGENT_MUTED, accent, hover.max(press)), crate::paint::H::Center);
    r.clicked()
}

/// footer 按钮行的矩形（与 `widgets::draw_footer_with_enabled` 同一套布局）。
/// 供 banner 这种需要给按钮换契约 id 的地方复用。
pub(crate) fn footer_rects(c: &Card, y: f32, w: f32, n: usize) -> Vec<egui::Rect> {
    if n == 0 {
        return Vec::new();
    }
    let total = w - PAD * 2.0;
    let count = n as f32;
    let bw = (total - BTN_GAP * (count - 1.0)) / count;
    (0..n).map(|i| c.rect(PAD + i as f32 * (bw + BTN_GAP), y, bw, BTN_H)).collect()
}

/// UI Map：岛体条（面板顶部那条 44pt 的岛体），矩形取面板裁剪区的上沿。
pub(crate) fn map_island_bar(c: &Card) {
    if !crate::ui_map::enabled() {
        return;
    }
    let clip = c.painter.clip_rect();
    let height = 44.0_f32.min(clip.height());
    crate::ui_map::node("island.bar", egui::Rect::from_min_size(clip.min, egui::vec2(clip.width(), height)), "岛体", true);
}

/// UI Map：头部右上角「收起」图标（widgets::draw_head 画它，但矩形在这里算最省事）。
pub(crate) fn map_collapse_chip(c: &Card, w: f32, collapse: bool) {
    if !collapse {
        return;
    }
    let x = w - PAD - 22.0;
    crate::ui_map::node("chip.collapse", c.rect(x, HEAD_TOP + 5.0, 22.0, 22.0), "收起", true);
}

/// 判断一个屏幕点是否落在卡片内的任一交互元素上（用于悬停暂停交回）。
pub(crate) fn hit_any(c: &Card, rect: egui::Rect) -> bool {
    c.live && c.ui.rect_contains_pointer(rect)
}

/// 绘制勾选。
pub(crate) fn check(c: &Card, x: f32, y: f32, size: f32, color: Color32) {
    c.path(&[(x - size * 0.34, y), (x - size * 0.08, y + size * 0.28), (x + size * 0.36, y - size * 0.30)], 1.7, color);
}

/// 绘制警示圆点。
pub(crate) fn dot(c: &Card, x: f32, y: f32, color: Color32) {
    c.circle(x, y, 2.6, color);
}

/// 呼吸中的等待描边（1.8s 周期）：给待处理卡片一圈很淡的 alert 光。
pub(crate) fn attention(c: &Card, t: f64, x: f32, y: f32, w: f32, h: f32, radius: f32) {
    let phase = ((t / 1.8) * std::f64::consts::TAU).sin() as f32 * 0.5 + 0.5;
    let alpha = 0.28 + 0.34 * phase;
    c.stroke(x, y, w, h, radius, 1.0, crate::paint::alpha(theme::AGENT_ALERT, alpha));
}

/// `Pos2` 便捷构造，供绘制辅助函数使用。
pub(crate) fn pos(x: f32, y: f32) -> Pos2 {
    Pos2::new(x, y)
}

/// 统一的描边。
pub(crate) fn edge(color: Color32, width: f32) -> Stroke {
    Stroke::new(width, color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_wraps_to_the_given_width() {
        let ctx = eframe::egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let _ = ctx.run(eframe::egui::RawInput {
            screen_rect: Some(eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(640.0, 480.0))),
            ..Default::default()
        }, |_| {});
        let m = M::new(&ctx);
        for text in ["中文句子重复很多次。".repeat(20), "hello world ".repeat(40), "a".repeat(400)] {
            let g = m.text(&text, 9.5, W::Regular, 383.0);
            assert!(g.rows.len() > 1, "换行失效：{} 行（{} 字符）", g.rows.len(), text.chars().count());
            assert!(g.size().x <= 384.0, "宽度超标：{}", g.size().x);
        }
    }
}
