//! 完成通知（轻提示 / 展开卡）与「本轮回复」查看层。
//!
//! 参考 island-hud 的 jump 场景：正文按受限 markdown 渲染（标题/列表/引用/代码块/段落），
//! 限高滚动，提供复制按钮；不读取历史、不落盘，只用宿主事件里带来的正文。

use super::metrics::{self as m, galley, inset, M, BTN_H, CONTENT_W, FOOT_PAD, META, PAD};
use super::widgets::{self, Viewport};
use super::{Action, DoneView, ResultView};
use crate::paint::Card;
use crate::theme::{self, W};
use eframe::egui::{self, Color32};
use std::sync::Arc;

/// 完成通知的两种形态尺寸。
pub(crate) fn measure_done(m: &M, v: &DoneView) -> (f32, f32) {
    if v.compact {
        return (334.0, 44.0);
    }
    let body_h = if v.text.trim().is_empty() || v.text.trim() == crate::agent_notice::Status::Finished.label() { 0.0 } else { m.height(&m.text(&preview_text(v.text, 90), m::META, W::Regular, CONTENT_W)) };
    let h = (m::HEAD_TOP + m::ICON + 8.0 + body_h + 10.0 + BTN_H + 14.0).max(96.0);
    (super::CARD_W, h.min(160.0))
}

