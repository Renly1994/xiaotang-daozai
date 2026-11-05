//! Claude 桌面端自己的数据：每个账号各有一份 Code 会话列表，以及写着登录账号的日志。
//! 会话列表只读写 local_*.json 列表条目（对话正文在 ~/.claude/projects，不碰）；
//! 写之前整份备份，桌面端开着时不写（它会用内存里的列表覆盖回去）。
use super::store;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MANIFEST: &str = "claude-hub-manifest.json";
const APP_ID: &str = "Claude_pzs8sxrjxfjjc!Claude";

fn env_dir(key: &str, fallback: &[&str]) -> PathBuf {
    std::env::var_os(key).map(PathBuf::from).unwrap_or_else(|| fallback.iter().fold(store::home(), |p, s| p.join(s)))
}
fn local() -> PathBuf {
    env_dir("LOCALAPPDATA", &["AppData", "Local"])
}
fn roaming() -> PathBuf {
    env_dir("APPDATA", &["AppData", "Roaming"])
}
pub fn backups_dir() -> PathBuf {
    std::env::var_os("CLAUDE_HUB_DESKTOP_BACKUPS").map(PathBuf::from).unwrap_or_else(|| store::hub_dir().join("desktop-backups"))
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect();
    out.sort();
    out
}

fn store_package(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("claude") || n.starts_with("anthropic")
}

/// 会话列表可能在三处：商店版新位置 Claude-Data、%APPDATA%\Claude、商店版的重定向副本
fn candidates() -> Vec<PathBuf> {
    if let Some(root) = std::env::var_os("CLAUDE_HUB_SESSIONS_ROOT") {
        return vec![PathBuf::from(root)];
    }
    let mut out = vec![roaming().join("Claude").join("claude-code-sessions")];
    for d in subdirs(&local().join("Claude-Data")) {
        out.push(d.join("claude-code-sessions"));
    }
    for d in subdirs(&local().join("Packages")) {
        if d.file_name().is_some_and(|n| store_package(&n.to_string_lossy())) {
            out.push(d.join("LocalCache").join("Roaming").join("Claude").join("claude-code-sessions"));
        }
    }
    out
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub file: PathBuf,
    pub last: i64,
    pub archived: bool,
}

#[derive(Clone, Debug)]
pub struct List {
    pub acct: String,
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    pub last: i64,
}

fn read_lists(root: &Path) -> Vec<List> {
    let mut out = Vec::new();
    for acct_dir in subdirs(root) {
        for org_dir in subdirs(&acct_dir) {
            let mut entries = Vec::new();
            for e in fs::read_dir(&org_dir).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if !(name.starts_with("local_") && name.ends_with(".json")) {
                    continue;
                }
                // 条目只取会话 ID、最后活动时间和归档标记
                let Some(d) = store::read_json(&e.path()) else { continue };
                if d["sessionId"].as_str().is_none() {
                    continue;
                }
                entries.push(Entry { name, file: e.path(), last: d["lastActivityAt"].as_i64().unwrap_or(0), archived: d["isArchived"].as_bool().unwrap_or(false) });
            }
            let last = entries.iter().map(|e| e.last).max().unwrap_or(0);
            out.push(List { acct: acct_dir.file_name().unwrap_or_default().to_string_lossy().into_owned(), dir: org_dir, entries, last });
        }
    }
    out
}

/// 这个账号的会话列表目录
pub fn list_dirs(acct: &str) -> Vec<PathBuf> {
    session_lists().into_iter().filter(|l| l.acct == acct).map(|l| l.dir).collect()
}

/// 有多处时用条目最新的那一处
pub fn session_lists() -> Vec<List> {
    let mut best: Option<(i64, Vec<List>)> = None;
    for root in candidates() {
        if !root.is_dir() {
            continue;
        }
        let lists = read_lists(&root);
        let last = lists.iter().map(|l| l.last).max().unwrap_or(-1);
        if best.as_ref().map_or(true, |(b, _)| last > *b) {
            best = Some((last, lists));
        }
    }
    best.map(|b| b.1).unwrap_or_default()
}

/// 一个账号可能有多个组织目录：用条目最多的那个
pub fn list_of<'a>(lists: &'a [List], acct: &str) -> Option<&'a List> {
    lists.iter().filter(|l| l.acct == acct).max_by_key(|l| l.entries.len())
}

