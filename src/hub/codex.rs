//! Codex 额度：只读 Codex 自己会话记录里的 rate_limits 字段，不读 auth.json，也不读对话正文。
use super::store::{self, Account};
use super::usage::{Limits, Win};
use serde_json::Value;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const TAIL: u64 = 512 * 1024;

/// default 账号用当前的 CODEX_HOME；cli 账号有自己独立的 CODEX_HOME
pub fn home_of(a: &Account) -> PathBuf {
    if a.kind == "cli" {
        return a.config_dir.clone().unwrap_or_else(|| store::home().join(".codex-accounts").join(&a.id));
    }
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| store::home().join(".codex"))
}

fn ms(v: &Value) -> Option<i64> {
    let n = v.as_f64()?;
    Some(if n > 1e12 { n as i64 } else { (n * 1000.0) as i64 })
}

/// 一行 token_count 事件里的额度
pub fn parse_line(line: &str) -> Option<Limits> {
    let j: Value = serde_json::from_str(line).ok()?;
    let p = if j["payload"].is_object() { &j["payload"] } else { &j };
    let rl = &p["rate_limits"];
    if !rl.is_object() {
        return None;
    }
    let at = j["timestamp"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|d| d.timestamp_millis()).unwrap_or(0);
    let win = |w: &Value| -> Option<(Option<i64>, Win)> {
        let used = w["used_percent"].as_f64()? as f32;
        let mut reset = ms(&w["resets_at"]);
        if reset.is_none() && at > 0 {
            reset = w["resets_in_seconds"].as_f64().map(|s| at + (s * 1000.0) as i64);
        }
        Some((w["window_minutes"].as_i64(), Win { used, resets_at: reset }))
    };
    let windows: Vec<_> = [win(&rl["primary"]), win(&rl["secondary"])].into_iter().flatten().collect();
    if windows.is_empty() {
        return None;
    }
    let w5 = windows.iter().find(|w| w.0 == Some(300)).map(|w| w.1);
    let week = windows.iter().find(|w| w.0 == Some(10080)).or_else(|| windows.iter().rev().find(|w| w.0 != Some(300))).map(|w| w.1);
    Some(Limits { at, w5, week, plan: rl["plan_type"].as_str().unwrap_or_default().to_owned() })
}

fn last_in(file: &Path) -> Option<Limits> {
    let mut f = fs::File::open(file).ok()?;
    let size = f.metadata().ok()?.len();
    let len = size.min(TAIL);
    f.seek(SeekFrom::Start(size - len)).ok()?;
    let mut buf = Vec::with_capacity(len as usize);
    f.take(len).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    text.lines().rev().filter(|l| l.contains("\"rate_limits\"")).find_map(parse_line)
}

fn walk(dir: &Path, out: &mut Vec<(std::time::SystemTime, PathBuf)>, depth: usize) {
    if depth > 5 {
        return;
    }
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            walk(&e.path(), out, depth + 1);
        } else if e.file_name().to_string_lossy().ends_with(".jsonl") {
            if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                out.push((m, e.path()));
            }
        }
    }
}

/// 最近几次会话里最新的一条官方额度
pub fn latest(home: &Path) -> Option<Limits> {
    let mut files = Vec::new();
    walk(&home.join("sessions"), &mut files, 0);
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.iter().take(8).find_map(|f| last_in(&f.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn token_count_line_gives_week_and_five_hour() {
        let line = r#"{"timestamp":"2030-01-01T00:00:00Z","payload":{"type":"token_count","rate_limits":{"plan_type":"pro","primary":{"used_percent":12.5,"window_minutes":300,"resets_in_seconds":60},"secondary":{"used_percent":4,"window_minutes":10080,"resets_at":1900000000}}}}"#;
        let l = parse_line(line).unwrap();
        assert_eq!(l.w5.unwrap().used, 12.5);
        assert_eq!(l.w5.unwrap().resets_at, Some(1_893_456_060_000));
        assert_eq!(l.week.unwrap().resets_at, Some(1_900_000_000_000));
        assert_eq!(l.plan, "pro");
        assert!(parse_line(r#"{"payload":{"rate_limits":{}}}"#).is_none());
    }
}
