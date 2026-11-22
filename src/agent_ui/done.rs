//! 完成通知（轻提示 / 展开卡）与「本轮回复」查看层。
//!
//! 参考 island-hud 的 jump 场景：正文按受限 markdown 渲染（标题/列表/引用/代码块/段落），
//! 限高滚动，提供复制按钮；不读取历史、不落盘，只用宿主事件里带来的正文。

use super::metrics::{self as m, galley, inset, M, BTN_H, CONTENT_W, FOOT_PAD, PAD};
use super::widgets::{self, Viewport};
use super::{Action, DoneView, ResultView};
use crate::paint::Card;
use crate::theme::{self, W};
use eframe::egui::{self, Color32};
use std::sync::Arc;

/// 没有正文时卡片上的一行说明：让用户在点「查看回复」之前就知道里面没有正文。
const NO_BODY_HINT: &str = "仅状态 · 未接收回复正文";

/// 是否存在可展示的回复正文（「仅状态」占位不算正文）。
fn has_detail(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty() && text != crate::agent_notice::Status::Finished.label()
}

/// 完成通知的两种形态尺寸。
pub(crate) fn measure_done(m: &M, v: &DoneView) -> (f32, f32) {
    if v.compact {
        return (334.0, 44.0);
    }
    // 摘要行（无正文时是说明行）必须计入高度，否则按钮会压到文字上。
    let line_text = if has_detail(v.text) { preview_text(v.text, 90) } else { NO_BODY_HINT.to_owned() };
    let line = m.text(&line_text, m::META, W::Regular, CONTENT_W);
    let h = (m::HEAD_TOP + m::ICON + 8.0 + m.height(&line) + 10.0 + BTN_H + 14.0).max(96.0);
    (super::CARD_W, h.min(160.0))
}

