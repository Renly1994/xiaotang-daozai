//! 后台线程：读账号、同步会话列表、切换桌面端账号、取官方额度。界面只发请求、收快照。
use super::store::{self, Account, Group};
use super::usage::{self, Limits};
use super::{codex, desktop, heat, login, secrets, tasks};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const CURRENT_EVERY: i64 = 10 * 60 * 1000;
const OTHERS_EVERY: i64 = 60 * 60 * 1000;

pub enum Req {
    /// 关闭多账号模式或退出：结束后台线程
    Stop,
    Reload,
    /// 用户展开了额度页：当前账号的数据旧了就问一次
    Opened,
    Refresh(Option<String>),
    Switch(String),
    CancelWait,
    Undo,
    Login(String),
    CycleStatus(String),
    Remove(String),
    Bind(String, String),
    Unbind(String),
    Add { group: Group, name: String, email: String },
    Update { id: String, field: &'static str, value: String },
    SetPassword(String, String),
    Reveal(String),
    Copy(String),
    OpenTerminal(String),
    SetAutoFill(bool),
    SetAlert(Alert),
    /// 用户看过了，已结束的任务不再占位
    DismissDone,
    Heat(String),
    QuotaDone(Vec<(String, Result<(), String>)>),
    LoginDone(String, Result<String, String>),
}

pub enum Ev {
    View(Box<View>),
    Note(String),
    /// 用户点“显示”后的明文，界面 10 秒后丢弃
    Secret(String, String),
    /// 交给界面复制到剪贴板，30 秒后若没变就清空
    Copy(String),
    /// 当前账号的额度用完了。每个重置周期第一次 repeat 为 false；还没换号时按设置再提醒。
    /// 只是提醒，换不换号由用户决定。
    Exhausted { id: String, window: String, resets_at: Option<i64>, repeat: bool },
    /// 当前账号的对话任务变了
    Tasks(Vec<tasks::Task>),
    /// 近 4 周每天的用量
    Heat(String, Vec<u64>),
}

/// 额度用完时怎么提醒
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alert {
    /// 第一次提醒震动几秒，0 表示不震动
    pub shake_secs: u32,
    /// 没换号时隔几分钟再提醒，0 表示不再提醒
    pub remind_min: u32,
    pub remind_shake: bool,
}

impl Default for Alert {
    fn default() -> Self {
        Self { shake_secs: 3, remind_min: 5, remind_shake: false }
    }
}

fn alert_of(d: &Value) -> Alert {
    let a = &d["settings"]["quotaAlert"];
    let base = Alert::default();
    Alert {
        shake_secs: a["shakeSecs"].as_u64().map_or(base.shake_secs, |v| v.min(10) as u32),
        remind_min: a["remindMin"].as_u64().map_or(base.remind_min, |v| v.min(120) as u32),
        remind_shake: a["remindShake"].as_bool().unwrap_or(base.remind_shake),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Running { target: String, steps: Vec<String>, at: usize },
    /// 桌面端已重新打开，等它日志里出现目标账号的登录
    Waiting { target: String, still: Option<String>, wrong: Option<String> },
}

#[derive(Clone, Debug)]
pub struct AccountView {
    pub a: Account,
    pub lim: Option<Limits>,
    pub sessions: Option<usize>,
    pub missing: Option<usize>,
    pub signed_in: bool,
    pub has_password: bool,
    pub quota_note: String,
    pub exhausted: Vec<String>,
    pub backups: usize,
    pub logging_in: bool,
}

#[derive(Clone, Debug)]
pub struct View {
    pub accounts: Vec<AccountView>,
    pub current: String,
    pub current_codex: String,
    pub phase: Phase,
    pub app_running: bool,
    pub quota_busy: bool,
    pub unbound: Option<String>,
    pub auto_fill: bool,
    pub last_auto_fill: String,
    pub alert: Alert,
}

impl View {
    pub fn get(&self, id: &str) -> Option<&AccountView> {
        self.accounts.iter().find(|a| a.a.id == id)
    }
    pub fn is_current(&self, a: &Account) -> bool {
        match a.group {
            Group::Claude => self.current == a.id,
            Group::Codex => self.current_codex == a.id,
        }
    }
    /// 能不能切过去：正常状态，Claude 账号还要已对上桌面端账号、桌面端里有它的会话列表
    pub fn can_switch(&self, v: &AccountView) -> Result<(), &'static str> {
        if self.is_current(&v.a) {
            return Err("正在使用");
        }
        if v.a.status != "active" {
            return Err(if v.a.status == "banned" { "这个账号已封号，只能查看以前的记录" } else { "这个账号已停用，只能查看以前的记录" });
        }
        if v.a.group == Group::Claude && (v.a.account_uuid.is_none() || v.sessions.is_none()) {
            return Err("还没在 Claude 桌面端登录验证过：先用它在桌面端登录一次，再到设置里对上账号");
        }
        Ok(())
    }
}

pub struct Service {
    tx: mpsc::Sender<Req>,
    rx: mpsc::Receiver<Ev>,
}

impl Service {
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (tx, requests) = mpsc::channel();
        let (out, rx) = mpsc::channel();
        let worker_tx = tx.clone();
        std::thread::Builder::new()
            .name("account-island".into())
            .spawn(move || {
                let mut w = Worker {
                    tx: worker_tx,
                    out,
                    wake: Arc::new(wake),
                    heat: Arc::new(Mutex::new(heat::Cache::default())),
                    tl: desktop::Timeline::default(),
                    phase: Phase::Idle,
                    switch: None,
                    running: None,
                    busy_until: Instant::now(),
                    quota_busy: false,
                    quota_notes: HashMap::new(),
                    attempts: HashMap::new(),
                    logins: HashSet::new(),
                    last_view: Instant::now() - Duration::from_secs(3600),
                    newest: None,
                    activity_checked: Instant::now(),
                    activity_refreshed: Instant::now() - Duration::from_secs(3600),
                    reminded_at: None,
                    watcher: tasks::Watcher::default(),
                    task_acct: None,
                    task_dirs: Vec::new(),
                    task_dirs_at: Instant::now(),
                };
                w.publish();
                loop {
                    match requests.recv_timeout(Duration::from_secs(3)) {
                        Ok(Req::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Ok(req) => w.handle(req),
                        Err(mpsc::RecvTimeoutError::Timeout) => w.tick(),
                    }
                }
            })
            .ok();
        Self { tx, rx }
    }
    pub fn send(&self, r: Req) {
        let _ = self.tx.send(r);
    }
    pub fn try_recv(&self) -> Option<Ev> {
        self.rx.try_recv().ok()
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.send(Req::Stop);
    }
}

struct SwitchCtx {
    id: String,
    uuid: String,
    since: i64,
    prev: Option<String>,
}

struct Worker {
    tx: mpsc::Sender<Req>,
    out: mpsc::Sender<Ev>,
    wake: Arc<dyn Fn() + Send + Sync>,
    heat: Arc<Mutex<heat::Cache>>,
    tl: desktop::Timeline,
    phase: Phase,
    switch: Option<SwitchCtx>,
    running: Option<bool>,
    busy_until: Instant,
    quota_busy: bool,
    quota_notes: HashMap<String, String>,
    attempts: HashMap<String, i64>,
    logins: HashSet<String>,
    last_view: Instant,
    newest: Option<std::time::SystemTime>,
    activity_checked: Instant,
    activity_refreshed: Instant,
    /// 上次提醒当前账号额度用完的时间
    reminded_at: Option<Instant>,
    watcher: tasks::Watcher,
    task_acct: Option<String>,
    task_dirs: Vec<std::path::PathBuf>,
    task_dirs_at: Instant,
}

/// 默认目录下最新一个对话文件的修改时间：只看文件时间，不读内容
fn newest_transcript() -> Option<std::time::SystemTime> {
    let root = store::home().join(".claude").join("projects");
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .flat_map(|project| std::fs::read_dir(project.path()).into_iter().flatten().flatten())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".jsonl"))
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()
}

