//! 账号岛的界面：额度页、切换流程、账号管理、账号详情，以及设置窗口里的账号表单。
//! 只负责显示和发请求，读写和切换都在 hub::Service 的后台线程。
use crate::app::{App, Cmd};
use crate::extras::{setting_row, settings_card, settings_control_row, settings_label};
use crate::hub::{tasks::{Task, TaskState}, usage::{self, Limits, Win}, AccountView, Ev, Group, Phase, Req, View};
use crate::paint::{self, Card, H};
use crate::theme::{self, W};
use eframe::egui::{self, Color32};

const ORANGE: Color32 = Color32::from_rgb(255, 184, 107);
const TASK_RED: Color32 = Color32::from_rgb(255, 90, 90);
const TASK_YELLOW: Color32 = Color32::from_rgb(255, 197, 61);
const TASK_GREEN: Color32 = Color32::from_rgb(60, 203, 127);
const LEFT: f32 = 43.0;
const RIGHT: f32 = 376.0;
const MANAGE_ROWS: usize = 4;
const TASK_ROWS: usize = 6;
const QUICK_ROWS: usize = 4;
/// 刘海文字区的宽度和翻动节奏
const BAR_TEXT_W: f32 = 214.0;
const BAR_COUNT_W: f32 = 62.0;
const CLOCK_W: f32 = 44.0;
const FLIP_SECS: f64 = 3.0;
const FLIP_MOVE_SECS: f64 = 0.36;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Page {
    #[default]
    Quota,
    Manage,
    Detail,
    /// 额度用完时的快速换号
    Quick,
    Tasks,
}

fn task_color(s: TaskState) -> Color32 {
    match s {
        TaskState::Running => TASK_RED,
        TaskState::Waiting => TASK_YELLOW,
        TaskState::Done => TASK_GREEN,
    }
}

fn task_label(s: TaskState) -> &'static str {
    match s {
        TaskState::Running => "进行中",
        TaskState::Waiting => "等你确认",
        TaskState::Done => "已结束",
    }
}

