//! 收起状态的请求卡：用户从详情收起后仍在岛上保留一条待处理提示。
//!
//! 与详情共用设计语言：来源图标 + 标题 + 状态行 + 两枚按钮；不再出现「信息一行、
//! 按钮贴底、中间大片空白」的旧观感。

use super::metrics::{self as m, galley, M, BTN_H, META, PAD};
use super::widgets;
use super::{Action, Head};
use crate::paint::Card;
use crate::theme::{self, W};
use std::sync::Arc;

pub(crate) struct BannerView<'a> {
    pub head: Head<'a>,
    pub summary: &'a str,
    /// 自动交回剩余秒数。
    pub handoff: Option<u8>,
    pub queue: usize,
    /// 是否审批（决定第二个按钮文案）。
    pub approval: bool,
    pub age: f32,
}

pub(crate) struct Layout {
    pub w: f32,
    pub h: f32,
    head: widgets::HeadLayout,
    summary: Arc<eframe::egui::Galley>,
    handoff: Arc<eframe::egui::Galley>,
    queue: Arc<eframe::egui::Galley>,
}

pub(crate) fn measure(m: &M, v: &BannerView, width: f32) -> Layout {
    let w = super::card_width(width);
    // 收起态提示卡没有可收起的详情，不显示收起图标。
    let head = widgets::measure_head_with(m, &v.head, w, false);
    let summary = m.fit(v.summary, m::BODY, W::Regular, w - PAD * 2.0 - 96.0);
    let handoff = m.line_mono(
        &match v.handoff {
            Some(0) => "已交回客户端".to_owned(),
            Some(s) => format!("{s} 秒后交回"),
            None => String::new(),
        },
        META,
        W::Regular,
    );
    let queue = m.line_mono(
        &if v.queue > 1 { format!("还有 {} 条", v.queue - 1) } else { String::new() },
        META,
        W::Regular,
    );
    Layout { w, h: 130.0, head, summary, handoff, queue }
}

pub(crate) fn measure_banner(m: &M, v: &BannerView, width: f32) -> (f32, f32) {
    let layout = measure(m, v, width);
    (layout.w, layout.h)
}

pub(crate) fn draw_banner(c: &Card, m: &M, v: &BannerView, t: f64) -> Option<Action> {
    let layout = measure(m, v, super::CARD_W);
    let w = layout.w;
    let (alpha, dy) = super::entrance(v.age);
    let c = c.faded(alpha).shifted(0.0, dy);
    let mut action = None;
    if widgets::draw_head(&c, &layout.head, &v.head, w) == widgets::HeadHit::Jump {
        action = Some(Action::Jump);
    }

    // 摘要左（带一个语义标记），右侧依次是队列与交回倒计时；下面才是按钮行。
    let summary_w = c.galley_w(&layout.summary);
    let handoff_w = c.galley_w(&layout.handoff);
    let queue_w = c.galley_w(&layout.queue);
    let mut right = w - PAD;
    if !layout.handoff.text().is_empty() {
        right -= handoff_w;
        galley(&c, right, 59.0, &layout.handoff, theme::AGENT_ALERT);
        right -= 12.0;
    }
    if !layout.queue.text().is_empty() {
        right -= queue_w;
        galley(&c, right, 59.0, &layout.queue, theme::AGENT_SUBTLE);
    }
    if v.approval {
        crate::agent_ui::widgets::warn_marker(&c, PAD + 4.0, 58.0 + c.galley_h(&layout.summary) / 2.0);
    } else {
        c.circle(PAD + 4.0, 58.0 + c.galley_h(&layout.summary) / 2.0, 2.2, theme::AGENT_ALERT);
    }
    let _ = summary_w;
    galley(&c, PAD + 16.0, 58.0, &layout.summary, theme::AGENT_MUTED);
    let fy = layout.h - 14.0 - BTN_H;
    let specs = [
        ("agent-return", "回客户端", false),
        ("agent-review", if v.approval { "查看并审批" } else { "查看并回答" }, true),
    ];
    if let Some(i) = widgets::draw_footer(&c, fy, w, &specs, true) {
        action = Some(if i == 0 { Action::Return } else { Action::Review });
    }
    let _ = t;
    action
}
