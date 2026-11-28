use super::*;
use crate::plugin_status::StatusHealth;
use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

fn install(h: &mut H, profile: &Path, id: &str, name: &str, data: &Value) {
    let source = profile.join(id).join("status.json");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, data.to_string()).unwrap();
    install_with_source(h, profile, id, name, &source, None);
}

fn install_with_source(
    h: &mut H,
    profile: &Path,
    id: &str,
    name: &str,
    source: &Path,
    layout: Option<Value>,
) {
    let folder = profile.join(id);
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join("cover.png"),
        include_bytes!("../spike/examples/payday/cover.png"),
    )
    .unwrap();
    fs::write(
        folder.join("main.js"),
        r#"
function refresh({source}) {
    return {status: source.status === "ready" ? JSON.parse(source.text) : null};
}
"#,
    )
    .unwrap();
    let manifest = folder.join("plugin.json");
    let mut definition = json!({
            "id":id, "name":name, "author":"验收", "version":"1.0.0", "api":1,
            "runtime":"js", "entry":"main.js", "cover":"cover.png", "owner":"status",
            "capabilities":["status.page","timer"], "dataVersion":1,
            "permissions":{"source.read":"file"}, "settings":[]
    });
    if let Some(layout) = layout {
        definition["statusLayout"] = layout;
    }
    fs::write(&manifest, definition.to_string()).unwrap();
    h.app.plugins.install(&manifest).unwrap();
    let installed = profile
        .join("local-plugins")
        .join(format!("local-{id}.json"));
    let mut record: Value = serde_json::from_slice(&fs::read(&installed).unwrap()).unwrap();
    record["source_grant"] = json!({"File":source});
    fs::write(installed, record.to_string()).unwrap();
}

fn text_y(h: &H, label: &str) -> f32 {
    h.shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(text.visual_bounding_rect().center().y)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("未绘制：{label}"))
}

fn render_settings(h: &mut H, size: egui::Vec2, events: Vec<egui::Event>) -> egui::FullOutput {
    h.ctx.set_embed_viewports(false);
    h.t += 1.0 / 60.0;
    let time = Some(h.t);
    let output = std::sync::Arc::new(Mutex::new(None));
    let capture = output.clone();
    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
        let id = viewport.ids.this;
        let raw = egui::RawInput {
            viewport_id: id,
            time,
            events: events.clone(),
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            viewports: [(id, egui::ViewportInfo { native_pixels_per_point: Some(1.0), ..Default::default() })].into_iter().collect(),
            ..Default::default()
        };
        *capture.lock().unwrap() = Some(ctx.run(raw, |ctx| (viewport.viewport_ui_cb)(ctx)));
    });
    let _ = h.ctx.run(egui::RawInput { time, ..Default::default() }, |ctx| h.app.settings_ui(ctx, false));
    let result = output.lock().unwrap().take().unwrap();
    result
}