/// 预览：把多行回复压成一段摘要，并去掉 markdown 标记，避免卡片上出现 `##`、```。
fn preview_text(text: &str, limit: usize) -> String {
    let mut flat = String::new();
    for raw in text.lines() {
        let mut line = raw.trim();
        if line.starts_with("```") {
            continue;
        }
        while let Some(rest) = line.strip_prefix('#').or_else(|| line.strip_prefix('>')) {
            line = rest.trim_start();
        }
        if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            line = rest;
        }
        if line.is_empty() {
            continue;
        }
        if !flat.is_empty() {
            flat.push(' ');
        }
        flat.push_str(line);
    }
    if flat.is_empty() {
        return "本轮已结束".to_owned();
    }
    let mut out = String::new();
    for (i, ch) in flat.chars().enumerate() {
        if i >= limit {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

pub(crate) fn draw_done(c: &Card, m: &M, v: &DoneView, t: f64) -> Option<Action> {
    let (w, h) = measure_done(m, v);
    let (alpha, dy) = super::entrance(v.age);
    let c = c.faded(alpha).shifted(0.0, dy);
    let mut action = None;
    let status = crate::agent_notice::Status::Finished.label();
    let has_detail = !v.text.trim().is_empty() && v.text.trim() != status;
    // 没有正文时不要重复同一句话；摘要行留空，卡片只说「本轮已结束」。
    let summary_text = if has_detail { preview_text(v.text, 90) } else { String::new() };
    if v.compact {
        // 单行提示：图标 + 来源 + 摘要，右侧两枚小按钮。
        let title = m.fit(v.kicker, 11.5, W::Semibold, 120.0);
        crate::extras::agent_icon(&c, 20.0, h / 2.0, 22.0, v.provider);
        galley(&c, 40.0, 7.0, &title, theme::WHITE);
        if !summary_text.is_empty() {
            let body = m.fit(&preview_text(v.text, 60), 10.0, W::Regular, 150.0);
            galley(&c, 40.0, 24.0, &body, theme::AGENT_MUTED);
        }
        if has_detail
            && widgets::chip(&c, w - 96.0, (h - 22.0) / 2.0, 40.0, 22.0, "agent-result-open", "查看", theme::AGENT_OK)
        {
            action = Some(Action::ShowResult);
        }
        if widgets::chip(&c, w - 50.0, (h - 22.0) / 2.0, 40.0, 22.0, "agent-done-close", "关闭", theme::AGENT_MUTED) {
            action = Some(Action::Dismiss);
        }
        return action;
    }
    // 展开卡：✓ 状态行 + 摘要 + 两个按钮。
    crate::extras::agent_icon(&c, PAD + m::ICON / 2.0, m::HEAD_TOP + m::ICON / 2.0, m::ICON, v.provider);
    // 完成徽标：图标右下角的深色圆底 + 绿色勾，与参考项目 completed-head 的 ✓ 同义。
    let (bx, by) = (PAD + m::ICON - 2.5, m::HEAD_TOP + m::ICON - 2.5);
    c.circle(bx, by, 6.5, Color32::from_rgb(12, 12, 14));
    widgets::check(&c, bx, by, 8.0, theme::AGENT_OK);
    galley(&c, PAD + 32.0, m::HEAD_TOP + 1.0, &m.fit(v.kicker, m::TITLE, W::Semibold, w - PAD * 2.0 - 40.0), theme::WHITE);
    galley(
        &c,
        PAD + 32.0,
        m::HEAD_TOP + 1.0 + m::TITLE * 1.5,
        &m.fit(&format!("{} · 不代表任务成功", v.provider.label()), m::SUBTITLE, W::Regular, w - PAD * 2.0 - 40.0),
        theme::AGENT_SUBTLE,
    );
    let body_top = m::HEAD_TOP + m::ICON + 8.0;
    let body = m.text(&summary_text, m::META, W::Regular, CONTENT_W);
    if !summary_text.is_empty() {
        galley(&c, PAD, body_top, &body, theme::AGENT_MUTED);
    }    let fy = (h - 14.0 - BTN_H).max(body_top + c.galley_h(&body) + 6.0);
    let specs = [("agent-result-open", "查看回复", true), ("agent-done-close", "关闭", false)];
    if let Some(i) = widgets::draw_footer(&c, fy, w, &specs, true) {
        action = Some(if i == 0 { Action::ShowResult } else { Action::Dismiss });
    }
    let _ = t;
    action
}

// ---------------- 结果正文（受限 markdown） ----------------

enum Seg {
    H1(Arc<egui::Galley>),
    H2(Arc<egui::Galley>),
    Paragraph(Arc<egui::Galley>),
    Bullet(Arc<egui::Galley>),
    Quote(Arc<egui::Galley>),
    Code(Arc<egui::Galley>),
    Gap(f32),
}

fn parse(m: &M, text: &str, width: f32) -> Vec<Seg> {
    let mut segs = Vec::new();
    let mut in_code = false;
    let mut code = String::new();
    let mut paragraph = String::new();
    let flush_paragraph = |paragraph: &mut String, segs: &mut Vec<Seg>| {
        let t = paragraph.trim();
        if !t.is_empty() {
            segs.push(Seg::Paragraph(m.text(t, m::BODY, W::Regular, width)));
        }
        paragraph.clear();
    };
    for raw in text.lines() {
        if raw.trim_start().starts_with("```") {
            if in_code {
                segs.push(Seg::Code(m.mono(code.trim_end(), m::CODE, W::Regular, width - 16.0)));
                segs.push(Seg::Gap(8.0));
                code.clear();
            } else {
                flush_paragraph(&mut paragraph, &mut segs);
            }
            in_code = !in_code;
            continue;
        }
        if in_code {
            code.push_str(raw);
            code.push('\n');
            continue;
        }
        let line = raw.trim_end();
        if line.trim().is_empty() {
            flush_paragraph(&mut paragraph, &mut segs);
            continue;
        }
        if let Some(rest) = line.strip_prefix("### ") {
            flush_paragraph(&mut paragraph, &mut segs);
            segs.push(Seg::H2(m.text(rest, 12.0, W::Semibold, width)));
        } else if let Some(rest) = line.strip_prefix("## ") {
            flush_paragraph(&mut paragraph, &mut segs);
            segs.push(Seg::H2(m.text(rest, 12.5, W::Semibold, width)));
        } else if let Some(rest) = line.strip_prefix("# ") {
            flush_paragraph(&mut paragraph, &mut segs);
            segs.push(Seg::H1(m.text(rest, 13.5, W::Semibold, width)));
        } else if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            flush_paragraph(&mut paragraph, &mut segs);
            segs.push(Seg::Bullet(m.text(rest, m::BODY, W::Regular, width - 14.0)));
        } else if let Some(rest) = line.strip_prefix("> ") {
            flush_paragraph(&mut paragraph, &mut segs);
            segs.push(Seg::Quote(m.text(rest, m::BODY, W::Regular, width - 12.0)));
        } else {
            if !paragraph.is_empty() {
                paragraph.push(' ');
            }
            paragraph.push_str(line.trim());
        }
    }
    flush_paragraph(&mut paragraph, &mut segs);
    if !code.trim().is_empty() {
        segs.push(Seg::Code(m.mono(code.trim_end(), m::CODE, W::Regular, width - 16.0)));
        segs.push(Seg::Gap(8.0));
    }
    segs
}

fn seg_height(c: &Card, seg: &Seg) -> f32 {
    match seg {
        Seg::H1(g) | Seg::H2(g) | Seg::Paragraph(g) | Seg::Bullet(g) | Seg::Quote(g) => c.galley_h(g),
        Seg::Code(g) => c.galley_h(g) + 16.0,
        Seg::Gap(h) => *h,
    }
}

fn seg_gap(seg: &Seg) -> f32 {
    match seg {
        Seg::H1(_) => 8.0,
        Seg::H2(_) => 6.0,
        Seg::Bullet(_) | Seg::Paragraph(_) | Seg::Quote(_) => 7.0,
        Seg::Code(_) => 0.0,
        Seg::Gap(_) => 0.0,
    }
}

pub(crate) fn measure_result(m: &M, v: &ResultView) -> (f32, f32) {
    let w = super::CARD_W;
    let body_top = m::HEAD_TOP + m::ICON + 10.0;
    let segs = parse(m, v.text, w - PAD * 2.0);
    let mut content = 0.0;
    for (i, seg) in segs.iter().enumerate() {
        if i > 0 {
            content += seg_gap(&segs[i - 1]);
        }
        content += match seg {
            Seg::Code(g) => g.size().y + 16.0,
            Seg::Gap(h) => *h,
            Seg::H1(g) | Seg::H2(g) | Seg::Paragraph(g) | Seg::Bullet(g) | Seg::Quote(g) => g.size().y,
        };
    }
    let fixed = body_top + 12.0 + BTN_H + FOOT_PAD;
    let view = content.min((super::MAX_DETAIL_H - fixed).max(80.0));
    (w, fixed + view)
}

pub(crate) fn draw_result(c: &Card, m: &M, v: &ResultView, t: f64) -> Option<Action> {
    let w = super::CARD_W;
    let (alpha, dy) = super::entrance(v.age);
    let c = c.faded(alpha).shifted(0.0, dy);
    let mut action = None;
    crate::extras::agent_icon(&c, PAD + m::ICON / 2.0, m::HEAD_TOP + m::ICON / 2.0, m::ICON, v.provider);
    galley(&c, PAD + 32.0, m::HEAD_TOP + 1.0, &m.fit(v.kicker, m::TITLE, W::Semibold, CONTENT_W - 90.0), theme::WHITE);
    let hint = if v.copied.is_some() { "已复制到剪贴板" } else { "可滚动阅读 · 只读，不落盘" };
    let hint_color = if v.copied.is_some() { theme::AGENT_OK } else { theme::AGENT_SUBTLE };
    galley(&c, PAD + 32.0, m::HEAD_TOP + 1.0 + m::TITLE * 1.5, &m.line(hint, m::SUBTITLE, W::Regular), hint_color);
    let body_top = m::HEAD_TOP + m::ICON + 10.0;
    let segs = parse(m, v.text, w - PAD * 2.0);
    let content: f32 = {
        let mut total = 0.0;
        for (i, seg) in segs.iter().enumerate() {
            if i > 0 {
                total += seg_gap(&segs[i - 1]);
            }
            total += seg_height(&c, seg);
        }
        total
    };
    let (_, h) = measure_result(m, v);
    let view_h = (h - (body_top + 12.0 + BTN_H + FOOT_PAD)).max(60.0);
    let viewport = Viewport { top: body_top, height: view_h, content, offset: v.scroll };
    let scroll = viewport.wheel(&c, v.scroll, w);
    let body = c.clipped(PAD - 6.0, body_top, w - PAD * 2.0 + 12.0, view_h).shifted(0.0, -scroll);
    let mut y = body_top;
    for (i, seg) in segs.iter().enumerate() {
        if i > 0 {
            y += seg_gap(&segs[i - 1]);
        }
        match seg {
            Seg::H1(g) => galley(&body, PAD, y, g, theme::WHITE),
            Seg::H2(g) => galley(&body, PAD, y, g, theme::WHITE),
            Seg::Paragraph(g) => galley(&body, PAD, y, g, theme::AGENT_MUTED),
            Seg::Bullet(g) => {
                // 无序列表用圆点而不是勾：勾会被读成「已完成事项」，正文里只是普通条目。
                body.circle(PAD + 5.0, y + 7.0, 2.0, theme::AGENT_SUBTLE);
                galley(&body, PAD + 14.0, y, g, theme::AGENT_MUTED);
            }
            Seg::Quote(g) => {
                body.fill(PAD, y, 2.0, c.galley_h(g), 1.0, Color32::from_white_alpha(60));
                galley(&body, PAD + 10.0, y, g, theme::AGENT_SUBTLE);
            }
            Seg::Code(g) => {
                let bh = c.galley_h(g) + 16.0;
                inset(&body, PAD, y, CONTENT_W, bh, theme::AGENT_INSET, theme::AGENT_INSET_EDGE);
                galley(&body, PAD + 8.0, y + 8.0, g, Color32::from_rgb(224, 217, 207));
            }
            Seg::Gap(_) => {}
        }
        y += seg_height(&c, seg);
    }
    viewport.bar(&c, w, 1.0);
    let fy = h - FOOT_PAD - BTN_H;
    let specs = [("agent-result-copy", "复制内容", false), ("agent-result-close", "关闭", true)];
    if let Some(i) = widgets::draw_footer(&c, fy, w, &specs, true) {
        action = Some(if i == 0 { Action::CopyResult } else { Action::Dismiss });
    }
    let _ = t;
    action
}
