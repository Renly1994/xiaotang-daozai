//! Marvis 用量。只读取 Marvis 写入的本地 JSON 快照，不访问数据库、不联网、不产生常驻线程。
//! 快照路径固定为数据目录下的 marvis-usage.json，由外部低频任务生成。
use super::{Account, Snapshot, Window};
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub const FILE: &str = "marvis-usage.json";

#[derive(Clone, Debug, Default)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Model {
    pub id: String,
    pub tokens: u64,
}

/// 快照明细。字段缺失一律按 0 处理，显示层再决定是否隐藏。
#[derive(Clone, Debug, Default)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Details {
    pub today: u64,
    pub week: u64,
    pub month: u64,
    pub total: u64,
    pub avg30: u64,
    pub prev_week: u64,
    pub prev_month: u64,
    pub input: u64,
    pub output: u64,
    pub thinking: u64,
    pub cached: u64,
    pub requests: u64,
    pub updated: i64,
    /// 当前对话今日用量，仅作对照展示，不参与额度计算。
    pub today_current: u64,
    /// 今日窗口的重置时刻（次日 00:00），由快照给出，缺失为 0。
    pub today_reset: i64,
    /// 本月窗口的重置时刻（次月 1 日 00:00），由快照给出，缺失为 0。
    pub month_reset: i64,
    pub models: Vec<Model>,
}

pub fn path() -> PathBuf {
    crate::model::config_dir().join(FILE)
}

/// 快照超过这个时长就顺手触发一次后台刷新，避免读到的永远是启动那天的数据。
const STALE: Duration = Duration::from_secs(60);
/// 同一时刻只允许一个刷新任务在跑，防止反复重试时堆进程。
static REFRESHING: AtomicBool = AtomicBool::new(false);

fn age() -> Option<Duration> {
    fs::metadata(path()).ok()?.modified().ok()?.elapsed().ok()
}

/// 找 pythonw.exe：优先 PATH，其次常见安装位置与 Marvis 自带运行时。
/// 只做有限层目录探查，不做全盘遍历，避免拖慢调用方。
fn pythonw() -> Option<PathBuf> {
    if let Ok(list) = std::env::var("PATH") {
        for dir in std::env::split_paths(&list) {
            let candidate = dir.join("pythonw.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let mut found: Vec<PathBuf> = Vec::new();
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let base = PathBuf::from(local).join("Programs").join("Python");
        if let Ok(entries) = fs::read_dir(&base) {
            for entry in entries.flatten() {
                let candidate = entry.path().join("pythonw.exe");
                if candidate.is_file() {
                    found.push(candidate);
                }
            }
        }
    }
    for root in [
        r"C:\Program Files\Tencent\Marvis\MarvisAgent",
        r"D:\Program Files\Tencent\Marvis\MarvisAgent",
        r"E:\Program Files\Tencent\Marvis\MarvisAgent",
    ] {
        let Ok(entries) = fs::read_dir(root) else { continue };
        for entry in entries.flatten() {
            let Ok(runtimes) = fs::read_dir(entry.path().join("runtime")) else { continue };
            for runtime in runtimes.flatten() {
                let candidate = runtime.path().join("pythonw.exe");
                if candidate.is_file() {
                    found.push(candidate);
                }
            }
        }
    }
    found.sort_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok());
    found.pop()
}

/// 后台刷新一次快照：默认由程序自己读库生成，无需任何外部运行时。
/// 只有在自带聚合失败、且本机恰好存在旧版 Python 脚本时才退回脚本兜底。
pub fn refresh_async() {
    if REFRESHING.swap(true, Ordering::SeqCst) {
        return;
    }
    let script = crate::model::config_dir().join("marvis_usage_snapshot.py");
    let python = pythonw().filter(|_| script.is_file());
    std::thread::spawn(move || {
        if crate::marvis_snapshot::write_snapshot() {
            REFRESHING.store(false, Ordering::SeqCst);
            return;
        }
        let Some(python) = python else {
            REFRESHING.store(false, Ordering::SeqCst);
            return;
        };
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        let _ = Command::new(python)
            .arg(&script)
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW：不闪黑框
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        REFRESHING.store(false, Ordering::SeqCst);
    });
}

fn count(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// 余量百分比：环只画剩下的部分，用光即为 0，未用为 100。
fn remain(used: u64, budget: u64) -> f32 {
    if budget == 0 {
        return 0.0;
    }
    let left = budget.saturating_sub(used) as f64;
    ((left / budget as f64) * 100.0).max(0.0).min(100.0) as f32
}

/// 今日还能用多少 token：日总量减已用，不足按 0 计。
pub fn daily_left(d: &Details) -> u64 {
    daily_budget(d).saturating_sub(d.today)
}

/// 本月还能用多少 token：当月总量减本月已用，不足按 0 计。
pub fn month_left(d: &Details) -> u64 {
    month_budget(d).saturating_sub(d.month)
}

/// 人类可读的 token 量，供明细文本使用。
pub fn text(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.0}K", tokens as f64 / 1_000.0)
    } else {
        format!("{tokens}")
    }
}