#[test]
fn plugin_settings_pages_gear_routes_by_id_and_sidebar_fits() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut h = H::new();
    let profile = crate::model::config_dir();
    let standalone = [
        ("test.settings-alpha", "便签测试"),
        ("test.settings-beta", "金价测试"),
        ("test.settings-gamma", "工资测试"),
        ("test.settings-delta", "时钟测试"),
    ];
    let task = ("test.settings-days", "日子测试");
    for (id, name) in standalone.into_iter().chain([task]) {
        let dir = profile.join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("cover.png"), include_bytes!("../spike/examples/payday/cover.png")).unwrap();
        let mut capabilities = vec!["status.page", "settings"];
        if id == standalone[1].0 { capabilities.push("island.pinned"); }
        let owner = if id == task.0 { capabilities.push("card"); "tasks" } else { "status" };
        let mut output = json!({"status":{"state":"idle","label":name}});
        if id == task.0 { output["card"] = json!(name); }
        fs::write(dir.join("main.js"), format!("function refresh() {{ return {output}; }}")).unwrap();
        let manifest = dir.join("plugin.json");
        fs::write(&manifest, json!({
            "id":id, "name":name, "author":"验收", "version":"1.0.0", "api":1,
            "runtime":"js", "entry":"main.js", "cover":"cover.png", "owner":owner,
            "capabilities":capabilities, "dataVersion":1, "permissions":{},
            "settings":[{"key":"text","type":"text","label":format!("{name}专用参数"),"default":"值"}]
        }).to_string()).unwrap();
        h.app.plugins.install(&manifest).unwrap();
        if id != standalone[0].0 {
            h.app.plugins.enable(id, true).unwrap();
        }
    }
    h.app.set_page_visible(crate::app::TODO_PAGE, true);
    let open_from_island = |h: &mut H, id: &str| {
        h.app.settings_open = false;
        h.app.show_status_page(id);
        let gear = H::local(394.0, 18.0);
        h.hover(gear, 0.8);
        h.click(gear);
        assert!(h.app.settings_open);
    };
    open_from_island(&mut h, standalone[1].0);
    assert_eq!(h.app.settings_plugin.as_deref(), Some(standalone[1].0));
    assert_eq!(h.app.settings_tab, crate::extras::SETTINGS_TAB_PLUGIN_DETAIL);

    let render_settings = |h: &mut H, size| {
        let _ = render_settings(h, size, vec![
            egui::Event::PointerMoved(egui::pos2(80.0, 220.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -800.0),
                modifiers: Default::default(),
            },
        ]);
        // 插件可以排在侧栏末尾，窄窗口中应能滚动到所有入口。
        for _ in 0..12 {
            let _ = render_settings(h, size, vec![]);
        }
        render_settings(h, size, vec![])
    };
    let visible_rows = |output: &egui::FullOutput, label: &str, sidebar: bool| {
        let mut rows: Vec<_> = output.shapes.iter().filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => {
                let rect = text.visual_bounding_rect();
                (text.galley.text() == label && (rect.left() < 180.0) == sidebar
                    && clipped.clip_rect.contains_rect(rect)).then_some(rect.center().y)
            }
            _ => None,
        }).collect();
        rows.sort_by(f32::total_cmp);
        rows.dedup_by(|left, right| (*left - *right).abs() < 3.0);
        rows.len()
    };
    let visible = |output: &egui::FullOutput, label: &str, sidebar: bool| visible_rows(output, label, sidebar) > 0;
    let (id, name) = standalone[0];
    let size = egui::vec2(690.0, 560.0);
    let installed = render_settings(&mut h, size);
    assert!(!visible(&installed, name, true));
    let other_page = h.app.page;
    h.app.show_status_page(id);
    assert_eq!(h.app.page, other_page);
    h.app.plugins.enable(id, true).unwrap();

    for size in [egui::vec2(690.0, 560.0), egui::vec2(600.0, 450.0)] {
        let output = render_settings(&mut h, size);
        for (_, name) in standalone { assert!(visible(&output, name, true), "侧栏未完整显示 {name}"); }
        assert!(visible(&output, "插件", true));
        assert_eq!(visible_rows(&output, "插件", false), 1, "独立详情只在标题标记插件来源");
        assert!(visible(&output, "关于", true));
        assert!(visible(&output, "打开功能页", false));
        assert!(visible(&output, "启用金价测试", false));
        assert!(visible(&output, "金价测试专用参数", false));
        assert!(visible(&output, "在岛上常驻显示", false));
        assert!(visible(&output, "保存设置", false));
        assert!(!visible(&output, "便签测试专用参数", false));
    }
    h.app.plugins.set_status_page_enabled(standalone[1].0, false).unwrap();
    let output = render_settings(&mut h, size);
    assert!(!visible(&output, "打开功能页", false));
    assert!(visible(&output, standalone[1].1, true));
    assert!(visible(&output, "金价测试专用参数", false));

    h.app.plugins.set_status_page_display(standalone[1].0, false, true).unwrap();
    h.app.set_page_settings_hidden(crate::app::MUSIC_PAGE, true);
    let hidden = render_settings(&mut h, size);
    assert!(!visible(&hidden, standalone[1].1, true));
    assert!(visible(&hidden, name, true), "隐藏音乐后，其他插件入口仍应显示");
    assert!(visible(&hidden, "金价测试专用参数", false), "从插件管理仍可直达插件设置");
    h.app.plugins.set_status_page_enabled(standalone[1].0, true).unwrap();
    assert!(visible(&render_settings(&mut h, size), standalone[1].1, true));

    h.app.show_status_page(id);
    assert!(h.app.page >= crate::app::PAGE_NAMES.len());
    h.app.open_plugin_settings(id);
    assert_eq!(h.app.settings_plugin.as_deref(), Some(id));
    h.app.plugins.enable(id, false).unwrap();
    let stopped = render_settings(&mut h, size);
    assert!(!visible(&stopped, name, true));
    assert!(h.app.settings_plugin.is_none());
    assert_eq!(h.app.settings_tab, crate::extras::SETTINGS_TAB_PLUGINS);

    h.app.settings_open = false;
    h.frame(vec![]);
    let fallback = h.app.page;
    assert!(fallback < crate::app::PAGE_NAMES.len());
    h.app.show_status_page(id);
    assert_eq!(h.app.page, fallback);
    h.app.open_plugin_settings(id);
    assert!(h.app.settings_plugin.is_none());
    assert_eq!(h.app.settings_tab, crate::extras::SETTINGS_TAB_PLUGINS);

    open_from_island(&mut h, task.0);
    assert!(h.app.settings_plugin.is_none());
    assert_eq!(h.app.settings_tab, crate::app::PAGE_SETTINGS_TABS[crate::app::TODO_PAGE]);
}

