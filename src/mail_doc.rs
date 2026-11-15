//! 邮件正文的排版：解析 HTML，按块级结构与内联样式还原成可渲染的文档。
//!
//! 目标是「看起来和原邮件一样」，不是浏览器级别的还原：
//! 支持字号、字重、颜色、斜体/下划线、对齐、段间距、列表、引用、表格与图片，
//! 不支持绝对定位、浮动、动画和外部样式表（邮件本身也几乎不用这些）。

use eframe::egui::Color32;
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

/// 正文默认字号；邮件里显式写了 `font-size` 就按它来。
const BASE_SIZE: f32 = 15.0;
/// 字号上下限：广告邮件会写 40px 的大标题，撑爆阅读窗口。
const MIN_SIZE: f32 = 10.0;
const MAX_SIZE: f32 = 30.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Doc {
    pub blocks: Vec<Block>,
    /// 排版还原置信度（0..100）。低分表示这封邮件的 HTML 里有较多我们渲染不了的特征，
    /// 阅读窗口会提示用户改用「查看完整排版」。
    pub confidence: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Paragraph(Paragraph),
    ListItem(ListItem),
    Image(Image),
    Table(Table),
    Divider,
    Space(f32),
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Paragraph {
    pub runs: Vec<Run>,
    pub align: Align,
    /// 左缩进（逻辑像素）
    pub indent: f32,
    pub before: f32,
    pub after: f32,
    /// 引用块：左侧画一条竖线
    pub quote: bool,
    /// 源邮件里这一块的背景色（表格单元格 / 容器），渲染成淡色底。
    pub bg: Option<Color32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub color: Color32,
    /// `Some` 表示这段文字来自 `<a href>`，点击时直接打开这个地址。
    pub link: Option<std::sync::Arc<str>>,
    /// `<pre>` / `<code>` 用等宽字体，且保留原始换行与缩进。
    pub mono: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    /// `•`、`1.` 或 `a.`；由列表类型和序号决定
    pub marker: String,
    pub level: u8,
    pub para: Paragraph,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    /// `cid:xxx` 是邮件内嵌图，`http(s)://` 是远程图
    pub src: String,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub alt: String,
    pub align: Align,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub rows: Vec<TableRow>,
    /// 有可见边框时才画线；营销邮件的布局表格不画
    pub bordered: bool,
    /// 单元格里再嵌的表（按钮、明细表）。早期版本把它们的行「提」到父表里，
    /// 结果是父单元格的文字和图片被整块丢掉；现在独立成块，阅读顺序不变。
    pub nested: Vec<Table>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableRow {
    pub cells: Vec<TableCell>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableCell {
    pub runs: Vec<Run>,
    /// 单元格里的图片。真实邮件的 logo / hero / 商品图几乎都嵌在表格里，
    /// 早期版本只收文字，导致这些图在岛内直接消失。
    pub images: Vec<Image>,
    pub align: Align,
    pub header: bool,
    pub span: usize,
    pub shade: bool,
    /// 单元格背景色（bgcolor / background-color），渲染成淡色底。
    pub bg: Option<Color32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// 条目与阅读窗口共用：HTML 优先，纯文本兜底，按实际文字量择优。
pub fn render_item(item: &crate::mail::Item) -> Doc {
    render(&item.html, &item.summary)
}

pub fn render(html: &str, text: &str) -> Doc {
    let html = clean(html);
    let text = clean(text);
    let rich = if html.trim().is_empty() { Vec::new() } else { from_html(&html) };
    let plain = if text.trim().is_empty() { Vec::new() } else { from_text(&text) };
    // 只有 HTML 解析出来完全没有文字（纯图片/追踪像素邮件）才退回纯文本。
    // 不能按字数比大小：纯文本版本里常把链接展开成裸 URL，字数反而更多，
    // 按字数择优会丢掉格式和链接，把一封排版邮件渲染成「一行行网址」。
    if (rich.is_empty() || weight(&rich) == 0) && weight(&plain) > 0 {
        Doc { blocks: plain, confidence: 100 }
    } else {
        let confidence = confidence(&html, &rich);
        Doc { blocks: rich, confidence }
    }
}

/// 估算排版还原度：只统计「岛内 egui 渲染确实会丢」的结构。
///
/// 旧版按 `padding` / `<table>` / `<style>` 出现次数机械扣分，导致 NVIDIA 这类
/// 排版其实很好的事务邮件被打到 10 分，提示反而成了噪音。布局表会被摊平成段落、
/// 内联样式会被解析、留白差异几乎看不出来，这些都不该算「还原不了」。
pub fn confidence(html: &str, blocks: &[Block]) -> u8 {
    use crate::mail_tuning as tuning;
    if html.trim().is_empty() { return 100; }
    let lower = html.to_ascii_lowercase();
    let count = |needle: &str| lower.matches(needle).count().min(200) as u32;
    let mut loss: u32 = 0;
    // 真正会走 Grid、把多列对齐压扁的表格才算还原损失。
    let grids = blocks.iter().filter(|b| matches!(b, Block::Table(t)
        if t.rows.iter().map(|r| r.cells.len()).max().unwrap_or(0) > 1)).count() as u32;
    loss += grids.min(tuning::LOSS_COUNT_CAP) * tuning::LOSS_PER_GRID_TABLE;
    loss += count("display:flex").min(tuning::LOSS_COUNT_CAP) * tuning::LOSS_FLEX;
    loss += count("display:grid").min(tuning::LOSS_COUNT_CAP) * tuning::LOSS_GRID;
    loss += count("position:absolute").min(tuning::LOSS_COUNT_CAP) * tuning::LOSS_ABSOLUTE;
    loss += count("position:fixed").min(tuning::LOSS_COUNT_CAP) * tuning::LOSS_FIXED;
    loss += count("background-image").min(tuning::LOSS_COUNT_CAP) * tuning::LOSS_BACKGROUND_IMAGE;
    loss += count("float:").min(tuning::LOSS_COUNT_CAP) * tuning::LOSS_FLOAT;
    loss += count("<iframe").min(tuning::LOSS_RARE_CAP) * tuning::LOSS_IFRAME;
    loss += count("<form").min(tuning::LOSS_RARE_CAP) * tuning::LOSS_FORM;
    let images = blocks.iter().filter(|b| matches!(b, Block::Image(_))).count() as u32;
    if images > tuning::IMAGE_SOFT_LIMIT {
        loss += (images - tuning::IMAGE_SOFT_LIMIT).min(6);
    }
    (100u32
        .saturating_sub(loss.min(tuning::CONFIDENCE_LOSS_CAP)))
    .max(tuning::CONFIDENCE_FLOOR) as u8
}

/// 低置信度时给用户的引导阈值。
pub const CONFIDENCE_HINT: u8 = crate::mail_tuning::CONFIDENCE_HINT;

/// 正文里有没有远程图片（http/https）。没有就不必显示「显示远程图片」按钮。
pub fn has_remote_images(doc: &Doc) -> bool {
    fn remote(image: &Image) -> bool {
        // `//cdn...` 是协议相对地址，同样是外联请求，必须算远程图。
        is_remote_src(&image.src)
    }
    doc.blocks.iter().any(|block| match block {
        Block::Image(image) => remote(image),
        Block::Table(table) => remote_in_table(table),
        _ => false,
    })
}

fn remote_in_table(table: &Table) -> bool {
    let remote = |image: &Image| is_remote_src(&image.src);
    table
        .rows
        .iter()
        .flat_map(|row| row.cells.iter())
        .flat_map(|cell| cell.images.iter())
        .any(remote)
        || table.nested.iter().any(remote_in_table)
}

/// 远程图地址判定：`http(s)://` 与协议相对 `//` 都算外联。
pub fn is_remote_src(src: &str) -> bool {
    let lower = src.trim_start();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("//")
}

fn weight(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|block| match block {
            Block::Paragraph(p) => p.runs.iter().map(|r| r.text.chars().count()).sum::<usize>(),
            Block::ListItem(i) => i.para.runs.iter().map(|r| r.text.chars().count()).sum::<usize>(),
            Block::Table(t) => t
                .rows
                .iter()
                .flat_map(|row| row.cells.iter())
                .map(|cell| cell.runs.iter().map(|r| r.text.chars().count()).sum::<usize>())
                .sum(),
            Block::Image(i) => i.alt.chars().count(),
            _ => 0,
        })
        .sum()
}

/// 零宽空格、零宽连接符、软连字符都不占位，留着只会让断行和复制变得古怪。
fn clean(text: &str) -> std::borrow::Cow<'_, str> {
    let noisy = |c: char| matches!(c, '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}' | '\u{feff}' | '\u{00ad}');
    if text.contains(noisy) {
        std::borrow::Cow::Owned(text.chars().filter(|c| !noisy(*c)).collect())
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

// ---------------------------------------------------------------- 样式

#[derive(Clone, Copy, Debug)]
struct Style {
    size: f32,
    bold: bool,
    italic: bool,
    underline: bool,
    color: Color32,
    align: Align,
    bg: Option<Color32>,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            size: BASE_SIZE,
            bold: false,
            italic: false,
            underline: false,
            color: Color32::from_gray(232),
            align: Align::Left,
            bg: None,
        }
    }
}

impl Style {
    fn run(&self, text: String, link: Option<&std::sync::Arc<str>>) -> Run {
        Run {
            text,
            size: self.size,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            color: self.color,
            link: link.cloned(),
            mono: false,
        }
    }
}

/// 把 `style="a:b;c:d"` 拆成键值对，键统一小写。
fn declarations(style: &str) -> Vec<(String, String)> {
    style
        .split(';')
        .filter_map(|item| {
            let (key, value) = item.split_once(':')?;
            Some((key.trim().to_ascii_lowercase(), value.trim().to_string()))
        })
        .filter(|(key, value)| !key.is_empty() && !value.is_empty())
        .collect()
}

fn apply_style(mut style: Style, css: &str) -> Style {
    for (key, value) in declarations(css) {
        match key.as_str() {
            "font-size" => {
                if let Some(size) = parse_size(&value, style.size) {
                    style.size = size;
                }
            }
            "font-weight" => match value.to_ascii_lowercase().as_str() {
                "bold" | "bolder" => style.bold = true,
                "normal" | "lighter" => style.bold = false,
                other => {
                    if let Ok(weight) = other.parse::<u32>() {
                        style.bold = weight >= 500;
                    }
                }
            },
            "font-style" => match value.to_ascii_lowercase().as_str() {
                "italic" | "oblique" => style.italic = true,
                "normal" => style.italic = false,
                _ => {}
            },
            "text-decoration" | "text-decoration-line" => {
                let lower = value.to_ascii_lowercase();
                if lower.contains("underline") {
                    style.underline = true;
                }
                if lower.contains("none") {
                    style.underline = false;
                }
            }
            "text-align" => {
                style.align = match value.to_ascii_lowercase().as_str() {
                    "center" => Align::Center,
                    "right" | "end" => Align::Right,
                    "left" | "start" | "justify" => Align::Left,
                    _ => style.align,
                };
            }
            "color" => {
                if let Some(color) = parse_color(&value) {
                    style.color = color;
                }
            }
            "background-color" | "background" => {
                // 只认纯色（含简写里的颜色值），图片背景忽略。
                let candidate = value.split_whitespace().find(|part| parse_color(part).is_some());
                if let Some(color) = candidate.and_then(parse_color) {
                    style.bg = Some(color);
                }
            }
            _ => {}
        }
    }
    style
}

fn parse_size(value: &str, parent: f32) -> Option<f32> {
    let value = value.trim().to_ascii_lowercase();
    let number = |suffix: &str| -> Option<f32> { value.strip_suffix(suffix)?.trim().parse::<f32>().ok() };
    let size = if let Some(px) = number("px") {
        px
    } else if let Some(pt) = number("pt") {
        pt * 4.0 / 3.0
    } else if let Some(em) = number("em") {
        em * parent
    } else if let Some(rem) = number("rem") {
        rem * BASE_SIZE
    } else if let Some(percent) = number("%") {
        percent / 100.0 * parent
    } else if value == "small" {
        parent * 0.85
    } else if value == "large" {
        parent * 1.2
    } else if value == "x-large" {
        parent * 1.4
    } else {
        parent
    };
    Some(size.clamp(MIN_SIZE, MAX_SIZE))
}

fn parse_color(value: &str) -> Option<Color32> {
    let value = value.trim().to_ascii_lowercase();
    if let Some(hex) = value.strip_prefix('#') {
        let digits: Vec<u8> = hex.chars().filter_map(|c| c.to_digit(16).map(|d| d as u8)).collect();
        return match digits.len() {
            3 => Some(Color32::from_rgb(digits[0] * 17, digits[1] * 17, digits[2] * 17)),
            6 => Some(Color32::from_rgb(
                digits[0] * 16 + digits[1],
                digits[2] * 16 + digits[3],
                digits[4] * 16 + digits[5],
            )),
            _ => None,
        };
    }
    if let Some(rest) = value.strip_prefix("rgb") {
        if let Some(inner) = rest.split_once('(').map(|(_, tail)| tail.trim_end_matches(')')) {
            let parts: Vec<u8> = inner
                .split(',')
                .filter_map(|part| part.trim().parse::<f32>().ok())
                .map(|v| v.clamp(0.0, 255.0) as u8)
                .collect();
            if parts.len() >= 3 {
                return Some(Color32::from_rgb(parts[0], parts[1], parts[2]));
            }
        }
    }
    let named = match value.as_str() {
        "black" => Color32::from_rgb(0, 0, 0),
        "white" => Color32::WHITE,
        "gray" | "grey" => Color32::from_gray(128),
        "red" => Color32::from_rgb(200, 40, 40),
        "blue" => Color32::from_rgb(30, 100, 200),
        "green" => Color32::from_rgb(30, 140, 70),
        "orange" => Color32::from_rgb(220, 130, 40),
        "silver" => Color32::from_gray(192),
        _ => return None,
    };
    Some(named)
}

// ---------------------------------------------------------------- 样式表

/// 邮件 `<style>` 里的一条规则：简单选择器（tag/.class/#id 组合）+ 声明。
/// 只做邮件里真正会用到的子集；带伪类、属性选择器、组合符的规则直接跳过，
/// 交给内联样式兜底，避免半吊子解析产生奇怪结果。
#[derive(Clone)]
struct Rule {
    tag: Option<String>,
    classes: Vec<String>,
    id: Option<String>,
    specificity: u32,
    declarations: Vec<(String, String)>,
}

#[derive(Default)]
struct StyleSheet {
    rules: Vec<Rule>,
}

impl StyleSheet {
    fn apply(&self, tag: &str, attrs: &[(String, String)], mut style: Style) -> Style {
        let mut class_attr = "";
        let mut id_attr = "";
        for (key, value) in attrs {
            match key.as_str() {
                "class" => class_attr = value,
                "id" => id_attr = value,
                _ => {}
            }
        }
        let classes: Vec<&str> = class_attr.split_whitespace().collect();
        let mut matched: Vec<&Rule> = self
            .rules
            .iter()
            .filter(|rule| {
                rule.tag.as_deref().is_none_or(|want| want == tag)
                    && rule.id.as_deref().is_none_or(|want| want == id_attr)
                    && rule.classes.iter().all(|want| classes.iter().any(|have| have == want))
            })
            .collect();
        matched.sort_by_key(|rule| rule.specificity);
        for rule in matched {
            let css: String = rule
                .declarations
                .iter()
                .map(|(key, value)| format!("{key}:{value}"))
                .collect::<Vec<_>>()
                .join(";");
            style = apply_style(style, &css);
        }
        style
    }
}

/// 去掉注释、@media/@supports 等块，再解析 `选择器 { 声明 }`。
fn parse_stylesheet(css: &str) -> StyleSheet {
    let mut sheet = StyleSheet::default();
    let mut text = String::with_capacity(css.len());
    let mut rest = css;
    // 去注释
    while let Some(start) = rest.find("/*") {
        text.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => { rest = ""; }
        }
    }
    text.push_str(rest);
    // 去掉 @ 规则（@media/@font-face 等），保留其余
    let mut cleaned = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'@' {
            // 跳到块结束（处理嵌套大括号）
            let mut depth = 0i32;
            let mut cursor = index;
            while cursor < bytes.len() {
                match bytes[cursor] {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth <= 0 { cursor += 1; break; }
                    }
                    _ => {}
                }
                cursor += 1;
            }
            index = cursor;
            continue;
        }
        cleaned.push(bytes[index] as char);
        index += 1;
    }
    let mut rest = cleaned.as_str();
    while let Some(open) = rest.find('{') {
        let selector_text = rest[..open].trim();
        let Some(close) = rest[open..].find('}') else { break };
        let body = &rest[open + 1..open + close];
        rest = &rest[open + close + 1..];
        let declarations = declarations(body);
        if declarations.is_empty() {
            continue;
        }
        for selector in selector_text.split(',') {
            let selector = selector.trim();
            if selector.is_empty()
                || selector.contains([':', '[', ']', '>', '+', '~', '*'])
                || selector.split_whitespace().count() != 1
            {
                continue;
            }
            let mut tag = String::new();
            let mut classes = Vec::new();
            let mut id = None;
            let mut current = String::new();
            let mut kind = 't';
            for ch in selector.chars() {
                match ch {
                    '.' | '#' => {
                        if !current.is_empty() {
                            match kind {
                                '#' => id = Some(std::mem::take(&mut current)),
                                '.' => classes.push(std::mem::take(&mut current)),
                                _ => tag = std::mem::take(&mut current),
                            }
                        }
                        kind = if ch == '.' { '.' } else { '#' };
                    }
                    _ => current.push(ch),
                }
            }
            if !current.is_empty() {
                match kind {
                    '#' => id = Some(current),
                    '.' => classes.push(current),
                    _ => tag = current,
                }
            }
            let specificity = id.is_some() as u32 * 100
                + classes.len() as u32 * 10
                + (!tag.is_empty()) as u32;
            sheet.rules.push(Rule {
                tag: if tag.is_empty() { None } else { Some(tag.to_ascii_lowercase()) },
                classes,
                id,
                specificity,
                declarations: declarations.clone(),
            });
        }
    }
    sheet
}

