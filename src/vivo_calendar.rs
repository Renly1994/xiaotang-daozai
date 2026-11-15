//! vivo 办公套件日程：只读本机同步缓存。
//!
//! 办公套件自己负责把 vivo 账号云端的日程同步到本机
//! `%APPDATA%\pcsuite\database\CalendarSync.db`（Android CalendarProvider 风格）。
//! 这里只做只读查询并映射成岛仔的 `Meeting`：不联网、不读取账号凭据、不写入数据库。
//! 表结构属于未公开的私有实现，缺表/缺列时返回错误而不是崩溃，由调用方降级展示。

use std::{
    ffi::{c_char, c_int, c_void, CStr, CString},
    path::{Path, PathBuf},
    ptr,
};

use chrono::{Duration, Local, Utc};

use crate::{local_calendar::Entry, mail::Meeting};

/// `sqlite3_open_v2` 的 SQLITE_OPEN_READONLY。
const READONLY: c_int = 1;

type RowCallback =
    unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_char, *mut *mut c_char) -> c_int;

#[link(name = "winsqlite3")]
extern "C" {
    fn sqlite3_open_v2(
        name: *const c_char,
        db: *mut *mut c_void,
        flags: c_int,
        vfs: *const c_char,
    ) -> c_int;
    fn sqlite3_exec(
        db: *mut c_void,
        sql: *const c_char,
        callback: Option<RowCallback>,
        data: *mut c_void,
        error: *mut *mut c_char,
    ) -> c_int;
    fn sqlite3_close(db: *mut c_void) -> c_int;
}

/// 一次只读快照。
pub(crate) struct Snapshot {
    pub entries: Vec<Entry>,
    /// 办公套件自己的 `Config.lastSyncSuccessTime`（UTC 毫秒），仅用于设置页展示。
    pub last_sync_ms: i64,
}

/// 办公套件本地日历数据库的位置。
pub(crate) fn database_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        PathBuf::from(appdata)
            .join("pcsuite")
            .join("database")
            .join("CalendarSync.db"),
    )
}

pub(crate) fn load() -> Result<Snapshot, String> {
    let path = database_path().ok_or_else(|| "未找到系统应用数据目录".to_string())?;
    if !path.is_file() {
        return Err("未找到 vivo 办公套件日程数据：请先安装并登录办公套件，开启日程同步".into());
    }
    load_from(&path)
}

/// 收集 `sqlite3_exec` 的每一行；列按 SQL 顺序，值统一按文本解析。
unsafe extern "C" fn collect(
    data: *mut c_void,
    count: c_int,
    values: *mut *mut c_char,
    _: *mut *mut c_char,
) -> c_int {
    let rows = &mut *(data as *mut Vec<Vec<Option<String>>>);
    let mut row = Vec::with_capacity(count.max(0) as usize);
    for i in 0..count {
        let value = *values.offset(i as isize);
        row.push(if value.is_null() {
            None
        } else {
            Some(CStr::from_ptr(value).to_string_lossy().into_owned())
        });
    }
    rows.push(row);
    0
}

fn query(db: *mut c_void, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
    let statement = CString::new(sql).map_err(|_| "查询语句无效".to_string())?;
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let ok = unsafe {
        sqlite3_exec(
            db,
            statement.as_ptr(),
            Some(collect),
            &mut rows as *mut Vec<Vec<Option<String>>> as *mut c_void,
            ptr::null_mut(),
        ) == 0
    };
    if !ok {
        return Err("读取日程数据失败：办公套件的表结构可能已变化，或数据库正在更新".into());
    }
    Ok(rows)
}

fn load_from(path: &Path) -> Result<Snapshot, String> {
    let name = CString::new(path.to_string_lossy().as_bytes())
        .map_err(|_| "数据库路径无效".to_string())?;
    let mut db = ptr::null_mut();
    let opened = unsafe { sqlite3_open_v2(name.as_ptr(), &mut db, READONLY, ptr::null()) == 0 };
    if !opened {
        if !db.is_null() {
            unsafe { sqlite3_close(db); }
        }
        return Err("无法只读打开 vivo 办公套件日程数据库".into());
    }

    // 与 .ics 导入保持一致的可见范围：过去 30 天到未来一年。
    // Instances 已把重复日程展开成实例，不需要自己解析 RRULE。
    // lo/hi 是 i64 毫秒，直接内联进 SQL，不接受任何外部输入。
    let now = Utc::now();
    let lo = (now - Duration::days(30)).timestamp_millis();
    let hi = (now + Duration::days(366)).timestamp_millis();
    let sql = format!(
        "SELECT i.begin, i.end, e._id, e.title, e.allDay FROM Instances i JOIN Events e ON e._id = i.event_id WHERE (e.deleted IS NULL OR e.deleted = 0) AND i.begin < {hi} AND i.end > {lo} ORDER BY i.begin"
    );
    let events = query(db, &sql);
    // Config 是可选表，缺失不影响日程读取。
    let last_sync_ms = query(db, "SELECT lastSyncSuccessTime FROM Config LIMIT 1")
        .ok()
        .and_then(|rows| rows.into_iter().next())
        .and_then(|row| row.into_iter().next().flatten())
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    unsafe { sqlite3_close(db); }

    let entries = events?
        .iter()
        .filter_map(|row| to_entry(row))
        .collect::<Vec<_>>();
    Ok(Snapshot { entries, last_sync_ms })
}

