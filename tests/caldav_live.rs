//! 显式授权后运行的真实服务验收；普通 cargo test 不连接网络。
//! 只向本轮预先登记、随机生成且经 UID 核验的资源发条件写请求。
use daozai::{
    calendar::{
        self,
        caldav::{
            dav::{Component, DavClient},
            ical::{
                CalendarDocument, CalendarItem, CommonPatch, EventPatch, Patch, TaskPatch,
                TaskStateChange, TaskStatus,
            },
            sync,
        },
        Config, Provider,
    },
    organizer,
};
use std::collections::{BTreeMap, BTreeSet};

type TestResult<T = ()> = Result<T, String>;
fn failure(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn ensure(condition: bool, message: &str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn environment(name: &str) -> TestResult<String> {
    std::env::var(name).map_err(|_| format!("缺少显式测试环境变量 {name}"))
}

struct Isolated {
    root: std::path::PathBuf,
    previous: Option<std::ffi::OsString>,
}
impl Isolated {
    fn new() -> TestResult<Self> {
        let root = std::env::temp_dir().join(format!("daozai-live-caldav-{}", organizer::new_id()));
        std::fs::create_dir(&root).map_err(failure)?;
        let previous = std::env::var_os("DAOZAI_DATA_DIR");
        std::env::set_var("DAOZAI_DATA_DIR", &root);
        Ok(Self { root, previous })
    }
}
impl Drop for Isolated {
    fn drop(&mut self) {
        if let Some(value) = &self.previous {
            std::env::set_var("DAOZAI_DATA_DIR", value);
        } else {
            std::env::remove_var("DAOZAI_DATA_DIR");
        }
        // 只删除自己随机新建的隔离目录；其中可能包含只读拉取的真实 ICS。
        if let Err(error) = std::fs::remove_dir_all(&self.root) {
            eprintln!("隔离目录清理失败：{error}");
        }
    }
}

#[derive(Clone)]
struct Owned {
    id: String,
    href: String,
    uid: String,
}
fn uid_checked(raw: &str, owned: &Owned) -> TestResult {
    let document = CalendarDocument::parse(raw).map_err(failure)?;
    let items = document.items().map_err(failure)?;
    ensure(
        !items.is_empty() && items.iter().all(|item| item.common().uid == owned.uid),
        "拒绝写入：资源 UID 不属于本轮测试",
    )
}
fn own_remote(client: &DavClient, owned: &Owned) -> TestResult<(String, String)> {
    let response = client.get(&owned.href).map_err(failure)?;
    uid_checked(&response.body, owned)?;
    let etag = response.etag.ok_or("自建资源缺少 ETag")?;
    calendar::caldav::dav::require_strong_etag(&etag).map_err(failure)?;
    Ok((response.body, etag))
}
fn guarded_sync(cfg: &Config, password: &str, owned: &[Owned]) -> TestResult {
    let state = organizer::read_state()?;
    // 所有潜在的网络写必须事先在本轮拥有列表中，现有对象永远不能进入队列。
    for pending in &state.pending {
        let resource = state
            .resources
            .get(&pending.resource_id)
            .ok_or("队列资源不存在")?;
        let item = owned
            .iter()
            .find(|item| item.id == resource.id && item.href == resource.href)
            .ok_or("拒绝同步：队列包含非本轮测试资源")?;
        for raw in [
            pending.content.as_deref(),
            pending.baseline.as_deref(),
            resource.local_ics.as_deref(),
            resource.baseline.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            uid_checked(raw, item)?;
        }
    }
    sync::sync_account(cfg, password)
}
fn stage(owned: &Owned, raw: Option<String>) -> TestResult {
    if let Some(raw) = &raw {
        uid_checked(raw, owned)?;
    }
    organizer::transaction(|state| {
        let resource = state.resources.get(&owned.id).ok_or("本轮资源已丢失")?;
        ensure(resource.href == owned.href, "资源 href 已变化")?;
        organizer::stage(state, &owned.id, raw)
    })
}
fn originals(client: &DavClient, urls: &[String]) -> TestResult<BTreeMap<String, String>> {
    let mut all = BTreeMap::new();
    for url in urls {
        for resource in client.list_resources(url).map_err(failure)?.resources {
            all.insert(resource.href, resource.etag);
        }
    }
    Ok(all)
}
fn cleanup(client: &DavClient, owned: &[Owned]) -> TestResult {
    let mut errors = Vec::new();
    for item in owned {
        match client.get(&item.href) {
            Err(error) if error.status == Some(404) => {}
            Err(error) => errors.push(format!("{}：{error}", item.uid)),
            Ok(response) => {
                let result = (|| {
                    uid_checked(&response.body, item)?;
                    let etag = response.etag.ok_or("清理自建资源时缺少 ETag")?;
                    client.delete(&item.href, &etag).map_err(failure)?;
                    Ok::<(), String>(())
                })();
                if let Err(error) = result {
                    errors.push(format!("{}：{error}", item.uid));
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("自建测试资源清理失败：{}", errors.join("；")))
    }
}

#[test]
#[ignore = "需用户明确授权；环境变量提供服务地址、两集合、用户名及临时应用密码"]
fn authorized_caldav_roundtrip_only_own_resources() -> TestResult {
    ensure(
        environment("DAOZAI_CALDAV_LIVE_AUTHORIZED")? == "own-resources-only",
        "必须明确限制为自建资源",
    )?;
    let server = environment("DAOZAI_CALDAV_LIVE_SERVER")?;
    let username = environment("DAOZAI_CALDAV_LIVE_USERNAME")?;
    // 凭据可由用户预先安全注入环境，或通过 Windows 凭据管理器提供；测试不保存密码。
    let password = match std::env::var("DAOZAI_CALDAV_LIVE_PASSWORD") {
        Ok(password) if !password.is_empty() => password,
        _ => {
            #[cfg(windows)]
            {
                keyring::Entry::new_with_target("Daozai.Caldav.Live.Test", "Daozai CalDAV Live Test", &username)
                    .map_err(failure)?.get_password()
                    .map_err(|_| "请先在 Windows 凭据管理器添加普通凭据 Daozai.Caldav.Live.Test，或由用户安全注入 DAOZAI_CALDAV_LIVE_PASSWORD".to_owned())?
            }
            #[cfg(not(windows))]
            {
                return Err("缺少 DAOZAI_CALDAV_LIVE_PASSWORD".into());
            }
        }
    };
    let events = environment("DAOZAI_CALDAV_LIVE_EVENTS")?;
    let todos = environment("DAOZAI_CALDAV_LIVE_TODOS")?;
    let _isolation = Isolated::new()?;
    let client = DavClient::new(&server, &username, &password).map_err(failure)?;
    let event_collection = client
        .validate_collection(&events, Component::Event, true)
        .map_err(failure)?;
    let todo_collection = client
        .validate_collection(&todos, Component::Todo, true)
        .map_err(failure)?;
    let urls = BTreeSet::from([event_collection.url.clone(), todo_collection.url.clone()])
        .into_iter()
        .collect::<Vec<_>>();
    let before = originals(&client, &urls)?;
    eprintln!(
        "只读权限与强 ETag 清单检查通过，原有资源 {} 个",
        before.len()
    );
    let cfg = Config {
        provider: Provider::Custom,
        account_id: organizer::new_id(),
        username,
        server: Some(server),
        ..Default::default()
    };
    organizer::transaction(|state| {
        state.accounts = vec![cfg.clone()];
        state.active_provider = Provider::Custom;
        Ok(())
    })?;
    let generation = organizer::read_state()?.generation;
    sync::initialize(&cfg, &events, &todos, &password, generation)?;
    let cfg = organizer::read_state()?.accounts.remove(0);
    ensure(
        organizer::read_state()?.pending.is_empty(),
        "只读初始化意外产生上传队列",
    )?;
    eprintln!("隔离仓库双集合初始化通过，未发送 PUT/DELETE");
    let run = format!("daozai-test-{}", organizer::new_id());
    let mut owned = Vec::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> TestResult {
        let now = chrono::Utc::now();
        let start = now + chrono::Duration::days(7);
        let first = start.format("%Y%m%dT%H%M%SZ");
        let end = (start + chrono::Duration::hours(1)).format("%Y%m%dT%H%M%SZ");
        let recurrence = (start + chrono::Duration::weeks(1)).format("%Y%m%dT%H%M%SZ");
        let moved = (start + chrono::Duration::weeks(1) + chrono::Duration::hours(2))
            .format("%Y%m%dT%H%M%SZ");
        let stamp = now.format("%Y%m%dT%H%M%SZ");
        let event_uid = format!("{run}-event");
        let task_uid = format!("{run}-task");
        let fixtures=vec![
            (event_collection.url.clone(),event_uid.clone(),format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Daozai Authorized Test//ZH\r\nBEGIN:VEVENT\r\nUID:{event_uid}\r\nDTSTAMP:{stamp}\r\nDTSTART:{first}\r\nDTEND:{end}\r\nSUMMARY:岛仔临时测试日程 {run}\r\nRRULE:FREQ=WEEKLY;COUNT=2\r\nX-DAOZAI-TEST:{run}\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:临时测试提醒一\r\nTRIGGER:-PT5M\r\nEND:VALARM\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:临时测试提醒二\r\nTRIGGER:-PT10M\r\nEND:VALARM\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:{event_uid}\r\nDTSTAMP:{stamp}\r\nRECURRENCE-ID:{recurrence}\r\nDTSTART:{moved}\r\nSUMMARY:岛仔临时测试移动例外\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n")),
            (todo_collection.url.clone(),task_uid.clone(),format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Daozai Authorized Test//ZH\r\nBEGIN:VTODO\r\nUID:{task_uid}\r\nDTSTAMP:{stamp}\r\nSUMMARY:岛仔临时测试待办 {run}\r\nDUE:{first}\r\nSTATUS:IN-PROCESS\r\nPERCENT-COMPLETE:40\r\nPRIORITY:3\r\nCATEGORIES:岛仔测试\r\nX-APPLE-SORT-ORDER:123\r\nX-DAOZAI-TEST:{run}\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:临时测试截止提醒\r\nTRIGGER;RELATED=END:-PT5M\r\nEND:VALARM\r\nEND:VTODO\r\nEND:VCALENDAR\r\n")),
            (todo_collection.url.clone(),format!("{run}-child"),format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Daozai Authorized Test//ZH\r\nBEGIN:VTODO\r\nUID:{run}-child\r\nDTSTAMP:{stamp}\r\nSUMMARY:岛仔临时测试子任务 {run}\r\nRELATED-TO;RELTYPE=PARENT:{task_uid}\r\nSTATUS:NEEDS-ACTION\r\nX-DAOZAI-TEST:{run}\r\nEND:VTODO\r\nEND:VCALENDAR\r\n"))
        ];
        for (collection, uid, raw) in fixtures {
            let item = organizer::transaction(|state| {
                let id = organizer::create_resource(state, &collection, raw)?;
                Ok(Owned {
                    id: id.clone(),
                    href: state.resources[&id].href.clone(),
                    uid,
                })
            })?;
            ensure(
                !before.contains_key(&item.href),
                "随机 href 与既有资源碰撞，停止测试",
            )?;
            // 登记后才允许第一次发送；即使后续失败也只清理这三个确切 href。
            owned.push(item);
        }
        guarded_sync(&cfg, &password, &owned)?;
        ensure(
            organizer::read_state()?.pending.is_empty(),
            "新建后仍有待确认队列",
        )?;
        eprintln!("新建日程系列/移动例外、待办和子任务，并 GET 确认通过");
        for item in &owned {
            let (raw, _) = own_remote(&client, item)?;
            ensure(raw.contains(&run), "服务端丢失本轮标识")?;
        }
        let event = &owned[0];
        let (raw, etag) = own_remote(&client, event)?;
        let mut doc = CalendarDocument::parse(&raw).map_err(failure)?;
        let item = doc
            .items()
            .map_err(failure)?
            .into_iter()
            .find(|item| item.common().recurrence_id.is_none())
            .ok_or("测试主系列丢失")?;
        doc.patch_event(
            item.common().index,
            &EventPatch {
                common: CommonPatch {
                    title: Patch::Set(format!("岛仔测试本机修改 {run}")),
                    ..Default::default()
                },
                ..Default::default()
            },
            chrono::Utc::now(),
        )
        .map_err(failure)?;
        stage(event, Some(doc.to_ics()))?;
        guarded_sync(&cfg, &password, &owned)?;
        let (raw, _) = own_remote(&client, event)?;
        ensure(
            raw.contains("RECURRENCE-ID:")
                && raw.matches("BEGIN:VALARM").count() == 2
                && raw.contains("RRULE:"),
            "修改标题丢失系列/例外/提醒",
        )?;
        let task = &owned[1];
        let (raw, _) = own_remote(&client, task)?;
        let mut doc = CalendarDocument::parse(&raw).map_err(failure)?;
        let index = doc.items().map_err(failure)?[0].common().index;
        doc.patch_task(
            index,
            &TaskPatch {
                state: Patch::Set(TaskStateChange::Complete),
                ..Default::default()
            },
            chrono::Utc::now(),
        )
        .map_err(failure)?;
        stage(task, Some(doc.to_ics()))?;
        guarded_sync(&cfg, &password, &owned)?;
        let (raw, _) = own_remote(&client, task)?;
        let mut doc = CalendarDocument::parse(&raw).map_err(failure)?;
        let CalendarItem::Task(item) = doc.items().map_err(failure)?.remove(0) else {
            return Err("测试待办类型变化".into());
        };
        ensure(
            item.status == Some(TaskStatus::Completed)
                && item.percent_complete == Some(100)
                && item.completed_at.is_some(),
            "完成状态未往返",
        )?;
        doc.patch_task(
            item.common.index,
            &TaskPatch {
                state: Patch::Set(TaskStateChange::Reopen {
                    previous: Some((TaskStatus::InProcess, 40)),
                }),
                ..Default::default()
            },
            chrono::Utc::now(),
        )
        .map_err(failure)?;
        stage(task, Some(doc.to_ics()))?;
        guarded_sync(&cfg, &password, &owned)?;
        let (raw, _) = own_remote(&client, task)?;
        let CalendarItem::Task(item) = CalendarDocument::parse(&raw)
            .map_err(failure)?
            .items()
            .map_err(failure)?
            .remove(0)
        else {
            return Err("测试待办丢失".into());
        };
        ensure(
            item.status == Some(TaskStatus::InProcess)
                && item.percent_complete == Some(40)
                && item.completed_at.is_none()
                && item.due.is_some(),
            "取消完成或标准截止时间未保留",
        )?;
        eprintln!("本机修改、完成/取消完成、标准 DUE 和扩展字段往返通过");
        // 反向修改与冲突仍仅访问本轮已登记资源。
        let (raw, current_etag) = own_remote(&client, event)?;
        let mut doc = CalendarDocument::parse(&raw).map_err(failure)?;
        let index = doc
            .items()
            .map_err(failure)?
            .into_iter()
            .find(|item| item.common().recurrence_id.is_none())
            .ok_or("主系列丢失")?
            .common()
            .index;
        doc.patch_event(
            index,
            &EventPatch {
                common: CommonPatch {
                    title: Patch::Set(format!("岛仔测试服务端修改 {run}")),
                    ..Default::default()
                },
                ..Default::default()
            },
            chrono::Utc::now(),
        )
        .map_err(failure)?;
        let external = doc.to_ics();
        uid_checked(&external, event)?;
        client
            .update(&event.href, &current_etag, &external)
            .map_err(failure)?;
        let stale = client
            .update(&event.href, &etag, &external)
            .err()
            .ok_or("服务端意外接受陈旧 ETag")?;
        ensure(stale.status == Some(412), "陈旧 ETag 未返回 412")?;
        guarded_sync(&cfg, &password, &owned)?;
        ensure(
            organizer::read_state()?.resources[&event.id]
                .local_ics
                .as_ref()
                .is_some_and(|raw| raw.contains("岛仔测试服务端修改")),
            "反向修改未同步",
        )?;
        let (raw, _) = own_remote(&client, event)?;
        let mut local = CalendarDocument::parse(&raw).map_err(failure)?;
        local
            .patch_event(
                index,
                &EventPatch {
                    common: CommonPatch {
                        title: Patch::Set(format!("岛仔测试冲突本机 {run}")),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                chrono::Utc::now(),
            )
            .map_err(failure)?;
        stage(event, Some(local.to_ics()))?;
        let (raw, etag) = own_remote(&client, event)?;
        let mut remote = CalendarDocument::parse(&raw).map_err(failure)?;
        remote
            .patch_event(
                index,
                &EventPatch {
                    common: CommonPatch {
                        title: Patch::Set(format!("岛仔测试冲突服务端 {run}")),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                chrono::Utc::now(),
            )
            .map_err(failure)?;
        let raw = remote.to_ics();
        uid_checked(&raw, event)?;
        client.update(&event.href, &etag, &raw).map_err(failure)?;
        guarded_sync(&cfg, &password, &owned)?;
        let state = organizer::read_state()?;
        let conflict = state
            .conflicts
            .iter()
            .find(|item| item.resource_id == event.id && !item.resolved)
            .ok_or("未形成三方冲突")?;
        sync::resolve_conflict(&conflict.id, true)?;
        guarded_sync(&cfg, &password, &owned)?;
        ensure(
            own_remote(&client, event)?.0.contains("岛仔测试冲突本机"),
            "冲突选择本机版本未发送",
        )?;
        eprintln!("服务端反向修改、陈旧 ETag 412 和三方冲突选择通过");
        let task = &owned[1];
        let (raw, _) = own_remote(&client, task)?;
        let mut doc = CalendarDocument::parse(&raw).map_err(failure)?;
        let index = doc.items().map_err(failure)?[0].common().index;
        doc.patch_task(
            index,
            &TaskPatch {
                common: CommonPatch {
                    title: Patch::Set(format!("岛仔测试离线修改 {run}")),
                    ..Default::default()
                },
                ..Default::default()
            },
            chrono::Utc::now(),
        )
        .map_err(failure)?;
        stage(task, Some(doc.to_ics()))?;
        sync::set_enabled(&cfg.account_id, false)?;
        let disabled = organizer::read_state()?.accounts.remove(0);
        let (_, etag) = own_remote(&client, task)?;
        guarded_sync(&disabled, &password, &owned)?;
        ensure(own_remote(&client, task)?.1 == etag, "关闭后仍发送修改")?;
        sync::set_enabled(&cfg.account_id, true)?;
        guarded_sync(&cfg, &password, &owned)?;
        ensure(
            own_remote(&client, task)?.0.contains("岛仔测试离线修改"),
            "重启后待处理修改未发送",
        )?;
        // 反向删除子任务，确认不会级联删除父任务。
        let child = &owned[2];
        let (_, etag) = own_remote(&client, child)?;
        client.delete(&child.href, &etag).map_err(failure)?;
        guarded_sync(&cfg, &password, &owned)?;
        ensure(
            organizer::read_state()?.resources[&child.id]
                .local_ics
                .is_none(),
            "反向删除未同步",
        )?;
        own_remote(&client, task)?;
        for item in [&owned[0], &owned[1]] {
            stage(item, None)?;
        }
        guarded_sync(&cfg, &password, &owned)?;
        for item in &owned {
            ensure(
                client
                    .get(&item.href)
                    .err()
                    .is_some_and(|error| error.status == Some(404)),
                "自建测试资源未删除",
            )?;
        }
        eprintln!("关闭不写入、重新启用、双向删除及无级联删除通过");
        Ok(())
    })).unwrap_or_else(|_| Err("真实测试发生 panic，已转入自建资源清理".into()));
    // 业务断言失败也执行逐资源的 UID+ETag 条件清理，不发送集合级 DELETE。
    let cleaned = cleanup(&client, &owned);
    let after = originals(&client, &urls)?;
    let unchanged = before
        .iter()
        .all(|(href, etag)| after.get(href) == Some(etag));
    let absent = owned.iter().all(|item| !after.contains_key(&item.href));
    eprintln!(
        "清理检查：本轮 {} 个测试资源均已移除={}；原有 {} 个资源 href/ETag 不变={}",
        owned.len(),
        absent,
        before.len(),
        unchanged
    );
    cleaned?;
    result?;
    ensure(absent, "仍有本轮测试资源残留")?;
    ensure(
        unchanged,
        "原有资源清单变化，需区分其他客户端并发操作；测试从未向这些 href 发写请求",
    )?;
    Ok(())
}