fn collect_style_text(node: &Handle, out: &mut String) {
    for child in node.children.borrow().iter() {
        if let NodeData::Element { name, .. } = &child.data {
            let tag = name.local.to_string().to_ascii_lowercase();
            if tag == "style" {
                for text in child.children.borrow().iter() {
                    if let NodeData::Text { contents } = &text.data {
                        out.push_str(&contents.borrow());
                    }
                }
            } else {
                collect_style_text(child, out);
            }
        }
    }
}

// ---------------------------------------------------------------- 构建

struct Build {
    blocks: Vec<Block>,
    /// 当前行内缓冲
    runs: Vec<Run>,
    /// 当前段落的额外属性
    align: Align,
    indent: f32,
    quote: bool,
    before: f32,
    after: f32,
    /// 列表上下文：(是否有序, 当前序号)
    lists: Vec<(bool, usize)>,
    /// 正在等待写入的列表项标记
    pending_marker: Option<(String, u8)>,
    /// 最近的块级间距（用于避免重复空行）
    last_gap: bool,
    /// 正在处理 <pre>：空白与换行都要原样保留
    pre: bool,
    /// 当前块的背景色
    bg: Option<Color32>,
    /// 内联按钮（`<a style="background-color:…">`）的底色。
    /// 只有整段就是这个短链接时才用得上，否则会把普通链接染成一块色条。
    link_bg: Option<Color32>,
}

impl Build {
    fn new() -> Self {
        Self {
            blocks: Vec::new(),
            runs: Vec::new(),
            align: Align::Left,
            indent: 0.0,
            quote: false,
            before: 0.0,
            after: 8.0,
            lists: Vec::new(),
            pending_marker: None,
            last_gap: true,
            pre: false,
            bg: None,
            link_bg: None,
        }
    }

    fn flush(&mut self, style: Style) {
        if self.runs.iter().all(|r| r.text.trim().is_empty()) {
            self.runs.clear();
            self.pending_marker = None;
            self.link_bg = None;
            return;
        }
        // 独立短链接（按钮）优先用它自己的底色：`<a style="background:#0b57d0">查看活动</a>`
        // 是最常见的写法，格子本身没有底色。
        let bg = self.bg.or_else(|| {
            if short_standalone_link(&self.runs) {
                self.link_bg.and_then(visible_bg)
            } else {
                None
            }
        });
        let para = Paragraph {
            runs: std::mem::take(&mut self.runs),
            align: self.align,
            indent: self.indent,
            before: self.before,
            after: self.after,
            quote: self.quote,
            bg,
        };
        if let Some((marker, level)) = self.pending_marker.take() {
            self.blocks.push(Block::ListItem(ListItem { marker, level, para }));
        } else {
            self.blocks.push(Block::Paragraph(para));
        }
        self.align = style.align;
        self.bg = None;
        self.link_bg = None;
        self.before = 0.0;
        self.after = 8.0;
        self.last_gap = false;
    }

    fn push_space(&mut self, height: f32) {
        if height <= 0.0 || self.last_gap {
            return;
        }
        self.blocks.push(Block::Space(height));
        self.last_gap = true;
    }
}

fn from_html(html: &str) -> Vec<Block> {
    let dom = html5ever::parse_document(RcDom::default(), Default::default())
        .from_utf8()
        .read_from(&mut html.as_bytes());
    let Ok(dom) = dom else {
        return from_text(&strip_tags(html));
    };
    let mut css = String::new();
    collect_style_text(&dom.document, &mut css);
    let sheet = parse_stylesheet(&css);
    let mut build = Build::new();
    walk(&dom.document, Style::default(), &None, &sheet, &mut build);
    build.flush(Style::default());
    trim(&mut build.blocks);
    build.blocks
}

fn block_name(tag: &str) -> bool {
    matches!(
        tag,
        "div"
            | "p"
            | "article"
            | "section"
            | "header"
            | "footer"
            | "main"
            | "aside"
            | "nav"
            | "center"
            | "address"
            | "figure"
            | "figcaption"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "ul"
            | "ol"
            | "li"
            | "table"
            | "blockquote"
            | "pre"
            | "hr"
    )
}