fn name_of(accs: &[Account], uuid: &str) -> String {
    accs.iter().find(|a| a.account_uuid.as_deref() == Some(uuid)).map(|a| a.id.clone()).unwrap_or_else(|| format!("{}（未登记）", &uuid[..8.min(uuid.len())]))
}

fn limits_of(a: &Account) -> Option<Limits> {
    match a.group {
        Group::Claude => usage::quota_dir(a).and_then(|d| usage::read_for(&d)),
        Group::Codex => codex::latest(&codex::home_of(a)),
    }
}

fn clock(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map(|d| d.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string()).unwrap_or_default()
}

impl Worker {
    fn emit(&self, ev: Ev) {
        let _ = self.out.send(ev);
        (self.wake)();
    }
    fn note(&self, text: impl Into<String>) {
        self.emit(Ev::Note(text.into()));
    }

    fn view(&mut self) -> View {
        let d = store::load();
        let accs = store::accounts(&d);
        let lists = desktop::session_lists();
        let changes = desktop::changes(&self.tl.read());
        let signed = changes.last().map(|p| p.acct.clone());
        let backups = desktop::backups();
        let exhausted: Vec<Value> = d["exhausted"].as_array().cloned().unwrap_or_default();
        let accounts: Vec<AccountView> = accs
            .into_iter()
            .map(|a| {
                let uuid = a.account_uuid.clone();
                AccountView {
                    lim: limits_of(&a),
                    sessions: uuid.as_deref().and_then(|u| desktop::visible_count(&lists, u)),
                    missing: uuid.as_deref().and_then(|u| desktop::missing(&lists, u)),
                    signed_in: uuid.is_some() && uuid == signed,
                    has_password: secrets::has(&format!("acc:{}", a.id)),
                    quota_note: self.quota_notes.get(&a.id).cloned().unwrap_or_default(),
                    exhausted: exhausted
                        .iter()
                        .rev()
                        .filter(|x| x["account"].as_str() == Some(a.id.as_str()))
                        .map(|x| format!("{} · {}用完", clock(x["at"].as_i64().unwrap_or(0)), x["window"].as_str().unwrap_or("额度")))
                        .collect(),
                    backups: backups.iter().filter(|b| uuid.is_some() && b.manifest["account"].as_str() == uuid.as_deref() && b.manifest["undone"].is_null()).count(),
                    logging_in: self.logins.contains(&a.id),
                    a,
                }
            })
            .collect();
        // 任务只看现在登录在桌面端的那个账号
        self.task_acct = signed.clone().or_else(|| accounts.iter().find(|v| d["current"].as_str() == Some(v.a.id.as_str())).and_then(|v| v.a.account_uuid.clone()));
        let unbound = signed.filter(|s| !accounts.iter().any(|v| v.a.account_uuid.as_deref() == Some(s.as_str())));
        let last = &d["lastAutoFill"];
        View {
            alert: alert_of(&d),
            current: d["current"].as_str().unwrap_or_default().into(),
            current_codex: d["currentCodex"].as_str().unwrap_or_default().into(),
            phase: self.phase.clone(),
            app_running: self.running.unwrap_or(false),
            quota_busy: self.quota_busy,
            unbound,
            auto_fill: d["settings"]["autoFill"].as_bool() != Some(false),
            last_auto_fill: last["account"].as_str().map(|n| format!("{} 给 {n} 补了 {} 条", clock(last["at"].as_i64().unwrap_or(0)), last["added"].as_i64().unwrap_or(0) + last["replaced"].as_i64().unwrap_or(0))).unwrap_or_default(),
            accounts,
        }
    }

