use super::*;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
};

const PRINTER: &str = "fixture.bambu";
const OTHER: &str = "fixture.other-status";
const TASKS: &str = "fixture.tasks";
const TODAY: &str = "2026-11-22";
const SOURCE_STATUS: &str = "
    function refresh(i) {
        if (i.source.status !== 'ready') return {status:null};
        return JSON.parse(i.source.text);
    }";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "daozai-structured-status-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&root)?;
        Ok(Self(root))
    }

    fn host(&self) -> LocalPlugins {
        LocalPlugins::open(self.0.join("installed"))
    }

    fn plugin(
        &self,
        id: &str,
        owner: &str,
        capabilities: &[&str],
        permissions: Value,
        code: &str,
    ) -> Result<PathBuf> {
        let dir = self.0.join(id);
        fs::create_dir(&dir)?;
        fs::write(
            dir.join(COVER_FILE),
            include_bytes!("examples/payday/cover.png"),
        )?;
        fs::write(dir.join("main.js"), code)?;
        let path = dir.join("plugin.json");
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "id": id, "name": id, "author": "验收", "version": "1.0.0",
                "api": 1, "runtime": "js", "entry": "main.js", "owner": owner, "cover": COVER_FILE,
                "capabilities": capabilities, "dataVersion": 1, "permissions": permissions, "settings": []
            }))?,
        )?;
        Ok(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn wait(mut until: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(6);
    while !until() {
        assert!(Instant::now() < deadline, "等待结构化状态刷新超时");
        thread::sleep(Duration::from_millis(2));
    }
}

fn settle(host: &mut LocalPlugins, tasks_on: bool) {
    wait(|| {
        host.tick_features(tasks_on, false, false, TODAY);
        host.jobs.is_empty()
    });
}

fn status(host: &LocalPlugins, id: &str) -> (Option<f64>, StatusHealth) {
    let (_, snapshot, health) = host.status_page(id).expect("已启用的状态页应仍可打开");
    (snapshot.and_then(|snapshot| snapshot.progress_pct), health)
}

fn snapshot(progress: u8) -> String {
    json!({"status":{
        "state":"running", "label":"打印中", "progress_pct":progress,
        "metrics":[{"label":"喷嘴温度","value":"220 °C"},{"label":"层数","value":"74 / 180"}]
    }})
    .to_string()
}

#[test]
fn canvas_title_accent_must_be_a_proper_status_page_suffix() -> Result<()> {
    let mut manifest: Value = serde_json::from_str(include_str!("examples/canvas-demo/plugin.json"))?;
    let decode = |manifest: &Value| Manifest::decode(&manifest.to_string());
    assert!(decode(&manifest).is_ok());
    for invalid in [json!("画布"), json!("画布图元"), json!(""), json!(null)] {
        manifest["titleAccent"] = invalid;
        assert!(decode(&manifest).is_err());
    }
    manifest["titleAccent"] = json!("图元");
    manifest["owner"] = json!("tasks");
    manifest["capabilities"] = json!(["card"]);
    manifest.as_object_mut().unwrap().remove("statusLayout");
    assert!(decode(&manifest).is_err());
    Ok(())
}

#[test]
fn canvas_demo_assets_survive_install_and_market_round_trip() -> Result<()> {
    let fixture = Fixture::new()?;
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/canvas-demo/plugin.json");
    let bundle = fixture.0.join("canvas-demo.zip");
    market::pack(&path, &bundle)?;
    let mut zip = zip::ZipArchive::new(fs::File::open(bundle)?)?;
    assert!(zip.by_name("badge.svg").is_ok());
    let id = "example.canvas-demo";
    let mut host = fixture.host();
    host.install(&path)?;
    host.enable(id, true)?;
    drop(host);
    let mut host = fixture.host();
    settle(&mut host, false);
    let (_, snapshot, health) = host.status_page(id).unwrap();
    assert_eq!(health, StatusHealth::Ready);
    assert_eq!(snapshot.unwrap().canvas.as_ref().unwrap().height, 172.0);
    Ok(())
}

#[test]
fn canvas_invalid_image_is_reported_in_status_and_devtools() -> Result<()> {
    let fixture = Fixture::new()?;
    let id = "fixture.canvas";
    let path = fixture.plugin(id, "status", &["status.page", "status.canvas"], json!({}),
        "function refresh() {return {canvas:{height:50,nodes:[{image:'missing.svg',rect:[0,0,10,10]}]}};}",
    )?;
    let mut host = fixture.host();
    host.install(&path)?;
    host.enable(id, true)?;
    settle(&mut host, false);
    assert_eq!(status(&host, id), (None, StatusHealth::CanvasInvalid));
    assert!(host.devtools.logs.snapshot().iter().any(|line| line.contains("missing.svg")));
    Ok(())
}

#[test]
fn structured_status_layout_is_optional_and_preserved_in_author_packages() -> Result<()> {
    use crate::plugin_status::{ProgressVariant, StatusComponent, StatusDensity, StatusSurface};

    let fixture = Fixture::new()?;
    let path = fixture.plugin(
        PRINTER,
        "status",
        &["status.page"],
        json!({}),
        "function refresh() {return {status:{state:'running',label:'打印中',progress_pct:42}};}",
    )?;
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path)?)?;
    assert!(Manifest::decode(&manifest.to_string())?
        .status_layout
        .is_none());
    manifest["statusLayout"] = json!({
        "components":[{"type":"metrics"},{"type":"progress","variant":"bar"},{"type":"state"}],
        "style":{"accent":"#28a7DB","density":"compact","surface":"plain"}
    });
    fs::write(&path, serde_json::to_vec(&manifest)?)?;

    let bundle = fixture.0.join("status-layout.zip");
    market::pack(&path, &bundle)?;
    let mut zip = zip::ZipArchive::new(fs::File::open(bundle)?)?;
    let mut packed = String::new();
    zip.by_name("plugin.json")?.read_to_string(&mut packed)?;
    let layout = Manifest::decode(&packed)?.status_layout.unwrap();
    assert_eq!(
        layout.components,
        [
            StatusComponent::Metrics { columns: 2 },
            StatusComponent::Progress {
                variant: ProgressVariant::Bar
            },
            StatusComponent::State {},
        ]
    );
    assert_eq!(layout.style.accent_rgb(), Some([0x28, 0xa7, 0xdb]));
    assert_eq!(layout.style.density, StatusDensity::Compact);
    assert_eq!(layout.style.surface, StatusSurface::Plain);

    let mut host = fixture.host();
    host.install(&path)?;
    host.enable(PRINTER, true)?;
    assert_eq!(host.status_layout(PRINTER), Some(&layout));
    settle(&mut host, false);
    assert_eq!(status(&host, PRINTER), (Some(42.0), StatusHealth::Ready));
    let saved = host.path(PRINTER);
    drop(host);
    assert_eq!(fixture.host().status_layout(PRINTER), Some(&layout));

    let mut stored: Value = serde_json::from_slice(&fs::read(&saved)?)?;
    stored["package"]["manifest"]["statusLayout"]["components"] = json!([]);
    fs::write(saved, serde_json::to_vec(&stored)?)?;
    let host = fixture.host();
    assert!(host.invalid.contains(PRINTER));
    assert!(host.status_layout(PRINTER).is_none());
    assert!(host.status_page(PRINTER).is_none());
    Ok(())
}

