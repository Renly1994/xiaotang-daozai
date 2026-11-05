//! 当前账号在 Claude 桌面端里的对话任务：进行中、等你确认、已结束。
//! 只从会话列表条目取标题、目录和时间；对话记录只看每条记录的类型、结束原因和工具名，
//! 不保存、不记录对话正文。这些都是桌面端的内部格式，读不懂时就当没有任务，不会报错。
use super::store;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const DAY_MS: i64 = 24 * 3_600_000;
/// 记录这么久没变化还没结束，多半是对话已经断了
const STALE_SECS: i64 = 20 * 60;
const SLOW_TOOL_STALE_SECS: i64 = 60 * 60;
const DONE_KEEP_MS: i64 = 12 * 3_600_000;
const TAIL_BYTES: u64 = 256 * 1024;
const LONG_TAIL_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    Running,
    Waiting,
    Done,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub project: String,
    pub state: TaskState,
    /// 最后一次发消息的时间
    pub since: i64,
}

/// 对话最后停在哪一步
#[derive(Clone, Debug, PartialEq)]
pub enum Turn {
    Working,
    /// 明确在问用户：提问或计划确认
    Asking,
    /// 调了工具，结果还没回来
    Tool(String),
    Ended,
    Unknown,
}

/// 从记录尾部往回找最后一条有意义的记录。只读类型、结束原因和工具名。
pub fn last_turn(tail: &str) -> Turn {
    for line in tail.lines().rev() {
        let Ok(r) = serde_json::from_str::<Value>(line) else { continue };
        if r["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        match r["type"].as_str() {
            Some("system") => {
                if matches!(r["subtype"].as_str(), Some("stop_hook_summary" | "turn_duration")) {
                    return Turn::Ended;
                }
            }
            Some("assistant") => {
                let tool = r["message"]["content"].as_array().and_then(|c| c.iter().rev().find(|b| b["type"].as_str() == Some("tool_use"))).and_then(|b| b["name"].as_str());
                if let Some(name) = tool {
                    return if matches!(name, "AskUserQuestion" | "ExitPlanMode") { Turn::Asking } else { Turn::Tool(name.to_owned()) };
                }
                return if matches!(r["message"]["stop_reason"].as_str(), Some("end_turn" | "stop_sequence" | "max_tokens" | "refusal")) { Turn::Ended } else { Turn::Working };
            }
            Some("user") => {
                if r["isMeta"].as_bool() == Some(true) {
                    continue;
                }
                return if interrupted(&r["message"]["content"]) { Turn::Ended } else { Turn::Working };
            }
            _ => {}
        }
    }
    Turn::Unknown
}

/// 用户点了停止
fn interrupted(content: &Value) -> bool {
    const MARK: &str = "[Request interrupted";
    match content {
        Value::String(s) => s.starts_with(MARK),
        Value::Array(list) => list.iter().any(|b| b["text"].as_str().is_some_and(|s| s.starts_with(MARK))),
        _ => false,
    }
}

/// 工具等多久没结果就算“可能在等你批准”。对话记录里没有批准提示，这只能是推测：
/// 读文件这类工具本来一瞬间就完成，命令行和外部工具可能真的要跑很久，所以等得更久才算。
fn approval_wait_secs(tool: &str) -> Option<i64> {
    match tool {
        "Agent" | "Task" | "Workflow" => None,
        "Bash" | "PowerShell" => Some(60),
        t if t.starts_with("mcp__") => Some(60),
        _ => Some(20),
    }
}

pub fn state_of(turn: &Turn, idle_secs: i64, mode: &str) -> Option<TaskState> {
    match turn {
        Turn::Unknown => None,
        Turn::Ended => Some(TaskState::Done),
        Turn::Working => (idle_secs < STALE_SECS).then_some(TaskState::Running),
        Turn::Asking => (idle_secs * 1000 < DAY_MS).then_some(TaskState::Waiting),
        Turn::Tool(name) => match approval_wait_secs(name) {
            None => (idle_secs < SLOW_TOOL_STALE_SECS).then_some(TaskState::Running),
            Some(_) if mode == "bypassPermissions" => (idle_secs < SLOW_TOOL_STALE_SECS).then_some(TaskState::Running),
            Some(wait) if idle_secs < wait => Some(TaskState::Running),
            Some(_) => (idle_secs * 1000 < DAY_MS).then_some(TaskState::Waiting),
        },
    }
}

fn project_of(cwd: &str) -> String {
    if cwd.contains("scratch-workspaces") {
        return "临时目录".into();
    }
    cwd.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().filter(|s| !s.is_empty()).unwrap_or("未知目录").to_owned()
}

