//! 读取本地 JSON 简报，供岛上的提醒卡片使用。
//!
//! 只读文件。不联网，不监听端口，不执行外部命令。
//! 数据契约见 PR 说明；解析失败一律返回 `Malformed`，不 panic。
//!
//! 说明：本文件假设仓库已有 `serde_json` 依赖（配置文件解析已用到）。
//! 若 `Cargo.toml` 里没有，把 `parse` 换成手写扫描，其余不变。

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

pub const MAX_FILE_BYTES: u64 = 64 * 1024;
pub const MAX_LINES: usize = 3;
pub const DEFAULT_TTL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Attention,
    Info,
    Done,
}

impl Severity {
    fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "attention" => Self::Attention,
            "done" => Self::Done,
            _ => Self::Info,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Brief {
    pub key: String,
    pub title: String,
    pub lines: Vec<String>,
    pub severity: Severity,
    pub action: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOutcome {
    Ready(Brief),
    Missing,
    Stale,
    Malformed(String),
}

fn str_field(obj: &serde_json::Value, name: &str) -> Option<String> {
    obj.get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn parse(raw: &str) -> Result<Brief, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("JSON 解析失败: {e}"))?;
    let obj = value.as_object().ok_or("顶层必须是对象")?;

    let key = str_field(&value, "key").ok_or("缺少 key")?;
    let title = str_field(&value, "title").unwrap_or_else(|| key.clone());

    let mut lines = Vec::new();
    if let Some(arr) = obj.get("lines").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(text) = item.as_str() {
                let text = text.trim();
                if !text.is_empty() {
                    lines.push(text.to_string());
                }
                if lines.len() == MAX_LINES {
                    break;
                }
            }
        }
    }

    let severity = str_field(&value, "severity")
        .map(|s| Severity::parse(&s))
        .unwrap_or(Severity::Info);

    // 只放行 http(s)。执行命令不在这个槽位的范围内。
    let action = str_field(&value, "action").filter(|s| {
        s.starts_with("https://") || s.starts_with("http://")
    });

    Ok(Brief {
        key,
        title,
        lines,
        severity,
        action,
        updated_at: str_field(&value, "updated_at"),
    })
}

pub fn read(path: &Path, ttl: Duration) -> ReadOutcome {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return ReadOutcome::Missing,
    };
    if meta.len() > MAX_FILE_BYTES {
        return ReadOutcome::Malformed(format!("文件超过 {MAX_FILE_BYTES} 字节"));
    }
    if let Ok(modified) = meta.modified() {
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default();
        if age > ttl {
            return ReadOutcome::Stale;
        }
    }
    match fs::read_to_string(path) {
        Ok(raw) => match parse(&raw) {
            Ok(brief) => ReadOutcome::Ready(brief),
            Err(e) => ReadOutcome::Malformed(e),
        },
        Err(e) => ReadOutcome::Malformed(format!("读取失败: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(tag: &str, body: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "daozai-brief-{tag}-{}.json",
            std::process::id()
        ));
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn parses_a_complete_brief() {
        let raw = r#"{
            "key": "local.inbox-brief",
            "title": "收件箱 · 5 待处理",
            "lines": ["5 封待你决定", "3 件待办", "其余 7 封可忽略"],
            "severity": "attention",
            "action": "http://127.0.0.1:5199",
            "updated_at": "2026-11-20T08:50:12+08:00"
        }"#;
        let brief = parse(raw).unwrap();
        assert_eq!(brief.key, "local.inbox-brief");
        assert_eq!(brief.lines.len(), 3);
        assert_eq!(brief.severity, Severity::Attention);
        assert_eq!(brief.action.as_deref(), Some("http://127.0.0.1:5199"));
    }

    #[test]
    fn rejects_missing_key() {
        assert!(parse(r#"{"title":"x"}"#).is_err());
    }

    #[test]
    fn keeps_at_most_three_lines_and_skips_blanks() {
        let raw = r#"{"key":"k","lines":["一","","二","三","四"]}"#;
        let brief = parse(raw).unwrap();
        assert_eq!(brief.lines, vec!["一", "二", "三"]);
    }

    #[test]
    fn drops_non_http_action() {
        let raw = r#"{"key":"k","action":"cmd.exe /c del *"}"#;
        assert!(parse(raw).unwrap().action.is_none());
    }

    #[test]
    fn reports_missing_file() {
        let path = std::env::temp_dir().join("daozai-brief-does-not-exist.json");
        let _ = fs::remove_file(&path);
        assert_eq!(read(&path, DEFAULT_TTL), ReadOutcome::Missing);
    }

    #[test]
    fn reports_stale_by_mtime() {
        let path = temp_file("stale", r#"{"key":"k"}"#);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(read(&path, Duration::from_millis(1)), ReadOutcome::Stale);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn reports_malformed_json() {
        let path = temp_file("bad", "{ not json");
        match read(&path, DEFAULT_TTL) {
            ReadOutcome::Malformed(_) => {}
            other => panic!("expected Malformed, got {other:?}"),
        }
        let _ = fs::remove_file(&path);
    }
}
