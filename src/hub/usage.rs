//! 官方额度：在每个账号自己的命令行配置目录里运行 `claude -p /usage`。
//! 这是本地命令，不调用模型、不耗额度；岛不读令牌，只是让 claude 用它自己的登录去问。
//! 结果写到 %USERPROFILE%\claude-hub\limits\usage-*.json，格式与 claude-hub 相同。
use super::{proc, store, store::Account};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Win {
    /// 已用百分比
    pub used: f32,
    pub resets_at: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Limits {
    pub at: i64,
    pub w5: Option<Win>,
    pub week: Option<Win>,
    pub plan: String,
}

fn limits_dir() -> PathBuf {
    store::hub_dir().join("limits")
}

/// 额度从哪个命令行登录取：终端账号就是它自己的目录；桌面端账号是另外绑定的目录
pub fn quota_dir(a: &Account) -> Option<PathBuf> {
    if a.group != store::Group::Claude {
        return None;
    }
    if a.kind == "cli" {
        return Some(a.config_dir.clone().unwrap_or_else(|| store::home().join(".claude-accounts").join(&a.id)));
    }
    a.quota_dir.clone()
}

/// 新账号默认的取额度目录
pub fn default_quota_dir(id: &str) -> PathBuf {
    store::home().join(".claude-accounts").join(format!("{id}-usage"))
}

pub fn norm_dir(dir: &Path) -> String {
    dir.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

pub fn to_ms(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_f64() {
        return Some(if n > 1e12 { n as i64 } else { (n * 1000.0) as i64 });
    }
    let s = v.as_str()?.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<f64>() {
        return to_ms(&json!(n));
    }
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis())
}

fn win(v: &Value) -> Option<Win> {
    Some(Win { used: v["used_percentage"].as_f64()? as f32, resets_at: to_ms(&v["resets_at"]) })
}

/// 官方 /usage 的结构化结果：usage_report.rate_limits.limits = [{ kind: session|weekly_all|weekly_scoped, percent, resets_at }]
pub fn parse_usage_report(stdout: &str) -> Option<Value> {
    for line in stdout.lines() {
        let Ok(m) = serde_json::from_str::<Value>(line.trim()) else { continue };
        let rep = if m["usage_report"].is_object() { &m["usage_report"] } else { &m["message"]["usage_report"] };
        if !rep.is_object() {
            continue;
        }
        let rows: Vec<&Value> = rep["rate_limits"]["limits"].as_array().into_iter().flatten().filter(|r| r["percent"].is_number()).collect();
        if rows.is_empty() {
            return None;
        }
        let pick = |kind: &str| rows.iter().find(|r| r["kind"].as_str() == Some(kind)).map(|r| json!({"used_percentage": r["percent"], "resets_at": r["resets_at"]}));
        let mut out = json!({});
        if let Some(w) = pick("session") {
            out["five_hour"] = w;
        }
        if let Some(w) = pick("weekly_all") {
            out["seven_day"] = w;
        }
        return Some(out);
    }
    None
}

fn sha1_12(text: &str) -> String {
    let d = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, text.as_bytes());
    d.as_ref().iter().take(6).map(|b| format!("{b:02x}")).collect()
}

pub fn parse_record(v: &Value) -> Option<(String, Limits)> {
    let dir = v["configDir"].as_str()?;
    let rl = &v["rate_limits"];
    Some((norm_dir(Path::new(dir)), Limits { at: v["at"].as_i64().unwrap_or(0), w5: win(&rl["five_hour"]), week: win(&rl["seven_day"]), plan: String::new() }))
}

/// 该目录最近一次记下的额度（官方 /usage 或状态栏脚本写的都算）
pub fn read_for(dir: &Path) -> Option<Limits> {
    let want = norm_dir(dir);
    let mut best: Option<Limits> = None;
    for e in fs::read_dir(limits_dir()).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".json") || name.starts_with('_') {
            continue;
        }
        let Some((d, lim)) = store::read_json(&e.path()).as_ref().and_then(parse_record) else { continue };
        if d == want && best.as_ref().map_or(true, |b| lim.at > b.at) {
            best = Some(lim);
        }
    }
    best
}

fn claude_line(args: &str) -> String {
    format!("claude {args}")
}

#[derive(Clone, Debug, Default)]
pub struct AuthStatus {
    pub ok: bool,
    pub logged_in: bool,
    pub email: Option<String>,
}

