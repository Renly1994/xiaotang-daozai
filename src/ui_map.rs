//! 调试用「UI Map」导出：把当前可见屏的控件矩形与状态写成 JSON。
//!
//! 目的：让 E2E 测试直接读岛仔自报的矩形与状态、按坐标点击，不再「截图 + OCR 找字」。
//! **只影响调试/测试路径**：未设置 `DAOZAI_UI_MAP` 时不分配、不加锁、不落盘。
//!
//! 契约：`docs/superpowers/research/2026-11-20-agent-ui-map-contract.md`（schema = 1）。
//!
//! - 开关：`DAOZAI_UI_MAP=<文件路径|目录>`；已存在的目录或以 `\` / `/` 结尾时写 `<dir>/ui-map.json`。
//! - 坐标：`nodes[].rect = [x, y, w, h]` 是 **canvas 逻辑坐标**（egui points，原点 = 画布左上角）；
//!   测试脚本按 `屏幕坐标 = 窗口左上 + rect * scale` 换算（scale = `ctx.pixels_per_point()`）。
//! - 落盘：原子写（`<target>.tmp` → rename），UTF-8 无 BOM；两次写间隔 ≥ 50ms，状态变化立即写。
//! - 范围：只登记**当前可见的那一屏**（岛体 + 当前卡片/结果层），不读整页内容。
//!
//! 帧生命周期（**不需要**改 `App::frame` / `Cards::update`）：
//! 1. 首次登记时给当前 `egui::Context` 装一次 `on_begin_pass` / `on_end_pass` 回调；
//! 2. 每个 pass 开始清空节点与可见屏状态（默认「什么都没有」）；
//! 3. 各卡片的 `draw_*()` 在真正绘制处调用 [`node`] 登记矩形、调用 `publish_*` 发布状态；
//! 4. pass 结束时自动结算：内容有变化且（状态变了 或 距上次写 ≥ 50ms）才落盘。
//!
//! 这样卡片刚关闭的那一帧（没有任何卡片绘制）也会被写成「没有卡片」，不会停在旧快照上。
//!
//! 还没有接入的部分（需要在别人负责的文件里加 1~2 行）见 `docs` 里的清单，例如
//! `state.card.id`（会话 id）与完成卡/结果层的节点。

use eframe::egui;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// JSON 契约版本。
pub(crate) const SCHEMA: u32 = 1;
/// 两次落盘之间的最小间隔。
const THROTTLE: Duration = Duration::from_millis(50);
/// 环境变量名。
const ENV_KEY: &str = "DAOZAI_UI_MAP";
/// 目录形态的目标文件名。
const FILE_NAME: &str = "ui-map.json";
/// 安装在 `egui::Context` 上的标记 id。
const INSTALLED_KEY: &str = "daozai-ui-map-installed";

// ---------------------------------------------------------------
// 开关
// ---------------------------------------------------------------

/// 缓存解析后的目标（`None` = 关闭）。生产路径只读这个 OnceLock，关闭时零分配、零加锁。
static ENV_TARGET: OnceLock<Option<PathBuf>> = OnceLock::new();

// 测试用目标放**线程局部**：`cargo test` 的用例共享进程，共享全局目标会让并行用例互相污染。
#[cfg(test)]
thread_local! {
    static TEST_TARGET: RefCell<Option<Option<PathBuf>>> = const { RefCell::new(None) };
}

/// 当前是否开启导出。关闭时每次调用只是一次原子读 + 分支，不分配、不加锁、不碰文件。
pub(crate) fn enabled() -> bool {
    #[cfg(test)]
    if let Some(forced) = TEST_TARGET.with(|slot| slot.borrow().clone()) {
        return forced.is_some();
    }
    env_target().is_some()
}

fn env_target() -> &'static Option<PathBuf> {
    ENV_TARGET.get_or_init(|| resolve_target(std::env::var_os(ENV_KEY).as_deref()))
}

fn target() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(forced) = TEST_TARGET.with(|slot| slot.borrow().clone()) {
        return forced;
    }
    env_target().clone()
}

