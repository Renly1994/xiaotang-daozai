//! 直接读 Marvis 的 data.db 聚合 token 用量，生成 marvis-usage.json。
//!
//! 旧版依赖外置 Python 脚本，换台电脑没有 pythonw 就没有额度环。这里用
//! 系统自带的 winsqlite3 只读打开 Marvis 数据库，把同样的口径在 Rust 里重算一遍：
//! 1. 只算办公室场景主对话（model_id = main-auto），后台模型不计；
//! 2. 统计办公室场景的全部对话（账号级累计），避免开新对话就归零；
//! 3. 缓存命中的部分不计，只算真实新增 (input - cached) + output + thinking。
//!
//! 全程只读，不写任何 Marvis 数据，输出固定为数据目录下的 marvis-usage.json。

use chrono::{Datelike, Duration as Span, Local, NaiveDate, TimeZone};
use serde_json::{json, Value as JsonValue};
use std::collections::BTreeMap;
use std::ffi::{c_char, c_int, c_void, CString};
use std::fs;
use std::path::{Path, PathBuf};

/// 只统计办公室场景（主对话）的用量，与办公室面板口径保持一致。
const OFFICE_MODEL: &str = "main-auto";

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_INTEGER: c_int = 1;
const SQLITE_NULL: c_int = 5;
const SQLITE_OPEN_READONLY: c_int = 0x0000_0001;
/// SQLITE_TRANSIENT：让 sqlite 自己复制绑定参数，调用返回后不依赖我们的内存。
const SQLITE_TRANSIENT: isize = -1;

#[link(name = "winsqlite3")]
extern "C" {
    fn sqlite3_open_v2(name: *const c_char, db: *mut *mut c_void, flags: c_int, vfs: *const c_char) -> c_int;
    fn sqlite3_close(db: *mut c_void) -> c_int;
    fn sqlite3_busy_timeout(db: *mut c_void, milliseconds: c_int) -> c_int;
    fn sqlite3_prepare_v2(db: *mut c_void, sql: *const c_char, len: c_int, statement: *mut *mut c_void, tail: *mut *const c_char) -> c_int;
    fn sqlite3_bind_text(statement: *mut c_void, index: c_int, text: *const c_char, len: c_int, destroy: isize) -> c_int;
    fn sqlite3_step(statement: *mut c_void) -> c_int;
    fn sqlite3_column_count(statement: *mut c_void) -> c_int;
    fn sqlite3_column_type(statement: *mut c_void, column: c_int) -> c_int;
    fn sqlite3_column_int64(statement: *mut c_void, column: c_int) -> i64;
    fn sqlite3_column_text(statement: *mut c_void, column: c_int) -> *const u8;
    fn sqlite3_column_bytes(statement: *mut c_void, column: c_int) -> c_int;
    fn sqlite3_finalize(statement: *mut c_void) -> c_int;
}

/// 一列取值，只区分整数 / 文本 / NULL，够用且不必引入额外依赖。
enum Cell {
    Int(i64),
    Text(String),
    Null,
}

impl Cell {
    fn int(&self) -> i64 {
        match self {
            Cell::Int(v) => *v,
            Cell::Text(s) => s.trim().parse().unwrap_or(0),
            Cell::Null => 0,
        }
    }

    fn text(&self) -> String {
        match self {
            Cell::Int(v) => v.to_string(),
            Cell::Text(s) => s.clone(),
            Cell::Null => String::new(),
        }
    }
}

/// 只读句柄，Drop 时关闭，避免异常分支漏关。
struct Db(*mut c_void);

impl Db {
    fn open(path: &Path) -> Option<Db> {
        let name = CString::new(path.to_string_lossy().as_bytes()).ok()?;
        let mut handle: *mut c_void = std::ptr::null_mut();
        let rc = unsafe { sqlite3_open_v2(name.as_ptr(), &mut handle, SQLITE_OPEN_READONLY, std::ptr::null()) };
        if rc != SQLITE_OK || handle.is_null() {
            if !handle.is_null() {
                unsafe { sqlite3_close(handle) };
            }
            return None;
        }
        // Marvis 可能正在写入，给一点等待窗口，读不到就跳过这一轮。
        unsafe { sqlite3_busy_timeout(handle, 3000) };
        Some(Db(handle))
    }

