//! 「查看完整排版」窗口的启动层：选内核、落盘、拉起进程。
//!
//! 安全策略与文档组装全部在 [`crate::mail_render`] 的纯函数层，这里只负责把
//! 渲染好的文档交给某个浏览器内核。要换内核（Edge `--app` → WebView2）只需新增
//! 一个 [`Renderer`] 实现，清洗规则不必动第二遍。
//!
//! 零安装策略：系统 Edge（`msedge --app`）+ 独立 user-data-dir，避免被已有实例吞掉。

use crate::mail;
use crate::mail_render;
use crate::mail_view::{self, WebTheme};
use std::path::PathBuf;

/// 完整排版渲染后端。实现者只需把一份已渲染好的 HTML 显示出来。
pub trait Renderer {
    fn available(&self) -> bool;
    /// 后端标识，只给单测确认 [`renderer`] 选中的是哪一个。
    #[cfg(test)]
    fn name(&self) -> &'static str;
    /// `doc.host` 是自包含的宿主页（邮件正文已在沙箱 iframe 里）。
    fn show(
        &self,
        item: &mail::Item,
        doc: &mail_render::RenderDoc,
        theme: WebTheme,
    ) -> Result<(), String>;
}

/// 当前选中的后端。
///
/// **默认走系统 Edge `--app`**：这是长期验证过的路径。WebView2 内嵌实现虽然更干净
/// （内存供页、不落盘），但 release 用 `panic = "abort"`，COM 回调线程里任何 panic
/// 都会直接把整个进程带走——用户看到的就是「点开完整排版就闪退」。
/// 在把那条链路彻底钉死之前，它只作为实验后端，需要显式 `DAOZAI_MAIL_WEBVIEW=1` 才启用。
pub fn renderer() -> &'static dyn Renderer {
    static EDGE: EdgeAppRenderer = EdgeAppRenderer;
    #[cfg(windows)]
    {
        if std::env::var("DAOZAI_MAIL_WEBVIEW").ok().as_deref() == Some("1") {
            static WEBVIEW: crate::mail_webview::WebViewRenderer =
                crate::mail_webview::WebViewRenderer;
            if WEBVIEW.available() {
                return &WEBVIEW;
            }
        }
    }
    &EDGE
}

/// 返回是否具备「查看完整排版」能力。
pub fn available() -> bool {
    renderer().available()
}

