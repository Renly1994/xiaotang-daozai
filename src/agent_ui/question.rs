//! 提问详情：一题一屏、选项带描述与完整选中态、自定义回答在正文流内、最后一题先审阅再提交。
//!
//! 交互约束（见 `docs/PROJECT-HANDOFF.md` 第 6 节，不得回退成一次性大表单）：
//! 1. 只显示当前题；上一题/下一题切换保留各题答案，切题时正文滚动复位（由上层写回）；
//! 2. 单选与自定义互斥，多选累加；自定义输入在正文流内（跟随滚动、真实可输入）；
//! 3. 最后一题不直接提交：先进入「检查答案」审阅页，确认后才提交，审阅页支持 `Ctrl+Enter`；
//! 4. 提交类按钮在未答满时禁用，并在按钮上方给出可见原因（不只是变灰）；
//! 5. 关闭详情只是收起，不交回客户端。
//!
//! 工程约束：`measure()` 与 `draw_question()` 共用**同一份**布局计算结果（所有 y 由游标累加
//! 后存进布局结构），因此测量高度必然等于绘制所需高度；正文超出 `body_view` 时裁剪并允许滚动。

use super::metrics::{
    self as m, galley, hairline, inset, M, BODY_TOP_GAP, BTN_H, FOOT_PAD, HEAD_H, LABEL, PAD,
};
use super::widgets::{self, HeadLayout, Status, StatusLayout, Viewport};
use super::{Action, Answer, Question, QuestionView};
use crate::paint::{self, Card};
use crate::theme::{self, W};
use eframe::egui::{self, Color32};
use std::sync::Arc;

// ---------- 提问卡专用尺寸 token ----------

/// 选项行最小高度（任务要求 ≥30pt；带描述时自然更高）。
const ROW_MIN_H: f32 = 30.0;
/// 选项行上下内边距。
const ROW_PAD: f32 = 7.0;
/// 选项行左内边距。
const ROW_INSET: f32 = 7.0;
/// 序号块边长。
const NUM: f32 = 19.0;
/// 选项行之间的间距。
const ROW_GAP: f32 = 6.0;
/// 行内文字左边界（相对行左边缘）：左内边距 + 序号块 + 与文字的间距。
const ROW_TEXT_X: f32 = ROW_INSET + NUM + 9.0;
/// 右侧 ✓ 占位（左右各留白），保证文字不会压到勾选。
const CHECK_ROOM: f32 = 22.0;
/// 自定义输入框高度（单行 TextEdit）。
const INPUT_H: f32 = 30.0;
/// 题干/说明与选项之间的间距。
const OPTIONS_GAP: f32 = 10.0;
/// 选项与自定义区之间的间距。
const ANSWER_GAP: f32 = 12.0;
/// 选项描述第二行字号（10pt muted）。
const DESC: f32 = 10.0;

// ---------- 提问卡专用颜色（对齐 island-hud §2.7 的选中/悬停配方） ----------

/// 选项行未选中底色。
const OPT_FILL: Color32 = Color32::from_rgb(24, 22, 20);
/// 选项行未选中悬停底色。
const OPT_FILL_HOVER: Color32 = Color32::from_rgb(44, 36, 28);
/// 选中行底色（浅绿）。
const OPT_SELECTED: Color32 = Color32::from_rgb(22, 42, 26);
/// 选中行悬停底色。
const OPT_SELECTED_HOVER: Color32 = Color32::from_rgb(29, 52, 33);
/// 选中行 1px 绿边。
const OPT_EDGE: Color32 = Color32::from_rgb(55, 108, 67);
/// 序号块：未选中 / 选中。
const NUM_IDLE: Color32 = Color32::from_rgb(86, 59, 33);
const NUM_ON: Color32 = Color32::from_rgb(55, 108, 67);
const NUM_IDLE_INK: Color32 = Color32::from_rgb(255, 193, 132);
const NUM_ON_INK: Color32 = Color32::from_rgb(236, 255, 240);
/// 选中行右侧 ✓。
const CHECK_INK: Color32 = Color32::from_rgb(107, 216, 119);
/// 选项描述（第二行）文字色。
const DESC_INK: Color32 = Color32::from_rgb(179, 170, 160);
/// 输入框聚焦描边。
const FOCUS_EDGE: Color32 = Color32::from_rgb(152, 109, 62);