    fn rows(&self, sql: &str, args: &[&str]) -> Vec<Vec<Cell>> {
        let mut out: Vec<Vec<Cell>> = Vec::new();
        let Ok(statement_sql) = CString::new(sql) else { return out };
        let mut stmt: *mut c_void = std::ptr::null_mut();
        let rc = unsafe {
            sqlite3_prepare_v2(self.0, statement_sql.as_ptr(), -1, &mut stmt, std::ptr::null_mut())
        };
        if rc != SQLITE_OK || stmt.is_null() {
            return out;
        }
        for (index, arg) in args.iter().enumerate() {
            let Ok(text) = CString::new(*arg) else {
                unsafe { sqlite3_finalize(stmt) };
                return out;
            };
            unsafe {
                sqlite3_bind_text(
                    stmt,
                    (index + 1) as c_int,
                    text.as_ptr(),
                    text.as_bytes().len() as c_int,
                    SQLITE_TRANSIENT,
                )
            };
        }
        loop {
            if unsafe { sqlite3_step(stmt) } != SQLITE_ROW {
                break;
            }
            let columns = unsafe { sqlite3_column_count(stmt) };
            let mut row: Vec<Cell> = Vec::with_capacity(columns.max(0) as usize);
            for column in 0..columns {
                let kind = unsafe { sqlite3_column_type(stmt, column) };
                if kind == SQLITE_NULL {
                    row.push(Cell::Null);
                } else if kind == SQLITE_INTEGER {
                    row.push(Cell::Int(unsafe { sqlite3_column_int64(stmt, column) }));
                } else {
                    let pointer = unsafe { sqlite3_column_text(stmt, column) };
                    let length = unsafe { sqlite3_column_bytes(stmt, column) };
                    if pointer.is_null() || length <= 0 {
                        row.push(Cell::Null);
                    } else {
                        let bytes = unsafe { std::slice::from_raw_parts(pointer, length as usize) };
                        row.push(Cell::Text(String::from_utf8_lossy(bytes).into_owned()));
                    }
                }
            }
            out.push(row);
        }
        unsafe { sqlite3_finalize(stmt) };
        out
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { sqlite3_close(self.0) };
        }
    }
}

/// 所有 Marvis 账号的 data.db；没有 Marvis 或没登录过就是空列表。
fn databases() -> Vec<PathBuf> {
    let Some(appdata) = std::env::var_os("APPDATA") else { return Vec::new() };
    let root = PathBuf::from(appdata).join("Tencent").join("Marvis").join("User");
    let Ok(entries) = fs::read_dir(&root) else { return Vec::new() };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path().join("database").join("data.db"))
        .filter(|path| path.is_file())
        .collect();
    found.sort();
    found
}

fn parse_day(text: &str) -> Option<NaiveDate> {
    let head: String = text.chars().take(10).collect();
    NaiveDate::parse_from_str(&head, "%Y-%m-%d").ok()
}

/// 本地零点的时间戳，用作"何时重置"的提示。
fn reset_at(day: NaiveDate) -> i64 {
    let Some(midnight) = day.and_hms_opt(0, 0, 0) else { return 0 };
    Local
        .from_local_datetime(&midnight)
        .earliest()
        .map(|dt| dt.timestamp())
        .unwrap_or(0)
}

/// 最近一次有 token 记录的对话，作为办公室当前对话的兜底。
fn latest_conversation(dbs: &[PathBuf]) -> Option<String> {
    let mut best: Option<(String, String)> = None;
    for db in dbs {
        let Some(handle) = Db::open(db) else { continue };
        let rows = handle.rows(
            "select conversation_id, max(created_at) from llm_token_usage \
             where conversation_id is not null and conversation_id<>'' group by conversation_id",
            &[],
        );
        for row in rows {
            let conversation = row.first().map(Cell::text).unwrap_or_default();
            let last = row.get(1).map(Cell::text).unwrap_or_default();
            if conversation.is_empty() || last.is_empty() {
                continue;
            }
            if best.as_ref().map(|(stamp, _)| last > *stamp).unwrap_or(true) {
                best = Some((last, conversation));
            }
        }
    }
    best.map(|(_, conversation)| conversation)
}

/// 办公室当前主对话：今日 main-auto 真实新增最多的会话，避免零星小会话抢占导致数字归零。
fn pick_office_conversation(dbs: &[PathBuf], day: NaiveDate) -> String {
    let key = day.format("%Y-%m-%d").to_string();
    let mut best: Option<(i64, String)> = None;
    for db in dbs {
        let Some(handle) = Db::open(db) else { continue };
        let rows = handle.rows(
            "select conversation_id, \
             sum(max(input_tokens-cached_tokens,0)+output_tokens+thinking_tokens) \
             from llm_token_usage where lower(trim(model_id))=? and usage_date=? \
             and conversation_id is not null and conversation_id<>'' group by conversation_id",
            &[OFFICE_MODEL, key.as_str()],
        );
        for row in rows {
            let conversation = row.first().map(Cell::text).unwrap_or_default();
            let used = row.get(1).map(Cell::int).unwrap_or(0);
            if conversation.is_empty() || used <= 0 {
                continue;
            }
            if best.as_ref().map(|(top, _)| used > *top).unwrap_or(true) {
                best = Some((used, conversation));
            }
        }
    }
    match best {
        Some((_, conversation)) => conversation,
        None => latest_conversation(dbs).unwrap_or_default(),
    }
}