pub struct Item {
    pub name: String,
    pub file: PathBuf,
    pub replace: bool,
}

/// 要复制到 acct 的条目：其他账号里没归档的，且比目标里同名条目新
pub fn plan_sync<'a>(lists: &'a [List], acct: &str) -> Option<(&'a List, Vec<Item>)> {
    let dst = list_of(lists, acct)?;
    let have: HashMap<&str, i64> = dst.entries.iter().map(|e| (e.name.as_str(), e.last)).collect();
    let mut best: HashMap<&str, &Entry> = HashMap::new();
    for l in lists.iter().filter(|l| l.acct != acct) {
        for e in l.entries.iter().filter(|e| !e.archived) {
            if best.get(e.name.as_str()).map_or(true, |cur| e.last > cur.last) {
                best.insert(&e.name, e);
            }
        }
    }
    let mut items: Vec<Item> = best
        .into_values()
        .filter(|e| !have.get(e.name.as_str()).is_some_and(|t| *t >= e.last))
        .map(|e| Item { name: e.name.clone(), file: e.file.clone(), replace: have.contains_key(e.name.as_str()) })
        .collect();
    items.sort_by(|a, b| a.name.cmp(&b.name));
    Some((dst, items))
}

/// 还差几条（没有列表时为 None）
pub fn missing(lists: &[List], acct: &str) -> Option<usize> {
    plan_sync(lists, acct).map(|(_, items)| items.len())
}

pub fn visible_count(lists: &[List], acct: &str) -> Option<usize> {
    list_of(lists, acct).map(|l| l.entries.iter().filter(|e| !e.archived).count())
}

// ------------------------------------------------------------------
// 登录时间线：桌面端日志里的 cacheKey=acct:<ID>，只取 ID 和时间
// ------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signin {
    pub at: i64,
    pub acct: String,
}

#[derive(Default)]
struct LogFile {
    len: u64,
    points: Vec<Signin>,
}

/// 增量读日志：文件只追加时只读新增部分
#[derive(Default)]
pub struct Timeline {
    files: HashMap<PathBuf, LogFile>,
}

fn log_dirs() -> Vec<PathBuf> {
    if let Some(d) = std::env::var_os("CLAUDE_HUB_LOG_DIR") {
        return vec![PathBuf::from(d)];
    }
    let mut out = vec![local().join("Claude").join("logs"), roaming().join("Claude").join("logs")];
    for d in subdirs(&local().join("Packages")) {
        if d.file_name().is_some_and(|n| store_package(&n.to_string_lossy())) {
            out.push(d.join("LocalCache").join("Roaming").join("Claude").join("logs"));
        }
    }
    out
}

pub fn parse_signins(text: &str) -> Vec<Signin> {
    use chrono::TimeZone;
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(pos) = line.find("cacheKey=acct:") else { continue };
        let acct: String = line[pos + 14..].chars().take(36).collect();
        if acct.len() != 36 || !acct.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            continue;
        }
        let Some(stamp) = line.get(..19) else { continue };
        let Ok(t) = chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M:%S") else { continue };
        let Some(at) = chrono::Local.from_local_datetime(&t).earliest() else { continue };
        out.push(Signin { at: at.timestamp_millis(), acct });
    }
    out
}

impl Timeline {
    /// 按时间排好的全部登录记录（含重复，切换时要看“之后有没有出现过”）
    pub fn read(&mut self) -> Vec<Signin> {
        use std::io::{Read, Seek, SeekFrom};
        let mut seen = Vec::new();
        for dir in log_dirs() {
            for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if !(name.starts_with("main") && name.ends_with(".log")) {
                    continue;
                }
                let path = e.path();
                let len = e.metadata().map(|m| m.len()).unwrap_or(0);
                seen.push(path.clone());
                let cached = self.files.entry(path.clone()).or_default();
                if len == cached.len {
                    continue;
                }
                let start = if len > cached.len { cached.len } else { 0 };
                if start == 0 {
                    cached.points.clear();
                }
                let Ok(mut f) = fs::File::open(&path) else { continue };
                if f.seek(SeekFrom::Start(start)).is_err() {
                    continue;
                }
                let mut buf = Vec::new();
                if f.take(len - start).read_to_end(&mut buf).is_err() {
                    continue;
                }
                // 只处理完整的行，半行留到下次
                let end = buf.iter().rposition(|b| *b == b'\n').map(|i| i + 1).unwrap_or(0);
                cached.points.extend(parse_signins(&String::from_utf8_lossy(&buf[..end])));
                cached.len = start + end as u64;
            }
        }
        self.files.retain(|p, _| seen.contains(p));
        let mut points: Vec<Signin> = self.files.values().flat_map(|f| f.points.iter().cloned()).collect();
        points.sort_by_key(|p| p.at);
        points
    }
}