fn walk(node: &Handle, style: Style, link: &Option<std::sync::Arc<str>>, sheet: &StyleSheet, build: &mut Build) {
    match &node.data {
        NodeData::Document => {
            for child in node.children.borrow().iter() {
                walk(child, style, link, sheet, build);
            }
        }
        NodeData::Text { contents } => {
            let raw = contents.borrow().to_string();
            let text = decode_entities(&raw);
            if build.pre {
                // <pre> 里换行和缩进是内容的一部分，不能折叠。
                let text = text.replace("\r\n", "\n").replace('\r', "\n");
                if !text.is_empty() {
                    build.runs.push(style.run(text, link.as_ref()));
                }
                return;
            }
            let text = text.replace(['\r', '\n', '\t'], " ");
            // HTML 里连续空白会折叠成一个空格，这里保持一致。
            let mut spaced = String::with_capacity(text.len());
            let mut last_space = false;
            for ch in text.chars() {
                if ch.is_whitespace() {
                    if !last_space {
                        spaced.push(' ');
                    }
                    last_space = true;
                } else {
                    spaced.push(ch);
                    last_space = false;
                }
            }
            if build.runs.is_empty() && spaced.trim().is_empty() {
                return;
            }
            build.runs.push(style.run(spaced, link.as_ref()));
        }
        NodeData::Element { name, attrs, .. } => {
            let tag = name.local.to_string().to_ascii_lowercase();
            let attr = |key: &str| -> Option<String> {
                attrs
                    .borrow()
                    .iter()
                    .find(|a| a.name.local.to_string().eq_ignore_ascii_case(key))
                    .map(|a| a.value.to_string())
            };

            let attr_pairs: Vec<(String, String)> = attrs
                .borrow()
                .iter()
                .map(|a| (a.name.local.to_string().to_ascii_lowercase(), a.value.to_string()))
                .collect();
            let mut style = sheet.apply(&tag, &attr_pairs, style);
            if let Some(css) = attr("style") {
                style = apply_style(style, &css);
            }
            // 老邮件用 <font> 标签
            if tag == "font" {
                if let Some(size) = attr("size").and_then(|v| v.parse::<i32>().ok()) {
                    style.size = (BASE_SIZE + (size - 3) as f32 * 2.0).clamp(MIN_SIZE, MAX_SIZE);
                }
                if let Some(color) = attr("color").and_then(|v| parse_color(&v)) {
                    style.color = color;
                }
            }
            if tag == "b" || tag == "strong" || tag == "th" {
                style.bold = true;
            }
            if tag == "i" || tag == "em" {
                style.italic = true;
            }
            if tag == "u" || tag == "a" {
                style.underline = true;
            }
            // <a href> 的地址随子树往下传，正文里的链接就能直接点开。
            let mut link = link.clone();
            if tag == "a" {
                if let Some(href) = attr("href") {
                    let href = href.trim().to_string();
                    if href.starts_with("http://") || href.starts_with("https://") || href.starts_with("mailto:") {
                        link = Some(std::sync::Arc::from(href.as_str()));
                    }
                }
                // 按钮常把底色写在链接自己身上，留给 flush 判断「整段是否就是这个按钮」。
                if style.bg.is_some() {
                    build.link_bg = style.bg;
                }
            }
            // table 的 align 只决定表格自身摆在哪，不会把内容居中；
            // 其他元素（td/p/div）的 align 才继承给文本。
            if tag != "table" {
                if let Some(align) = attr("align") {
                    style.align = match align.to_ascii_lowercase().as_str() {
                        "center" => Align::Center,
                        "right" => Align::Right,
                        "left" => Align::Left,
                        _ => style.align,
                    };
                }
            }
            if matches!(tag.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
                style.bold = true;
                let scale = match tag.as_str() {
                    "h1" => 1.7,
                    "h2" => 1.45,
                    "h3" => 1.25,
                    "h4" => 1.12,
                    _ => 1.0,
                };
                style.size = (BASE_SIZE * scale).clamp(MIN_SIZE, MAX_SIZE);
            }

            match tag.as_str() {
                "script" | "style" | "head" | "title" | "meta" | "link" => return,
                "br" => {
                    build.runs.push(style.run("\n".into(), None));
                    return;
                }
                "hr" => {
                    build.flush(style);
                    build.push_space(10.0);
                    build.blocks.push(Block::Divider);
                    build.push_space(10.0);
                    return;
                }
                "img" => {
                    let src = attr("src").unwrap_or_default();
                    if src.is_empty() {
                        return;
                    }
                    let width = attr("width").and_then(|v| v.trim_end_matches("px").trim().parse::<f32>().ok());
                    let height = attr("height").and_then(|v| v.trim_end_matches("px").trim().parse::<f32>().ok());
                    // 追踪像素：任一边 ≤2px 就不必占位。真图不会声明这么小的尺寸，
                    // 放过它们会在岛内正文里排成一整片小方框（截图里的主要噪点）。
                    if width.is_some_and(|w| w <= crate::mail_tuning::TRACKING_PIXEL_MAX_PX)
                        || height.is_some_and(|h| h <= crate::mail_tuning::TRACKING_PIXEL_MAX_PX)
                    {
                        return;
                    }
                    build.flush(style);
                    build.blocks.push(Block::Image(Image {
                        src,
                        width,
                        height,
                        alt: attr("alt").unwrap_or_default().trim().to_string(),
                        align: style.align,
                    }));
                    build.last_gap = false;
                    return;
                }
                "ul" | "ol" => {
                    build.flush(style);
                    build.push_space(4.0);
                    build.lists.push((tag == "ol", 0));
                    for child in node.children.borrow().iter() {
                        walk(child, style, &link, sheet, build);
                    }
                    build.flush(style);
                    build.lists.pop();
                    build.push_space(4.0);
                    return;
                }
                "li" => {
                    build.flush(style);
                    let level = build.lists.len().saturating_sub(1).min(3) as u8;
                    let marker = match build.lists.last_mut() {
                        Some((true, index)) => {
                            *index += 1;
                            format!("{}. ", index)
                        }
                        _ => "•  ".to_string(),
                    };
                    build.pending_marker = Some((marker, level));
                    build.indent = 14.0 + level as f32 * 14.0;
                    for child in node.children.borrow().iter() {
                        walk(child, style, &link, sheet, build);
                    }
                    build.flush(style);
                    build.indent = 0.0;
                    return;
                }
                "pre" => {
                    build.flush(style);
                    build.push_space(4.0);
                    build.pre = true;
                    for child in node.children.borrow().iter() {
                        walk(child, style, &link, sheet, build);
                    }
                    build.pre = false;
                    // 整段标记为等宽字体。
                    for run in build.runs.iter_mut() {
                        run.mono = true;
                    }
                    build.flush(style);
                    build.push_space(4.0);
                    return;
                }
                "blockquote" => {
                    build.flush(style);
                    build.push_space(6.0);
                    build.quote = true;
                    build.indent = 12.0;
                    for child in node.children.borrow().iter() {
                        walk(child, style, &link, sheet, build);
                    }
                    build.flush(style);
                    build.quote = false;
                    build.indent = 0.0;
                    build.push_space(6.0);
                    return;
                }
                "table" => {
                    build.flush(style);
                    build.push_space(6.0);
                    // 按钮底色常写在 `<table bgcolor="#76b900">` 上，要往下传给单元格。
                    let mut table_style = style;
                    for (key, value) in node_attrs(node) {
                        if key.eq_ignore_ascii_case("bgcolor") {
                            if let Some(color) = parse_color(&value) {
                                table_style.bg = Some(color);
                            }
                        }
                    }
                    if let Some(table) = collect_table(node, table_style, sheet) {
                        // 邮件 HTML 的表格有两种用途：有边框的数据表格（对账单、参数表）
                        // 和无边框的布局表格（几乎所有营销/通知邮件的骨架）。
                        // 数据表格保留行列结构；布局表格按 DOM 顺序展开——单元格里的
                        // 文字、图片、按钮、嵌套表都回到原来的位置，标题不会跑到页脚后面，
                        // 页脚的四行也不会被拼成一段。嵌套表按同样规则递归处理。
                        if is_data_table(&table) {
                            emit_table(&mut build.blocks, table);
                        } else {
                            layout_table(node, table_style, sheet, build);
                        }
                        build.last_gap = false;
                    }
                    build.push_space(6.0);
                    return;
                }
                _ => {}
            }

            let is_block = block_name(&tag);
            if is_block {
                build.flush(style);
                // text-align 是继承属性，块级元素开始时把它带给后面的段落。
                build.align = style.align;
            }
            for child in node.children.borrow().iter() {
                walk(child, style, &link, sheet, build);
            }
            if is_block {
                build.flush(style);
            }
        }
        _ => {}
    }
}

/// 真正画得出来的边框才算数。邮件里 `border-collapse:collapse`、
/// `border-radius:6px`、`border:none` 都是布局骨架的外观属性，
/// 若把它们当成「有边框数据表」，整封邮件会被塞进 Grid 压成窄列。
fn has_visible_border(style: &str) -> bool {
    for (key, value) in declarations(style) {
        if !key.starts_with("border") {
            continue;
        }
        // 这些只是外观/合并方式，不代表看得见的边框线。
        if matches!(
            key.as_str(),
            "border-radius" | "border-collapse" | "border-spacing" | "border-image" | "border-style"
        ) {
            continue;
        }
        let value = value.to_ascii_lowercase();
        if value.is_empty() || value == "none" || value == "transparent" {
            continue;
        }
        // `border:1px solid #ddd` / `border-top:2px dashed …` 才算。
        let width = value.split_whitespace().find_map(|part| {
            part.trim_end_matches("px").parse::<f32>().ok()
        });
        if width.is_some_and(|w| w > 0.0)
            || value.contains("solid")
            || value.contains("dashed")
            || value.contains("dotted")
        {
            return true;
        }
    }
    false
}

/// 表格单独收集：行、单元格、表头、跨列与对齐。
fn collect_table(node: &Handle, mut style: Style, sheet: &StyleSheet) -> Option<Table> {
    let mut rows = Vec::new();
    let mut nested = Vec::new();
    let mut bordered = false;
    for (key, value) in node_attrs(node) {
        if key.eq_ignore_ascii_case("border") && value.trim().parse::<u32>().is_ok_and(|n| n > 0) {
            bordered = true;
        }
        if key.eq_ignore_ascii_case("style") && has_visible_border(&value) {
            bordered = true;
        }
        // 按钮/卡片常把底色写在 `<table bgcolor>` 上，往下继承给单元格。
        if key.eq_ignore_ascii_case("bgcolor") {
            if let Some(color) = parse_color(&value) {
                style.bg = Some(color);
            }
        }
    }
    collect_rows(node, style, sheet, &mut rows, &mut nested);
    rows.retain(|row: &TableRow| !row.cells.is_empty());
    if rows.is_empty() && nested.is_empty() {
        return None;
    }
    Some(Table { rows, bordered, nested })
}