#[test]
fn structured_status_layout_rejects_out_of_contract_manifests() -> Result<()> {
    use crate::plugin_status::{ProgressVariant, StatusComponent, StatusDensity, StatusSurface};

    let fixture = Fixture::new()?;
    let path = fixture.plugin(
        PRINTER,
        "status",
        &["status.page"],
        json!({}),
        "function refresh() {return {status:null};}",
    )?;
    let mut manifest: Value = serde_json::from_slice(&fs::read(path)?)?;
    for invalid in [
        Value::Null,
        json!({"components":[]}),
        json!({"components":vec![json!({"type":"alert"}); 7]}),
        json!({"components":[{"type":"progress","variant":"ring"},{"type":"progress","variant":"bar"}]}),
        json!({"components":[{"type":"state","text":"越权字段"}]}),
        json!({"components":[{"type":"progress","style":"ring"}]}),
        json!({"components":[{"type":"progress","variant":"dial"}]}),
        json!({"components":[{"type":"metrics","columns":3}]}),
        json!({"components":[{"type":"metrics","columns":null}]}),
        json!({"components":[{"type":"summary"}],"style":{"accent":"red"}}),
        json!({"components":[{"type":"summary"}],"style":{"accent":"#12+345"}}),
        json!({"components":[{"type":"summary"}],"style":{"accent":null}}),
        json!({"components":[{"type":"summary"}],"style":{"surface":"custom"}}),
        json!({"components":[{"type":"summary"}],"style":{"font":"自定字体"}}),
        json!({"components":[{"type":"summary"}],"style":null}),
        json!({"components":[{"type":"summary"}],"width":300}),
    ] {
        manifest["statusLayout"] = invalid;
        assert!(
            Manifest::decode(&manifest.to_string()).is_err(),
            "误接受：{}",
            manifest["statusLayout"]
        );
    }

    manifest["statusLayout"] = json!({"components":[{"type":"progress"}]});
    let layout = Manifest::decode(&manifest.to_string())?
        .status_layout
        .unwrap();
    assert_eq!(
        layout.components,
        [StatusComponent::Progress {
            variant: ProgressVariant::Ring
        }]
    );
    assert_eq!(layout.style.density, StatusDensity::Comfortable);
    assert_eq!(layout.style.surface, StatusSurface::Card);
    assert_eq!(layout.style.accent_rgb(), None);

    manifest["owner"] = json!("tasks");
    manifest["capabilities"] = json!(["card"]);
    assert!(Manifest::decode(&manifest.to_string()).is_err());
    manifest.as_object_mut().unwrap().remove("statusLayout");
    assert!(Manifest::decode(&manifest.to_string()).is_ok());
    Ok(())
}