/// 解析 `DAOZAI_UI_MAP`：目录写 `<dir>/ui-map.json`，其余按文件路径原样使用。
fn resolve_target(raw: Option<&OsStr>) -> Option<PathBuf> {
    let raw = raw?;
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(raw);
    let text = raw.to_string_lossy();
    let trailing = text.ends_with('\\') || text.ends_with('/');
    if trailing || path.is_dir() {
        return Some(path.join(FILE_NAME));
    }
    Some(path)
}

// ---------------------------------------------------------------
// 状态与节点
// ---------------------------------------------------------------

#[derive(Clone, PartialEq, Default)]
struct CardState {
    kind: Option<&'static str>,
    id: String,
    details: bool,
    /// 当前题号（提问卡）。
    index: usize,
    /// 题目总数（提问卡）。
    count: usize,
    /// 已发出决定、等待宿主收口。
    submitting: bool,
    /// 发送失败等需要展示的原因。
    error: Option<String>,
}

#[derive(Clone, PartialEq)]
struct ResultState {
    open: bool,
    /// 本机生成的说明层（没有回复正文）。
    hint: bool,
    title: String,
    scroll: f32,
    max_scroll: f32,
}

impl Default for ResultState {
    fn default() -> Self {
        Self { open: true, hint: false, title: String::new(), scroll: 0.0, max_scroll: 0.0 }
    }
}

/// 每个 pass 会重置的「可见屏」状态。
#[derive(Clone, PartialEq)]
struct State {
    /// 岛上是否有 Agent 面板（详情 / 结果层）占位。
    panel: bool,
    /// 详情是否展开。
    details: bool,
    /// `none` / `agent`。
    notice: &'static str,
    /// 待处理请求数（跨 pass 保留：结果层那一屏没有卡片可问）。
    queue: usize,
    /// 当前卡片 id（跨 pass 保留，由 `publish_card_id` 更新）。
    id: String,
    card: CardState,
    result: Option<ResultState>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            panel: false,
            details: false,
            notice: "none",
            queue: 0,
            id: String::new(),
            card: CardState::default(),
            result: None,
        }
    }
}

#[derive(Clone)]
struct Node {
    id: String,
    /// 登记时是屏幕坐标（egui points）；落盘前减去画布原点转成 canvas 逻辑坐标。
    rect: [f32; 4],
    label: Option<String>,
    enabled: bool,
}

#[derive(Clone)]
struct Snapshot {
    t: f64,
    scale: f32,
    /// `[min_x, min_y, w, h]`（屏幕坐标）。
    canvas: [f32; 4],
    state: State,
    nodes: Vec<Node>,
}

#[derive(Default)]
struct Map {
    started: bool,
    nodes: Vec<Node>,
    state: State,
    /// 画布（窗口客户区）在屏幕坐标下的矩形：`[min_x, min_y, w, h]`。
    canvas: [f32; 4],
    scale: f32,
    t: f64,
    /// 被 50ms 节流推迟的快照。
    pending: Option<Snapshot>,
    last_attempt: Option<Instant>,
    last_write: Option<Instant>,
    /// 不含 `t` 的内容签名，用于判断「画面其实没变」。
    last_signature: Option<String>,
    last_state: Option<State>,
    #[allow(dead_code)]
    last_error: Option<String>,
    writes: u64,
}

static MAP: OnceLock<Mutex<Map>> = OnceLock::new();

fn map() -> &'static Mutex<Map> {
    MAP.get_or_init(|| Mutex::new(Map::default()))
}

// ---------------------------------------------------------------
// 每 pass 的自动挂钩（调用方零感知）
// ---------------------------------------------------------------

/// 确保 per-pass 回调已经装到当前上下文（幂等，几纳秒级）。
///
/// 所有 `publish_*` 会自动调用；`metrics::button` / `metrics::chip` 也调用它，
/// 这样即使某个屏（例如完成卡）暂时还没有 `publish_*`，节点登记也能正常落盘。
pub(crate) fn ensure(ctx: &egui::Context) {
    if !enabled() {
        return;
    }
    ensure_installed(ctx);
}

