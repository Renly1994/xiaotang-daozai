//! 审批详情：工具 / 操作内容 / 申请理由 + 固定底部按钮。
//!
//! 布局原则：头部与状态行固定，正文放进带滚动条的裁剪视口，按钮行固定在底部——
//! 内容再长也不会把按钮挤出去，也不会因为「字符数估算」把命令截断。

use super::metrics::{self as m, galley, inset, M, BODY_TOP_GAP, BTN_H, CONTENT_W, FOOT_PAD, HEAD_H, LABEL, PAD};
use super::widgets::{self, HeadLayout, Status, StatusLayout, Viewport};
use super::{Action, ApprovalView, State};
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
    // 状态行同时承载「阶段」：已发出决定后不能还写着「权限请求」，否则用户不知道点了有没有生效。
    let (tone, text) = match v.state {
        State::Pending if v.native_scope => (theme::AGENT_WORK, format!("{} · 授权请求", v.head.provider.short())),
        State::Pending => (theme::AGENT_ALERT, format!("{} · 权限请求", v.head.provider.short())),
        State::Submitting => (theme::AGENT_WORK, "已提交 · 等待客户端确认".to_owned()),
        State::Uncertain => (theme::AGENT_ALERT, "提交结果未知 · 请回客户端确认".to_owned()),
    };
    let status = Status {
        tone,
        text,
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

    // 提示槽位：优先显示发送失败的原因；没有失败但缺少完整操作时，解释「仅允许这一次」为什么不可点。
    let allow_ready = !v.operation.trim().is_empty() && !v.tool.trim().is_empty();
    let note = v.error.map(str::to_owned).or_else(|| {
        (!v.native_scope && !allow_ready)
            .then(|| "缺操作内容，无法在岛上允许 · 回客户端或拒绝".to_owned())
    });
    let error = note;
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
    match widgets::draw_head(&c, &layout.head, &v.head, w) {
        widgets::HeadHit::Jump => action = Some(Action::Jump),
        widgets::HeadHit::Collapse => action = Some(Action::Collapse),
        widgets::HeadHit::None => {}
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
        widgets::warn_marker(&body, PAD + 5.0, cy);
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
    if busy {
        // 提交中/结果未知：底部给一条不确定进度条，让「点了」这件事有可见反馈。
        widgets::progress(&c, PAD, footer_y + BTN_H + 5.0, w - PAD * 2.0, 2.0, t as f32);
    }
    // 提交中/结果未知时只禁用「会造成第二次决定」的按钮；「回客户端」必须始终可点——
    // 结果未知的提示语就是「请回客户端确认」，按钮却点不动是死路。
    if let Some(index) = widgets::draw_footer_with_enabled(&c, footer_y, w, &layout.footer, true, |id| {
        id == "agent-return" || (!busy && (id != "agent-allow" || allow_ready))
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