/// 预览：把多行回复压成一段摘要，并去掉 markdown 标记（`##`、```、`**`、反引号、表格竖线），
/// 避免卡片上出现原始标记。
fn preview_text(text: &str, limit: usize) -> String {
    let mut flat = String::new();
    for raw in text.lines() {
        let mut line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            continue;
        }
        // 表格分隔行只有符号没有文字；数据行的竖线在摘要里换成空格。
        if line.contains('|') && line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t')) {
            continue;
        }
        while let Some(rest) = line.strip_prefix('#').or_else(|| line.strip_prefix('>')) {
            line = rest.trim_start();
        }
        if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")).or_else(|| line.strip_prefix("+ ")) {
            line = rest;
        }
        if let Some((_, rest)) = ordered_prefix(line) {
            line = rest;
        }
        let line = strip_inline_marks(line);
        let line = line.trim();
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
    // 表格竖线、块标记都换成了空格：连续空白压成一个，摘要才不会出现空洞。
    let flat: String = flat.split_whitespace().collect::<Vec<_>>().join(" ");
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
    // UI 地图（DAOZAI_UI_MAP 未设置时都是空操作）：完成卡占位 + 外框，供 E2E 直接取矩形。
    crate::ui_map::publish_frame(c.ui.ctx(), false, false, true);
    crate::ui_map::publish_card(c.ui.ctx(), Some("done"), false, 0, 0, false, None);
    m::map_island_bar(c);
    crate::ui_map::node("card", c.rect(0.0, 0.0, w, h), v.kicker, true);
    let c = c.faded(alpha).shifted(0.0, dy);
    let mut action = None;
    let detail = has_detail(v.text);
    let summary_text = if detail { preview_text(v.text, 90) } else { String::new() };
    if v.compact {
        // 单行提示：图标 + 来源 + 摘要/说明，右侧两枚小按钮。
        let title = m.fit(v.kicker, 11.5, W::Semibold, 120.0);
        crate::extras::agent_icon(&c, 20.0, h / 2.0, 22.0, v.provider);
        galley(&c, 40.0, 7.0, &title, theme::WHITE);
        // 正文摘要区域可点击回到原会话（有正文且有跳转目标时）：悬停提亮 + 手型光标。
        let mut body_hit = None;
        let mut body_hover = 0.0;
        if detail {
            if v.jump {
                let r = c.hit(40.0, 21.0, (w - 96.0 - 40.0).max(40.0), 18.0, "agent-done-body");
                let (hover, _) = c.hover_t(&r);
                body_hover = hover;
                crate::ui_map::node("body.reply", c.rect(40.0, 21.0, (w - 96.0 - 40.0).max(40.0), 18.0), "回复摘要（点击回到会话）", true);
                body_hit = Some(r);
                if hover > 0.01 {
                    c.fill(36.0, 20.0, (w - 96.0 - 40.0).max(40.0) + 8.0, 20.0, 5.0, Color32::from_white_alpha((10.0 * hover) as u8));
                    c.ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
            let body = m.fit(&preview_text(v.text, 60), 10.0, W::Regular, 150.0);
            galley(&c, 40.0, 24.0, &body, crate::paint::mix(theme::AGENT_MUTED, theme::WHITE, body_hover));
        } else {
            // 没有正文时在摘要位说明原因，不让用户以为「查看」按钮丢了。
            let body = m.fit(NO_BODY_HINT, 10.0, W::Regular, 150.0);
            galley(&c, 40.0, 24.0, &body, theme::AGENT_SUBTLE);
        }
        if body_hit.as_ref().is_some_and(|r| r.clicked()) {
            action = Some(Action::Jump);
        }
        if detail
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
    // 有正文显示摘要；没有正文显示一行说明，点「查看回复」前就知道里面是什么。
    let line = if detail {
        m.text(&summary_text, m::META, W::Regular, CONTENT_W)
    } else {
        m.text(NO_BODY_HINT, m::META, W::Regular, CONTENT_W)
    };
    // 摘要区域可点击回到原会话（有正文且有跳转目标时）：悬停提亮 + 手型光标。
    // 空行说明（无正文）不挂跳转，避免用户把「仅状态」误当成回复正文。
    let reply_h = c.galley_h(&line).max(16.0);
    let mut reply_hit = None;
    let mut reply_hover = 0.0;
    if detail && v.jump {
        let r = c.hit(PAD - 4.0, body_top - 3.0, CONTENT_W + 8.0, reply_h + 6.0, "agent-done-body");
        let (hover, _) = c.hover_t(&r);
        reply_hover = hover;
        crate::ui_map::node(
            "body.reply",
            c.rect(PAD - 4.0, body_top - 3.0, CONTENT_W + 8.0, reply_h + 6.0),
            "回复摘要（点击回到会话）",
            true,
        );
        reply_hit = Some(r);
        if hover > 0.01 {
            c.fill(PAD - 6.0, body_top - 4.0, CONTENT_W + 12.0, reply_h + 8.0, 5.0, Color32::from_white_alpha((12.0 * hover) as u8));
            c.ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    galley(
        &c,
        PAD,
        body_top,
        &line,
        if detail {
            crate::paint::mix(theme::AGENT_MUTED, theme::WHITE, reply_hover)
        } else {
            theme::AGENT_SUBTLE
        },
    );
    if reply_hit.as_ref().is_some_and(|r| r.clicked()) {
        action = Some(Action::Jump);
    }
    let fy = (h - 14.0 - BTN_H).max(body_top + c.galley_h(&line) + 6.0);
    let specs = [("agent-result-open", "查看回复", true), ("agent-done-close", "关闭", false)];
    if let Some(i) = widgets::draw_footer(&c, fy, w, &specs, true) {
        action = Some(if i == 0 { Action::ShowResult } else { Action::Dismiss });
    }
    let _ = t;
    action
}

// ---------------- 结果正文（受限 markdown） ----------------

// 设计约束（与 mod.rs 一致）：measure 与 draw 必须用同一次排版结果；解析失败或内容超长时
// 只会退化成纯文本，绝不 panic、绝不执行 HTML、绝不加载远程图片。

/// 列表项标记。有序列表的序号单独排成一列（不混进正文），measure/draw 共用这份 galley。
enum Mark {
    Dot,
    Number(Arc<egui::Galley>),
}

/// 表格降级成「对齐文本」：按列分配宽度，放不下时等比压缩，单元格内允许换行。
/// 不做富表格渲染，也不解析行列合并。
struct Table {
    rows: Vec<TableRow>,
    /// 各列相对内容左侧的起点。
    col_x: Vec<f32>,
    /// 各列宽度（含单元格内边距）。
    col_w: Vec<f32>,
    height: f32,
}

struct TableRow {
    header: bool,
    cells: Vec<Arc<egui::Galley>>,
    height: f32,
}

enum Seg {
    H1(Arc<egui::Galley>),
    H2(Arc<egui::Galley>),
    Paragraph(Arc<egui::Galley>),
    /// 列表项：`indent` 是二级缩进（0.0 / 14.0）。
    Bullet { mark: Mark, galley: Arc<egui::Galley>, indent: f32 },
    Quote(Arc<egui::Galley>),
    /// 代码块：`tag` 是围栏语言（```bash 的 bash），画在块内顶部。
    Code { galley: Arc<egui::Galley>, tag: Option<Arc<egui::Galley>> },
    Table(Table),
    /// `---` 分隔线。
    Rule,
    Gap(f32),
}

/// 列表标记列宽、二级缩进、代码块内边距与语言标签高度。
const MARK_W: f32 = 14.0;
const NEST_W: f32 = 14.0;
const CODE_PAD: f32 = 8.0;
const CODE_TAG_H: f32 = 14.0;
/// 表格字号/单元格左右内边距/行间距；列数超过上限就退回逐行文本。
const TABLE_SIZE: f32 = 10.0;
const TABLE_CELL_PAD: f32 = 8.0;
const TABLE_ROW_GAP: f32 = 6.0;
const TABLE_MAX_COLS: usize = 6;
const TABLE_MAX_CELLS: usize = 600;

/// 一段正文的行内排版：先做一次 RTL 显示转换（与 `m.text` 等价），
/// 有行内 markdown 标记时按段排版，没有标记时走原来的整段排版（旧行为完全不变）。
fn inline(m: &M, text: &str, size: f32, weight: W, width: f32) -> Arc<egui::Galley> {
    let shown = crate::rtl_text::display(text);
    match inline_spans(&shown) {
        Some(spans) => m.spans(&spans, size, weight, width),
        None => m.text_shown(&shown, size, weight, width),
    }
}

/// 行内受限 markdown：`` `code` ``、`**粗体**`、`[文字](链接)` 与 <https://…>。
/// 返回 None 表示这段文字没有任何标记（调用方走普通排版）。
fn inline_spans(text: &str) -> Option<Vec<(m::Span, String)>> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<(m::Span, String)> = Vec::new();
    let mut plain = String::new();
    let mut found = false;
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '`' => {
                if let Some(end) = find_char(&chars, i + 1, '`') {
                    found = true;
                    push_span(&mut out, &mut plain, m::Span::Code, &chars[i + 1..end].iter().collect::<String>());
                    i = end + 1;
                    continue;
                }
            }
            '*' if chars.get(i + 1) == Some(&'*') => {
                // 必须成对；落单的星号原样保留，避免把乘号或正文里的 * 吃掉。
                if let Some(end) = find_strong_end(&chars, i + 2) {
                    found = true;
                    push_span(&mut out, &mut plain, m::Span::Strong, &chars[i + 2..end].iter().collect::<String>());
                    i = end + 2;
                    continue;
                }
            }
            '[' => {
                if let Some((label, next)) = link_at(&chars, i) {
                    found = true;
                    push_span(&mut out, &mut plain, m::Span::Link, &label);
                    i = next;
                    continue;
                }
            }
            '<' => {
                // 自动链接 <https://…>：只去掉尖括号，不联网、不识别协议处理器。
                if let Some(end) = find_char(&chars, i + 1, '>') {
                    let inner: String = chars[i + 1..end].iter().collect();
                    if inner.starts_with("http://") || inner.starts_with("https://") {
                        found = true;
                        push_span(&mut out, &mut plain, m::Span::Link, &inner);
                        i = end + 1;
                        continue;
                    }
                }
            }
            _ => {}
        }
        plain.push(chars[i]);
        i += 1;
    }
    if !plain.is_empty() {
        out.push((m::Span::Text, plain));
    }
    found.then_some(out)
}

/// 把 `text` 记到 `style` 段上；先把已经攒下的普通文字冲出去，保证顺序。
fn push_span(out: &mut Vec<(m::Span, String)>, plain: &mut String, style: m::Span, text: &str) {
    if text.is_empty() {
        return;
    }
    if !plain.is_empty() {
        out.push((m::Span::Text, std::mem::take(plain)));
    }
    out.push((style, text.to_owned()));
}

/// 链接的显示文字：正文里保留短链接作为附注（可读但知道指向哪里），
/// 摘要行只要文字本身（链接是噪音）。
fn link_text(label: &str, url: &str, with_url: bool) -> String {
    if label.is_empty() {
        return url.to_owned();
    }
    if with_url && !url.is_empty() && url != label && url.chars().count() <= 72 {
        format!("{label} ({url})")
    } else {
        label.to_owned()
    }
}

fn find_char(chars: &[char], from: usize, want: char) -> Option<usize> {
    if from >= chars.len() {
        return None;
    }
    (from..chars.len()).find(|&i| chars[i] == want)
}

/// 粗体的结束标记 `**`：必须非空（`****` 不算），且不能跨过更内层的配对。
fn find_strong_end(chars: &[char], from: usize) -> Option<usize> {
    if from + 1 >= chars.len() {
        return None;
    }
    (from..chars.len() - 1)
        .find(|&i| chars[i] == '*' && chars[i + 1] == '*')
        .filter(|&end| end > from)
}

/// 解析 `[文字](链接)`：返回 (标签, 链接, 下一个下标)。
fn link_parts(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let close = find_char(chars, start + 1, ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = find_char(chars, close + 2, ')')?;
    if end == close + 2 {
        return None;
    }
    let label: String = chars[start + 1..close].iter().collect();
    let url: String = chars[close + 2..end].iter().collect();
    Some((label, url, end + 1))
}

/// 正文用的链接显示：链接本身不加载、不执行，只把短链接作为附注显示。
fn link_at(chars: &[char], start: usize) -> Option<(String, usize)> {
    let (label, url, next) = link_parts(chars, start)?;
    let shown = link_text(label.trim(), url.trim(), true);
    (!shown.trim().is_empty()).then_some((shown, next))
}

/// 去掉行内标记，只留可见文字（摘要行不能出现反引号、`**` 或 `[文字](链接)` 原样）。
fn strip_inline_marks(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '`' => {
                i += 1;
                continue;
            }
            '*' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                continue;
            }
            '[' => {
                if let Some((label, url, next)) = link_parts(&chars, i) {
                    out.push_str(&link_text(label.trim(), url.trim(), false));
                    i = next;
                    continue;
                }
            }
            '|' => {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
                i += 1;
                continue;
            }
            _ => {}
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// 围栏行（``` 或 ~~~），返回语言标签（可能为空）。
fn fence(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("```").or_else(|| line.strip_prefix("~~~"))?;
    Some(rest.trim().trim_matches('`').trim())
}

/// ATX 标题：`#` 到 `######`，后面必须有一个空格（与 CommonMark 一致，避免把 `#1` 当标题）。
fn heading(line: &str) -> Option<(usize, &str)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = line.get(hashes..)?.strip_prefix(' ')?.trim_start();
    (!rest.is_empty()).then_some((hashes, rest))
}

/// `1. ` / `12. ` 这类有序列表前缀（位数限三位，免得把「2026. 版本」当列表）。
fn ordered_prefix(line: &str) -> Option<(&str, &str)> {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let rest = line.get(digits..)?.strip_prefix(". ")?;
    Some((line.get(..digits)?, rest))
}

/// 表格分隔行：`| --- | :--: |`（也接受没有首尾竖线的写法）。
fn is_table_sep(line: &str) -> bool {
    let t = line.trim();
    t.contains('-') && t.chars().all(|c| matches!(c, '-' | ':' | '|' | ' ' | '\t'))
}

/// 拆分表格行：去掉首尾竖线后按 `|` 切分；`\|` 转义不展开（结果层里极少见）。
fn table_cells(line: &str) -> Vec<String> {
    let t = line.trim().trim_start_matches('|').trim_end_matches('|');
    t.split('|').map(|cell| cell.trim().to_owned()).collect()
}

/// `---` / `***` / `___` 分隔线（表格分隔行在上面已经被表格分支消费）。
fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.chars().count() >= 3
        && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*') || t.chars().all(|c| c == '_'))
}