/// 单日一行的累加器：[真实新增, input, output, thinking, cached, 请求数]
type DayRow = [i64; 6];

fn collect() -> (BTreeMap<NaiveDate, DayRow>, BTreeMap<String, i64>, i64, String, i64) {
    let today = Local::now().date_naive();
    let key = today.format("%Y-%m-%d").to_string();
    let dbs = databases();
    let conversation = pick_office_conversation(&dbs, today);
    let mut per_day: BTreeMap<NaiveDate, DayRow> = BTreeMap::new();
    let mut models: BTreeMap<String, i64> = BTreeMap::new();
    let mut raw_today: i64 = 0;
    let mut all_today: i64 = 0;
    // 没有任何对话时用一个不可能存在的 id，等价于查不到行。
    let target = if conversation.is_empty() { "\u{0}".to_string() } else { conversation.clone() };

    for db in &dbs {
        let Some(handle) = Db::open(db) else { continue };
        for row in handle.rows("select model_id,sum(total_tokens) from llm_token_usage group by model_id", &[]) {
            let model = row.first().map(Cell::text).unwrap_or_default();
            let total = row.get(1).map(Cell::int).unwrap_or(0);
            if !model.is_empty() && total > 0 {
                *models.entry(model).or_insert(0) += total;
            }
        }
        for row in handle.rows(
            "select sum(total_tokens) from llm_token_usage where usage_date=?",
            &[key.as_str()],
        ) {
            raw_today += row.first().map(Cell::int).unwrap_or(0);
        }
        for row in handle.rows(
            "select sum(max(input_tokens-cached_tokens,0)+output_tokens+thinking_tokens) \
             from llm_token_usage where lower(trim(model_id))=? and usage_date=?",
            &[OFFICE_MODEL, key.as_str()],
        ) {
            all_today += row.first().map(Cell::int).unwrap_or(0);
        }
        // 只聚合办公室主对话，不混入其他窗口会话与子任务模型。
        for row in handle.rows(
            "select usage_date,sum(input_tokens),sum(output_tokens),sum(thinking_tokens),\
             sum(cached_tokens),count(*) from llm_token_usage \
             where lower(trim(model_id))=? and conversation_id=? group by usage_date",
            &[OFFICE_MODEL, target.as_str()],
        ) {
            let Some(day) = row.first().map(Cell::text).and_then(|text| parse_day(&text)) else {
                continue;
            };
            let input = row.get(1).map(Cell::int).unwrap_or(0);
            let output = row.get(2).map(Cell::int).unwrap_or(0);
            let thinking = row.get(3).map(Cell::int).unwrap_or(0);
            let cached = row.get(4).map(Cell::int).unwrap_or(0);
            let hits = row.get(5).map(Cell::int).unwrap_or(0);
            let used = (input - cached).max(0) + output + thinking;
            let slot = per_day.entry(day).or_insert([0; 6]);
            slot[0] += used;
            slot[1] += input;
            slot[2] += output;
            slot[3] += thinking;
            slot[4] += cached;
            slot[5] += hits;
        }
    }
    (per_day, models, raw_today, conversation, all_today)
}

fn shift_month(day: NaiveDate, back: u32) -> NaiveDate {
    let mut year = day.year();
    let mut month = day.month() as i64 - back as i64;
    while month <= 0 {
        month += 12;
        year -= 1;
    }
    NaiveDate::from_ymd_opt(year, month as u32, 1).unwrap_or(day)
}