// ---------- 布局数据结构（测量结果即绘制依据） ----------

struct OptionRow {
    /// 作为答案值的选项原文（wire 上就用它）。
    key: String,
    label: Arc<egui::Galley>,
    description: Option<Arc<egui::Galley>>,
    number: Arc<egui::Galley>,
    /// 相对正文内容顶部的偏移。
    top: f32,
    h: f32,
}

struct AnswerBody {
    qid: String,
    multi: bool,
    cw: f32,
    question: Arc<egui::Galley>,
    detail: Option<Arc<egui::Galley>>,
    detail_top: f32,
    rows: Vec<OptionRow>,
    answer_label: Arc<egui::Galley>,
    answer_label_top: f32,
    input_top: f32,
    input_h: f32,
    content: f32,
}

struct ReviewItem {
    title: Arc<egui::Galley>,
    answer: Arc<egui::Galley>,
    top: f32,
    missing: bool,
}

struct ReviewBody {
    heading: Arc<egui::Galley>,
    items: Vec<ReviewItem>,
    cw: f32,
    content: f32,
}

enum Body {
    Answer(AnswerBody),
    Review(ReviewBody),
}

impl Body {
    fn content(&self) -> f32 {
        match self {
            Self::Answer(a) => a.content,
            Self::Review(r) => r.content,
        }
    }
}

pub(crate) struct Layout {
    pub w: f32,
    pub h: f32,
    head: HeadLayout,
    status: Status,
    status_g: StatusLayout,
    body: Body,
    body_top: f32,
    body_view: f32,
    error: Option<String>,
    error_h: f32,
    hint: Option<Arc<egui::Galley>>,
    hint_h: f32,
    footer: [(&'static str, &'static str, bool); 2],
    reviewing: bool,
    index: usize,
    all_ready: bool,
    ready_now: bool,
}

fn build_answer(m: &M, q: &Question, cw: f32) -> AnswerBody {
    let question = m.text(&q.question, m::BODY, W::Semibold, cw);
    let mut y = m.height(&question);
    let detail = if q.detail.trim().is_empty() {
        None
    } else {
        Some(m.text(q.detail.trim(), m::SUBTITLE, W::Regular, cw))
    };
    let detail_top = y + 6.0;
    if let Some(d) = &detail {
        y = detail_top + m.height(d);
    }
    let text_w = (cw - ROW_TEXT_X - CHECK_ROOM).max(60.0);
    let mut rows = Vec::with_capacity(q.options.len());
    let mut oy = y + OPTIONS_GAP;
    for (i, o) in q.options.iter().enumerate() {
        let label = m.text(&o.label, m::BODY, W::Semibold, text_w);
        let description = if o.description.trim().is_empty() {
            None
        } else {
            Some(m.text(o.description.trim(), DESC, W::Regular, text_w))
        };
        let mut h = ROW_PAD * 2.0 + m.height(&label);
        if let Some(d) = &description {
            h += 2.0 + m.height(d);
        }
        let h = h.max(ROW_MIN_H);
        if i > 0 {
            oy += ROW_GAP;
        }
        rows.push(OptionRow {
            key: o.label.clone(),
            label,
            description,
            number: m.line_mono(&(i + 1).to_string(), 9.5, W::Semibold),
            top: oy,
            h,
        });
        oy += h;
    }
    let label_top = if rows.is_empty() { y + OPTIONS_GAP } else { oy + ANSWER_GAP };
    let answer_label = m.line(
        if q.options.is_empty() {
            "你的回答"
        } else if q.multi {
            "补充内容（可选）"
        } else {
            "或自行输入"
        },
        LABEL,
        W::Regular,
    );
    let input_top = label_top + m.height(&answer_label) + 6.0;
    AnswerBody {
        qid: q.id.clone(),
        multi: q.multi,
        cw,
        question,
        detail,
        detail_top,
        rows,
        answer_label,
        answer_label_top: label_top,
        input_top,
        input_h: INPUT_H,
        content: input_top + INPUT_H,
    }
}