#[test]
fn plugin_page_order_drag_persists_across_restart_and_visibility() {
    use crate::app::{FOCUS_PAGE, MUSIC_PAGE, PAGE_NAMES};

    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut h = H::new();
    let profile = crate::model::config_dir();
    let (id, name) = ("test.page-order", "排序插件");
    install(&mut h, &profile, id, name, &json!({"state":"idle","label":name}));
    h.app.plugins = crate::sdk_spike::local::LocalPlugins::open(profile.join("local-plugins"));
    h.app.plugins.enable(id, true).unwrap();
    h.app.settings_tab = crate::extras::SETTINGS_TAB_PAGES;
    h.app.open_settings();

    let size = egui::vec2(690.0, 1700.0);
    for _ in 0..2 {
        render_settings(&mut h, size, vec![]);
    }
    let position = |output: &egui::FullOutput, label: &str| {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => {
                let point = text.visual_bounding_rect().left_center();
                (text.galley.text() == label && point.x > 180.0
                    && shape.clip_rect.contains(point)).then_some(point)
            }
            _ => None,
        }).unwrap_or_else(|| panic!("未显示页面管理条目：{label}"))
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    let sidebar_y = |output: &egui::FullOutput, label: &str| {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => {
                let rect = text.visual_bounding_rect();
                (text.galley.text() == label && rect.right() < 180.0
                    && shape.clip_rect.contains_rect(rect)).then_some(rect.center().y)
            }
            _ => None,
        })
    };
    let assert_sidebar_sorted = |output: &egui::FullOutput| {
        let music = sidebar_y(output, "音乐").expect("左栏应显示音乐");
        let plugin = sidebar_y(output, name).expect("左栏应显示已启用的插件");
        let focus = sidebar_y(output, "专注").expect("左栏应显示专注与健康");
        assert!(music < plugin && plugin < focus, "左栏应与页面管理使用同一顺序");
    };
    let original = render_settings(&mut h, size, vec![]);
    let music = position(&original, "播放控制与同步歌词");
    let focus = position(&original, "番茄钟与休息计时");
    let plugin = position(&original, name);
    assert!(focus.y < plugin.y, "新插件默认排在内置页之后");
    assert!(sidebar_y(&original, "无线设备").unwrap() < sidebar_y(&original, name).unwrap(),
        "新插件的设置入口默认排在内置功能之后");
    let from = egui::pos2(music.x - 18.0, plugin.y + 6.0);
    render_settings(&mut h, size, vec![egui::Event::PointerMoved(from), button(from, true)]);
    render_settings(&mut h, size, vec![egui::Event::PointerMoved(from - egui::vec2(0.0, 10.0))]);
    render_settings(&mut h, size, vec![egui::Event::PointerMoved(focus)]);
    let dragging = render_settings(&mut h, size, vec![]);
    assert!(position(&dragging, name).y > focus.y, "松手前保持原顺序");
    render_settings(&mut h, size, vec![button(focus, false)]);
    let sorted = render_settings(&mut h, size, vec![]);
    let plugin = position(&sorted, name);
    assert!(position(&sorted, "播放控制与同步歌词").y < plugin.y);
    assert!(plugin.y < position(&sorted, "番茄钟与休息计时").y);
    assert_sidebar_sorted(&sorted);

    h.app = App::new(&h.ctx, Options { fetch_lyrics: false, pet_idle: 1e9 });
    let cursor = H::local(225.0, 80.0);
    let next = H::local(398.0, 68.0);
    h.app.show_page(MUSIC_PAGE);
    h.hover(cursor, 0.75);
    h.click(next);
    assert_eq!(h.app.page(), PAGE_NAMES.len(), "重启后仍先切到插件页");
    h.hover(cursor, 0.75);
    h.click(next);
    assert_eq!(h.app.page(), FOCUS_PAGE);

    for (enabled, expected) in [(false, FOCUS_PAGE), (true, PAGE_NAMES.len())] {
        h.app.plugins.set_status_page_enabled(id, enabled).unwrap();
        h.app.show_page(MUSIC_PAGE);
        h.hover(cursor, 0.75);
        h.click(next);
        assert_eq!(h.app.page(), expected, "插件页开关应保留其排序");
    }
    h.app.open_settings();
    assert_sidebar_sorted(&render_settings(&mut h, size, vec![]));
    h.app.plugins.set_status_page_enabled(id, false).unwrap();
    assert!(sidebar_y(&render_settings(&mut h,size,vec![]),name).is_none(),"关闭后台会隐藏入口，但保留排序");
    h.app.plugins.enable(id, false).unwrap();
    assert!(sidebar_y(&render_settings(&mut h, size, vec![]), name).is_none());
    h.app.plugins.enable(id, true).unwrap();
    h.app.plugins.set_status_page_enabled(id, true).unwrap();
    assert_sidebar_sorted(&render_settings(&mut h, size, vec![]));
}