/// `claude auth status --json`：未登录时退出码为 1，但仍输出 JSON
pub fn auth_status(dir: &Path) -> AuthStatus {
    let mut c = proc::shell(&claude_line("auth status --json"));
    c.env("CLAUDE_CONFIG_DIR", dir);
    let out = proc::run(c, Duration::from_secs(25));
    let Some(start) = out.stdout.find('{') else { return AuthStatus::default() };
    let Ok(j) = serde_json::from_str::<Value>(&out.stdout[start..]) else { return AuthStatus::default() };
    let email = j["email"].as_str().or_else(|| j["emailAddress"].as_str()).map(str::to_owned);
    AuthStatus { ok: true, logged_in: j["loggedIn"].as_bool().unwrap_or(false), email }
}

pub fn logout(dir: &Path) {
    let mut c = proc::shell(&claude_line("auth logout"));
    c.env("CLAUDE_CONFIG_DIR", dir);
    proc::run(c, Duration::from_secs(25));
}

/// 运行官方 /usage 并记下结果
pub fn refresh(dir: &Path) -> Option<Limits> {
    let mut c = proc::shell(&claude_line("-p /usage --output-format stream-json --verbose --no-session-persistence"));
    c.env("CLAUDE_CONFIG_DIR", dir);
    let out = proc::run(c, Duration::from_secs(90));
    let rl = parse_usage_report(&out.stdout)?;
    let path_text = dir.to_string_lossy().into_owned();
    let rec = json!({"at": store::now_ms(), "configDir": path_text, "src": "usage", "rate_limits": rl});
    let name = format!("usage-{}.json", sha1_12(&norm_dir(dir)));
    store::write_atomic(&limits_dir().join(name), rec.to_string().as_bytes()).ok()?;
    parse_record(&rec).map(|r| r.1)
}

/// 这个账号的 5 小时或本周额度是否已用到 100%（还没到重置时间）
pub fn exhausted_window(lim: &Limits, now: i64) -> Option<(&'static str, Option<i64>)> {
    for (name, w) in [("5 小时", lim.w5), ("本周", lim.week)] {
        if let Some(w) = w {
            if w.used >= 99.5 && w.resets_at.map_or(true, |t| t > now) {
                return Some((name, w.resets_at));
            }
        }
    }
    None
}

/// 记一笔“额度用完时切走”，只留最近 8 天。只是记录，不会因此自动换号。
pub fn record_exhausted(d: &mut Value, from: &str, to: &str, lim: Option<&Limits>, now: i64) {
    let Some(lim) = lim else { return };
    let Some((window, resets_at)) = exhausted_window(lim, now) else { return };
    if from.is_empty() || from == to {
        return;
    }
    let mut list: Vec<Value> = d["exhausted"].as_array().cloned().unwrap_or_default();
    list.retain(|x| now - x["at"].as_i64().unwrap_or(0) < 8 * 86_400_000);
    list.push(json!({"account": from, "to": to, "window": window, "resetsAt": resets_at, "at": now}));
    d["exhausted"] = Value::Array(list);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn usage_report_maps_session_and_weekly() {
        let out = "{\"type\":\"system\"}\n{\"type\":\"result\",\"usage_report\":{\"rate_limits\":{\"limits\":[{\"kind\":\"session\",\"percent\":33,\"resets_at\":\"2030-01-01T00:00:00Z\"},{\"kind\":\"weekly_all\",\"percent\":84},{\"kind\":\"weekly_scoped\",\"percent\":5}]}}}\n";
        let rl = parse_usage_report(out).unwrap();
        assert_eq!(rl["five_hour"]["used_percentage"], 33);
        assert_eq!(rl["seven_day"]["used_percentage"], 84);
        let (_, lim) = parse_record(&json!({"at":5,"configDir":"C:\\A\\","rate_limits":rl})).unwrap();
        assert_eq!(lim.w5.unwrap().resets_at, Some(1_893_456_000_000));
        assert!(parse_usage_report("not json\n{\"usage_report\":{\"rate_limits\":{\"limits\":[]}}}").is_none());
    }
    #[test]
    fn record_names_match_desktop_hub() {
        // 与 claude-hub 的 sha1(normDir).slice(0,12) 一致
        assert_eq!(norm_dir(Path::new("C:\\Users\\X\\.claude-accounts\\a-usage\\")), "c:\\users\\x\\.claude-accounts\\a-usage");
        assert_eq!(sha1_12("abc"), "a9993e364706");
    }
    #[test]
    fn exhausted_only_before_reset() {
        let lim = Limits { at: 0, w5: Some(Win { used: 100.0, resets_at: Some(2000) }), week: None, plan: String::new() };
        assert!(exhausted_window(&lim, 1000).is_some());
        assert!(exhausted_window(&lim, 3000).is_none());
        let mut d = json!({});
        record_exhausted(&mut d, "a", "b", Some(&lim), 1000);
        assert_eq!(d["exhausted"][0]["window"], "5 小时");
    }
}