/// 把某题的作答拼成一行可读文本：选项用「；」连接，已填的自定义内容追加在末尾。
fn answer_text(a: &Answer) -> String {
    let mut parts = a.selected.clone();
    let custom = a.custom.trim();
    if !custom.is_empty() {
        parts.push(custom.to_owned());
    }
    if parts.is_empty() {
        "未填写（将跳过此题）".to_owned()
    } else {
        parts.join("；")
    }
}

fn build_review(m: &M, v: &QuestionView, cw: f32) -> ReviewBody {
    let heading = m.line(&format!("检查答案 · 共 {} 题", v.questions.len()), m::BODY, W::Semibold);
    let mut y = m.height(&heading) + 12.0;
    let mut items = Vec::with_capacity(v.questions.len());
    for (i, q) in v.questions.iter().enumerate() {
        let answer = v.answers.get(i);
        let missing = !answer.is_some_and(|a| a.ready());
        let text = match answer {
            Some(a) if !missing => answer_text(a),
            _ => "未填写（将跳过此题）".to_owned(),
        };
        let title = m.text(&format!("{}. {}", i + 1, q.question), m::BODY, W::Semibold, cw);
        let line = m.text(&text, m::SUBTITLE, W::Regular, cw);
        let top = y;
        y += 10.0 + m.height(&title) + 4.0 + m.height(&line) + 10.0;
        items.push(ReviewItem { title, answer: line, top, missing });
    }
    ReviewBody { heading, items, cw, content: y }
}

fn extra_line(queue: usize, handoff: Option<u8>) -> String {
    // 倒计时比队列更紧急，占用同一个位置时优先显示倒计时。
    match handoff {
        Some(0) => return "已交回客户端".to_owned(),
        Some(s) => return format!("{s} 秒后回客户端"),
        None => {}
    }
    if queue > 1 {
        format!("1 / {queue} 待处理")
    } else {
        String::new()
    }
}

