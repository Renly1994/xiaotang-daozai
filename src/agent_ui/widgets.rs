//! 卡片共用部件：头部、状态行、底部按钮行、滚动正文区。
//!
//! 这里只放「多张卡都要用」的东西；具体分卡内容在 `approval.rs` / `question.rs` / `done.rs`。

use super::metrics::{
    self as m, button, dot, galley, scrollbar, M, BTN_GAP, BTN_H, CONTENT_W, META, PAD, TITLE,
};
use super::Head;
use crate::paint::Card;
use crate::theme::{self, W};
use eframe::egui::{self, Color32};
use std::sync::Arc;

pub(crate) use super::metrics::{check, chip};

/// 判断一个屏幕点是否落在卡片内的任一交互元素上（用于悬停暂停交回）。
pub(crate) fn hit_any(c: &Card, rect: egui::Rect) -> bool {
    c.live && c.ui.rect_contains_pointer(rect)
}

/// 头部测量结果。
pub(crate) struct HeadLayout {
    pub title: Arc<egui::Galley>,
    pub subtitle: Arc<egui::Galley>,
    pub jump: Option<Arc<egui::Galley>>,
    pub jump_w: f32,
}

pub(crate) fn measure_head(m: &M, head: &Head, w: f32) -> HeadLayout {
    let jump = head.jump.map(|t| m.line(t.action_label(), META, W::Semibold));
    let jump_w = jump.as_ref().map_or(0.0, |g| m.width(g) + 18.0);
    let text_w = w - PAD - 32.0 - if jump_w > 0.0 { jump_w + 10.0 } else { 0.0 };
    // 标题用卡片的语义标题（等待你审批 / 需要你回答），副标题给出「来源 · 目录」。
    let title = m.fit(head.kicker, TITLE, W::Semibold, text_w);
    let subtitle_text = if head.subtitle.trim().is_empty() {
        head.provider.label().to_owned()
    } else {
        format!("{} · {}", head.provider.short(), head.subtitle.trim())
    };
    let subtitle = m.fit(&subtitle_text, m::SUBTITLE, W::Regular, text_w);
    HeadLayout { title, subtitle, jump, jump_w }
}

/// 画头部；返回「点了跳转」。
pub(crate) fn draw_head(c: &Card, l: &HeadLayout, head: &Head, w: f32) -> bool {
    let top = m::HEAD_TOP;
    crate::extras::agent_icon(c, PAD + m::ICON / 2.0, top + m::ICON / 2.0, m::ICON, head.provider);
    galley(c, PAD + 32.0, top + 1.0, &l.title, theme::WHITE);
    let sub_top = top + 1.0 + c.galley_h(&l.title) + 3.0;
    if !head.subtitle.trim().is_empty() {
        galley(c, PAD + 32.0, sub_top, &l.subtitle, theme::AGENT_SUBTLE);
    }
    if let Some(g) = &l.jump {
        let x = w - PAD - l.jump_w;
        if chip(c, x, top + 6.0, l.jump_w, 22.0, "agent-jump-chip", g.text(), theme::AGENT_WORK) {
            return true;
        }
    }
    false
}

/// 头部高度（含图标与两行文字）。
pub(crate) fn head_height() -> f32 {
    m::HEAD_H
}

/// 状态行要展示的内容。
pub(crate) struct Status {
    pub tone: Color32,
    pub text: String,
    pub right: String,
    pub extra: String,
}

pub(crate) struct StatusLayout {
    pub text: Arc<egui::Galley>,
    pub right: Arc<egui::Galley>,
    pub extra: Arc<egui::Galley>,
}

pub(crate) fn measure_status(m: &M, s: &Status, w: f32) -> StatusLayout {
    let extra = m.line_mono(&s.extra, META, W::Regular);
    let right = m.line_mono(&s.right, META, W::Regular);
    let avail = w - PAD * 2.0 - m.width(&extra) - m.width(&right) - 26.0;
    StatusLayout {
        text: m.fit(&s.text, m::STATUS, W::Semibold, avail.max(40.0)),
        right,
        extra,
    }
}