/// 给当前 `egui::Context` 装一次 per-pass 回调（已经装过就直接返回）。
fn ensure_installed(ctx: &egui::Context) {
    let flag = egui::Id::new(INSTALLED_KEY);
    if ctx.data(|d| d.get_temp::<bool>(flag).unwrap_or(false)) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(flag, true));
    ctx.on_begin_pass("daozai.ui_map.begin", Arc::new(|ctx| begin_pass(ctx)));
    ctx.on_end_pass("daozai.ui_map.end", Arc::new(|ctx| end_pass(ctx)));
}

/// pass 开始：清空本帧节点，把可见屏状态复位成「什么都没有」。
fn begin_pass(ctx: &egui::Context) {
    let Ok(mut m) = map().lock() else {
        return;
    };
    m.started = true;
    m.nodes.clear();
    let queue = m.state.queue;
    let id = std::mem::take(&mut m.state.id);
    m.state = State { queue, id, ..State::default() };
    m.set_canvas(ctx);
    m.scale = ctx.pixels_per_point();
    m.t = ctx.input(|i| i.time);
}

/// pass 结束：结算本帧并（按策略）落盘。没有登记任何东西的帧也会在这里把「已清空」写出去。
fn end_pass(ctx: &egui::Context) {
    let Ok(mut m) = map().lock() else {
        return;
    };
    if !m.started {
        return;
    }
    m.set_canvas(ctx);
    m.scale = ctx.pixels_per_point();
    m.t = ctx.input(|i| i.time);
    if let Some(snap) = m.finalize() {
        m.submit(snap, Some(ctx));
    }
    m.started = false;
}

// ---------------------------------------------------------------
// 登记接口（绘制处调用）
// ---------------------------------------------------------------

/// 登记一个可见控件。`rect` 用屏幕坐标（`Card::rect(..)` 的返回值即可）。
///
/// `label` 是屏幕上的文案（空串 = 不写该字段）；`interactive = false` 时才写 `enabled: false`。
pub(crate) fn node(id: &str, rect: egui::Rect, label: &str, interactive: bool) {
    if !enabled() {
        return;
    }
    let Ok(mut m) = map().lock() else {
        return;
    };
    if !m.started {
        return;
    }
    let label = (!label.trim().is_empty()).then(|| label.to_owned());
    m.push_node(id, rect, label, interactive);
}

/// 撤销本帧已登记的节点（例如同一个按钮在两个卡片里登记了不同 id）。
pub(crate) fn drop_node(id: &str) {
    if !enabled() {
        return;
    }
    let Ok(mut m) = map().lock() else {
        return;
    };
    m.nodes.retain(|n| n.id != id);
}

/// 整屏状态：`panel` = 岛体是否被 Agent 面板占用，`details` = 详情是否展开。
pub(crate) fn publish_frame(ctx: &egui::Context, panel: bool, details: bool, agent_notice: bool) {
    if !enabled() {
        return;
    }
    ensure_installed(ctx);
    let Ok(mut m) = map().lock() else {
        return;
    };
    m.state.panel = panel;
    m.state.details = details;
    m.state.notice = if agent_notice { "agent" } else { "none" };
}

/// 待处理请求数（跨 pass 保留）。
pub(crate) fn publish_queue(queue: usize) {
    if !enabled() {
        return;
    }
    let Ok(mut m) = map().lock() else {
        return;
    };
    m.state.queue = queue;
}

/// 当前会话 id（跨 pass 保留；由 `agent_cards` 侧补 1 行调用，见任务报告清单）。
#[allow(dead_code)]
pub(crate) fn publish_card_id(id: &str) {
    if !enabled() {
        return;
    }
    let Ok(mut m) = map().lock() else {
        return;
    };
    m.state.id = id.to_owned();
    m.state.card.id = id.to_owned();
}

/// 当前卡片（审批 / 提问 / 完成）状态。`kind = None` 表示这一屏没有卡片。
pub(crate) fn publish_card(
    ctx: &egui::Context,
    kind: Option<&'static str>,
    details: bool,
    index: usize,
    count: usize,
    submitting: bool,
    error: Option<&str>,
) {
    if !enabled() {
        return;
    }
    ensure_installed(ctx);
    let Ok(mut m) = map().lock() else {
        return;
    };
    m.state.card = CardState {
        kind,
        id: m.state.id.clone(),
        details,
        index,
        count,
        submitting,
        error: error.map(str::to_owned),
    };
}