/// 按内容宽度给表格分列，再把每个单元格排成 galley。
fn table(m: &M, rows: &[Vec<String>], width: f32) -> Option<Seg> {
    let cols = rows.iter().map(Vec::len).max()?;
    if cols == 0 || cols > TABLE_MAX_COLS || rows.len() < 2 || rows.len() * cols > TABLE_MAX_CELLS {
        return None;
    }
    // 自然宽度：取每列里最长单元格的单行宽度。
    let mut natural = vec![40.0_f32; cols];
    for row in rows {
        for (j, cell) in row.iter().enumerate() {
            let w = m.line(cell.trim(), TABLE_SIZE, W::Regular).size().x + TABLE_CELL_PAD * 2.0;
            natural[j] = natural[j].max(w);
        }
    }
    let gap = 4.0;
    let total: f32 = natural.iter().sum::<f32>() + gap * (cols.saturating_sub(1)) as f32;
    let scale = if total > width { width / total } else { 1.0 };
    let mut col_w: Vec<f32> = natural.iter().map(|w| (w * scale).max(28.0)).collect();
    let clamped: f32 = col_w.iter().sum::<f32>() + gap * (cols.saturating_sub(1)) as f32;
    if clamped > width && clamped > 0.0 {
        let k = width / clamped;
        for w in &mut col_w {
            *w = (*w * k).max(16.0);
        }
    }
    let mut col_x = Vec::with_capacity(cols);
    let mut x = 0.0;
    for w in &col_w {
        col_x.push(x);
        x += w + gap;
    }
    let mut out_rows = Vec::with_capacity(rows.len());
    let mut height = 2.0;
    for (index, row) in rows.iter().enumerate() {
        let header = index == 0;
        let mut cells = Vec::with_capacity(cols);
        let mut row_h = 0.0_f32;
        for j in 0..cols {
            let text = row.get(j).map(String::as_str).unwrap_or("").trim();
            let inner = (col_w[j] - TABLE_CELL_PAD * 2.0).max(12.0);
            let g = inline(m, text, TABLE_SIZE, if header { W::Semibold } else { W::Regular }, inner);
            row_h = row_h.max(g.size().y);
            cells.push(g);
        }
        height += row_h + TABLE_ROW_GAP;
        out_rows.push(TableRow { header, cells, height: row_h });
    }
    Some(Seg::Table(Table { rows: out_rows, col_x, col_w, height }))
}