/// 一行 `Instances JOIN Events` → 一条岛仔日程。
fn to_entry(row: &[Option<String>]) -> Option<Entry> {
    let cell = |i: usize| row.get(i).and_then(|value| value.as_deref());
    // 办公套件用 UTC 毫秒（Android CalendarProvider），岛仔用 UTC 秒。
    let begin = cell(0)?.parse::<i64>().ok()?;
    if begin <= 0 {
        return None;
    }
    let start = begin / 1000;
    let end_ms = cell(1).and_then(|value| value.parse::<i64>().ok()).unwrap_or(0);
    let all_day = cell(4).and_then(|value| value.parse::<i32>().ok()).unwrap_or(0) != 0;
    let fallback = if all_day { 86_400 } else { 3_600 };
    let mut end = if end_ms > begin { end_ms / 1000 } else { start + fallback };
    if end <= start {
        end = start + fallback;
    }
    let id = cell(2).unwrap_or("0");
    let title = cell(3).unwrap_or("").trim();
    let at = chrono::DateTime::from_timestamp(start, 0)?
        .with_timezone(&Local)
        .format("%m-%d %H:%M")
        .to_string();
    Some(Entry {
        event: Meeting {
            uid: format!("vivo-{id}-{begin}"),
            title: if title.is_empty() { "日程".into() } else { title.into() },
            start,
            end: Some(end),
            all_day,
            at,
        },
        // 办公套件本地提醒表通常为空；给定时日程一个默认提前量，全天不提醒。
        reminder: (!all_day).then_some(15),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用系统 SQLite 造一个最小样本库，只覆盖本模块实际读到的列。
    fn sample(path: &Path, rows: &str) {
        let name = CString::new(path.to_string_lossy().as_bytes()).unwrap();
        let mut db = ptr::null_mut();
        unsafe {
            assert_eq!(sqlite3_open_v2(name.as_ptr(), &mut db, 6, ptr::null()), 0);
            let ddl = CString::new(format!(
                "CREATE TABLE Events (_id INTEGER, title TEXT, allDay INTEGER, deleted INTEGER); CREATE TABLE Instances (_id INTEGER, event_id INTEGER, begin INTEGER, end INTEGER); CREATE TABLE Config (lastSyncSuccessTime INTEGER); INSERT INTO Config VALUES (1794736732926); {rows}"
            )).unwrap();
            assert_eq!(sqlite3_exec(db, ddl.as_ptr(), None, ptr::null_mut(), ptr::null_mut()), 0);
            sqlite3_close(db);
        }
    }

    #[test]
    fn maps_utc_milliseconds_and_skips_deleted_events() {
        let path = std::env::temp_dir().join(format!("daozai-vivo-{}.db", std::process::id()));
        let now_ms = Utc::now().timestamp_millis() + 3_600_000;
        let end_ms = now_ms + 3_600_000;
        let later_ms = now_ms + 2_000;
        sample(&path, &format!(
            "INSERT INTO Events VALUES (1,'周会',0,0),(2,'已删除',0,1),(3,'休假',1,0); INSERT INTO Instances VALUES (1,1,{now_ms},{end_ms}),(2,2,{now_ms},{end_ms}),(3,3,{later_ms},{later_ms});"
        ));
        let snapshot = load_from(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(snapshot.entries.len(), 2, "已删除的日程必须过滤");
        assert_eq!(snapshot.last_sync_ms, 1794736732926);
        let meeting = &snapshot.entries[0].event;
        assert_eq!(meeting.start, now_ms / 1000);
        assert_eq!(meeting.end, Some(end_ms / 1000));
        assert_eq!(meeting.title, "周会");
        assert!(meeting.uid.starts_with("vivo-1-"));
        assert_eq!(snapshot.entries[0].reminder, Some(15));
        assert!(snapshot.entries[1].event.all_day);
        assert_eq!(snapshot.entries[1].reminder, None, "全天日程不弹提醒");
    }

    #[test]
    fn missing_tables_return_an_error_instead_of_panicking() {
        let path = std::env::temp_dir().join(format!("daozai-vivo-empty-{}.db", std::process::id()));
        let name = CString::new(path.to_string_lossy().as_bytes()).unwrap();
        let mut db = ptr::null_mut();
        unsafe {
            assert_eq!(sqlite3_open_v2(name.as_ptr(), &mut db, 6, ptr::null()), 0);
            sqlite3_close(db);
        }
        assert!(load_from(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