/// 结果层（「查看回复」）状态。
#[allow(dead_code)]
pub(crate) fn publish_result(ctx: &egui::Context, hint: bool, title: &str) {
    if !enabled() {
        return;
    }
    ensure_installed(ctx);
    let Ok(mut m) = map().lock() else {
        return;
    };
    m.state.result = Some(ResultState { open: true, hint, title: title.to_owned(), ..ResultState::default() });
}

/// 结果层正文的滚动偏移与可滚动上限。
#[allow(dead_code)]
pub(crate) fn publish_result_scroll(scroll: f32, max_scroll: f32) {
    if !enabled() {
        return;
    }
    let Ok(mut m) = map().lock() else {
        return;
    };
    if let Some(result) = m.state.result.as_mut() {
        result.scroll = scroll;
        result.max_scroll = max_scroll;
    }
}

/// 内部按钮 id（`agent_allow` 等）→ 契约节点 id。
pub(crate) fn footer_node_id(id: &str) -> Option<&'static str> {
    Some(match id {
        "agent-return" => "btn.return",
        "agent-deny" => "btn.deny",
        "agent-allow" => "btn.allow",
        "agent-prev" | "agent-review-back" => "btn.prev",
        "agent-next" => "btn.next",
        "agent-review" => "btn.submit",
        "agent-review-submit" => "btn.confirm",
        "agent-result-open" => "btn.view",
        "agent-done-close" | "agent-result-close" => "btn.close",
        "agent-result-copy" => "btn.copy",
        _ => return None,
    })
}

// ---------------------------------------------------------------
// 内部实现
// ---------------------------------------------------------------

impl Map {
    fn set_canvas(&mut self, ctx: &egui::Context) {
        let rect = ctx.screen_rect();
        self.canvas = [rect.min.x, rect.min.y, rect.width(), rect.height()];
    }

    fn push_node(&mut self, id: &str, rect: egui::Rect, label: Option<String>, enabled: bool) {
        let rect = [rect.min.x, rect.min.y, rect.width(), rect.height()];
        let node = Node { id: id.to_owned(), rect, label, enabled };
        if let Some(slot) = self.nodes.iter_mut().find(|n| n.id == id) {
            *slot = node;
        } else {
            self.nodes.push(node);
        }
    }

    /// 结算本帧：节点从屏幕坐标换算成 canvas 逻辑坐标。
    fn finalize(&mut self) -> Option<Snapshot> {
        let origin = [self.canvas[0], self.canvas[1]];
        let nodes: Vec<Node> = self
            .nodes
            .iter()
            .map(|n| Node {
                id: n.id.clone(),
                rect: [n.rect[0] - origin[0], n.rect[1] - origin[1], n.rect[2], n.rect[3]],
                label: n.label.clone(),
                enabled: n.enabled,
            })
            .collect();
        Some(Snapshot {
            t: self.t,
            scale: self.scale,
            canvas: self.canvas,
            state: self.state.clone(),
            nodes,
        })
    }

    /// 落盘策略：状态变化立即写；否则 50ms 节流；内容没变不写（不做无意义的 rename）。
    fn submit(&mut self, snap: Snapshot, ctx: Option<&egui::Context>) {
        let state_changed = self.last_state.as_ref() != Some(&snap.state);
        let due = self.last_attempt.is_none_or(|at| at.elapsed() >= THROTTLE);
        if !state_changed && !due {
            // 只有动画/滚动在动：留到下一帧再比较，并保证下一帧会到来。
            if let Some(ctx) = ctx {
                let wait = THROTTLE.saturating_sub(self.last_write.map_or(Duration::ZERO, |at| at.elapsed()));
                ctx.request_repaint_after(wait.max(Duration::from_millis(1)));
            }
            self.pending = Some(snap);
            return;
        }
        let text = render(&snap);
        // 内容比较不看 `t`：t 每帧都变，若把它算进内容里，静止画面也会每 50ms 重写一次。
        let signature = render(&Snapshot { t: 0.0, ..snap.clone() });
        if self.last_signature.as_deref() == Some(signature.as_str()) {
            self.pending = None;
            return;
        }
        self.last_attempt = Some(Instant::now());
        if let Some(path) = target() {
            match write_atomic(&path, text.as_bytes()) {
                Ok(()) => {
                    self.last_write = Some(Instant::now());
                    self.last_signature = Some(signature);
                    self.last_state = Some(snap.state);
                    self.last_error = None;
                    self.writes += 1;
                }
                Err(err) => self.last_error = Some(err.to_string()),
            }
        }
        self.pending = None;
    }
}