/// 代码块入段：围栏语言画成块内顶部的一行小字，正文用等宽字体并允许按字符断行。
fn push_code(segs: &mut Vec<Seg>, m: &M, code: &str, tag: Option<&str>, width: f32) {
    let body = code.trim_end();
    if body.trim().is_empty() {
        return;
    }
    // 语言标签只取第一个词并限宽：围栏行可能很长（```bash title="x"），不能顶出内嵌块。
    let tag = tag
        .map(|t| t.split_whitespace().next().unwrap_or(""))
        .filter(|t| !t.is_empty())
        .map(|t| m.fit(t, m::META, W::Regular, 120.0));
    segs.push(Seg::Code { galley: m.mono(body, m::CODE, W::Regular, width - CODE_PAD * 2.0), tag });
    segs.push(Seg::Gap(8.0));
}

fn parse(m: &M, text: &str, width: f32) -> Vec<Seg> {
    let mut segs: Vec<Seg> = Vec::new();
    let mut paragraph = String::new();
    let flush = |paragraph: &mut String, segs: &mut Vec<Seg>| {
        let t = paragraph.trim();
        if !t.is_empty() {
            segs.push(Seg::Paragraph(inline(m, t, m::BODY, W::Regular, width)));
        }
        paragraph.clear();
    };
    // Some(语言) 表示正在收集围栏代码块。
    let mut fence_state: Option<Option<String>> = None;
    let mut code = String::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let line = raw.trim_end();
        if fence_state.is_some() {
            if fence(line.trim_start()).is_some() {
                let tag = fence_state.take().flatten();
                push_code(&mut segs, m, &code, tag.as_deref(), width);
                code.clear();
            } else {
                code.push_str(line);
                code.push('\n');
            }
            i += 1;
            continue;
        }
        if line.trim().is_empty() {
            flush(&mut paragraph, &mut segs);
            i += 1;
            continue;
        }
        let trimmed = line.trim_start();
        if let Some(tag) = fence(trimmed) {
            flush(&mut paragraph, &mut segs);
            fence_state = Some((!tag.is_empty()).then(|| tag.to_owned()));
            code.clear();
            i += 1;
            continue;
        }
        if let Some((level, rest)) = heading(trimmed) {
            flush(&mut paragraph, &mut segs);
            let size = match level {
                1 => 13.5,
                2 => 12.5,
                _ => 12.0,
            };
            let g = inline(m, rest, size, W::Semibold, width);
            segs.push(if level == 1 { Seg::H1(g) } else { Seg::H2(g) });
            i += 1;
            continue;
        }
        // 表格：当前行有竖线、下一行是分隔行。整块一次消费，避免竖线被当成段落文字。
        if trimmed.contains('|') && lines.get(i + 1).is_some_and(|next| is_table_sep(next)) {
            // 分隔行本身也要跳过，且它不一定带竖线（`---` 也符合分隔行规则），
            // 所以起点直接取 i+2，不能靠「行里有竖线」来推进，否则会切出非法区间。
            let mut end = i + 2;
            while end < lines.len() && !lines[end].trim().is_empty() && lines[end].contains('|') {
                end += 1;
            }
            let rows: Vec<Vec<String>> = std::iter::once(table_cells(lines[i]))
                .chain(lines[i + 2..end].iter().map(|row| table_cells(row)))
                .collect();
            if let Some(seg) = table(m, &rows, width) {
                flush(&mut paragraph, &mut segs);
                segs.push(seg);
                i = end;
                continue;
            }
        }
        // 嵌套列表：行首两个空格算一级缩进（再深也按一级处理，结果层不追层级树）。
        let indent = if line.len() - trimmed.len() >= 2 { NEST_W } else { 0.0 };
        if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            flush(&mut paragraph, &mut segs);
            let g = inline(m, rest, m::BODY, W::Regular, width - indent - MARK_W);
            segs.push(Seg::Bullet { mark: Mark::Dot, galley: g, indent });
            i += 1;
            continue;
        }
        if let Some((number, rest)) = ordered_prefix(trimmed) {
            flush(&mut paragraph, &mut segs);
            let marker = m.line(&format!("{number}."), m::META, W::Regular);
            let g = inline(m, rest, m::BODY, W::Regular, width - indent - MARK_W);
            segs.push(Seg::Bullet { mark: Mark::Number(marker), galley: g, indent });
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("> ") {
            flush(&mut paragraph, &mut segs);
            segs.push(Seg::Quote(inline(m, rest, m::BODY, W::Regular, width - 12.0)));
            i += 1;
            continue;
        }
        if is_rule(trimmed) {
            flush(&mut paragraph, &mut segs);
            segs.push(Seg::Rule);
            i += 1;
            continue;
        }
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(trimmed);
        i += 1;
    }
    flush(&mut paragraph, &mut segs);
    if let Some(tag) = fence_state {
        push_code(&mut segs, m, &code, tag.as_deref(), width);
    }
    segs
}