#[test]
fn structured_status_file_disconnect_and_revoke_are_isolated() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut host = fixture.host();
    for id in [PRINTER, OTHER] {
        host.install(&fixture.plugin(
            id,
            "status",
            &["status.page", "timer"],
            json!({"source.read":"file"}),
            SOURCE_STATUS,
        )?)?;
        host.enable(id, true)?;
        assert_eq!(status(&host, id), (None, StatusHealth::Unconfigured));
    }
    host.install(&fixture.plugin(
        TASKS,
        "tasks",
        &["card"],
        json!({}),
        "function refresh() {return {card:'原有待办卡片'};}",
    )?)?;
    host.enable(TASKS, true)?;
    let printer = fixture.0.join("模拟打印机.json");
    let other = fixture.0.join("其他来源.json");
    fs::write(&printer, snapshot(42))?;
    fs::write(&other, snapshot(25))?;
    host.set_source_grant(PRINTER, Some(source_read::grant_file(&printer)?))?;
    host.set_source_grant(OTHER, Some(source_read::grant_file(&other)?))?;
    settle(&mut host, true);
    assert_eq!(status(&host, PRINTER), (Some(42.0), StatusHealth::Ready));
    assert_eq!(status(&host, OTHER), (Some(25.0), StatusHealth::Ready));
    assert_eq!(host.cards[TASKS], "原有待办卡片");

    fs::remove_file(&printer)?;
    fs::write(&other, snapshot(63))?;
    host.next.remove(PRINTER);
    host.next.remove(OTHER);
    settle(&mut host, true);
    assert_eq!(status(&host, PRINTER), (None, StatusHealth::Offline));
    assert_eq!(status(&host, OTHER), (Some(63.0), StatusHealth::Ready));
    assert_eq!(host.cards[TASKS], "原有待办卡片");

    fs::write(&printer, snapshot(58))?;
    host.next.remove(PRINTER);
    settle(&mut host, true);
    assert_eq!(status(&host, PRINTER), (Some(58.0), StatusHealth::Ready));
    host.set_source_grant(PRINTER, None)?;
    assert_eq!(status(&host, PRINTER), (None, StatusHealth::Unconfigured));
    settle(&mut host, true);
    assert!(!host.next.contains_key(PRINTER));
    assert_eq!(status(&host, OTHER), (Some(63.0), StatusHealth::Ready));
    assert_eq!(host.status_pages().count(), 2);
    Ok(())
}