/// 把快照渲染成契约 JSON（`serde_json` 保证不会输出 `NaN` 之类的非法 JSON）。
fn render(snap: &Snapshot) -> String {
    let nodes: Vec<Value> = snap
        .nodes
        .iter()
        .map(|n| {
            let mut obj = serde_json::Map::new();
            obj.insert("id".into(), json!(n.id));
            obj.insert("rect".into(), json!([num(n.rect[0]), num(n.rect[1]), num(n.rect[2]), num(n.rect[3])]));
            if let Some(label) = &n.label {
                obj.insert("label".into(), json!(label));
            }
            if !n.enabled {
                obj.insert("enabled".into(), json!(false));
            }
            Value::Object(obj)
        })
        .collect();
    let card = match snap.state.card.kind {
        Some(kind) => json!({
            "kind": kind,
            "id": snap.state.card.id,
            "details": snap.state.card.details,
            "index": snap.state.card.index,
            "count": snap.state.card.count,
            "submitting": snap.state.card.submitting,
            "error": snap.state.card.error,
        }),
        None => Value::Null,
    };
    let result = match &snap.state.result {
        Some(result) => json!({
            "open": result.open,
            "hint": result.hint,
            "title": result.title,
            "scroll": num(result.scroll),
            "maxScroll": num(result.max_scroll),
        }),
        None => Value::Null,
    };
    let state = json!({
        "panel": snap.state.panel,
        "details": snap.state.details,
        "notice": snap.state.notice,
        "queue": snap.state.queue,
        "card": card,
        "result": result,
    });
    let body = json!({
        "schema": SCHEMA,
        "t": num64(snap.t),
        "scale": num(snap.scale),
        "canvas": { "w": num(snap.canvas[2]), "h": num(snap.canvas[3]) },
        "state": state,
        "nodes": nodes,
    });
    serde_json::to_string_pretty(&body).unwrap_or_default()
}

/// 归一化浮点：保留两位小数，避免每帧末位抖动导致「内容其实没变却重新写」。
fn num(v: f32) -> f64 {
    ((v as f64) * 100.0).round() / 100.0
}