/// 去掉连续重复，得到“谁在什么时候登录”的时间线
pub fn changes(points: &[Signin]) -> Vec<Signin> {
    let mut out: Vec<Signin> = Vec::new();
    for p in points {
        if out.last().map_or(true, |l| l.acct != p.acct) {
            out.push(p.clone());
        }
    }
    out
}

// ------------------------------------------------------------------
// 桌面端进程：只看可执行文件路径，不读任何内容
// ------------------------------------------------------------------

pub fn is_desktop_exe(path: &str) -> bool {
    let s = path.replace('/', "\\").to_ascii_lowercase();
    if !s.ends_with("\\claude.exe") {
        return false;
    }
    let store_app = s.find("\\windowsapps\\").is_some_and(|i| store_package(&s[i + 13..]));
    store_app || s.contains("\\anthropicclaude\\") || Path::new(path).parent().is_some_and(|d| d.join("resources").join("app.asar").exists())
}

/// 桌面端的根进程（父进程不是桌面端的那些）
#[cfg(windows)]
pub fn desktop_pids() -> Vec<u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION};
    let mut found: Vec<(u32, u32)> = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return vec![] };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]);
            if name.eq_ignore_ascii_case("claude.exe") {
                if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, e.th32ProcessID) {
                    let mut buf = [0u16; 1024];
                    let mut size = buf.len() as u32;
                    if QueryFullProcessImageNameW(h, PROCESS_NAME_FORMAT(0), windows::core::PWSTR(buf.as_mut_ptr()), &mut size).is_ok() {
                        let path = String::from_utf16_lossy(&buf[..size as usize]);
                        if is_desktop_exe(&path) {
                            found.push((e.th32ProcessID, e.th32ParentProcessID));
                        }
                    }
                    let _ = CloseHandle(h);
                }
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    let ids: Vec<u32> = found.iter().map(|f| f.0).collect();
    found.into_iter().filter(|(_, parent)| !ids.contains(parent)).map(|f| f.0).collect()
}
#[cfg(not(windows))]
pub fn desktop_pids() -> Vec<u32> {
    vec![]
}

pub fn app_running() -> bool {
    if std::env::var("CLAUDE_HUB_SKIP_APP_CHECK").as_deref() == Ok("1") {
        return false;
    }
    !desktop_pids().is_empty()
}

