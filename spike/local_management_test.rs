use super::*;
use eframe::egui;

pub(super) fn frame(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    draw: impl FnMut(&mut egui::Ui),
) -> egui::FullOutput {
    frame_in(ctx, egui::vec2(500., 550.), events, draw)
}

/// 与真实设置窗口差不多大的画布，教程抽屉这类长内容用。
fn frame_in(
    ctx: &egui::Context,
    size: egui::Vec2,
    events: Vec<egui::Event>,
    mut draw: impl FnMut(&mut egui::Ui),
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, &mut draw);
        },
    )
}

pub(super) fn click(ctx: &egui::Context, label: &str, mut draw: impl FnMut(&mut egui::Ui)) {
    let _ = frame(ctx, vec![], &mut draw);
    let out = frame(ctx, vec![], &mut draw);
    let pos = out
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(text.visual_bounding_rect().center())
            }
            _ => None,
        })
        .or_else(|| {
            let id = match label {
                "启用" | "停用" => ["example.payday", "example.material"]
                    .into_iter()
                    .map(|plugin| egui::Id::new(("settings-plugin-switch", plugin)))
                    .find(|id| ctx.read_response(*id).is_some())?,
                "刷新目录" => egui::Id::new("plugin-market-refresh"),
                _ => return None,
            };
            ctx.read_response(id).map(|response| response.rect.center())
        })
        .unwrap_or_else(|| panic!("找不到按钮 {label}"));
    click_at(ctx, pos, draw);
}

pub(super) fn click_at(ctx: &egui::Context, pos: egui::Pos2, mut draw: impl FnMut(&mut egui::Ui)) {
    for pressed in [true, false] {
        let _ = frame(
            ctx,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ],
            &mut draw,
        );
    }
}

pub(super) fn has_text(output: &egui::FullOutput, label: &str) -> bool {
    output
        .shapes
        .iter()
        .any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == label))
}