/// 中文口径的 token 量：面板沿用办公室习惯的「万 / 亿」，整数不带多余小数。
pub fn text_cn(tokens: u64) -> String {
    fn trim(value: f64) -> String {
        let raw = format!("{value:.1}");
        raw.strip_suffix(".0").map(str::to_string).unwrap_or(raw)
    }
    if tokens >= 100_000_000 {
        format!("{}亿", trim(tokens as f64 / 100_000_000.0))
    } else if tokens >= 10_000 {
        format!("{}万", trim(tokens as f64 / 10_000.0))
    } else {
        format!("{tokens}")
    }
}

fn delta(used: u64, previous: u64) -> String {
    if previous == 0 || used == 0 {
        return "—".into();
    }
    format!("{:+.0}%", ((used as f64 - previous as f64) / previous as f64) * 100.0)
}

/// 日额度总量：默认 1000 万 token，可用 settings.txt 的 `marvis_daily_budget=` 覆盖。
pub fn daily_budget(_d: &Details) -> u64 {
    configured_daily_budget()
}

/// 读取用户配置的日额度，5 秒缓存，改 settings.txt 后无需重启太久即可生效。
fn configured_daily_budget() -> u64 {
    static CACHE: std::sync::Mutex<Option<(i64, u64)>> = std::sync::Mutex::new(None);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if let Ok(guard) = CACHE.lock() {
        if let Some((at, value)) = *guard {
            if now.saturating_sub(at) < 5 {
                return value;
            }
        }
    }
    let value = read_daily_budget().unwrap_or(DAILY_BUDGET);
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some((now, value));
    }
    value
}

fn read_daily_budget() -> Option<u64> {
    let text = std::fs::read_to_string(crate::model::config_dir().join("settings.txt")).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "marvis_daily_budget" {
                return value.trim().parse::<u64>().ok().filter(|v| *v > 0);
            }
        }
    }
    None
}

/// 每日额度上限：办公室口径每日 1000 万 token。
pub const DAILY_BUDGET: u64 = 10_000_000;

