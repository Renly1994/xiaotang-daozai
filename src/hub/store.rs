//! 账号配置：%USERPROFILE%\claude-hub\profiles.json。
//! 整份按 JSON 读写，不认识的字段原样保留，可以和使用同一格式的其他工具交替使用。不含任何密码或令牌。
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default()
}

/// 数据目录；测试时可用 CLAUDE_HUB_DIR 指到临时目录
pub fn hub_dir() -> PathBuf {
    std::env::var_os("CLAUDE_HUB_DIR").map(PathBuf::from).unwrap_or_else(|| home().join("claude-hub"))
}

fn file() -> PathBuf {
    hub_dir().join("profiles.json")
}

/// 本机是否已有多账号数据
pub fn has_data() -> bool {
    file().is_file()
}

pub fn defaults() -> Value {
    json!({
        "version": 2, "hubDir": hub_dir(), "current": null, "currentCodex": null,
        "accounts": [], "keys": [], "switches": [],
        "settings": {"autoBackup": true, "keepBackups": 10, "remind": true, "checkCleanup": true, "autoDetect": true, "autoFollow": true, "statusline": false, "autoFill": true}
    })
}

pub fn read_json(path: &Path) -> Option<Value> {
    let raw = fs::read(path).ok()?;
    serde_json::from_slice(raw.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&raw)).ok()
}

pub fn load() -> Value {
    let mut v = read_json(&file()).unwrap_or_else(defaults);
    if !v.is_object() {
        v = defaults();
    }
    for key in ["accounts", "switches", "keys"] {
        if !v[key].is_array() {
            v[key] = json!([]);
        }
    }
    if !v["settings"].is_object() {
        v["settings"] = defaults()["settings"].clone();
    }
    v
}

pub fn write_atomic(path: &Path, raw: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("island-tmp");
    fs::write(&tmp, raw)?;
    fs::rename(&tmp, path)
}

pub fn save(v: &Value) -> Result<(), String> {
    let raw = serde_json::to_vec_pretty(v).map_err(|_| "配置无法保存".to_string())?;
    write_atomic(&file(), &raw).map_err(|e| format!("配置无法保存：{e}"))
}

/// 读改写一次；f 返回 Err 时不保存
pub fn mutate<T>(f: impl FnOnce(&mut Value) -> Result<T, String>) -> Result<T, String> {
    let mut v = load();
    let r = f(&mut v)?;
    save(&v)?;
    Ok(r)
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn ymd(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string()).unwrap_or_default()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Claude,
    Codex,
}

impl Group {
    pub fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
        }
    }
}

/// 界面用的账号资料（从 JSON 里取，不改原数据）
#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub id: String,
    pub group: Group,
    pub kind: String,
    pub email: String,
    pub mail_url: String,
    pub plan: String,
    pub reg_date: String,
    pub note: String,
    pub status: String,
    pub status_at: String,
    pub account_uuid: Option<String>,
    pub config_dir: Option<PathBuf>,
    pub quota_dir: Option<PathBuf>,
    pub color: Option<[u8; 3]>,
    pub pinned: bool,
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().trim().to_owned()
}

fn color(v: &Value) -> Option<[u8; 3]> {
    let raw = v.as_str()?;
    if let Some(hex) = raw.strip_prefix('#') {
        let n = u32::from_str_radix(hex, 16).ok()?;
        return (hex.len() == 6).then(|| [(n >> 16) as u8, (n >> 8) as u8, n as u8]);
    }
    let parts: Vec<u8> = raw.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    (parts.len() == 3).then(|| [parts[0], parts[1], parts[2]])
}

pub fn accounts(v: &Value) -> Vec<Account> {
    v["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(i, a)| {
            let id = a["id"].as_str()?.to_owned();
            let group = if a["group"].as_str() == Some("codex") { Group::Codex } else { Group::Claude };
            let path = |k: &str| a[k].as_str().filter(|p| !p.trim().is_empty()).map(PathBuf::from);
            let status = match a["status"].as_str() {
                Some("disabled") => "disabled",
                Some("banned") => "banned",
                _ => "active",
            };
            Some(Account {
                group,
                kind: a["kind"].as_str().unwrap_or(if group == Group::Codex { "default" } else { "desktop" }).to_owned(),
                email: s(a, "email"),
                mail_url: s(a, "mailUrl"),
                plan: s(a, "plan"),
                reg_date: s(a, "regDate"),
                note: s(a, "note"),
                status: status.into(),
                status_at: s(a, "statusAt"),
                account_uuid: a["accountUuid"].as_str().filter(|u| u.len() == 36).map(str::to_owned),
                config_dir: path("configDir"),
                quota_dir: path("quotaDir"),
                color: color(&a["color"]),
                // 没设过的话，前三个账号常驻刘海
                pinned: a["pinned"].as_bool().unwrap_or(i < 3),
                id,
            })
        })
        .collect()
}

pub fn account_mut<'a>(v: &'a mut Value, id: &str) -> Result<&'a mut Value, String> {
    v["accounts"].as_array_mut().and_then(|list| list.iter_mut().find(|a| a["id"].as_str() == Some(id))).ok_or_else(|| "没有这个账号".to_string())
}

pub fn same_email(a: &str, b: &str) -> bool {
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 40 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// 只记文件名、数量和结果，不记会话内容
pub fn write_log(name: &str, entry: Value) {
    let dir = hub_dir().join("logs");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let mut line = entry.to_string();
    line.push('\n');
    use std::io::Write;
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir.join(name)) {
        let _ = f.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accounts_keep_unknown_fields_and_parse_colors() {
        let v = json!({"accounts":[
            {"id":"a","group":"claude","kind":"cli","color":"28,78,216","configDir":"C:\\x","accountUuid":"11111111-2222-4333-8444-555555555555","extra":1},
            {"id":"b","group":"codex","color":"#ff8000","status":"banned","pinned":false}
        ]});
        let list = accounts(&v);
        assert_eq!(list[0].color, Some([28, 78, 216]));
        assert_eq!(list[1].color, Some([255, 128, 0]));
        assert_eq!(list[1].status, "banned");
        assert!(list[0].pinned && !list[1].pinned);
        assert_eq!(list[0].config_dir.as_deref(), Some(Path::new("C:\\x")));
        let mut v2 = v.clone();
        account_mut(&mut v2, "a").unwrap()["pinned"] = json!(false);
        assert_eq!(v2["accounts"][0]["extra"], 1, "不认识的字段要原样保留");
    }
    #[test]
    fn names_are_restricted() {
        assert!(valid_name("work-2"));
        assert!(!valid_name("工作"));
        assert!(!valid_name("a b"));
    }
}
