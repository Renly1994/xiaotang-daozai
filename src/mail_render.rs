//! 邮件 HTML 渲染的纯函数层：清洗 → 样式提取 → 文档组装。
//!
//! 这里不碰磁盘、不启动进程、不读配置，任何渲染后端（Edge `--app` / WebView2 /
//! 未来别的内核）都复用同一份产物。要换内核只实现 [`crate::mail_html_view::Renderer`]，
//! 不要在渲染后端里重写清洗规则——安全策略只允许有一处定义。
//!
//! 排版原则：**尽量少干预**。邮件自己的 `<style>` 会被提取、过滤后注入邮件文档，
//! 阅读层 CSS 只提供底色、字体兜底与防溢出，不用 `display:block` /
//! `border-collapse` / `!important` 去改写邮件精心排好的表格。

use crate::mail_view::WebTheme;
use base64::Engine as _;
use regex::Regex;
use std::collections::{HashMap, HashSet};

/// 一次渲染请求：只含与具体账号/服务商无关的输入。
pub struct RenderRequest<'a> {
    /// 原始邮件 HTML（未清洗）。
    pub html: &'a str,
    /// `cid:` 内嵌图：Content-ID → 图片字节。
    pub images: &'a HashMap<String, std::sync::Arc<Vec<u8>>>,
    /// 是否放行远程图片。false 时换成隐藏占位并保留原尺寸。
    pub allow_remote: bool,
    pub theme: WebTheme,
    /// 信息条文本，调用方负责转义。
    pub meta: String,
    /// 窗口标题，调用方负责转义。
    pub title: String,
}

/// 渲染结果：一份自包含的宿主页 HTML + 被拦下的远程图数量。
pub struct RenderDoc {
    pub host: String,
    /// 被拦下的远程图数量。宿主页里已经画好提示条，这里保留给调用方做诊断/统计。
    #[allow(dead_code)]
    pub blocked: usize,
}

/// 唯一的公开入口：把邮件 HTML 变成可交给浏览器内核的完整文档。
pub fn build(req: &RenderRequest) -> RenderDoc {
    let html = inline_cid(req.html, req.images);
    // 清洗会删掉 <style> 标签，必须先从原始 HTML 提取邮件样式。
    let mail_css = extract_styles(&html);
    let body_style = body_attrs(&html);
    let (cleaned, blocked) = sanitize(&html, req.allow_remote);
    let host = wrap_document(
        &cleaned,
        &req.title,
        blocked,
        req.theme,
        &mail_css,
        &req.meta,
        &body_style,
    );
    RenderDoc { host, blocked }
}

pub fn mime_of(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.len() > 12 && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "image/png"
    }
}

/// 把 `cid:` 引用替换成 data URI，避免写临时图片文件。
pub fn inline_cid(html: &str, images: &HashMap<String, std::sync::Arc<Vec<u8>>>) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(index) = rest.to_ascii_lowercase().find("cid:") {
        out.push_str(&rest[..index]);
        let tail = &rest[index + 4..];
        let end = tail
            .find(|c: char| c == '"' || c == '\'' || c == ')' || c == '>' || c.is_whitespace())
            .unwrap_or(tail.len());
        let id = tail[..end].trim().trim_start_matches('<').trim_end_matches('>');
        let key = id.to_string();
        let bytes = images.get(&key).or_else(|| {
            images.iter().find(|(k, _)| k.eq_ignore_ascii_case(&key)).map(|(_, v)| v)
        });
        match bytes {
            Some(data) => {
                out.push_str("data:");
                out.push_str(mime_of(data));
                out.push_str(";base64,");
                out.push_str(&base64::engine::general_purpose::STANDARD.encode(data.as_ref()));
            }
            None => out.push_str("cid:"),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

fn style_regex() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)<style\b[^>]*>(.*?)</style>").unwrap())
}

fn img_regex() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)<img\b[^>]*>").unwrap())
}

fn body_regex() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)<body\b([^>]*)>").unwrap())
}

