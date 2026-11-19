//! Agent 通知 / 审批 / 提问 / 完成结果的表现层。
//!
//! 这个模块只负责「怎么画」和「画多高」：状态机、队列与协议仍在 `crate::agent_cards`。
//! 设计原则：
//! 1. **一份真值**：先用 `measure_*` 按真实字体排版算出高度，`draw_*` 复用同一份结果；
//!    不再用「字符数 ÷ 每行字数」估算。
//! 2. **自顶向下游标**：所有元素按 `y += 高度 + 间距` 排列，不写死 `height-29.0` 这类坐标。
//! 3. **长内容滚动不截断**：正文放进带细滚动条的裁剪视口，避免「看不见全貌却要授权」。
//! 4. **状态可见**：等待/队列/倒计时/提交中/失败都有对应视觉；危险操作弱化、主操作反白。

mod approval;
mod banner;
mod done;
mod metrics;
mod question;
mod widgets;

pub(crate) use approval::{draw_approval, measure_approval};
pub(crate) use banner::{draw_banner, measure_banner, BannerView};
pub(crate) use done::{draw_done, draw_result, measure_done, measure_result};
pub(crate) use metrics::{M as Measurer, BAR_H, BTN_H, CARD_W, CONTENT_W, PAD};
pub(crate) use question::{draw_question, measure_question, QuestionResult};
pub(crate) use widgets::hit_any;

use crate::model::Settings;
use eframe::egui;
use serde::{Deserialize, Serialize};

/// 卡片当前处在哪个交互阶段。宿主仍以 `cancel` / 回执为最终真值，这里只影响表现。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum State {
    /// 等待用户操作。
    #[default]
    Pending,
    /// 已发出决定，等待宿主收口。
    Submitting,
    /// 发送失败或迟迟没有回执：锁住控件并提示回原客户端确认。
    Uncertain,
}

impl State {
    pub(crate) fn busy(self) -> bool {
        self != Self::Pending
    }
}

/// 卡片上可产生的动作；由 `agent_cards` 翻译成 wire reply。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Action {
    /// 交回原客户端。
    Return,
    /// 拒绝审批。
    Deny,
    /// 仅允许这一次。
    Allow,
    /// 提交提问答案。
    Answer,
    /// 打开原会话/客户端。
    Jump,
    /// 从收起态展开详情。
    Review,
    /// 收起详情（只收起，不交回）。
    Collapse,
    /// 关闭完成通知。
    Dismiss,
    /// 查看本轮回复正文。
    ShowResult,
    /// 复制正文到剪贴板。
    CopyResult,
}

/// 卡片头部用的会话信息。
pub(crate) struct Head<'a> {
    pub provider: crate::agent_notice::Provider,
    pub kicker: &'a str,
    pub subtitle: &'a str,
    pub jump: Option<&'a crate::agent_jump::Target>,
}

/// 审批详情视图（只读字段 + 由上层维护的展示状态）。
pub(crate) struct ApprovalView<'a> {
    pub head: Head<'a>,
    pub tool: &'a str,
    pub reason: &'a str,
    pub cwd: &'a str,
    pub operation: &'a str,
    pub native_scope: bool,
    /// 队列总数（>1 时显示 `1 / N`）。
    pub queue: usize,
    /// 自动交回剩余秒数。
    pub handoff: Option<u8>,
    pub state: State,
    /// 发送失败等需要展示的错误行。
    pub error: Option<&'a str>,
    /// 入场进度 0..1（1 = 完全显示）。
    pub age: f32,
}

/// 提问详情视图。
pub(crate) struct QuestionView<'a> {
    pub head: Head<'a>,
    pub questions: &'a [Question],
    pub answers: &'a mut [Answer],
    pub index: usize,
    pub scroll: f32,
    /// 是否处于「检查答案」页：最后一题确认前先总览一遍作答。
    pub reviewing: bool,
    pub queue: usize,
    pub handoff: Option<u8>,
    pub state: State,
    pub error: Option<&'a str>,
    pub age: f32,
}

/// 完成通知视图。
pub(crate) struct DoneView<'a> {
    pub provider: crate::agent_notice::Provider,
    pub kicker: &'a str,
    pub text: &'a str,
    pub compact: bool,
    pub age: f32,
}