#[test]
fn plugin_page_management_controls_fit_one_row_at_minimum_width() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut h = H::new();
    let profile = crate::model::config_dir();
    let (id, name) = ("test.page-row", "排版");
    install(&mut h, &profile, id, name, &json!({"state":"idle","label":name}));
    h.app.plugins = crate::sdk_spike::local::LocalPlugins::open(profile.join("local-plugins"));
    h.app.plugins.enable(id, true).unwrap();
    h.app.settings.plugin_page_order.push((id.into(), 0));
    h.app.settings_tab = crate::extras::SETTINGS_TAB_PAGES;
    h.app.open_settings();

    let size = egui::vec2(600.0, 900.0);
    render_settings(&mut h, size, vec![]);
    let output = render_settings(&mut h, size, vec![]);
    let text_rect = |labels: &[&str], near_y: Option<f32>| {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => {
                let rect = text.visual_bounding_rect();
                (labels.contains(&text.galley.text()) && rect.left() > 180.0
                    && shape.clip_rect.contains_rect(rect)
                    && near_y.is_none_or(|y| (rect.center().y - y).abs() < 26.0))
                    .then_some(rect)
            }
            _ => None,
        }).unwrap_or_else(|| panic!("页面管理没有完整显示 {labels:?}"))
    };
    let title = text_rect(&[name], None);
    let settings=text_rect(&["设置 ›","设置"],Some(title.center().y));
    let switch=output.shapes.iter().find_map(|shape|match &shape.shape {
        egui::Shape::Rect(fill) if (fill.rect.size()-egui::vec2(crate::extras::SETTINGS_SWITCH_W,20.)).length()<0.2
            &&fill.rect.left()>title.right()&&(fill.rect.center().y-settings.center().y).abs()<5.
            &&shape.clip_rect.contains_rect(fill.rect)=>Some(fill.rect),_=>None,
    }).expect("插件后台开关与设置同行且完整可见");
    assert!(title.right()<switch.left()&&switch.right()+28.<settings.left(),"为图钉和眼睛保留独立空间");
    assert!(settings.right()<size.x-22.);
    assert!(!output.shapes.iter().any(|shape|matches!(&shape.shape,egui::Shape::Text(t) if ["打开","关闭并隐藏","显示入口","隐藏入口"].contains(&t.galley.text()))));

}