fn build(per_day: &BTreeMap<NaiveDate, DayRow>, models: &BTreeMap<String, i64>, raw_today: i64, conversation: &str, all_today: i64) -> JsonValue {
    let today = Local::now().date_naive();
    let week_start = today - Span::days(today.weekday().num_days_from_monday() as i64);
    let month_start = today.with_day(1).unwrap_or(today);
    let prev_week_start = week_start - Span::days(7);
    let prev_month_start = shift_month(month_start, 1);
    let start30 = today - Span::days(29);

    let row = per_day.get(&today).copied().unwrap_or([0; 6]);
    let sum_between = |from: NaiveDate, to: NaiveDate| -> i64 {
        per_day
            .iter()
            .filter(|(day, _)| **day >= from && **day <= to)
            .map(|(_, values)| values[0])
            .sum()
    };
    let week = sum_between(week_start, today);
    let prev_week: i64 = per_day
        .iter()
        .filter(|(day, _)| **day >= prev_week_start && **day < week_start)
        .map(|(_, values)| values[0])
        .sum();
    let month = sum_between(month_start, today);
    let prev_month: i64 = per_day
        .iter()
        .filter(|(day, _)| **day >= prev_month_start && **day < month_start)
        .map(|(_, values)| values[0])
        .sum();
    let total: i64 = per_day.values().map(|values| values[0]).sum();
    let last30 = sum_between(start30, today);
    let active = per_day.keys().filter(|day| **day >= start30 && **day <= today).count().max(1) as f64;
    let avg30 = (last30 as f64 / active).round() as i64;

    let mut ranked: Vec<(&String, &i64)> = models.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let top: Vec<JsonValue> = ranked
        .into_iter()
        .take(5)
        .map(|(id, tokens)| json!({ "id": id, "tokens": tokens }))
        .collect();

    let tomorrow = today + Span::days(1);
    let next_month = shift_month(month_start, 0);
    let next_month = if month_start.month() == 12 {
        NaiveDate::from_ymd_opt(month_start.year() + 1, 1, 1).unwrap_or(next_month)
    } else {
        NaiveDate::from_ymd_opt(month_start.year(), month_start.month() + 1, 1).unwrap_or(next_month)
    };

    json!({
        "scope": format!("office:{}:current", OFFICE_MODEL),
        "conversation": conversation,
        "today": {
            "tokens": row[0],
            "all": all_today,
            "billing": "office-current",
            "raw_total": raw_today,
            "input": row[1],
            "output": row[2],
            "thinking": row[3],
            "cached": row[4],
            "requests": row[5],
        },
        "office": { "week": week, "month": month },
        "week": { "tokens": week, "prev": prev_week },
        "month": { "tokens": month, "prev": prev_month },
        "total_tokens": total,
        "avg30": avg30,
        "today_reset_at": reset_at(tomorrow),
        "month_reset_at": reset_at(next_month),
        "updated_at": Local::now().timestamp(),
        "models": top,
    })
}

/// 生成并原子替换快照；读不到任何记录时返回 false，让调用方决定是否兜底。
pub fn write_snapshot() -> bool {
    let (per_day, models, raw_today, conversation, all_today) = collect();
    if per_day.is_empty() && models.is_empty() {
        return false;
    }
    let payload = build(&per_day, &models, raw_today, &conversation, all_today);
    let directory = crate::model::config_dir();
    if fs::create_dir_all(&directory).is_err() {
        return false;
    }
    let out = directory.join(crate::quota::marvis::FILE);
    let temp = out.with_extension("json.tmp");
    let Ok(text) = serde_json::to_string(&payload) else { return false };
    if fs::write(&temp, text.as_bytes()).is_err() {
        let _ = fs::remove_file(&temp);
        return false;
    }
    if fs::rename(&temp, &out).is_err() {
        let _ = fs::remove_file(&temp);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用本机真实 Marvis 库跑一遍，和旧 Python 脚本产出的快照逐项对照口径。
    /// 没有 Marvis 数据或没有旧快照时直接跳过，避免在别人机器上误报。
    #[test]
    fn 聚合结果与旧脚本口径一致() {
        let Ok(appdata) = std::env::var("APPDATA") else { return };
        let reference = PathBuf::from(&appdata).join("Daozai").join(crate::quota::marvis::FILE);
        let Ok(raw) = fs::read(&reference) else { return };
        let Ok(old) = serde_json::from_slice::<JsonValue>(&raw) else { return };
        let (per_day, models, raw_today, conversation, all_today) = collect();
        assert!(!per_day.is_empty(), "读不到 Marvis 用量记录");
        let fresh = build(&per_day, &models, raw_today, &conversation, all_today);
        assert_eq!(fresh["conversation"], old["conversation"], "办公室主对话不一致");
        assert_eq!(fresh["today"]["tokens"], old["today"]["tokens"], "今日不一致");
        assert_eq!(fresh["today"]["all"], old["today"]["all"], "今日全账号不一致");
        assert_eq!(fresh["today"]["raw_total"], old["today"]["raw_total"], "今日原始总量不一致");
        assert_eq!(fresh["week"]["tokens"], old["week"]["tokens"], "本周不一致");
        assert_eq!(fresh["month"]["tokens"], old["month"]["tokens"], "本月不一致");
        assert_eq!(fresh["month"]["prev"], old["month"]["prev"], "上月不一致");
        assert_eq!(fresh["total_tokens"], old["total_tokens"], "累计不一致");
        assert_eq!(fresh["avg30"], old["avg30"], "30 日均值不一致");
        assert_eq!(fresh["models"], old["models"], "模型排行不一致");
    }
}