    fn publish(&mut self) {
        let v = self.view();
        self.last_view = Instant::now();
        self.remind_exhausted(&v);
        self.emit(Ev::View(Box::new(v)));
    }

    /// 当前账号额度用完时提醒一次；记在配置里，重启也不重复。
    /// 之后如果当前 Claude 账号还没换号，按设置隔一段时间再提醒。不换号，不挑账号。
    fn remind_exhausted(&mut self, v: &View) {
        let now = store::now_ms();
        let mut claude_out = None;
        for id in [&v.current, &v.current_codex] {
            let Some(acc) = v.get(id) else { continue };
            let Some(lim) = &acc.lim else { continue };
            let Some((window, resets_at)) = usage::exhausted_window(lim, now) else { continue };
            if id == &v.current {
                claude_out = Some((window, resets_at));
            }
            let key = format!("{window}:{}", resets_at.map_or_else(|| store::ymd(now), |t| t.to_string()));
            let fresh = store::mutate(|d| {
                if !d["quotaReminded"].is_object() {
                    d["quotaReminded"] = json!({});
                }
                if d["quotaReminded"][id.as_str()].as_str() == Some(key.as_str()) {
                    return Err(String::new());
                }
                d["quotaReminded"][id.as_str()] = json!(key);
                Ok(())
            });
            if fresh.is_ok() {
                self.emit(Ev::Exhausted { id: id.clone(), window: window.into(), resets_at, repeat: false });
            }
        }
        let Some((window, resets_at)) = claude_out else {
            self.reminded_at = None;
            return;
        };
        let every = Duration::from_secs(v.alert.remind_min as u64 * 60);
        match self.reminded_at {
            None => self.reminded_at = Some(Instant::now()),
            Some(at) if v.alert.remind_min > 0 && at.elapsed() >= every && self.phase == Phase::Idle => {
                self.reminded_at = Some(Instant::now());
                self.emit(Ev::Exhausted { id: v.current.clone(), window: window.into(), resets_at, repeat: true });
            }
            Some(_) => {}
        }
    }

    /// 看一眼当前账号的对话任务，有变化才告诉界面
    fn scan_tasks(&mut self) {
        let Some(acct) = self.task_acct.clone() else { return };
        if self.task_dirs.is_empty() || self.task_dirs_at.elapsed() >= Duration::from_secs(30) {
            self.task_dirs = desktop::list_dirs(&acct);
            self.task_dirs_at = Instant::now();
        }
        let running = self.running.unwrap_or(false);
        if self.watcher.scan(&self.task_dirs, running, store::now_ms()) {
            self.emit(Ev::Tasks(self.watcher.list()));
        }
    }

    /// 对话有新回复时（文件时间变了）问一次当前账号的额度，两次之间至少隔 1 分钟
    fn watch_activity(&mut self) {
        if self.activity_checked.elapsed() < Duration::from_secs(30) {
            return;
        }
        self.activity_checked = Instant::now();
        self.start_quota(false);
        let newest = newest_transcript();
        let changed = self.newest.is_some() && newest > self.newest;
        if newest.is_some() {
            self.newest = newest;
        }
        if changed && self.phase == Phase::Idle && self.activity_refreshed.elapsed() >= Duration::from_secs(60) {
            self.activity_refreshed = Instant::now();
            let cur = store::load()["current"].as_str().unwrap_or_default().to_owned();
            if !cur.is_empty() {
                self.start_quota_for(vec![cur]);
            }
        }
    }

    fn set_phase(&mut self, p: Phase) {
        self.phase = p;
        self.publish();
    }