/// 桌面端按工作目录给对话记录分文件夹：字母数字以外的字符都换成短横线
fn slug(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

fn read_tail(path: &Path, len: u64, want: u64) -> Option<String> {
    let mut f = fs::File::open(path).ok()?;
    let start = len.saturating_sub(want);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut raw = Vec::with_capacity((len - start) as usize);
    f.take(want).read_to_end(&mut raw).ok()?;
    let text = String::from_utf8_lossy(&raw);
    // 从文件中间开始读时，第一行是半截
    Some(if start > 0 { text.split_once('\n').map(|x| x.1.to_owned()).unwrap_or_default() } else { text.into_owned() })
}

fn read_turn(path: &Path, len: u64) -> Turn {
    let turn = read_tail(path, len, TAIL_BYTES).map_or(Turn::Unknown, |t| last_turn(&t));
    if turn == Turn::Unknown && len > TAIL_BYTES {
        return read_tail(path, len, LONG_TAIL_BYTES).map_or(Turn::Unknown, |t| last_turn(&t));
    }
    turn
}

#[derive(Clone)]
struct Meta {
    id: String,
    cli: String,
    title: String,
    cwd: String,
    origin: String,
    mode: String,
    since: i64,
    last: i64,
    archived: bool,
}

fn read_meta(path: &Path) -> Option<Meta> {
    let d = store::read_json(path)?;
    let text = |k: &str| d[k].as_str().unwrap_or_default().to_owned();
    let last = d["lastActivityAt"].as_i64().unwrap_or(0);
    Some(Meta {
        id: d["sessionId"].as_str()?.to_owned(),
        cli: text("cliSessionId"),
        title: text("title"),
        cwd: text("cwd"),
        origin: text("originCwd"),
        mode: text("permissionMode"),
        since: d["latestUserFrameAt"].as_i64().unwrap_or(last),
        last,
        archived: d["isArchived"].as_bool().unwrap_or(false),
    })
}

pub struct Watcher {
    projects: PathBuf,
    entries: HashMap<PathBuf, (SystemTime, Option<Meta>)>,
    paths: HashMap<String, PathBuf>,
    turns: HashMap<String, (SystemTime, u64, Turn)>,
    /// 按第一次出现的顺序排，状态变了位置不变，这样编号才稳定
    tasks: Vec<Task>,
}

impl Default for Watcher {
    fn default() -> Self {
        Self::new(store::home().join(".claude").join("projects"))
    }
}

impl Watcher {
    pub fn new(projects: PathBuf) -> Self {
        Self { projects, entries: HashMap::new(), paths: HashMap::new(), turns: HashMap::new(), tasks: Vec::new() }
    }
    pub fn list(&self) -> Vec<Task> {
        self.tasks.clone()
    }
    /// 用户看过之后，已结束的任务不再占位
    pub fn dismiss_done(&mut self) {
        self.tasks.retain(|t| t.state != TaskState::Done);
    }

    fn read_entries(&mut self, dirs: &[PathBuf]) -> Vec<Meta> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for dir in dirs {
            for e in fs::read_dir(dir).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if !(name.starts_with("local_") && name.ends_with(".json")) {
                    continue;
                }
                let path = e.path();
                let Some(mtime) = e.metadata().ok().and_then(|m| m.modified().ok()) else { continue };
                seen.insert(path.clone());
                let cached = self.entries.get(&path).filter(|c| c.0 == mtime).map(|c| c.1.clone());
                let meta = cached.unwrap_or_else(|| {
                    let meta = read_meta(&path);
                    self.entries.insert(path.clone(), (mtime, meta.clone()));
                    meta
                });
                out.extend(meta);
            }
        }
        self.entries.retain(|p, _| seen.contains(p));
        out
    }

    fn transcript(&mut self, m: &Meta) -> Option<PathBuf> {
        if let Some(p) = self.paths.get(&m.cli).filter(|p| p.is_file()) {
            return Some(p.clone());
        }
        let name = format!("{}.jsonl", m.cli);
        let direct = self.projects.join(slug(&m.cwd)).join(&name);
        let found = if direct.is_file() {
            Some(direct)
        } else {
            // 目录名太长时桌面端会截断，找不到就在各项目目录里找一次
            fs::read_dir(&self.projects).into_iter().flatten().flatten().map(|e| e.path().join(&name)).find(|p| p.is_file())
        };
        if let Some(p) = &found {
            self.paths.insert(m.cli.clone(), p.clone());
        }
        found
    }

    /// 返回任务列表有没有变化
    pub fn scan(&mut self, dirs: &[PathBuf], app_running: bool, now: i64) -> bool {
        let before = self.tasks.clone();
        if !app_running {
            // 桌面端关了，没结束的任务也不会再动
            self.tasks.retain(|t| t.state == TaskState::Done);
        } else {
            let mut live = HashSet::new();
            for m in self.read_entries(dirs) {
                if m.archived || m.cli.is_empty() || now - m.last > DAY_MS {
                    continue;
                }
                let Some(path) = self.transcript(&m) else { continue };
                let Ok(md) = fs::metadata(&path) else { continue };
                let Ok(mtime) = md.modified() else { continue };
                let len = md.len();
                let turn = match self.turns.get(&m.id).filter(|c| c.0 == mtime && c.1 == len) {
                    Some(c) => c.2.clone(),
                    None => {
                        let turn = read_turn(&path, len);
                        self.turns.insert(m.id.clone(), (mtime, len, turn.clone()));
                        turn
                    }
                };
                let changed_ms = mtime.duration_since(SystemTime::UNIX_EPOCH).map_or(now, |d| d.as_millis() as i64);
                let state = state_of(&turn, (now - changed_ms).max(0) / 1000, &m.mode);
                let at = self.tasks.iter().position(|t| t.id == m.id);
                let title = if m.title.trim().is_empty() { "未命名对话".to_owned() } else { m.title.clone() };
                let project = project_of(if m.origin.is_empty() { &m.cwd } else { &m.origin });
                match (at, state) {
                    (Some(i), Some(state)) => {
                        let t = &mut self.tasks[i];
                        (t.title, t.project, t.state, t.since) = (title, project, state, m.since);
                        live.insert(m.id.clone());
                    }
                    // 没见过它在跑，就不算刚结束：刚启动时不把以前的对话都列出来
                    (None, Some(TaskState::Running | TaskState::Waiting)) => {
                        self.tasks.push(Task { id: m.id.clone(), title, project, state: state.unwrap_or(TaskState::Running), since: m.since });
                        live.insert(m.id.clone());
                    }
                    _ => {}
                }
            }
            self.tasks.retain(|t| live.contains(&t.id) && !(t.state == TaskState::Done && now - t.since > DONE_KEEP_MS));
        }
        before != self.tasks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(v: Value) -> String {
        format!("{v}\n")
    }
    fn assistant(stop: &str, blocks: Value) -> String {
        line(json!({"type":"assistant","message":{"stop_reason":stop,"content":blocks}}))
    }
    fn tool(name: &str) -> String {
        assistant("tool_use", json!([{"type":"tool_use","name":name,"input":{"secret":"不该被读取"}}]))
    }

    #[test]
    fn turns_are_read_from_record_types_only() {
        assert_eq!(last_turn(""), Turn::Unknown);
        assert_eq!(last_turn(&assistant("end_turn", json!([{"type":"text","text":"正文"}]))), Turn::Ended);
        assert_eq!(last_turn(&tool("Read")), Turn::Tool("Read".into()));
        assert_eq!(last_turn(&tool("AskUserQuestion")), Turn::Asking);
        assert_eq!(last_turn(&tool("ExitPlanMode")), Turn::Asking);
        assert_eq!(last_turn(&line(json!({"type":"user","message":{"content":[{"type":"tool_result"}]}}))), Turn::Working);
        assert_eq!(last_turn(&line(json!({"type":"user","message":{"content":"帮我改一下"}}))), Turn::Working);
        assert_eq!(last_turn(&line(json!({"type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}))), Turn::Ended);
        assert_eq!(last_turn(&line(json!({"type":"system","subtype":"stop_hook_summary"}))), Turn::Ended);
    }
    #[test]
    fn bookkeeping_and_subagent_records_do_not_change_the_turn() {
        let tail = tool("Read") + &line(json!({"type":"last-prompt"})) + &line(json!({"type":"custom-title"})) + &line(json!({"type":"assistant","isSidechain":true,"message":{"stop_reason":"end_turn","content":[]}})) + "这一行坏了\n";
        assert_eq!(last_turn(&tail), Turn::Tool("Read".into()));
    }
    #[test]
    fn a_quiet_tool_counts_as_waiting_only_after_its_own_wait() {
        let read = Turn::Tool("Read".into());
        let shell = Turn::Tool("Bash".into());
        assert_eq!(state_of(&read, 5, "auto"), Some(TaskState::Running));
        assert_eq!(state_of(&read, 25, "auto"), Some(TaskState::Waiting));
        assert_eq!(state_of(&shell, 25, "auto"), Some(TaskState::Running), "命令行可能真的在跑");
        assert_eq!(state_of(&shell, 70, "auto"), Some(TaskState::Waiting));
        assert_eq!(state_of(&shell, 70, "bypassPermissions"), Some(TaskState::Running), "这个模式不会弹批准");
        assert_eq!(state_of(&Turn::Tool("Agent".into()), 600, "auto"), Some(TaskState::Running), "子任务不弹批准，只是久");
        assert_eq!(state_of(&Turn::Working, STALE_SECS + 1, "auto"), None, "断掉的对话不能一直显示进行中");
        assert_eq!(state_of(&Turn::Asking, 3600, "auto"), Some(TaskState::Waiting));
    }
    #[test]
    fn projects_come_from_the_folder_name() {
        assert_eq!(project_of("D:\\work\\账号岛\\"), "账号岛");
        assert_eq!(project_of("/home/me/site"), "site");
        assert_eq!(project_of("C:\\x\\scratch-workspaces\\a\\b"), "临时目录");
        assert_eq!(slug("D:\\测试目录"), "D------");
    }

    struct Fixture {
        root: PathBuf,
        list: PathBuf,
        projects: PathBuf,
    }
    impl Fixture {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!("daozai-tasks-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let (list, projects) = (root.join("list"), root.join("projects"));
            fs::create_dir_all(&list).unwrap();
            fs::create_dir_all(projects.join("C--work")).unwrap();
            Self { root, list, projects }
        }
        fn entry(&self, id: &str, now: i64, archived: bool) {
            let v = json!({"sessionId":id,"cliSessionId":format!("cli-{id}"),"title":format!("任务 {id}"),"cwd":"C:\\work","permissionMode":"auto","lastActivityAt":now,"latestUserFrameAt":now,"isArchived":archived});
            fs::write(self.list.join(format!("local_{id}.json")), v.to_string()).unwrap();
        }
        fn transcript(&self, id: &str, text: &str) {
            fs::write(self.projects.join("C--work").join(format!("cli-{id}.jsonl")), text).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn numbers_stay_put_and_finished_tasks_wait_to_be_seen() {
        let f = Fixture::new("flow");
        let now = store::now_ms();
        let dirs = vec![f.list.clone()];
        let mut w = Watcher::new(f.projects.clone());
        // 启动前就结束的对话不列出来
        f.entry("old", now, false);
        f.transcript("old", &assistant("end_turn", json!([])));
        assert!(!w.scan(&dirs, true, now));
        assert!(w.list().is_empty());
        // 两个任务开始
        for id in ["a", "b"] {
            f.entry(id, now, false);
            f.transcript(id, &line(json!({"type":"user","message":{"content":"开始"}})));
        }
        assert!(w.scan(&dirs, true, now));
        let names: Vec<String> = w.list().iter().map(|t| t.title.clone()).collect();
        assert_eq!(w.list().len(), 2);
        assert!(w.list().iter().all(|t| t.state == TaskState::Running && t.project == "work"));
        // 第一个结束：状态变了，位置和编号不变
        let first = w.list()[0].id.clone();
        f.transcript(&first, &(line(json!({"type":"user","message":{"content":"开始"}})) + &assistant("end_turn", json!([]))));
        assert!(w.scan(&dirs, true, now));
        assert_eq!(w.list().iter().map(|t| t.title.clone()).collect::<Vec<_>>(), names);
        assert_eq!(w.list()[0].state, TaskState::Done);
        // 看过之后消失，也不会因为还是“已结束”又冒出来
        w.dismiss_done();
        w.scan(&dirs, true, now);
        assert_eq!(w.list().len(), 1);
        // 桌面端关了，没结束的也清掉
        assert!(w.scan(&dirs, false, now));
        assert!(w.list().is_empty());
    }
    /// 只打印状态、项目和标题长度，不打印标题和对话内容
    #[test]
    #[ignore = "按需核对本机读到的任务"]
    fn real_tasks_probe() {
        use super::super::desktop;
        let mut timeline = desktop::Timeline::default();
        let acct = desktop::changes(&timeline.read()).last().map(|p| p.acct.clone()).expect("桌面端日志里没有登录记录");
        let dirs = desktop::list_dirs(&acct);
        let mut w = Watcher::default();
        w.scan(&dirs, true, store::now_ms());
        println!("list_dirs={} tasks={}", dirs.len(), w.list().len());
        for t in w.list() {
            println!("{:?} project={} title_chars={}", t.state, t.project, t.title.chars().count());
        }
    }
    #[test]
    fn archived_and_old_conversations_are_left_out() {
        let f = Fixture::new("skip");
        let now = store::now_ms();
        let mut w = Watcher::new(f.projects.clone());
        f.entry("arch", now, true);
        f.transcript("arch", &tool("Read"));
        f.entry("old", now - 2 * DAY_MS, false);
        f.transcript("old", &tool("Read"));
        w.scan(&[f.list.clone()], true, now);
        assert!(w.list().is_empty());
    }
}
