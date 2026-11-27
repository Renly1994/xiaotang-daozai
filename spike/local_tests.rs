use super::*;
fn settle(host: &mut LocalPlugins, today: &str) {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        host.tick(true, true, today);
        if host.jobs.is_empty() {
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn write_shortcut_package(dir: &Path, id: &str, version: &str, colors: Value) -> Result<PathBuf> {
    fs::create_dir_all(dir)?;
    fs::write(dir.join(COVER_FILE), include_bytes!("examples/payday/cover.png"))?;
    fs::write(
        dir.join(PAGE_SHORTCUT_ENTRY),
        serde_json::to_vec(&json!({ "colors": colors }))?,
    )?;
    let path = dir.join("plugin.json");
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "id": id, "name": "快捷切换", "author": "示例", "version": version,
            "api": 1, "runtime": "assets", "entry": PAGE_SHORTCUT_ENTRY,
            "cover": COVER_FILE, "owner": "pages", "capabilities": ["navigation.shortcuts"],
            "dataVersion": 1, "permissions": {}, "settings": [],
        }))?,
    )?;
    Ok(path)
}

#[test]
fn one_plugin_fills_a_tasks_card_and_a_calendar_marker_under_separate_switches() -> Result<()> {
    const DAY: &str = "2026-11-23";
    const NEXT: &str = "2026-11-24";
    let root = std::env::temp_dir().join(format!(
        "daozai-surfaces-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let dir = root.join("package");
    fs::create_dir_all(&dir)?;
    fs::write(dir.join(COVER_FILE), include_bytes!("examples/payday/cover.png"))?;
    fs::write(
        dir.join("main.js"),
        "function refresh(i){return {card:'发薪 '+i.today, marker:i.today==='2026-11-23'?'发薪日':''}}",
    )?;
    let manifest = json!({
        "id": "example.payday-marker", "name": "发薪日", "author": "示例", "version": "1.0.0",
        "api": 1, "runtime": "js", "entry": "main.js", "cover": COVER_FILE, "owner": "tasks",
        "capabilities": ["card", "calendar.marker", "timer"], "dataVersion": 1,
        "permissions": {}, "settings": [],
    });
    let path = dir.join("plugin.json");
    fs::write(&path, serde_json::to_vec(&manifest)?)?;
    for (field, value) in [
        ("capabilities", json!(["calendar.marker"])),
        ("capabilities", json!(["card", "card"])),
        ("capabilities", json!(["card", "network"])),
        ("owner", json!("appearance")),
    ] {
        let mut invalid = manifest.clone();
        invalid[field] = value;
        assert!(Manifest::decode(&invalid.to_string()).is_err(), "{field} {}", invalid[field]);
    }

    let id = "example.payday-marker";
    let mut host = LocalPlugins::open(root.join("installed"));
    host.install(&path)?;
    host.enable(id, true)?;
    assert!(host.calendar_annotation_enabled());
    settle(&mut host, DAY);
    assert_eq!(host.cards.get(id).map(String::as_str), Some("发薪 2026-11-23"));
    assert_eq!(host.calendar_annotation(DAY), Some("发薪日"));

    // 关日程只撤下日历标注，待办卡片照常更新。
    host.tick(true, false, DAY);
    assert_eq!(host.calendar_annotation(DAY), None);
    assert!(host.has_card());
    host.next.clear();
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        host.tick(true, false, DAY);
        if host.jobs.is_empty() && !host.next.is_empty() {
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(host.markers.is_empty() && host.has_card());

    // 再开日程，不用等定时刷新就补回标注。
    settle(&mut host, DAY);
    assert_eq!(host.calendar_annotation(DAY), Some("发薪日"));

    // 换日后旧标注作废，新的一天没有标注时也不残留。
    host.tick(true, true, NEXT);
    assert_eq!(host.calendar_annotation(NEXT), None);
    settle(&mut host, NEXT);
    assert_eq!(host.calendar_annotation(NEXT), None);
    assert_eq!(host.cards.get(id).map(String::as_str), Some("发薪 2026-11-24"));

    // 关掉所属的待办模块，整个插件暂停，两处输出都撤下。
    host.tick(false, true, NEXT);
    assert!(!host.has_card());
    assert!(host.next.get(id).is_none() && host.jobs.is_empty());
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn page_shortcuts_are_data_only_and_follow_the_installed_switch() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "daozai-shortcuts-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let (first, second) = ("example.first", "example.second");
    let path = write_shortcut_package(&root.join("first"), first, "1.0.0", json!({"music":"#FA3E72"}))?;
    let other = write_shortcut_package(&root.join("second"), second, "1.0.0", json!({"weather":"#4c9be8"}))?;
    let entry = root.join("first").join(PAGE_SHORTCUT_ENTRY);
    for invalid in [
        json!({}),
        json!({"colors": {}}),
        json!({"colors": {"music":"#abc"}}),
        json!({"colors": {"music": [250, 62, 114]}}),
        json!({"colors": {"unknown":"#FA3E72"}}),
        json!({"colors": {"music":"#FA3E72"}, "command": "refresh"}),
    ] {
        fs::write(&entry, serde_json::to_vec(&invalid)?)?;
        assert!(Package::read(&path).is_err(), "{invalid}");
    }
    fs::write(&entry, r##"{"colors":{"music":"#FA3E72"}}"##)?;
    let manifest: Value = serde_json::from_slice(&fs::read(&path)?)?;
    for (field, value) in [
        ("owner", json!("tasks")),
        ("entry", json!("nested/shortcuts.json")),
        ("permissions", json!({"screen.backdrop": true})),
        ("capabilities", json!(["navigation.shortcuts", "timer"])),
        ("refreshSeconds", json!(61)),
    ] {
        let mut invalid = manifest.clone();
        invalid[field] = value;
        assert!(Manifest::decode(&invalid.to_string()).is_err(), "{field}");
    }

    let storage = root.join("installed");
    let mut host = LocalPlugins::open(storage.clone());
    host.install(&path)?;
    host.install(&other)?;
    assert!(host.active_page_shortcut_colors().is_none());
    host.enable(first, true)?;
    assert_eq!(host.active_page_shortcut_colors().unwrap()["music"], [250, 62, 114]);
    assert!(host.enable(second, true).is_err());
    assert!(!host.records[second].enabled);
    host.tick(true, true, "2026-11-22");
    assert!(host.jobs.is_empty());

    let mut host = LocalPlugins::open(storage.clone());
    assert_eq!(host.active_page_shortcut_colors().unwrap()["music"], [250, 62, 114]);
    host.enable(first, false)?;
    assert!(host.active_page_shortcut_colors().is_none());
    host.enable(second, true)?;
    assert_eq!(host.active_page_shortcut_colors().unwrap()["weather"], [76, 155, 232]);
    host.uninstall(second, true)?;
    assert!(host.active_page_shortcut_colors().is_none());
    host.install(&other)?;
    host.enable(second, true)?;
    host.uninstall(second, false)?;
    assert!(host.active_page_shortcut_colors().is_none());

    host.enable(first, true)?;
    let record_path = host.path(first);
    let mut tampered: Value = serde_json::from_slice(&fs::read(&record_path)?)?;
    tampered["package"]["source"] = json!(r##"{"colors":{"music":"#NOPE"}}"##);
    fs::write(record_path, serde_json::to_vec(&tampered)?)?;
    let host = LocalPlugins::open(storage);
    assert!(host.invalid.contains(first));
    assert!(host.active_page_shortcut_colors().is_none());
    fs::remove_dir_all(root)?;
    Ok(())
}

#[cfg(windows)]
mod test_market {
    use super::*;
    use crate::sdk_spike::local::market_state::MarketStatus;

    const ID: &str = "example.payday";

    struct Fixture {
        root: PathBuf,
        manifest_path: PathBuf,
        index: PathBuf,
    }

    impl Fixture {
        fn new() -> Result<Self> {
            let root = std::env::temp_dir().join(format!(
                "daozai-market-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            ));
            let source = root.join("source");
            let market_dir = root.join("market");
            fs::create_dir_all(&source)?;
            fs::create_dir_all(&market_dir)?;
            let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/payday");
            let manifest_path = source.join("plugin.json");
            fs::copy(sample.join("plugin.json"), &manifest_path)?;
            fs::copy(sample.join("main.js"), source.join("main.js"))?;
            fs::copy(sample.join(COVER_FILE), source.join(COVER_FILE))?;
            Ok(Self {
                root,
                manifest_path,
                index: market_dir.join("catalog.json"),
            })
        }

        fn publish(&self, version: &str, incompatible: bool) -> Result<()> {
            let mut manifest: Value = serde_json::from_slice(&fs::read(&self.manifest_path)?)?;
            manifest["version"] = json!(version);
            if incompatible {
                manifest["settings"][0]["type"] = json!("date");
                manifest["settings"][0]["default"] = json!("2026-12-01");
            }
            fs::write(&self.manifest_path, serde_json::to_vec(&manifest)?)?;
            let zip = self.root.join(format!("submit-{version}.zip"));
            market::pack(&self.manifest_path, &zip)?;
            market::submit(&zip, &self.index)
        }

        fn install(&self, host: &mut LocalPlugins) -> Result<()> {
            self.publish("1.0.0", false)?;
            host.set_market_catalog(&self.index)?;
            host.market_action(false, ID)?;
            settle_market(host);
            assert_eq!(status(host), MarketStatus::Installed);
            assert!(!host.records[ID].enabled);
            Ok(())
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn settle_market(host: &mut LocalPlugins) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while host.market_pending() {
            host.poll_market();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn status(host: &LocalPlugins) -> MarketStatus {
        let catalog = host.market_catalog(false).unwrap();
        host.market_status(&catalog, &catalog.entries[0])
    }

    #[test]
    fn market_can_package_install_and_update_page_shortcut_assets() -> Result<()> {
        let fixture = Fixture::new()?;
        let id = "example.shortcuts";
        let mut host = LocalPlugins::open(fixture.root.join("installed"));
        for (version, color, expected) in [
            ("1.0.0", "#FA3E72", [250, 62, 114]),
            ("1.1.0", "#4C9BE8", [76, 155, 232]),
        ] {
            let path = write_shortcut_package(
                &fixture.root.join("shortcut-source"),
                id,
                version,
                json!({"music": color}),
            )?;
            let zip = fixture.root.join(format!("shortcuts-{version}.zip"));
            assert!(market::validate_project(&path)?.contains(id));
            market::pack(&path, &zip)?;
            market::submit(&zip, &fixture.index)?;
            host.set_market_catalog(&fixture.index)?;
            host.market_action(false, id)?;
            settle_market(&mut host);
            assert_eq!(status(&host), MarketStatus::Installed, "{}", host.notice);
            if version == "1.0.0" {
                assert!(host.active_page_shortcut_colors().is_none());
                host.enable(id, true)?;
            }
            assert_eq!(host.active_page_shortcut_colors().unwrap()["music"], expected);
            assert!(host.records[id].enabled);
        }
        Ok(())
    }

    #[test]
    fn market_favorites_survive_a_restart_and_delisting() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.publish("1.0.0", false)?;
        let entry = market::Catalog::open(&fixture.index)?.entries.remove(0);
        let storage = fixture.root.join("installed");
        let mut host = LocalPlugins::open(storage.clone());
        assert!(host.favorites().is_empty());
        host.set_favorite(&entry, true)?;
        host.set_favorite(&entry, true)?;
        assert!(host.is_favorite(ID));
        assert_eq!(host.favorites().len(), 1);

        // 删除在线目录后，收藏仍可显示名称并取消。
        fs::remove_file(&fixture.index)?;
        let mut host = LocalPlugins::open(storage.clone());
        assert_eq!(host.favorites().get(ID), Some(&entry.name));
        host.remove_favorite(ID)?;
        assert!(LocalPlugins::open(storage).favorites().is_empty());
        Ok(())
    }

    #[test]
    fn market_install_update_restart_and_reinstall_keep_data() -> Result<()> {
        let fixture = Fixture::new()?;
        let storage = fixture.root.join("installed");
        let mut host = LocalPlugins::open(storage.clone());
        fixture.install(&mut host)?;
        let previous_index = fs::read(&fixture.index)?;
        host.enable(ID, true)?;
        let mut values = host.records[ID].values.clone();
        values.insert("title".into(), "我的发薪日".into());
        host.save(ID, values.clone())?;
        settle(&mut host, "2026-12-01");
        let saved = host.records[ID].storage.clone();
        assert!(!saved.is_null());
        host.tick(false, true, "2026-12-02");
        assert!(host.records[ID].enabled && !host.has_card());

        // 无封面的旧快照从磁盘恢复，在线更新后随包保存封面。
        let mut old: Value = serde_json::from_slice(&fs::read(host.path(ID))?)?;
        let snapshot = old["package"].as_object_mut().unwrap();
        snapshot.remove("cover");
        snapshot
            .get_mut("manifest")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("cover");
        fs::write(host.path(ID), serde_json::to_vec(&old)?)?;
        let mut host = LocalPlugins::open(storage.clone());
        assert!(host.records[ID]
            .package
            .as_ref()
            .unwrap()
            .cover_png()
            .is_none());
        assert!(host.records[ID].enabled);
        assert_eq!(host.records[ID].values, values);

        fixture.publish("1.1.0", false)?;
        host.refresh_market_catalog()?;
        assert_eq!(status(&host), MarketStatus::Update);
        host.tick(true, true, "2026-12-02");
        host.market_action(false, ID)?;
        settle_market(&mut host);
        assert!(host.jobs[ID].cancelled.load(Ordering::Acquire), "上个版本的迟到结果应作废");
        assert_eq!(host.records[ID].storage, saved);
        assert_eq!(status(&host), MarketStatus::Installed);
        assert!(host.records[ID].enabled);
        assert_eq!(host.records[ID].values, values);
        settle(&mut host, "2026-12-01");

        let mut host = LocalPlugins::open(storage);
        assert_eq!(status(&host), MarketStatus::Installed);
        assert_eq!(host.records[ID].values, values);
        assert_eq!(host.records[ID].storage, saved);
        assert!(host.records[ID].enabled);
        assert_eq!(
            host.records[ID]
                .package
                .as_ref()
                .unwrap()
                .cover_png()
                .as_deref(),
            Some(include_bytes!("examples/payday/cover.png").as_slice())
        );
        host.uninstall(ID, true)?;
        host.market_action(false, ID)?;
        settle_market(&mut host);
        assert_eq!(host.records[ID].values, values);
        assert_eq!(host.records[ID].storage, saved);
        assert!(!host.records[ID].enabled);

        fs::write(&fixture.index, previous_index)?;
        host.refresh_market_catalog()?;
        assert_eq!(status(&host), MarketStatus::Unavailable);
        assert!(host.market_action(false, ID).is_err());
        Ok(())
    }

    #[test]
    fn lunar_update_preserves_state_and_follows_only_the_calendar_switch() -> Result<()> {
        const LUNAR: &str = "daozai.lunar-today";
        const TODAY: &str = "2026-11-21";
        const TOMORROW: &str = "2026-11-22";
        let fixture = Fixture::new()?;
        let installed = fixture.root.join("installed");
        let mut host = LocalPlugins::open(installed.clone());
        let mut manifest: Value = serde_json::from_slice(&fs::read(&fixture.manifest_path)?)?;
        manifest["id"] = json!(LUNAR);
        manifest["name"] = json!("今日农历");
        fs::write(&fixture.manifest_path, serde_json::to_vec(&manifest)?)?;
        fs::write(
            fixture.manifest_path.with_file_name("main.js"),
            "function refresh(i){return {card:i.today,storage:i.storage}}",
        )?;
        fixture.publish("1.0.0", false)?;
        host.set_market_catalog(&fixture.index)?;
        host.market_action(false, LUNAR)?;
        settle_market(&mut host);
        host.enable(LUNAR, true)?;
        let mut record = host.installed(LUNAR)?;
        record.values.insert("title".into(), "自定义标题".into());
        record.storage = json!({"saved": true});
        host.put(LUNAR, record.clone())?;
        host.tick(true, true, TODAY);
        assert!(host.jobs.is_empty());
        assert!(host.records[LUNAR].enabled);
        assert!(!host.has_card() && !host.calendar_annotation_enabled());

        manifest["owner"] = json!("calendar");
        fs::write(&fixture.manifest_path, serde_json::to_vec(&manifest)?)?;
        fixture.publish("1.0.1", false)?;
        host.refresh_market_catalog()?;
        assert_eq!(status(&host), MarketStatus::Update);
        host.market_action(false, LUNAR)?;
        settle_market(&mut host);
        assert_eq!(status(&host), MarketStatus::Installed);
        let mut host = LocalPlugins::open(installed);
        let migrated = host.installed(LUNAR)?;
        assert!(migrated.enabled);
        assert_eq!(migrated.origin, record.origin);
        assert_eq!(migrated.values, record.values);
        assert_eq!(migrated.storage, record.storage);
        assert_eq!(migrated.package.unwrap().manifest.owner, "calendar");

        host.tick(true, false, TODAY);
        assert!(host.jobs.is_empty());
        assert!(host.calendar_annotation_enabled());
        assert_eq!(host.calendar_annotation(TODAY), None);
        settle(&mut host, TODAY);
        assert_eq!(host.calendar_annotation(TODAY), Some(TODAY));
        assert!(!host.has_card());

        let sample =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/payday/plugin.json");
        host.install(&sample)?;
        host.enable(ID, true)?;
        settle(&mut host, TODAY);
        assert!(host.has_card());
        host.pause_owner("calendar");
        assert!(host.has_card() && host.calendar_annotation_enabled());
        assert_eq!(host.calendar_annotation(TODAY), None);
        settle(&mut host, TODAY);
        host.tick(false, true, TODAY);
        assert!(!host.has_card());
        assert_eq!(host.calendar_annotation(TODAY), Some(TODAY));

        assert_eq!(host.calendar_annotation(TOMORROW), None);
        host.tick(false, true, TOMORROW);
        assert_eq!(host.calendar_annotation(TODAY), None);
        settle(&mut host, TOMORROW);
        assert_eq!(host.calendar_annotation(TOMORROW), Some(TOMORROW));
        host.enable(LUNAR, false)?;
        assert!(!host.calendar_annotation_enabled());
        assert_eq!(host.calendar_annotation(TOMORROW), None);
        Ok(())
    }

    #[test]
    fn market_bad_or_cancelled_update_never_replaces_installed_record() -> Result<()> {
        let fixture = Fixture::new()?;
        let mut host = LocalPlugins::open(fixture.root.join("installed"));
        fixture.install(&mut host)?;
        host.enable(ID, true)?;
        let previous = fs::read(host.path(ID))?;

        fixture.publish("1.1.0", false)?;
        let zip = fixture
            .index
            .with_file_name(format!("plugin-{ID}-1.1.0.zip"));
        let mut bytes = fs::read(&zip)?;
        bytes[0] ^= 1;
        fs::write(&zip, bytes)?;
        host.refresh_market_catalog()?;
        host.market_action(false, ID)?;
        settle_market(&mut host);
        assert!(host.notice.contains("更新失败"));
        assert_eq!(fs::read(host.path(ID))?, previous);

        fixture.publish("1.2.0", true)?;
        host.refresh_market_catalog()?;
        host.market_action(false, ID)?;
        settle_market(&mut host);
        assert!(host.notice.contains("字段类型"));
        assert_eq!(fs::read(host.path(ID))?, previous);

        host.market_action(false, ID)?;
        host.enable(ID, false)?;
        let disabled = fs::read(host.path(ID))?;
        settle_market(&mut host);
        assert!(host.notice.contains("插件状态已变化"));
        assert_eq!(fs::read(host.path(ID))?, disabled);
        assert!(!host.records[ID].enabled);

        let mut manifest: Value = serde_json::from_slice(&fs::read(&fixture.manifest_path)?)?;
        manifest["owner"] = json!("calendar");
        fs::write(&fixture.manifest_path, serde_json::to_vec(&manifest)?)?;
        fixture.publish("1.3.0", false)?;
        host.refresh_market_catalog()?;
        host.market_action(false, ID)?;
        settle_market(&mut host);
        assert!(host.notice.contains("更新失败"));
        assert_eq!(fs::read(host.path(ID))?, disabled);
        Ok(())
    }

    #[test]
    fn official_market_updates_from_its_mirror_and_keeps_verified_state_on_failure() -> Result<()> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        use std::{
            io::{BufRead, BufReader, Cursor, Write},
            net::TcpListener,
        };

        let fixture = Fixture::new()?;
        fixture.publish("1.0.0", false)?;
        let mut first: Value = serde_json::from_slice(&fs::read(&fixture.index)?)?;
        let avatar_name = "avatar-author.png";
        first["plugins"][0]["presentation"] = json!({
            "official": true,
            "sample": true,
            "maintainer": "renly1994",
            "author_avatar": avatar_name
        });
        let first = serde_json::to_vec(&first)?;
        let mut avatar = Cursor::new(Vec::new());
        image::RgbaImage::new(1, 1).write_to(&mut avatar, image::ImageFormat::Png)?;
        let avatar = avatar.into_inner();
        let first_name = format!("plugin-{ID}-1.0.0.zip");
        let first_zip = fs::read(fixture.index.with_file_name(&first_name))?;
        fixture.publish("1.1.0", false)?;
        let second = fs::read(&fixture.index)?;
        let second_name = format!("plugin-{ID}-1.1.0.zip");
        let second_zip = fs::read(fixture.index.with_file_name(&second_name))?;
        let mut corrupt = second_zip.clone();
        corrupt[0] ^= 1;

        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let index_url = url::Url::parse(&format!(
            "http://{}/api/v5/repos/owner/plugins/contents/preview/catalog.json?ref=master",
            listener.local_addr()?
        ))?;
        let replies = [
            ("catalog.json", first, false),
            (avatar_name, avatar.clone(), false),
            (first_name.as_str(), first_zip.clone(), false),
            (first_name.as_str(), first_zip, false),
            ("catalog.json", second.clone(), false),
            (second_name.as_str(), corrupt, false),
            (second_name.as_str(), second_zip, false),
            ("catalog.json", Vec::new(), true),
        ]
        .map(|(name, body, redirect)| {
            let path = format!("preview/{name}");
            let expected =
                format!("GET /api/v5/repos/owner/plugins/contents/{path}?ref=master HTTP/1.1");
            let wrapper = json!({
                "type": "file", "encoding": "base64", "path": path,
                "size": body.len(), "content": STANDARD.encode(&body)
            });
            (expected, serde_json::to_vec(&wrapper).unwrap(), redirect)
        });
        let server = std::thread::spawn(move || {
            for (expected, body, redirect) in replies {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = BufReader::new(&stream);
                let mut line = String::new();
                request.read_line(&mut line).unwrap();
                assert_eq!(line.trim_end(), expected);
                loop {
                    line.clear();
                    if request.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                if redirect {
                    write!(
                        stream,
                        "HTTP/1.1 302 Found\r\nLocation: https://example.invalid/outside\r\n"
                    )
                    .unwrap();
                } else {
                    write!(stream, "HTTP/1.1 200 OK\r\n").unwrap();
                }
                write!(
                    stream,
                    "Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });

        let storage = fixture.root.join("installed");
        let mut host = LocalPlugins::open(storage.clone());
        host.set_market_catalog(&fixture.index)?;
        host.set_official_test_index(index_url.clone());
        host.ensure_official_catalog();
        settle_market(&mut host);
        let remote = host.market_catalog(true).unwrap();
        assert_eq!(remote.entries[0].version, "1.0.0");
        assert_eq!(
            host.market_catalog(false).unwrap().entries[0].version,
            "1.1.0"
        );
        let local = host.market_catalog(false).unwrap();
        assert!(matches!(
            host.official_avatar(&local, &remote.entries[0]),
            Some(Err(()))
        ));
        assert!(host.official_avatar(&remote, &remote.entries[0]).is_none());
        settle_market(&mut host);
        let image = host
            .official_avatar(&remote, &remote.entries[0])
            .unwrap()
            .unwrap();
        assert_eq!(image.bytes(), avatar.as_slice());
        assert!(host.official_cover(&remote, &remote.entries[0]).is_none());
        assert_eq!(host.official_cover_is_material(&remote.entries[0]), None);
        settle_market(&mut host);
        assert!(matches!(
            host.official_cover(&remote, &remote.entries[0]),
            Some(Ok(_))
        ));
        assert_eq!(host.official_cover_is_material(&remote.entries[0]), Some(false));
        host.market_action(true, ID)?;
        settle_market(&mut host);
        assert!(host.records[ID].official_market);
        assert!(host.records[ID].presentation.as_ref().unwrap().sample);
        assert!(!host.records[ID].enabled);
        assert_eq!(status(&host), MarketStatus::OtherSource);
        host.enable(ID, true)?;
        let mut values = host.records[ID].values.clone();
        values.insert("title".into(), "我的发薪日".into());
        host.save(ID, values.clone())?;
        let installed = fs::read(host.path(ID))?;

        host.refresh_official_catalog();
        settle_market(&mut host);
        let remote = host.market_catalog(true).unwrap();
        assert_eq!(
            host.market_status(&remote, &remote.entries[0]),
            MarketStatus::Update
        );
        host.market_action(true, ID)?;
        settle_market(&mut host);
        assert!(host.notice.contains("SHA-256"));
        assert_eq!(fs::read(host.path(ID))?, installed);
        host.market_action(true, ID)?;
        settle_market(&mut host);
        assert_eq!(
            host.market_status(&remote, &remote.entries[0]),
            MarketStatus::Installed
        );
        assert_eq!(host.records[ID].values, values);
        assert!(host.records[ID].enabled);

        let mut host = LocalPlugins::open(storage);
        host.set_official_test_index(index_url);
        host.ensure_official_catalog();
        assert!(!host.official_catalog_busy());
        assert!(host.official_catalog_error().is_none());
        assert_eq!(
            host.market_catalog(true).unwrap().entries[0].version,
            "1.1.0"
        );
        host.refresh_official_catalog();
        settle_market(&mut host);
        let remote = host.market_catalog(true).unwrap();
        assert!(host
            .official_catalog_error()
            .unwrap()
            .contains("非预期跳转"));
        assert_eq!(remote.entries[0].version, "1.1.0");
        assert_eq!(fs::read(&remote.path)?, second);
        server.join().unwrap();
        Ok(())
    }
}

#[test]
fn local_cover_requires_a_bounded_png_inside_the_package() -> Result<()> {
    use image::{codecs::png::PngEncoder, ExtendedColorType, ImageEncoder};

    let root = std::env::temp_dir().join(format!("daozai-cover-test-{}", std::process::id()));
    let package = root.join("package");
    fs::create_dir_all(&package)?;
    let path = package.join("plugin.json");
    let cover = package.join(COVER_FILE);
    let mut manifest: Value = serde_json::from_str(include_str!("examples/payday/plugin.json"))?;
    fs::write(
        package.join("main.js"),
        include_str!("examples/payday/main.js"),
    )?;
    fs::write(&path, serde_json::to_vec(&manifest)?)?;
    assert!(Package::read(&path).is_err());
    fs::write(&cover, b"broken")?;
    assert!(Package::read(&path).is_err());
    fs::write(&cover, vec![0; cover::COVER_LIMIT as usize + 1])?;
    assert!(Package::read(&path)
        .err()
        .unwrap()
        .to_string()
        .contains("256 KiB"));

    let mut too_wide = Vec::new();
    PngEncoder::new(&mut too_wide).write_image(&[0; 1921], 1921, 1, ExtendedColorType::L8)?;
    fs::write(&cover, too_wide)?;
    assert!(Package::read(&path).is_err());

    let png = include_bytes!("examples/payday/cover.png");
    let outside = root.join("outside.png");
    fs::write(&outside, png)?;
    fs::remove_file(&cover)?;
    fs::hard_link(&outside, &cover)?;
    assert!(Package::read(&path).is_err());
    fs::remove_file(&cover)?;
    fs::write(&cover, png)?;

    manifest["cover"] = json!("../outside.png");
    fs::write(&path, serde_json::to_vec(&manifest)?)?;
    assert!(Package::read(&path).is_err());
    manifest.as_object_mut().unwrap().remove("cover");
    fs::write(&path, serde_json::to_vec(&manifest)?)?;
    assert!(Package::read(&path).is_err());
    manifest["cover"] = json!(COVER_FILE);
    fs::write(&path, serde_json::to_vec(&manifest)?)?;
    assert_eq!(
        Package::read(&path)?.cover_png().as_deref(),
        Some(png.as_slice())
    );
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn two_packages_survive_disable_restart_and_both_uninstall_choices() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "daozai-local-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let dir = root.join("installed");
    let mut host = LocalPlugins::open(dir.clone());
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/payday/plugin.json");
    let trip = root.join("source");
    fs::create_dir_all(&trip)?;
    let second = trip.join("plugin.json");
    let mut m: Value = serde_json::from_slice(&fs::read(&sample)?)?;
    m["id"] = json!("example.trip");
    m["name"] = json!("旅行");
    m["settings"][0]["default"] = json!("旅行");
    fs::write(&second, serde_json::to_vec(&m)?)?;
    fs::write(
        trip.join("main.js"),
        "function refresh(i){return {card:i.settings.title,storage:(i.storage||0)+1}}",
    )?;
    fs::copy(sample.with_file_name(COVER_FILE), trip.join(COVER_FILE))?;
    let (a, b) = ("example.payday", "example.trip");
    host.install(&sample)?;
    host.install(&second)?;
    assert!(host.install(&sample).is_err());
    settle(&mut host, "2026-12-01");
    assert!(host.cards.is_empty());
    host.enable(a, true)?;
    host.enable(b, true)?;
    let mut v = host.records[a].values.clone();
    v.insert("target".into(), "2026-12-03".into());
    host.save(a, v)?;
    settle(&mut host, "2026-12-01");
    assert_eq!(host.cards[a], "距离发薪还有 2 天");
    assert_eq!(host.cards[b], "旅行");
    host.enable(b, false)?;
    host.tick(false, false, "");
    assert!(host.cards.is_empty());
    drop(host);
    let mut host = LocalPlugins::open(dir.clone());
    host.tick(false, false, "");
    assert!(host.cards.is_empty());
    settle(&mut host, "2026-12-02");
    assert_eq!(host.cards[a], "距离发薪还有 1 天");
    assert!(!host.cards.contains_key(b));
    assert_eq!(host.records[b].storage, 1);
    host.enable(b, true)?;
    settle(&mut host, "2026-12-02");
    assert_eq!(host.records[b].storage, 2);
    host.uninstall(a, true)?;
    assert!(!host.cards.contains_key(a));
    assert!(host.cards.contains_key(b));
    host.install(&sample)?;
    assert_eq!(host.records[a].values["target"], "2026-12-03");
    assert_eq!(host.records[a].storage["lastDays"], 1);
    host.uninstall(a, false)?;
    host.install(&sample)?;
    assert_eq!(host.records[a].values["target"], "2026-12-25");
    assert!(host.records[a].storage.is_null());
    assert_eq!(host.records[b].storage, 2);
    m["entry"] = json!("../outside.js");
    fs::write(&second, serde_json::to_vec(&m)?)?;
    assert!(Package::read(&second).is_err());
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn deadline_work_never_blocks_frames_or_starts_twice() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("daozai-frame-test-{}", std::process::id()));
    let mut host = LocalPlugins::open(dir.clone());
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/payday/plugin.json");
    host.install(&sample)?;
    let id = "example.payday";
    host.records
        .get_mut(id)
        .unwrap()
        .package
        .as_mut()
        .unwrap()
        .source = "function refresh(){while(true){}}".into();
    host.enable(id, true)?;
    let start = Instant::now();
    host.tick(true, true, "2026-11-19");
    let worker = host.jobs[id].result.thread().id();
    assert_ne!(worker, std::thread::current().id());
    for _ in 0..10 {
        host.tick(true, true, "2026-11-19");
        assert_eq!(host.jobs[id].result.thread().id(), worker);
    }
    assert!(
        start.elapsed() < Duration::from_millis(25),
        "十次界面更新不应等待 30 毫秒的插件"
    );
    settle(&mut host, "2026-11-19");
    assert!(host.errors.contains_key(id));
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn delayed_result_discarded_after_disable_uninstall_and_restart_validation() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("daozai-late-test-{}", std::process::id()));
    let mut host = LocalPlugins::open(dir.clone());
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/payday/plugin.json");
    let id = "example.payday";
    host.install(&sample)?;
    host.enable(id, true)?;
    host.tick(true, true, "2026-11-19");
    host.enable(id, false)?;
    settle(&mut host, "2026-11-19");
    assert!(host.cards.is_empty());
    assert!(host.records[id].storage.is_null());
    assert!(LocalPlugins::open(dir.clone()).records[id]
        .storage
        .is_null());
    host.enable(id, true)?;
    host.tick(true, true, "2026-11-19");
    host.uninstall(id, true)?;
    host.install(&sample)?;
    settle(&mut host, "2026-11-19");
    assert!(host.cards.is_empty());
    assert!(host.records[id].storage.is_null());
    assert!(LocalPlugins::open(dir.clone()).records[id]
        .storage
        .is_null());
    let mut broken = host.records[id].clone();
    broken.enabled = true;
    broken.package.as_mut().unwrap().manifest.entry = "../other.js".into();
    host.put(id, broken)?;
    drop(host);
    let mut host = LocalPlugins::open(dir.clone());
    host.tick(true, true, "2026-11-19");
    assert!(host.invalid.contains(id));
    assert!(host.jobs.is_empty());
    assert!(host.cards.is_empty());
    host.install(&sample)?;
    assert!(host.invalid.is_empty());
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn design_card_example_runs_with_both_installed_manifests() -> Result<()> {
    let design = include_str!("../docs/plugins/sdk-v1-design.md");
    let source = design
        .split_once("```js")
        .unwrap()
        .1
        .split_once("```")
        .unwrap()
        .0;
    for sample in ["payday", "trip"] {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples");
        let mut pkg = Package::read(&root.join(sample).join("plugin.json"))?;
        let values: Values = pkg
            .manifest
            .settings
            .iter()
            .map(|f| (f.key.clone(), f.default.clone()))
            .collect();
        let first = pkg.render(&values, &Value::Null, "2026-11-19")?;
        pkg.source = source.to_owned();
        let doc_example = pkg.render(&values, &first.storage, "2026-11-19")?;
        assert_eq!(doc_example.storage, first.storage);
        assert!(doc_example.card.contains(&values["title"]));
    }
    Ok(())
}

#[cfg(feature = "sdk-material")]
mod material_tests {
    use super::*;

    const SHADER: &str = "fn material(i: MaterialIn) -> vec4<f32> { return vec4<f32>(0.0); }";

    fn root() -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("daozai-material-{}-{nonce}", std::process::id()))
    }

    fn manifest(id: &str) -> Value {
        json!({
            "id": id, "name": "测试材质", "author": "测试", "version": "1.0.0", "api": 1,
            "runtime": "material", "entry": "shaders/material.wgsl", "cover": COVER_FILE,
            "owner": "appearance",
            "capabilities": ["appearance.material", "settings"], "dataVersion": 1,
            "permissions": {"screen.backdrop": true},
            "material": {"controls": true, "spring": {"stiffness": 1.0, "damping": 0.5}},
            "settings": [
                {"key": "strength", "label": "强度", "type": "number", "min": 0, "max": 120, "step": 1, "default": "48"},
                {"key": "blur", "label": "模糊", "type": "number", "min": 0, "max": 10, "step": 0.5, "default": "1.5"}
            ]
        })
    }

    fn package(dir: &Path, id: &str) -> Result<PathBuf> {
        let shader_dir = dir.join("shaders");
        fs::create_dir_all(&shader_dir)?;
        fs::write(shader_dir.join("material.wgsl"), SHADER)?;
        fs::write(
            dir.join(COVER_FILE),
            include_bytes!("examples/liquid-glass/cover.png"),
        )?;
        fs::create_dir_all(dir.join("docs"))?;
        fs::write(dir.join("docs/README.md"), "材质说明")?;
        let path = dir.join("plugin.json");
        fs::write(&path, serde_json::to_vec(&manifest(id))?)?;
        Ok(path)
    }

    #[test]
    fn material_manifest_rejects_invalid_settings_and_capabilities() -> Result<()> {
        let valid = manifest("example.material");
        serde_json::from_value::<Manifest>(valid.clone())?.validate()?;

        for key in ["permissions", "material", "settings"] {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(
                serde_json::from_value::<Manifest>(missing)
                    .ok()
                    .is_none_or(|manifest| manifest.validate().is_err()),
                "{key}"
            );
        }
        for key in ["min", "max", "step"] {
            let mut missing = valid.clone();
            missing["settings"][0].as_object_mut().unwrap().remove(key);
            assert!(
                serde_json::from_value::<Manifest>(missing)?
                    .validate()
                    .is_err(),
                "{key}"
            );
        }
        for (pointer, replacement) in [
            ("/permissions/screen.backdrop", json!(false)),
            ("/material/spring/stiffness", json!(3.1)),
            ("/material/spring/damping", json!(0.0)),
            ("/owner", json!("tasks")),
            ("/capabilities", json!(["card", "settings"])),
            ("/capabilities", json!(["appearance.material", "network"])),
            (
                "/capabilities",
                json!(["appearance.material", "appearance.material"]),
            ),
            ("/capabilities", json!(["appearance.material"])),
            ("/settings/0/type", json!("text")),
            ("/settings/0/min", json!(150)),
            ("/settings/0/step", json!(0)),
            ("/settings/0/step", json!(150)),
            ("/settings/0/default", json!("128")),
            ("/settings/0/default", json!("NaN")),
            ("/entry", json!("../material.wgsl")),
            ("/entry", json!("material.js")),
        ] {
            let mut invalid = valid.clone();
            *invalid.pointer_mut(pointer).unwrap() = replacement;
            assert!(
                serde_json::from_value::<Manifest>(invalid)?
                    .validate()
                    .is_err(),
                "{pointer}"
            );
        }
        let mut too_many = valid;
        too_many["settings"] = Value::Array((0..33).map(|index| {
            json!({"key":format!("param{index}"),"label":"参数","type":"number","min":0,"max":1,"step":0.1,"default":"0"})
        }).collect());
        assert!(serde_json::from_value::<Manifest>(too_many.clone())?
            .validate()
            .is_err());
        too_many["settings"].as_array_mut().unwrap().pop();
        serde_json::from_value::<Manifest>(too_many)?.validate()?;
        Ok(())
    }

    #[test]
    fn material_manifest_accepts_optional_units_and_display_precision() -> Result<()> {
        let mut valid = manifest("example.material");
        serde_json::from_value::<Manifest>(valid.clone())?.validate()?;
        for (unit, precision) in [("px", 0), ("毫秒单位", 3)] {
            valid["settings"][0]["unit"] = json!(unit);
            valid["settings"][0]["precision"] = json!(precision);
            serde_json::from_value::<Manifest>(valid.clone())?.validate()?;
        }
        for (key, value) in [
            ("unit", json!("毫秒单位符")),
            ("unit", json!(3)),
            ("unit", Value::Null),
            ("precision", json!(-1)),
            ("precision", json!(4)),
            ("precision", json!(1.5)),
            ("precision", Value::Null),
        ] {
            let mut invalid = valid.clone();
            invalid["settings"][0][key] = value;
            assert!(
                serde_json::from_value::<Manifest>(invalid)
                    .ok()
                    .is_none_or(|manifest| manifest.validate().is_err()),
                "{key}"
            );
        }
        Ok(())
    }

    #[test]
    fn future_material_api_prompts_to_update_daozai_at_installation() -> Result<()> {
        let root = root();
        let path = package(&root.join("package"), "example.material")?;
        let mut host = LocalPlugins::open(root.join("installed"));
        let mut future = manifest("example.material");
        future["unexpectedFutureField"] = json!(true);
        fs::write(&path, serde_json::to_vec(&future)?)?;
        assert!(!host
            .install(&path)
            .unwrap_err()
            .to_string()
            .contains("需要更新岛仔"));

        future["api"] = json!(2);
        fs::write(&path, serde_json::to_vec(&future)?)?;
        assert!(host
            .install(&path)
            .unwrap_err()
            .to_string()
            .contains("需要更新岛仔"));
        assert!(!host.records.contains_key("example.material"));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn material_manifest_validates_optional_shadow_and_foreground() -> Result<()> {
        let mut value = manifest("example.material");
        let old = serde_json::from_value::<Manifest>(value.clone())?
            .material
            .unwrap();
        assert_eq!(old.shadow, None);
        assert!(matches!(old.foreground, MaterialForeground::Skin));
        for (shadow, foreground) in [(0.0, "skin"), (1.0, "glass"), (1.5, "glass")] {
            value["material"]["shadow"] = json!(shadow);
            value["material"]["foreground"] = json!(foreground);
            serde_json::from_value::<Manifest>(value.clone())?.validate()?;
        }
        for (key, invalid) in [
            ("shadow", json!(-0.001)),
            ("shadow", json!(1.501)),
            ("shadow", json!("1.0")),
            ("shadow", Value::Null),
            ("foreground", json!("opaque")),
            ("foreground", json!(true)),
            ("foreground", Value::Null),
        ] {
            let mut invalid_manifest = value.clone();
            invalid_manifest["material"][key] = invalid;
            assert!(
                serde_json::from_value::<Manifest>(invalid_manifest)
                    .ok()
                    .is_none_or(|manifest| manifest.validate().is_err()),
                "{key}"
            );
        }
        Ok(())
    }

    #[test]
    fn material_package_allows_only_the_manifest_shader_and_documentation() -> Result<()> {
        let root = root();
        let path = package(&root, "example.material")?;
        assert_eq!(Package::read(&path)?.source, SHADER);
        for name in [
            "main.js",
            "extra.wgsl",
            "helper.exe",
            "docs/asset.bin",
            "docs/plugin.json",
        ] {
            let extra = root.join(name);
            fs::write(&extra, "多余文件")?;
            assert!(Package::read(&path).is_err(), "{name}");
            fs::remove_file(extra)?;
        }
        let shader = root.join("shaders/material.wgsl");
        fs::write(&shader, "a".repeat(MATERIAL_SOURCE_LIMIT as usize))?;
        assert_eq!(
            Package::read(&path)?.source.len() as u64,
            MATERIAL_SOURCE_LIMIT
        );
        fs::write(&shader, "a".repeat(MATERIAL_SOURCE_LIMIT as usize + 1))?;
        assert!(Package::read(&path).is_err());
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    #[cfg(windows)]
    fn liquid_glass_package_installs_and_keeps_parameters_when_disabled() -> Result<()> {
        let root = root();
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/liquid-glass/plugin.json");
        let manifest = Package::read(&path)?.manifest;
        let id = &manifest.id;
        let first_parameter = &manifest.settings[0].key;
        let mut host = LocalPlugins::open(root.clone());
        host.install(&path)?;
        assert!(host.enable(id, true).is_err());
        assert!(host.active_material().is_none());
        assert!(!LocalPlugins::startup_material(&root));

        host.activate_material(Some(id), true)?;
        assert!(LocalPlugins::startup_material(&root));
        let active = host.active_material().unwrap();
        assert!(active.controls);
        assert!(active.glass_foreground);
        assert_eq!(active.shadow, Some(1.0));
        assert_eq!((active.stiffness, active.damping), (1.0, 0.5));
        let mut expected = [48.0, 8.0, 1.5, 87.0, 1.5, 0.9, 1.0];
        assert_eq!(active.params[..expected.len()], expected);
        expected[0] = 49.0;
        host.set_material_parameter(id, first_parameter, expected[0])?;
        assert_eq!(
            host.active_material().unwrap().params[..expected.len()],
            expected
        );
        host.enable(id, false)?;
        assert!(host.active_material().is_none());
        assert!(!LocalPlugins::startup_material(&root));

        let mut host = LocalPlugins::open(root.clone());
        assert!(host.active_material().is_none());
        host.enable(id, true)?;
        assert_eq!(
            host.active_material().unwrap().params[..expected.len()],
            expected
        );
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    #[cfg(windows)]
    fn an_authorized_material_is_selected_exclusively_and_reloaded_with_its_parameters(
    ) -> Result<()> {
        let root = root();
        let storage = root.join("installed");
        let (a, b) = ("example.first", "example.second");
        let path_a = package(&root.join("a"), a)?;
        let path_b = package(&root.join("b"), b)?;
        let mut host = LocalPlugins::open(storage.clone());
        host.install(&path_a)?;
        host.install(&path_b)?;
        assert!(!LocalPlugins::startup_material(&storage));
        assert!(host.enable(a, true).is_err());
        fs::write(
            storage.join("active-material.json"),
            serde_json::to_vec(&a)?,
        )?;
        assert!(!LocalPlugins::startup_material(&storage));
        assert!(LocalPlugins::open(storage.clone())
            .active_material()
            .is_none());

        host.activate_material(Some(a), true)?;
        assert!(LocalPlugins::startup_material(&storage));
        assert_eq!(host.active_material().unwrap().params[..2], [48.0, 1.5]);
        assert!(host.activate_material(Some(b), false).is_err());
        assert_eq!(host.selected_material_id(), Some(a));
        host.activate_material(Some(b), true)?;
        assert_eq!(host.active_material().unwrap().id, b);
        assert!(host.material_permission_granted(a));
        host.enable(a, false)?;
        assert_eq!(host.selected_material_id(), Some(b));
        assert!(host.set_material_parameter(b, "strength", 121.0).is_err());
        host.set_material_parameter(b, "blur", 2.5)?;
        assert_eq!(host.active_material().unwrap().params[..2], [48.0, 2.5]);
        host.tick(true, true, "2026-11-21");
        assert!(host.jobs.is_empty());

        let mut host = LocalPlugins::open(storage.clone());
        assert_eq!(host.active_material().unwrap().params[..2], [48.0, 2.5]);
        host.activate_material(None, false)?;
        assert!(!LocalPlugins::startup_material(&storage));
        host.activate_material(Some(b), false)?;
        host.uninstall(b, true)?;
        assert!(host.active_material().is_none());
        host.install(&path_b)?;
        assert!(host.activate_material(Some(b), false).is_err());
        host.activate_material(Some(b), true)?;
        assert_eq!(host.active_material().unwrap().params[1], 2.5);
        fs::remove_dir_all(root)?;
        Ok(())
    }
}

#[test]
fn mail_view_is_code_free_scoped_and_follows_activation() -> Result<()> {
    let root = std::env::temp_dir().join(format!("daozai-mail-view-{}-{}",
        std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()));
    fs::create_dir_all(&root)?;
    let mut host = LocalPlugins::open(root.join("installed"));
    let make = |id: &str| -> Result<PathBuf> {
        let dir = root.join(id);
        fs::create_dir(&dir)?;
        fs::write(dir.join(COVER_FILE), include_bytes!("examples/payday/cover.png"))?;
        fs::write(dir.join("mail-view.json"), r#"{"version":1,"showUnread":true,"startDateSetting":"since"}"#)?;
        let path = dir.join("plugin.json");
        fs::write(&path, json!({
            "id":id, "name":"邮箱视图", "author":"测试", "version":"1.0.0",
            "api":1, "runtime":"assets", "entry":"mail-view.json", "cover":COVER_FILE,
            "owner":"mail", "capabilities":["mail.view","settings"], "dataVersion":1,
            "permissions":{}, "settings":[{
                "key":"since","label":"日期","type":"date","default":"0001-01-01"
            }]
        }).to_string())?;
        Ok(path)
    };
    let first = make("example.mail-first")?;
    let second = make("example.mail-second")?;
    let manifest: Value = serde_json::from_slice(&fs::read(&first)?)?;
    for (key, val) in [
        ("permissions", json!({"source.read":"https"})),
        ("owner", json!("status")),
        ("capabilities", json!(["mail.view","action"])),
        ("runtime", json!("js")),
    ] {
        let mut test = manifest.clone();
        test[key] = val;
        assert!(Manifest::decode(&test.to_string()).is_err());
    }
    let pkg = Package::read(&first)?;
    assert!(pkg.mail_view(&Values::new())?.start_date.is_none());
    let view = pkg.mail_view(&Values::from([("since".into(), "2026-11-28".into())]))?;
    assert_eq!(view.start_date, chrono::NaiveDate::from_ymd_opt(2026, 11, 28));
    let entry = first.with_file_name("mail-view.json");
    let original = fs::read(&entry)?;
    for raw in [
        r#"{"version":1,"showUnread":true,"command":"readAll"}"#,
        r#"{"version":1,"showUnread":true,"startDateSetting":"not-declared"}"#,
        r#"{"version":2,"showUnread":true}"#,
    ] {
        fs::write(&entry, raw)?;
        assert!(Package::read(&first).is_err());
    }
    fs::write(&entry, original)?;
    host.install(&first)?;
    host.install(&second)?;
    host.enable("example.mail-first", true)?;
    assert_eq!(host.active_mail_view().unwrap().0, "example.mail-first");
    assert!(host.enable("example.mail-second", true).is_err());
    host.enable("example.mail-first", false)?;
    assert!(host.active_mail_view().is_none());
    host.enable("example.mail-second", true)?;
    assert_eq!(host.active_mail_view().unwrap().0, "example.mail-second");
    host.uninstall("example.mail-second", false)?;
    assert!(host.active_mail_view().is_none());
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn lyrics_style_is_code_free_scoped_and_follows_activation() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "daozai-lyrics-style-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()
    ));
    fs::create_dir_all(&root)?;
    let mut host = LocalPlugins::open(root.join("installed"));
    let make = |id: &str| -> Result<PathBuf> {
        let dir = root.join(id);
        fs::create_dir(&dir)?;
        fs::write(dir.join(COVER_FILE), include_bytes!("examples/payday/cover.png"))?;
        fs::write(
            dir.join("lyrics-style.json"),
            r##"{"version":1,"island":true,"expanded":true,"defaultColor":"rainbow","defaultCustomColor":"#38BDF8","islandSetting":"island_enabled","expandedSetting":"expanded_enabled","colorSetting":"color","customColorSetting":"custom_color"}"##,
        )?;
        let path = dir.join("plugin.json");
        fs::write(
            &path,
            json!({
                "id": id, "name": "逐字变色歌词", "author": "swidan123", "version": "1.0.1",
                "api": 1, "runtime": "assets", "entry": "lyrics-style.json", "cover": COVER_FILE,
                "owner": "music", "capabilities": ["lyrics.style", "settings"], "dataVersion": 1,
                "permissions": {}, "settings": [
                    {"key": "island_enabled", "label": "岛上歌词逐字变色", "type": "switch", "default": "true"},
                    {"key": "expanded_enabled", "label": "展开歌词逐字变色", "type": "switch", "default": "true"},
                    {"key": "color", "label": "已唱颜色", "type": "text", "default": "rainbow"},
                    {"key": "custom_color", "label": "自定义色号", "type": "text", "default": "#38BDF8"}
                ]
            })
            .to_string(),
        )?;
        Ok(path)
    };
    let first = make("example.lyrics-first")?;
    let second = make("example.lyrics-second")?;
    let manifest: Value = serde_json::from_slice(&fs::read(&first)?)?;
    for (key, val) in [
        ("permissions", json!({"source.read": "https"})),
        ("owner", json!("status")),
        ("capabilities", json!(["lyrics.style", "action"])),
        ("runtime", json!("js")),
    ] {
        let mut test = manifest.clone();
        test[key] = val;
        assert!(Manifest::decode(&test.to_string()).is_err());
    }
    let pkg = Package::read(&first)?;
    let default_style = pkg.lyrics_style(&Values::new())?;
    assert!(default_style.island && default_style.expanded);
    assert_eq!(default_style.color, crate::model::LyricColor::Rainbow);
    let custom_style = pkg.lyrics_style(&Values::from([
        ("island_enabled".into(), "false".into()),
        ("expanded_enabled".into(), "true".into()),
        ("color".into(), "custom".into()),
        ("custom_color".into(), "#007F00".into()),
    ]))?;
    assert!(!custom_style.island && custom_style.expanded);
    assert_eq!(custom_style.color, crate::model::LyricColor::Custom);
    assert_eq!(custom_style.custom_color, [0, 127, 0]);
    let mint_style = pkg.lyrics_style(&Values::from([
        ("island_enabled".into(), "true".into()),
        ("expanded_enabled".into(), "false".into()),
        ("color".into(), "mint_green".into()),
    ]))?;
    assert!(mint_style.island && !mint_style.expanded);
    assert_eq!(mint_style.color, crate::model::LyricColor::MintGreen);
    // 非 ASCII 6 字节自定义色号不得 panic，应安全回退默认色号。
    let fallback_style = pkg.lyrics_style(&Values::from([
        ("color".into(), "custom".into()),
        ("custom_color".into(), "a中aa".into()),
    ]))?;
    assert_eq!(fallback_style.custom_color, [56, 189, 248]);

    let entry = first.with_file_name("lyrics-style.json");
    let original = fs::read(&entry)?;
    for raw in [
        r#"{"version":1,"unknown":true}"#,
        r#"{"version":1,"defaultColor":"invalid"}"#,
        r#"{"version":1,"colorSetting":"not-declared"}"#,
        r#"{"version":1,"islandSetting":"island_enabled","expandedSetting":"island_enabled"}"#,
        r#"{"version":2}"#,
    ] {
        fs::write(&entry, raw)?;
        assert!(Package::read(&first).is_err());
    }
    fs::write(&entry, original)?;

    host.install(&first)?;
    host.install(&second)?;
    host.enable("example.lyrics-first", true)?;
    assert_eq!(host.active_lyrics_style().unwrap().0, "example.lyrics-first");
    assert!(host.enable("example.lyrics-second", true).is_err());
    host.enable("example.lyrics-first", false)?;
    assert!(host.active_lyrics_style().is_none());
    host.enable("example.lyrics-second", true)?;
    assert_eq!(host.active_lyrics_style().unwrap().0, "example.lyrics-second");
    host.uninstall("example.lyrics-second", false)?;
    assert!(host.active_lyrics_style().is_none());

    // v1.0.0（enabled/expanded + 2 项 choice color）→ v1.0.1（island_enabled/expanded_enabled + text color）
    // 本地重载与集市升级均应迁移用户已关闭的开关与合法颜色偏好。
    let mig_dir = root.join("example.lyrics-migrate");
    fs::create_dir(&mig_dir)?;
    fs::write(mig_dir.join(COVER_FILE), include_bytes!("examples/payday/cover.png"))?;
    let mig_manifest = mig_dir.join("plugin.json");
    let mig_entry = mig_dir.join("lyrics-style.json");
    fs::write(
        &mig_entry,
        r##"{"version":1,"karaoke":true,"expanded":true,"defaultColor":"rainbow","defaultCustomColor":"#38BDF8","enabledSetting":"enabled","expandedSetting":"expanded","colorSetting":"color","customColorSetting":"custom_color"}"##,
    )?;
    fs::write(
        &mig_manifest,
        json!({
            "id": "example.lyrics-migrate", "name": "逐字变色歌词", "author": "swidan123", "version": "1.0.0",
            "api": 1, "runtime": "assets", "entry": "lyrics-style.json", "cover": COVER_FILE,
            "owner": "music", "capabilities": ["lyrics.style", "settings"], "dataVersion": 1,
            "permissions": {}, "settings": [
                {"key": "enabled", "label": "歌词逐字变色", "type": "switch", "default": "true"},
                {"key": "expanded", "label": "展开歌词同步变色", "type": "switch", "default": "true"},
                {"key": "color", "label": "已唱颜色", "type": "choice", "default": "rainbow", "choices": [
                    {"value": "rainbow", "label": "彩虹流光"},
                    {"value": "custom", "label": "自定义色号"}
                ]},
                {"key": "custom_color", "label": "自定义色号", "type": "text", "default": "#38BDF8"}
            ]
        })
        .to_string(),
    )?;
    host.install(&mig_manifest)?;
    host.enable("example.lyrics-migrate", true)?;
    host.save(
        "example.lyrics-migrate",
        Values::from([
            ("enabled".into(), "false".into()),
            ("expanded".into(), "false".into()),
            ("color".into(), "custom".into()),
            ("custom_color".into(), "#007F00".into()),
        ]),
    )?;
    let v100_record = host.records["example.lyrics-migrate"].clone();
    // 原地更新为 v1.0.1 清单并测试本地热重载迁移。
    fs::write(
        &mig_entry,
        r##"{"version":1,"island":true,"expanded":true,"defaultColor":"rainbow","defaultCustomColor":"#38BDF8","islandSetting":"island_enabled","expandedSetting":"expanded_enabled","colorSetting":"color","customColorSetting":"custom_color"}"##,
    )?;
    fs::write(
        &mig_manifest,
        json!({
            "id": "example.lyrics-migrate", "name": "逐字变色歌词", "author": "swidan123", "version": "1.0.1",
            "api": 1, "runtime": "assets", "entry": "lyrics-style.json", "cover": COVER_FILE,
            "owner": "music", "capabilities": ["lyrics.style", "settings"], "dataVersion": 1,
            "permissions": {}, "settings": [
                {"key": "island_enabled", "label": "岛上歌词逐字变色", "type": "switch", "default": "true"},
                {"key": "expanded_enabled", "label": "展开歌词逐字变色", "type": "switch", "default": "true"},
                {"key": "color", "label": "已唱颜色", "type": "text", "default": "rainbow"},
                {"key": "custom_color", "label": "自定义色号", "type": "text", "default": "#38BDF8"}
            ]
        })
        .to_string(),
    )?;
    let epoch_before_reload = host.view_epoch;
    let v101_pkg = Package::read(&mig_manifest)?;
    host.reload_local("example.lyrics-migrate")?;
    assert!(host.view_epoch > epoch_before_reload, "热重载应递增视图代际以作废临时草稿");
    let reloaded = host.active_lyrics_style().unwrap().1;
    assert!(!reloaded.island && !reloaded.expanded);
    assert_eq!(reloaded.color, crate::model::LyricColor::Custom);
    assert_eq!(reloaded.custom_color, [0, 127, 0]);
    // 不兼容的本地热重载（如删掉 custom_color 或 choice 不包含当前已保存的 custom）必须被拒绝，不能静默重置设置。
    fs::write(
        &mig_entry,
        r##"{"version":1,"island":true,"expanded":true,"defaultColor":"rainbow","defaultCustomColor":"#38BDF8","islandSetting":"island_enabled","expandedSetting":"expanded_enabled","colorSetting":"color"}"##,
    )?;
    fs::write(
        &mig_manifest,
        json!({
            "id": "example.lyrics-migrate", "name": "逐字变色歌词", "author": "swidan123", "version": "1.0.2",
            "api": 1, "runtime": "assets", "entry": "lyrics-style.json", "cover": COVER_FILE,
            "owner": "music", "capabilities": ["lyrics.style", "settings"], "dataVersion": 1,
            "permissions": {}, "settings": [
                {"key": "island_enabled", "label": "岛上歌词逐字变色", "type": "switch", "default": "true"},
                {"key": "expanded_enabled", "label": "展开歌词逐字变色", "type": "switch", "default": "true"},
                {"key": "color", "label": "已唱颜色", "type": "choice", "default": "rainbow", "choices": [
                    {"value": "rainbow", "label": "彩虹流光"},
                    {"value": "aurora", "label": "极光渐变"}
                ]}
            ]
        })
        .to_string(),
    )?;
    assert!(host.reload_local("example.lyrics-migrate").is_err(), "不兼容的歌词样式热重载不应静默重置设置");
    let still_preserved = host.active_lyrics_style().unwrap().1;
    assert!(!still_preserved.island && !still_preserved.expanded);
    assert_eq!(still_preserved.color, crate::model::LyricColor::Custom);
    assert_eq!(still_preserved.custom_color, [0, 127, 0]);
    #[cfg(windows)]
    {
        let upgraded = market_state::prepare_record(v100_record, v101_pkg)?;
        assert_eq!(upgraded.values.get("island_enabled").map(String::as_str), Some("false"));
        assert_eq!(upgraded.values.get("expanded_enabled").map(String::as_str), Some("false"));
        assert_eq!(upgraded.values.get("color").map(String::as_str), Some("custom"));
        assert_eq!(upgraded.values.get("custom_color").map(String::as_str), Some("#007F00"));
    }
    #[cfg(not(windows))]
    let _ = (v100_record, v101_pkg);
    fs::remove_dir_all(root)?;
    Ok(())
}