    fn handle(&mut self, req: Req) {
        let r: Result<Option<String>, String> = match req {
            Req::Stop | Req::Reload => Ok(None),
            Req::Opened => {
                self.start_quota(true);
                Ok(None)
            }
            Req::Refresh(id) => {
                if let Some(id) = id {
                    self.attempts.remove(&id);
                    self.start_quota_for(vec![id]);
                } else {
                    self.attempts.clear();
                    self.start_quota(false);
                }
                Ok(None)
            }
            Req::Switch(id) => self.switch(&id).map(Some),
            Req::CancelWait => {
                if let Some(ctx) = self.switch.take() {
                    self.phase = Phase::Idle;
                    self.busy_until = Instant::now() + Duration::from_secs(15);
                    Ok(Some(format!("已取消切换到 {}，桌面端的账号不变", ctx.id)))
                } else {
                    Ok(None)
                }
            }
            Req::Undo => self.undo().map(Some),
            Req::Login(id) => self.login(&id).map(Some),
            Req::CycleStatus(id) => store::mutate(|d| {
                let cur = d["current"].as_str() == Some(id.as_str()) || d["currentCodex"].as_str() == Some(id.as_str());
                let a = store::account_mut(d, &id)?;
                let next = match a["status"].as_str() {
                    Some("disabled") => "banned",
                    Some("banned") => "active",
                    _ => "disabled",
                };
                if cur && next != "active" {
                    return Err("当前账号不能标记停用或封号，先切到别的账号".into());
                }
                a["status"] = json!(next);
                a["statusAt"] = json!(if next == "active" { String::new() } else { store::ymd(store::now_ms()) });
                Ok(None)
            }),
            Req::Remove(id) => store::mutate(|d| {
                if d["current"].as_str() == Some(id.as_str()) || d["currentCodex"].as_str() == Some(id.as_str()) {
                    return Err("不能移除当前账号".into());
                }
                if let Some(list) = d["accounts"].as_array_mut() {
                    list.retain(|a| a["id"].as_str() != Some(id.as_str()));
                }
                Ok(())
            })
            .and_then(|_| secrets::set(&format!("acc:{id}"), "").map(|_| Some(format!("已移除 {id}（它的登录和会话文件都还在，只是不再管理）")))),
            Req::Bind(id, uuid) => self.bind(&id, &uuid).map(Some),
            Req::Unbind(id) => store::mutate(|d| {
                if let Some(o) = store::account_mut(d, &id)?.as_object_mut() {
                    o.remove("accountUuid");
                }
                Ok(Some(format!("已解绑 {id} 的桌面端账号 ID")))
            }),
            Req::Add { group, name, email } => self.add(group, name, email),
            Req::Update { id, field, value } => {
                if !["email", "mailUrl", "plan", "regDate", "note"].contains(&field) {
                    Err("不能改这一项".into())
                } else {
                    store::mutate(|d| {
                        store::account_mut(d, &id)?[field] = json!(value.trim().chars().take(300).collect::<String>());
                        Ok(None)
                    })
                }
            }
            Req::SetPassword(id, value) => secrets::set(&format!("acc:{id}"), &value).map(|_| Some(if value.is_empty() { "已删除保存的密码".into() } else { "密码已加密保存".into() })),
            Req::Reveal(id) => match secrets::get(&format!("acc:{id}")) {
                Some(v) => {
                    self.emit(Ev::Secret(id, v));
                    return;
                }
                None => Err(if secrets::has(&format!("acc:{id}")) { "这个密码解不开，请到设置里重新保存".into() } else { "这个账号没有保存密码".into() }),
            },
            Req::Copy(id) => match secrets::get(&format!("acc:{id}")) {
                Some(v) => {
                    let keep = v.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_secs(30));
                        clear_clipboard_if(&keep);
                    });
                    self.emit(Ev::Copy(v));
                    Ok(Some("密码已复制，30 秒后清空剪贴板".to_string()))
                }
                None => Err(if secrets::has(&format!("acc:{id}")) { "这个密码解不开，请到设置里重新保存".into() } else { "这个账号没有保存密码".into() }),
            },
            Req::OpenTerminal(id) => self.open_terminal(&id).map(Some),
            Req::SetAutoFill(on) => store::mutate(|d| {
                d["settings"]["autoFill"] = json!(on);
                Ok(None)
            }),
            Req::SetAlert(a) => store::mutate(|d| {
                d["settings"]["quotaAlert"] = json!({"shakeSecs": a.shake_secs, "remindMin": a.remind_min, "remindShake": a.remind_shake});
                Ok(None)
            }),
            Req::DismissDone => {
                self.watcher.dismiss_done();
                self.emit(Ev::Tasks(self.watcher.list()));
                return;
            }
            Req::Heat(id) => {
                let d = store::load();
                let Some(a) = store::accounts(&d).into_iter().find(|a| a.id == id) else { return };
                let switches = d["switches"].as_array().cloned().unwrap_or_default();
                let (cache, out, wake) = (self.heat.clone(), self.out.clone(), self.wake.clone());
                std::thread::spawn(move || {
                    let days = cache.lock().map(|mut c| c.daily(&a, &switches)).unwrap_or_default();
                    let _ = out.send(Ev::Heat(id, days));
                    wake();
                });
                return;
            }
            Req::QuotaDone(results) => {
                self.quota_busy = false;
                for (id, r) in results {
                    match r {
                        Ok(()) => self.quota_notes.remove(&id),
                        Err(e) => self.quota_notes.insert(id, e),
                    };
                }
                Ok(None)
            }
            Req::LoginDone(id, r) => {
                self.logins.remove(&id);
                match r {
                    Ok(email) => {
                        self.quota_notes.remove(&id);
                        self.start_quota_for(vec![id.clone()]);
                        Ok(Some(format!("{id} 已在无痕窗口登录（{email}），正在取额度")))
                    }
                    Err(e) => {
                        self.quota_notes.insert(id.clone(), e.clone());
                        Err(format!("{id}：{e}"))
                    }
                }
            }
        };
        match r {
            Ok(Some(text)) => self.note(text),
            Ok(None) => {}
            Err(e) => self.note(e),
        }
        self.publish();
    }

    fn tick(&mut self) {
        let running = desktop::app_running();
        let was = self.running.replace(running);
        if was == Some(true) && !running && self.phase == Phase::Idle && Instant::now() >= self.busy_until {
            self.auto_fill();
        }
        let mut dirty = was != Some(running);
        if self.switch.is_some() {
            dirty |= self.check_wait();
        } else if self.phase == Phase::Idle {
            dirty |= self.follow();
        }
        self.watch_activity();
        self.scan_tasks();
        if dirty || self.last_view.elapsed() >= Duration::from_secs(60) {
            self.publish();
        }
    }

    // ---------------- 切换 ----------------

    fn switch(&mut self, id: &str) -> Result<String, String> {
        if matches!(self.phase, Phase::Running { .. }) {
            return Err("正在切换，请稍等".into());
        }
        let d = store::load();
        let accs = store::accounts(&d);
        let a = accs.iter().find(|a| a.id == id).ok_or("没有这个账号")?.clone();
        if a.status != "active" {
            return Err("停用或封号的账号不能切换".into());
        }
        if a.group == Group::Codex {
            let prev = d["currentCodex"].as_str().unwrap_or_default().to_owned();
            let lim = accs.iter().find(|x| x.id == prev).and_then(limits_of);
            store::mutate(|d| {
                usage::record_exhausted(d, &prev, id, lim.as_ref(), store::now_ms());
                d["currentCodex"] = json!(id);
                Ok(())
            })?;
            return Ok(format!("Codex 已切到 {id}：之后打开的终端用它的登录"));
        }
        let uuid = a.account_uuid.clone().ok_or("这个账号还没在 Claude 桌面端登录验证过，不能切换")?;
        let lists = desktop::session_lists();
        if desktop::list_of(&lists, &uuid).is_none() {
            return Err("桌面端里还没有这个账号的会话列表，先用它在桌面端登录一次".into());
        }
        let missing = desktop::missing(&lists, &uuid).unwrap_or(0);
        let steps = vec![
            "强制关闭 Claude 桌面端".to_string(),
            if missing > 0 { format!("同步会话列表（{missing} 条）") } else { "同步会话列表（已是全的）".into() },
            "自动启动 Claude 桌面端".into(),
            "检测桌面端登录".into(),
        ];
        self.switch = None;
        let prev = desktop::changes(&self.tl.read()).last().map(|p| p.acct.clone());
        let since = store::now_ms();
        self.busy_until = Instant::now() + Duration::from_secs(3600);
        self.set_phase(Phase::Running { target: id.into(), steps: steps.clone(), at: 0 });
        if desktop::app_running() && !desktop::quit_app() {
            self.phase = Phase::Idle;
            self.busy_until = Instant::now();
            return Err("桌面端没能关闭，请从托盘退出后再试".into());
        }
        self.set_phase(Phase::Running { target: id.into(), steps: steps.clone(), at: 1 });
        let synced = desktop::sync_to(&uuid);
        let mut text = match &synced {
            Ok(s) if s.added + s.replaced > 0 => {
                store::write_log("sync.log", json!({"at": chrono::Utc::now().to_rfc3339(), "op": "desktop-sessions", "from": "island", "account": id, "added": s.added, "replaced": s.replaced, "backup": s.backup}));
                format!("同步了 {} 条会话列表；", s.added + s.replaced)
            }
            Ok(_) => String::new(),
            Err(e) => format!("会话列表没同步（{e}）；"),
        };
        self.set_phase(Phase::Running { target: id.into(), steps, at: 2 });
        if !desktop::open_app() {
            text.push_str("桌面端没能自动打开，请手动打开；");
        }
        let still = prev.as_ref().filter(|p| **p != uuid).map(|p| name_of(&accs, p));
        self.switch = Some(SwitchCtx { id: id.into(), uuid, since, prev });
        self.phase = Phase::Waiting { target: id.into(), still, wrong: None };
        text.push_str("请在桌面端用它登录，认出后才算切换完成");
        Ok(text)
    }

    /// 等登录：桌面端日志里出现目标账号就完成；出现别的账号就提示
    fn check_wait(&mut self) -> bool {
        let Some(ctx) = &self.switch else { return false };
        let points = self.tl.read();
        let after: Vec<&desktop::Signin> = points.iter().filter(|p| p.at >= ctx.since - 2000).collect();
        if after.iter().any(|p| p.acct == ctx.uuid) {
            let ctx = self.switch.take().unwrap();
            self.confirm(&ctx);
            return true;
        }
        let accs = store::accounts(&store::load());
        let last = after.last().map(|p| p.acct.clone());
        let (still, wrong) = match &last {
            Some(acct) if Some(acct) != ctx.prev.as_ref() => (None, Some(name_of(&accs, acct))),
            _ => (ctx.prev.as_ref().map(|p| name_of(&accs, p)), None),
        };
        let next = Phase::Waiting { target: ctx.id.clone(), still, wrong };
        let changed = next != self.phase;
        self.phase = next;
        changed
    }

    fn confirm(&mut self, ctx: &SwitchCtx) {
        let now = store::now_ms();
        let r = store::mutate(|d| {
            let accs = store::accounts(d);
            let cur = d["current"].as_str().unwrap_or_default().to_owned();
            if cur != ctx.id {
                let lim = accs.iter().find(|a| a.id == cur).and_then(limits_of);
                usage::record_exhausted(d, &cur, &ctx.id, lim.as_ref(), now);
                d["current"] = json!(ctx.id);
                if let Some(list) = d["switches"].as_array_mut() {
                    list.push(json!({"at": now, "account": ctx.id}));
                }
            }
            d["lastSignedAcct"] = json!(ctx.uuid);
            d["lastFollow"] = json!({"id": ctx.id, "at": now});
            Ok(())
        });
        self.phase = Phase::Idle;
        self.busy_until = Instant::now() + Duration::from_secs(15);
        match r {
            Ok(()) => {
                self.note(format!("桌面端已登录 {}，切换完成", ctx.id));
                self.attempts.remove(&ctx.id);
                self.start_quota_for(vec![ctx.id.clone()]);
            }
            Err(e) => self.note(e),
        }
    }

    /// 在桌面端里直接换了号（没走岛）：按日志把“当前”跟过去
    fn follow(&mut self) -> bool {
        let Some(signed) = desktop::changes(&self.tl.read()).last().cloned() else { return false };
        let d = store::load();
        if d["lastSignedAcct"].as_str() == Some(signed.acct.as_str()) {
            return false;
        }
        let r = store::mutate(|d| {
            d["lastSignedAcct"] = json!(signed.acct);
            let accs = store::accounts(d);
            let Some(a) = accs.iter().find(|a| a.group == Group::Claude && a.account_uuid.as_deref() == Some(signed.acct.as_str())) else { return Ok(None) };
            let cur = d["current"].as_str().unwrap_or_default().to_owned();
            if d["settings"]["autoFollow"].as_bool() == Some(false) || cur == a.id {
                return Ok(None);
            }
            let lim = accs.iter().find(|x| x.id == cur).and_then(limits_of);
            let now = store::now_ms();
            usage::record_exhausted(d, &cur, &a.id, lim.as_ref(), now);
            d["current"] = json!(a.id);
            let last = d["switches"].as_array().and_then(|l| l.last()).and_then(|s| s["at"].as_i64()).unwrap_or(0);
            if let Some(list) = d["switches"].as_array_mut() {
                list.push(json!({"at": now.min((last + 1).max(signed.at)), "account": a.id}));
            }
            d["lastFollow"] = json!({"id": a.id, "at": now});
            Ok(Some(a.id.clone()))
        });
        if let Ok(Some(id)) = r {
            self.note(format!("桌面端换成了 {id}，已跟着切过去"));
        }
        true
    }

    /// 桌面端退出时：给最后登录的账号补齐缺的会话列表（先备份、可撤销）
    fn auto_fill(&mut self) {
        let d = store::load();
        if d["settings"]["autoFill"].as_bool() == Some(false) {
            return;
        }
        let Some(signed) = desktop::changes(&self.tl.read()).last().map(|p| p.acct.clone()) else { return };
        let lists = desktop::session_lists();
        if desktop::missing(&lists, &signed).unwrap_or(0) == 0 {
            return;
        }
        let Ok(r) = desktop::sync_to(&signed) else { return };
        let accs = store::accounts(&d);
        let name = name_of(&accs, &signed);
        store::write_log("sync.log", json!({"at": chrono::Utc::now().to_rfc3339(), "op": "desktop-sessions", "auto": true, "from": "island", "account": name, "added": r.added, "replaced": r.replaced, "backup": r.backup}));
        let _ = store::mutate(|d| {
            d["lastAutoFill"] = json!({"at": store::now_ms(), "account": name, "added": r.added, "replaced": r.replaced});
            Ok(())
        });
        self.note(format!("桌面端退出时，给 {name} 补齐了 {} 条会话列表", r.added + r.replaced));
    }

    fn undo(&mut self) -> Result<String, String> {
        if self.phase != Phase::Idle {
            return Err("正在切换，稍后再撤销".into());
        }
        let was = desktop::app_running();
        self.busy_until = Instant::now() + Duration::from_secs(3600);
        if was && !desktop::quit_app() {
            self.busy_until = Instant::now();
            return Err("桌面端没能关闭，请从托盘退出后再试".into());
        }
        let r = desktop::undo_last();
        if was {
            desktop::open_app();
        }
        self.busy_until = Instant::now() + Duration::from_secs(15);
        let (removed, restored) = r?;
        Ok(format!("已撤销上次同步：去掉 {removed} 条，放回 {restored} 条"))
    }

    // ---------------- 额度 ----------------

    fn start_quota(&mut self, opened: bool) {
        if self.quota_busy || self.phase != Phase::Idle {
            return;
        }
        let d = store::load();
        let cur = d["current"].as_str().unwrap_or_default().to_owned();
        let now = store::now_ms();
        let due: Vec<String> = store::accounts(&d)
            .iter()
            .filter(|a| a.status == "active" && usage::quota_dir(a).is_some())
            .filter(|a| {
                let at = usage::quota_dir(a).and_then(|dir| usage::read_for(&dir)).map(|l| l.at).unwrap_or(0).max(*self.attempts.get(&a.id).unwrap_or(&0));
                let every = if a.id == cur { if opened { 2 * 60 * 1000 } else { CURRENT_EVERY } } else { OTHERS_EVERY };
                (!opened || a.id == cur) && now - at >= every
            })
            .map(|a| a.id.clone())
            .collect();
        if !due.is_empty() {
            self.start_quota_for(due);
        }
    }

    fn start_quota_for(&mut self, ids: Vec<String>) {
        if self.quota_busy {
            return;
        }
        let accs = store::accounts(&store::load());
        let jobs: Vec<(String, std::path::PathBuf, String)> = ids
            .iter()
            .filter_map(|id| accs.iter().find(|a| &a.id == id))
            .filter_map(|a| Some((a.id.clone(), usage::quota_dir(a)?, a.email.clone())))
            .collect();
        if jobs.is_empty() {
            return;
        }
        let now = store::now_ms();
        for j in &jobs {
            self.attempts.insert(j.0.clone(), now);
        }
        self.quota_busy = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let mut results = Vec::new();
            for (id, dir, email) in jobs {
                // 以官方 auth status 为准判断这个目录登录的是谁
                let st = usage::auth_status(&dir);
                let r = if !st.logged_in {
                    Err("还没登录取额度：点“无痕登录”".to_string())
                } else if let Some(e) = st.email.as_ref().filter(|e| !email.is_empty() && !store::same_email(&email, e)) {
                    Err(format!("取额度的登录成了 {e}，请重新无痕登录"))
                } else if usage::refresh(&dir).is_some() {
                    Ok(())
                } else {
                    Err("这次没取到额度，稍后自动再试".to_string())
                };
                results.push((id, r));
            }
            let _ = tx.send(Req::QuotaDone(results));
        });
    }

    fn login(&mut self, id: &str) -> Result<String, String> {
        if self.logins.contains(id) {
            return Err("这个账号正在登录".into());
        }
        let accs = store::accounts(&store::load());
        let a = accs.iter().find(|a| a.id == id).ok_or("没有这个账号")?.clone();
        if a.group != Group::Claude {
            return self.open_terminal(id);
        }
        let dir = match usage::quota_dir(&a) {
            Some(d) => d,
            None => {
                let d = usage::default_quota_dir(id);
                store::mutate(|v| {
                    store::account_mut(v, id)?["quotaDir"] = json!(d);
                    Ok(())
                })?;
                d
            }
        };
        self.logins.insert(id.into());
        let tx = self.tx.clone();
        let id = id.to_owned();
        let email = a.email.clone();
        std::thread::spawn(move || {
            let r = login::run(&id, &dir, &email).and_then(|_| {
                let st = usage::auth_status(&dir);
                let matches = st.email.as_ref().is_some_and(|e| email.is_empty() || store::same_email(&email, e));
                login::log(&id, "verify", json!({"loggedIn": st.logged_in, "matches": matches}));
                if !st.logged_in {
                    return Err("没有登录成功".into());
                }
                if !matches {
                    usage::logout(&dir);
                    return Err(format!("登录成了 {}，已退出这次登录", st.email.unwrap_or_default()));
                }
                if email.is_empty() {
                    let e = st.email.clone().unwrap_or_default();
                    let _ = store::mutate(|v| {
                        store::account_mut(v, &id)?["email"] = json!(e);
                        Ok(())
                    });
                }
                Ok(st.email.unwrap_or_default())
            });
            let _ = tx.send(Req::LoginDone(id, r));
        });
        Ok(format!("{} 的登录页会在无痕窗口打开，授权后自动完成", a.id))
    }

    fn open_terminal(&self, id: &str) -> Result<String, String> {
        let accs = store::accounts(&store::load());
        let a = accs.iter().find(|a| a.id == id).ok_or("没有这个账号")?;
        let (key, dir, line) = match a.group {
            Group::Codex => ("CODEX_HOME", codex::home_of(a), "codex login"),
            Group::Claude => ("CLAUDE_CONFIG_DIR", usage::quota_dir(a).unwrap_or_else(|| usage::default_quota_dir(id)), "claude"),
        };
        std::fs::create_dir_all(&dir).map_err(|e| format!("无法建目录：{e}"))?;
        let mut c = super::proc::command("cmd.exe");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0000_0010); // 新控制台窗口
            c.raw_arg(format!("/k {line}"));
        }
        c.env(key, &dir);
        c.spawn().map_err(|_| "打不开终端".to_string())?;
        Ok(format!("已在新终端里打开 {line}（{}）", a.id))
    }

    fn bind(&mut self, id: &str, uuid: &str) -> Result<String, String> {
        let lists = desktop::session_lists();
        let seen = self.tl.read().iter().any(|p| p.acct == uuid) || lists.iter().any(|l| l.acct == uuid);
        if !seen {
            return Err("桌面端里没见过这个账号 ID".into());
        }
        store::mutate(|d| {
            if store::accounts(d).iter().any(|a| a.account_uuid.as_deref() == Some(uuid)) {
                return Err("这个账号 ID 已经绑定到别的账号".into());
            }
            let a = store::account_mut(d, id)?;
            if a["group"].as_str() == Some("codex") {
                return Err("只有 Claude 账号需要绑定".into());
            }
            a["accountUuid"] = json!(uuid);
            Ok(())
        })?;
        Ok(format!("已把桌面端账号 {} 对上 {id}", &uuid[..8]))
    }

    fn add(&mut self, group: Group, name: String, email: String) -> Result<Option<String>, String> {
        let name = name.trim().to_owned();
        let email = email.trim().to_owned();
        if !store::valid_name(&name) {
            return Err("名字只能用英文、数字和短横线".into());
        }
        if !email.is_empty() && !super::proc::safe_email(&email) {
            return Err("邮箱格式不对".into());
        }
        store::mutate(|d| {
            let accs = store::accounts(d);
            if accs.iter().any(|a| a.id == name) {
                return Err("已经有同名账号".into());
            }
            let mut a = json!({"id": name, "name": name, "group": group.key(), "email": email, "mailUrl": "", "plan": "", "regDate": "", "note": "", "status": "active", "statusAt": "", "createdAt": store::now_ms()});
            match group {
                Group::Claude => {
                    a["kind"] = json!("cli");
                    a["configDir"] = json!(usage::default_quota_dir(&name));
                }
                Group::Codex => {
                    if accs.iter().any(|x| x.group == Group::Codex && x.kind == "default") {
                        a["kind"] = json!("cli");
                        a["configDir"] = json!(store::home().join(".codex-accounts").join(&name));
                    } else {
                        a["kind"] = json!("default");
                    }
                }
            }
            if let Some(list) = d["accounts"].as_array_mut() {
                list.push(a);
            }
            Ok(())
        })?;
        let follow = if group == Group::Claude { self.login(&name)? } else { self.open_terminal(&name)? };
        Ok(Some(format!("已添加 {name}。{follow}")))
    }
}