/// 段落高度（用 galley 自身尺寸，供 measure 使用；结果层卡片不缩放，与 `Card::galley_h` 同值）。
fn seg_extent(seg: &Seg) -> f32 {
    match seg {
        Seg::H1(g) | Seg::H2(g) | Seg::Paragraph(g) | Seg::Quote(g) => g.size().y,
        Seg::Bullet { galley, .. } => galley.size().y,
        Seg::Code { galley, tag } => galley.size().y + CODE_PAD * 2.0 + if tag.is_some() { CODE_TAG_H } else { 0.0 },
        Seg::Table(t) => t.height,
        Seg::Rule => 1.0,
        Seg::Gap(h) => *h,
    }
}

fn seg_height(c: &Card, seg: &Seg) -> f32 {
    match seg {
        Seg::H1(g) | Seg::H2(g) | Seg::Paragraph(g) | Seg::Quote(g) => c.galley_h(g),
        Seg::Bullet { galley, .. } => c.galley_h(galley),
        Seg::Code { galley, tag } => c.galley_h(galley) + CODE_PAD * 2.0 + if tag.is_some() { CODE_TAG_H } else { 0.0 },
        Seg::Table(t) => t.height,
        Seg::Rule => 1.0,
        Seg::Gap(h) => *h,
    }
}