#[test]
fn stopping_the_status_page_cancels_an_in_flight_read_and_persists() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut host = fixture.host();
    host.install(&fixture.plugin(
        PRINTER,
        "status",
        &["status.page", "timer", "storage"],
        json!({"source.read":"loopback"}),
        SOURCE_STATUS,
    )?)?;
    host.install(&fixture.plugin(
        OTHER,
        "status",
        &["status.page"],
        json!({}),
        "function refresh() {return {status:{state:'idle',label:'另一个插件',progress_pct:17}};}",
    )?)?;
    host.enable(PRINTER, true)?;
    host.enable(OTHER, true)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}/status", listener.local_addr()?);
    host.set_source_grant(PRINTER, Some(source_read::grant_url(&url)?))?;
    let (received, pending) = mpsc::channel();
    let (resume, hold) = mpsc::channel();
    let server = thread::spawn(move || -> std::io::Result<TcpListener> {
        let mut connection = None;
        wait(|| match listener.accept() {
            Ok((stream, _)) => {
                connection = Some(stream);
                true
            }
            Err(_) => false,
        });
        let mut stream = connection.unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        assert_eq!(line.trim_end(), "GET /status HTTP/1.1");
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 || line == "\r\n" {
                break;
            }
        }
        received.send(()).map_err(std::io::Error::other)?;
        hold.recv_timeout(Duration::from_secs(5))
            .map_err(std::io::Error::other)?;
        let payload = json!({
            "status":{"state":"succeeded","label":"迟到的更新","progress_pct":100},
            "storage":{"should_not_persist":true}
        })
        .to_string();
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        );
        Ok(listener)
    });

    host.tick_features(false, false, false, TODAY);
    pending.recv_timeout(Duration::from_secs(3))?;
    assert!(host.jobs.contains_key(PRINTER));
    host.set_status_page_enabled(PRINTER, false)?;
    assert!(host.status_page(PRINTER).is_none());
    assert!(!host.records[PRINTER].enabled);
    assert_eq!(host.status_pages().collect::<Vec<_>>(), [(OTHER, OTHER)]);
    resume.send(())?;
    let listener = server.join().map_err(|_| fail("测试回环服务退出"))??;
    settle(&mut host, false);
    assert!(host.records[PRINTER].storage.is_null());
    assert!(!host.status.contains_key(PRINTER));
    assert_eq!(status(&host, OTHER), (Some(17.0), StatusHealth::Ready));
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );

    drop(host);
    let mut host = fixture.host();
    settle(&mut host, false);
    assert!(!host.records[PRINTER].enabled);
    assert!(host.status_page(PRINTER).is_none());
    assert_eq!(status(&host, OTHER), (Some(17.0), StatusHealth::Ready));
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
    drop(listener);

    host.set_status_page_enabled(PRINTER, true)?;
    settle(&mut host, false);
    assert_eq!(status(&host, PRINTER), (None, StatusHealth::Offline));
    host.enable(PRINTER, false)?;
    assert!(host.status_page(PRINTER).is_none());
    assert_eq!(status(&host, OTHER), (Some(17.0), StatusHealth::Ready));
    Ok(())
}