#[cfg(windows)]
fn clear_clipboard_if(text: &str) {
    use windows::Win32::Foundation::{HGLOBAL, HWND};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    unsafe {
        if OpenClipboard(HWND::default()).is_err() {
            return;
        }
        let same = (|| {
            let h = GetClipboardData(13).ok()?;
            let g = HGLOBAL(h.0 as _);
            let p = GlobalLock(g) as *const u16;
            if p.is_null() {
                return None;
            }
            let mut n = 0usize;
            while n < 65_536 && *p.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            let _ = GlobalUnlock(g);
            Some(s == text)
        })();
        if same == Some(true) {
            let _ = EmptyClipboard();
        }
        let _ = CloseClipboard();
    }
}
#[cfg(not(windows))]
fn clear_clipboard_if(_: &str) {}

#[cfg(test)]
mod tests {
    use super::*;
    /// 按需在本机核对：能读到账号、会话列表和登录记录，官方 /usage 走得通，密码解得开。
    /// 不改配置，不关桌面端，不打印邮箱或密码。
    #[test]
    #[ignore = "按需核对本机真实数据"]
    fn real_data_read_only_probe() {
        let d = store::load();
        let accs = store::accounts(&d);
        let lists = desktop::session_lists();
        let signins = desktop::Timeline::default().read();
        let last = desktop::changes(&signins).last().map(|p| name_of(&accs, &p.acct));
        println!("accounts={} lists={} signins={} last_signed={last:?} desktop_running={}", accs.len(), lists.len(), signins.len(), desktop::app_running());
        for a in &accs {
            let uuid = a.account_uuid.as_deref().unwrap_or_default();
            let lim = limits_of(a);
            let auth = usage::quota_dir(a).map(|dir| usage::auth_status(&dir));
            println!(
                "{} group={} sessions={:?} missing={:?} w5={:?} week={:?} logged_in={:?} email_matches={:?} password_saved={} password_opens={}",
                a.id,
                a.group.key(),
                desktop::visible_count(&lists, uuid),
                desktop::missing(&lists, uuid),
                lim.as_ref().and_then(|l| l.w5.map(|w| w.used)),
                lim.as_ref().and_then(|l| l.week.map(|w| w.used)),
                auth.as_ref().map(|s| s.logged_in),
                auth.as_ref().map(|s| s.email.as_ref().is_some_and(|e| store::same_email(&a.email, e))),
                secrets::has(&format!("acc:{}", a.id)),
                secrets::get(&format!("acc:{}", a.id)).is_some(),
            );
        }
        let cur = accs.iter().find(|a| Some(a.id.as_str()) == d["current"].as_str()).expect("应有当前账号");
        let fresh = usage::quota_dir(cur).and_then(|dir| usage::refresh(&dir));
        println!("official_usage_refresh={:?}", fresh.map(|l| (l.w5.map(|w| w.used), l.week.map(|w| w.used))));
    }
}