/// 桌面端的关闭按钮只会缩到托盘，所以像托盘里的“退出”一样结束它的进程树
pub fn quit_app() -> bool {
    for pid in desktop_pids() {
        super::proc::kill_tree(pid);
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(20) {
        if !app_running() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

pub fn open_app() -> bool {
    super::proc::spawn_detached("explorer.exe", &[&format!("shell:AppsFolder\\{APP_ID}")])
}

// ------------------------------------------------------------------
// 同步与撤销
// ------------------------------------------------------------------

pub struct Synced {
    pub added: usize,
    pub replaced: usize,
    pub backup: Option<PathBuf>,
}

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// 把其他账号的会话列表条目复制到 acct。先整份备份目标目录，记下新增和替换了哪些，便于撤销。
pub fn sync_to(acct: &str) -> Result<Synced, String> {
    if app_running() {
        return Err("Claude 桌面端还开着，需要先关闭".into());
    }
    let lists = session_lists();
    let (dst, items) = plan_sync(&lists, acct).ok_or("桌面端里还没有这个账号的会话列表，先用它在桌面端登录一次")?;
    if items.is_empty() {
        return Ok(Synced { added: 0, replaced: 0, backup: None });
    }
    let short: String = acct.chars().take(8).collect();
    let bk = backups_dir().join(format!("{}-{short}", stamp()));
    fs::create_dir_all(&bk).map_err(|e| format!("无法建备份目录：{e}"))?;
    for e in &dst.entries {
        fs::copy(&e.file, bk.join(&e.name)).map_err(|e| format!("备份失败：{e}"))?;
    }
    let added: Vec<&str> = items.iter().filter(|i| !i.replace).map(|i| i.name.as_str()).collect();
    let replaced: Vec<&str> = items.iter().filter(|i| i.replace).map(|i| i.name.as_str()).collect();
    let manifest = json!({"version":1,"target":dst.dir,"account":acct,"created":store::now_ms(),"added":added,"replaced":replaced});
    fs::write(bk.join(MANIFEST), serde_json::to_vec_pretty(&manifest).unwrap_or_default()).map_err(|e| format!("备份失败：{e}"))?;
    for i in &items {
        fs::copy(&i.file, dst.dir.join(&i.name)).map_err(|e| format!("复制失败：{e}"))?;
    }
    Ok(Synced { added: added.len(), replaced: replaced.len(), backup: Some(bk) })
}

pub struct Backup {
    pub dir: PathBuf,
    pub manifest: Value,
}

/// 新的在前
pub fn backups() -> Vec<Backup> {
    let mut dirs = subdirs(&backups_dir());
    dirs.reverse();
    dirs.into_iter().filter_map(|dir| Some(Backup { manifest: store::read_json(&dir.join(MANIFEST))?, dir })).collect()
}

/// 撤销最近一次：删掉新增的条目，把被替换的放回去
pub fn undo_last() -> Result<(usize, usize), String> {
    if app_running() {
        return Err("Claude 桌面端还开着，需要先关闭".into());
    }
    let mut last = backups().into_iter().find(|b| b.manifest["undone"].is_null()).ok_or("没有可以撤销的同步")?;
    let target = PathBuf::from(last.manifest["target"].as_str().unwrap_or_default());
    let names = |k: &str| last.manifest[k].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect::<Vec<_>>();
    let (added, replaced) = (names("added"), names("replaced"));
    for n in &added {
        let _ = fs::remove_file(target.join(n));
    }
    for n in &replaced {
        fs::copy(last.dir.join(n), target.join(n)).map_err(|e| format!("放回失败：{e}"))?;
    }
    last.manifest["undone"] = json!(store::now_ms());
    let _ = fs::write(last.dir.join(MANIFEST), serde_json::to_vec_pretty(&last.manifest).unwrap_or_default());
    Ok((added.len(), replaced.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(dir: &Path, name: &str, last: i64, archived: bool) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), json!({"sessionId":name,"lastActivityAt":last,"isArchived":archived}).to_string()).unwrap();
    }
    #[test]
    fn plan_copies_newer_unarchived_entries_only() {
        let root = std::env::temp_dir().join(format!("island-sessions-{}", store::now_ms()));
        let a = root.join("acct-a").join("org");
        let b = root.join("acct-b").join("org");
        entry(&a, "local_1.json", 10, false);
        entry(&a, "local_2.json", 50, false);
        entry(&a, "local_3.json", 70, true);
        entry(&b, "local_2.json", 40, false);
        let lists = read_lists(&root);
        let (_, items) = plan_sync(&lists, "acct-b").unwrap();
        let names: Vec<_> = items.iter().map(|i| (i.name.as_str(), i.replace)).collect();
        assert_eq!(names, vec![("local_1.json", false), ("local_2.json", true)]);
        assert_eq!(missing(&lists, "acct-b"), Some(2));
        assert_eq!(missing(&lists, "nobody"), None);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn signins_only_take_account_id_and_time() {
        let text = "2026-11-04 18:22:01 [info] fetch cacheKey=acct:11111111-2222-4333-8444-555555555555 token=secret\nbroken line cacheKey=acct:short\n";
        let p = parse_signins(text);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].acct, "11111111-2222-4333-8444-555555555555");
    }
    #[test]
    fn desktop_exe_is_told_apart_from_cli() {
        assert!(is_desktop_exe("C:\\Program Files\\WindowsApps\\Claude_1.0.0_x64__pzs8sxrjxfjjc\\app\\claude.exe"));
        assert!(!is_desktop_exe("C:\\Users\\x\\.local\\bin\\claude.exe"));
        assert!(!is_desktop_exe("C:\\Program Files\\WindowsApps\\Other_1\\claude.exe"));
    }
}
