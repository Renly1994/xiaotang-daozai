//! `DAOZAI_UI_MAP`（UI Map 调试导出）的自测。
//!
//! 契约：`docs/superpowers/research/2026-11-20-agent-ui-map-contract.md`（schema = 1）。
//!
//! 这里不启动真实窗口：直接构造 `egui::Context` + `Card` 渲染审批 / 提问 / 收起提示卡，
//! 让 `draw_*` 真正跑一遍（节点登记与 footer 按钮都在绘制路径里发生），
//! 再读导出的 JSON 断言「有卡片 + 关键按钮 + 矩形在画布内 + state 与界面一致」。

use crate::agent_notice::Provider;
use crate::agent_ui::{
    draw_approval, draw_banner, draw_question, Action, Answer, ApprovalView, BannerView, Choice, Head, Measurer, Question, QuestionView, State,
};
use crate::app::{App, Env, Options};
use crate::paint::Card;
use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// 与 `ui_tests::ENV_LOCK` 同理：UI Map 的测试目标与内部状态是进程级/线程级共享的。
static MAP_LOCK: Mutex<()> = Mutex::new(());
static SEQ: AtomicUsize = AtomicUsize::new(0);

const CANVAS: Vec2 = Vec2::new(640.0, 520.0);
const PPP: f32 = 1.25;

fn temp_target(tag: &str) -> (PathBuf, PathBuf) {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("daozai-ui-map-{}-{seq}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ui-map.json");
    (dir, path)
}

fn read_map(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("UI Map 没有落盘：{}（{e}）", path.display()));
    let value: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("UI Map 不是合法 JSON：{e}\n{text}"));
    assert_eq!(value["schema"], 1, "schema 必须固定为 1");
    value
}

fn read_map_opt(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 把一次导出复制到 `TEMP/daozai-ui-map-samples/`，供人工/报告取证（失败不影响测试）。
fn save_sample(path: &Path, name: &str) {
    let dir = std::env::temp_dir().join("daozai-ui-map-samples");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::copy(path, dir.join(name));
}

fn node<'a>(map: &'a Value, id: &str) -> &'a Value {
    map["nodes"]
        .as_array()
        .and_then(|nodes| nodes.iter().find(|n| n["id"] == id))
        .unwrap_or_else(|| panic!("缺少节点 {id}：{}", map["nodes"]))
}

fn node_ids(map: &Value) -> Vec<String> {
    map["nodes"].as_array().unwrap().iter().filter_map(|n| n["id"].as_str().map(str::to_owned)).collect()
}

fn rect_of(n: &Value) -> (f64, f64, f64, f64) {
    let r = &n["rect"];
    (r[0].as_f64().unwrap(), r[1].as_f64().unwrap(), r[2].as_f64().unwrap(), r[3].as_f64().unwrap())
}

/// 断言所有节点都落在 canvas 逻辑坐标内，并且矩形非空。
///
/// 允许顶部/左边缘溢出 `ENTRANCE_SLACK`：卡片入场动画有 6pt 上移（`shifted(0,-6)`），
/// 那几帧的矩形确实会略高于画布上沿；横向与底边必须严格在画布内。
fn assert_inside_canvas(map: &Value) {
    const ENTRANCE_SLACK: f64 = 8.0;
    let cw = map["canvas"]["w"].as_f64().unwrap();
    let ch = map["canvas"]["h"].as_f64().unwrap();
    assert!(cw > 0.0 && ch > 0.0, "canvas 尺寸必须有效：{cw}x{ch}");
    for n in map["nodes"].as_array().unwrap() {
        let (x, y, w, h) = rect_of(n);
        assert!(w > 0.0 && h > 0.0, "节点 {} 的矩形必须非空：{:?}", n["id"], n["rect"]);
        assert!(
            x >= -0.5 && x + w <= cw + 0.5 && y >= -ENTRANCE_SLACK && y + h <= ch + 0.5,
            "节点 {} 超出 canvas {cw}x{ch}：{:?}",
            n["id"],
            n["rect"]
        );
    }
}