pub(crate) fn measure(m: &M, v: &QuestionView, width: f32) -> Layout {
    let w = super::card_width(width);
    let cw = w - PAD * 2.0;
    let head = widgets::measure_head(m, &v.head, w);
    let total = v.questions.len();
    let index = if total == 0 { 0 } else { v.index.min(total - 1) };
    let answered = v.answers.iter().take(total).filter(|a| a.ready()).count();
    let missing = total.saturating_sub(answered);
    let reviewing = v.reviewing && total > 0;
    let all_ready = total > 0 && missing == 0;
    let ready_now = v.answers.get(index).is_some_and(|a| a.ready());
    let multi = v.questions.get(index).is_some_and(|q| q.multi);

    // 任务约定：左侧一句状态语义，右侧给进度；审阅页换成「已答 x/N 题 + 还差 / 待确认」。
    let (tone, status_text, status_right) = if reviewing {
        if all_ready {
            (theme::AGENT_OK, "检查答案".to_owned(), format!("已答 {answered}/{total} 题 · 核对后确认提交"))
        } else {
            (theme::AGENT_ALERT, "检查答案".to_owned(), format!("已答 {answered}/{total} 题 · 还差 {missing} 题"))
        }
    } else {
        (
            theme::AGENT_ALERT,
            "需要回答后才能继续".to_owned(),
            format!("第 {}/{} 题 · {} · 已答 {answered}", index + 1, total.max(1), if multi { "多选" } else { "单选" }),
        )
    };
    let status = Status { tone, text: status_text, right: status_right, extra: extra_line(v.queue, v.handoff) };
    let status_g = widgets::measure_status(m, &status, w);

    let body = if reviewing {
        Body::Review(build_review(m, v, cw))
    } else {
        match v.questions.get(index) {
            Some(q) => Body::Answer(build_answer(m, q, cw)),
            None => Body::Answer(build_answer(
                m,
                &Question { id: String::new(), question: "题目已失效，请回客户端查看。".into(), detail: String::new(), multi: false, options: Vec::new() },
                cw,
            )),
        }
    };

    let error = v.error.map(str::to_owned);
    let error_h = widgets::error_height(m, error.as_deref(), w);
    // 禁用原因必须可见，不能只把按钮变灰。
    let hint_text = if reviewing && missing > 0 {
        Some(format!("还有 {missing} 题未答，返回修改补全后可提交"))
    } else if !reviewing && total > 0 && index + 1 >= total && missing > 0 {
        Some(format!("还有 {missing} 题未答"))
    } else {
        None
    };
    // **固定提示槽位**：可能出提示的界面（最后一题 / 审阅页）无论当前有没有提示都预留一行高度。
    // 否则「选中最后一个选项」会让 hint 消失、卡片变矮，底部按钮整体上移约 20pt（实测点不中）。
    let hint_capable = reviewing || (total > 0 && index + 1 >= total);
    let hint_h = if hint_capable { m.height(&m.line("测", LABEL, W::Regular)) + 6.0 } else { 0.0 };
    // 提示固定压成单行（超宽省略），保证不会超出预留槽位。
    let hint = hint_text.as_deref().map(|t| m.fit(t, LABEL, W::Regular, cw));

    let body_top = m::HEAD_TOP + HEAD_H + m::STATUS_H + BODY_TOP_GAP;
    let fixed = body_top + error_h + hint_h + 12.0 + BTN_H + FOOT_PAD;
    let body_view = body.content().min((super::MAX_DETAIL_H - fixed).max(56.0));
    let h = fixed + body_view;

    let first = if index > 0 { ("agent-prev", "上一题", false) } else { ("agent-return", "回客户端", false) };
    let footer = if reviewing {
        [("agent-review-back", "返回修改", false), ("agent-review-submit", "确认提交", true)]
    } else if index + 1 >= total || total == 0 {
        [first, ("agent-review", "检查答案", true)]
    } else {
        [first, ("agent-next", "下一题", true)]
    };

    Layout { w, h, head, status, status_g, body, body_top, body_view, error, error_h, hint, hint_h, footer, reviewing, index, all_ready, ready_now }
}

pub(crate) fn measure_question(m: &M, v: &QuestionView, width: f32) -> (f32, f32) {
    let layout = measure(m, v, width);
    (layout.w, layout.h)
}

/// 提问卡的绘制结果。
#[derive(Default)]
pub(crate) struct QuestionResult {
    /// 需要交给状态机的动作（交回 / 提交答案 / 跳转）。
    pub action: Option<Action>,
    /// 写回本卡正文滚动偏移。
    pub scroll: f32,
    /// 需要切换到的题号（上一题 / 下一题）。
    pub index: Option<usize>,
    /// 本帧选项或自定义回答是否发生变化。
    pub input_changed: bool,
    /// 是否切换「检查答案」页：`Some(true)` 进入审阅，`Some(false)` 返回修改。
    pub reviewing: Option<bool>,
}

/// 在正文流内放一个真实可输入的单行输入框。
///
/// 不用 `egui::Area`：这里用与卡片同层、同 `Card` 坐标的子 `Ui`，因此它天然被正文视口裁剪、
/// 跟随滚动、跟随卡片淡入，也不会在岛体收起后浮在面板外（旧实现的第 5.7 条问题）。
fn text_input(c: &Card, rect: egui::Rect, clip: egui::Rect, text: &mut String, salt: &str, enabled: bool) -> egui::Response {
    let mut ui = egui::Ui::new(
        c.ui.ctx().clone(),
        egui::Id::new(("agent-custom-ui", salt)),
        egui::UiBuilder::new().layer_id(c.ui.layer_id()).max_rect(rect).style(c.ui.style().clone()),
    );
    ui.set_clip_rect(clip.intersect(c.painter.clip_rect()));
    ui.set_opacity(c.painter.opacity());
    if !enabled || !c.live {
        ui.disable();
    }
    ui.spacing_mut().button_padding = egui::vec2(0.0, 0.0);
    let edit = egui::TextEdit::singleline(text)
        .id(egui::Id::new(("agent-custom", salt)))
        .frame(false)
        .margin(egui::Margin::symmetric(8, 6))
        .vertical_align(egui::Align::Center)
        .hint_text("自定义回答")
        .font(theme::font(11.0, W::Regular))
        .text_color(theme::WHITE);
    ui.put(rect, edit)
}