#[test]
fn plugin_status_layout_renders_author_ring_and_bar_from_the_same_snapshot() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut h = H::new();
    let profile = crate::model::config_dir();
    let ring = "test.bambu-ring";
    let bar = "test.bambu-bar";
    let source = profile.join("shared-status.json");
    fs::create_dir_all(&profile).unwrap();
    fs::write(
        &source,
        json!({
            "state":"running", "label":"打印中", "summary":"拓竹模拟任务 · 收纳盒",
            "progress_pct":48, "observed_at":1793756400,
            "metrics":[
                {"label":"喷嘴温度","value":"220 °C"},
                {"label":"热床温度","value":"60 °C"},
                {"label":"层数","value":"58 / 120"},
                {"label":"剩余时间","value":"24 分钟"}
            ],
            "alert":"耗材余量偏低"
        })
        .to_string(),
    )
    .unwrap();
    install_with_source(
        &mut h,
        &profile,
        ring,
        "拓竹打印（圆环）",
        &source,
        Some(json!({
            "components": [
                {"type":"state"}, {"type":"summary"}, {"type":"progress","variant":"ring"},
                {"type":"metrics","columns":2}, {"type":"alert"}, {"type":"updated_at"}
            ],
            "style": {"accent":"#74C99F"}
        })),
    );
    install_with_source(
        &mut h,
        &profile,
        bar,
        "拓竹打印（进度条）",
        &source,
        Some(json!({
            "components": [
                {"type":"state"}, {"type":"metrics","columns":1},
                {"type":"progress","variant":"bar"}, {"type":"alert"}, {"type":"updated_at"}
            ],
            "style":{"accent":"#66B8FF","density":"compact","surface":"plain"}
        })),
    );
    h.app.plugins = crate::sdk_spike::local::LocalPlugins::open(profile.join("local-plugins"));
    h.app.plugins.enable(ring, true).unwrap();
    h.app.plugins.enable(bar, true).unwrap();
    settled(&mut h, ring, StatusHealth::Ready);
    settled(&mut h, bar, StatusHealth::Ready);
    let (_, first, _) = h.app.plugins.status_page(ring).unwrap();
    let (_, second, _) = h.app.plugins.status_page(bar).unwrap();
    assert_eq!(first, second);

    let cursor = H::local(225.0, 85.0);
    let dir = std::env::var_os("DAOZAI_PLUGIN_STATUS_LAYOUT_PREVIEW").map(std::path::PathBuf::from);
    if let Some(dir) = &dir {
        fs::create_dir_all(dir).unwrap();
    }
    h.app.show_status_page(ring);
    h.hover(cursor, 0.8);
    assert!(has_text(&h, "拓竹模拟任务 · 收纳盒"));
    assert!(text_y(&h, "48") < text_y(&h, "220 °C"));
    if let Some(dir) = &dir {
        shoot_feature(
            &mut h,
            &mut feature_raster(),
            dir,
            "拓竹模拟-圆环",
            30,
            Some(cursor),
            "2026-11-04",
        );
    }
    h.app.show_status_page(bar);
    h.hover(cursor, 0.8);
    assert!(text_y(&h, "220 °C") < text_y(&h, "48"));
    assert!(!has_text(&h, "拓竹模拟任务 · 收纳盒"));
    assert!(has_text(&h, "耗材余量偏低"));
    assert!(h.shapes.iter().any(|shape| matches!(&shape.shape,egui::Shape::Rect(rect)
        if rect.rect.width()>90.0 && rect.rect.height()<14.0 && rect.fill==egui::Color32::from_rgb(102,184,255)
    )), "应绘制作者指定蓝色的横向进度条");
    if let Some(dir) = &dir {
        shoot_feature(
            &mut h,
            &mut feature_raster(),
            dir,
            "拓竹模拟-横向进度条",
            30,
            Some(cursor),
            "2026-11-04",
        );
    }
}

fn has_text(h: &H, label: &str) -> bool {
    h.shapes
        .iter()
        .any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == label))
}