fn head() -> Head<'static> {
    Head { provider: Provider::Dsh, kicker: "等待你审批", subtitle: "C:\\work\\api-gateway", jump: None }
}

fn run_pass(ctx: &egui::Context, t: f64, mut draw: impl FnMut(&mut egui::Ui)) {
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, CANVAS)),
        time: Some(t),
        focused: true,
        viewports: std::iter::once((
            egui::ViewportId::ROOT,
            egui::ViewportInfo { native_pixels_per_point: Some(PPP), ..Default::default() },
        ))
        .collect(),
        ..Default::default()
    };
    let _ = ctx.run(raw, |ctx| {
        egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx, |ui| {
            ui.painter().rect_filled(ui.max_rect(), 0.0, Color32::from_rgb(16, 17, 19));
            draw(ui);
        });
    });
}

/// 建立带字体的上下文，并先空跑一帧让字体表就绪。
fn new_ctx() -> egui::Context {
    let ctx = egui::Context::default();
    crate::theme::load_fonts(&ctx);
    run_pass(&ctx, 0.0, |_| {});
    ctx
}

fn approval<'a>(tool: &'a str, operation: &'a str, reason: &'a str, state: State) -> ApprovalView<'a> {
    ApprovalView {
        head: head(),
        tool,
        reason,
        cwd: "C:\\work\\api-gateway",
        operation,
        native_scope: false,
        queue: 1,
        handoff: None,
        state,
        error: None,
        age: 1.0,
    }
}

/// 在 card 坐标系里画一张审批卡（与 `Cards::draw_details` 相同的入口）。
fn pass_approval(ctx: &egui::Context, t: f64, view: &ApprovalView) {
    run_pass(ctx, t, |ui| {
        let card = Card::new(ui, ui.max_rect().min, ui.max_rect());
        let m = Measurer::new(ui.ctx());
        let _ = draw_approval(&card, &m, view, 0.0, t);
    });
}