#[test]
fn script_settings_controls_save_values_in_the_owner_page() -> Result<()> {
    const ID: &str = "example.script-settings";
    let dir = std::env::temp_dir().join(format!(
        "daozai-script-settings-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let mut host = LocalPlugins::open(dir.clone());
    host.install(&Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/payday/plugin.json"))?;
    let mut record = host.records["example.payday"].clone();
    let package = record.package.as_mut().unwrap();
    package.manifest.id = ID.into();
    package.manifest.name = "设置控件测试".into();
    package.manifest.owner = "tasks".into();
    package.manifest.capabilities = vec!["card".into(), "settings".into()];
    package.manifest.settings = serde_json::from_value(json!([
        {"key":"switch", "label":"启用选项", "type":"switch", "default":"false"},
        {"key":"choice", "label":"计数方式", "type":"choice", "default":"remain",
         "choices":[{"value":"remain","label":"剩余天数"},{"value":"elapsed","label":"累计天数"}]},
        {"key":"number", "label":"计数步长", "type":"number", "default":"4",
         "min":1, "max":10, "step":1, "unit":"天", "precision":0},
        {"key":"time", "label":"提醒时间", "type":"time", "default":"08:00"},
        {"key":"weekdays", "label":"重复日期", "type":"weekdays", "default":"1,3"},
        {"key":"note", "label":"说明", "type":"text", "default":"第一行\n第二行", "multiline":true},
        {"key":"list", "label":"城市列表", "type":"list", "default":"[]", "maxItems":2,
         "item":[{"key":"city","label":"城市","type":"text","default":"北京"}]}
    ]))?;
    package.source = "function refresh() { return {card: '设置控件测试'}; }".into();
    package.manifest.validate()?;
    record.values = package.settings_values(&Values::new());
    host.put(ID, record)?;
    host.enable(ID, true)?;

    let ctx = egui::Context::default();
    crate::theme::load_fonts(&ctx);
    let mut draw = |ui: &mut egui::Ui| host.settings(ui, "tasks", true);
    let first = frame(&ctx, vec![], &mut draw);
    assert!(has_text(&first, "设置控件测试"));
    assert!(has_text(&first, "插件"));
    let switch = ctx
        .read_response(egui::Id::new(("settings-plugin-switch", format!("{ID}:switch"))))
        .expect("脚本插件设置开关")
        .rect
        .center();
    click_at(&ctx, switch, &mut draw);
    click(&ctx, "累计天数", &mut draw);
    click(&ctx, "4天", &mut draw);
    frame(&ctx, vec![], &mut draw);
    frame(&ctx, vec![egui::Event::Text("7".into())], &mut draw);
    frame(&ctx, vec![egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    }], &mut draw);
    click(&ctx, "+", &mut draw);
    click(&ctx, "日", &mut draw);
    click(&ctx, "添加一行", &mut draw);
    click(&ctx, "保存设置", &mut draw);

    let mut changed = host.records[ID].values.clone();
    changed.insert("note".into(), "改过的第一行\n保留第二行".into());
    host.save(ID, changed)?;
    drop(host);
    let host = LocalPlugins::open(dir.clone());
    let values = &host.records[ID].values;
    assert_eq!(values["switch"], "true");
    assert_eq!(values["choice"], "elapsed");
    assert_eq!(values["number"], "7");
    assert_eq!(values["time"], "08:15");
    assert_eq!(values["weekdays"], "1,3,7");
    assert_eq!(values["note"], "改过的第一行\n保留第二行");
    assert_eq!(serde_json::from_str::<Value>(&values["list"])?, json!([{"city":"北京"}]));
    drop(host);
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[cfg(windows)]
#[test]
fn storage_limit_error_is_visible_in_local_plugin_details() -> Result<()> {
    const ID: &str = "example.payday";
    const ERROR: &str = "私有存储已满，上限为 256 KiB";
    let dir = std::env::temp_dir().join(format!(
        "daozai-storage-detail-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let mut host = LocalPlugins::open(dir.clone());
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/payday/plugin.json");
    host.install(&sample)?;
    let mut record = host.records[ID].clone();
    record.storage = json!("已保存的数据");
    record.package.as_mut().unwrap().source =
        r#"function refresh() {return {card:"待办",storage:"x".repeat(256*1024)};}"#.into();
    host.put(ID, record)?;
    host.enable(ID, true)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !host.errors.contains_key(ID) {
        host.tick_features(true, false, false, "2026-11-23");
        assert!(Instant::now() < deadline, "等待插件存储错误超时");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(host.errors[ID], ERROR);
    assert_eq!(host.records[ID].storage, json!("已保存的数据"));

    let ctx = egui::Context::default();
    crate::theme::load_fonts(&ctx);
    let mut draw = |ui: &mut egui::Ui| {
        host.manage(
            ui,
            true,
            true,
            true,
            #[cfg(feature = "sdk-material")]
            None,
        );
    };
    click(&ctx, "···", &mut draw);
    click(&ctx, "查看详情", &mut draw);
    for _ in 0..20 {
        let _ = frame(&ctx, vec![], &mut draw);
    }
    let output = frame(&ctx, vec![], &mut draw);
    assert!(has_text(&output, ID));
    let count = output
        .shapes
        .iter()
        .filter(|shape| {
            matches!(
                &shape.shape,
                egui::Shape::Text(text) if text.galley.text().contains(ERROR)
            )
        })
        .count();
    assert!(count >= 2, "已安装列表和详情都应显示存储超限原因");
    let record_path = host.path(ID);
    drop(host);
    fs::remove_file(record_path)?;
    fs::remove_dir(dir)?;
    Ok(())
}

#[test]
fn status_page_management_opens_and_switches_independently() -> Result<()> {
    const ID: &str = "example.status-management";
    const NAME: &str = "模拟状态详情";
    let dir = std::env::temp_dir().join(format!(
        "daozai-status-management-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let mut host = LocalPlugins::open(dir.clone());
    host.install(&Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/payday/plugin.json"))?;
    let mut record = host.records["example.payday"].clone();
    let package = record.package.as_mut().unwrap();
    package.manifest.id = ID.into();
    package.manifest.name = NAME.into();
    package.manifest.owner = "status".into();
    package.manifest.capabilities = vec!["status.page".into(), "settings".into()];
    package.source = "function refresh() { return {status: null}; }".into();
    record.values = package.settings_values(&Values::new());
    host.put(ID, record)?;
    let ctx = egui::Context::default();
    crate::theme::load_fonts(&ctx);
    let draw = |ui: &mut egui::Ui, host: &mut LocalPlugins| {
        let _ = host.status_page_settings(ui, ID);
    };
    let toggle_page = |host: &mut LocalPlugins| {
        for _ in 0..2 {
            frame(&ctx, vec![], |ui| draw(ui, host));
        }
        let switch = ctx
            .read_response(egui::Id::new(("settings-plugin-switch", ID)))
            .expect("页面管理中的插件开关")
            .rect
            .center();
        click_at(&ctx, switch, |ui| draw(ui, host));
    };
    assert!(has_text(&frame(&ctx, vec![], |ui| draw(ui, &mut host)), NAME));
    assert!(host.status_pages().all(|(id,_)|id!=ID));
    assert!(host.status_page_settings_hidden(ID));
    host.set_status_page_settings_hidden(ID,false)?;
    assert!(host.status_page_settings_hidden(ID), "关闭时眼睛不可修改入口");
    toggle_page(&mut host);
    assert!(host.records[ID].enabled);
    assert!(host.status_pages().any(|(id,_)|id==ID));
    assert!(!host.status_page_settings_hidden(ID));
    host.set_status_page_settings_hidden(ID,true)?;
    assert!(host.status_pages().any(|(id,_)|id==ID), "眼睛不影响后台");
    drop(host);
    let mut host=LocalPlugins::open(dir.clone());
    assert!(host.records[ID].enabled && host.status_page_settings_hidden(ID));
    host.set_status_page_settings_hidden(ID,false)?;
    toggle_page(&mut host);
    assert!(!host.records[ID].enabled && host.status_page_settings_hidden(ID));
    assert!(host.status_pages().all(|(id,_)|id!=ID));
    let mut route=None;
    click(&ctx,"设置 ›",|ui|route=host.status_page_settings(ui,ID).or(route.take()));
    assert_eq!(route,Some(SettingsNavigation::Plugin(ID.into())));
    host.set_status_page_enabled(ID,true)?;
    assert!(!host.status_page_settings_hidden(ID), "重新开启自动显示入口");
    let mut record=host.records[ID].clone();
    record.package.as_mut().unwrap().manifest.name="独立功能模块".repeat(13);
    host.put(ID,record)?;
    let size=egui::vec2(310.0,550.0);
    frame_in(&ctx,size,vec![],|ui|host.standalone_settings(ui,ID));
    let switch=ctx.read_response(egui::Id::new(("settings-plugin-switch",format!("{ID}:status-page")))).unwrap();
    assert!(switch.rect.right()<=size.x);
    drop(host);
    let _=fs::remove_dir_all(dir);
    Ok(())
}

#[test]
fn management_buttons_jump_disable_and_confirm_uninstall() -> Result<()> {
    let dir = std::env::temp_dir().join(format!(
        "daozai-manage-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let mut host = LocalPlugins::open(dir.clone());
    host.install(&Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/payday/plugin.json"))?;
    let mut legacy = host.records["example.payday"].clone();
    let package = legacy.package.as_mut().unwrap();
    package.manifest.cover = None;
    package.cover = None;
    host.put("example.payday", legacy)?;
    drop(host);
    let mut host = LocalPlugins::open(dir.clone());
    let ctx = egui::Context::default();
    crate::theme::load_fonts(&ctx);
    let first = frame(&ctx, vec![], |ui| {
        host.manage(
            ui,
            true,
            true,
            true,
            #[cfg(all(windows, feature = "sdk-material"))]
            None,
        );
    });
    assert!(has_text(&first, "暂无封面"));
    let mut jumped = false;
    let mut click = |host: &mut LocalPlugins, label: &str| {
        click(&ctx, label, |ui| {
            let navigation = host.manage(
                ui,
                true,
                true,
                true,
                #[cfg(all(windows, feature = "sdk-material"))]
                None,
            );
            if let Some(SettingsNavigation::Plugin(id)) = navigation {
                jumped |= host.settings_destination(&id) == Some(4);
            }
        });
    };
    click(&mut host, "启用");
    assert!(host.records["example.payday"].enabled);
    click(&mut host, "设置 › 待办与置物 ›");
    assert_eq!(host.focus.as_deref(), Some("example.payday"));
    click(&mut host, "停用");
    assert!(!host.records["example.payday"].enabled);
    click(&mut host, "···");
    click(&mut host, "卸载…");
    click(&mut host, "取消");
    assert!(host.records["example.payday"].package.is_some());
    click(&mut host, "···");
    click(&mut host, "卸载…");
    click(&mut host, "删除数据");
    assert!(!host.records.contains_key("example.payday"));
    assert!(jumped);
    fs::remove_dir(dir)?;
    Ok(())
}

#[test]
fn management_module_pause_and_plugin_switch_keep_their_own_choices() -> Result<()> {
    let dir = std::env::temp_dir().join(format!(
        "daozai-manage-switches-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let mut host = LocalPlugins::open(dir.clone());
    host.install(&Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/payday/plugin.json"))?;
    let ctx = egui::Context::default();
    crate::theme::load_fonts(&ctx);
    let settings = |host: &mut LocalPlugins, owner| {
        frame(&ctx, vec![], |ui| host.settings(ui, "tasks", owner))
    };
    let manage = |host: &mut LocalPlugins, ui: &mut egui::Ui, owner| {
        host.manage(
            ui,
            owner,
            true,
            true,
            #[cfg(all(windows, feature = "sdk-material"))]
            None,
        )
    };
    assert!(!has_text(&settings(&mut host, true), "倒计时名称"));
    click(&ctx, "启用", |ui| {
        let _ = manage(&mut host, ui, true);
    });
    assert!(has_text(&settings(&mut host, true), "倒计时名称"));
    assert!(!has_text(&settings(&mut host, false), "倒计时名称"));
    let paused = frame(&ctx, vec![], |ui| {
        let _ = manage(&mut host, ui, false);
    });
    assert!(has_text(&paused, "页面管理 › 待办与置物 ›"));
    assert!(!has_text(&paused, "设置 › 待办与置物 ›"));
    let mut jump = None;
    click(&ctx, "页面管理 › 待办与置物 ›", |ui| {
        jump = manage(&mut host, ui, false).or(jump.take());
    });
    assert_eq!(jump, Some(SettingsNavigation::Tab(crate::extras::SETTINGS_TAB_PAGES)));

    click(&ctx, "停用", |ui| {
        let _ = manage(&mut host, ui, false);
    });
    assert!(!has_text(&settings(&mut host, true), "倒计时名称"));
    click(&ctx, "启用", |ui| {
        let _ = manage(&mut host, ui, false);
    });
    assert!(!has_text(&settings(&mut host, false), "倒计时名称"));
    assert!(has_text(&settings(&mut host, true), "倒计时名称"));

    host.cards
        .insert("example.payday".into(), "发薪倒计时".into());
    let mut opened = false;
    let mut draw = |ui: &mut egui::Ui| {
        let card = crate::paint::Card::new(ui, egui::Pos2::ZERO, ui.max_rect());
        opened |= host.card(&card);
    };
    let _ = frame(&ctx, vec![], &mut draw);
    let _ = frame(&ctx, vec![], &mut draw);
    click_at(&ctx, egui::pos2(363., 145.), &mut draw);
    assert!(opened);
    assert_eq!(host.focus.as_deref(), Some("example.payday"));
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn invalid_record_can_be_removed_from_the_installed_list() -> Result<()> {
    let dir = std::env::temp_dir().join(format!(
        "daozai-manage-invalid-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let mut host = LocalPlugins::open(dir.clone());
    host.install(&Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/payday/plugin.json"))?;
    drop(host);
    let record = dir.join("local-example.payday.json");
    fs::write(&record, "{}")?;
    let mut host = LocalPlugins::open(dir.clone());
    assert!(host.invalid.contains("example.payday"));
    let ctx = egui::Context::default();
    crate::theme::load_fonts(&ctx);
    let mut draw = |ui: &mut egui::Ui| {
        host.manage(
            ui,
            true,
            true,
            true,
            #[cfg(all(windows, feature = "sdk-material"))]
            None,
        );
    };
    assert!(has_text(&frame(&ctx, vec![], &mut draw), "需要处理"));
    click(&ctx, "移除记录", &mut draw);
    assert!(host.invalid.is_empty());
    assert!(!record.exists());
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[cfg(windows)]
#[test]
fn market_buttons_install_and_update_an_existing_sample() -> Result<()> {
    let dir = std::env::temp_dir().join(format!(
        "daozai-manage-market-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let files = dir.join("author");
    let published = dir.join("market");
    fs::create_dir_all(&files)?;
    fs::create_dir(&published)?;
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/payday/plugin.json");
    let catalog = published.join("catalog.json");
    let first = files.join("first.zip");
    market::pack(&sample, &first)?;
    market::submit(&first, &catalog)?;
    let mut host = LocalPlugins::open(dir.join("installed"));
    host.set_market_catalog(&catalog)?;

    let ctx = egui::Context::default();
    crate::theme::load_fonts(&ctx);
    ctx.style_mut(|style| style.animation_time = 0.);
    let draw = |host: &mut LocalPlugins, ui: &mut egui::Ui| {
        host.header_tabs(ui);
        host.manage(
            ui,
            true,
            true,
            true,
            #[cfg(all(windows, feature = "sdk-material"))]
            None,
        );
    };
    let click_button = |host: &mut LocalPlugins, label| click(&ctx, label, |ui| draw(host, ui));
    let finish = |host: &mut LocalPlugins| {
        let deadline = Instant::now() + Duration::from_secs(3);
        while host.market_busy() && Instant::now() < deadline {
            frame(&ctx, vec![], |ui| draw(host, ui));
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!host.market_busy(), "测试集市操作未完成");
    };
    let official = frame(&ctx, vec![], |ui| draw(&mut host, ui));
    assert!(ctx
        .read_response(egui::Id::new("plugin-market-refresh"))
        .is_some());
    assert!(!has_text(&official, "发薪倒计时"));
    assert!(!has_text(&official, "导入测试集市目录…"));
    click_button(&mut host, "已安装");
    click_button(&mut host, "开发者工具");
    let developers = frame(&ctx, vec![], |ui| draw(&mut host, ui));
    assert!(has_text(&developers, "导入测试集市目录…"));
    click_button(&mut host, "关闭");
    click_button(&mut host, "集市");
    click_button(&mut host, "本地测试");
    assert!(has_text(
        &frame(&ctx, vec![], |ui| draw(&mut host, ui)),
        "本地测试集市"
    ));
    click_button(&mut host, "安装");
    finish(&mut host);
    assert!(!host.records["example.payday"].enabled);
    click_button(&mut host, "已安装");
    click_button(&mut host, "启用");
    assert!(host.records["example.payday"].enabled);

    let mut manifest: Value = serde_json::from_slice(&fs::read(&sample)?)?;
    manifest["version"] = json!("1.1.0");
    let updated = files.join("plugin.json");
    fs::write(&updated, serde_json::to_vec(&manifest)?)?;
    fs::copy(sample.with_file_name("main.js"), files.join("main.js"))?;
    fs::copy(sample.with_file_name("cover.png"), files.join("cover.png"))?;
    let second = files.join("second.zip");
    market::pack(&updated, &second)?;
    market::submit(&second, &catalog)?;
    click_button(&mut host, "集市");
    click_button(&mut host, "本地测试");
    click_button(&mut host, "刷新目录");
    let available = frame(&ctx, vec![], |ui| draw(&mut host, ui));
    assert!(has_text(&available, "更新"));
    click_button(&mut host, "更新");
    finish(&mut host);

    let record = &host.records["example.payday"];
    assert_eq!(record.package.as_ref().unwrap().manifest.version, "1.1.0");
    assert!(record.enabled);
    assert!(has_text(
        &frame(&ctx, vec![], |ui| draw(&mut host, ui)),
        "已安装"
    ));
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[cfg(windows)]
#[test]
fn official_market_only_requests_uncached_data_or_explicit_refresh() -> Result<()> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use market_metrics::tests::next_request;
    use std::{io::Write, net::TcpListener, sync::mpsc, thread};

    const ID: &str = "example.payday";
    const CONTENTS: &str = "/contents/preview/";
    const ZIP: &str = "plugin-example.payday-1.0.0.zip";
    let dir = std::env::temp_dir().join(format!(
        "daozai-refresh-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    fs::create_dir_all(&dir)?;
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/payday/plugin.json");
    let bundle = dir.join("sample.zip");
    let index_file = dir.join("catalog.json");
    market::pack(&sample, &bundle)?;
    market::submit(&bundle, &index_file)?;
    let contents = |name: &str| -> Result<Vec<u8>> {
        let bytes = fs::read(dir.join(name))?;
        Ok(serde_json::to_vec(&json!({
            "type": "file", "path": format!("preview/{name}"), "encoding": "base64",
            "size": bytes.len(), "content": STANDARD.encode(&bytes)
        }))?)
    };
    let index_body = contents("catalog.json")?;
    let zip_body = contents(ZIP)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    listener.set_nonblocking(true)?;
    let (log_request, requests) = mpsc::channel();
    let (stop, stopped) = mpsc::channel::<()>();
    let server = thread::spawn(move || {
        let mut downloads = 0;
        let mut favorites = 0;
        while let Err(mpsc::TryRecvError::Empty) = stopped.try_recv() {
            let (mut stream, _) = match listener.accept() {
                Ok(peer) => peer,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            stream.set_nonblocking(false).unwrap();
            let (line, data) = next_request(&mut stream);
            let line = line.trim_end();
            let snapshot = |downloads, favorites| {
                serde_json::to_vec(&json!({"plugins": {
                    ID: {"downloads": downloads, "favorites": favorites}
                }}))
                .unwrap()
            };
            let (kind, body) = match line {
                "GET /contents/preview/catalog.json?ref=master HTTP/1.1" => {
                    ("目录 GET", index_body.clone())
                }
                "GET /contents/preview/plugin-example.payday-1.0.0.zip?ref=master HTTP/1.1" => {
                    ("ZIP GET", zip_body.clone())
                }
                "GET /v1/stats?channel=preview HTTP/1.1" => {
                    ("统计 GET", snapshot(downloads, favorites))
                }
                "POST /v1/events HTTP/1.1" => {
                    assert_eq!(data["channel"], "preview");
                    let events = data["events"].as_array().unwrap();
                    assert_eq!(events.len(), 1);
                    let event = &events[0];
                    assert_eq!(event["plugin_id"], ID);
                    let kind = match event["kind"].as_str().unwrap() {
                        "download" => {
                            assert_eq!(event["version"], "1.0.0");
                            downloads += 1;
                            "下载 POST"
                        }
                        "favorite" => {
                            favorites = u64::from(event["saved"].as_bool().unwrap());
                            if favorites == 1 {
                                "收藏 POST"
                            } else {
                                "取消收藏 POST"
                            }
                        }
                        kind => panic!("未预期的事件 {kind}"),
                    };
                    (kind, snapshot(downloads, favorites))
                }
                line => panic!("未预期的请求 {line}"),
            };
            log_request.send(kind).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        }
    });

    let storage = dir.join("installed");
    let open = || -> Result<LocalPlugins> {
        let mut host = LocalPlugins::open(storage.clone());
        host.set_official_test_index(url::Url::parse(&format!(
            "http://{addr}{CONTENTS}catalog.json?ref=master"
        ))?);
        host.market_metrics = market_metrics::MarketMetrics::for_test(
            &storage,
            url::Url::parse(&format!("http://{addr}"))?,
        );
        Ok(host)
    };
    let ctx = egui::Context::default();
    crate::theme::load_fonts(&ctx);
    ctx.style_mut(|style| style.animation_time = 0.);
    let draw = |host: &mut LocalPlugins, ui: &mut egui::Ui| {
        host.header_tabs(ui);
        let _ = host.manage(
            ui,
            true,
            true,
            true,
            #[cfg(feature = "sdk-material")]
            None,
        );
    };
    let click_button = |host: &mut LocalPlugins, label| click(&ctx, label, |ui| draw(host, ui));
    let finish = |host: &mut LocalPlugins| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            frame(&ctx, vec![], |ui| draw(host, ui));
            if !host.market_pending() {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
    };
    let assert_requests = |phase, expected: &[&str]| {
        let mut actual: Vec<_> = requests.try_iter().collect();
        let mut expected = expected.to_vec();
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected, "{phase}");
        println!("{phase} {actual:?}");
    };

    let mut host = open()?;
    host.set_market_catalog(&index_file)?;
    finish(&mut host);
    assert_requests("首次打开", &["目录 GET", "统计 GET", "ZIP GET"]);
    assert!(
        host.market_stats_repaint_delay().is_none(),
        "空闲集市无需定时刷新"
    );
    click_button(&mut host, "收藏 0");
    click_button(&mut host, "全部");
    click_button(&mut host, "已安装");
    click_button(&mut host, "去插件集市");
    click_button(&mut host, "本地测试");
    click_button(&mut host, "官方");
    click_button(&mut host, "发薪倒计时");
    let detail = egui::Id::new("plugin-market-details");
    assert_eq!(
        ctx.data(|data| data.get_temp::<String>(detail)).as_deref(),
        Some(ID)
    );
    let _ = frame(
        &ctx,
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }],
        |ui| draw(&mut host, ui),
    );
    assert!(ctx.data(|data| data.get_temp::<String>(detail)).is_none());
    let _ = frame(&ctx, vec![], |ui| {
        ui.label("其他设置");
    });
    finish(&mut host);
    assert_requests("切标签、从详情返回及重新打开设置", &[]);

    click_button(&mut host, "刷新目录");
    finish(&mut host);
    assert_requests("手动刷新", &["目录 GET", "统计 GET"]);
    click_button(&mut host, "安装");
    finish(&mut host);
    assert_eq!(
        host.records[ID].package.as_ref().unwrap().manifest.version,
        "1.0.0"
    );
    assert_eq!(host.market_counts(ID).unwrap().downloads, 1);
    assert_requests("安装", &["ZIP GET", "下载 POST"]);
    let catalog = host.market_catalog(true).unwrap();
    let entry = &catalog.entries[0];
    host.set_favorite(entry, true)?;
    finish(&mut host);
    assert_eq!(host.market_counts(ID).unwrap().favorites, 1);
    assert_requests("收藏", &["收藏 POST"]);
    host.remove_favorite(ID)?;
    finish(&mut host);
    assert_eq!(host.market_counts(ID).unwrap().favorites, 0);
    assert_requests("取消收藏", &["取消收藏 POST"]);

    drop(host);
    let mut host = open()?;
    host.ensure_official_catalog();
    assert_eq!(host.market_counts(ID).unwrap().downloads, 1);
    assert!(host.market_catalog(true).is_some());
    assert!(!host.market_pending());
    assert_requests("重启后恢复目录和统计", &[]);
    stop.send(())?;
    server.join().unwrap();
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[cfg(all(windows, feature = "sdk-material"))]
#[test]
fn material_management_waits_for_permission_and_offers_to_enable_compatibility() -> Result<()> {
    let dir = std::env::temp_dir().join(format!(
        "daozai-manage-material-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let package = dir.join("package");
    fs::create_dir_all(&package)?;
    let manifest = package.join("plugin.json");
    fs::write(
        package.join("cover.png"),
        include_bytes!("examples/payday/cover.png"),
    )?;
    fs::write(
        package.join("material.wgsl"),
        "fn material(i: MaterialIn) -> vec4<f32> { return vec4<f32>(i.px, 0.0, 1.0); }",
    )?;
    fs::write(
        &manifest,
        serde_json::to_vec(&json!({
            "id": "example.material",
            "name": "测试材质",
            "author": "测试",
            "version": "1.0.0",
            "api": 1,
            "runtime": "material",
            "entry": "material.wgsl",
            "cover": "cover.png",
            "owner": "appearance",
            "capabilities": ["appearance.material", "settings"],
            "dataVersion": 1,
            "permissions": {"screen.backdrop": true},
            "material": {"controls": true, "spring": {"stiffness": 1.0, "damping": 0.5}},
            "settings": [
                {"key": "tint", "label": "着色量", "type": "number", "min": 0, "max": 1, "step": 0.1, "default": "0.5", "unit": "px", "precision": 2},
                {"key": "blur", "label": "模糊", "type": "number", "min": 0, "max": 1, "step": 0.005, "default": "0.5"},
                {"key": "strength", "label": "强度", "type": "number", "min": 0, "max": 1, "step": 0.05, "default": "0.54", "precision": 0}
            ]
        }))?,
    )?;
    let mut host = LocalPlugins::open(dir.join("installed"));
    let ctx = egui::Context::default();
    crate::theme::load_fonts(&ctx);
    let appearance = |host: &mut LocalPlugins| {
        frame(&ctx, vec![], |ui| {
            assert!(!host.material_settings(ui, None, false));
        })
    };
    assert!(!has_text(&appearance(&mut host), "材质"));
    host.install(&manifest)?;
    assert!(!host.material_permission_granted("example.material"));
    assert!(has_text(&appearance(&mut host), "材质"));
    let manage_click = |host: &mut LocalPlugins, label: &str| {
        let mut jump = None;
        click(&ctx, label, |ui| {
            jump = host.manage(ui, true, true, true, None).or(jump.take())
        });
        jump
    };

    assert!(manage_click(&mut host, "启用").is_none());
    assert!(host.selected_material_id().is_none());
    assert!(!host.material_permission_granted("example.material"));
    assert!(manage_click(&mut host, "取消").is_none());
    assert!(host.selected_material_id().is_none());
    assert!(manage_click(&mut host, "启用").is_none());
    assert_eq!(
        manage_click(&mut host, "允许并启用"),
        Some(SettingsNavigation::Tab(crate::extras::SETTINGS_TAB_APPEARANCE))
    );
    assert_eq!(host.selected_material_id(), Some("example.material"));

    let mut restart = false;
    click(&ctx, "开启并重启", |ui| {
        restart |= host.material_settings(ui, None, false)
    });
    assert!(restart);
    assert_eq!(host.selected_material_id(), Some("example.material"));
    let settings = frame(&ctx, vec![], |ui| {
        host.material_settings(ui, None, false);
    });
    for expected in ["0.50px", "0.500", "插件"] {
        assert!(
            settings.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == expected)
            }),
            "{expected}"
        );
    }
    assert_eq!(host.records["example.material"].values["strength"], "0.54");
    host.uninstall("example.material", true)?;
    assert!(!has_text(&appearance(&mut host), "材质"));
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn header_tabs_keep_market_on_the_left_in_a_right_aligned_title_row() {
    let dir = std::env::temp_dir().join(format!("daozai-header-tabs-{}", std::process::id()));
    let mut host = LocalPlugins::open(dir.join("installed"));
    let ctx = egui::Context::default();
    crate::theme::load_fonts(&ctx);
    let out = frame(&ctx, vec![], |ui| {
        ui.horizontal(|ui| {
            ui.label("插件");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                host.header_tabs(ui)
            });
        });
    });
    let x = |label: &str| {
        out.shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => Some(text.pos.x),
                _ => None,
            })
            .unwrap_or_else(|| panic!("找不到 {label}"))
    };
    assert!(x("集市") < x("已安装"));
    let _ = fs::remove_dir_all(dir);
}

#[cfg(windows)]
#[test]
fn market_channel_toolbar_keeps_categories_clickable_at_narrow_width() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("daozai-market-toolbar-{}", std::process::id()));
    fs::create_dir_all(&dir)?;
    let catalog = dir.join("catalog.json");
    let entries = [
        ("example.appearance", "外观样例", "appearance"),
        ("example.pages", "页面样例", "pages"),
        ("example.status", "功能样例", "status"),
        ("example.calendar", "日程样例", "calendar"),
        ("example.tasks", "待办样例", "tasks"),
        ("example.quota", "额度样例", "quota"),
        ("example.wireless", "无线样例", "wireless"),
    ]
    .map(|(id, name, owner)| {
        serde_json::json!({
            "id": id, "name": name, "author": "示例作者", "version": "1.0.0",
            "owner": owner, "size": 1, "sha256": "0".repeat(64),
        })
    });
    fs::write(&catalog, serde_json::to_vec(&serde_json::json!({
        "format": 1, "plugins": entries,
    }))?)?;

    let mut host = LocalPlugins::open(dir.join("installed"));
    host.set_market_catalog(&catalog)?;
    let ctx = egui::Context::default();
    crate::theme::load_fonts(&ctx);
    let size = egui::vec2(370.0, 800.0);
    let mut draw = |ui: &mut egui::Ui| {
        host.manage(
            ui,
            true,
            true,
            true,
            #[cfg(feature = "sdk-material")]
            None,
        );
    };
    click_in(&ctx, size, "本地测试", &mut draw);
    let output = frame_in(&ctx, size, vec![], &mut draw);
    let bounds = |label| {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                let rect = text.visual_bounding_rect();
                assert!(shape.clip_rect.contains_rect(rect), "{label} 被裁剪");
                Some(rect)
            }
            _ => None,
        }).unwrap_or_else(|| panic!("找不到 {label}"))
    };
    let online = if option_env!("DAOZAI_TEST_BUILD").is_some() { "预览集市" } else { "官方" };
    let channel_bottom = bounds(online).bottom().max(bounds("本地测试").bottom());
    for label in ["全部", "外观", "页面管理", "独立功能", "日程", "待办与置物", "AI 额度", "无线设备"] {
        assert!(bounds(label).top() > channel_bottom, "{label} 与频道工具行重叠");
    }

    click_in(&ctx, size, "独立功能", &mut draw);
    let filtered = frame_in(&ctx, size, vec![], &mut draw);
    assert!(has_text(&filtered, "功能样例"));
    assert!(!has_text(&filtered, "外观样例"));
    fs::remove_dir_all(dir)?;
    Ok(())
}

const WINDOW: egui::Vec2 = egui::vec2(940., 860.);

fn click_in(ctx: &egui::Context, size: egui::Vec2, label: &str, mut draw: impl FnMut(&mut egui::Ui)) {
    let _ = frame_in(ctx, size, vec![], &mut draw);
    let out = frame_in(ctx, size, vec![], &mut draw);
    let pos = out
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(text.visual_bounding_rect().center())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("找不到按钮 {label}"));
    for pressed in [true, false] {
        let _ = frame_in(
            ctx,
            size,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ],
            &mut draw,
        );
    }
}

#[test]
fn guide_opens_from_the_market_copies_a_prompt_and_stacks_the_template_form() {
    let dir = std::env::temp_dir().join(format!("daozai-guide-{}", std::process::id()));
    let mut host = LocalPlugins::open(dir.join("installed"));
    let ctx = egui::Context::default();
    crate::theme::load_fonts(&ctx);
    ctx.style_mut(|style| style.animation_time = 0.);
    let draw = |host: &mut LocalPlugins, ui: &mut egui::Ui| {
        host.header_tabs(ui);
        host.manage(
            ui,
            true,
            true,
            true,
            #[cfg(feature = "sdk-material")]
            None,
        );
    };
    let market = frame_in(&ctx, WINDOW, vec![], |ui| draw(&mut host, ui));
    assert!(has_text(&market, "看教程"));
    assert!(!has_text(&market, "做一个岛仔插件"));
    click_in(&ctx, WINDOW, "看教程", |ui| draw(&mut host, ui));
    let guide = frame_in(&ctx, WINDOW, vec![], |ui| draw(&mut host, ui));
    let capabilities_title = "当前版本已开放的 SDK 能力";
    for expected in [
        "做一个岛仔插件", capabilities_title, "一句话交给 AI 做", "JS 卡片模板",
        "复制这段话", "从模板创建", "投稿指南",
    ] {
        assert!(has_text(&guide, expected), "{expected}");
    }
    assert!(!has_text(&guide, "卡片与标注"));

    click_in(&ctx, WINDOW, capabilities_title, |ui| draw(&mut host, ui));
    let expanded = frame_in(&ctx, WINDOW, vec![], |ui| draw(&mut host, ui));
    let text_rect = |label: &str| {
        expanded.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text().starts_with(label) => {
                Some(text.visual_bounding_rect())
            }
            _ => None,
        }).unwrap_or_else(|| panic!("找不到教程文字 {label}"))
    };
    assert!(
        text_rect("卡片与标注").bottom() <= text_rect("待办信息卡、日历日期标注").top(),
        "能力说明应排在标题下方",
    );
    for expected in ["卡片与标注", "自绘画布", "岛上常驻", "交互与数据", "页面交互"] {
        assert!(has_text(&expanded, expected), "{expected}");
    }
    let pinned_description = "开启常驻的插件在岛收起时一起显示，可在「页面管理 › 岛上显示」排序。同一插件可提供多张轮播画布。";
    assert!(has_text(&expanded, pinned_description));
    assert!(text_rect("岛上常驻").bottom() <= text_rect(pinned_description).top());
    click_in(&ctx, WINDOW, capabilities_title, |ui| draw(&mut host, ui));
    let collapsed = frame_in(&ctx, WINDOW, vec![], |ui| draw(&mut host, ui));
    assert!(!has_text(&collapsed, "卡片与标注"));

    // 复制出来的那段话带示例想法和校验命令。
    let _ = frame_in(&ctx, WINDOW, vec![], |ui| draw(&mut host, ui));
    let out = frame_in(&ctx, WINDOW, vec![], |ui| draw(&mut host, ui));
    let pos = out
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "复制这段话" => {
                Some(text.visual_bounding_rect().center())
            }
            _ => None,
        })
        .expect("复制按钮");
    let mut copied = String::new();
    for pressed in [true, false] {
        let out = frame_in(
            &ctx,
            WINDOW,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ],
            |ui| draw(&mut host, ui),
        );
        for command in out.platform_output.commands {
            if let egui::OutputCommand::CopyText(text) = command {
                copied = text;
            }
        }
    }
    assert!(copied.contains("在待办里显示离下次还信用卡还有几天"), "{copied}");
    assert!(copied.contains("--plugin-test plugin.json"), "{copied}");
    assert!(has_text(&frame_in(&ctx, WINDOW, vec![], |ui| draw(&mut host, ui)), "已复制"));

    click_in(&ctx, WINDOW, "已经做好了", |ui| draw(&mut host, ui));
    assert!(has_text(
        &frame_in(&ctx, WINDOW, vec![], |ui| draw(&mut host, ui)),
        "一句话交给 AI 改造"
    ));

    // 「从模板创建」的表单盖在教程上面；Esc 先关表单，再关教程。
    click_in(&ctx, WINDOW, "从模板创建", |ui| draw(&mut host, ui));
    let form = frame_in(&ctx, WINDOW, vec![], |ui| draw(&mut host, ui));
    assert!(has_text(&form, "选择位置并创建…"));
    assert!(has_text(&form, "做一个岛仔插件"));
    let id_format = "用英文句点分隔，每段以小写英文字母开头，只能用小写字母、数字和连字符，最多 64 个字符。";
    let uppercase_hint = "请将插件 ID 中的大写字母改成小写。";
    assert!(has_text(&form, id_format));
    assert!(!has_text(&form, uppercase_hint));

    click_in(&ctx, WINDOW, "例如 xiaoming.payday", |ui| draw(&mut host, ui));
    let uppercase = frame_in(
        &ctx,
        WINDOW,
        vec![egui::Event::Text("Walalala.TraePoints".into())],
        |ui| draw(&mut host, ui),
    );
    assert!(has_text(&uppercase, uppercase_hint));
    let corrected = frame_in(
        &ctx,
        WINDOW,
        vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            },
            egui::Event::Text("walalala.trae-points".into()),
        ],
        |ui| draw(&mut host, ui),
    );
    assert!(has_text(&corrected, "walalala.trae-points"));
    assert!(has_text(&corrected, id_format));
    assert!(!has_text(&corrected, uppercase_hint));

    let escape = || {
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }]
    };
    let _ = frame_in(&ctx, WINDOW, escape(), |ui| draw(&mut host, ui));
    let after = frame_in(&ctx, WINDOW, vec![], |ui| draw(&mut host, ui));
    assert!(!has_text(&after, "选择位置并创建…"));
    assert!(has_text(&after, "做一个岛仔插件"));
    let _ = frame_in(&ctx, WINDOW, escape(), |ui| draw(&mut host, ui));
    assert!(!has_text(
        &frame_in(&ctx, WINDOW, vec![], |ui| draw(&mut host, ui)),
        "做一个岛仔插件"
    ));
    let _ = fs::remove_dir_all(dir);
}