#[test]
fn structured_status_discards_old_data_and_rejects_other_owners() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut host = fixture.host();
    host.install(&fixture.plugin(
        PRINTER,
        "status",
        &["status.page", "timer"],
        json!({"source.read":"file"}),
        SOURCE_STATUS,
    )?)?;
    let file = fixture.0.join("模拟打印机.json");
    fs::write(
        &file,
        r#"{"status":{"state":"idle","label":"旧数据","expires_at":1}}"#,
    )?;
    host.enable(PRINTER, true)?;
    host.set_source_grant(PRINTER, Some(source_read::grant_file(&file)?))?;
    settle(&mut host, false);
    assert_eq!(status(&host, PRINTER), (None, StatusHealth::Stale));

    fs::write(&file, snapshot(35))?;
    host.next.remove(PRINTER);
    settle(&mut host, false);
    assert_eq!(status(&host, PRINTER), (Some(35.0), StatusHealth::Ready));
    fs::write(&file, snapshot(101))?;
    host.next.remove(PRINTER);
    settle(&mut host, false);
    assert_eq!(status(&host, PRINTER), (None, StatusHealth::Invalid));
    fs::write(&file, r#"{"status":null}"#)?;
    host.next.remove(PRINTER);
    settle(&mut host, false);
    assert_eq!(status(&host, PRINTER), (None, StatusHealth::Waiting));

    let foreign = fixture.plugin(
        "fixture.foreign-status",
        "tasks",
        &["card"],
        json!({}),
        "function refresh(){return {card:'原有卡片',status:{state:'idle',label:'伪造状态'}};}",
    )?;
    assert!(Package::read(&foreign)?
        .render(&Values::new(), &Value::Null, TODAY)
        .is_err());
    let unauthorized = fixture.plugin(
        "fixture.no-status-capability",
        "status",
        &["card"],
        json!({}),
        "function refresh(){return {card:'状态'};}",
    )?;
    assert!(Package::read(&unauthorized).is_err());
    Ok(())
}

#[test]
fn printer_status_example_installs_and_returns_all_new_components() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut host = fixture.host();
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/printer-status/plugin.json");
    let id = "example.printer-status";
    host.install(&path)?;
    host.enable(id, true)?;
    settle(&mut host, false);
    let (_, snapshot, health) = host.status_page(id).unwrap();
    assert_eq!(health, StatusHealth::Ready);
    let snapshot = snapshot.unwrap();
    assert_eq!(snapshot.items.len(), 2);
    assert_eq!(snapshot.series.as_ref().unwrap().len(), 5);
    assert!(snapshot.note.as_ref().unwrap().contains('\n'));
    assert_eq!(snapshot.tags.len(), 3);
    assert_eq!(host.status_layout(id).unwrap().components.len(), 8);
    Ok(())
}

#[test]
fn realtime_examples_install_and_return_bounded_host_values() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut host = fixture.host();
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples");
    for (folder, id) in [
        ("live-pay", "example.live-pay"),
        ("world-clock", "example.world-clock"),
    ] {
        let path = examples.join(folder).join("plugin.json");
        market::pack(&path, &fixture.0.join(format!("{folder}.zip")))?;
        host.install(&path)?;
        host.enable(id, true)?;
    }
    host.set_pinned("example.live-pay", true)?;
    settle(&mut host, false);

    let pin = host.pinned_displays(false).0.into_iter().next()
        .expect("工资样例应提供常驻倒计时");
    let (_, snapshot, health) = host.status_page("example.live-pay").unwrap();
    assert_eq!(health, StatusHealth::Ready);
    let salary = snapshot.unwrap();
    assert!(salary.counter[0].sensitive);
    assert_eq!(salary.countdown.len(), 1);
    assert_eq!(pin.text, salary.countdown[0].label);
    assert!(pin.value.is_some_and(|text| !text.contains('¥')));

    let (_, snapshot, health) = host.status_page("example.world-clock").unwrap();
    assert_eq!(health, StatusHealth::Ready);
    assert_eq!(snapshot.unwrap().clock.len(), 4);
    Ok(())
}