fn num64(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// 原子写：先写 `<target>.tmp`，再 rename 覆盖；UTF-8 无 BOM。
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

// ---------------------------------------------------------------
// 测试钩子（单测在 `src/ui_map_tests.rs`）
// ---------------------------------------------------------------

#[cfg(test)]
pub(crate) fn set_test_target(path: Option<PathBuf>) {
    TEST_TARGET.with(|slot| *slot.borrow_mut() = Some(path));
    reset();
}

#[cfg(test)]
pub(crate) fn clear_test_target() {
    TEST_TARGET.with(|slot| *slot.borrow_mut() = None);
    reset();
}

#[cfg(test)]
pub(crate) fn reset() {
    if let Ok(mut m) = map().lock() {
        *m = Map::default();
    }
}

#[cfg(test)]
pub(crate) fn write_count() -> u64 {
    map().lock().map(|m| m.writes).unwrap_or(0)
}

// ---------------------------------------------------------------
// 私有辅助（路径解析 / JSON 契约 / 原子写）的单测；
// 面向 UI 的行为测试在 `src/ui_map_tests.rs`。
// ---------------------------------------------------------------

#[cfg(test)]
mod unit {
    use super::*;

    fn snapshot() -> Snapshot {
        // 画布原点不是 (0,0)：finalize 负责把屏幕坐标换算成 canvas 逻辑坐标。
        let mut m = Map::default();
        m.started = true;
        m.canvas = [10.0, 20.0, 960.0, 520.0];
        m.scale = 1.25;
        m.t = 12.3456;
        m.state.panel = true;
        m.state.details = true;
        m.state.notice = "agent";
        m.state.queue = 1;
        m.state.card = CardState {
            kind: Some("question"),
            id: "e2e-session-1".into(),
            details: true,
            index: 0,
            count: 2,
            submitting: false,
            error: None,
        };
        m.push_node("island.bar", egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(640.0, 44.0)), None, true);
        m.push_node(
            "option.0",
            egui::Rect::from_min_size(egui::pos2(30.0, 30.5), egui::vec2(383.0, 30.0)),
            Some("A".into()),
            false,
        );
        m.finalize().unwrap()
    }

    #[test]
    fn target_accepts_files_and_directories() {
        assert_eq!(resolve_target(None), None);
        assert_eq!(resolve_target(Some(OsStr::new(""))), None);
        assert_eq!(resolve_target(Some(OsStr::new(r"C:\tmp\map.json"))).unwrap(), PathBuf::from(r"C:\tmp\map.json"));
        let dir = std::env::temp_dir().join(format!("daozai-ui-map-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(resolve_target(Some(dir.as_os_str())).unwrap(), dir.join(FILE_NAME), "已存在的目录写 ui-map.json");
        assert_eq!(
            resolve_target(Some(OsStr::new(r"C:\tmp\new-dir\"))).unwrap(),
            PathBuf::from(r"C:\tmp\new-dir").join(FILE_NAME),
            "以分隔符结尾按目录处理"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn json_matches_the_contract_and_uses_canvas_coordinates() {
        let snap = snapshot();
        assert_eq!(snap.scale, 1.25);
        assert_eq!(snap.canvas, [10.0, 20.0, 960.0, 520.0]);
        let text = render(&snap);
        assert!(!text.starts_with('\u{feff}'), "不能带 BOM");
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema"], 1);
        assert_eq!(value["t"], 12.346);
        assert_eq!(value["scale"], 1.25);
        assert_eq!(value["canvas"]["w"], 960.0);
        assert_eq!(value["canvas"]["h"], 520.0);
        assert_eq!(value["state"]["panel"], true);
        assert_eq!(value["state"]["notice"], "agent");
        assert_eq!(value["state"]["card"]["kind"], "question");
        assert_eq!(value["state"]["card"]["id"], "e2e-session-1");
        assert_eq!(value["state"]["card"]["count"], 2);
        assert_eq!(value["state"]["result"], Value::Null);
        let nodes = value["nodes"].as_array().unwrap();
        let island = nodes.iter().find(|n| n["id"] == "island.bar").unwrap();
        assert_eq!(
            island["rect"].as_array().unwrap(),
            &vec![json!(0.0), json!(0.0), json!(640.0), json!(44.0)][..],
            "节点必须换算成 canvas 逻辑坐标（减去画布原点）"
        );
        assert!(island.get("label").is_none(), "空 label 不写字段");
        let option = nodes.iter().find(|n| n["id"] == "option.0").unwrap();
        assert_eq!(option["label"], "A");
        assert_eq!(option["enabled"], false);
        assert_eq!(option["rect"], json!([20.0, 10.5, 383.0, 30.0]), "节点坐标同样要换算成 canvas 逻辑坐标");
    }

    #[test]
    fn atomic_write_leaves_no_tmp_and_can_overwrite() {
        let dir = std::env::temp_dir().join(format!("daozai-ui-map-atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("ui-map.json");
        write_atomic(&path, b"{\"schema\":1}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"schema\":1}");
        assert!(!dir.join("ui-map.json.tmp").exists(), "临时文件必须被 rename 掉");
        write_atomic(&path, b"{\"schema\":2}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"schema\":2}", "rename 覆盖已存在的目标");
        let _ = std::fs::remove_dir_all(dir);
    }
}
