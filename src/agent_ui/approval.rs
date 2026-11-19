//! 审批详情：工具 / 操作内容 / 申请理由 + 固定底部按钮。
//!
//! 布局原则：头部与状态行固定，正文放进带滚动条的裁剪视口，按钮行固定在底部——
//! 内容再长也不会把按钮挤出去，也不会因为「字符数估算」把命令截断。

use super::metrics::{self as m, galley, inset, M, BODY_TOP_GAP, BTN_H, CONTENT_W, FOOT_PAD, HEAD_H, LABEL, PAD};
use super::widgets::{self, HeadLayout, Status, StatusLayout, Viewport};
use super::{Action, ApprovalView};
use crate::paint::Card;
use crate::theme::{self, W};
use eframe::egui::{self, Color32};
use std::sync::Arc;

struct Block {
    label: Option<Arc<egui::Galley>>,
    body: Arc<egui::Galley>,
    /// 正文是否画在代码块里（等宽 + 内嵌底色）。
    code: bool,
    body_h: f32,
    block_h: f32,
}

pub(crate) struct Layout {
    pub w: f32,
    pub h: f32,
    head: HeadLayout,
    status: Status,
    status_g: StatusLayout,
    tool: Option<Arc<egui::Galley>>,
    blocks: Vec<Block>,
    body_top: f32,
    body_content: f32,
    body_view: f32,
    error: Option<String>,
    error_h: f32,
    footer: Vec<(&'static str, &'static str, bool)>,
}

fn label_galley(m: &M, text: &str) -> Arc<egui::Galley> {
    m.line(text, LABEL, W::Regular)
}

pub(crate) fn measure(m: &M, v: &ApprovalView, width: f32) -> Layout {
    let w = super::card_width(width);
    let content = w - PAD * 2.0;
    let head = widgets::measure_head(m, &v.head, w);
    let status = Status {
        tone: if v.native_scope { theme::AGENT_WORK } else { theme::AGENT_ALERT },
        text: if v.native_scope {
            format!("{} · 授权请求", v.head.provider.short())
        } else {
            format!("{} · 权限请求", v.head.provider.short())
        },
        right: if v.queue > 1 { format!("1 / {} 待处理", v.queue) } else { String::new() },
        extra: match v.handoff {
            Some(0) => "已交回客户端".to_owned(),
            Some(s) => format!("{s} 秒后回客户端"),
            None => String::new(),
        },
    };
    let status_g = widgets::measure_status(m, &status, w);

    let tool = if !v.native_scope && !v.tool.trim().is_empty() {
        Some(m.fit(v.tool.trim(), 11.5, W::Semibold, content - 16.0))
    } else {
        None
    };

    let mut blocks = Vec::new();
    if v.native_scope {
        let body = m.text(
            "该授权的有效范围由客户端决定。岛仔只能展示、拒绝或交回，请在客户端核对后再授权。",
            m::BODY,
            W::Regular,
            content,
        );
        let body_h = m.height(&body);
        blocks.push(Block { label: None, body, code: false, body_h, block_h: body_h });
    } else {
        if v.operation.trim().is_empty() {
            let body = m.text("客户端未提供操作详情，请回客户端确认后再授权。", m::BODY, W::Regular, content);
            let body_h = m.height(&body);
            blocks.push(Block { label: Some(label_galley(m, "操作内容")), body, code: false, body_h, block_h: body_h });
        } else {
            let body = m.mono(v.operation, m::CODE, W::Regular, content - 16.0);
            let body_h = m.height(&body);
            let block_h = body_h + 16.0;
            blocks.push(Block { label: Some(label_galley(m, "操作内容")), body, code: true, body_h, block_h });
        }
        if !v.reason.trim().is_empty() {
            let body = m.text(v.reason.trim(), m::BODY, W::Regular, content);
            let body_h = m.height(&body);
            blocks.push(Block { label: Some(label_galley(m, "申请理由")), body, code: false, body_h, block_h: body_h });
        }
    }

    let mut body_content = 0.0;
    if let Some(tool) = &tool {
        body_content += m.height(tool) + 10.0;
    }
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            body_content += 12.0;
        }
        if let Some(label) = &block.label {
            body_content += m.height(label) + 4.0;
        }
        body_content += block.block_h;
    }

    let error = v.error.map(str::to_owned);
    let error_h = widgets::error_height(m, error.as_deref(), w);
    let body_top = m::HEAD_TOP + HEAD_H + m::STATUS_H + BODY_TOP_GAP;
    // 固定区域：头部、状态行、错误行、按钮行与底部留白。
    let fixed = body_top + error_h + 12.0 + BTN_H + FOOT_PAD;
    let body_view = body_content.min((super::MAX_DETAIL_H - fixed).max(48.0));
    let h = fixed + body_view;

    let footer = if v.native_scope {
        vec![("agent-return", "回客户端", true), ("agent-deny", "拒绝", false)]
    } else {
        vec![
            ("agent-return", "回客户端", false),
            ("agent-deny", "拒绝", false),
            ("agent-allow", "仅允许这一次", true),
        ]
    };

    Layout { w, h, head, status, status_g, tool, blocks, body_top, body_content, body_view, error, error_h, footer }
}