fn run_realtime_sample(package: &Package, mut input: Value) -> Result<Output> {
    let invocation = Call::Refresh.prepare(&mut input);
    let result = devtools::run_script(
        package.source.as_bytes(),
        &package.manifest.entry,
        input.to_string(),
        invocation,
        &mut Vec::new(),
    )?;
    let result: Output = serde_json::from_str(&result)?;
    assert!(result.status.as_ref().is_some_and(PluginStatus::valid));
    assert!(result.pinned.as_ref().is_none_or(pinned::Pinned::valid));
    Ok(result)
}

#[test]
fn realtime_pay_uses_local_work_hours_and_schedules_non_sensitive_pins() -> Result<()> {
    use crate::plugin_status::StatusState;

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/live-pay/plugin.json");
    let package = Package::read(&path)?;
    let settings = package.settings_values(&Values::new());
    let timestamp = |instant: &str| instant.parse::<chrono::DateTime<chrono::Utc>>().unwrap();
    let mut input = json!({"settings":settings,"tz":{"offset_minutes":480,"name":"Asia/Shanghai"}});
    for (instant, state, label, wages, countdown, next_refresh) in [
        (
            "2026-11-23T00:30:00Z",
            StatusState::Idle,
            "距上班",
            "¥0.00",
            "0天 00:30",
            "2026-11-23T01:00:00Z",
        ),
        (
            "2026-11-23T02:00:00Z",
            StatusState::Running,
            "距下班",
            "¥52.91",
            "08:00:00",
            "2026-11-23T10:00:00Z",
        ),
        (
            "2026-11-23T11:00:00Z",
            StatusState::Idle,
            "距上班",
            "¥476.19",
            "0天 14:00",
            "2026-11-23T16:00:00Z",
        ),
    ] {
        let now = timestamp(instant);
        input["now"] = json!(now.timestamp_millis());
        let result = run_realtime_sample(&package, input.clone())?;
        let status = result.status.unwrap();
        assert_eq!(status.state, state);
        assert_eq!(status.countdown[0].label, label);
        assert_eq!(status.countdown[0].value_at(now.timestamp()), countdown);
        assert_eq!(status.counter[0].value_at(now.timestamp()), wages);
        assert!(status.counter[0].sensitive);
        assert!(result.pinned.is_some());
        assert_eq!(
            result.next_refresh_at,
            Some(timestamp(next_refresh).timestamp())
        );
    }

    input["settings"]["days"] = json!("");
    let result = run_realtime_sample(&package, input)?;
    assert_eq!(
        result.status.unwrap().summary.as_deref(),
        Some("请先选择工作日")
    );
    assert!(result.pinned.is_none() && result.next_refresh_at.is_none());
    Ok(())
}

#[test]
fn world_clock_uses_ordered_city_settings_and_host_daylight_saving() -> Result<()> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/world-clock/plugin.json");
    let package = Package::read(&path)?;
    let settings = package.settings_values(&Values::from([(
        "cities".into(),
        r#"[{"label":"纽约","zone":"America/New_York"},{"label":"北京","zone":"Asia/Shanghai"},{"label":"待检查","zone":"Example/Unknown"}]"#.into(),
    )]));
    for (iso, new_york) in [
        ("2026-01-10T12:00:00Z", "07:00:00"),
        ("2026-07-10T12:00:00Z", "08:00:00"),
    ] {
        let now = iso.parse::<chrono::DateTime<chrono::Utc>>()?;
        let mut input = json!({"settings":settings});
        script_settings::time_input(
            &mut input,
            now,
            package.manifest.timezones,
            &package.manifest.settings,
            &settings,
        );
        let status = run_realtime_sample(&package, input)?.status.unwrap();
        assert_eq!(status.clock.len(), 2);
        assert_eq!(status.clock[0].label, "纽约");
        assert_eq!(status.clock[0].value_at(now.timestamp()), new_york);
        assert_eq!(status.clock[1].label, "北京");
        assert_eq!(status.clock[1].value_at(now.timestamp()), "20:00:00");
        assert!(status
            .summary
            .as_deref()
            .unwrap()
            .contains("1 个时区无法识别"));
    }
    Ok(())
}