fn settled(h: &mut H, id: &str, expected: StatusHealth) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        h.frame(vec![]);
        if h.app
            .plugins
            .status_page(id)
            .is_some_and(|(_, _, health)| health == expected)
        {
            return;
        }
        assert!(Instant::now() < deadline, "状态未就绪：{id} / {expected:?}");
        std::thread::sleep(Duration::from_millis(4));
    }
}

#[test]
fn plugin_canvas_demo_renders_with_mixed_layout() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut h = H::new();
    let id = "example.canvas-demo";
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/canvas-demo/plugin.json");
    h.app.plugins.install(&path).unwrap();
    h.app.plugins.enable(id, true).unwrap();
    settled(&mut h, id, StatusHealth::Ready);
    assert_eq!(h.app.plugins.status_height(id), 42.0 + 172.0 + 26.0 + 16.0 + 6.0 + 16.0);
    h.app.show_status_page(id);
    let cursor = H::local(225.0, 85.0);
    if let Some(dir) = std::env::var_os("DAOZAI_CANVAS_SCREENSHOT_DIR") {
        let dir = std::path::PathBuf::from(dir);
        fs::create_dir_all(&dir).unwrap();
        shoot_feature(&mut h, &mut feature_raster(), &dir, "A-画布图元", 30, Some(cursor), "2026-11-24");
    }
    h.hover(cursor, 0.8);
    assert!(has_text(&h, "这行是与画布混排的标准状态组件。"));
    assert!(has_text(&h, "画布"));
    assert!(h.shapes.iter().any(|shape| matches!(&shape.shape,
        egui::Shape::Text(text) if text.galley.text() == "图元"
            && text.fallback_color == egui::Color32::from_rgb(0x34, 0x7f, 0x69))));
}

#[test]
fn plugin_canvas_scroll_consumes_wheel_without_turning_the_island_page() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut h = H::new();
    let id = "example.canvas-live-demo";
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/canvas-live-demo/plugin.json");
    h.app.plugins.install(&path).unwrap();
    h.app.plugins.enable(id, true).unwrap();
    settled(&mut h, id, StatusHealth::Ready);
    h.app.show_status_page(id);
    let cursor = H::local(294.0, 153.0);
    h.hover(cursor, 0.8);
    let page = h.app.page();
    let last_visible = |h: &H| h.shapes.iter().any(|shape| {
        matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "记录 10"
            && shape.clip_rect.contains_rect(text.visual_bounding_rect()))
    });
    assert!(!last_visible(&h));
    h.wheel(-350.0);
    h.hover(cursor, 0.5);
    assert_eq!(h.app.page(), page);
    assert!(last_visible(&h));
}

