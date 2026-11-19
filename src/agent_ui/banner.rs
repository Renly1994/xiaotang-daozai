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
}

pub(crate) fn measure(m: &M, v: &BannerView, width: f32) -> Layout {
    let w = super::card_width(width);
    // 收起态提示卡没有可收起的详情，不显示收起图标。
    let head = widgets::measure_head_with(m, &v.head, w, false);
    let summary = m.fit(v.summary, m::BODY, W::Regular, w - PAD * 2.0 - 60.0);
    let handoff = m.line_mono(
        &match v.handoff {
            Some(0) => "已交回客户端".to_owned(),
            Some(s) => format!("{s} 秒后交回"),
            None => String::new(),
        },
        META,
        W::Regular,
    );
    Layout { w, h: 130.0, head, summary, handoff }
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

    // 摘要左、交回倒计时右，同一行；下面才是按钮行。
    let handoff_w = c.galley_w(&layout.handoff);
    galley(&c, PAD, 58.0, &layout.summary, theme::AGENT_MUTED);
    if !layout.handoff.text().is_empty() {
        galley(&c, w - PAD - handoff_w, 59.0, &layout.handoff, theme::AGENT_ALERT);
    }
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