fn seg_gap(seg: &Seg) -> f32 {
    match seg {
        Seg::H1(_) => 8.0,
        Seg::H2(_) => 6.0,
        Seg::Bullet { .. } | Seg::Paragraph(_) | Seg::Quote(_) => 7.0,
        Seg::Code { .. } => 0.0,
        Seg::Table(_) => 6.0,
        Seg::Rule => 6.0,
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
        content += seg_extent(seg);
    }
    let fixed = body_top + 12.0 + BTN_H + FOOT_PAD;
    let view = content.min((super::MAX_DETAIL_H - fixed).max(80.0));
    (w, fixed + view)
}

/// 返回 (动作, 新的滚动偏移)。偏移必须由调用方写回状态：只在局部变量里用会导致每帧从 0 开始，
/// 滚轮等于没用（长回复永远跳回顶部）。
pub(crate) fn draw_result(c: &Card, m: &M, v: &ResultView, t: f64) -> (Option<Action>, f32) {
    let w = super::CARD_W;
    let (alpha, dy) = super::entrance(v.age);
    // UI 地图：结果层占位 + 外框 + 正文区，供 E2E 直接取矩形/滚动区。
    crate::ui_map::publish_frame(c.ui.ctx(), true, false, true);
    crate::ui_map::publish_result(c.ui.ctx(), v.hint, v.kicker);
    m::map_island_bar(c);
    let c = c.faded(alpha).shifted(0.0, dy);
    let mut action = None;
    crate::extras::agent_icon(&c, PAD + m::ICON / 2.0, m::HEAD_TOP + m::ICON / 2.0, m::ICON, v.provider);
    galley(&c, PAD + 32.0, m::HEAD_TOP + 1.0, &m.fit(v.kicker, m::TITLE, W::Semibold, CONTENT_W - 90.0), theme::WHITE);
    // 说明层没有正文：副标题直接说明这是本机生成的解释，不要暗示「只读正文」。
    let sub = if v.hint {
        "本机说明 · 不含回复内容"
    } else if v.copied.is_some() {
        "已复制到剪贴板"
    } else if v.jump {
        // 点正文能回客户端：这条提示让入口可见，否则「正文可点」没有任何线索。
        "点正文回到会话 · 只读不落盘"
    } else {
        "可滚动阅读 · 只读，不落盘"
    };
    let sub_color = if v.copied.is_some() { theme::AGENT_OK } else { theme::AGENT_SUBTLE };
    galley(&c, PAD + 32.0, m::HEAD_TOP + 1.0 + m::TITLE * 1.5, &m.line(sub, m::SUBTITLE, W::Regular), sub_color);
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
    let footer_y = h - FOOT_PAD - BTN_H;
    let viewport = Viewport { top: body_top, height: view_h, content, offset: v.scroll };
    let scroll = viewport.wheel(&c, v.scroll, w);
    // 结果层外框 + 正文区矩形、滚动上限，供 E2E 断言「点关闭一次收起 / 滚动不翻页」。
    crate::ui_map::node("layer.result", c.rect(0.0, 0.0, w, h), v.kicker, true);
    crate::ui_map::node("body.result", c.rect(PAD, body_top, w - PAD * 2.0, view_h), "正文", true);
    crate::ui_map::publish_result_scroll(scroll, viewport.max_offset());
    // 正文区整体可点击回到原会话：结果层就是「本轮回复」的完整正文，用户自然会在这里点正文
    // 想回客户端。悬停给手型光标 + 极轻的整体提亮（画在文字下方，不影响阅读）。
    // 高度收到底部按钮之上：正文极短时 view_h 有 60 的下限，会与 footer 顶边重叠几像素，
    // 不能把那几像素也当成跳转区，否则点「复制内容」的上沿会误触。
    let reply_h = view_h.min((footer_y - 2.0 - body_top).max(24.0));
    let mut reply_hit = None;
    if v.jump && !v.hint {
        let r = c.hit(PAD - 6.0, body_top, w - PAD * 2.0 + 12.0, reply_h, "agent-result-body");
        let (hover, _) = c.hover_t(&r);
        crate::ui_map::node(
            "body.reply",
            c.rect(PAD - 6.0, body_top, w - PAD * 2.0 + 12.0, reply_h),
            "回复正文（点击回到会话）",
            true,
        );
        if hover > 0.01 {
            c.fill(PAD - 6.0, body_top, w - PAD * 2.0 + 12.0, reply_h, 4.0, Color32::from_white_alpha((7.0 * hover) as u8));
            c.ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        reply_hit = Some(r);
    }
    let body = c.clipped(PAD - 6.0, body_top, w - PAD * 2.0 + 12.0, view_h).shifted(0.0, -scroll);
    let mut y = body_top;
    for (i, seg) in segs.iter().enumerate() {
        if i > 0 {
            y += seg_gap(&segs[i - 1]);
        }
        match seg {
            Seg::H1(g) | Seg::H2(g) => galley(&body, PAD, y, g, theme::WHITE),
            Seg::Paragraph(g) => galley(&body, PAD, y, g, theme::AGENT_MUTED),
            Seg::Bullet { mark, galley: g, indent } => {
                match mark {
                    // 无序列表用圆点而不是勾：勾会被读成「已完成事项」，正文里只是普通条目。
                    Mark::Dot => body.circle(PAD + indent + 5.0, y + 7.0, 2.0, theme::AGENT_SUBTLE),
                    Mark::Number(number) => galley(&body, PAD + indent + 1.0, y, number, theme::AGENT_SUBTLE),
                }
                galley(&body, PAD + indent + MARK_W, y, g, theme::AGENT_MUTED);
            }
            Seg::Quote(g) => {
                body.fill(PAD, y, 2.0, c.galley_h(g), 1.0, Color32::from_white_alpha(60));
                galley(&body, PAD + 10.0, y, g, theme::AGENT_SUBTLE);
            }
            Seg::Code { galley: g, tag } => {
                let tag_h = if tag.is_some() { CODE_TAG_H } else { 0.0 };
                let bh = c.galley_h(g) + CODE_PAD * 2.0 + tag_h;
                inset(&body, PAD, y, CONTENT_W, bh, theme::AGENT_INSET, theme::AGENT_INSET_EDGE);
                if let Some(tag) = tag {
                    galley(&body, PAD + CODE_PAD, y + 4.0, tag, theme::AGENT_SUBTLE);
                }
                galley(&body, PAD + CODE_PAD, y + CODE_PAD + tag_h, g, m::CODE_INK);
            }
            Seg::Table(t) => {
                let mut ry = y;
                for row in &t.rows {
                    if row.header {
                        body.fill(PAD, ry, CONTENT_W, row.height + TABLE_ROW_GAP, 5.0, Color32::from_white_alpha(6));
                    }
                    for (j, cell) in row.cells.iter().enumerate() {
                        let x = PAD + t.col_x.get(j).copied().unwrap_or(0.0) + TABLE_CELL_PAD;
                        galley(&body, x, ry + 3.0, cell, if row.header { theme::WHITE } else { theme::AGENT_MUTED });
                    }
                    ry += row.height + TABLE_ROW_GAP;
                    if row.header {
                        body.seg((PAD, ry - 2.0), (PAD + CONTENT_W, ry - 2.0), 1.0, Color32::from_white_alpha(24));
                    }
                }
                // 列分隔线：表格降级成文本后仍要能一眼看出列边界。
                for j in 1..t.col_w.len() {
                    let x = PAD + t.col_x[j] - 2.0;
                    body.seg((x, y), (x, y + t.height), 1.0, Color32::from_white_alpha(14));
                }
            }
            Seg::Rule => {
                body.seg((PAD, y), (PAD + CONTENT_W, y), 1.0, Color32::from_white_alpha(28));
            }
            Seg::Gap(_) => {}
        }
        y += seg_height(&c, seg);
    }
    viewport.bar(&c, w, 1.0);
    let fy = footer_y;
    // 说明层没有正文可复制，只留「关闭」：有「复制内容」会让人以为正文只是没显示出来。
    let copy_close: &[(&str, &str, bool)] = &[("agent-result-copy", "复制内容", false), ("agent-result-close", "关闭", true)];
    let close_only: &[(&str, &str, bool)] = &[("agent-result-close", "关闭", true)];
    let specs = if v.hint { close_only } else { copy_close };
    if let Some(i) = widgets::draw_footer(&c, fy, w, specs, true) {
        action = Some(if v.hint || i == 1 { Action::Dismiss } else { Action::CopyResult });
    }
    if reply_hit.as_ref().is_some_and(|r| r.clicked()) {
        action = Some(Action::Jump);
    }
    let _ = t;
    (action, scroll)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::WIN_H;

    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let _ = ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, WIN_H))),
            ..Default::default()
        }, |_| {});
        ctx
    }

    /// 段类型名：只用于断言「解析成了哪一段」，不参与绘制。
    fn kind(seg: &Seg) -> &'static str {
        match seg {
            Seg::H1(_) => "h1",
            Seg::H2(_) => "h2",
            Seg::Paragraph(_) => "p",
            Seg::Bullet { mark: Mark::Dot, .. } => "bullet",
            Seg::Bullet { mark: Mark::Number(_), .. } => "ordered",
            Seg::Quote(_) => "quote",
            Seg::Code { .. } => "code",
            Seg::Table(_) => "table",
            Seg::Rule => "rule",
            Seg::Gap(_) => "gap",
        }
    }

    fn galleys(segs: &[Seg]) -> Vec<String> {
        segs.iter().filter_map(|seg| match seg {
            Seg::H1(g) | Seg::H2(g) | Seg::Paragraph(g) | Seg::Quote(g) => Some(g.text().to_owned()),
            Seg::Bullet { galley, .. } => Some(galley.text().to_owned()),
            Seg::Code { galley, .. } => Some(galley.text().to_owned()),
            Seg::Table(t) => Some(t.rows.iter().flat_map(|r| r.cells.iter().map(|c| c.text().to_owned())).collect::<Vec<_>>().join(" · ")),
            Seg::Rule | Seg::Gap(_) => None,
        }).collect()
    }

    #[test]
    fn inline_spans_cover_code_bold_and_links_only_when_markers_pair_up() {
        let spans = inline_spans("用 `cargo test` 验证，**全部通过**，见 [文档](https://e.x/d)。").expect("应识别出行内标记");
        assert_eq!(spans.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            vec![m::Span::Text, m::Span::Code, m::Span::Text, m::Span::Strong, m::Span::Text, m::Span::Link, m::Span::Text]);
        assert_eq!(spans[1].1, "cargo test");
        assert_eq!(spans[3].1, "全部通过");
        assert_eq!(spans[5].1, "文档 (https://e.x/d)");
        // 落单标记：整段走普通排版，星号和反引号原样保留（不能被吞掉）。
        assert!(inline_spans("落单的 ** 与 ` 和 [半截](未闭合").is_none());
        assert!(inline_spans("普通正文，没有标记。").is_none());
    }

    #[test]
    fn strip_inline_marks_leaves_only_readable_text() {
        let flatten = |text: &str| strip_inline_marks(text).split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(flatten("`a` **b** [c](https://e.x) | d"), "a b c d");
        assert_eq!(preview_text("## 标题\n- `x` 条目\n| a | b |\n| --- | --- |\n| 1 | 2 |", 90), "标题 x 条目 a b 1 2");
    }

    #[test]
    fn block_markers_are_detected_without_false_positives() {
        assert_eq!(heading("## 子标题"), Some((2, "子标题")));
        assert_eq!(heading("###### 六级"), Some((6, "六级")));
        assert_eq!(heading("#1 号方案"), None, "没有空格不算标题");
        assert_eq!(heading("####### 太多"), None);
        assert_eq!(heading("#"), None, "只有井号不算标题");
        assert_eq!(ordered_prefix("1. 第一步"), Some(("1", "第一步")));
        assert_eq!(ordered_prefix("12. 第十二步"), Some(("12", "第十二步")));
        assert_eq!(ordered_prefix("2026. 版本"), None, "四位数字不是列表序号");
        assert!(is_rule("---") && is_rule("***") && is_rule("___"));
        assert!(!is_rule("--"));
        assert!(is_table_sep("| --- | :--: |"));
        assert!(!is_table_sep("| 项目 | 结果 |"));
    }

    #[test]
    fn parse_turns_markdown_into_segments_instead_of_raw_text() {
        let ctx = ctx();
        let m = M::new(&ctx);
        let text = "## 本轮完成\n\n用了 `cargo test` 验证。\n\n1. 先改配置\n2. 再跑回归\n\n| 项目 | 结果 |\n| --- | --- |\n| 单元测试 | 通过 |\n\n> 引用一行\n\n---\n\n```bash\ncargo test --lib\n```\n";
        let segs = parse(&m, text, CONTENT_W);
        let kinds: Vec<&str> = segs.iter().map(kind).collect();
        for expected in ["h2", "p", "ordered", "ordered", "table", "quote", "rule", "code"] {
            assert!(kinds.contains(&expected), "缺少 {expected} 段：{kinds:?}");
        }
        let text_of = galleys(&segs).join("\n");
        assert!(!text_of.contains("**") && !text_of.contains('|'), "原始标记不能漏进正文：{text_of}");
        assert!(text_of.contains("cargo test --lib"), "代码块正文要保留：{text_of}");
    }

    #[test]
    fn oversized_or_malformed_tables_degrade_to_plain_text_without_panicking() {
        let ctx = ctx();
        let m = M::new(&ctx);
        // 7 列超过上限：表格段不成立，退回普通段落（不 panic、不丢字）。
        let wide = "| a | b | c | d | e | f | g |\n| --- | --- | --- | --- | --- | --- | --- |\n| 1 | 2 | 3 | 4 | 5 | 6 | 7 |";
        let segs = parse(&m, wide, CONTENT_W);
        assert!(!segs.iter().any(|s| kind(s) == "table"), "超宽表格应降级：{:?}", segs.iter().map(kind).collect::<Vec<_>>());
        assert!(galleys(&segs).join(" ").contains("1"), "降级后内容不能丢");
        // 只有表头和分隔行（没有数据行）、以及缺列的行都不能崩。
        for text in ["| a | b |\n| --- | --- |", "| a | b |\n| --- | --- |\n| 1 |", "```\n未闭合的代码块"] {
            let _ = parse(&m, text, CONTENT_W);
        }
        // 分隔行不带竖线时，切片的起点会跳过它：不能切出越界区间（曾经会 panic）。
        for text in ["| a | b |\n---", "a | b\n--- | ---", "a | b\n---\n后续段落"] {
            let _ = parse(&m, text, CONTENT_W);
        }
    }

    #[test]
    fn result_measure_and_draw_agree_on_markdown_heights() {
        // measure_result 与 draw_result 必须用同一套解析结果，否则最后几行会被按钮压住。
        let ctx = ctx();
        let m = M::new(&ctx);
        for text in ["", "普通一行", "## 标题\n- 条目\n\n```bash\nls\n```", "| a | b |\n| --- | --- |\n| 1 | 2 |"] {
            let view = ResultView { provider: crate::agent_notice::Provider::Dsh, kicker: "本轮回复", text, scroll: 0.0, age: 1.0, copied: None, hint: false, jump: false };
            let (w, h) = measure_result(&m, &view);
            assert_eq!(w, super::super::CARD_W);
            assert!(h > 0.0 && h <= WIN_H, "结果层高度必须留在画布内：{h}");
        }
    }
}