#[test]
fn plugin_status_pages_render_from_authorized_data_and_keep_the_native_carousel() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut h = H::new();
    let profile = crate::model::config_dir();
    let printer = "example.a-printer";
    let companion = "example.b-companion";
    let todo = crate::app::TODO_PAGE;
    h.app.settings.visible_pages.fill(false);
    h.app.settings.visible_pages[todo] = true;
    install(
        &mut h,
        &profile,
        printer,
        "拓竹打印（模拟）",
        &json!({
            "state":"running", "label":"打印中", "summary":"拓竹模拟任务 · 收纳盒",
            "progress_pct":48,
            "metrics":[
                {"label":"喷嘴温度","value":"220 °C"}, {"label":"热床温度","value":"60 °C"},
                {"label":"层数","value":"58 / 120"}, {"label":"剩余时间","value":"24 分钟"}
            ],
            "alert":"耗材余量偏低", "observed_at":1793785200
        }),
    );
    install(
        &mut h,
        &profile,
        companion,
        "模拟状态源",
        &json!({
            "state":"idle", "label":"另一来源就绪", "metrics":[{"label":"已处理","value":"42 项"}]
        }),
    );
    h.app.plugins = crate::sdk_spike::local::LocalPlugins::open(profile.join("local-plugins"));
    h.app.plugins.enable(printer, true).unwrap();
    h.app.plugins.enable(companion, true).unwrap();
    h.app.show_page(todo);
    let cursor = H::local(225.0, 80.0);
    h.hover(cursor, 0.75);
    settled(&mut h, printer, StatusHealth::Ready);
    settled(&mut h, companion, StatusHealth::Ready);
    h.wheel(-40.0);
    h.hover(cursor, 0.65);
    assert_eq!(h.app.page(), crate::app::PAGE_NAMES.len());
    assert!(has_text(&h, "拓竹打印（模拟）"));
    assert!(has_text(&h, "打印中"));
    assert!(has_text(&h, "48"));
    assert!(has_text(&h, "220 °C"));
    assert!(has_text(&h, "耗材余量偏低"));
    if let Some(path) = std::env::var_os("DAOZAI_PLUGIN_STATUS_PREVIEW") {
        let path = std::path::PathBuf::from(path);
        fs::create_dir_all(&path).unwrap();
        shoot_feature(
            &mut h,
            &mut feature_raster(),
            &path,
            "拓竹结构化状态页",
            30,
            Some(cursor),
            "2026-11-04",
        );
    }

    h.wheel(-40.0);
    h.hover(cursor, 0.65);
    assert!(has_text(&h, "另一来源就绪"));
    h.app
        .plugins
        .set_status_page_enabled(printer, false)
        .unwrap();
    h.frame(vec![]);
    assert_eq!(h.app.page(), crate::app::PAGE_NAMES.len());
    assert!(
        has_text(&h, "另一来源就绪"),
        "其他插件页的编号移动后应保留所选来源"
    );
    assert!(h.app.plugins.status_page(printer).is_none());

    fs::remove_file(profile.join(printer).join("status.json")).unwrap();
    h.app
        .plugins
        .set_status_page_enabled(printer, true)
        .unwrap();
    settled(&mut h, printer, StatusHealth::Offline);
    h.app.show_status_page(printer);
    h.hover(cursor, 0.65);
    assert!(has_text(&h, "数据源失联"));
    assert!(!has_text(&h, "220 °C"));

    h.app
        .plugins
        .set_status_page_enabled(printer, false)
        .unwrap();
    h.frame(vec![]);
    assert_eq!(h.app.page(), todo, "关闭当前插件页后应回到仍开启的内置页");
    assert!(has_text(&h, "待办"));
    assert!(h
        .app
        .plugins
        .status_page(companion)
        .is_some_and(|(_, _, health)| health == StatusHealth::Ready));
}

#[test]
fn plugin_status_long_card_scrolls_without_changing_island_page() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut h = H::new();
    let profile = crate::model::config_dir();
    let id = "example.long-status";
    let source_path = profile.join("long-status.json");
    fs::create_dir_all(&profile).unwrap();
    fs::write(
        &source_path,
        json!({
            "state":"running", "label":"打印任务",
            "note":(1..=14).map(|i| format!("说明第 {i} 项")).collect::<Vec<_>>().join("\n"),
            "items":(1..=8).map(|i| json!({"label":format!("队列第 {i} 项")})).collect::<Vec<_>>(),
            "tags":["队列底部"]
        })
        .to_string(),
    )
    .unwrap();
    install_with_source(
        &mut h,
        &profile,
        id,
        "打印任务",
        &source_path,
        Some(json!({
            "components":[{"type":"state"},{"type":"text"},{"type":"list"},{"type":"badge"}]
        })),
    );
    h.app.plugins = crate::sdk_spike::local::LocalPlugins::open(profile.join("local-plugins"));
    h.app.plugins.enable(id, true).unwrap();
    settled(&mut h, id, StatusHealth::Ready);
    h.app.show_status_page(id);
    let cursor = H::local(225.0, 95.0);
    h.hover(cursor, 0.8);
    let page = h.app.page();
    let bottom_visible = |h: &H| {
        h.shapes.iter().any(|shape| {
            matches!(
                &shape.shape,
                egui::Shape::Text(text) if text.galley.text() == "队列底部"
                    && shape.clip_rect.contains_rect(text.visual_bounding_rect())
            )
        })
    };
    assert!(!bottom_visible(&h));
    h.wheel(-350.0);
    h.hover(cursor, 0.5);
    assert_eq!(h.app.page(), page);
    assert!(bottom_visible(&h), "滚动后应显示卡片底部的标签");
}