/// 把一张表按「数据表 / 布局表」分别落到块结构，再递归处理它内部嵌套的表。
fn emit_table(blocks: &mut Vec<Block>, mut table: Table) {
    let nested = std::mem::take(&mut table.nested);
    if !table.rows.is_empty() {
        if is_data_table(&table) {
            blocks.push(Block::Table(table));
        } else {
            blocks.extend(flatten_table(table));
        }
    }
    for child in nested {
        emit_table(blocks, child);
    }
}

/// 只有「列少、每格都是短标签」的表才值得画成网格。
/// 有边框只是加分项，不能单独决定：腾讯云/GitHub 这类外层骨架表
/// 也带 `border-radius` 或 `border-collapse`，但内容是一整封正文。
fn is_data_table(table: &Table) -> bool {
    // 单元格里带图的表不可能是「数据表」：那是图片卡片/商品卡。
    // 按网格画会留出一整列空白，还把图挤到一侧（苹果广告邮件那种布局）。
    if table.rows.iter().any(|row| row.cells.iter().any(|cell| !cell.images.is_empty())) {
        return false;
    }
    // 全空的表格（外层包裹器、纯间距行）画成网格就是正文里一个空框。
    let has_content = table.rows.iter().any(|row| {
        row.cells.iter().any(|cell| {
            !cell.images.is_empty() || cell.runs.iter().any(|run| !run.text.trim().is_empty())
        })
    });
    if !has_content {
        return false;
    }
    let columns = table
        .rows
        .iter()
        .map(|row| row.cells.iter().map(|cell| cell.span).sum::<usize>())
        .max()
        .unwrap_or(0);
    if columns > crate::mail_tuning::MAX_DATA_COLUMNS
        || table.rows.len() < crate::mail_tuning::MIN_DATA_ROWS
    {
        return false;
    }
    let longest = table
        .rows
        .iter()
        .flat_map(|row| row.cells.iter())
        .map(|cell| cell_len(&cell.runs))
        .max()
        .unwrap_or(0);
    // 布局表格的单元格里常塞整段文章；数据表格每格只有几个词。
    // 这里不再因为 `bordered` 直接放行：外层骨架表同样会写 border 相关样式。
    longest <= crate::mail_tuning::MAX_DATA_CELL_CHARS
}

// ---------------------------------------------------------------------------
// 布局表格：按 DOM 顺序展开
// ---------------------------------------------------------------------------

/// 布局表格不画网格，也不「先收文字再补图」，而是逐行逐格地重走 `walk`。
///
/// 摊平版（`flatten_table`）会丢掉单元格内部的块级结构：一个 `<td>` 里
/// 如果排了标题、正文、按钮、页脚四个 `<div>`，它们会被拼成一段；单元格里的
/// 表格则要等整张表收完才补在后面，于是「页脚跑到标题前面」这种错位就出现了。
/// 直接递归原 DOM 可以同时解决顺序、段落切分和嵌套表位置三个问题。
fn layout_table(node: &Handle, style: Style, sheet: &StyleSheet, build: &mut Build) {
    let mut rows: Vec<Vec<(Handle, Style)>> = Vec::new();
    collect_layout_rows(node, style, &mut rows);
    for row in rows {
        if row.is_empty() {
            continue;
        }
        // 一行里全是「纯文字短格」就并成同一行（导航条、`项目 / 金额` 明细行）；
        // 只要有一格带块级结构或图片，就交给逐格展开，顺序才不会乱。
        let join = row.len() <= crate::mail_tuning::SHORT_LABEL_CELLS
            && row.iter().all(|(cell, cell_style)| {
                inline_only(cell)
                    && cell_len(&collect_runs_own(cell, *cell_style, sheet))
                        <= crate::mail_tuning::SHORT_LABEL_CHARS
            });
        if join {
            let align = row[0].1.align;
            let mut runs: Vec<Run> = Vec::new();
            let mut bg = None;
            for (index, (cell, cell_style)) in row.iter().enumerate() {
                if index > 0 {
                    runs.push(Run {
                        text: "  ".into(),
                        size: BASE_SIZE,
                        bold: false,
                        italic: false,
                        underline: false,
                        color: Color32::from_gray(232),
                        link: None,
                        mono: false,
                    });
                }
                runs.extend(collect_runs_own(cell, *cell_style, sheet));
                bg = bg.or(cell_style.bg.and_then(visible_bg));
            }
            if runs.iter().any(|run| !run.text.trim().is_empty()) {
                build.flush(style);
                build.blocks.push(Block::Paragraph(Paragraph {
                    runs,
                    align,
                    indent: 0.0,
                    before: 0.0,
                    after: 8.0,
                    quote: false,
                    bg,
                }));
            }
            continue;
        }
        for (cell, cell_style) in row {
            walk_cell(&cell, cell_style, sheet, build);
        }
    }
}

/// 把一个单元格当成独立的排版单元：进入前收掉上一格的残留，出来时一定 flush，
/// 这样每个单元格的段落边界和原文一致，不会被邻格粘在一起。
fn walk_cell(cell: &Handle, cell_style: Style, sheet: &StyleSheet, build: &mut Build) {
    build.flush(cell_style);
    build.align = cell_style.align;
    build.bg = cell_style.bg.and_then(visible_bg);
    let start = build.blocks.len();
    for child in cell.children.borrow().iter() {
        walk(child, cell_style, &None, sheet, build);
    }
    build.flush(cell_style);
    build.bg = None;
    // 整格就是一个短链接时（`<a style="background:#1a73e8">查看活动</a>`），
    // 把链接自己的底色补回段落上，阅读窗口才会画成按钮而不是一行下划线文字。
    if build.blocks.len() == start + 1 {
        if let Block::Paragraph(para) = &mut build.blocks[start] {
            if para.bg.is_none() && short_standalone_link(&para.runs) {
                para.bg = cta_background(cell, 0).and_then(visible_bg);
            }
        }
    }
}

/// 整段就是一个短链接（按钮的典型写法）。
fn short_standalone_link(runs: &[Run]) -> bool {
    let visible: Vec<&Run> = runs.iter().filter(|run| !run.text.trim().is_empty()).collect();
    if visible.is_empty() {
        return false;
    }
    let text: String = visible.iter().map(|run| run.text.as_str()).collect();
    !text.contains('\n')
        && text.trim().chars().count() <= crate::mail_tuning::MAX_DATA_CELL_CHARS - 2
        && visible.iter().all(|run| run.link.is_some())
}

/// 单元格里只有行内标签（文字、链接、加粗、换行）时才允许和同行的其他格并排。
/// 出现图片、嵌套表、`div/p` 这类块级结构就必须逐格展开，否则顺序会乱。
fn inline_only(node: &Handle) -> bool {
    fn check(node: &Handle, depth: usize) -> bool {
        if depth > 12 {
            return false;
        }
        for child in node.children.borrow().iter() {
            let NodeData::Element { name, .. } = &child.data else { continue };
            let tag = name.local.to_string().to_ascii_lowercase();
            if matches!(
                tag.as_str(),
                "table"
                    | "tr"
                    | "td"
                    | "th"
                    | "tbody"
                    | "thead"
                    | "tfoot"
                    | "img"
                    | "div"
                    | "p"
                    | "ul"
                    | "ol"
                    | "li"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "pre"
                    | "blockquote"
                    | "hr"
            ) {
                return false;
            }
            if !check(child, depth + 1) {
                return false;
            }
        }
        true
    }
    check(node, 0)
}

/// 按文档顺序收集布局表的行与单元格（`thead/tbody/tfoot` 只是包裹层）。
fn collect_layout_rows(
    node: &Handle,
    style: Style,
    out: &mut Vec<Vec<(Handle, Style)>>,
) {
    for child in node.children.borrow().iter() {
        let NodeData::Element { name, .. } = &child.data else { continue };
        let tag = name.local.to_string().to_ascii_lowercase();
        if tag == "tr" {
            let mut cells = Vec::new();
            for cell in child.children.borrow().iter() {
                let NodeData::Element { name, attrs, .. } = &cell.data else { continue };
                let cell_tag = name.local.to_string().to_ascii_lowercase();
                if cell_tag != "td" && cell_tag != "th" {
                    continue;
                }
                let mut cell_style = style;
                for attr in attrs.borrow().iter() {
                    let key = attr.name.local.to_string().to_ascii_lowercase();
                    let value = attr.value.to_string();
                    match key.as_str() {
                        "style" => cell_style = apply_style(cell_style, &value),
                        "align" => {
                            cell_style.align = match value.to_ascii_lowercase().as_str() {
                                "center" => Align::Center,
                                "right" => Align::Right,
                                _ => cell_style.align,
                            }
                        }
                        "bgcolor" => {
                            if let Some(color) = parse_color(&value) {
                                cell_style.bg = Some(color);
                            }
                        }
                        _ => {}
                    }
                }
                if cell_tag == "th" {
                    cell_style.bold = true;
                }
                cells.push((cell.clone(), cell_style));
            }
            if !cells.is_empty() {
                out.push(cells);
            }
            continue;
        }
        // 不要下钻到嵌套的 `<table>`：它属于某个单元格，由 `walk_cell` 处理。
        if tag == "table" {
            continue;
        }
        collect_layout_rows(child, style, out);
    }
}

/// 布局表格按阅读顺序摊平：
/// - 单格行、或「短标签行」（≤4 格且每格 ≤12 字）按行合并，读起来像一行文字；
/// - 其余一行多格（多列布局）逐格成段，保留各自的颜色与对齐。
fn flatten_table(table: Table) -> Vec<Block> {
    let mut blocks = Vec::new();
    for row in table.rows {
        let cells: Vec<&TableCell> = row
            .cells
            .iter()
            .filter(|cell| cell_len(&cell.runs) > 0 || !cell.images.is_empty())
            .collect();
        if cells.is_empty() {
            continue;
        }
        let short = cells.len() <= crate::mail_tuning::SHORT_LABEL_CELLS
            && cells.iter().all(|cell| cell_len(&cell.runs) <= crate::mail_tuning::SHORT_LABEL_CHARS);
        if cells.len() == 1 || short {
            let align = cells[0].align;
            // 取第一个「真正有语义」的底色，别被外层骨架继承的白底挡住 CTA 颜色。
            let bg = cells.iter().find_map(|cell| cell.bg.and_then(visible_bg));
            let mut runs: Vec<Run> = Vec::new();
            for (index, cell) in cells.iter().enumerate() {
                if index > 0 {
                    runs.push(Run {
                        text: "  ".into(),
                        size: BASE_SIZE,
                        bold: false,
                        italic: false,
                        underline: false,
                        color: Color32::from_gray(232),
                        link: None,
                        mono: false,
                    });
                }
                runs.extend(cell.runs.iter().cloned());
            }
            let has_text = runs.iter().any(|run| !run.text.trim().is_empty());
            if has_text {
                // 单卡邮件里按钮常写在内联 `<a style="background:…">` 上，会被并进正文，
                // 阅读窗口只能画成一段下划线文字。只有确实存在「被换行包围的短链接」时
                // 才切分段落，其余情况保持原样，避免无谓地打散正文。
                if let Some(parts) = split_standalone_ctas(&runs) {
                    for part in parts {
                        blocks.push(Block::Paragraph(Paragraph {
                            runs: part,
                            align,
                            indent: 0.0,
                            before: 0.0,
                            after: 8.0,
                            quote: false,
                            bg,
                        }));
                    }
                } else {
                    blocks.push(Block::Paragraph(Paragraph {
                        runs,
                        align,
                        indent: 0.0,
                        before: 0.0,
                        after: 8.0,
                        quote: false,
                        bg,
                    }));
                }
            }
            // 图片在文字之后按原顺序补上：logo / hero 常在单元格里独占一行。
            for cell in &cells {
                blocks.extend(cell.images.iter().cloned().map(Block::Image));
            }
        } else {
            for cell in cells {
                if cell.runs.iter().any(|run| !run.text.trim().is_empty()) {
                    blocks.push(Block::Paragraph(Paragraph {
                        runs: cell.runs.clone(),
                        align: cell.align,
                        indent: 0.0,
                        before: 0.0,
                        after: 6.0,
                        quote: false,
                        bg: cell.bg.and_then(visible_bg),
                    }));
                }
                blocks.extend(cell.images.iter().cloned().map(Block::Image));
            }
        }
    }
    blocks
}