fn draw_answer_body(body: &Card, c: &Card, a: &AnswerBody, v: &mut QuestionView, body_top: f32, scroll: f32, busy: bool, out: &mut QuestionResult) {
    let x = PAD;
    galley(body, x, body_top, &a.question, theme::WHITE);
    if let Some(d) = &a.detail {
        galley(body, x, body_top + a.detail_top, d, theme::AGENT_MUTED);
    }

    let mut answer = v.answers.get_mut(v.index.min(v.answers.len().saturating_sub(1)));
    for (i, row) in a.rows.iter().enumerate() {
        let ry = body_top + row.top;
        let selected = answer.as_ref().is_some_and(|ans| ans.selected.iter().any(|s| s == &row.key));
        let r = body.hit(x, ry, a.cw, row.h, format!("agent-option-{}-{}", a.qid, i));
        let (hover, _) = if busy { (0.0, 0.0) } else { body.hover_t(&r) };
        let fill = if selected {
            paint::mix(OPT_SELECTED, OPT_SELECTED_HOVER, hover)
        } else {
            paint::mix(OPT_FILL, OPT_FILL_HOVER, hover)
        };
        body.fill(x, ry, a.cw, row.h, 9.0, fill);
        // 未选中也画 1px 透明描边：选中时边框宽度不变，行内文字不会跳动。
        body.stroke(x, ry, a.cw, row.h, 9.0, 1.0, if selected { OPT_EDGE } else { Color32::TRANSPARENT });
        let ny = ry + (row.h - NUM) / 2.0;
        body.fill(x + ROW_INSET, ny, NUM, NUM, 5.0, if selected { NUM_ON } else { NUM_IDLE });
        let nw = body.galley_w(&row.number);
        let nh = body.galley_h(&row.number);
        galley(body, x + ROW_INSET + (NUM - nw) / 2.0, ny + (NUM - nh) / 2.0, &row.number, if selected { NUM_ON_INK } else { NUM_IDLE_INK });
        let tx = x + ROW_TEXT_X;
        galley(body, tx, ry + ROW_PAD, &row.label, theme::WHITE);
        if let Some(d) = &row.description {
            galley(body, tx, ry + ROW_PAD + body.galley_h(&row.label) + 2.0, d, DESC_INK);
        }
        if selected {
            widgets::check(body, x + a.cw - ROW_INSET - 6.0, ry + row.h / 2.0, 12.0, CHECK_INK);
        }
        if r.clicked() && !busy {
            if let Some(ans) = answer.as_mut() {
                out.input_changed = true;
                if a.multi {
                    if selected {
                        ans.selected.retain(|s| s != &row.key);
                    } else {
                        ans.selected.push(row.key.clone());
                    }
                } else {
                    // 单选：点谁就选谁（再次点击不取消），并与自定义回答互斥。
                    ans.selected.clear();
                    ans.custom.clear();
                    ans.selected.push(row.key.clone());
                }
            }
        }
    }

    galley(body, x, body_top + a.answer_label_top, &a.answer_label, theme::AGENT_SUBTLE);
    let iy = body_top + a.input_top;
    inset(body, x, iy, a.cw, a.input_h, theme::AGENT_INSET, theme::AGENT_INSET_EDGE);
    if let Some(ans) = answer.as_mut() {
        let rect = c.rect(x, iy - scroll, a.cw, a.input_h);
        let clip = body.painter.clip_rect();
        let resp = text_input(c, rect, clip, &mut ans.custom, &a.qid, !busy);
        if resp.changed() {
            out.input_changed = true;
            // 单选：一旦手写答案，就不再占用某个选项。
            if !a.multi {
                ans.selected.clear();
            }
        }
        if resp.has_focus() {
            body.stroke(x, iy, a.cw, a.input_h, 9.0, 1.0, FOCUS_EDGE);
        }
    }
}