pub(crate) fn measure_approval(m: &M, v: &ApprovalView, width: f32) -> (f32, f32) {
    let layout = measure(m, v, width);
    (layout.w, layout.h)
}

pub(crate) fn draw_approval(c: &Card, m: &M, v: &ApprovalView, scroll: f32, t: f64) -> (Option<Action>, f32) {
    let layout = measure(m, v, super::CARD_W);
    let w = layout.w;
    let (alpha, dy) = super::entrance(v.age);
    let c = c.faded(alpha).shifted(0.0, dy);
    let mut action = None;
    if widgets::draw_head(&c, &layout.head, &v.head, w) {
        action = Some(Action::Jump);
    }
    let status_y = m::HEAD_TOP + HEAD_H;
    widgets::draw_status(&c, &layout.status, &layout.status_g, status_y, w);

    // 正文视口：只在这里裁剪与滚动。
    let viewport = Viewport {
        top: layout.body_top,
        height: layout.body_view,
        content: layout.body_content,
        offset: scroll,
    };
    let scroll = viewport.wheel(&c, scroll, w);
    let body = c.clipped(PAD - 6.0, layout.body_top, w - PAD * 2.0 + 12.0, layout.body_view).shifted(0.0, -scroll);
    let mut y = layout.body_top;
    if let Some(tool) = &layout.tool {
        // 工具名前的警示三角用矢量画，避免 ⚠ 在微软雅黑里退化成方框。
        let cy = y + c.galley_h(tool) / 2.0;
        body.poly(&[(PAD + 5.0, cy - 5.0), (PAD + 10.0, cy + 4.5), (PAD, cy + 4.5)], theme::AGENT_ALERT);
        body.fill(PAD + 4.4, cy - 1.4, 1.2, 3.2, 0.6, Color32::BLACK);
        body.circle(PAD + 5.0, cy + 3.0, 0.7, Color32::BLACK);
        galley(&body, PAD + 16.0, y, tool, theme::AGENT_ALERT);
        y += c.galley_h(tool) + 10.0;
    }
    for (i, block) in layout.blocks.iter().enumerate() {
        if i > 0 {
            y += 12.0;
        }
        if let Some(label) = &block.label {
            galley(&body, PAD, y, label, theme::AGENT_SUBTLE);
            y += c.galley_h(label) + 4.0;
        }
        if block.code {
            let bh = block.block_h.max(block.body_h + 16.0);
            inset(&body, PAD, y, CONTENT_W, bh, theme::AGENT_INSET, theme::AGENT_INSET_EDGE);
            galley(&body, PAD + 8.0, y + 8.0, &block.body, Color32::from_rgb(224, 217, 207));
            y += bh;
        } else {
            galley(&body, PAD, y, &block.body, theme::AGENT_MUTED);
            y += block.body_h;
        }
    }
    viewport.bar(&c, w, 1.0);

    let mut footer_y = layout.body_top + layout.body_view + 12.0;
    if layout.error_h > 0.0 {
        let eh = widgets::draw_error(&c, m, footer_y, layout.error.as_deref(), w);
        footer_y += eh + 6.0;
    }
    let busy = v.state.busy();
    let allow_ready = !v.operation.trim().is_empty() && !v.tool.trim().is_empty();
    if let Some(index) = widgets::draw_footer_with_enabled(&c, footer_y, w, &layout.footer, !busy, |id| {
        id != "agent-allow" || allow_ready
    }) {
        action = Some(match layout.footer[index].0 {
            "agent-allow" => Action::Allow,
            "agent-deny" => Action::Deny,
            _ => Action::Return,
        });
    }
    let _ = t;
    (action, scroll)
}