/// 把「一行就是一个短链接」的片段独立出来，让 CTA 能被画成按钮。
/// 返回 `None` 表示整段没有这种片段，调用方保持原来的整段排版。
fn split_standalone_ctas(runs: &[Run]) -> Option<Vec<Vec<Run>>> {
    fn qualifies(segment: &[Run]) -> bool {
        let visible: Vec<&Run> = segment.iter().filter(|run| !run.text.trim().is_empty()).collect();
        visible.len() == 1
            && visible[0].link.is_some()
            && visible[0].text.trim().chars().count() <= crate::mail_tuning::MAX_DATA_CELL_CHARS
    }

    // 按换行切成若干「行」；块级标签在 collect_into 里就是靠 '\n' 分段的。
    let mut segments: Vec<Vec<Run>> = Vec::new();
    let mut current: Vec<Run> = Vec::new();
    for run in runs {
        if !run.text.contains('\n') {
            current.push(run.clone());
            continue;
        }
        for (index, piece) in run.text.split('\n').enumerate() {
            if index > 0 {
                segments.push(std::mem::take(&mut current));
            }
            if !piece.is_empty() {
                let mut copy = run.clone();
                copy.text = piece.to_string();
                current.push(copy);
            }
        }
    }
    segments.push(current);

    if !segments.iter().any(|segment| qualifies(segment)) {
        return None;
    }

    let mut out: Vec<Vec<Run>> = Vec::new();
    let mut pending: Vec<Run> = Vec::new();
    for segment in segments {
        if qualifies(&segment) {
            if !pending.is_empty() {
                out.push(std::mem::take(&mut pending));
            }
            out.push(segment);
        } else {
            if !pending.is_empty() {
                if let Some(last) = pending.last().cloned() {
                    let mut separator = last;
                    separator.text = "\n".into();
                    separator.link = None;
                    separator.underline = false;
                    pending.push(separator);
                }
            }
            pending.extend(segment);
        }
    }
    if !pending.is_empty() {
        out.push(pending);
    }
    out.retain(|part| part.iter().any(|run| !run.text.trim().is_empty()));
    Some(out)
}

/// 只有明确的强调色才配拥有底色。
/// 邮件里绝大多数 `bgcolor="#ffffff"` / `#f7f8fa` / `#e8e8e8` 是布局骨架的纸张色，
/// 在深色界面上会被画成一条条脏灰横条；近白、近黑、低饱和灰一律丢弃。
pub(crate) fn visible_bg(color: Color32) -> Option<Color32> {
    use crate::mail_tuning as tuning;
    let (r, g, b) = (color.r() as f32, color.g() as f32, color.b() as f32);
    let luma = 0.299 * r + 0.587 * g + 0.114 * b;
    let saturation = r.max(g).max(b) - r.min(g).min(b);
    if color.a() < tuning::BACKGROUND_MIN_ALPHA
        || luma >= tuning::BACKGROUND_MAX_LUMA
        || luma <= tuning::BACKGROUND_MIN_LUMA
        || saturation < tuning::BACKGROUND_MIN_SATURATION
    {
        None
    } else {
        Some(color)
    }
}

/// 去掉空白后的文字长度；判断数据表格与过滤空单元格都用它。
fn cell_len(runs: &[Run]) -> usize {
    runs.iter().flat_map(|run| run.text.chars()).filter(|c| !c.is_whitespace()).count()
}

/// 收集单元格里的图片。`collect_runs` 会跳过 `<img>`，早期版本因此把表格里的
/// logo / hero / 商品图整片丢掉；这里按与正文相同的规则补回来。
fn collect_cell_images(node: &Handle, align: Align, skip_tables: bool) -> Vec<Image> {
    fn walk(node: &Handle, align: Align, out: &mut Vec<Image>, depth: usize, skip_tables: bool) {
        if depth > 32 {
            return;
        }
        for child in node.children.borrow().iter() {
            let NodeData::Element { name, attrs, .. } = &child.data else { continue };
            let tag = name.local.to_string().to_ascii_lowercase();
            // 单元格里有嵌套表时，表内的图归嵌套表自己收，这里跳过，
            // 否则同一张 logo 会在正文里出现两遍（GitHub 通知邮件满屏重复图就是这个）。
            if tag == "table" && skip_tables {
                continue;
            }
            if tag == "img" {
                let mut src = String::new();
                let mut width = None;
                let mut height = None;
                let mut alt = String::new();
                for attr in attrs.borrow().iter() {
                    let key = attr.name.local.to_string().to_ascii_lowercase();
                    let value = attr.value.to_string();
                    match key.as_str() {
                        "src" => src = value,
                        "width" => {
                            width = value.trim_end_matches("px").trim().parse::<f32>().ok()
                        }
                        "height" => {
                            height = value.trim_end_matches("px").trim().parse::<f32>().ok()
                        }
                        "alt" => alt = value.trim().to_string(),
                        _ => {}
                    }
                }
                if src.is_empty() {
                    continue;
                }
                // 追踪像素与透明间隔条不占位（与正文同一套规则）。
                if width.is_some_and(|w| w <= crate::mail_tuning::TRACKING_PIXEL_MAX_PX)
                    || height.is_some_and(|h| h <= crate::mail_tuning::TRACKING_PIXEL_MAX_PX)
                {
                    continue;
                }
                out.push(Image { src, width, height, alt, align });
                continue;
            }
            walk(child, align, out, depth + 1, skip_tables);
        }
    }
    let mut out = Vec::new();
    walk(node, align, &mut out, 0, skip_tables);
    out
}

fn collect_rows(
    node: &Handle,
    style: Style,
    sheet: &StyleSheet,
    rows: &mut Vec<TableRow>,
    nested: &mut Vec<Table>,
) {
    for child in node.children.borrow().iter() {
        let NodeData::Element { name, .. } = &child.data else { continue };
        let tag = name.local.to_string().to_ascii_lowercase();
        match tag.as_str() {
            "tr" => {
                let mut cells = Vec::new();
                for cell in child.children.borrow().iter() {
                    let NodeData::Element { name, attrs, .. } = &cell.data else { continue };
                    let cell_tag = name.local.to_string().to_ascii_lowercase();
                    if cell_tag != "td" && cell_tag != "th" {
                        continue;
                    }
                    // 单元格里还套着表格（按钮、明细表）是邮件最常见的排版方式。
                    // 以前为了「按阅读顺序摊平」把内层行直接提上来，代价是整格文字和
                    // logo/hero 全被丢掉（单卡 CTA 邮件只剩一个按钮，就是这条造成的）。
                    let mut cell_style = style;
                    let mut span = 1usize;
                    let mut shade = cell_tag == "th";
                    for attr in attrs.borrow().iter() {
                        let key = attr.name.local.to_string().to_ascii_lowercase();
                        let value = attr.value.to_string();
                        match key.as_str() {
                            "style" => cell_style = apply_style(cell_style, &value),
                            "align" => {
                                cell_style.align = match value.to_ascii_lowercase().as_str() {
                                    "center" => Align::Center,
                                    "right" => Align::Right,
                                    _ => cell_style.align,
                                }
                            }
                            "colspan" => span = value.parse::<usize>().unwrap_or(1).clamp(1, 8),
                            "bgcolor" => {
                                shade = true;
                                if let Some(color) = parse_color(&value) {
                                    cell_style.bg = Some(color);
                                }
                            }
                            _ => {}
                        }
                    }
                    if cell_tag == "th" {
                        cell_style.bold = true;
                    }
                    let mut inner = Vec::new();
                    collect_nested_tables(cell, style, sheet, &mut inner, 0);
                    let runs = if inner.is_empty() {
                        collect_runs(cell, cell_style, sheet, 0)
                    } else {
                        // 有嵌套表时只收本格自己的文字，嵌套表的文字由它自己进块，
                        // 否则同一段内容会读两遍。
                        collect_runs_own(cell, cell_style, sheet)
                    };
                    let images = collect_cell_images(cell, cell_style.align, !inner.is_empty());
                    nested.extend(inner);
                    // 按钮常把底色写在 `<a style="background-color:…">` 上，而不是 `<td>`。
                    // 只有「整格就是一个短链接」时才把它当成按钮底色，避免给整段文字染色。
                    let text_len = cell_len(&runs);
                    let standalone_link = text_len > 0
                        && text_len <= 30
                        && runs.iter().filter(|r| !r.text.trim().is_empty()).all(|r| r.link.is_some());
                    let bg = if standalone_link {
                        cta_background(cell, 0).or(cell_style.bg)
                    } else {
                        cell_style.bg
                    };
                    cells.push(TableCell {
                        runs,
                        images,
                        align: cell_style.align,
                        header: cell_tag == "th",
                        span,
                        shade,
                        bg,
                    });
                }
                if !cells.is_empty() {
                    rows.push(TableRow { cells });
                }
            }
            // 嵌套的按钮表（NVIDIA 的 `<table bgcolor="#76b900">`）把自己的底色
            // 传给子单元格，否则 CTA 会退化成没有底色的普通链接。
            "table" => {
                let mut table_style = style;
                for (key, value) in node_attrs(child) {
                    if key.eq_ignore_ascii_case("bgcolor") {
                        if let Some(color) = parse_color(&value) {
                            table_style.bg = Some(color);
                        }
                    } else if key.eq_ignore_ascii_case("style") {
                        for (name, decl) in declarations(&value) {
                            if name == "background-color" || name == "background" {
                                if let Some(color) = parse_color(decl.split_whitespace().next().unwrap_or("")) {
                                    table_style.bg = Some(color);
                                }
                            }
                        }
                    }
                }
                if let Some(table) = collect_table(child, table_style, sheet) {
                    nested.push(table);
                }
            }
            "thead" | "tbody" | "tfoot" => collect_rows(child, style, sheet, rows, nested),
            _ => collect_rows(child, style, sheet, rows, nested),
        }
    }
}