pub(crate) fn draw_status(c: &Card, s: &Status, g: &StatusLayout, y: f32, w: f32) {
    let cy = y + m::STATUS_H / 2.0;
    dot(c, PAD + 3.0, cy, s.tone);
    galley(c, PAD + 11.0, y + 2.0, &g.text, s.tone);
    // 右侧：队列与倒计时从右往左排（等宽字体保证逐帧不跳宽）。
    let mut edge = w - PAD;
    if !s.extra.is_empty() {
        let ew = c.galley_w(&g.extra);
        galley(c, edge - ew, y + 2.0, &g.extra, theme::AGENT_ALERT);
        edge -= ew + 10.0;
    }
    if !s.right.is_empty() {
        let rw = c.galley_w(&g.right);
        galley(c, edge - rw, y + 2.0, &g.right, theme::AGENT_SUBTLE);
    }
}

pub(crate) fn footer_y(body_end: f32) -> f32 {
    body_end + 12.0
}

/// 卡片总高度 = 底部按钮之下再留 `FOOT_PAD`。
pub(crate) fn height_for(footer_y: f32) -> f32 {
    footer_y + BTN_H + m::FOOT_PAD
}

pub(crate) fn draw_footer(c: &Card, y: f32, w: f32, specs: &[(&str, &str, bool)], enabled: bool) -> Option<usize> {
    draw_footer_with_enabled(c, y, w, specs, enabled, |_| true)
}

/// 底部按钮行；`enabled_for` 允许按按钮 id 单独禁用（例如缺少完整操作时不能允许）。
pub(crate) fn draw_footer_with_enabled<F: Fn(&str) -> bool>(
    c: &Card,
    y: f32,
    w: f32,
    specs: &[(&str, &str, bool)],
    any_enabled: bool,
    enabled_for: F,
) -> Option<usize> {
    if specs.is_empty() {
        return None;
    }
    let total = w - PAD * 2.0;
    let n = specs.len() as f32;
    let bw = (total - BTN_GAP * (n - 1.0)) / n;
    for (i, (id, label, primary)) in specs.iter().enumerate() {
        let x = PAD + i as f32 * (bw + BTN_GAP);
        let enabled = any_enabled && enabled_for(id);
        if button(c, x, y, bw, BTN_H, id, label, *primary, enabled) {
            return Some(i);
        }
    }
    None
}

/// 提示行（提交中 / 结果未知 / 跳转失败）的测量与绘制。
pub(crate) fn error_height(m: &M, error: Option<&str>, w: f32) -> f32 {
    match error {
        Some(text) if !text.trim().is_empty() => m.height(&m.text(text, META, W::Regular, w - PAD * 2.0)) + 6.0,
        _ => 0.0,
    }
}

pub(crate) fn draw_error(c: &Card, m: &M, y: f32, error: Option<&str>, w: f32) -> f32 {
    match error {
        Some(text) if !text.trim().is_empty() => {
            let g = m.text(text, META, W::Regular, w - PAD * 2.0);
            let h = c.galley_h(&g);
            galley(c, PAD, y, &g, Color32::from_rgb(255, 176, 144));
            h
        }
        _ => 0.0,
    }
}

/// 正文滚动视口。
pub(crate) struct Viewport {
    pub top: f32,
    pub height: f32,
    pub content: f32,
    pub offset: f32,
}

impl Viewport {
    pub fn ratio(&self) -> f32 {
        if self.content <= 0.0 {
            1.0
        } else {
            (self.height / self.content).clamp(0.0, 1.0)
        }
    }
    pub fn max_offset(&self) -> f32 {
        (self.content - self.height).max(0.0)
    }
    /// 处理滚轮：返回新的偏移。只在指针位于正文区时生效。
    pub fn wheel(&self, c: &Card, offset: f32, w: f32) -> f32 {
        let rect = c.rect(PAD - 6.0, self.top, w - PAD * 2.0 + 12.0, self.height);
        let ctx = c.ui.ctx();
        let delta = ctx.input(|i| i.raw_scroll_delta.y);
        if delta.abs() > 0.01 && ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| rect.contains(p)) {
            return (offset - delta).clamp(0.0, self.max_offset());
        }
        offset.clamp(0.0, self.max_offset())
    }
    pub fn bar(&self, c: &Card, w: f32, alpha: f32) {
        scrollbar(
            c,
            w - PAD + 4.5,
            self.top,
            self.height,
            self.ratio(),
            if self.max_offset() > 0.0 { self.offset / self.max_offset() } else { 0.0 },
            alpha,
        );
    }
}

/// 空内容占位（例如客户端没提供操作详情）。
pub(crate) fn placeholder(c: &Card, m: &M, x: f32, y: f32, text: &str) {
    galley(c, x, y, &m.text(text, m::BODY, W::Regular, CONTENT_W), theme::AGENT_SUBTLE);
}