/// ammonia 会去掉 `<body>` 标签，连带邮件自己设的背景色/字体一起丢。
/// 这里把 body 上的安全属性提取出来，重新挂到生成文档的 body 上。
pub fn body_attrs(html: &str) -> String {
    let Some(caps) = body_regex().captures(html) else { return String::new() };
    let raw = &caps[1];
    let mut out = String::new();
    for (name, css) in [("bgcolor", "background-color"), ("background", "background-image")] {
        let re = Regex::new(&format!(r#"(?i)\b{name}\s*=\s*"([^"]*)""#)).unwrap();
        if let Some(value) = re.captures(raw).map(|c| c[1].to_string()) {
            if value.contains(['<', '>', '"']) || value.to_ascii_lowercase().contains("javascript:") { continue; }
            out.push_str(&format!(" {css}:{value};"));
        }
    }
    // style 属性单独安全过滤后并入。
    let style_re = Regex::new(r#"(?is)\bstyle\s*=\s*"([^"]*)""#).unwrap();
    if let Some(value) = style_re.captures(raw).map(|c| c[1].to_string()) {
        out.push(' ');
        out.push_str(&safe_css(&value));
        out.push(';');
    }
    out.chars().take(2048).collect()
}

/// 邮件 `<head><style>` 里的 class 规则是排版的主要来源；ammonia 会整块删除 style 标签，
/// 所以在清洗前先提取出来，做安全过滤后再注入生成的文档。
/// 只保留纯 CSS：去掉注释、`@import`、可执行表达式、DOM 标签与固定定位。
pub fn extract_styles(html: &str) -> String {
    let mut out = String::new();
    for caps in style_regex().captures_iter(html) {
        out.push_str(&safe_css(&caps[1]));
        out.push('\n');
    }
    out.chars().take(64 * 1024).collect()
}

/// 邮件里的远程装饰背景图（腾讯 `email_bg.png` 这类 780×235 白色信纸外壳）
/// 在深色主题下会变成一块突兀的白框，同时也是追踪像素之外的又一处外联请求。
/// 只删含远程 `url(...)` 的 background 声明，`data:` / `cid:` 内联图与纯色背景保留。
fn strip_remote_backgrounds(css: &str) -> String {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?i)\bbackground(?:-image)?\s*:\s*[^;}]*url\(\s*['"]?(?:https?:)?//[^)]*\)[^;}]*;?"#).unwrap()
    });
    re.replace_all(css, " ").into_owned()
}

pub fn safe_css(raw: &str) -> String {
    let mut s = raw.to_string();
    for (pattern, replacement) in [
        (r"(?is)/\*.*?\*/", " "),
        (r"(?i)@import[^;]*;?", " "),
        (r"(?i)expression\s*\([^)]*\)", " "),
        (r"(?i)-moz-binding\s*:[^;}]*;?", " "),
        (r"(?i)behavior\s*:[^;}]*;?", " "),
        (r"(?i)javascript\s*:", " "),
        (r"(?i)position\s*:\s*fixed", "position: static"),
        (r"(?i)</?style\b[^>]*>", " "),
    ] {
        if let Ok(re) = Regex::new(pattern) {
            s = re.replace_all(&s, replacement).into_owned();
        }
    }
    let s = strip_remote_backgrounds(&s);
    s.replace(['<', '>'], " ")
}

pub fn sanitize(html: &str, allow_remote: bool) -> (String, usize) {
    let tags: HashSet<&str> = [
        "table", "thead", "tbody", "tfoot", "tr", "td", "th", "caption", "colgroup", "col",
        "div", "span", "p", "br", "hr", "h1", "h2", "h3", "h4", "h5", "h6",
        "ul", "ol", "li", "blockquote", "pre", "code", "b", "strong", "i", "em", "u", "s",
        "small", "big", "font", "center", "a", "img", "sup", "sub",
    ]
    .into_iter()
    .collect();
    let mut generic: HashSet<&str> = [
        // `style` 放行：邮件按钮的绿底、标题字号、单元格 padding 都写在内联样式里。
        "style", "class", "align", "valign", "dir", "lang", "title",
        "width", "height", "bgcolor", "border", "cellpadding", "cellspacing", "colspan", "rowspan",
    ]
    .into_iter()
    .collect();
    generic.insert("color");
    let mut tag_attrs: HashMap<&str, HashSet<&str>> = HashMap::new();
    tag_attrs.insert("a", ["href", "target"].into_iter().collect());
    tag_attrs.insert("img", ["src", "alt", "width", "height", "border"].into_iter().collect());
    tag_attrs.insert("font", ["color", "size", "face"].into_iter().collect());

    let mut builder = ammonia::Builder::default();
    builder
        .tags(tags)
        .generic_attributes(generic)
        .tag_attributes(tag_attrs)
        .url_schemes(["http", "https", "mailto", "data"].into_iter().collect())
        .link_rel(Some("noopener noreferrer"));
    let mut cleaned = builder.clean(html).to_string();

    // 未授权远程图片时整枚 `<img>` 换成 1×1 透明像素并保留原 width/height：
    // 邮件常用 spacer 图撑间距，直接 display:none 会让整段布局塌掉。
    let blocked = if allow_remote {
        0
    } else {
        let (out, count) = replace_remote_images(&cleaned);
        cleaned = out;
        count
    };
    // 内联 style 里的远程背景图同样要去掉（<style> 块已由 safe_css 处理）。
    cleaned = strip_inline_remote_backgrounds(&cleaned);
    cleaned = cleaned.replace("<a ", "<a target=\"_blank\" ");
    // 邮件里常见 `src="//cdn.example/x.png"`；在 file:// 页面会被解析成本地路径而加载失败。
    cleaned = normalize_protocol_relative(&cleaned);
    (cleaned, blocked)
}

fn strip_inline_remote_backgrounds(html: &str) -> String {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"(?is)\bstyle\s*=\s*"([^"]*)""#).unwrap());
    re.replace_all(html, |caps: &regex::Captures| {
        let css = strip_remote_backgrounds(&caps[1]).replace('"', "&quot;");
        format!("style=\"{css}\"")
    })
    .into_owned()
}

/// 把协议相对的图片/背景 URL 补成 https，避免 file:// 预览里图片全空白。
fn normalize_protocol_relative(html: &str) -> String {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"(?i)\b(src|href)\s*=\s*"//"#).unwrap());
    re.replace_all(html, |caps: &regex::Captures| {
        format!("{}=\"https://", &caps[1])
    }).into_owned()
}

/// 返回（新 HTML，替换掉的远程图片张数）。只匹配真正的 `<img>`，纯链接邮件不会误报。
/// 被拦的图保留原 width/height/alt 占位，只是换成透明像素并隐藏：
/// 邮件常用 spacer 图撑间距，直接 display:none 会让整段布局塌掉。
pub fn replace_remote_images(html: &str) -> (String, usize) {
    const PIXEL: &str = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==";
    let mut replaced = 0usize;
    let out = img_regex().replace_all(html, |caps: &regex::Captures| {
        let tag = &caps[0];
        if !is_remote_img(tag) { return tag.to_string(); }
        replaced += 1;
        // 预览只在用户打开窗口时生成一次，这里按属性名现编译即可。
        let attr = |name: &str| -> String {
            Regex::new(&format!(r#"(?i)\b{name}\s*=\s*"([^"]*)""#))
                .ok()
                .and_then(|re| re.captures(tag).map(|c| c[1].replace('"', "&quot;")))
                .unwrap_or_default()
        };
        let width = attr("width");
        let height = attr("height");
        let alt = attr("alt");
        let size = match (width.is_empty(), height.is_empty()) {
            (false, false) => format!(" width=\"{width}\" height=\"{height}\""),
            (false, true) => format!(" width=\"{width}\""),
            (true, false) => format!(" height=\"{height}\""),
            (true, true) => " width=\"1\" height=\"1\"".to_string(),
        };
        format!("<img src=\"{PIXEL}\" alt=\"{alt}\"{size} class=\"dz-blocked\">")
    });
    (out.into_owned(), replaced)
}

fn is_remote_img(tag: &str) -> bool {
    let lower = tag.to_ascii_lowercase();
    [
        "src=\"http://", "src=\"https://", "src=\"//",
        "src='http://", "src='https://", "src='//",
        "src=http://", "src=https://", "src=//",
    ]
    .iter()
    .any(|p| lower.contains(p))
}

/// 宿主页样式：只作用于窗口 chrome（信息条、提示条、iframe 容器）。
/// 邮件正文放在独立 iframe 里，这里的任何规则都进不去邮件文档，
/// 邮件自带的 `body{}` / `h1{}` / `a{}` 也污染不到信息条。
/// 占位符用替换而非 format!，避免 CSS 花括号转义。
const HOST_CSS: &str = r#"
html,body{height:100%;margin:0;padding:0;}
body{background:__PAGE__;color:__INK__;
 font-family:system-ui,-apple-system,'Segoe UI','Microsoft YaHei UI','PingFang SC',sans-serif;
 display:flex;flex-direction:column;overflow:hidden;}
.dz-bar{flex:none;padding:9px 16px;background:__PAPER__;border-bottom:1px solid __LINE__;
 font:12px/1.5 system-ui,'Microsoft YaHei UI',sans-serif;color:__INK__;
 display:flex;gap:12px;align-items:baseline;flex-wrap:wrap;}
.dz-bar .dz-meta{opacity:.62;}
.dz-bar .dz-brand{margin-left:auto;opacity:.5;white-space:nowrap;}
.dz-note{flex:none;padding:7px 16px;background:__PAPER__;border-bottom:1px solid __LINE__;
 font:11.5px/1.5 system-ui,'Microsoft YaHei UI',sans-serif;color:__INK__;opacity:.8;}
iframe.dz-mail{flex:1;width:100%;border:0;vertical-align:top;background:__PAPER__;}
@media print{.dz-bar,.dz-note{display:none}iframe.dz-mail{height:100vh}}
"#;

/// iframe 内的最小兜底：给没写背景的纯文本邮件一张浅色信纸 + 深色字，并防图片横向溢出。
/// 邮件自己的 `<style>` 排在这段之后，永远能覆盖它；不再有 900px 版心、
/// `hr` / `blockquote` 重写、`display:block` 这类会压塌表格的规则。
const MAIL_BASE_CSS: &str = r#"
html,body{margin:0;padding:0;}
html{background:__MAILPAPER__;}
body{background:__MAILPAPER__;color:__READINK__;
 font-family:system-ui,-apple-system,'Segoe UI','Microsoft YaHei UI','PingFang SC',sans-serif;
 font-size:15px;line-height:1.6;-webkit-font-smoothing:antialiased;
 overflow-x:auto;}
img{max-width:100%;}
img.dz-blocked{visibility:hidden;}
pre{white-space:pre-wrap;word-break:break-word;}
__DARK_FILTER__
"#;

/// 深色主题：整篇邮件做一次「智能反色」。
///
/// 邮件正文里到处是硬编码的深色文字（`color:#24292f`、`bgcolor="#ffffff"` 这类），
/// 只把信纸换成深色，字会留在深底上直接看不见。整体反色把底色和文字一起翻过来，
/// 图片/视频/画布再反一次还原成原图。裸的 `filter:invert` 只作用在 `html` 上，
/// 不给表格/容器加反色，避免嵌套反色把品牌色翻两遍。
const DARK_FILTER: &str = "html{background:#fff;filter:invert(1) hue-rotate(180deg);}\
img,video,picture,svg,canvas{filter:hue-rotate(180deg) invert(1);}";

#[cfg(test)]
pub(crate) fn wrap(body: &str, title: &str, blocked_remote: usize, theme: WebTheme) -> String {
    wrap_document(body, title, blocked_remote, theme, "", "", "")
}

/// `mail_css` 是邮件自带的样式规则，放在阅读层之后，让邮件自己决定字号、按钮与配色。
pub fn wrap_document(
    body: &str,
    title: &str,
    blocked_remote: usize,
    theme: WebTheme,
    mail_css: &str,
    meta: &str,
    body_style: &str,
) -> String {
    let (page, paper, ink, line, _) = theme.palette();
    let theme = theme.resolve(false);
    // 邮件文档在 iframe 里始终按「白纸黑字」铺，深色主题由 `DARK_FILTER` 整体反色，
    // 这样邮件自带的 `color:#333` 也会一起翻成浅色，不会留在深底上看不见。
    let (read_bg, read_ink) = match theme {
        WebTheme::Paper | WebTheme::Dark => ("#ffffff", "#1f2328"),
        WebTheme::Sepia => ("#fbfaf7", "#26221c"),
        WebTheme::Follow => unreachable!("resolve 之后不再有 Follow"),
    };
    let host_css = HOST_CSS
        .replace("__PAGE__", page)
        .replace("__PAPER__", paper)
        .replace("__INK__", ink)
        .replace("__LINE__", line);
    let mail_base = MAIL_BASE_CSS
        .replace("__MAILPAPER__", read_bg)
        .replace("__READINK__", read_ink)
        .replace("__DARK_FILTER__", if theme == WebTheme::Dark { DARK_FILTER } else { "" });
    let banner = if blocked_remote > 0 {
        format!("<div class=\"dz-note\">已隐藏 {blocked_remote} 张远程图片；可在设置里开启「自动显示远程图片」，或回到岛仔点「显示远程图片」。</div>")
    } else {
        String::new()
    };
    // frame-src 必须显式放行：srcdoc 继承宿主页 CSP，'none' 会让整块正文变空白。
    let scheme = if blocked_remote > 0 {
        "default-src 'none'; img-src data:; font-src data:; style-src 'unsafe-inline'; script-src 'none'; object-src 'none'; frame-src 'self' data: blob:; connect-src 'none'; form-action 'none'; base-uri 'none'"
    } else {
        "default-src 'none'; img-src data: https: http:; font-src data:; style-src 'unsafe-inline'; script-src 'none'; object-src 'none'; frame-src 'self' data: blob:; connect-src 'none'; form-action 'none'; base-uri 'none'"
    };
    let meta = if meta.is_empty() { String::new() } else { format!("<span class=\"dz-meta\">{meta}</span>") };
    // body 属性里可能残留引号，先清掉，避免破坏 iframe 内 `<body style="...">`。
    let body_style = body_style.replace(['"', '\''], " ");
    // 邮件文档独立成篇：邮件 CSS 只在 iframe 内生效，宿主页 CSS 完全进不来。
    // `<base target="_blank">` 让正文里的链接走「新窗口」而不是把正文自己导航掉：
    // 内嵌阅读面板据此把点击转成系统浏览器（见 mail_panel::webview 的 NewWindowRequested）。
    let mail_doc = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><base target=\"_blank\">\
<style>{mail_base}</style><style>{mail_css}</style></head>\
<body style=\"{body_style}\">{body}</body></html>"
    );
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\">\
<meta http-equiv=\"Content-Security-Policy\" content=\"{scheme}\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>{title}</title><style>{host_css}</style></head><body>\
<div class=\"dz-bar\">{meta}<span class=\"dz-meta dz-brand\">完整排版 · 岛仔</span></div>\
{banner}<iframe class=\"dz-mail\" title=\"邮件正文\" \
sandbox=\"allow-popups allow-popups-to-escape-sandbox\" srcdoc=\"{srcdoc}\"></iframe>\
</body></html>",
        srcdoc = escape_attr(&mail_doc)
    )
}

/// 把整篇邮件文档塞进 `srcdoc` 属性：浏览器解析属性时会还原成原 HTML。
pub fn escape_attr(html: &str) -> String {
    html.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_images_are_counted_when_blocked() {
        let html = r#"<img src="https://track.example/p.gif" width="20" height="10"><a href="https://example.com">link</a>"#;
        let (replaced, count) = replace_remote_images(html);
        assert_eq!(count, 1);
        assert!(replaced.contains("data:image/gif;base64"));
        assert!(!replaced.contains("track.example"));
        assert!(replaced.contains("https://example.com"), "链接不能被当成图片");
        assert!(replaced.contains("width=\"20\""), "spacer 图要保留原占位尺寸，不能让布局塌掉");
        assert!(replaced.contains("height=\"10\""), "高度同样要保留");
        assert!(replaced.contains("dz-blocked"), "被拦的图应隐藏而不是撑出空白色块");
    }

    #[test]
    fn link_only_mail_does_not_trigger_the_banner() {
        let html = r#"<a href="https://example.com">read more</a>"#;
        let (_, blocked) = sanitize(html, false);
        assert_eq!(blocked, 0);
        assert!(!wrap("<p>hi</p>", "t", blocked, WebTheme::Paper).contains("已隐藏"));
    }

    #[test]
    fn sanitizer_removes_scripts_and_keeps_layout_tags() {
        let (cleaned, _) = sanitize(
            r##"<script>alert(1)</script><table bgcolor="#eeeeee"><tr><td style="color:red" onclick="x()">A</td></tr></table><iframe src="x"></iframe>"##,
            false,
        );
        assert!(!cleaned.contains("<script"), "{cleaned}");
        assert!(!cleaned.contains("onclick"), "{cleaned}");
        assert!(!cleaned.contains("<iframe"), "{cleaned}");
        assert!(cleaned.contains("<table"), "{cleaned}");
        assert!(cleaned.contains("style=\"color:red\""), "{cleaned}");
    }

    #[test]
    fn wrapped_document_locks_down_scripts_and_remote_images() {
        let blocked = wrap("<p>hi</p>", "t", 2, WebTheme::Paper);
        assert!(blocked.contains("script-src 'none'"));
        assert!(blocked.contains("img-src data:;"));
        assert!(blocked.contains("已隐藏 2 张"));
        let allowed = wrap("<p>hi</p>", "t", 0, WebTheme::Paper);
        assert!(allowed.contains("img-src data: https: http:;"));
    }

    #[test]
    fn mail_body_links_request_a_new_window() {
        let html = wrap("<p><a href=\"https://e.com\">e</a></p>", "t", 0, WebTheme::Paper);
        // 正文 iframe 里 CSP 禁了脚本，注入不了拦截代码；靠 <base target="_blank">
        // 把每次点击变成「新窗口请求」，内嵌面板才能在 NewWindowRequested 里接住它，
        // 转成系统浏览器打开，而不是把正文自己导航掉。
        assert!(html.contains("&lt;base target=&quot;_blank&quot;&gt;"), "{html}");
    }

    #[test]
    fn reading_layer_does_not_rewrite_mail_layout() {
        let html = wrap("<p>hi</p>", "t", 0, WebTheme::Paper);
        // 这三条曾把邮件表格压塌、把行内图标挤成竖排、给 logo 强加圆角。
        assert!(!html.contains("display:block"), "不能强制图片块级显示: {html}");
        assert!(!html.contains("border-collapse"), "不能改写邮件表格合并方式: {html}");
        assert!(!html.contains("!important"), "不能用 !important 压过邮件样式: {html}");
        assert!(!html.contains("max-width:900px"), "不能再给邮件套一层强制版心: {html}");
        assert!(html.contains("img{max-width:100%"), "仍要防止图片横向溢出");
        assert!(html.contains("srcdoc="), "邮件正文要放进独立 iframe");
        for theme in [WebTheme::Paper, WebTheme::Sepia, WebTheme::Dark] {
            let doc = wrap("<p>hi</p>", "t", 0, theme);
            let (page, _, _, _, _) = theme.palette();
            assert!(doc.contains(page), "主题底色没生效");
        }
        let dark = wrap("<p>hi</p>", "t", 0, WebTheme::Dark);
        assert!(dark.contains("filter:invert(1)"), "深色主题要整体反色，否则邮件硬编码的深色字看不见");
    }

    #[test]
    #[ignore="导出主题样例：DAOZAI_MAIL_THEME_PREVIEW=目录"]
    fn dump_theme_previews(){
        let Some(dest)=std::env::var_os("DAOZAI_MAIL_THEME_PREVIEW") else{return};
        let dest=std::path::PathBuf::from(dest);std::fs::create_dir_all(&dest).unwrap();
        // 覆盖三类真实邮件写法：正文默认色、硬编码深色、白底表格、内嵌图片。
        let html=r##"<div><h2>We will officially begin requiring 2FA</h2>
<p>You don't need to do anything in response to this email. <a href="https://github.com">our documentation</a></p>
<p style="color:#24292f">这段是邮件硬编码的深色文字（#24292f）</p>
<table bgcolor="#ffffff" width="100%"><tr><td style="padding:10px;color:#333">白底表格里的深色文字</td></tr></table>
<p><svg width="60" height="30" style="vertical-align:middle"><rect width="60" height="30" fill="#c0392b"/></svg> 红色块应该仍是红的</p>
<p><a href="https://github.com" style="display:inline-block;background:#2f6fed;color:#fff;padding:8px 18px;border-radius:8px;text-decoration:none">蓝色按钮白字</a></p>
</div>"##;
        for(name,theme)in[("paper",WebTheme::Paper),("dark",WebTheme::Dark)]{
            let doc=wrap_document(html,"GitHub 2FA",0,theme,"","GitHub · 11-08 03:13 · demo@example.test","");
            std::fs::write(dest.join(format!("{name}.html")),doc).unwrap();
        }
    }

    #[test]
    fn mail_css_is_isolated_inside_sandboxed_iframe() {
        // 网页客户端的核心做法：邮件 CSS 与宿主页 CSS 不在同一个命名空间。
        // 邮件里 `body{}` / `h1{}` 这类全局选择器不能再污染信息条，反之亦然。
        let css = "body{background:#111}h1{color:#e00}.cta{padding:12px}";
        let doc = wrap_document("<h1>标题</h1><div class=\"cta\">Go</div>", "t", 0,
            WebTheme::Paper, css, "发件人", "");
        let split = doc.find("<iframe").expect("应有 iframe");
        let (host, mail) = doc.split_at(split);
        assert!(host.contains("class=\"dz-bar\""), "信息条在宿主页: {host}");
        assert!(mail.starts_with("<iframe") && mail.contains("sandbox="), "iframe 要沙箱隔离: {mail}");
        assert!(mail.contains("srcdoc="), "邮件正文走 srcdoc: {mail}");
        assert!(mail.contains(".cta{padding:12px}"), "邮件 CSS 只在 iframe 内: {mail}");
        assert!(!host.contains(".cta{padding:12px}"), "邮件 CSS 不能污染宿主页 head: {host}");
        assert!(!host.contains("body{background:#111}"), "邮件 body 规则不能改宿主页底色: {host}");
        assert!(mail.contains("&lt;h1&gt;"), "srcdoc 要做属性转义: {mail}");
        assert!(!mail.contains("<h1>"), "未转义的标签会提前闭合宿主页: {mail}");
        assert!(doc.contains("frame-src 'self'"), "srcdoc 需要显式放行 frame-src");
    }

    #[test]
    fn dark_theme_inverts_the_whole_mail_paper() {
        // 浅色主题：信纸是浅底 + 深字，正文直接可读。
        for theme in [WebTheme::Paper, WebTheme::Sepia] {
            let doc = wrap_document("<h1>账号登录提醒</h1>", "t", 0, theme, "", "meta", "");
            assert!(!doc.contains("__READ"), "占位符必须全部替换: {doc}");
            let mail = &doc[doc.find("<iframe").unwrap()..];
            assert!(mail.contains("background:#ffffff") || mail.contains("background:#fbfaf7"), "iframe 内是浅色信纸: {mail}");
            assert!(mail.contains("color:#1f2328") || mail.contains("color:#26221c"), "iframe 内文字是深色: {mail}");
            assert!(!mail.contains("invert(1)"), "浅色主题不能反色: {mail}");
        }
        // 深色主题：整篇按白纸渲染再反色，邮件自带的深色文字才会翻成浅色；
        // 图片单独再反一次还原，不会被反成底片。
        let doc = wrap_document("<h1>账号登录提醒</h1>", "t", 0, WebTheme::Dark, "", "meta", "");
        let mail = &doc[doc.find("<iframe").unwrap()..];
        assert!(mail.contains("filter:invert(1)"), "深色主题要反色整篇正文: {mail}");
        assert!(mail.contains("img,video,picture,svg,canvas{filter:hue-rotate(180deg) invert(1)"), "图片要再反一次还原: {mail}");
    }

    #[test]
    fn original_mail_css_is_injected_without_dangerous_parts() {
        let raw = r#"<html><head><style>
@import url(https://tracker.example/a.css);
.cta{background:#2f6fed;color:#fff;padding:12px 22px;border-radius:10px}
.h1{font-size:26px;font-weight:700}
body{position:fixed;behavior:url(x)}
</style></head><body><div class="cta">Go</div></body></html>"#;
        let css = extract_styles(raw);
        assert!(css.contains(".cta"), "邮件 class 样式必须保留: {css}");
        assert!(css.contains("font-size:26px"), "标题字号必须保留: {css}");
        assert!(!css.to_ascii_lowercase().contains("@import"), "不能加载远程样式表: {css}");
        assert!(!css.to_ascii_lowercase().contains("behavior"), "不能保留 behavior: {css}");
        assert!(!css.to_ascii_lowercase().contains("position:fixed"), "固定定位会遮挡正文: {css}");
        let doc = wrap_document("<div class=\"cta\">Go</div>", "t", 0, WebTheme::Paper, &css, "meta", "");
        assert!(doc.contains("font-size:26px"), "样式要真正注入文档");
        assert!(doc.contains("meta"), "信息条要带邮件元信息");
        assert!(doc.contains("完整排版 · 岛仔"));
    }

    #[test]
    fn body_background_survives_sanitizing() {
        let raw = r##"<body bgcolor="#eef1f6" style="margin:0;padding:0;color:#222"><div>hi</div></body>"##;
        let style = body_attrs(raw);
        assert!(style.contains("background-color:#eef1f6"), "邮件自己的底色不能丢: {style}");
        assert!(style.contains("color:#222"), "body 上的字体色要保留: {style}");
        let clean = body_attrs(r#"<body bgcolor="foo" onload="alert(1)" style="color:red"><p>x</p></body>"#);
        assert!(!clean.contains("onload"), "不能把事件属性带进文档: {clean}");
    }

    #[test]
    fn cid_images_become_data_uris() {
        let mut images = HashMap::new();
        images.insert("logo".to_string(), std::sync::Arc::new(vec![0x89, b'P', b'N', b'G', 1, 2, 3]));
        let html = r#"<img src="cid:logo" width="40">"#;
        let out = inline_cid(html, &images);
        assert!(out.contains("data:image/png;base64,"), "{out}");
        assert!(!out.contains("cid:logo"), "{out}");
    }

    #[test]
    fn remote_decorative_background_is_stripped() {
        // 腾讯云登录提醒邮件：`.open_email` 用一张 780×235 的远程白底信纸图做外壳，
        // 在深色主题下就是截图里那块突兀的白框；同时它也是一次外联请求。
        let css = r#".open_email{background:url(http://imgcache.qq.com/bossweb/pay/images/mailmsg/email_bg.png) no-repeat 0 -35px;width:760px;padding:10px;margin:0 auto;}
.email_btn{background:url(https://cdn.example/btn.png) no-repeat 0 0;width:84px;}
.keep{background-color:#f7f8fa;background-image:url(data:image/png;base64,AAA);color:#333}"#;
        let clean = safe_css(css);
        assert!(!clean.contains("imgcache.qq.com"), "远程装饰图必须删掉: {clean}");
        assert!(!clean.contains("cdn.example"), "https 远程图同样要删: {clean}");
        assert!(clean.contains("width:760px"), "其余布局样式不能丢: {clean}");
        assert!(clean.contains("background-color:#f7f8fa"), "纯色背景要保留: {clean}");
        assert!(clean.contains("data:image/png"), "内联背景图要保留: {clean}");

        let (sanitized, _) = sanitize(
            r##"<div style="background:url(https://cdn.example/paper.png) no-repeat;width:760px;color:#333">正文</div>"##,
            true,
        );
        assert!(!sanitized.contains("cdn.example"), "内联远程背景图必须删掉: {sanitized}");
        assert!(sanitized.contains("color:#333"), "其它内联样式不能丢: {sanitized}");
    }

    #[test]
    fn inline_button_styles_survive_sanitizing() {
        // 真实邮件（NVIDIA / GitHub）把按钮底色、内边距、圆角都写在内联 style 上。
        // 清洗若把它们整段删掉，绿底按钮会退化成一条纯色块或裸文字。
        let raw = r##"<td bgcolor="#76b900"><a href="https://example.com/verify" style="color:#000000;background-color:#1f883d;text-decoration:none;padding:12px 20px;border-radius:6px">验证电子邮箱地址</a></td>"##;
        let (clean, blocked) = sanitize(raw, true);
        assert_eq!(blocked, 0);
        assert!(clean.contains("background-color"), "按钮底色必须保留: {clean}");
        assert!(clean.contains("border-radius"), "按钮圆角必须保留: {clean}");
        assert!(clean.contains("padding"), "按钮内边距必须保留: {clean}");
        assert!(!clean.to_ascii_lowercase().contains("onclick"), "事件属性不能保留: {clean}");
    }
}