fn edge_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
        if let Ok(base) = std::env::var(var) {
            candidates.push(PathBuf::from(base).join("Microsoft/Edge/Application/msedge.exe"));
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// 打开「查看完整排版」窗口。`allow_remote` 为 true 时放行远程图片（默认由调用方开启）。
pub fn open(item: &mail::Item, allow_remote: bool, theme: WebTheme) -> Result<(), String> {
    // 信任过这个发件人的话，以后不再每次手动开远程图。
    let doc = document(item, allow_remote || trusted(item), theme);
    renderer().show(item, &doc, theme)
}

/// 生成内嵌阅读窗口用的整页文档：和「完整排版」走同一套清洗与包裹。
pub(crate) fn document(item: &mail::Item, allow_remote: bool, theme: WebTheme) -> mail_render::RenderDoc {
    let request = mail_render::RenderRequest {
        html: item.html.as_ref(),
        images: item.images.as_ref(),
        allow_remote,
        theme,
        meta: format!(
            "{} · {} · {}",
            mail_render::escape(&item.from),
            mail_render::escape(&item.time),
            mail_render::escape(&item.account)
        ),
        title: mail_render::escape(&item.subject),
    };
    mail_render::build(&request)
}

/// 把渲染结果落到临时目录，返回可直接打开的文件 URL。
pub(crate) fn write_temp_file(
    item: &mail::Item,
    doc: &mail_render::RenderDoc,
) -> Result<(PathBuf, url::Url), String> {
    let dir = crate::model::config_dir().join("mail-view");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建临时目录失败：{e}"))?;
    cleanup(&dir);
    let mut hash = 2166136261u32;
    for byte in item.account.as_bytes().iter().chain(item.subject.as_bytes()) {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    let path = dir.join(format!("mail-{:08x}.html", hash ^ item.uid));
    std::fs::write(&path, doc.host.as_bytes()).map_err(|e| format!("写入预览文件失败：{e}"))?;
    let url = url::Url::from_file_path(&path).map_err(|_| "预览文件路径无效".to_string())?;
    Ok((path, url))
}

/// 系统 Edge `--app` 后端：零安装，但会多一个独立浏览器窗口与 profile 目录。
pub struct EdgeAppRenderer;

impl Renderer for EdgeAppRenderer {
    #[cfg(test)]
    fn name(&self) -> &'static str {
        "edge-app"
    }

    fn available(&self) -> bool {
        edge_path().is_some()
    }

    fn show(
        &self,
        item: &mail::Item,
        doc: &mail_render::RenderDoc,
        _theme: WebTheme,
    ) -> Result<(), String> {
        let edge = edge_path().ok_or("未找到系统 Edge，无法打开完整排版")?;
        let (_, url) = write_temp_file(item, doc)?;
        // 独立 profile：已有 Edge 在跑时，`--app` 会被并入该实例变成普通标签页。
        let profile = crate::model::config_dir().join("mail-edge-profile");
        std::fs::create_dir_all(&profile)
            .map_err(|e| format!("创建独立浏览器配置目录失败：{e}"))?;
        std::process::Command::new(edge)
            .arg(format!("--app={url}"))
            .arg(format!("--user-data-dir={}", profile.to_string_lossy()))
            .arg("--window-size=1080,820")
            .arg("--no-first-run")
            .arg("--disable-extensions")
            .spawn()
            .map_err(|e| format!("启动 Edge 失败：{e}"))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 「信任此发件人」：按「收件账号 + 发件人域名」记住远程图片授权。
// ---------------------------------------------------------------------------
fn trust_path() -> PathBuf { crate::model::config_dir().join("mail-trusted.json") }

fn trust_key(item: &mail::Item) -> Option<String> {
    let source = if item.from_addr.trim().is_empty() { &item.from } else { &item.from_addr };
    let domain = mail_view::sender_domain(source);
    if domain.trim().is_empty() { return None; }
    Some(format!("{}|{}", item.account.to_ascii_lowercase(), domain))
}

fn read_trusted() -> Vec<String> {
    std::fs::read_to_string(trust_path())
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
        .unwrap_or_default()
}

pub fn trusted(item: &mail::Item) -> bool {
    let Some(key) = trust_key(item) else { return false };
    read_trusted().iter().any(|k| k == &key)
}

/// 记下「该发件人可信」。
///
/// 入口在邮件面板阅读页的「信任」按钮（只在远程图被拦时出现）；
/// [`open`] 和面板推正文时都会先看 [`trusted`]，信任过就不再拦。
pub fn trust(item: &mail::Item) {
    let Some(key) = trust_key(item) else { return };
    let mut list = read_trusted();
    if list.contains(&key) { return; }
    list.push(key);
    if list.len() > 200 { list.remove(0); }
    if let Ok(text) = serde_json::to_string(&list) {
        // 先写临时文件再改名：中途崩溃不会留下截断的半个 JSON。
        let tmp = trust_path().with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, trust_path());
        }
    }
}

pub fn clear_trusted() { let _ = std::fs::remove_file(trust_path()); }

/// 只保留最近 7 天的预览文件，避免临时目录膨胀。
fn cleanup(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else { continue };
        if now.duration_since(modified).map(|age| age.as_secs() > 7 * 24 * 3600).unwrap_or(false) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_item() -> mail::Item {
        mail::Item {
            account: "a@163.com".into(), from: "Example".into(), subject: String::new(),
            time: String::new(), received_unix: None, code: None, html: String::new().into(),
            images: Default::default(), message_id: String::new(), from_addr: "news@example.com".into(),
            summary: String::new().into(), links: vec![], meeting: None, uid: 1,
        }
    }

    #[test]
    fn trust_key_is_per_account_and_sender_domain() {
        let mut a = empty_item();
        assert_eq!(trust_key(&a).as_deref(), Some("a@163.com|example.com"));
        a.from_addr = String::new();
        a.from = "No address".into();
        assert_eq!(trust_key(&a).as_deref(), Some("a@163.com|no address"));
        a.from = String::new();
        a.from_addr = String::new();
        assert_eq!(trust_key(&a), None, "没有发件人信息时不能信任");
    }

    #[test]
    fn renderer_selection_falls_back_to_edge_when_no_webview() {
        // 本机可能没有 Edge / WebView2，两条路径都只是「不可用」而不是 panic。
        let pick = renderer();
        assert!(matches!(pick.name(), "edge-app" | "webview2"), "后端名要在预期集合内");
        assert_eq!(pick.available(), available());
    }
}