#[test]
fn approval_card_exports_nodes_state_and_rects_inside_the_canvas() {
    let _g = MAP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (dir, path) = temp_target("approval");
    crate::ui_map::set_test_target(Some(path.clone()));

    let ctx = new_ctx();
    let view = approval("write", "{\"file_path\":\"config/gateway.yaml\"}", "发布前确认写入生产配置", State::Pending);
    // 第 1 帧安装 per-pass 回调（当帧还不结算），第 2 帧起真正导出。
    for i in 0..3 {
        pass_approval(&ctx, 1.0 + f64::from(i), &view);
    }

    let map = read_map(&path);
    assert_eq!(map["state"]["card"]["kind"], "approval");
    assert_eq!(map["state"]["card"]["details"], true);
    assert_eq!(map["state"]["card"]["submitting"], false);
    assert_eq!(map["state"]["card"]["error"], Value::Null);
    assert_eq!(map["state"]["queue"], 1);
    assert_eq!(map["state"]["panel"], true);
    assert_eq!(map["state"]["notice"], "agent");
    assert_eq!(map["state"]["result"], Value::Null);
    assert_eq!(map["scale"], PPP as f64, "scale 必须取 ctx.pixels_per_point()");
    assert_eq!(map["canvas"]["w"], CANVAS.x as f64);
    assert_eq!(map["canvas"]["h"], CANVAS.y as f64);

    for id in ["island.bar", "card", "chip.collapse", "btn.return", "btn.deny", "btn.allow", "body.operation"] {
        node(&map, id);
    }
    let allow = node(&map, "btn.allow");
    assert_eq!(allow["label"], "仅允许这一次");
    assert!(allow.get("enabled").is_none(), "available 按钮不写 enabled 字段");
    assert_eq!(node(&map, "btn.deny")["label"], "拒绝");
    assert_eq!(node(&map, "btn.return")["label"], "回客户端");
    assert_inside_canvas(&map);
    assert!(!path.with_extension("json.tmp").exists(), "不能残留 .tmp 文件");

    // 卡片外框必须覆盖 footer 按钮（否则点击坐标会落在卡外）。
    let (cx, cy, cw, ch) = rect_of(node(&map, "card"));
    for id in ["btn.allow", "btn.deny", "btn.return"] {
        let (bx, by, bw, bh) = rect_of(node(&map, id));
        assert!(bx >= cx - 0.5 && by >= cy - 0.5 && bx + bw <= cx + cw + 0.5 && by + bh <= cy + ch + 0.5, "{id} 必须落在 card 内");
    }

    crate::ui_map::clear_test_target();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn submitting_approval_exports_disabled_controls() {
    let _g = MAP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (dir, path) = temp_target("approval-submitting");
    crate::ui_map::set_test_target(Some(path.clone()));

    let ctx = new_ctx();
    let view = approval("write", "{\"file_path\":\"a.txt\"}", "确认", State::Submitting);
    for i in 0..3 {
        pass_approval(&ctx, 1.0 + f64::from(i), &view);
    }
    let map = read_map(&path);
    assert_eq!(map["state"]["card"]["submitting"], true, "提交中要能被测试脚本看见");
    assert_eq!(node(&map, "btn.allow")["enabled"], false, "提交中不能再点允许");
    assert_eq!(node(&map, "btn.deny")["enabled"], false, "提交中不能再点拒绝");
    assert!(node(&map, "btn.return").get("enabled").is_none(), "「回客户端」必须始终可点");

    crate::ui_map::clear_test_target();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn question_card_exports_options_custom_input_and_submit() {
    let _g = MAP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (dir, path) = temp_target("question");
    crate::ui_map::set_test_target(Some(path.clone()));

    let ctx = new_ctx();
    let questions = vec![Question {
        id: "q1".into(),
        question: "发布前用哪种验证策略？".into(),
        detail: "选择后本轮会按该策略继续执行。".into(),
        multi: false,
        options: vec![
            Choice { label: "仅跑冒烟测试".into(), description: String::new() },
            Choice { label: "跑完整回归".into(), description: String::new() },
            Choice { label: "先不验证".into(), description: String::new() },
        ],
    }];
    let mut answers = vec![Answer { id: "q1".into(), ..Default::default() }];
    for i in 0..3 {
        let t = 1.0 + f64::from(i);
        let mut view = QuestionView {
            head: head(),
            questions: &questions,
            answers: &mut answers,
            index: 0,
            scroll: 0.0,
            reviewing: false,
            queue: 2,
            handoff: None,
            state: State::Pending,
            error: None,
            age: 1.0,
        };
        run_pass(&ctx, t, |ui| {
            let card = Card::new(ui, ui.max_rect().min, ui.max_rect());
            let m = Measurer::new(ui.ctx());
            let _ = draw_question(&card, &m, &mut view, 0.0, t);
        });
    }

    let map = read_map(&path);
    assert_eq!(map["state"]["card"]["kind"], "question");
    assert_eq!(map["state"]["card"]["index"], 0);
    assert_eq!(map["state"]["card"]["count"], 1);
    assert_eq!(map["state"]["queue"], 2);
    for id in ["card", "option.0", "option.1", "option.2", "input.custom", "btn.return", "btn.submit"] {
        node(&map, id);
    }
    assert_eq!(node(&map, "option.0")["label"], "仅跑冒烟测试");
    assert_eq!(node(&map, "btn.submit")["label"], "检查答案", "单题提问的提交确认按钮是「检查答案」");
    assert_inside_canvas(&map);
    save_sample(&path, "question.json");

    crate::ui_map::clear_test_target();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn banner_card_exports_view_and_return_buttons() {
    let _g = MAP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (dir, path) = temp_target("banner");
    crate::ui_map::set_test_target(Some(path.clone()));

    let ctx = new_ctx();
    let view = BannerView {
        head: head(),
        summary: "write · config/gateway.yaml",
        handoff: Some(12),
        queue: 3,
        approval: true,
        age: 1.0,
    };
    for i in 0..3 {
        let t = 1.0 + f64::from(i);
        run_pass(&ctx, t, |ui| {
            let card = Card::new(ui, ui.max_rect().min, ui.max_rect());
            let m = Measurer::new(ui.ctx());
            let _ = draw_banner(&card, &m, &view, t);
        });
    }

    let map = read_map(&path);
    assert_eq!(map["state"]["card"]["kind"], "approval");
    assert_eq!(map["state"]["card"]["details"], false, "收起提示卡不是详情态");
    assert_eq!(map["state"]["panel"], false);
    assert_eq!(map["state"]["queue"], 3);
    for id in ["card", "btn.return", "btn.view"] {
        node(&map, id);
    }
    assert_eq!(node(&map, "btn.view")["label"], "查看并审批");
    assert!(!node_ids(&map).contains(&"btn.submit".to_owned()), "提示卡的查看按钮不能伪装成提交");
    assert_inside_canvas(&map);
    save_sample(&path, "banner.json");

    crate::ui_map::clear_test_target();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn closing_the_card_clears_the_map_within_a_frame() {
    let _g = MAP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (dir, path) = temp_target("close");
    crate::ui_map::set_test_target(Some(path.clone()));

    let ctx = new_ctx();
    let view = approval("write", "{}", "", State::Pending);
    for i in 0..3 {
        pass_approval(&ctx, 1.0 + f64::from(i), &view);
    }
    assert_eq!(read_map(&path)["state"]["card"]["kind"], "approval");

    // 卡片关闭：后续帧没有任何卡片绘制，地图必须在下一帧变成「没有卡片 + 没有节点」。
    for i in 0..2 {
        run_pass(&ctx, 10.0 + f64::from(i), |_| {});
    }
    let map = read_map(&path);
    assert_eq!(map["state"]["card"], Value::Null, "卡片关闭后不能停在旧快照");
    assert!(map["nodes"].as_array().unwrap().is_empty(), "卡片关闭后不应残留可点节点");
    assert_eq!(map["state"]["notice"], "none");

    crate::ui_map::clear_test_target();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn writes_are_throttled_but_state_changes_are_written_immediately() {
    let _g = MAP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (dir, path) = temp_target("throttle");
    crate::ui_map::set_test_target(Some(path.clone()));

    let ctx = new_ctx();
    let settled = approval("write", "{}", "", State::Pending);
    for i in 0..3 {
        pass_approval(&ctx, 1.0 + f64::from(i), &settled);
    }
    let written = crate::ui_map::write_count();
    assert!(written >= 1, "首先必须成功落盘");

    // 只改入场动画（矩形变化、状态不变）：50ms 内不允许再写。
    let mut animating = approval("write", "{}", "", State::Pending);
    animating.age = 0.08;
    pass_approval(&ctx, 1.5, &animating);
    assert_eq!(crate::ui_map::write_count(), written, "50ms 内纯位置变化必须被节流");

    // 状态变化：不受节流限制，立即写。
    let submitting = approval("write", "{}", "", State::Submitting);
    pass_approval(&ctx, 1.6, &submitting);
    assert_eq!(crate::ui_map::write_count(), written + 1, "状态变化必须立即落盘");
    assert_eq!(read_map(&path)["state"]["card"]["submitting"], true);

    crate::ui_map::clear_test_target();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn identical_content_is_not_rewritten() {
    let _g = MAP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (dir, path) = temp_target("idempotent");
    crate::ui_map::set_test_target(Some(path.clone()));

    let ctx = new_ctx();
    let view = approval("write", "{}", "", State::Pending);
    for i in 0..3 {
        pass_approval(&ctx, 1.0 + f64::from(i), &view);
    }
    let written = crate::ui_map::write_count();
    // 等过节流窗口，再画一帧完全一样的内容：内容没变就不该再 rename。
    std::thread::sleep(std::time::Duration::from_millis(80));
    let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    pass_approval(&ctx, 2.0, &view);
    assert_eq!(crate::ui_map::write_count(), written, "内容没变不应重复落盘");
    let after = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    assert_eq!(stamp, after, "文件不应被无意义地重写");

    crate::ui_map::clear_test_target();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn nothing_is_written_when_the_switch_is_off() {
    let _g = MAP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (dir, path) = temp_target("off");
    crate::ui_map::set_test_target(None);
    assert!(!crate::ui_map::enabled(), "没有目标时必须关闭");

    let ctx = new_ctx();
    let view = approval("write", "{}", "", State::Pending);
    for i in 0..4 {
        pass_approval(&ctx, 1.0 + f64::from(i), &view);
    }
    assert!(!path.exists(), "关闭时绝不能落盘：{}", path.display());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "关闭时目录里不应出现任何文件");
    assert_eq!(crate::ui_map::write_count(), 0);

    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------
// 从「地图」到「真点」：用导出的矩形中心发真实的鼠标事件
// ---------------------------------------------------------------

/// 地图里的节点中心（测试画布原点恒为 (0,0)，所以 canvas 坐标就是屏幕坐标）。
fn center_of(map: &Value, id: &str) -> Pos2 {
    let (x, y, w, h) = rect_of(node(map, id));
    Pos2::new((x + w / 2.0) as f32, (y + h / 2.0) as f32)
}

fn run_approval_pass(ctx: &egui::Context, t: f64, view: &ApprovalView, events: Vec<egui::Event>, action: &mut Option<Action>) {
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, CANVAS)),
        time: Some(t),
        focused: true,
        events,
        viewports: std::iter::once((
            egui::ViewportId::ROOT,
            egui::ViewportInfo { native_pixels_per_point: Some(PPP), ..Default::default() },
        ))
        .collect(),
        ..Default::default()
    };
    let _ = ctx.run(raw, |ctx| {
        egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx, |ui| {
            let card = Card::new(ui, ui.max_rect().min, ui.max_rect());
            let m = Measurer::new(ui.ctx());
            let (got, _) = draw_approval(&card, &m, view, 0.0, t);
            if got.is_some() {
                *action = got;
            }
        });
    });
}

#[test]
fn clicking_the_mapped_allow_rect_really_produces_allow() {
    // 契约验收 2：`rect` 与「把 label 点下去后实际发生的动作」一致。
    let _g = MAP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (dir, path) = temp_target("click-allow");
    crate::ui_map::set_test_target(Some(path.clone()));

    let ctx = new_ctx();
    let view = approval("write", "{\"file_path\":\"config/gateway.yaml\"}", "确认写入", State::Pending);
    let mut action = None;
    for i in 0..3 {
        run_approval_pass(&ctx, 1.0 + f64::from(i), &view, vec![], &mut action);
    }
    let map = read_map(&path);
    let target = center_of(&map, "btn.allow");
    let (_, _, bw, bh) = rect_of(node(&map, "btn.allow"));
    assert!(bw > 40.0 && bh > 20.0, "按钮矩形必须是有面积的可点区域");

    // 鼠标移到地图给出的中心 → 按下 → 抬起。
    let mut t = 2.0;
    run_approval_pass(&ctx, t, &view, vec![egui::Event::PointerMoved(target)], &mut action);
    t += 0.02;
    run_approval_pass(
        &ctx,
        t,
        &view,
        vec![egui::Event::PointerButton { pos: target, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }],
        &mut action,
    );
    t += 0.02;
    run_approval_pass(
        &ctx,
        t,
        &view,
        vec![egui::Event::PointerButton { pos: target, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }],
        &mut action,
    );
    assert_eq!(action, Some(Action::Allow), "按地图坐标点击 btn.allow 必须真的触发「仅允许这一次」");

    crate::ui_map::clear_test_target();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn clicking_the_mapped_option_rect_selects_that_option() {
    let _g = MAP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (dir, path) = temp_target("click-option");
    crate::ui_map::set_test_target(Some(path.clone()));

    let ctx = new_ctx();
    let questions = vec![Question {
        id: "q1".into(),
        question: "发布前用哪种验证策略？".into(),
        detail: String::new(),
        multi: false,
        options: vec![
            Choice { label: "仅跑冒烟测试".into(), description: String::new() },
            Choice { label: "跑完整回归".into(), description: String::new() },
        ],
    }];
    let mut answers = vec![Answer { id: "q1".into(), ..Default::default() }];
    let mut pass = |events: Vec<egui::Event>, t: f64, answers: &mut Vec<Answer>| {
        let mut view = QuestionView {
            head: head(), questions: &questions, answers, index: 0, scroll: 0.0, reviewing: false,
            queue: 1, handoff: None, state: State::Pending, error: None, age: 1.0,
        };
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, CANVAS)),
            time: Some(t), focused: true, events,
            viewports: std::iter::once((egui::ViewportId::ROOT, egui::ViewportInfo { native_pixels_per_point: Some(PPP), ..Default::default() })).collect(),
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx, |ui| {
                let card = Card::new(ui, ui.max_rect().min, ui.max_rect());
                let m = Measurer::new(ui.ctx());
                let _ = draw_question(&card, &m, &mut view, 0.0, t);
            });
        });
    };
    for i in 0..3 {
        pass(vec![], 1.0 + f64::from(i), &mut answers);
    }
    let map = read_map(&path);
    let target = center_of(&map, "option.1");
    pass(vec![egui::Event::PointerMoved(target)], 2.0, &mut answers);
    pass(vec![egui::Event::PointerButton { pos: target, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 2.02, &mut answers);
    pass(vec![egui::Event::PointerButton { pos: target, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 2.04, &mut answers);
    assert_eq!(answers[0].selected, vec!["跑完整回归".to_owned()], "按地图坐标点击 option.1 必须选中第二个选项");

    crate::ui_map::clear_test_target();
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------
// 真实 App 帧循环（与 `main.rs` 同一路径）：验证画布/几何/节点与真实绘制一致
// ---------------------------------------------------------------

fn app_frame(app: &mut App, ctx: &egui::Context, t: &mut f64, cursor: Option<Pos2>) -> Vec<egui::epaint::ClippedShape> {
    *t += 1.0 / 60.0;
    let finished = app.agent_cards.update(&app.settings, ctx);
    for (provider, text) in finished {
        app.agent_finished(provider, text);
    }
    let env = Env {
        t: *t,
        clock: "09:41:00".into(),
        today: "2026-11-20".into(),
        date: "11月20日 周五".into(),
        cursor,
        bt_ready: true,
        ..Default::default()
    };
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(crate::app::CANVAS_W, crate::app::WIN_H))),
        time: Some(*t),
        focused: true,
        viewports: std::iter::once((
            egui::ViewportId::ROOT,
            egui::ViewportInfo { native_pixels_per_point: Some(1.0), ..Default::default() },
        ))
        .collect(),
        ..Default::default()
    };
    let output = ctx.run(raw, |ctx| {
        egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx, |ui| {
            let win = ui.max_rect();
            app.frame(ui, win, &env);
        });
    });
    output.shapes
}