/// 把长任务名分成几条，每条整条显示完再翻到下一条；优先在标点处断开。
fn split_title(text: &str, max_w: f32, width_of: impl Fn(&str) -> f32) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    let mut cut: Option<usize> = None;
    for ch in text.chars() {
        let mut next = line.clone();
        next.push(ch);
        if width_of(&next) > max_w && !line.is_empty() {
            match cut.filter(|c| *c > line.len() / 2 && *c < line.len()) {
                Some(c) => {
                    let rest = line.split_off(c);
                    out.push(std::mem::replace(&mut line, rest));
                }
                None => out.push(std::mem::take(&mut line)),
            }
            cut = None;
        }
        line.push(ch);
        if matches!(ch, '，' | '、' | '。' | '；' | '：' | ',' | '.' | ' ') {
            cut = Some(line.len());
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// 一条翻动的文字
struct BarLine {
    num: String,
    text: String,
    color: Color32,
    num_color: Color32,
    flash: bool,
}

#[derive(Default)]
pub(crate) struct Form {
    pub codex: bool,
    pub name: String,
    pub email: String,
    pub edit: String,
    pub edit_email: String,
    pub edit_mail: String,
    pub edit_plan: String,
    pub edit_reg: String,
    pub edit_note: String,
    pub password: String,
}

#[derive(Default)]
pub(crate) struct State {
    /// 多账号模式：默认关闭，关闭时一切走原来的 AI 额度页（见 `mode_on`）
    pub active: bool,
    pub view: Option<View>,
    pub out: Vec<Req>,
    pub page: Page,
    pub codex_tab: bool,
    pub claude_at: String,
    pub codex_at: String,
    /// 额度页的小卡片：名称、邮箱、密码
    pub info: bool,
    pub detail: String,
    pub detail_from_quota: bool,
    pub secret: Option<(String, String, f64)>,
    pub confirm: Option<(String, f64)>,
    pub manage_start: usize,
    pub heat: Option<(String, Vec<u64>)>,
    pub was_open: bool,
    pub form: Form,
    /// 当前账号的对话任务，顺序就是编号，状态变了也不换位置
    pub tasks: Vec<Task>,
    pub task_start: usize,
    /// 额度用完时震动到这个时间
    pub shake_until: f64,
    pub flip_sig: String,
    pub flip_at: f64,
}

impl State {
    pub fn height(&self) -> f32 {
        match self.page {
            Page::Quota if self.view.as_ref().is_some_and(|v| v.phase != Phase::Idle) => 252.0,
            Page::Quota => 182.0,
            Page::Manage | Page::Detail | Page::Quick | Page::Tasks => 252.0,
        }
    }
    fn unfinished(&self) -> usize {
        self.tasks.iter().filter(|t| t.state != TaskState::Done).count()
    }
    fn group(&self) -> Group {
        if self.codex_tab { Group::Codex } else { Group::Claude }
    }
    fn at(&self) -> &str {
        if self.codex_tab { &self.codex_at } else { &self.claude_at }
    }
    fn set_at(&mut self, id: String) {
        if self.codex_tab { self.codex_at = id } else { self.claude_at = id }
    }
    fn hide_secret(&mut self) {
        self.secret = None;
    }
}

fn mode_file() -> std::path::PathBuf {
    crate::model::config_dir().join("account-island.json")
}

fn saved_mode() -> Option<bool> {
    let raw = std::fs::read(mode_file()).ok()?;
    serde_json::from_slice::<serde_json::Value>(&raw).ok()?["enabled"].as_bool()
}

fn save_mode(on: bool) {
    let dir = crate::model::config_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = crate::model::atomic_write(&mode_file(), serde_json::json!({ "enabled": on }).to_string().as_bytes());
}

/// 用户选过就听用户的；没选过时，只有本机已有多账号数据才打开，其他人默认保持原样。
fn mode_on(saved: Option<bool>, has_data: bool) -> bool {
    saved.unwrap_or(has_data)
}

fn group_colors(g: Group) -> [Color32; 2] {
    match g {
        Group::Claude => [Color32::from_rgb(240, 138, 75), Color32::from_rgb(255, 194, 122)],
        Group::Codex => [Color32::from_rgb(139, 124, 255), Color32::from_rgb(185, 174, 255)],
    }
}

fn status_of(v: &View, a: &AccountView) -> (&'static str, Color32) {
    if a.a.status == "banned" {
        return ("已封号", Color32::from_rgb(255, 122, 122));
    }
    if a.a.status == "disabled" {
        return ("已停用", theme::DIM);
    }
    if matches!(&v.phase, Phase::Waiting { target, .. } if *target == a.a.id) {
        return ("等你登录", ORANGE);
    }
    if a.a.group == Group::Codex {
        return if v.is_current(&a.a) { ("使用中", theme::GREEN) } else { ("已登录", theme::GRAY) };
    }
    if a.signed_in && v.app_running {
        ("使用中", theme::GREEN)
    } else if a.signed_in {
        ("已登录", theme::GRAY)
    } else {
        ("未登录", theme::DIM)
    }
}

fn reset_text(w: Option<Win>) -> String {
    let Some(w) = w else { return "—".into() };
    if w.used <= 0.0 {
        return "未开始计时".into();
    }
    let Some(at) = w.resets_at.and_then(chrono::DateTime::from_timestamp_millis) else { return "重置时间未知".into() };
    let at = at.with_timezone(&chrono::Local);
    let now = chrono::Local::now();
    if at <= now {
        return "等待刷新".into();
    }
    if at.date_naive() == now.date_naive() {
        at.format("今天 %H:%M 重置").to_string()
    } else {
        at.format("%-m月%-d日 %H:%M 重置").to_string()
    }
}

fn clock_of(ms: Option<i64>) -> String {
    ms.and_then(chrono::DateTime::from_timestamp_millis).map(|d| d.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string()).unwrap_or_else(|| "时间未知".into())
}

fn left_of(w: Win) -> f32 {
    (100.0 - w.used).clamp(0.0, 100.0)
}

fn left_text(w: Option<Win>) -> String {
    w.map(|w| format!("{:.0}%", left_of(w))).unwrap_or_else(|| "—".into())
}

/// 额度圆环，画法和原来 AI 额度页的 `quota_ui::ring` 一样：画的是剩余额度；
/// 时间窗口短的在外圈（5 小时在外、本周在内）；底色是这一圈的颜色压暗；用完时只剩暗圈。
fn quota_ring(c: &Card, lim: Option<&Limits>, x: f32, y: f32, size: f32, colors: [Color32; 2]) {
    let windows: Vec<Win> = lim.map(|l| [l.w5, l.week].into_iter().flatten().collect()).unwrap_or_default();
    let dual = windows.len() > 1;
    let stroke = if dual { if size < 35.0 { 2.2 } else { 6.5 } } else { (size * 0.102).clamp(3.0, 6.5) };
    let radius = size / 2.0 - stroke / 2.0 - 1.0;
    let gap = if size < 35.0 { 1.5 } else { 3.0 };
    let start = -std::f32::consts::FRAC_PI_2;
    let full = start + std::f32::consts::TAU;
    if windows.is_empty() {
        c.arc(x, y, radius, start, full, stroke, theme::TRACK);
    }
    for (i, w) in windows.iter().copied().take(2).enumerate() {
        let r = radius - i as f32 * (stroke + gap);
        let col = colors[i];
        let dark = paint::mix(col, Color32::BLACK, 0.8);
        let left = left_of(w);
        // 整圈也用圆弧画：系统的圆描边向外扩，100% 和 0% 时圆环会比平时大一圈
        if left >= 100.0 {
            c.arc(x, y, r, start, full, stroke, col);
        } else if left <= 0.0 {
            c.arc(x, y, r, start, full, stroke, dark);
        } else {
            let end = start + std::f32::consts::TAU * left / 100.0;
            c.arc(x, y, r, end, full, stroke, dark);
            c.arc(x, y, r, start, end, stroke, col);
            for angle in [start, end] {
                c.circle(x + r * angle.cos(), y + r * angle.sin(), stroke / 2.0, col);
            }
        }
    }
}

/// 任务状态的表情：进行中愤怒地使劲干活，待确认疑惑，结束了高兴。不只靠颜色区分。
fn draw_task_eyes(c: &Card, cx: f32, state: TaskState, t: f32, moving: bool) {
    let color = task_color(state);
    match state {
        TaskState::Running => {
            let jitter = if moving { (t * 38.0).sin() * 0.55 } else { 0.0 };
            for side in [-1.0f32, 1.0] {
                let x = cx + side * 6.0 + jitter;
                c.glow(x, 15.0, 9.0, paint::alpha(color, 0.28));
                let (outer, inner) = (x + side * 3.4, x - side * 3.4);
                let pts = [(outer, 9.5), (inner, 13.2), (inner, 17.6), (x - side * 1.6, 19.4), (x + side * 1.6, 19.4), (outer, 17.6)];
                if side < 0.0 {
                    let mut flipped = pts;
                    flipped.reverse();
                    c.poly(&flipped, color);
                } else {
                    c.poly(&pts, color);
                }
            }
        }
        TaskState::Waiting => {
            let rock = if moving { (t * 2.6).sin() * 0.9 } else { 0.0 };
            let (l, r) = (cx - 6.5 + rock, cx + 3.2 + rock);
            c.glow(l, 15.0, 8.0, paint::alpha(color, 0.25));
            c.seg((l, 11.5), (l, 18.5), 5.0, color);
            c.circle(l, 11.5, 2.5, color);
            c.circle(l, 18.5, 2.5, color);
            c.seg((r - 1.3, 15.0), (r + 1.3, 15.0), 4.0, color);
            c.circle(r - 1.3, 15.0, 2.0, color);
            c.circle(r + 1.3, 15.0, 2.0, color);
            c.centered_text(cx + 12.0, 14.5, "?", 12.0, W::Bold, color, H::Center);
        }
        TaskState::Done => {
            let hop = if moving { -((t * 4.2).sin().abs()) * 1.8 } else { 0.0 };
            for side in [-1.0f32, 1.0] {
                let x = cx + side * 6.0;
                c.glow(x, 14.0, 8.0, paint::alpha(color, 0.25));
                let pts: Vec<(f32, f32)> = (0..=12).map(|n| {
                    let q = n as f32 / 6.0 - 1.0;
                    (x + q * 4.0, 17.5 + hop - (1.0 - q * q) * 5.0)
                }).collect();
                c.path(&pts, 3.2, color);
                c.circle(pts[0].0, pts[0].1, 1.6, color);
                c.circle(pts[12].0, pts[12].1, 1.6, color);
            }
        }
    }
}

fn button(c: &Card, x: f32, y: f32, w: f32, h: f32, label: &str, id: &str, primary: bool) -> bool {
    let (base, hover, text) = if primary { (Color32::from_rgb(242, 244, 248), Color32::WHITE, Color32::BLACK) } else { (theme::BTN, theme::BTN_HOVER, theme::WHITE) };
    let (r, b) = paint::capsule(c, x, y, w, h, id, base, hover);
    b.centered_text(x + w / 2.0, y + h / 2.0, label, 10.5, if primary { W::Bold } else { W::Regular }, text, H::Center);
    r.clicked()
}

fn chip(c: &Card, x: f32, cy: f32, label: &str, color: Color32, bg: Color32) -> f32 {
    let w = c.text_w(label, 9.5, W::Semibold) + 12.0;
    c.fill(x, cy - 8.0, w, 16.0, 8.0, bg);
    c.centered_text(x + w / 2.0, cy, label, 9.5, W::Semibold, color, H::Center);
    w
}

impl App {
    /// 启动时要不要进多账号模式
    pub fn hub_wanted(&self) -> bool {
        mode_on(saved_mode(), crate::hub::store::has_data())
    }
    pub fn hub_active(&self) -> bool {
        self.hub.active
    }
    pub fn hub_enable(&mut self) {
        self.hub.active = true;
    }
    fn hub_set_mode(&mut self, on: bool) {
        save_mode(on);
        self.hub = State { active: on, ..Default::default() };
        if !on {
            self.quota_initialize();
            self.pending.push(Cmd::QuotaConnected);
        }
    }
    /// 原额度设置页顶部的开关；打开后返回 true，这一帧不再画原来的设置
    pub(crate) fn hub_mode_offer(&mut self, ui: &mut egui::Ui) -> bool {
        settings_label(ui, "多账号");
        let mut on = false;
        settings_card(ui, |ui| {
            setting_row(ui, "多账号模式", "管理多个 Claude、Codex 账号并切换 Claude 桌面端。额度只走官方 /usage，不读取本机登录文件；开启后 Kimi、Grok Bot、Gemini 额度不显示", &mut on);
        });
        if on {
            self.hub_set_mode(true);
        }
        on
    }
    pub fn hub_drain(&mut self) -> Vec<Req> {
        std::mem::take(&mut self.hub.out)
    }
    pub fn hub_event(&mut self, ev: Ev) {
        match ev {
            Ev::View(v) => {
                if let Some(first) = v.accounts.iter().find(|a| a.a.group == Group::Claude) {
                    if !v.accounts.iter().any(|a| a.a.id == self.hub.claude_at) {
                        self.hub.claude_at = if v.current.is_empty() { first.a.id.clone() } else { v.current.clone() };
                    }
                }
                if let Some(first) = v.accounts.iter().find(|a| a.a.group == Group::Codex) {
                    if !v.accounts.iter().any(|a| a.a.id == self.hub.codex_at) {
                        self.hub.codex_at = if v.current_codex.is_empty() { first.a.id.clone() } else { v.current_codex.clone() };
                    }
                }
                self.hub.view = Some(*v);
            }
            Ev::Note(text) => self.say(text),
            Ev::Secret(id, value) => self.hub.secret = Some((id, value, self.t + 10.0)),
            Ev::Copy(_) => {}
            Ev::Heat(id, days) => self.hub.heat = Some((id, days)),
            Ev::Tasks(tasks) => self.hub.tasks = tasks,
            Ev::Exhausted { id, window, resets_at, repeat } => {
                let when = resets_at.map(|t| format!("，{} 重置", clock_of(Some(t)))).unwrap_or_default();
                let is_current = self.hub.view.as_ref().is_some_and(|v| v.current == id);
                let alert = self.hub.view.as_ref().map(|v| v.alert).unwrap_or_default();
                let (title, body) = if repeat {
                    ("额度还没恢复", format!("{id} 的{window}额度还没恢复{when}，可以换号。"))
                } else {
                    ("额度用完了", format!("{id} 的{window}额度用完了{when}，可以换号。"))
                };
                // 红字一直闪；震动只在第一次（设置里可以让再提醒时也震）
                if is_current && (!repeat || alert.remind_shake) {
                    self.hub.shake_until = self.t + alert.shake_secs as f64;
                }
                if repeat || !is_current {
                    self.say(body.clone());
                }
                self.pending.push(Cmd::Notify(title.into(), body));
            }
        }
    }
    pub(crate) fn hub_tick(&mut self) {
        if !self.hub.active {
            return;
        }
        if self.open {
            self.hub.shake_until = 0.0;
        }
        if self.hub.secret.as_ref().is_some_and(|s| self.t >= s.2) {
            self.hub.hide_secret();
        }
        if self.hub.confirm.as_ref().is_some_and(|s| self.t >= s.1) {
            self.hub.confirm = None;
        }
        let open = self.open && self.page == 6;
        if open && !self.hub.was_open {
            self.hub.out.push(Req::Opened);
        }
        if !open && self.hub.was_open {
            self.hub.hide_secret();
            self.hub.info = false;
            self.hub.confirm = None;
            // 换号页和任务页是临时的，下次展开回到额度页；看过的已结束任务不再占位
            if matches!(self.hub.page, Page::Quick | Page::Tasks) {
                self.hub.page = Page::Quota;
            }
            if self.hub.tasks.iter().any(|t| t.state == TaskState::Done) {
                self.hub.tasks.retain(|t| t.state != TaskState::Done);
                self.hub.out.push(Req::DismissDone);
            }
        }
        self.hub.was_open = open;
    }

    /// 需要点第二次才执行的操作：第一次点变成“确认”，4 秒内再点才算
    fn hub_confirmed(&mut self, key: &str) -> bool {
        if self.hub.confirm.as_ref().is_some_and(|c| c.0 == key) {
            self.hub.confirm = None;
            return true;
        }
        self.hub.confirm = Some((key.to_owned(), self.t + 4.0));
        false
    }
    fn hub_confirming(&self, key: &str) -> bool {
        self.hub.confirm.as_ref().is_some_and(|c| c.0 == key)
    }

    pub(crate) fn hub_step(&mut self, dir: isize) {
        let Some(v) = &self.hub.view else { return };
        match self.hub.page {
            Page::Quick => {}
            Page::Tasks => {
                let next = self.hub.task_start as isize + dir * TASK_ROWS as isize;
                if next >= 0 && (next as usize) < self.hub.tasks.len() {
                    self.hub.task_start = next as usize;
                }
            }
            Page::Manage => {
                let n = v.accounts.iter().filter(|a| a.a.group == self.hub.group()).count();
                let next = self.hub.manage_start as isize + dir * MANAGE_ROWS as isize;
                if next >= 0 && (next as usize) < n {
                    self.hub.manage_start = next as usize;
                }
            }
            Page::Quota | Page::Detail => {
                let ids: Vec<String> = v.accounts.iter().filter(|a| a.a.group == self.hub.group()).map(|a| a.a.id.clone()).collect();
                if ids.is_empty() {
                    return;
                }
                let current = if self.hub.page == Page::Detail { self.hub.detail.clone() } else { self.hub.at().to_owned() };
                let pos = ids.iter().position(|i| *i == current).unwrap_or(0) as isize;
                let id = ids[(pos + dir).rem_euclid(ids.len() as isize) as usize].clone();
                self.hub.info = false;
                self.hub.hide_secret();
                self.hub.confirm = None;
                if self.hub.page == Page::Detail {
                    self.hub.detail = id.clone();
                    self.hub.out.push(Req::Heat(id.clone()));
                }
                self.hub.set_at(id);
            }
        }
    }

    fn hub_open_detail(&mut self, id: String, from_quota: bool) {
        self.hub.page = Page::Detail;
        self.hub.detail = id.clone();
        self.hub.detail_from_quota = from_quota;
        self.hub.info = false;
        self.hub.hide_secret();
        self.hub.heat = None;
        self.hub.out.push(Req::Heat(id));
    }

    // ---------------- 收起时：当前账号、表情眼睛、任务或提醒 ----------------

    /// 待机只显示正在使用的账号
    pub(crate) fn hub_bar_indices(&self) -> Vec<usize> {
        if !self.hub_bar_visible() {
            return vec![];
        }
        let Some(v) = &self.hub.view else { return vec![] };
        v.accounts.iter().position(|a| a.a.id == v.current).into_iter().collect()
    }

    fn hub_bar_visible(&self) -> bool {
        self.hub.active && self.settings.visible_pages[6] && !self.settings.minimal && self.t >= self.notification_bell_until
    }

    /// 当前账号用完了哪些额度
    fn hub_out(&self) -> Option<(bool, bool)> {
        let v = self.hub.view.as_ref()?;
        let lim = v.get(&v.current)?.lim.as_ref()?;
        let now = crate::hub::store::now_ms();
        let full = |w: Option<Win>| w.is_some_and(|w| w.used >= 99.5 && w.resets_at.map_or(true, |t| t > now));
        let (five, week) = (full(lim.w5), full(lim.week));
        (five || week).then_some((five, week))
    }

    /// 额度用完优先：这时刘海只显示提醒，不翻任务
    pub(crate) fn hub_alerting(&self) -> bool {
        self.hub_bar_visible() && self.hub_out().is_some()
    }

    /// 文字区要比时钟多占多宽；None 表示显示时钟
    pub(crate) fn hub_bar_extra(&self) -> Option<f32> {
        if !self.hub_bar_visible() || self.hub_bar_indices().is_empty() {
            return None;
        }
        if self.hub_out().is_some() || self.hub.tasks.len() == 1 {
            return Some(BAR_TEXT_W - CLOCK_W);
        }
        (self.hub.tasks.len() > 1).then_some(BAR_TEXT_W + BAR_COUNT_W - CLOCK_W)
    }

    /// 震动时岛的水平偏移。系统关了动画就不震。
    pub(crate) fn hub_shake(&self) -> f32 {
        if self.t >= self.hub.shake_until || !crate::bar_layout::animations_enabled() {
            return 0.0;
        }
        (self.t as f32 * 38.0).sin() * 2.5
    }

    /// 现在该显示哪个任务的表情：跟着翻到的那条走；用完额度时回到平常的眼睛
    fn hub_eye_state(&self) -> Option<TaskState> {
        if !self.hub.active || self.settings.minimal || self.hub_out().is_some() {
            return None;
        }
        let n = self.hub.tasks.len();
        match n {
            0 => None,
            1 => Some(self.hub.tasks[0].state),
            _ => {
                let at = ((self.t - self.hub.flip_at).max(0.0) / FLIP_SECS) as usize % n;
                Some(self.hub.tasks[at].state)
            }
        }
    }

    pub(crate) fn hub_draw_eyes(&self, c: &Card, cx: f32) -> bool {
        let Some(state) = self.hub_eye_state() else { return false };
        draw_task_eyes(c, cx, state, self.t as f32, crate::bar_layout::animations_enabled());
        c.ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        true
    }

    fn hub_bar_lines(&self, c: &Card, size: f32) -> Vec<BarLine> {
        let line = |num: String, text: String, color: Color32, num_color: Color32, flash: bool| BarLine { num, text, color, num_color, flash };
        if let Some((five, week)) = self.hub_out() {
            let text = match (five, week) {
                (true, true) => "额度都用完了 · 点此换号",
                (true, false) => "5 小时额度已用完 · 点此换号",
                _ => "本周额度已用完 · 点此换号",
            };
            return vec![line(String::new(), text.into(), TASK_RED, TASK_RED, true)];
        }
        let tasks = &self.hub.tasks;
        if tasks.len() > 1 {
            // 每个任务只占一条，太长就省略；全名在展开后的任务列表里
            return tasks.iter().enumerate().map(|(i, t)| {
                let num = format!("{}/{}", i + 1, tasks.len());
                let room = BAR_TEXT_W - 30.0;
                let color = if t.state == TaskState::Waiting { TASK_YELLOW } else { theme::WHITE };
                line(num, c.fit(&t.title, size, W::Semibold, room), color, task_color(t.state), false)
            }).collect();
        }
        let Some(t) = tasks.first() else { return vec![] };
        let prefix = match t.state {
            TaskState::Running => "",
            TaskState::Waiting => "等你确认：",
            TaskState::Done => "已结束：",
        };
        let color = if t.state == TaskState::Waiting { TASK_YELLOW } else { theme::WHITE };
        split_title(&format!("{prefix}{}", t.title), BAR_TEXT_W - 4.0, |s| c.text_w(s, size, W::Semibold)).into_iter().map(|p| line(String::new(), p, color, color, false)).collect()
    }

    fn hub_draw_bar_line(&self, c: &Card, left: f32, line: &BarLine, size: f32) {
        let flash = if line.flash && crate::bar_layout::animations_enabled() { 0.61 + 0.39 * ((self.t as f32) * std::f32::consts::TAU).cos() } else { 1.0 };
        let c = c.faded(flash);
        let mut x = left;
        if !line.num.is_empty() {
            let w = c.text_w(&line.num, 9.5, W::Bold);
            c.centered_text(x, 15.0, &line.num, 9.5, W::Bold, line.num_color, H::Left);
            x += w + 7.0;
        }
        c.centered_text(x, 15.0, &line.text, size, W::Semibold, line.color, H::Left);
    }

    fn hub_draw_bar_text(&mut self, c: &Card, center: f32, v: &View) {
        let size = 11.5;
        let lines = self.hub_bar_lines(c, size);
        if lines.is_empty() {
            return;
        }
        let counts = self.hub_out().is_none() && self.hub.tasks.len() > 1;
        let slot = BAR_TEXT_W + if counts { BAR_COUNT_W } else { 0.0 };
        let left = center - slot / 2.0;
        // 内容变了从第一条重新开始。状态变化不算，这样编号不会跳
        let sig = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
        if sig != self.hub.flip_sig {
            self.hub.flip_sig = sig;
            self.hub.flip_at = self.t;
        }
        let elapsed = (self.t - self.hub.flip_at).max(0.0);
        let at = (elapsed / FLIP_SECS) as usize;
        let n = lines.len();
        let moved = if n > 1 && at > 0 && crate::bar_layout::animations_enabled() { ((elapsed % FLIP_SECS) / FLIP_MOVE_SECS).min(1.0) as f32 } else { 1.0 };
        let moved = crate::anim::ease_in_out(moved);
        let text_area = c.clipped(left - 2.0, 3.0, BAR_TEXT_W + 2.0, 24.0);
        if moved < 1.0 {
            self.hub_draw_bar_line(&text_area.shifted(0.0, -moved * 18.0).faded(1.0 - moved), left, &lines[(at + n - 1) % n], size);
        }
        self.hub_draw_bar_line(&text_area.shifted(0.0, (1.0 - moved) * 18.0).faded(moved), left, &lines[at % n], size);
        if counts {
            let mut x = left + slot - 6.0;
            for state in [TaskState::Done, TaskState::Waiting, TaskState::Running] {
                let count = self.hub.tasks.iter().filter(|t| t.state == state).count();
                if count == 0 {
                    continue;
                }
                let color = task_color(state);
                let w = c.text_w(&count.to_string(), 9.5, W::Bold);
                c.centered_text(x, 15.0, &count.to_string(), 9.5, W::Bold, color, H::Right);
                c.circle(x - w - 5.0, 15.0, 2.8, color);
                x -= w + 15.0;
            }
        }
        let tip = if let Some((five, week)) = self.hub_out() {
            let cur = v.get(&v.current);
            let reset = cur.and_then(|a| a.lim.as_ref()).and_then(|l| if five { l.w5 } else if week { l.week } else { None }).and_then(|w| w.resets_at);
            format!("额度用完了，{} 重置。点这里换号", clock_of(reset))
        } else {
            self.hub.tasks.iter().enumerate().map(|(i, t)| format!("{}. {} · {}", i + 1, task_label(t.state), t.title)).collect::<Vec<_>>().join("\n")
        };
        if crate::quota_ui::bar_tooltip(c.hit(left, 1.0, slot, 28.0, "hub-bar-text"), tip).clicked() {
            self.hub.codex_tab = false;
            self.hub.confirm = None;
            self.hub.task_start = 0;
            self.hub.page = if self.hub_out().is_some() { Page::Quick } else if self.hub.tasks.len() > 1 { Page::Tasks } else { Page::Quota };
            self.show_page(6);
        }
        c.ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }

    pub(crate) fn hub_draw_bar(&mut self, c: &Card, cx: f32, layout: &crate::bar_layout::Layout) {
        let indices = self.hub_bar_indices();
        let Some(v) = self.hub.view.clone() else { return };
        let mut chosen = None;
        if let Some(a) = indices.first().map(|i| &v.accounts[*i]) {
            let alpha = layout.pet_clearance(crate::bar_layout::QUOTA);
            let faded = c.faded(alpha);
            let faded = if alpha < 0.8 { faded.ghost() } else { faded };
            let c = &faded;
            if let Some(x) = layout.x[crate::bar_layout::QUOTA].map(|x| cx + x) {
                let colors = group_colors(a.a.group);
                let lim = a.lim.as_ref();
                quota_ring(c, lim, x, 15.0, 27.0, colors);
                let letter: String = a.a.id.chars().take(1).collect::<String>().to_uppercase();
                c.centered_text(x, 15.0, &letter, 8.0, W::Bold, theme::WHITE, H::Center);
                let (status, _) = status_of(&v, a);
                let tip = format!("{} · {status}\n5 小时剩余 {} · 本周剩余 {}", a.a.id, left_text(lim.and_then(|l| l.w5)), left_text(lim.and_then(|l| l.week)));
                if crate::quota_ui::bar_tooltip(c.hit(x - 14.0, 1.0, 28.0, 28.0, ("hub-pin", &a.a.id)), tip).clicked() {
                    chosen = Some((a.a.id.clone(), a.a.group));
                }
            }
        }
        if self.hub_bar_extra().is_some() {
            if let Some(x) = layout.x[crate::bar_layout::CLOCK] {
                self.hub_draw_bar_text(c, cx + x, &v);
            }
        }
        if let Some((id, group)) = chosen {
            self.hub.codex_tab = group == Group::Codex;
            self.hub.set_at(id);
            self.hub.page = Page::Quota;
            self.show_page(6);
        }
    }

    // ---------------- 展开的页面 ----------------

    pub(crate) fn page_hub(&mut self, c: &Card) {
        for (i, (label, codex)) in [("Claude", false), ("Codex", true)].into_iter().enumerate() {
            let x = 50.0 + i as f32 * 46.0;
            let on = self.hub.codex_tab == codex;
            let (r, b) = paint::capsule(c, x, 9.0, 44.0, 20.0, &format!("hub-tab-{i}"), if on { Color32::from_rgb(242, 244, 248) } else { theme::BTN }, if on { Color32::WHITE } else { theme::BTN_HOVER });
            b.centered_text(x + 22.0, 19.0, label, 10.0, W::Semibold, if on { Color32::BLACK } else { theme::GRAY }, H::Center);
            if r.clicked() && !on {
                self.hub.codex_tab = codex;
                self.hub.info = false;
                self.hub.hide_secret();
                self.hub.manage_start = 0;
                self.hub.confirm = None;
                if self.hub.page == Page::Detail {
                    self.hub.page = Page::Quota;
                }
            }
        }
        let Some(v) = self.hub.view.clone() else {
            c.text(LEFT, 70.0, "正在读取账号…", 12.0, W::Regular, theme::GRAY, H::Left);
            return;
        };
        if button(c, 270.0, 9.0, 50.0, 20.0, if v.quota_busy { "刷新中" } else { "刷新额度" }, "hub-refresh", false) && !v.quota_busy {
            self.hub.out.push(Req::Refresh(None));
        }
        if button(c, 322.0, 9.0, 52.0, 20.0, if self.hub.page == Page::Quota { "管理账号" } else { "看额度" }, "hub-manage", false) {
            self.hub.page = if self.hub.page == Page::Quota { Page::Manage } else { Page::Quota };
            self.hub.info = false;
            self.hub.hide_secret();
            self.hub.confirm = None;
        }
        match self.hub.page {
            Page::Quota => self.hub_quota(c, &v),
            Page::Manage => self.hub_manage(c, &v),
            Page::Detail => self.hub_detail(c, &v),
            Page::Quick => self.hub_quick(c, &v),
            Page::Tasks => self.hub_tasks(c, &v),
        }
    }

    /// 快速换号：列出其他 Claude 账号的额度，不排序、不推荐，由用户自己点。不会自动切换。
    fn hub_quick(&mut self, c: &Card, v: &View) {
        let cur = v.get(&v.current);
        let (title, reset) = match (cur, self.hub_out()) {
            (Some(a), Some((five, week))) => {
                let what = match (five, week) {
                    (true, true) => "额度都用完了",
                    (true, false) => " 5 小时额度用完了",
                    _ => "本周额度用完了",
                };
                let lim = a.lim.as_ref();
                let w = lim.and_then(|l| if five { l.w5 } else { l.week });
                (format!("{} 的{what}", a.a.id), format!("也可以等 {} 重置", clock_of(w.and_then(|w| w.resets_at))))
            }
            _ => ("换一个账号".to_owned(), String::new()),
        };
        c.text(LEFT, 50.0, &c.fit(&title, 12.0, W::Bold, 200.0), 12.0, W::Bold, TASK_RED, H::Left);
        c.text(RIGHT, 50.0, &reset, 9.5, W::Regular, theme::GRAY, H::Right);
        let others: Vec<&AccountView> = v.accounts.iter().filter(|a| a.a.group == Group::Claude && a.a.id != v.current).collect();
        if others.is_empty() {
            c.text(LEFT, 84.0, "还没有别的 Claude 账号。到设置里添加。", 11.0, W::Regular, theme::GRAY, H::Left);
        }
        let unfinished = self.hub.unfinished();
        let now = crate::hub::store::now_ms();
        for (row, a) in others.iter().take(QUICK_ROWS).enumerate() {
            let y = 60.0 + row as f32 * 36.0;
            let id = &a.a.id;
            let used_up = a.lim.as_ref().and_then(|l| usage::exhausted_window(l, now));
            let usable = v.can_switch(a).is_ok() && used_up.is_none();
            let dim = if usable { 1.0 } else { 0.6 };
            c.fill(LEFT - 2.0, y, RIGHT - LEFT + 4.0, 32.0, 10.0, theme::CARD);
            let t = c.faded(dim);
            t.text(LEFT + 8.0, y + 14.0, &c.fit(id, 11.0, W::Bold, 110.0), 11.0, W::Bold, theme::WHITE, H::Left);
            let lim = a.lim.as_ref();
            let quota = format!("5 小时剩余 {} · 本周剩余 {}", left_text(lim.and_then(|l| l.w5)), left_text(lim.and_then(|l| l.week)));
            t.text(LEFT + 8.0, y + 27.0, &c.fit(&quota, 9.5, W::Regular, 170.0), 9.5, W::Regular, theme::GRAY, H::Left);
            if usable {
                let key = format!("quick:{id}");
                let confirming = self.hub_confirming(&key);
                let label = if confirming { "仍然切换，中断任务" } else { "切换" };
                let w = if confirming { 104.0 } else { 44.0 };
                if button(c, RIGHT - w - 6.0, y + 6.0, w, 20.0, label, &format!("hub-quick-{id}"), true) && (unfinished == 0 || self.hub_confirmed(&key)) {
                    self.hub.confirm = None;
                    self.hub.page = Page::Quota;
                    self.hub.codex_tab = false;
                    self.hub.set_at(id.clone());
                    self.hub.out.push(Req::Switch(id.clone()));
                    return;
                }
            } else {
                let note = match (used_up, v.can_switch(a)) {
                    (Some((_, reset)), Ok(())) => format!("也用完了 · {} 重置", clock_of(reset)),
                    (_, Err(why)) => why.to_owned(),
                    _ => String::new(),
                };
                c.text(RIGHT - 6.0, y + 20.0, &c.fit(&note, 9.5, W::Regular, 124.0), 9.5, W::Regular, ORANGE, H::Right);
            }
        }
        let (foot, color) = if unfinished > 0 {
            (format!("还有 {unfinished} 个任务没结束，切换会强制关闭桌面端并中断它。"), ORANGE)
        } else {
            ("点切换后：关闭桌面端、同步会话列表、重新打开；你登录后认出账号才算切换成功。".to_owned(), theme::GRAY)
        };
        for (i, line) in c.wrap(&foot, 9.5, W::Regular, RIGHT - LEFT, 2).iter().enumerate() {
            c.text(LEFT, 212.0 + i as f32 * 13.0, line, 9.5, W::Regular, color, H::Left);
        }
    }

    /// 任务列表：编号和刘海上翻到的编号一一对应
    fn hub_tasks(&mut self, c: &Card, v: &View) {
        let name = v.get(&v.current).map(|a| a.a.id.as_str()).unwrap_or_default();
        c.text(LEFT, 50.0, &c.fit(&format!("{name} 的任务"), 12.0, W::Bold, 150.0), 12.0, W::Bold, theme::WHITE, H::Left);
        let mut x = RIGHT;
        for state in [TaskState::Done, TaskState::Waiting, TaskState::Running] {
            let count = self.hub.tasks.iter().filter(|t| t.state == state).count();
            if count == 0 {
                continue;
            }
            let text = format!("{} {count}", task_label(state));
            let w = c.text(x, 49.0, &text, 9.5, W::Semibold, task_color(state), H::Right);
            c.circle(x - w - 6.0, 45.5, 2.8, task_color(state));
            x -= w + 18.0;
        }
        if self.hub.tasks.is_empty() {
            c.text(LEFT, 84.0, "现在没有进行中的任务。", 11.0, W::Regular, theme::GRAY, H::Left);
            return;
        }
        let now = crate::hub::store::now_ms();
        let start = self.hub.task_start.min(self.hub.tasks.len().saturating_sub(1));
        for (row, t) in self.hub.tasks.iter().enumerate().skip(start).take(TASK_ROWS) {
            let y = 58.0 + (row - start) as f32 * 29.0;
            let color = task_color(t.state);
            c.fill(LEFT - 2.0, y, RIGHT - LEFT + 4.0, 26.0, 8.0, theme::CARD);
            c.centered_text(LEFT + 8.0, y + 13.0, &(row + 1).to_string(), 10.0, W::Bold, color, H::Center);
            c.centered_text(LEFT + 18.0, y + 13.0, task_label(t.state), 9.5, W::Semibold, color, H::Left);
            c.centered_text(LEFT + 66.0, y + 13.0, &c.fit(&t.title, 10.5, W::Regular, 132.0), 10.5, W::Regular, theme::WHITE, H::Left);
            let time = if t.state == TaskState::Done { "已结束".to_owned() } else { format!("{} 分钟", ((now - t.since).max(0) / 60_000).max(1)) };
            c.centered_text(RIGHT - 6.0, y + 13.0, &time, 9.0, W::Regular, theme::GRAY, H::Right);
            c.centered_text(RIGHT - 52.0, y + 13.0, &c.fit(&t.project, 9.0, W::Regular, 62.0), 9.0, W::Regular, theme::GRAY, H::Right);
        }
        if self.hub.tasks.len() > TASK_ROWS {
            c.centered_text(RIGHT, 236.0, &format!("第 {} 页 · 用左右箭头翻", start / TASK_ROWS + 1), 9.0, W::Regular, theme::DIM, H::Right);
        }
    }

    fn hub_quota(&mut self, c: &Card, v: &View) {
        let group = self.hub.group();
        let list: Vec<&AccountView> = v.accounts.iter().filter(|a| a.a.group == group).collect();
        let Some(pos) = list.iter().position(|a| a.a.id == self.hub.at()).or(if list.is_empty() { None } else { Some(0) }) else {
            c.text(LEFT, 62.0, &format!("还没有 {} 账号", group.name()), 13.0, W::Bold, theme::WHITE, H::Left);
            if button(c, LEFT, 76.0, 94.0, 24.0, "添加账号", "hub-add-empty", false) {
                self.settings_tab = 11;
                self.open_settings();
            }
            return;
        };
        let a = list[pos];
        let colors = group_colors(group);
        let lim = a.lim.as_ref();
        let (rx, ry) = (82.0, 76.0);
        quota_ring(c, lim, rx, ry, 80.0, colors);
        let letter: String = a.a.id.chars().take(1).collect::<String>().to_uppercase();
        c.centered_text(rx, ry - 3.0, &letter, 16.0, W::Bold, theme::WHITE, H::Center);
        c.centered_text(rx, ry + 10.0, group.name(), 7.5, W::Regular, theme::GRAY, H::Center);
        // 双击圆环中间：完整详情
        if c.hit(rx - 19.0, ry - 19.0, 38.0, 38.0, "hub-ring").on_hover_text("双击看账号详情").double_clicked() {
            self.hub_open_detail(a.a.id.clone(), true);
        }
        if list.len() > 1 {
            c.centered_text(rx, 123.0, &format!("{} / {}", pos + 1, list.len()), 9.0, W::Regular, theme::DIM, H::Center);
        }
        // 第一行：名字 + 标签。双击名字：名称、邮箱、密码的小卡片
        let x = 130.0;
        let name = c.fit(&a.a.id, 13.0, W::Bold, 104.0);
        let nw = c.text(x, 50.0, &name, 13.0, W::Bold, theme::WHITE, H::Left);
        if c.hit(x - 2.0, 37.0, nw + 4.0, 18.0, "hub-name").on_hover_text("双击看邮箱和密码").double_clicked() {
            self.hub.info = !self.hub.info;
            self.hub.hide_secret();
        }
        let mut cx = x + nw + 6.0;
        let plan = lim.map(|l| l.plan.as_str()).filter(|p| !p.is_empty()).unwrap_or(a.a.plan.as_str());
        if !plan.is_empty() {
            cx += chip(c, cx, 46.0, plan, theme::WHITE, theme::BTN) + 4.0;
        }
        let (status, scolor) = status_of(v, a);
        cx += chip(c, cx, 46.0, status, scolor, theme::BTN) + 4.0;
        if v.is_current(&a.a) && cx < RIGHT - 30.0 {
            chip(c, cx, 46.0, "当前", Color32::BLACK, Color32::from_rgb(242, 244, 248));
        }
        if self.hub.info {
            self.hub_info_card(c, a, x);
        } else {
            for (i, (label, w, dot)) in [("5 小时", lim.and_then(|l| l.w5), colors[0]), ("本周", lim.and_then(|l| l.week), colors[1])].into_iter().enumerate() {
                let y = 70.0 + i as f32 * 31.0;
                c.circle(x + 3.0, y - 4.0, 3.0, dot);
                c.text(x + 12.0, y, label, 11.0, W::Semibold, theme::WHITE, H::Left);
                c.text(x + 12.0, y + 14.0, &reset_text(w), 10.0, W::Regular, theme::GRAY, H::Left);
                c.text(RIGHT - 58.0, y + 8.0, "剩余", 9.5, W::Regular, theme::DIM, H::Right);
                c.centered_text(RIGHT, y + 4.0, &left_text(w), 20.0, W::Semibold, theme::WHITE, H::Right);
            }
        }
        match &v.phase {
            Phase::Running { target, steps, at } if *target == a.a.id => self.hub_steps(c, steps, *at, &[]),
            Phase::Waiting { target, still, wrong } if *target == a.a.id => {
                let steps = ["强制关闭 Claude 桌面端", "同步会话列表", "自动启动 Claude 桌面端", "检测桌面端登录…"].map(str::to_owned).to_vec();
                let email = if a.a.email.is_empty() { a.a.id.as_str() } else { a.a.email.as_str() };
                let (first, color) = match (wrong, still) {
                    (Some(w), _) => (format!("桌面端登录的是 {w}，不是 {}。请退出后用：", a.a.id), ORANGE),
                    (_, Some(s)) => (format!("桌面端还登录着 {s}。请退出后用："), theme::GRAY),
                    _ => ("请在桌面端用：".to_owned(), theme::GRAY),
                };
                self.hub_steps(c, &steps, 3, &[(first, color), (format!("{email} 登录，认出后才显示使用中"), theme::WHITE)]);
                if button(c, RIGHT - 52.0, 194.0, 52.0, 20.0, "取消切换", "hub-cancel", false) {
                    self.hub.out.push(Req::CancelWait);
                }
                // 重新登录时用得上：密码复制后 30 秒清空剪贴板
                if !a.a.email.is_empty() && button(c, RIGHT - 110.0, 194.0, 54.0, 20.0, "复制邮箱", "hub-copy-mail", false) {
                    c.ui.ctx().copy_text(a.a.email.clone());
                    self.say("邮箱已复制");
                }
                if a.has_password && button(c, RIGHT - 168.0, 194.0, 54.0, 20.0, "复制密码", "hub-copy-pass", false) {
                    self.hub.out.push(Req::Copy(a.a.id.clone()));
                }
            }
            _ => self.hub_action(c, v, a),
        }
    }

    fn hub_info_card(&mut self, c: &Card, a: &AccountView, x: f32) {
        c.fill(x - 4.0, 58.0, RIGHT - x + 8.0, 62.0, 9.0, theme::CARD);
        c.text(x + 2.0, 72.0, "名称", 9.5, W::Regular, theme::DIM, H::Left);
        c.text(x + 30.0, 72.0, &a.a.id, 10.5, W::Regular, theme::WHITE, H::Left);
        c.text(x + 2.0, 91.0, "邮箱", 9.5, W::Regular, theme::DIM, H::Left);
        let mail = if a.a.email.is_empty() { "没有填写" } else { a.a.email.as_str() };
        c.text(x + 30.0, 91.0, &c.fit(mail, 10.5, W::Regular, 146.0), 10.5, W::Regular, theme::WHITE, H::Left);
        if !a.a.mail_url.is_empty() && button(c, RIGHT - 46.0, 80.0, 46.0, 17.0, "打开邮箱", "hub-info-mail", false) {
            self.pending.push(Cmd::OpenUrl(a.a.mail_url.clone()));
        }
        c.text(x + 2.0, 110.0, "密码", 9.5, W::Regular, theme::DIM, H::Left);
        self.hub_password(c, a, x + 30.0, 110.0, 99.0, RIGHT, "info");
    }

    /// 密码一行：默认圆点；点“显示”解密 10 秒；复制后 30 秒清空剪贴板
    fn hub_password(&mut self, c: &Card, a: &AccountView, x: f32, base: f32, top: f32, right: f32, id: &str) {
        let shown = self.hub.secret.as_ref().filter(|s| s.0 == a.a.id).map(|s| s.1.clone());
        let text = match (&shown, a.has_password) {
            (Some(s), _) => s.clone(),
            (None, true) => "••••••••••".to_owned(),
            (None, false) => "没有保存".to_owned(),
        };
        c.text(x, base, &c.fit(&text, 10.5, W::Regular, right - 72.0 - x), 10.5, W::Regular, theme::WHITE, H::Left);
        if !a.has_password {
            return;
        }
        if button(c, right - 66.0, top, 32.0, 17.0, if shown.is_some() { "隐藏" } else { "显示" }, &format!("hub-eye-{id}"), false) {
            if shown.is_some() {
                self.hub.hide_secret();
            } else {
                self.hub.out.push(Req::Reveal(a.a.id.clone()));
            }
        }
        if button(c, right - 32.0, top, 32.0, 17.0, "复制", &format!("hub-copy-{id}"), false) {
            self.hub.out.push(Req::Copy(a.a.id.clone()));
        }
    }

    fn hub_steps(&self, c: &Card, steps: &[String], at: usize, lines: &[(String, Color32)]) {
        for (i, step) in steps.iter().enumerate() {
            let y = 140.0 + i as f32 * 16.0;
            let (color, fill) = if i < at { (theme::GREEN, true) } else if i == at { (theme::WHITE, false) } else { (theme::DIM, false) };
            if fill {
                c.circle(LEFT + 5.0, y - 3.0, 4.5, color);
            } else if i == at {
                let pulse = 0.35 + 0.65 * (0.5 + 0.5 * (self.t as f32 * 4.0).sin());
                c.circle(LEFT + 5.0, y - 3.0, 4.5, paint::alpha(ORANGE, pulse));
            } else {
                c.ring(LEFT + 5.0, y - 3.0, 4.0, 1.0, color);
            }
            c.text(LEFT + 16.0, y, step, 10.5, W::Regular, color, H::Left);
        }
        for (i, (line, color)) in lines.iter().enumerate() {
            c.text(LEFT, 225.0 + i as f32 * 12.0, &c.fit(line, 9.5, W::Regular, RIGHT - LEFT), 9.5, W::Regular, *color, H::Left);
        }
        c.ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
    }

    fn hub_action(&mut self, c: &Card, v: &View, a: &AccountView) {
        let (y, base) = (134.0, 146.0);
        let mut width = RIGHT - LEFT - 112.0;
        let exhausted = a.lim.as_ref().and_then(|l| usage::exhausted_window(l, crate::hub::store::now_ms()));
        let mut hint: Option<(String, Color32)> = None;
        let unfinished = self.hub.unfinished();
        if let Some((window, resets)) = exhausted {
            let tail = if v.is_current(&a.a) && a.a.group == Group::Claude { "可以点右边快速换号。" } else { "" };
            hint = Some((format!("{window}额度已用完 · {} 重置。{tail}", clock_of(resets)), TASK_RED));
        } else if a.a.group == Group::Claude && unfinished > 0 && v.can_switch(a).is_ok() {
            hint = Some((format!("还有 {unfinished} 个任务没结束，切换会强制关闭桌面端并中断它。"), ORANGE));
        } else if !a.quota_note.is_empty() {
            hint = Some((a.quota_note.clone(), ORANGE));
        }
        match v.can_switch(a) {
            Ok(()) => {
                let key = format!("switch:{}", a.a.id);
                let confirming = self.hub_confirming(&key);
                if button(c, RIGHT - 106.0, y, 106.0, 24.0, if confirming { "确认关闭桌面端并切换" } else { "切换到此账号" }, "hub-switch", true) && self.hub_confirmed(&key) {
                    self.hub.out.push(Req::Switch(a.a.id.clone()));
                }
                if hint.is_none() {
                    hint = Some(if a.a.group == Group::Codex {
                        ("立即生效：之后打开的终端用它的登录。".into(), theme::GRAY)
                    } else {
                        let miss = a.missing.unwrap_or(0);
                        (format!("会强制关闭桌面端，同步会话列表{}，再自动启动；登录后认出才算成功。", if miss > 0 { format!("（差 {miss} 条）") } else { String::new() }), theme::GRAY)
                    });
                }
            }
            Err("正在使用") => {
                if hint.is_none() {
                    hint = Some(if a.a.group == Group::Codex {
                        ("正在使用这个账号。Codex 账号只管理，不同步对话。".into(), theme::GRAY)
                    } else {
                        let list = a.sessions.map(|n| format!("桌面端会话列表 {n} 条{}", match a.missing { Some(m) if m > 0 => format!("，差 {m} 条（退出桌面端时自动补齐）"), _ => "，不缺".into() })).unwrap_or_else(|| "桌面端里还没有它的会话列表".into());
                        (format!("正在使用这个账号。{list}"), theme::GRAY)
                    });
                }
                let mut buttons_left = RIGHT;
                if exhausted.is_some() && a.a.group == Group::Claude {
                    buttons_left -= 70.0;
                    if button(c, buttons_left, y, 70.0, 24.0, "快速换号", "hub-quick", true) {
                        self.hub.confirm = None;
                        self.hub.page = Page::Quick;
                    }
                }
                if a.a.group == Group::Claude && !self.hub.tasks.is_empty() {
                    buttons_left -= 56.0;
                    if button(c, buttons_left, y, 52.0, 24.0, &format!("任务 {}", self.hub.tasks.len()), "hub-tasks", false) {
                        self.hub.task_start = 0;
                        self.hub.page = Page::Tasks;
                        self.hub.confirm = None;
                    }
                    width = (buttons_left - LEFT - 6.0).min(width);
                }
                if exhausted.is_none() && !a.quota_note.is_empty() && a.a.group == Group::Claude && button(c, RIGHT - 60.0, y, 60.0, 24.0, if a.logging_in { "登录中" } else { "无痕登录" }, "hub-login", false) && !a.logging_in {
                    self.hub.out.push(Req::Login(a.a.id.clone()));
                }
            }
            Err(why) => {
                hint = Some((why.to_owned(), ORANGE));
            }
        }
        if let Some((text, color)) = hint {
            for (i, line) in c.wrap(&text, 10.0, W::Regular, width, 2).iter().enumerate() {
                c.text(LEFT, base - 4.0 + i as f32 * 14.0, line, 10.0, W::Regular, color, H::Left);
            }
        }
    }

    fn hub_manage(&mut self, c: &Card, v: &View) {
        let group = self.hub.group();
        let list: Vec<&AccountView> = v.accounts.iter().filter(|a| a.a.group == group).collect();
        let start = self.hub.manage_start.min(list.len().saturating_sub(1));
        for (row, a) in list.iter().skip(start).take(MANAGE_ROWS).enumerate() {
            let y = 36.0 + row as f32 * 40.0;
            let id = &a.a.id;
            c.fill(LEFT - 2.0, y, RIGHT - LEFT + 4.0, 36.0, 10.0, theme::CARD);
            let colors = group_colors(group);
            c.circle(LEFT + 7.0, y + 18.0, 4.0, colors[0]);
            let name = c.fit(id, 11.0, W::Bold, 90.0);
            let nw = c.text(LEFT + 16.0, y + 15.0, &name, 11.0, W::Bold, theme::WHITE, H::Left);
            let (status, scolor) = status_of(v, a);
            c.text(LEFT + 20.0 + nw, y + 15.0, status, 9.5, W::Regular, scolor, H::Left);
            let (line, color) = if a.logging_in {
                ("正在无痕登录…".to_owned(), ORANGE)
            } else if !a.quota_note.is_empty() {
                (a.quota_note.clone(), ORANGE)
            } else if a.a.email.is_empty() {
                ("没有填写邮箱".to_owned(), theme::DIM)
            } else {
                (a.a.email.clone(), theme::GRAY)
            };
            c.text(LEFT + 16.0, y + 29.0, &c.fit(&line, 9.5, W::Regular, 138.0), 9.5, W::Regular, color, H::Left);
            if c.hit(LEFT, y, 158.0, 36.0, ("hub-row", id)).on_hover_text("看详情").clicked() {
                self.hub_open_detail(id.clone(), false);
                return;
            }
            // 右侧的按钮：登录 / 切换、状态、移除
            let by = y + 9.0;
            let mut bx = RIGHT - 2.0;
            let remove_key = format!("remove:{id}");
            let removing = self.hub_confirming(&remove_key);
            bx -= if removing { 34.0 } else { 18.0 };
            if !v.is_current(&a.a) && button(c, bx, by, if removing { 34.0 } else { 18.0 }, 18.0, if removing { "确认" } else { "×" }, &format!("hub-rm-{id}"), false) && self.hub_confirmed(&remove_key) {
                self.hub.out.push(Req::Remove(id.clone()));
            }
            bx -= 36.0;
            let state = match a.a.status.as_str() {
                "disabled" => "停用",
                "banned" => "封号",
                _ => "正常",
            };
            if button(c, bx, by, 33.0, 18.0, state, &format!("hub-st-{id}"), false) {
                self.hub.out.push(Req::CycleStatus(id.clone()));
            }
            bx -= 36.0;
            let needs_login = a.a.group == Group::Claude && a.lim.is_none() || !a.quota_note.is_empty();
            if needs_login {
                if button(c, bx, by, 33.0, 18.0, "登录", &format!("hub-lg-{id}"), false) && !a.logging_in {
                    self.hub.out.push(Req::Login(id.clone()));
                }
            } else if v.can_switch(a).is_ok() {
                let key = format!("switch:{id}");
                let confirming = self.hub_confirming(&key);
                if button(c, bx, by, 33.0, 18.0, if confirming { "确认" } else { "切换" }, &format!("hub-sw-{id}"), confirming) && self.hub_confirmed(&key) {
                    self.hub.page = Page::Quota;
                    self.hub.set_at(id.clone());
                    self.hub.out.push(Req::Switch(id.clone()));
                }
            }
        }
        let by = 200.0;
        if button(c, LEFT, by, 74.0, 22.0, "+ 添加账号", "hub-add", true) {
            self.settings_tab = 11;
            self.open_settings();
        }
        let tail = if !v.last_auto_fill.is_empty() && group == Group::Claude { format!("上次自动补齐：{}", v.last_auto_fill) } else { "额度只走官方 /usage，不读令牌".to_owned() };
        c.text(LEFT + 82.0, by + 15.0, &c.fit(&tail, 9.5, W::Regular, RIGHT - LEFT - 82.0), 9.5, W::Regular, theme::DIM, H::Left);
        if let (Some(uuid), Group::Claude) = (&v.unbound, group) {
            c.text(LEFT, by + 38.0, &c.fit(&format!("桌面端登录了未登记的账号 {}，到设置里对上", &uuid[..8.min(uuid.len())]), 9.5, W::Regular, RIGHT - LEFT), 9.5, W::Regular, ORANGE, H::Left);
        }
    }

    fn hub_detail(&mut self, c: &Card, v: &View) {
        let Some(a) = v.get(&self.hub.detail).cloned() else {
            self.hub.page = Page::Manage;
            return;
        };
        let (back, _) = paint::capsule(c, LEFT, 34.0, 20.0, 20.0, "hub-back", theme::BTN, theme::BTN_HOVER);
        c.centered_text(LEFT + 10.0, 43.5, "‹", 14.0, W::Bold, theme::WHITE, H::Center);
        if back.clicked() {
            self.hub.hide_secret();
            self.hub.page = if self.hub.detail_from_quota { Page::Quota } else { Page::Manage };
            if self.hub.detail_from_quota {
                self.hub.set_at(a.a.id.clone());
            }
            return;
        }
        let colors = group_colors(a.a.group);
        c.circle(LEFT + 29.0, 44.0, 4.0, colors[0]);
        let nw = c.text(LEFT + 38.0, 49.0, &a.a.id, 13.0, W::Bold, theme::WHITE, H::Left);
        let (status, scolor) = status_of(v, &a);
        chip(c, LEFT + 44.0 + nw, 44.0, status, scolor, theme::BTN);
        let row = |c: &Card, i: usize, label: &str, value: &str| -> (f32, f32) {
            let base = 72.0 + i as f32 * 20.0;
            c.text(LEFT, base, label, 9.5, W::Regular, theme::DIM, H::Left);
            c.text(LEFT + 50.0, base, &c.fit(value, 10.5, W::Regular, 180.0), 10.5, W::Regular, theme::WHITE, H::Left);
            (base, base - 12.0)
        };
        let (_, top) = row(c, 0, "邮箱", if a.a.email.is_empty() { "没有填写" } else { &a.a.email });
        if !a.a.mail_url.is_empty() && button(c, RIGHT - 52.0, top, 52.0, 17.0, "打开邮箱", "hub-mail", false) {
            self.pending.push(Cmd::OpenUrl(a.a.mail_url.clone()));
        }
        row(c, 1, "注册 / 套餐", &format!("{} · {}", if a.a.reg_date.is_empty() { "—" } else { &a.a.reg_date }, if a.a.plan.is_empty() { "—" } else { &a.a.plan }));
        if a.a.group == Group::Claude {
            let (_, top) = row(c, 2, "桌面端 ID", a.a.account_uuid.as_deref().map(|u| &u[..8]).unwrap_or("还没对上：到设置里绑定"));
            if a.a.account_uuid.is_some() {
                let key = format!("unbind:{}", a.a.id);
                let confirming = self.hub_confirming(&key);
                if button(c, RIGHT - 32.0, top, 32.0, 17.0, if confirming { "确认" } else { "解绑" }, "hub-unbind", false) && self.hub_confirmed(&key) {
                    self.hub.out.push(Req::Unbind(a.a.id.clone()));
                }
            }
            let source = usage::quota_dir(&a.a).and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned())).map(|n| format!("官方 /usage · {n}")).unwrap_or_else(|| "还没登录取额度".into());
            let (_, top) = row(c, 3, "额度来源", &source);
            if button(c, RIGHT - 52.0, top, 52.0, 17.0, if a.logging_in { "登录中" } else { "重新登录" }, "hub-relogin", false) && !a.logging_in {
                self.hub.out.push(Req::Login(a.a.id.clone()));
            }
        } else {
            row(c, 2, "登录目录", &crate::hub::codex::home_of(&a.a).to_string_lossy());
            row(c, 3, "额度来源", "Codex 会话记录里的官方额度");
        }
        c.text(LEFT, 152.0, "密码", 9.5, W::Regular, theme::DIM, H::Left);
        self.hub_password(c, &a, LEFT + 50.0, 152.0, 140.0, RIGHT, "detail");
        // 下半部分：额度用完记录 + 热力图
        c.text(LEFT, 174.0, "额度用完记录", 10.5, W::Semibold, theme::WHITE, H::Left);
        if a.exhausted.is_empty() {
            c.text(LEFT, 188.0, "还没用完过", 9.5, W::Regular, theme::DIM, H::Left);
        }
        for (i, line) in a.exhausted.iter().take(2).enumerate() {
            c.text(LEFT, 188.0 + i as f32 * 13.0, &c.fit(line, 9.5, W::Regular, 150.0), 9.5, W::Regular, theme::GRAY, H::Left);
        }
        let hx = RIGHT - 14.0 * 12.0;
        c.text(hx, 174.0, "近 4 周用量", 10.5, W::Semibold, theme::WHITE, H::Left);
        let days = self.hub.heat.as_ref().filter(|h| h.0 == a.a.id).map(|h| h.1.clone());
        let max = days.as_ref().and_then(|d| d.iter().max().copied()).unwrap_or(0).max(1) as f32;
        for i in 0..crate::hub::heat::DAYS {
            let (cx, cy) = (hx + (i % 14) as f32 * 12.0, 180.0 + (i / 14) as f32 * 12.0);
            let used = days.as_ref().map(|d| d[i]).unwrap_or(0);
            let color = if used == 0 { theme::TRACK } else { paint::alpha(colors[0], 0.3 + 0.7 * (used as f32 / max).sqrt()) };
            c.fill(cx, cy, 10.0, 10.0, 2.5, color);
        }
        // 底部：会话列表和撤销
        let text = if a.a.group == Group::Claude {
            let list = a.sessions.map(|n| format!("桌面端会话列表 {n} 条")).unwrap_or_else(|| "桌面端里还没有会话列表".into());
            let miss = a.missing.filter(|m| *m > 0).map(|m| format!("，差 {m} 条")).unwrap_or_default();
            format!("{list}{miss} · 同步备份 {} 份", a.backups)
        } else {
            "Codex 只管理账号和额度，不同步对话".into()
        };
        c.text(LEFT, 230.0, &c.fit(&text, 9.5, W::Regular, RIGHT - LEFT - 100.0), 9.5, W::Regular, theme::GRAY, H::Left);
        if a.a.group == Group::Claude && a.backups > 0 {
            let key = "undo";
            let confirming = self.hub_confirming(key);
            if button(c, RIGHT - 94.0, 216.0, 94.0, 20.0, if confirming { "确认（会关桌面端）" } else { "撤销上次同步" }, "hub-undo", false) && self.hub_confirmed(key) {
                self.hub.out.push(Req::Undo);
            }
        }
    }

    // ---------------- 设置窗口：添加账号、改资料、绑定 ----------------

    pub(crate) fn hub_settings(&mut self, ui: &mut egui::Ui) {
        let Some(v) = self.hub.view.clone() else {
            ui.label("正在读取账号…");
            return;
        };
        settings_label(ui, "多账号");
        let mut keep = true;
        settings_card(ui, |ui| {
            // 切换进行中不能关，否则等待登录的流程没人接手
            ui.add_enabled_ui(v.phase == Phase::Idle, |ui| {
                setting_row(ui, "多账号模式", "关闭后恢复原来的 AI 额度页（会读取本机登录文件）。账号资料保留", &mut keep);
            });
        });
        if !keep && v.phase == Phase::Idle {
            self.hub_set_mode(false);
            return;
        }
        settings_label(ui, "显示和同步");
        settings_card(ui, |ui| {
            let mut fill = v.auto_fill;
            setting_row(ui, "退出桌面端时自动补齐", "给最后登录的账号补上缺的会话列表，先备份、可撤销", &mut fill);
            if fill != v.auto_fill {
                self.hub.out.push(Req::SetAutoFill(fill));
            }
        });
        settings_label(ui, "额度用完的提醒");
        settings_card(ui, |ui| {
            let mut alert = v.alert;
            let row = |ui: &mut egui::Ui, title: &str, options: &[(u32, &str)], value: &mut u32| {
                settings_control_row(ui, title, 112.0, |ui| {
                    ui.horizontal(|ui| {
                        for (n, label) in options {
                            ui.selectable_value(value, *n, *label);
                        }
                    }).response
                });
            };
            row(ui, "震动时长", &[(0, "不震动"), (1, "1 秒"), (3, "3 秒"), (5, "5 秒")], &mut alert.shake_secs);
            row(ui, "没换号时再提醒", &[(0, "不再提醒"), (5, "每 5 分钟"), (10, "每 10 分钟"), (30, "每 30 分钟")], &mut alert.remind_min);
            ui.separator();
            setting_row(ui, "再提醒时也震动", "默认只发消息，不再震动", &mut alert.remind_shake);
            ui.label(egui::RichText::new("额度用完时红字一直闪，只短震一次。还没换号就按上面的间隔发系统通知；换号或额度重置后不再提醒。只是提醒，不会自动换号。").size(12.0));
            if alert != v.alert {
                self.hub.out.push(Req::SetAlert(alert));
                if let Some(view) = &mut self.hub.view {
                    view.alert = alert;
                }
            }
        });
        settings_label(ui, "添加账号");
        settings_card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.hub.form.codex, false, "Claude");
                ui.selectable_value(&mut self.hub.form.codex, true, "Codex");
            });
            settings_control_row(ui, "名字", 56.0, |ui| ui.add_sized([ui.available_width(), 27.0], egui::TextEdit::singleline(&mut self.hub.form.name).hint_text("英文、数字和短横线，如 work")));
            settings_control_row(ui, "邮箱", 56.0, |ui| ui.add_sized([ui.available_width(), 27.0], egui::TextEdit::singleline(&mut self.hub.form.email).hint_text("name@example.com")));
            ui.label(egui::RichText::new(if self.hub.form.codex { "添加后打开一个终端运行 codex login。" } else { "添加后在 Edge 无痕窗口登录，只用来取官方额度，不读令牌。再用它在桌面端登录一次，下面对上账号 ID 后就能切换。" }).size(12.0));
            if ui.button("添加").clicked() {
                let group = if self.hub.form.codex { Group::Codex } else { Group::Claude };
                self.hub.out.push(Req::Add { group, name: std::mem::take(&mut self.hub.form.name), email: std::mem::take(&mut self.hub.form.email) });
            }
        });
        settings_label(ui, "账号资料");
        settings_card(ui, |ui| {
            let selected = self.hub.form.edit.clone();
            egui::ComboBox::from_id_salt("hub-edit").selected_text(if selected.is_empty() { "选择账号" } else { selected.as_str() }).show_ui(ui, |ui| {
                for a in &v.accounts {
                    if ui.selectable_label(selected == a.a.id, &a.a.id).clicked() {
                        let f = &mut self.hub.form;
                        f.edit = a.a.id.clone();
                        f.edit_email = a.a.email.clone();
                        f.edit_mail = a.a.mail_url.clone();
                        f.edit_plan = a.a.plan.clone();
                        f.edit_reg = a.a.reg_date.clone();
                        f.edit_note = a.a.note.clone();
                        f.password.clear();
                    }
                }
            });
            let Some(a) = v.get(&selected) else { return };
            let mut save: Vec<(&'static str, String)> = Vec::new();
            {
                let f = &mut self.hub.form;
                let rows: [(&str, &'static str, &mut String, &str); 5] = [
                    ("邮箱", "email", &mut f.edit_email, a.a.email.as_str()),
                    ("邮箱网址", "mailUrl", &mut f.edit_mail, a.a.mail_url.as_str()),
                    ("套餐", "plan", &mut f.edit_plan, a.a.plan.as_str()),
                    ("注册时间", "regDate", &mut f.edit_reg, a.a.reg_date.as_str()),
                    ("备注", "note", &mut f.edit_note, a.a.note.as_str()),
                ];
                for (label, field, value, old) in rows {
                    settings_control_row(ui, label, 64.0, |ui| ui.add_sized([ui.available_width(), 27.0], egui::TextEdit::singleline(value)));
                    if value.trim() != old {
                        save.push((field, value.clone()));
                    }
                }
            }
            ui.add_enabled_ui(!save.is_empty(), |ui| {
                if ui.button("保存资料").clicked() {
                    for (field, value) in save.drain(..) {
                        self.hub.out.push(Req::Update { id: selected.clone(), field, value });
                    }
                }
            });
            ui.separator();
            settings_control_row(ui, "密码", 64.0, |ui| ui.add_sized([ui.available_width(), 27.0], egui::TextEdit::singleline(&mut self.hub.form.password).password(true).hint_text(if a.has_password { "已保存；输入新密码可替换" } else { "用 Windows 加密保存" })));
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.hub.form.password.is_empty(), egui::Button::new("保存密码")).clicked() {
                    self.hub.out.push(Req::SetPassword(selected.clone(), std::mem::take(&mut self.hub.form.password)));
                }
                if a.has_password && ui.button("删除已保存的密码").clicked() {
                    self.hub.out.push(Req::SetPassword(selected.clone(), String::new()));
                }
            });
            if a.a.group == Group::Claude {
                ui.separator();
                if let Some(uuid) = &v.unbound {
                    if a.a.account_uuid.is_none() && ui.button(format!("桌面端刚登录的账号 {} 就是它，对上", &uuid[..8.min(uuid.len())])).clicked() {
                        self.hub.out.push(Req::Bind(selected.clone(), uuid.clone()));
                    }
                } else if a.a.account_uuid.is_none() {
                    ui.label("用它在 Claude 桌面端登录一次，这里会出现对上账号的按钮。");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multi_account_mode_stays_off_unless_chosen_or_data_exists() {
        assert!(!mode_on(None, false), "没用过这个功能的人默认保持原样");
        assert!(mode_on(None, true), "本机已有账号数据时直接可用");
        assert!(!mode_on(Some(false), true), "用户关掉后不能因为有数据又打开");
        assert!(mode_on(Some(true), false));
    }
    #[test]
    fn multi_account_mode_choice_is_remembered() {
        let dir = std::env::temp_dir().join(format!("daozai-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let old = std::env::var_os("DAOZAI_DATA_DIR");
        std::env::set_var("DAOZAI_DATA_DIR", &dir);
        assert_eq!(saved_mode(), None);
        save_mode(true);
        assert_eq!(saved_mode(), Some(true));
        save_mode(false);
        assert_eq!(saved_mode(), Some(false));
        match old { Some(v) => std::env::set_var("DAOZAI_DATA_DIR", v), None => std::env::remove_var("DAOZAI_DATA_DIR") }
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn reset_text_never_invents_a_time() {
        assert_eq!(reset_text(None), "—");
        assert_eq!(reset_text(Some(Win { used: 0.0, resets_at: None })), "未开始计时");
        assert_eq!(reset_text(Some(Win { used: 50.0, resets_at: None })), "重置时间未知");
        assert_eq!(reset_text(Some(Win { used: 50.0, resets_at: Some(1000) })), "等待刷新");
    }
    #[test]
    fn pages_fit_inside_the_window() {
        let mut s = State::default();
        for page in [Page::Quota, Page::Manage, Page::Detail, Page::Quick, Page::Tasks] {
            s.page = page;
            assert!(s.height() <= crate::app::WIN_H - 10.0);
        }
    }
    #[test]
    fn long_titles_split_at_punctuation_and_keep_every_character() {
        let width = |s: &str| s.chars().count() as f32 * 10.0;
        let text = "修复额度用完不提醒的问题，并补上对应的测试";
        let parts = split_title(text, 140.0, width);
        assert!(parts.len() > 1);
        assert!(parts[0].ends_with('，'), "优先在标点处断开：{parts:?}");
        assert_eq!(parts.concat(), text, "分页不能丢字");
        assert!(parts.iter().all(|p| width(p) <= 140.0));
        assert_eq!(split_title("短", 140.0, width), vec!["短"]);
        let plain = split_title("一二三四五六七八九十", 40.0, width);
        assert_eq!(plain.concat(), "一二三四五六七八九十");
        assert!(plain.iter().all(|p| width(p) <= 40.0));
    }
    #[test]
    fn ring_percentages_mean_what_is_left() {
        assert_eq!(left_text(Some(Win { used: 84.0, resets_at: None })), "16%");
        assert_eq!(left_text(Some(Win { used: 100.0, resets_at: None })), "0%");
        assert_eq!(left_text(None), "—");
    }
}