/// 当月自然日天数：按月历实际天数折算，不按固定 30 天假设。
pub fn days_in_month() -> u64 {
    use chrono::Datelike;
    let now = chrono::Local::now();
    let (year, month) = (now.year(), now.month());
    let first = chrono::NaiveDate::from_ymd_opt(year, month, 1);
    let next = if month == 12 {
        chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        chrono::NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    match (first, next) {
        (Some(a), Some(b)) => (b - a).num_days().max(1) as u64,
        _ => 30,
    }
}

/// 月额度总量：日额度 × 当月日历天数。
pub fn month_budget(_d: &Details) -> u64 {
    DAILY_BUDGET.saturating_mul(days_in_month())
}

/// 明细弹窗文本。
pub fn details(d: &Details) -> String {
    let mut lines = vec![
        format!("今日 {} token · {} 次请求", text(d.today), d.requests),
        "口径：只算办公室当前主对话真实新增 token".to_string(),
        format!("全账号今日 {}（仅对照，不入额度）", text(d.today_current)),
        format!("输入 {} · 输出 {}", text(d.input), text(d.output)),
    ];
    if d.thinking > 0 || d.cached > 0 {
        lines.push(format!("思考 {} · 缓存 {}", text(d.thinking), text(d.cached)));
    }
    lines.push(format!("本周 {}（较上周 {}）", text(d.week), delta(d.week, d.prev_week)));
    lines.push(format!("本月 {}（较上月 {}）", text(d.month), delta(d.month, d.prev_month)));
    lines.push(format!("近 30 日日均 {}", text(d.avg30)));
    lines.push(format!("日总量 {} · 月总量 {}({} 天)", text(daily_budget(d)), text(month_budget(d)), days_in_month()));
    lines.push(format!("历史累计 {}", text(d.total)));
    if !d.models.is_empty() {
        let top = d.models.iter().take(3).map(|m| format!("{} {}", m.id, text(m.tokens))).collect::<Vec<_>>().join("、");
        lines.push(format!("各模型累计 {top}"));
    }
    if let Some(at) = chrono::DateTime::from_timestamp(d.updated, 0) {
        lines.push(format!("快照更新 {}", at.with_timezone(&chrono::Local).format("%m-%d %H:%M")));
    }
    lines.join("\n")
}

pub fn parse(v: &Value) -> Result<Snapshot, String> {
    let today_row = v.get("today").cloned().unwrap_or(Value::Null);
    let week_row = v.get("week").cloned().unwrap_or(Value::Null);
    let month_row = v.get("month").cloned().unwrap_or(Value::Null);
    let models = v["models"].as_array().into_iter().flatten().filter_map(|row| {
        let id = row["id"].as_str()?.to_string();
        let tokens = row["tokens"].as_u64().unwrap_or(0);
        (tokens > 0).then_some(Model { id, tokens })
    }).collect::<Vec<_>>();
    let d = Details {
        today: count(&today_row, "tokens"),
        week: count(&week_row, "tokens"),
        month: count(&month_row, "tokens"),
        total: v["total_tokens"].as_u64().unwrap_or(0),
        avg30: v["avg30"].as_u64().unwrap_or(0),
        prev_week: count(&week_row, "prev"),
        prev_month: count(&month_row, "prev"),
        input: count(&today_row, "input"),
        output: count(&today_row, "output"),
        thinking: count(&today_row, "thinking"),
        cached: count(&today_row, "cached"),
        requests: count(&today_row, "requests"),
        today_current: count(&today_row, "current"),
        updated: v["updated_at"].as_i64().unwrap_or(0),
        today_reset: v["today_reset_at"].as_i64().unwrap_or(0),
        month_reset: v["month_reset_at"].as_i64().unwrap_or(0),
        models,
    };
    if d.updated <= 0 {
        return Err("Marvis 用量快照缺少更新时间，请重新生成".into());
    }
    if d.today == 0 && d.week == 0 && d.total == 0 {
        return Err("Marvis 用量快照暂无数据".into());
    }
    // 今日对照日总量 1000 万，本月对照当月日历天数 × 1000 万。
    let daily = daily_budget(&d);
    let monthly = month_budget(&d);
    // 环一律表达余量：画出去的部分就是还能用的比例，用光为 0、未用为满。
    // 不提供 Amount：已用量可能高于额度总量，Amount 会把剩余钳到上限而显示成错误数字，
    // 剩余 token 原值由副行直接给出。
    let windows = vec![
        Window {
            label: "今日".into(),
            seconds: Some(86_400),
            remaining: remain(d.today, daily),
            reset: (d.today_reset > 0).then_some(d.today_reset),
            amount: None,
        },
        Window {
            label: "本月".into(),
            seconds: Some(2_592_000),
            remaining: remain(d.month, monthly),
            reset: (d.month_reset > 0).then_some(d.month_reset),
            amount: None,
        },
    ];
    let at = d.updated;
    Ok(Snapshot { marvis: Some(d), plan: "用量".into(), windows, at, ..Default::default() })
}

pub fn fetch(_account: &Account) -> Result<Snapshot, (String, u64)> {
    let stale = age().map(|elapsed| elapsed > STALE).unwrap_or(true);
    if stale {
        refresh_async();
    }
    let file = path();
    let raw = fs::read(&file).map_err(|_| ("正在生成 Marvis 用量快照，稍后自动刷新".to_string(), 30))?;
    let value = serde_json::from_slice(&raw).map_err(|_| ("Marvis 用量快照格式无效，请重新生成".to_string(), 600))?;
    parse(&value).map_err(|e| (e, 600))
}

/// Marvis 来源没有落盘凭据，移除账号时无需清理。
pub fn forget(_account: &Account) {}