fn draw_review_body(body: &Card, r: &ReviewBody, body_top: f32) {
    let x = PAD;
    galley(body, x, body_top, &r.heading, theme::WHITE);
    for (i, item) in r.items.iter().enumerate() {
        let y = body_top + item.top;
        // 分题分隔线：每题上方一条（首题不画，避免变成标题下划线），把答案切成可扫读的小块。
        if i > 0 {
            hairline(body, x, y, r.cw, 0.12);
        }
        galley(body, x, y + 10.0, &item.title, theme::WHITE);
        let ay = y + 10.0 + body.galley_h(&item.title) + 4.0;
        galley(body, x, ay, &item.answer, if item.missing { theme::AGENT_SUBTLE } else { theme::AGENT_MUTED });
    }
}

pub(crate) fn draw_question(c: &Card, m: &M, v: &mut QuestionView, scroll: f32, _t: f64) -> QuestionResult {
    let mut out = QuestionResult { scroll, ..Default::default() };
    let layout = measure(m, v, super::CARD_W);
    let w = layout.w;
    let (alpha, dy) = super::entrance(v.age);
    let c = c.faded(alpha).shifted(0.0, dy);
    if widgets::draw_head(&c, &layout.head, &v.head, w) {
        out.action = Some(Action::Jump);
    }
    widgets::draw_status(&c, &layout.status, &layout.status_g, m::HEAD_TOP + HEAD_H, w);

    let busy = v.state.busy();
    let viewport = Viewport { top: layout.body_top, height: layout.body_view, content: layout.body.content(), offset: scroll };
    let scroll = viewport.wheel(&c, scroll, w);
    out.scroll = scroll;
    let body = c.clipped(PAD - 6.0, layout.body_top, w - PAD * 2.0 + 12.0, layout.body_view).shifted(0.0, -scroll);

    match &layout.body {
        Body::Answer(a) => draw_answer_body(&body, &c, a, v, layout.body_top, scroll, busy, &mut out),
        Body::Review(r) => draw_review_body(&body, r, layout.body_top),
    }

    viewport.bar(&c, w, if viewport.max_offset() > 0.0 { 1.0 } else { 0.0 });

    let mut y = layout.body_top + layout.body_view + 12.0;
    if layout.error_h > 0.0 {
        let eh = widgets::draw_error(&c, m, y, layout.error.as_deref(), w);
        y += eh + 6.0;
    }
    if let Some(hint) = &layout.hint {
        galley(&c, PAD, y, hint, paint::alpha(theme::AGENT_ALERT, 0.85));
    }
    // 槽位恒定：有没有提示都推进同样的高度，按钮 y 不随作答状态变化。
    y += layout.hint_h;

    let clicked = widgets::draw_footer_with_enabled(&c, y, w, &layout.footer, !busy, |id| match id {
        // 下一题要求本题已作答；检查答案/确认提交要求所有题都已作答。
        "agent-next" => layout.ready_now,
        "agent-review" | "agent-review-submit" => layout.all_ready,
        _ => true,
    });
    if let Some(i) = clicked {
        match layout.footer[i].0 {
            "agent-return" => out.action = Some(Action::Return),
            "agent-prev" => {
                out.index = Some(layout.index.saturating_sub(1));
                out.scroll = 0.0;
            }
            "agent-next" => {
                out.index = Some(layout.index + 1);
                out.scroll = 0.0;
            }
            "agent-review" => {
                out.reviewing = Some(true);
                out.scroll = 0.0;
            }
            "agent-review-back" => {
                out.reviewing = Some(false);
                out.scroll = 0.0;
            }
            "agent-review-submit" if layout.all_ready => out.action = Some(Action::Answer),
            _ => {}
        }
    }
    // 审阅页 Ctrl+Enter 直接提交（与参考一致；输入法组合期间不会命中这个分支）。
    if layout.reviewing && layout.all_ready && !busy
        && c.ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))
    {
        out.action = Some(Action::Answer);
    }
    out
}