/// 找单元格里第一个 `<a style="background-color:…">`，用于把 CTA 还原成按钮。
fn cta_background(node: &Handle, depth: usize) -> Option<Color32> {
    if depth > 6 {
        return None;
    }
    for child in node.children.borrow().iter() {
        let NodeData::Element { name, attrs, .. } = &child.data else { continue };
        if name.local.to_string().eq_ignore_ascii_case("a") {
            for attr in attrs.borrow().iter() {
                if !attr.name.local.to_string().eq_ignore_ascii_case("style") {
                    continue;
                }
                for (key, value) in declarations(&attr.value) {
                    if key == "background-color" || key == "background" {
                        let first = value.split_whitespace().next().unwrap_or("");
                        if let Some(color) = parse_color(first) {
                            return Some(color);
                        }
                    }
                }
            }
        }
        if let Some(color) = cta_background(child, depth + 1) {
            return Some(color);
        }
    }
    None
}

fn node_attrs(node: &Handle) -> Vec<(String, String)> {
    let NodeData::Element { attrs, .. } = &node.data else { return Vec::new() };
    attrs
        .borrow()
        .iter()
        .map(|a| (a.name.local.to_string(), a.value.to_string()))
        .collect()
}

/// 后代里（任意深度、限 6 层）有没有 `<table>`。
/// 单元格内的行内内容（保留加粗、颜色、字号）。
fn collect_runs(node: &Handle, style: Style, sheet: &StyleSheet, depth: usize) -> Vec<Run> {
    let mut runs = Vec::new();
    collect_into(node, style, &None, sheet, depth, &mut runs, false);
    normalize_runs(runs)
}

/// 块级标签会前后各插一个换行，这里合并掉重复的，并去掉首尾空白。
fn normalize_runs(runs: Vec<Run>) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for run in runs {
        if run.text.chars().all(|c| c == '\n' || c.is_whitespace())
            && out.last().is_some_and(|last: &Run| last.text.ends_with('\n'))
        {
            continue;
        }
        out.push(run);
    }
    while out.first().is_some_and(|run| run.text.trim().is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|run| run.text.trim().is_empty()) {
        out.pop();
    }
    out
}

/// 只收单元格自己的文字：跳过嵌套 `<table>` 子树（它自己会单独成块）。
fn collect_runs_own(node: &Handle, style: Style, sheet: &StyleSheet) -> Vec<Run> {
    let mut runs = Vec::new();
    collect_into(node, style, &None, sheet, 0, &mut runs, true);
    normalize_runs(runs)
}

/// 收集单元格里的嵌套表格（按钮、明细表）。
///
/// 只处理最外层：`collect_table` 在内部已经把更深的嵌套收进 `Table::nested`，
/// 这里继续往下走会把同一张表收两遍。
fn collect_nested_tables(
    node: &Handle,
    style: Style,
    sheet: &StyleSheet,
    out: &mut Vec<Table>,
    depth: usize,
) {
    if depth > 8 {
        return;
    }
    for child in node.children.borrow().iter() {
        let NodeData::Element { name, .. } = &child.data else { continue };
        let tag = name.local.to_string().to_ascii_lowercase();
        if tag == "table" {
            if let Some(table) = collect_table(child, style, sheet) {
                out.push(table);
            }
            continue;
        }
        collect_nested_tables(child, style, sheet, out, depth + 1);
    }
}