/// 结果正文视图（完成通知的「查看回复」）。
pub(crate) struct ResultView<'a> {
    pub provider: crate::agent_notice::Provider,
    pub kicker: &'a str,
    pub text: &'a str,
    pub scroll: f32,
    pub age: f32,
    /// 已复制到剪贴板的反馈（剩余秒数）。
    pub copied: Option<f32>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Choice {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Question {
    pub id: String,
    pub question: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub multi: bool,
    #[serde(default)]
    pub options: Vec<Choice>,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Answer {
    pub id: String,
    pub selected: Vec<String>,
    pub custom: String,
}

impl Answer {
    pub(crate) fn ready(&self) -> bool {
        !self.selected.is_empty() || !self.custom.trim().is_empty()
    }
}

/// 卡片底宽：与展开面板一致，不再单独留 30pt 暗带。
pub(crate) fn card_width(panel_w: f32) -> f32 {
    panel_w.clamp(300.0, CARD_W)
}

/// 内容区宽度。
pub(crate) fn content_width(card_w: f32) -> f32 {
    (card_w - PAD * 2.0).max(120.0)
}

/// 详情最大高度：画布抬到 520 后允许 500 的卡片。
pub(crate) const MAX_DETAIL_H: f32 = 500.0;

/// 卡片入场：`age` 是「展开后经过的秒数」。返回 (透明度, 上移距离)。
pub(crate) fn entrance(age: f32) -> (f32, f32) {
    // 200ms 淡入 + 上移 60ms 延迟（对齐参考的 content-enter）。
    let t = ((age - 0.06) / 0.20).clamp(0.0, 1.0);
    let e = 1.0 - (1.0 - t).powi(3);
    (e, -6.0 * (1.0 - e))
}

/// 由设置推导「卡片是否展开成完整详情」——只在需要时抬画布。
pub(crate) fn wants_tall_canvas(settings: &Settings) -> bool {
    settings.agent.interactive && settings.dsh_notifications
}

/// 供测试与其他模块复用的颜色快捷方式。
pub(crate) fn alert_alpha(a: f32) -> egui::Color32 {
    crate::theme::AGENT_ALERT.gamma_multiply(a.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let _ = ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, WIN_H))),
            ..Default::default()
        }, |_| {});
        ctx
    }

    use crate::app::WIN_H;

    fn approval<'a>(head: Head<'a>, operation: &'a str, reason: &'a str) -> ApprovalView<'a> {
        ApprovalView {
            head,
            tool: "write",
            reason,
            cwd: "C:\\work\\api-gateway",
            operation,
            native_scope: false,
            queue: 1,
            handoff: None,
            state: State::Pending,
            error: None,
            age: 1.0,
        }
    }

    fn head<'a>() -> Head<'a> {
        Head { provider: crate::agent_notice::Provider::Dsh, kicker: "等待你审批", subtitle: "C:\\work", jump: None }
    }

    #[test]
    fn approval_height_stays_within_the_canvas_for_any_content_length() {
        let ctx = ctx();
        let m = Measurer::new(&ctx);
        for repeat in [0usize, 2, 40, 400] {
            let operation = "x".repeat(repeat);
            let reason = "说明".repeat(repeat);
            let view = approval(head(), &operation, &reason);
            let (w, h) = measure_approval(&m, &view, CARD_W);
            assert_eq!(w, CARD_W, "卡片宽度必须跟随面板，不能被内容撑开");
            assert!(h > 120.0 && h <= WIN_H, "高度必须留在画布内：{h}");
            assert!(h <= MAX_DETAIL_H, "高度不超过详情上限：{h}");
        }
    }

    #[test]
    fn approval_height_grows_with_content_until_the_cap() {
        let ctx = ctx();
        let m = Measurer::new(&ctx);
        let short = measure_approval(&m, &approval(head(), "{}", ""), CARD_W).1;
        let long_operation = "{\"k\":\"v\",}".repeat(120);
        let long = measure_approval(&m, &approval(head(), &long_operation, "为什么要跑"), CARD_W).1;
        assert!(long > short, "内容变多时卡片应该变高：{short} -> {long}");
        assert!(long <= MAX_DETAIL_H + 0.5);
    }

    #[test]
    fn question_height_does_not_jump_when_an_answer_is_selected() {
        let ctx = ctx();
        let m = Measurer::new(&ctx);
        let questions = vec![
            Question { id: "q1".into(), question: "选一个方案".into(), detail: "说明".into(), multi: false,
                options: vec![Choice { label: "A".into(), description: "a".into() }, Choice { label: "B".into(), description: String::new() }] },
        ];
        let mut answers = vec![Answer { id: "q1".into(), ..Default::default() }];
        let before = {
            let view = QuestionView { head: head(), questions: &questions, answers: &mut answers, index: 0, scroll: 0.0,
                reviewing: false, queue: 1, handoff: None, state: State::Pending, error: None, age: 1.0 };
            measure_question(&m, &view, CARD_W)
        };
        answers[0].selected.push("A".into());
        let after = {
            let view = QuestionView { head: head(), questions: &questions, answers: &mut answers, index: 0, scroll: 0.0,
                reviewing: false, queue: 1, handoff: None, state: State::Pending, error: None, age: 1.0 };
            measure_question(&m, &view, CARD_W)
        };
        assert_eq!(before, after, "作答前后卡片高度必须一致，否则按钮会跳位");
    }

    #[test]
    fn submitting_and_uncertain_keep_the_layout_stable() {
        // 提交中/结果未知只是状态变化，卡片尺寸不能跳（否则按钮会在点击后移位）。
        let ctx = ctx();
        let m = Measurer::new(&ctx);
        for state in [State::Pending, State::Submitting, State::Uncertain] {
            let mut view = approval(head(), "{\"a\":1}", "原因");
            view.state = state;
            assert_eq!(measure_approval(&m, &view, CARD_W), measure_approval(&m, &approval(head(), "{\"a\":1}", "原因"), CARD_W), "审批卡 {state:?} 高度不应变化");
        }
        let questions = vec![
            Question { id: "q1".into(), question: "选一个".into(), detail: String::new(), multi: false,
                options: vec![Choice { label: "A".into(), description: String::new() }] },
        ];
        let mut answers = vec![Answer { id: "q1".into(), ..Default::default() }];
        let base = measure_question(&m, &QuestionView { head: head(), questions: &questions, answers: &mut answers, index: 0, scroll: 0.0,
            reviewing: false, queue: 1, handoff: None, state: State::Pending, error: None, age: 1.0 }, CARD_W);
        for state in [State::Submitting, State::Uncertain] {
            let got = measure_question(&m, &QuestionView { head: head(), questions: &questions, answers: &mut answers, index: 0, scroll: 0.0,
                reviewing: false, queue: 1, handoff: None, state, error: None, age: 1.0 }, CARD_W);
            assert_eq!(got, base, "提问卡 {state:?} 高度不应变化");
        }
    }

    #[test]
    fn result_height_stays_within_the_canvas() {
        let ctx = ctx();
        let m = Measurer::new(&ctx);
        for repeat in [0usize, 3, 200] {
            let text = "## 标题\n- 条目\n\n段落文字。".repeat(repeat);
            let view = ResultView { provider: crate::agent_notice::Provider::Dsh, kicker: "本轮回复", text: &text, scroll: 0.0, age: 1.0, copied: None };
            let (w, h) = measure_result(&m, &view);
            assert_eq!(w, CARD_W);
            assert!(h > 0.0 && h <= WIN_H);
        }
    }

    #[test]
    fn compact_banner_and_done_fit_the_canvas() {
        let ctx = ctx();
        let m = Measurer::new(&ctx);
        let banner = BannerView { head: head(), summary: "命令摘要", handoff: Some(12), queue: 3, approval: true, age: 1.0 };
        let (_, bh) = measure_banner(&m, &banner, CARD_W);
        assert!(bh > 0.0 && bh <= WIN_H);
        for compact in [true, false] {
            let done = DoneView { provider: crate::agent_notice::Provider::Dsh, kicker: "本轮已结束", text: "摘要", compact, age: 1.0 };
            let (_, dh) = measure_done(&m, &done);
            assert!(dh > 0.0 && dh <= WIN_H);
        }
    }

    #[test]
    fn entrance_animation_is_monotonic_and_settles() {
        assert_eq!(entrance(0.0).0, 0.0);
        assert_eq!(entrance(-1.0).0, 0.0);
        assert_eq!(entrance(2.0), (1.0, 0.0));
        let mut last = 0.0;
        for step in 0..30 {
            let alpha = entrance(step as f32 / 60.0).0;
            assert!(alpha >= last - 1e-6, "入场透明度必须单调递增");
            last = alpha;
        }
    }
}