/// 手动导出的提问卡预览（`DAOZAI_QUESTION_PREVIEW_DIR=目录 cargo test --lib agent_ui::question -- --ignored`）。
///
/// 只用于开发期核对排版，不参与常规回归。
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_notice::Provider;
    use crate::agent_ui::{Head, Measurer, State};

    fn choice(label: &str, description: &str) -> super::super::Choice {
        super::super::Choice { label: label.to_owned(), description: description.to_owned() }
    }

    fn shot(name: &str, dir: &std::path::Path, questions: &[Question], answers: &mut Vec<Answer>, index: usize, reviewing: bool, state: State, error: Option<&str>) {
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let ppp = 2.0_f32;
        // 先按卡片上限给屏幕尺寸，布局在 run 里用真实字体测量（字体要等第一次 run 才可用）。
        let mut size = (super::super::CARD_W, super::super::MAX_DETAIL_H);
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(super::super::CARD_W, super::super::MAX_DETAIL_H))),
            time: Some(0.5),
            viewports: std::iter::once((
                egui::ViewportId::ROOT,
                egui::ViewportInfo { native_pixels_per_point: Some(ppp), ..Default::default() },
            ))
            .collect(),
            ..Default::default()
        };
        let output = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx, |ui| {
                ui.painter().rect_filled(ui.max_rect(), 0.0, egui::Color32::from_rgb(16, 17, 19));
                let card = Card::new(ui, ui.max_rect().min, ui.max_rect());
                let m = Measurer::new(ctx);
                let mut v = QuestionView {
                    head: Head { provider: Provider::Dsh, kicker: "需要你回答", subtitle: "C:\\work\\payments", jump: None },
                    questions,
                    answers: answers.as_mut_slice(),
                    index,
                    scroll: 0.0,
                    reviewing,
                    queue: 3,
                    handoff: Some(38),
                    state,
                    error,
                    age: 1.0,
                };
                size = measure_question(&m, &v, super::super::CARD_W);
                let _ = draw_question(&card, &m, &mut v, 0.0, 0.5);
            });
        });
        let (w, h) = size;
        let mut raster = crate::preview_raster::Raster::new((w * ppp) as usize, (h * ppp) as usize);
        raster.update_textures(&output.textures_delta);
        raster.buf.fill([0.05, 0.05, 0.06, 1.0]);
        let prims = ctx.tessellate(output.shapes, output.pixels_per_point);
        raster.draw(&prims, output.pixels_per_point);
        let mut rgba = Vec::new();
        raster.to_rgba8(&mut rgba);
        image::save_buffer(dir.join(format!("{name}.png")), &rgba, (w * ppp) as u32, (h * ppp) as u32, image::ColorType::Rgba8).unwrap();
        raster.free_textures(&output.textures_delta);
    }

    #[test]
    #[ignore = "开发期手动导出提问卡预览图"]
    fn preview_question_card() {
        let Some(dir) = std::env::var_os("DAOZAI_QUESTION_PREVIEW_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();

        let single = vec![Question {
            id: "q1".into(),
            question: "这次改动要包含哪些模块？".into(),
            detail: "可以多选，也可以在下面补充说明。".into(),
            multi: false,
            options: vec![
                choice("审批卡", "目标 / 操作内容 / 申请理由，长命令可滚动"),
                choice("提问卡", "选项描述、审阅页与自定义回答"),
                choice("完成通知", "本轮回复的受限 Markdown 阅读"),
            ],
        }];
        let mut answers = vec![Answer { id: "q1".into(), selected: vec!["提问卡".into()], custom: String::new() }];
        shot("answer-single", &dir, &single, &mut answers, 0, false, State::Pending, None);

        let multi = vec![Question {
            id: "q2".into(),
            question: "哪些细节需要一起打磨？".into(),
            detail: "多选：勾选后会累加，也可以再补充文字。".into(),
            multi: true,
            options: vec![
                choice("行高与字重", "正文 11.5，次级 10.5，标签 10"),
                choice("进度状态", "第 n/N 题 · 单选/多选 · 已答 x"),
                choice("自定义输入", "在正文流内，跟随滚动"),
                choice("审阅页", "列出每题所答，未填写会标注"),
                choice("禁用原因", "按钮变灰的同时给出原因"),
                choice("滚动条", "细滚动条，滚动时更明显"),
            ],
        }];
        let mut answers = vec![
            Answer { id: "q2a".into(), selected: vec!["行高与字重".into()], custom: String::new() },
            Answer { id: "q2".into(), selected: vec!["自定义输入".into(), "审阅页".into()], custom: "另外请留意导出格式。".into() },
            Answer { id: "q2c".into(), selected: vec![], custom: String::new() },
        ];
        shot("answer-multi", &dir, &multi, &mut answers, 0, false, State::Pending, None);

        let long = vec![Question {
            id: "q3".into(),
            question: "请确认这次重构的边界：只调整 Agent 通知、审批、提问与完成结果这条表现链路，协议、来源接入、托盘与其他页面保持不动；如果发现阻塞本功能的 bug 才顺带修，并记录在案。你同意这个范围吗？".into(),
            detail: "如果不同意，请在下方说明需要扩大或缩小的范围。".into(),
            multi: false,
            options: vec![
                choice("同意，按这个范围推进", "只动表现层，协议与来源接入保持不动"),
                choice("需要扩大范围", "例如把审批卡的 native_scope 授权语义一起补全"),
                choice("需要缩小范围", "先只做提问卡，其余留到下一轮"),
            ],
        }];
        let mut answers = vec![Answer { id: "q3".into(), selected: vec![], custom: String::new() }];
        shot("answer-long-stem", &dir, &long, &mut answers, 0, false, State::Pending, None);
        // 与上一张对照：最后一题已答（不再有 hint）时按钮 y 必须完全一致。
        let mut answers = vec![Answer { id: "q3".into(), selected: vec!["同意，按这个范围推进".into()], custom: String::new() }];
        shot("answer-long-stem-answered", &dir, &long, &mut answers, 0, false, State::Pending, None);

        let review_qs = vec![
            Question { id: "r1".into(), question: "这次改动要包含哪些模块？".into(), detail: String::new(), multi: false, options: vec![choice("审批卡", ""), choice("提问卡", "")] },
            Question { id: "r2".into(), question: "哪些细节需要一起打磨？".into(), detail: String::new(), multi: true, options: vec![choice("行高与字重", ""), choice("审阅页", "")] },
            Question { id: "r3".into(), question: "有没有需要保留的旧行为？".into(), detail: String::new(), multi: false, options: vec![choice("保留", ""), choice("不保留", "")] },
        ];
        let mut answers = vec![
            Answer { id: "r1".into(), selected: vec!["提问卡".into()], custom: String::new() },
            Answer { id: "r2".into(), selected: vec!["行高与字重".into(), "审阅页".into()], custom: "自定义回答会在末尾追加".into() },
            Answer { id: "r3".into(), selected: vec![], custom: String::new() },
        ];
        shot("review-incomplete", &dir, &review_qs, &mut answers, 2, true, State::Pending, None);
        let mut answers = vec![
            Answer { id: "r1".into(), selected: vec!["提问卡".into()], custom: String::new() },
            Answer { id: "r2".into(), selected: vec!["行高与字重".into()], custom: String::new() },
            Answer { id: "r3".into(), selected: vec!["保留".into()], custom: String::new() },
        ];
        shot("review-ready", &dir, &review_qs, &mut answers, 2, true, State::Pending, None);

        let mut answers = vec![Answer { id: "q1".into(), selected: vec!["审批卡".into()], custom: String::new() }];
        shot("answer-uncertain", &dir, &single, &mut answers, 0, false, State::Uncertain, Some("提交失败，结果未知。请回客户端确认后再处理。"));
    }
}
