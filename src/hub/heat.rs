//! 近 4 周每天的 token 用量（详情页的热力图）。
//! 只取 assistant 行里的 usage 数字和时间，不读也不保存正文；文件没变就用缓存。
use super::store::{self, Account, Group};
use super::{codex, usage};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const DAYS: usize = 28;
const DAY_MS: i64 = 86_400_000;

#[derive(Clone)]
struct Event {
    at: i64,
    total: u64,
    key: String,
}

#[derive(Default)]
pub struct Cache {
    files: HashMap<PathBuf, (u64, SystemTime, Vec<Event>)>,
}

fn jsonl_files(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 6 {
        return;
    }
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            jsonl_files(&e.path(), out, depth + 1);
        } else if e.file_name().to_string_lossy().ends_with(".jsonl") {
            out.push(e.path());
        }
    }
}

fn time_of(v: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(v["timestamp"].as_str()?).ok().map(|d| d.timestamp_millis())
}

fn claude_line(line: &str) -> Option<Event> {
    if !line.contains("\"usage\"") || !line.contains("\"assistant\"") {
        return None;
    }
    let o: Value = serde_json::from_str(line).ok()?;
    if o["type"].as_str() != Some("assistant") {
        return None;
    }
    let u = &o["message"]["usage"];
    if !u.is_object() {
        return None;
    }
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    let key = format!("{}:{}", o["message"]["id"].as_str().or_else(|| o["uuid"].as_str()).unwrap_or_default(), o["requestId"].as_str().unwrap_or_default());
    Some(Event { at: time_of(&o)?, total: n("input_tokens") + n("output_tokens") + n("cache_creation_input_tokens"), key })
}

fn codex_line(line: &str) -> Option<Event> {
    if !line.contains("\"last_token_usage\"") {
        return None;
    }
    let o: Value = serde_json::from_str(line).ok()?;
    let info = &o["payload"]["info"];
    let u = &info["last_token_usage"];
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    // 同一个累计值重复出现时只算一次
    let key = format!("codex:{}", info["total_token_usage"]["total_tokens"].as_u64().unwrap_or(0));
    Some(Event { at: time_of(&o)?, total: n("input_tokens").saturating_sub(n("cached_input_tokens")) + n("output_tokens"), key })
}

impl Cache {
    fn events(&mut self, root: &Path, since: i64, parse: fn(&str) -> Option<Event>) -> Vec<Event> {
        let mut files = Vec::new();
        jsonl_files(root, &mut files, 0);
        let mut out = Vec::new();
        let since_time = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(since.max(0) as u64);
        for file in files {
            let Ok(meta) = fs::metadata(&file) else { continue };
            let Ok(mtime) = meta.modified() else { continue };
            if mtime < since_time {
                continue;
            }
            let fresh = self.files.get(&file).is_some_and(|c| c.0 == meta.len() && c.1 == mtime);
            if !fresh {
                let Ok(f) = fs::File::open(&file) else { continue };
                let events: Vec<Event> = BufReader::new(f).lines().map_while(Result::ok).filter_map(|l| parse(&l)).collect();
                self.files.insert(file.clone(), (meta.len(), mtime, events));
            }
            if let Some(c) = self.files.get(&file) {
                out.extend(c.2.iter().filter(|e| e.at >= since).cloned());
            }
        }
        out
    }

    /// 从最早到今天，共 DAYS 天
    pub fn daily(&mut self, a: &Account, switches: &[Value]) -> Vec<u64> {
        let now = chrono::Local::now();
        let today = now.date_naive().and_hms_opt(0, 0, 0).and_then(|t| t.and_local_timezone(chrono::Local).earliest()).map(|d| d.timestamp_millis()).unwrap_or(0);
        let first = today - (DAYS as i64 - 1) * DAY_MS;
        let mut events = Vec::new();
        match a.group {
            Group::Codex => events = self.events(&codex::home_of(a).join("sessions"), first, codex_line),
            Group::Claude => {
                // 默认目录按切换记录归属到账号；账号自己的目录全算它的
                let mut marks: Vec<(i64, &str)> = switches.iter().filter_map(|s| Some((s["at"].as_i64()?, s["account"].as_str()?))).collect();
                marks.sort_by_key(|m| m.0);
                let shared = self.events(&store::home().join(".claude").join("projects"), first, claude_line);
                events.extend(shared.into_iter().filter(|e| marks.iter().rev().find(|m| m.0 <= e.at).is_some_and(|m| m.1 == a.id)));
                if let Some(dir) = usage::quota_dir(a) {
                    events.extend(self.events(&dir.join("projects"), first, claude_line));
                }
            }
        }
        let mut seen = HashSet::new();
        let mut days = vec![0u64; DAYS];
        for e in events {
            if !seen.insert(e.key) {
                continue;
            }
            let k = ((e.at - first) / DAY_MS).clamp(0, DAYS as i64 - 1) as usize;
            days[k] += e.total;
        }
        days
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_usage_numbers_are_taken_from_lines() {
        let e = claude_line(r#"{"type":"assistant","timestamp":"2030-01-01T00:00:00Z","uuid":"u","message":{"id":"m","content":"secret text","usage":{"input_tokens":3,"output_tokens":4,"cache_creation_input_tokens":5,"cache_read_input_tokens":999}}}"#).unwrap();
        assert_eq!(e.total, 12);
        assert_eq!(e.key, "m:");
        assert!(claude_line(r#"{"type":"user","message":{"content":"assistant usage"}}"#).is_none());
        let c = codex_line(r#"{"timestamp":"2030-01-01T00:00:00Z","payload":{"info":{"total_token_usage":{"total_tokens":77},"last_token_usage":{"input_tokens":10,"cached_input_tokens":4,"output_tokens":2}}}}"#).unwrap();
        assert_eq!(c.total, 8);
    }
}