#[test]
fn app_frame_exports_approval_card_matching_the_real_rendered_buttons() {
    let _g = MAP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (dir, path) = temp_target("app-approval");
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::env::set_var("DAOZAI_DATA_DIR", &data_dir);
    crate::ui_map::set_test_target(Some(path.clone()));

    let ctx = egui::Context::default();
    let mut app = App::new(&ctx, Options { fetch_lyrics: false, pet_idle: 1e9 });
    app.settings.dsh_notifications = true;
    app.settings.agent.interactive = true;
    app.agent_cards.receive(serde_json::json!({"id":"e2e-approval","provider":"dsh","kind":"approval",
        "data":{"tool":"write","reason":"发布前确认写入生产配置","cwd":"C:\\work\\api-gateway",
                "operation":"{\"file_path\":\"config/gateway.yaml\"}"}}), &app.settings);

    let mut t = 0.0;
    let mut shapes = vec![];
    for _ in 0..90 {
        shapes = app_frame(&mut app, &ctx, &mut t, None);
    }
    assert!(app.agent_detail_active(), "审批卡应自动展开，否则本用例前提不成立");

    // 等入场动画结束（卡片矩形回到画布上沿以内），避免拿动画中间帧对照文字位置。
    let mut map = read_map(&path);
    for _ in 0..240 {
        let (_, y, _, _) = rect_of(node(&map, "card"));
        if y >= -0.01 {
            break;
        }
        shapes = app_frame(&mut app, &ctx, &mut t, None);
        if let Some(next) = read_map_opt(&path) {
            map = next;
        }
    }
    assert!(rect_of(node(&map, "card")).1 >= -0.01, "卡片入场动画尚未结束：{:?}", node(&map, "card")["rect"]);
    assert_eq!(map["state"]["card"]["kind"], "approval");
    assert_eq!(map["state"]["panel"], true);
    assert_eq!(map["state"]["queue"], 1);
    assert_eq!(map["canvas"]["w"], crate::app::CANVAS_W as f64, "画布宽度必须是真实窗口宽度");
    assert_eq!(map["canvas"]["h"], crate::app::WIN_H as f64);
    assert_eq!(map["scale"], 1.0);
    for id in ["island.bar", "card", "chip.collapse", "btn.return", "btn.deny", "btn.allow", "body.operation"] {
        node(&map, id);
    }
    assert_inside_canvas(&map);

    // 地图上的按钮矩形必须真的盖住屏幕上画出来的按钮文字。
    let find_text = |label: &str| {
        shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == label => Some(t.visual_bounding_rect().center()),
            _ => None,
        })
    };
    for (label, id) in [("仅允许这一次", "btn.allow"), ("拒绝", "btn.deny"), ("回客户端", "btn.return")] {
        let center = find_text(label).unwrap_or_else(|| panic!("画面上找不到按钮文字：{label}"));
        let (x, y, w, h) = rect_of(node(&map, id));
        let inside = center.x >= x as f32 && center.y >= y as f32 && center.x <= (x + w) as f32 && center.y <= (y + h) as f32;
        assert!(inside, "地图节点 {id} 的矩形没有盖住按钮文字 {label}（文字中心 {center:?}）：{:?}", node(&map, id)["rect"]);
    }

    save_sample(&path, "approval-app.json");

    crate::ui_map::clear_test_target();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn app_frame_registers_done_card_and_result_layer_buttons() {
    let _g = MAP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (dir, path) = temp_target("app-done");
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::env::set_var("DAOZAI_DATA_DIR", &data_dir);
    crate::ui_map::set_test_target(Some(path.clone()));

    let ctx = egui::Context::default();
    let mut app = App::new(&ctx, Options { fetch_lyrics: false, pet_idle: 1e9 });
    app.settings.dsh_notifications = true;
    app.settings.agent.content = 2;
    app.settings.agent.expanded = true;
    app.settings.agent.dwell = 15;
    app.agent_finished(Provider::Dsh, "## 本轮完成\n- 更新网关超时与重试\n\n全部通过。".into());
    let mut t = 0.0;
    for _ in 0..70 {
        app_frame(&mut app, &ctx, &mut t, None);
    }
    // 完成卡：footer 按钮已经通过 `metrics::button` 自动登记（card/layer 外框待 done.rs 补）。
    let map = read_map(&path);
    for id in ["btn.view", "btn.close"] {
        node(&map, id);
    }
    assert_eq!(node(&map, "btn.view")["label"], "查看回复");
    assert_eq!(node(&map, "btn.close")["label"], "关闭");
    assert_inside_canvas(&map);
    save_sample(&path, "done-card-buttons.json");

    // 结果层：同样应先有 footer 按钮（layer.result/body.result 待 done.rs 补）。
    app.agent_cards.open_result(Provider::Dsh, "## 本轮完成\n- 更新连接池上限\n\n```bash\ncargo test\n```\n全部通过。".into());
    app.open_agent_panel();
    for _ in 0..70 {
        app_frame(&mut app, &ctx, &mut t, None);
    }
    let map = read_map(&path);
    for id in ["btn.copy", "btn.close"] {
        node(&map, id);
    }
    assert_eq!(node(&map, "btn.copy")["label"], "复制内容");
    assert_inside_canvas(&map);
    save_sample(&path, "result-layer-buttons.json");

    crate::ui_map::clear_test_target();
    let _ = std::fs::remove_dir_all(dir);
}