fn collect_into(node: &Handle, style: Style, link: &Option<std::sync::Arc<str>>, sheet: &StyleSheet, depth: usize, runs: &mut Vec<Run>, skip_tables: bool) {
    if depth > 12 {
        return;
    }
    for child in node.children.borrow().iter() {
        match &child.data {
            NodeData::Text { contents } => {
                let text = decode_entities(&contents.borrow().to_string()).replace(['\r', '\n', '\t'], " ");
                let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if collapsed.is_empty() {
                    if !runs.is_empty() && !runs.last().is_some_and(|r: &Run| r.text.ends_with(' ')) {
                        runs.push(style.run(" ".into(), link.as_ref()));
                    }
                } else {
                    runs.push(style.run(collapsed, link.as_ref()));
                }
            }
            NodeData::Element { name, attrs, .. } => {
                let tag = name.local.to_string().to_ascii_lowercase();
                if matches!(tag.as_str(), "script" | "style" | "head") {
                    continue;
                }
                if tag == "br" {
                    runs.push(style.run("\n".into(), None));
                    continue;
                }
                if tag == "img" {
                    continue;
                }
                let attr_pairs: Vec<(String, String)> = attrs
                    .borrow()
                    .iter()
                    .map(|a| (a.name.local.to_string().to_ascii_lowercase(), a.value.to_string()))
                    .collect();
                let mut style = sheet.apply(&tag, &attr_pairs, style);
                for attr in attrs.borrow().iter() {
                    if attr.name.local.to_string().eq_ignore_ascii_case("style") {
                        style = apply_style(style, &attr.value);
                    }
                }
                if tag == "b" || tag == "strong" {
                    style.bold = true;
                }
                if tag == "i" || tag == "em" {
                    style.italic = true;
                }
                if tag == "u" || tag == "a" {
                    style.underline = true;
                }
                // 单元格里的链接同样要能点：href 随子树往下传。
                let mut link = link.clone();
                if tag == "a" {
                    if let Some(href) = attrs.borrow().iter().find(|a| a.name.local.to_string().eq_ignore_ascii_case("href")) {
                        let href = href.value.to_string().trim().to_string();
                        if href.starts_with("http://") || href.starts_with("https://") || href.starts_with("mailto:") {
                            link = Some(std::sync::Arc::from(href.as_str()));
                        }
                    }
                }
                if let Some(align) = attrs.borrow().iter().find(|a| a.name.local.to_string().eq_ignore_ascii_case("align")) {
                    style.align = match align.value.to_string().to_ascii_lowercase().as_str() {
                        "center" => Align::Center,
                        "right" => Align::Right,
                        _ => style.align,
                    };
                }
                if tag == "pre" || tag == "code" {
                    let start = runs.len();
                    collect_into(child, style, &link, sheet, depth + 1, runs, skip_tables);
                    for run in runs.iter_mut().skip(start) {
                        run.mono = true;
                    }
                    continue;
                }
                // 嵌套表由 collect_table 单独成块；这里跳过它的子树，避免同一段文字
                // 既进单元格文字、又进嵌套表，读起来重复一遍。
                if skip_tables && tag == "table" {
                    continue;
                }
                // 单元格里常把多行内容拆成多个 div，必须换行，否则会挤成一句。
                // table/td/th 正常已在 collect_rows 处理，这里是漏网时的兜底。
                let is_block = matches!(tag.as_str(), "div" | "p" | "li" | "tr" | "td" | "th" | "table" | "section" | "article" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote" | "pre");
                if is_block {
                    runs.push(style.run("\n".into(), None));
                }
                collect_into(child, style, &link, sheet, depth + 1, runs, skip_tables);
                if is_block {
                    runs.push(style.run("\n".into(), None));
                }
            }
            _ => {}
        }
    }
}

fn trim(blocks: &mut Vec<Block>) {
    while matches!(blocks.last(), Some(Block::Space(_)) | Some(Block::Divider)) {
        blocks.pop();
    }
    while matches!(blocks.first(), Some(Block::Space(_)) | Some(Block::Divider)) {
        blocks.remove(0);
    }
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut inside = false;
    for ch in html.chars() {
        match ch {
            '<' => inside = true,
            '>' => {
                inside = false;
                out.push(' ');
            }
            _ if !inside => out.push(ch),
            _ => {}
        }
    }
    decode_entities(&out)
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        let end = tail.find(';').filter(|end| *end <= 12);
        let Some(end) = end else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..end];
        let decoded = match entity {
            "nbsp" | "#160" | "ensp" | "emsp" | "thinsp" | "#8194" | "#8195" | "#8201" => Some(' '),
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "middot" | "#183" | "bull" | "#8226" => Some('·'),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            "copy" => Some('©'),
            "reg" => Some('®'),
            "laquo" => Some('«'),
            "raquo" => Some('»'),
            "ldquo" => Some('“'),
            "rdquo" => Some('”'),
            "lsquo" => Some('‘'),
            "rsquo" => Some('’'),
            "zwnj" | "zwj" | "shy" => Some(' '),
            other => other
                .strip_prefix("#x")
                .or_else(|| other.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| other.strip_prefix('#').and_then(|dec| dec.parse::<u32>().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(ch) => {
                out.push(ch);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------- 纯文本

fn from_text(text: &str) -> Vec<Block> {
    let style = Style::default();
    let mut blocks = Vec::new();
    let mut para: Vec<String> = Vec::new();
    fn flush(para: &mut Vec<String>, blocks: &mut Vec<Block>, style: Style) {
        if para.is_empty() {
            return;
        }
        let joined = para.join("\n");
        para.clear();
        let text = joined.trim().to_string();
        if text.is_empty() {
            return;
        }
        // 纯文本里的 `1. ` / `• ` 也还原成列表，读起来更清楚。
        let (marker, body) = if let Some(rest) = text.strip_prefix("• ") {
            (Some("•  ".to_string()), rest.trim().to_string())
        } else if let Some((digits, rest)) = split_ordered(&text) {
            (Some(format!("{digits}. ")), rest.to_string())
        } else {
            (None, text)
        };
        let paragraph = Paragraph {
            runs: vec![style.run(body, None)],
            align: Align::Left,
            indent: if marker.is_some() { 14.0 } else { 0.0 },
            before: 0.0,
            after: 8.0,
            quote: false,
            bg: None,
        };
        blocks.push(match marker {
            Some(marker) => Block::ListItem(ListItem { marker, level: 0, para: paragraph }),
            None => Block::Paragraph(paragraph),
        });
    }
    for line in text.replace("\r\n", "\n").lines() {
        if line.trim().is_empty() {
            flush(&mut para, &mut blocks, style);
        } else {
            para.push(line.trim().to_string());
        }
    }
    flush(&mut para, &mut blocks, style);
    trim(&mut blocks);
    blocks
}

fn split_ordered(line: &str) -> Option<(&str, &str)> {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let rest = line.get(digits..)?;
    let body = rest.strip_prefix(". ")?;
    Some((&line[..digits], body.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(doc: &Doc) -> Vec<String> {
        doc.blocks
            .iter()
            .filter_map(|block| match block {
                Block::Paragraph(p) => Some(p.runs.iter().map(|r| r.text.as_str()).collect::<String>()),
                Block::ListItem(i) => Some(format!(
                    "{}{}",
                    i.marker,
                    i.para.runs.iter().map(|r| r.text.as_str()).collect::<String>()
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn layout_tables_flatten_into_reading_order() {
        // 营销邮件的骨架：无边框表格、单元格塞整段文字。硬画网格会把正文压成窄条，
        // 应摊平成整宽段落，而不是 Table 块。
        let doc = render(
            r#"<table cellpadding="0" cellspacing="0"><tr>
                <td style="font-size:16px">腾讯云AI资源包即将到期通知，请及时处理即将到期的资源，避免影响业务运行。</td>
                <td>右侧装饰列</td>
            </tr></table>"#,
            "",
        );
        assert!(
            !doc.blocks.iter().any(|b| matches!(b, Block::Table(_))),
            "无边框长文表格应摊平: {doc:?}"
        );
        assert!(matches!(doc.blocks[0], Block::Paragraph(_)), "{doc:?}");
        let text = texts(&doc).join("");
        assert!(text.contains("腾讯云AI资源包即将到期通知"));
        assert!(text.contains("右侧装饰列"), "摊平后不能丢内容: {text}");
    }

    #[test]
    fn nested_table_hidden_in_div_is_still_expanded() {
        // QQ 邮箱转发常用 <td><div><table>…</table></div></td> 包住原邮件；
        // 之前只检查直接子元素，这种嵌套会被当成一整个单元格。
        let doc = render(
            r#"<table><tr><td><div>转发说明
                <table border="1"><tr><th>项目</th><th>数值</th></tr><tr><td>未读</td><td>12</td></tr></table>
            </div></td></tr></table>"#,
            "",
        );
        let table = doc.blocks.iter().find_map(|b| match b {
            Block::Table(t) => Some(t),
            _ => None,
        });
        let table = table.expect("内层数据表格应被识别");
        assert_eq!(table.rows.len(), 2, "内层行应被提取上来: {table:?}");
        assert_eq!(table.rows[0].cells.len(), 2);
    }

    #[test]
    fn bordered_data_tables_stay_grids() {
        let doc = render(r#"<table border="1"><tr><th>商品</th><th>金额</th></tr><tr><td>键盘</td><td>499</td></tr></table>"#, "");
        assert!(doc.blocks.iter().any(|b| matches!(b, Block::Table(t) if t.bordered)), "{doc:?}");
    }

    #[test]
    fn html_wins_over_longer_plain_text_and_keeps_links() {
        // 纯文本版本常把链接展开成裸 URL，字数比 HTML 多；不能因此倒退成纯文本。
        let html = r#"<div>你需要<a href="https://example.com/upgrade">升级至 Plus 方案</a>或减少用量。</div>"#;
        let plain = "你需要前往 https://example.com/upgrade?a=1&amp;b=2 并升级至 Plus 方案，或减少现有储存空间的用量。这段纯文本被刻意写得很长很长很长很长很长很长很长很长很长很长很长很长很长很长很长很长";
        let doc = render(html, plain);
        let para = match &doc.blocks[0] {
            Block::Paragraph(p) => p,
            other => panic!("应优先 HTML: {other:?}"),
        };
        assert!(para.runs.iter().all(|run| !run.text.contains("upgrade?a=1")), "不应出现裸 URL: {para:?}");
        let link = para.runs.iter().find(|run| run.link.is_some()).expect("应保留链接");
        assert_eq!(link.text, "升级至 Plus 方案");
        assert_eq!(link.link.as_deref(), Some("https://example.com/upgrade"));
    }

    #[test]
    fn table_cell_links_are_kept() {
        // 「现在登出」这类按钮链接在表格单元格里，同样要能点。
        let doc = render(
            r#"<table border="1"><tr><td align="center"><a href="https://example.com/logout">现在登出</a></td></tr></table>"#,
            "",
        );
        // 单行表格是按钮/包裹器，会摊平成一个段落；无论哪种形态链接都不能丢。
        let has_link = doc.blocks.iter().any(|block| match block {
            Block::Table(t) => t.rows.iter().any(|row| row.cells.iter().any(|cell|
                cell.runs.iter().any(|run| run.link.as_deref() == Some("https://example.com/logout")))),
            Block::Paragraph(p) => p.runs.iter().any(|run| run.link.as_deref() == Some("https://example.com/logout")),
            _ => false,
        });
        assert!(has_link, "按钮链接必须保留: {doc:?}");
    }

    #[test]
    fn style_sheet_classes_apply() {
        let doc = render(
            r#"<style>.big{font-size:19px;color:#d93025}.center{text-align:center}</style><div class="big center">标题</div>"#,
            "",
        );
        let para = match &doc.blocks[0] {
            Block::Paragraph(p) => p,
            other => panic!("应为段落: {other:?}"),
        };
        assert_eq!(para.align, Align::Center);
        assert!((para.runs[0].size - 19.0).abs() < 0.01, "{:?}", para.runs[0]);
        assert_eq!(para.runs[0].color, Color32::from_rgb(0xd9, 0x30, 0x25));
    }

    #[test]
    fn long_cells_become_separate_paragraphs_with_background() {
        // 多列布局（每格都是整段文字）逐格成段；只有明确的强调色才铺底，
        // 中性灰/近白底在深色界面上只会变成一条条脏灰横条。
        let doc = render(
            r##"<table><tr><td bgcolor="#dbeafe">左侧是一整段比较长的说明文字，用来看逐格排版</td><td>右侧同样是一整段比较长的说明文字内容</td></tr></table>"##,
            "",
        );
        let paras: Vec<_> = doc
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Paragraph(p) => Some(p),
                _ => None,
            })
            .collect();
        assert_eq!(paras.len(), 2, "{doc:?}");
        assert_eq!(paras[0].bg, Some(Color32::from_rgb(0xdb, 0xea, 0xfe)));
        assert!(paras[1].bg.is_none());
        // 真实邮件里的骨架底色必须被丢掉，不能渲染成灰条。
        let neutral = render(
            r##"<table><tr><td bgcolor="#f7f8fa">一封邮件正文，内容比较长，用来检查布局表的底色</td><td>右侧文字内容</td></tr></table>"##,
            "",
        );
        assert!(neutral.blocks.iter().all(|b| match b {
            Block::Paragraph(p) => p.bg.is_none(),
            _ => true,
        }), "{neutral:?}");
    }

    #[test]
    fn short_cells_still_join_into_one_line() {
        let doc = render("<table><tr><td>首页</td><td>帮助</td><td>退订</td></tr></table>", "");
        assert_eq!(texts(&doc), ["首页  帮助  退订"]);
    }

    #[test]
    fn layout_table_keeps_dom_order_of_text_and_images() {
        // 单元格里「文字 → 图 → 文字」时，图必须留在原位，不能被补到段落后面。
        let doc = render(
            "<table><tr><td>第一段文字说明</td></tr><tr><td><img src=\"https://example.com/a.png\" width=\"120\" height=\"40\"></td></tr><tr><td>第二段文字说明</td></tr></table>",
            "",
        );
        let kinds: Vec<&str> = doc
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Paragraph(_) => Some("P"),
                Block::Image(_) => Some("IMG"),
                Block::Table(_) => Some("TABLE"),
                _ => None,
            })
            .collect();
        let text = kinds.join(" ");
        assert_eq!(text, "P IMG P", "顺序被改动了: {kinds:?} / {doc:?}");
    }

    #[test]
    fn headered_card_email_keeps_header_before_body_and_footer_apart() {
        // 复刻 Gmail 安全提醒的骨架（脱敏重写）：外层包裹表 → 灰底提示行（图标 + 文案）
        // → 带边框卡片（logo、标题、正文、按钮、页脚、副本说明）。
        // 以前这张表被摊平成一整段，提示行跑到页脚后面，页脚四行也挤成一段。
        let html = r##"<table width="100%" height="100%"><tbody>
            <tr height="32"><td></td></tr>
            <tr align="center"><td>
              <table><tbody><tr><td width="8"></td><td>
                <div style="background-color:#f5f5f5;padding:16px">
                  <table><tbody><tr>
                    <td><img src="https://example.com/icon.png" height="20"></td>
                    <td width="13"></td>
                    <td><span>这是发送给 <a href="https://example.com/a">user@example.com</a> 的安全提醒的副本。</span></td>
                  </tr></tbody></table>
                </div>
                <div style="border:1px solid #dadce0;border-radius:8px;padding:40px 20px" align="center">
                  <img src="https://example.com/logo.png" width="74" height="24" alt="Logo">
                  <div style="font-size:24px">在 Windows 设备上有新的登录活动</div>
                  <div>我们发现您的账号在一部 Windows 设备上有新的登录活动。</div>
                  <div style="padding-top:32px"><a href="https://example.com/alert" style="color:#ffffff;background-color:#0b57d0;padding:12px 24px">查看活动</a></div>
                  <div style="padding-top:20px;font-size:12px">您也可以访问以下网址查看安全性活动：<br><a href="https://example.com/notifications">https://example.com/notifications</a></div>
                  <div>我们向您发送这封电子邮件，目的是让您了解关于您的账号和服务的重大变化。</div>
                  <div>© 2026 Example LLC, 1600 Amphitheatre Parkway</div>
                </div>
              </td></tr></tbody></table>
            </td></tr>
        </tbody></table>"##;
        let doc = render(html, "");
        assert!(
            !doc.blocks.iter().any(|b| matches!(b, Block::Table(_))),
            "整封邮件的外层包裹表不该被画成空网格: {doc:?}"
        );
        let texts: Vec<String> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(p) => Some(p.runs.iter().map(|r| r.text.as_str()).collect()),
                _ => None,
            })
            .collect();
        let header = texts.iter().position(|t| t.contains("安全提醒的副本")).expect("提示行不能丢");
        let heading = texts.iter().position(|t| t.contains("新的登录活动") && t.contains("Windows")).expect("标题不能丢");
        assert!(header < heading, "提示行应在标题之前，实际 {texts:?}");
        // 页脚三行必须各自成段，不能挤在一条里。
        let footer: Vec<&String> = texts.iter().filter(|t| t.contains("重大变化") || t.contains("您也可以访问")).collect();
        assert_eq!(footer.len(), 2, "页脚应各自成段: {texts:?}");
        // 按钮的底色写在 `<a style="background-color">` 上，要还原成按钮。
        let cta = doc
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Paragraph(p) if p.runs.iter().any(|r| r.text.trim() == "查看活动") => Some(p),
                _ => None,
            })
            .expect("按钮段落应存在");
        assert_eq!(cta.bg, Some(Color32::from_rgb(0x0b, 0x57, 0xd0)), "{cta:?}");
        // 图标和 logo 都要保留，且顺序是「图标 → logo」。
        let images: Vec<&str> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Image(i) => Some(i.src.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(images, ["https://example.com/icon.png", "https://example.com/logo.png"], "{doc:?}");
    }

    #[test]
    fn empty_wrapper_table_leaves_no_block() {
        // 纯间距/包裹用的空表格不该在正文里留下一个空框。
        let doc = render("<table><tr><td></td></tr><tr><td></td></tr></table><div>正文</div>", "");
        assert!(!doc.blocks.iter().any(|b| matches!(b, Block::Table(_))), "{doc:?}");
        assert!(texts(&doc).join("").contains("正文"));
    }

    #[test]
    fn pre_keeps_newlines_and_monospace() {
        // <pre> 里的换行与缩进是内容本身，不能被折叠成一行。
        let doc = render("<pre>line one\n    indented\nlast</pre>", "");
        let para = match &doc.blocks[0] {
            Block::Paragraph(p) => p,
            other => panic!("应为段落: {other:?}"),
        };
        let text: String = para.runs.iter().map(|run| run.text.as_str()).collect();
        assert_eq!(text, "line one\n    indented\nlast", "{text:?}");
        assert!(para.runs.iter().all(|run| run.mono), "应使用等宽字体: {para:?}");
    }

    #[test]
    fn font_weight_and_size_are_preserved() {
        let doc = render(
            r#"<p style="font-size:17px"><span style="font-weight:600">重点</span>普通<span style="font-weight:300">细体</span></p>"#,
            "",
        );
        let runs = match &doc.blocks[0] {
            Block::Paragraph(p) => &p.runs,
            other => panic!("应为段落: {other:?}"),
        };
        assert!(runs.iter().any(|r| r.text == "重点" && r.bold));
        assert!(runs.iter().any(|r| r.text == "普通" && !r.bold));
        assert!(runs.iter().all(|r| (r.size - 17.0).abs() < 0.01), "字号应继承 17px: {runs:?}");
    }

    #[test]
    fn colors_and_alignment_are_read_from_inline_style() {
        let doc = render(r#"<div style="text-align:center;color:#008acd">蓝色居中</div>"#, "");
        match &doc.blocks[0] {
            Block::Paragraph(p) => {
                assert_eq!(p.align, Align::Center);
                assert_eq!(p.runs[0].color, Color32::from_rgb(0, 138, 205));
            }
            other => panic!("应为段落: {other:?}"),
        }
    }

    #[test]
    fn ordered_and_unordered_lists_keep_markers() {
        let doc = render("<ol><li>一</li><li>二</li></ol><ul><li>甲</li></ul>", "");
        let items: Vec<_> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::ListItem(i) => Some(i.marker.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(items, ["1. ", "2. ", "•  "]);
    }

    #[test]
    fn table_keeps_rows_cells_and_header() {
        let doc = render(
            r#"<table border="1"><tr><th>项目</th><th>数值</th></tr><tr><td>未读</td><td>12</td></tr></table>"#,
            "",
        );
        let table = doc
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Table(t) => Some(t),
                _ => None,
            })
            .expect("应有表格");
        assert!(table.bordered);
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0].cells.len(), 2);
        assert!(table.rows[0].cells[0].header);
        assert_eq!(table.rows[1].cells[1].runs[0].text, "12");
    }

    #[test]
    fn images_keep_cid_and_remote_sources() {
        let doc = render(
            r#"<p><img src="cid:logo" alt="公司标志"><img src="https://example.com/a.png" width="520"></p>"#,
            "",
        );
        let images: Vec<_> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Image(i) => Some(i),
                _ => None,
            })
            .collect();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].src, "cid:logo");
        assert_eq!(images[1].width, Some(520.0));
    }

    #[test]
    fn tracking_pixel_is_dropped() {
        let doc = render(r#"<img src="https://t.example/p.gif" width="1" height="1">"#, "");
        assert!(!doc.blocks.iter().any(|b| matches!(b, Block::Image(_))), "{doc:?}");
    }

    #[test]
    fn script_and_style_never_reach_the_body() {
        let doc = render("<style>p{color:red}</style><script>alert(1)</script><p>正文</p>", "");
        assert_eq!(texts(&doc), ["正文"]);
    }

    #[test]
    fn entities_and_quotes_are_decoded() {
        let doc = render("<p>价格 &lt; 100 &amp; 包邮&nbsp;·&nbsp;限今天</p>", "");
        assert_eq!(texts(&doc), ["价格 < 100 & 包邮 · 限今天"]);
    }

    #[test]
    fn plain_text_falls_back_with_paragraphs() {
        let doc = render("", "第一段\n\n第二段\n\n• 事项");
        assert_eq!(texts(&doc), ["第一段", "第二段", "•  事项"]);
    }

    #[test]
    fn icloud_style_markup_keeps_weight_and_size() {
        let html = r#"<div style="font-family:system-ui;font-size:17px">
            <span style="font-weight:600">你的 iCloud 储存空间已满。</span><span style="font-weight:300">由于你的使用量已超出方案。</span></div>"#;
        let doc = render(html, "");
        let runs = match &doc.blocks[0] {
            Block::Paragraph(p) => &p.runs,
            other => panic!("应为段落: {other:?}"),
        };
        assert!(runs[0].bold, "600 应加粗: {runs:?}");
        assert!((runs[0].size - 17.0).abs() < 0.01, "17px 应保留: {runs:?}");
    }

    #[test]
    fn border_collapse_layout_table_is_flattened_not_gridded() {
        // 腾讯云账号登录提醒：三列骨架表，`border-collapse:collapse` 不是可见边框。
        // 旧逻辑会把它画成 Grid，正文被压成中间窄列、空单元格变成三条灰块。
        let html = r##"<table style="width:100%;border-collapse:collapse"><tr>
            <td bgcolor="#f7f8fa"></td>
            <td bgcolor="#f7f8fa">账号登录提醒 尊敬的腾讯云用户，您好！您的账号于 2026-10-15 11:35:50 登录腾讯云，请关注此次登录行为。</td>
            <td bgcolor="#f7f8fa"></td>
        </tr><tr>
            <td bgcolor="#f7f8fa"></td>
            <td bgcolor="#f7f8fa">此为系统邮件，请勿回复</td>
            <td bgcolor="#f7f8fa"></td>
        </tr></table>"##;
        let doc = render(html, "");
        assert!(!doc.blocks.iter().any(|b| matches!(b, Block::Table(_))), "骨架表必须摊平: {doc:?}");
        assert!(texts(&doc).join("").contains("账号登录提醒"), "正文不能丢: {doc:?}");
        assert!(
            doc.blocks.iter().all(|b| matches!(b, Block::Paragraph(p) if p.bg.is_none())),
            "近白骨架底色不能变成灰条: {doc:?}"
        );
    }

    #[test]
    fn bordered_card_with_long_text_is_flattened() {
        // GitHub 通知：带 1px 边框的卡片，但里面装的是整段标题 + 按钮，
        // 不是「每格几个词」的数据表，不该用固定列宽 Grid 压窄。
        let html = r##"<table style="border:1px solid #d0d7de;border-radius:6px">
            <tr><td bgcolor="#ffffff"></td></tr>
            <tr><td bgcolor="#ffffff">[Renly1994/xiaotang-daozai] Windows 检查 workflow run，请查看失败详情</td></tr>
            <tr><td bgcolor="#ffffff"></td></tr>
            <tr><td bgcolor="#ffffff" align="center"><a href="https://example.com/run">View workflow run</a></td></tr>
        </table>"##;
        let doc = render(html, "");
        assert!(!doc.blocks.iter().any(|b| matches!(b, Block::Table(_))), "卡片表必须摊平: {doc:?}");
        assert!(texts(&doc).join("").contains("View workflow run"), "按钮不能丢: {doc:?}");
    }

    #[test]
    fn cta_background_is_taken_from_table_or_anchor() {
        // NVIDIA 把绿底写在 `<table bgcolor>`，GitHub 写在 `<a style="background-color">`。
        let nvidia = r##"<table bgcolor="#76b900" border="0" style="border:none"><tr>
            <td align="center"><a href="https://example.com/verify" style="color:#000000">验证电子邮箱地址</a></td>
        </tr></table>"##;
        let doc = render(nvidia, "");
        let para = doc.blocks.iter().find_map(|b| match b { Block::Paragraph(p) => Some(p), _ => None })
            .expect("应摊平成段落");
        assert_eq!(para.bg, Some(Color32::from_rgb(0x76, 0xb9, 0x00)), "{doc:?}");

        let github = r##"<table><tr><td align="center" bgcolor="#ffffff">
            <a href="https://example.com/run" style="background-color:#1f883d;color:#fff">View workflow run</a>
        </td></tr></table>"##;
        let doc = render(github, "");
        let para = doc.blocks.iter().find_map(|b| match b { Block::Paragraph(p) => Some(p), _ => None })
            .expect("应摊平成段落");
        assert_eq!(para.bg, Some(Color32::from_rgb(0x1f, 0x88, 0x3d)), "{doc:?}");
    }

    #[test]
    fn confidence_no_longer_punishes_flattened_layout_tables() {
        // NVIDIA 这类事务邮件 padding / <style> / 布局表很多，但岛内其实排得很好。
        let html = r#"<style>.x{padding:8px}.y{font-size:14px}</style>
            <table style="border-collapse:collapse"><tr><td style="padding:12px">您的账户刚刚使用我们最近未见过您使用的 Windows 设备登录，点击下方链接验证这是您本人。</td></tr></table>"#;
        let doc = render(html, "");
        assert!(doc.confidence >= 80, "不该再对可读排版显示「还原度 10%」: {}", doc.confidence);
    }

    #[test]
    fn table_with_images_is_flattened_not_gridded() {
        // 苹果广告/商品卡这类「左图右文」的表按网格画会留一整列空白、把图挤到一侧。
        let doc = render(
            r#"<table><tr><td><img src="https://e.test/a.png" width="40" height="40"></td><td>说明文字</td></tr><tr><td><img src="https://e.test/b.png" width="40" height="40"></td><td>另一段说明</td></tr></table>"#,
            "",
        );
        assert!(
            !doc.blocks.iter().any(|b| matches!(b, Block::Table(_))),
            "含图表格必须摊平: {doc:?}"
        );
        let images = doc.blocks.iter().filter(|b| matches!(b, Block::Image(_))).count();
        assert_eq!(images, 2, "两张图各出现一次: {doc:?}");
    }

    #[test]
    fn images_inside_nested_tables_are_collected_once() {
        // GitHub 通知邮件满屏重复图：父单元格把嵌套表里的图也收了一遍。
        let doc = render(
            r#"<table><tr><td><p>标题</p><table><tr><td><img src="https://e.test/logo.png" width="30" height="30"></td></tr></table></td></tr></table>"#,
            "",
        );
        let images: Vec<_> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Image(i) => Some(i.src.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(images, vec!["https://e.test/logo.png"], "嵌套表里的图不能重复收集: {doc:?}");
    }

    #[test]
    fn confidence_drops_only_for_markup_we_cannot_render() {
        let simple = render("<p>你好</p><a href=\"https://example.com\">链接</a>", "");
        assert!(simple.confidence >= 90, "简单邮件不该提示查看完整排版: {}", simple.confidence);
        let complex = r#"<style>.x{padding:8px;display:flex;position:absolute;background-image:url(a.png)}</style>
            <table cellpadding="8"><tr><td style="padding:8px">A</td><td style="padding:8px">B</td></tr></table>
            <table><tr><td>嵌套</td></tr></table><table><tr><td>嵌套</td></tr></table>
            <table><tr><td>嵌套</td></tr></table><table><tr><td>嵌套</td></tr></table>
            <div style="display:flex;position:absolute;float:left"><iframe src="x"></iframe><form></form></div>"#;
        let score = confidence(complex, &[]);
        assert!(score < CONFIDENCE_HINT, "复杂营销邮件应触发引导: {score}");
        assert_eq!(confidence("", &[]), 100, "纯文本邮件没有还原风险");
    }
}
