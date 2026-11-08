//! CalDAV 资源同步状态机。仅由 Calendar Service 串行调用这些入口。
//!
//! 网络请求绝不置于 organizer::transaction 内；每次请求前重新验证绑定，
//! 写请求先持久化 Unknown。响应确认只消费对应 operation/revision，后续编辑保留。
use super::dav::{self, CalendarResource, Collection, Component, DavClient, DavError, ResourceRef};
use super::ical::CalendarDocument;
use crate::calendar::{Config, ConflictPolicy};
use crate::organizer::{
    self, Binding, CollectionState, Conflict, PendingChange, Resource, SendState, State,
};
use std::collections::{BTreeMap, BTreeSet};

const INITIAL_SCAN_ATTEMPTS: usize = 3;
const CONFLICT_ATTEMPTS: usize = 3;

// 测试用独立仓库根目录，避免网络状态机测试接触用户的活动数据。
#[cfg(test)]
thread_local! {
    static TEST_ROOT: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
}
fn read_state() -> Result<State, String> {
    #[cfg(test)]
    if let Some(root) = TEST_ROOT.with(|root| root.borrow().clone()) {
        return organizer::store::read_at(&root);
    }
    organizer::read_state()
}
fn transaction<R>(change: impl FnOnce(&mut State) -> Result<R, String>) -> Result<R, String> {
    #[cfg(test)]
    if let Some(root) = TEST_ROOT.with(|root| root.borrow().clone()) {
        return organizer::store::transaction_at(&root, change);
    }
    organizer::transaction(change)
}

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn client(cfg: &Config, password: &str) -> Result<DavClient, String> {
    if !cfg.provider.two_way_capable() || cfg.account_id.is_empty() {
        return Err("只有具备稳定 account_id 的 CalDAV 账号可使用资源同步".into());
    }
    DavClient::new(&cfg.base(), &cfg.username, password).map_err(text)
}
fn account<'a>(state: &'a State, id: &str) -> Result<&'a Config, String> {
    state
        .accounts
        .iter()
        .find(|cfg| cfg.account_id == id)
        .ok_or_else(|| "同步账号已断开".into())
}
fn same_identity(a: &Config, b: &Config) -> bool {
    a.account_id == b.account_id
        && a.provider == b.provider
        && a.username == b.username
        && a.base() == b.base()
}
fn binding_matches_config(binding: &Binding, cfg: &Config) -> bool {
    binding.account_id == cfg.account_id
        && cfg.provider.two_way_capable()
        && cfg.event_collection.as_deref() == Some(binding.event_collection.as_str())
        && cfg.todo_collection.as_deref() == Some(binding.todo_collection.as_str())
}
/// 允许关闭/切换期间确认已经发出的请求，但绝不把旧 generation 的响应写入新绑定。
fn check_binding<'a>(
    state: &'a State,
    cfg: &Config,
    expected: &Binding,
) -> Result<&'a Config, String> {
    let current = state.binding.as_ref().ok_or("同步绑定已移除")?;
    let saved = account(state, &cfg.account_id)?;
    if current.generation != expected.generation
        || current.account_id != expected.account_id
        || current.event_collection != expected.event_collection
        || current.todo_collection != expected.todo_collection
        || !same_identity(saved, cfg)
        || !binding_matches_config(current, saved)
        || !binding_matches_config(current, cfg)
    {
        return Err("同步绑定、配置或 generation 已变化，请重新同步".into());
    }
    Ok(saved)
}
fn check_target(resource: &Resource, binding: &Binding) -> Result<(), String> {
    if resource.account_id != binding.account_id
        || (resource.collection_url != binding.event_collection
            && resource.collection_url != binding.todo_collection)
    {
        return Err("同步资源不属于当前账号或目标集合".into());
    }
    let collection = url::Url::parse(&resource.collection_url).map_err(text)?;
    let href = url::Url::parse(&resource.href).map_err(text)?;
    let prefix = format!("{}/", collection.path().trim_end_matches('/'));
    let suffix = href
        .path()
        .strip_prefix(&prefix)
        .ok_or("同步资源地址位于目标集合之外")?;
    let lower = href.path().to_ascii_lowercase();
    if href.origin() != collection.origin()
        || suffix.is_empty()
        || suffix.contains('/')
        || lower.contains("%2f")
        || lower.contains("%5c")
        || href.fragment().is_some()
        || !href.username().is_empty()
        || href.password().is_some()
    {
        return Err("同步资源地址不属于目标集合的直接资源".into());
    }
    Ok(())
}
fn check_write(
    state: &State,
    cfg: &Config,
    binding: &Binding,
    resource: &Resource,
) -> Result<(), String> {
    let saved = check_binding(state, cfg, binding)?;
    if !cfg.two_way || !saved.two_way || state.binding.as_ref().is_none_or(|b| b.switching) {
        return Err("双向同步已关闭或正在切换，已停止发送新写请求".into());
    }
    check_target(resource, binding)?;
    if !state
        .collections
        .get(&resource.collection_url)
        .is_some_and(|collection| collection.writable && collection.full_scan)
    {
        return Err("目标集合尚未完成可写验证和完整扫描".into());
    }
    Ok(())
}
fn validate_ics(ics: &str, writable: bool) -> Result<(), String> {
    let document = CalendarDocument::parse(ics).map_err(text)?;
    let items = document.items().map_err(text)?;
    if writable {
        document.validate_for_write().map_err(text)?;
        let first = items.first().ok_or("资源没有可写的 VEVENT/VTODO 组件")?;
        let kind = document
            .component(first.common().index)
            .map_err(text)?
            .name();
        let mut identities = Vec::new();
        for item in &items {
            let common = item.common();
            if common.uid != first.common().uid
                || !document
                    .component(common.index)
                    .map_err(text)?
                    .name()
                    .eq_ignore_ascii_case(kind)
            {
                return Err(
                    "一个 CalDAV 资源必须是同一 UID、同一组件类型的系列及例外；不能上传多组对象"
                        .into(),
                );
            }
            if identities.contains(&common.recurrence_id) {
                return Err("同一资源包含重复的 UID/RECURRENCE-ID，不能条件写入".into());
            }
            identities.push(common.recurrence_id.clone());
        }
        if document
            .root()
            .children()
            .any(|component| matches!(component.name(), "VJOURNAL" | "VFREEBUSY"))
        {
            return Err("资源包含当前不支持写入的主组件，原始 ICS 已保留".into());
        }
    }
    Ok(())
}
/// 供本机 stage 在事务内提前拒绝不可独立编辑的资源；无网络和持久化操作。
pub(crate) fn validate_resource_for_write(raw: &str) -> Result<(), String> {
    validate_ics(raw, true)
}
fn validate_component_target(
    state: &State,
    resource: &Resource,
    content: &str,
) -> Result<(), String> {
    let collection = state
        .collections
        .get(&resource.collection_url)
        .ok_or("目标集合已移除")?;
    let document = CalendarDocument::parse(content).map_err(text)?;
    for component in document.root().children() {
        if (component.name().eq_ignore_ascii_case("VEVENT") && !collection.events)
            || (component.name().eq_ignore_ascii_case("VTODO") && !collection.todos)
        {
            return Err("资源组件类型不属于此绑定的目标集合".into());
        }
    }
    Ok(())
}
fn references(list: &[ResourceRef]) -> Result<BTreeMap<String, String>, String> {
    let mut result = BTreeMap::new();
    for item in list {
        dav::require_strong_etag(&item.etag).map_err(text)?;
        if result
            .insert(item.href.clone(), item.etag.clone())
            .is_some()
        {
            return Err("服务端返回重复资源地址".into());
        }
    }
    Ok(result)
}
fn validate_resources(resources: &[CalendarResource]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for resource in resources {
        if !seen.insert(&resource.href) {
            return Err("服务端返回重复资源正文".into());
        }
        dav::require_strong_etag(&resource.etag).map_err(text)?;
        validate_ics(&resource.ics, false)?;
    }
    Ok(())
}

/// UI 必须先完成清空警告和编辑冻结，再调用本入口。验证期间只读服务端。
/// expected_generation 指 organizer State.generation，不是 Binding.generation。
/// 任一验证/提交失败均保留原活动数据；同一绑定重新开启不会再次清空。
pub fn initialize(
    cfg: &Config,
    event_path: &str,
    todo_path: &str,
    password: &str,
    expected_generation: u64,
) -> Result<(), String> {
    let original = read_state()?;
    if original.generation != expected_generation {
        return Err("初始化前数据已变化，请重新确认".into());
    }
    let saved = account(&original, &cfg.account_id)?.clone();
    if !same_identity(&saved, cfg) {
        return Err("初始化账号与当前保存的连接不一致".into());
    }
    let dav = client(cfg, password)?;
    let events = dav
        .validate_collection(event_path, Component::Event, true)
        .map_err(text)?;
    let todos = dav
        .validate_collection(todo_path, Component::Todo, true)
        .map_err(text)?;
    let mut collections = BTreeMap::new();
    collections.insert(events.url.clone(), events.clone());
    collections.insert(todos.url.clone(), todos.clone());
    // 两个集合共用一轮前后清单复核；同 URL 仅读取一次。不能拿时间窗口推断完整性。
    let mut snapshots = None;
    let mut last_error = String::new();
    for _ in 0..INITIAL_SCAN_ATTEMPTS {
        match initial_snapshot(&dav, &collections) {
            Ok(result) => {
                snapshots = Some(result);
                break;
            }
            Err(error) => last_error = error,
        }
    }
    let snapshots = snapshots.ok_or_else(|| format!("无法取得一致的首次完整快照：{last_error}"))?;
    transaction(|state| {
        if state.generation != expected_generation || account(state, &cfg.account_id)? != &saved {
            return Err("初始化验证期间本机数据或连接已变化，请重新确认；原数据已保留".into());
        }
        let resume = state.binding.as_ref().is_some_and(|binding| {
            binding.account_id == cfg.account_id
                && binding.event_collection == events.url
                && binding.todo_collection == todos.url
        });
        if !resume
            && state
                .pending
                .iter()
                .any(|change| change.state == SendState::Unknown)
        {
            return Err("旧绑定仍有结果未知的请求，请先对账后再更换集合".into());
        }
        let generation = if resume {
            state.binding.as_ref().unwrap().generation
        } else {
            state
                .binding
                .as_ref()
                .map_or(0, |b| b.generation)
                .checked_add(1)
                .ok_or("绑定 generation 已耗尽")?
        };
        if !resume {
            state.local_events.clear();
            state.todos.clear();
            state.resources.clear();
            state.pending.clear();
            state.conflicts.clear();
            state.metadata.clear();
            state.collections.clear();
        }
        let mut active = cfg.clone();
        active.two_way = true;
        active.event_collection = Some(events.url.clone());
        active.todo_collection = Some(todos.url.clone());
        *state
            .accounts
            .iter_mut()
            .find(|a| a.account_id == cfg.account_id)
            .ok_or("同步账号已移除")? = active;
        state.binding = Some(Binding {
            account_id: cfg.account_id.clone(),
            generation,
            event_collection: events.url.clone(),
            todo_collection: todos.url.clone(),
            switching: false,
        });
        for (url, remote) in &snapshots {
            let collection = &collections[url];
            state.collections.insert(
                url.clone(),
                CollectionState {
                    url: url.clone(),
                    events: *url == events.url,
                    todos: *url == todos.url,
                    writable: collection.privileges.writable(),
                    full_scan: true,
                    sync_token: None,
                    last_sync: Some(chrono::Utc::now().to_rfc3339()),
                    error: None,
                },
            );
            apply_download(state, cfg, url, remote, true, &[])?;
        }
        super::projection::refresh(state)
    })
}
fn initial_snapshot(
    dav: &DavClient,
    collections: &BTreeMap<String, Collection>,
) -> Result<BTreeMap<String, Vec<CalendarResource>>, String> {
    let mut before = BTreeMap::new();
    let mut result = BTreeMap::new();
    for url in collections.keys() {
        let listing = dav.list_resources(url).map_err(text)?;
        if listing.collection_url != *url {
            return Err("首次扫描的集合发生重定向，请重新验证".into());
        }
        references(&listing.resources)?;
        before.insert(url.clone(), listing.resources);
    }
    for (url, refs) in &before {
        let remote = dav.multiget(url, refs).map_err(text)?;
        validate_resources(&remote)?;
        if references(refs)?
            != remote
                .iter()
                .map(|r| (r.href.clone(), r.etag.clone()))
                .collect()
        {
            return Err("首次扫描未完整读取全部资源".into());
        }
        result.insert(url.clone(), remote);
    }
    for (url, refs) in &before {
        let after = dav.list_resources(url).map_err(text)?;
        if after.collection_url != *url || references(refs)? != references(&after.resources)? {
            return Err("服务端在初始化期间发生变化".into());
        }
    }
    Ok(result)
}

/// 关闭保留缓存、队列、冲突；打开仅恢复已初始化的同一绑定。
/// 必须由串行 Service 在已发送请求确认后调用；UI 可先设置 switching 阻止新编辑。
pub fn set_enabled(account_id: &str, enabled: bool) -> Result<(), String> {
    transaction(|state| {
        let binding = state.binding.as_ref().ok_or("尚未初始化 CalDAV 双向同步")?;
        let saved = account(state, account_id)?;
        if !binding_matches_config(binding, saved) || binding.account_id != account_id {
            return Err("账号或集合已变化，请重新执行初始化".into());
        }
        state
            .accounts
            .iter_mut()
            .find(|cfg| cfg.account_id == account_id)
            .unwrap()
            .two_way = enabled;
        state.binding.as_mut().unwrap().switching = false;
        Ok(())
    })
}

/// 每次先对账 Unknown、拉取服务端，再尝试发送每个资源的最早待处理 revision。
/// 单资源失败持久化在队列/集合中，继续处理其他资源；返回聚合错误供状态栏显示。
pub fn sync_account(cfg: &Config, password: &str) -> Result<(), String> {
    let dav = client(cfg, password)?;
    let state = read_state()?;
    let binding = state.binding.clone().ok_or("尚未初始化 CalDAV 同步")?;
    check_binding(&state, cfg, &binding)?;
    let mut errors = Vec::new();
    let unknown: Vec<_> = state
        .pending
        .iter()
        .filter(|p| p.state == SendState::Unknown)
        .map(|p| p.id.clone())
        .collect();
    for id in unknown {
        if let Err(error) = reconcile(&dav, cfg, &binding, &id) {
            record_pending_error(cfg, &binding, &id, &error)?;
            errors.push(error);
        }
    }
    let urls: BTreeSet<_> = [
        binding.event_collection.clone(),
        binding.todo_collection.clone(),
    ]
    .into_iter()
    .collect();
    let mut complete = BTreeSet::new();
    for url in urls {
        match download_collection(&dav, cfg, &binding, &url) {
            Ok(()) => {
                complete.insert(url);
            }
            Err(error) => {
                record_collection_error(cfg, &binding, &url, &error)?;
                errors.push(error);
            }
        }
    }
    let state = read_state()?;
    let saved = check_binding(&state, cfg, &binding)?;
    if saved.two_way && cfg.two_way && !state.binding.as_ref().unwrap().switching {
        let ids: BTreeSet<_> = state
            .pending
            .iter()
            .filter_map(|p| state.resources.get(&p.resource_id))
            .filter(|r| complete.contains(&r.collection_url))
            .map(|r| r.id.clone())
            .collect();
        for id in ids {
            if let Err(error) = upload_resource(&dav, cfg, &binding, &id) {
                errors.push(error);
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}

fn record_pending_error(
    cfg: &Config,
    binding: &Binding,
    id: &str,
    error: &str,
) -> Result<(), String> {
    transaction(|state| {
        check_binding(state, cfg, binding)?;
        if let Some(pending) = state.pending.iter_mut().find(|p| p.id == id) {
            pending.error = Some(error.into());
        }
        Ok(())
    })
}
fn record_collection_error(
    cfg: &Config,
    binding: &Binding,
    url: &str,
    error: &str,
) -> Result<(), String> {
    transaction(|state| {
        check_binding(state, cfg, binding)?;
        if let Some(collection) = state.collections.get_mut(url) {
            collection.error = Some(error.into());
        }
        Ok(())
    })
}

fn download_collection(
    dav: &DavClient,
    cfg: &Config,
    binding: &Binding,
    url: &str,
) -> Result<(), String> {
    let state = read_state()?;
    let saved = check_binding(&state, cfg, binding)?;
    let component = if url == binding.event_collection {
        Component::Event
    } else {
        Component::Todo
    };
    let capabilities = dav
        .validate_collection(url, component, saved.two_way && !binding.switching)
        .map_err(text)?;
    if capabilities.url != url {
        return Err("同步目标集合发生变化，请重新初始化".into());
    }
    if url == binding.event_collection && url == binding.todo_collection {
        dav.validate_collection(url, Component::Todo, saved.two_way && !binding.switching)
            .map_err(text)?;
    }
    let previous = state.collections.get(url).ok_or("集合尚未初始化")?;
    let response = if capabilities.supports_sync {
        dav.sync_collection(url, previous.sync_token.as_deref())
            .map_err(text)?
    } else {
        let listing = dav.list_resources(url).map_err(text)?;
        dav::SyncResult {
            collection_url: listing.collection_url,
            full: true,
            resources: listing.resources,
            deleted: vec![],
            token: None,
        }
    };
    if response.collection_url != url {
        return Err("资源清单返回的集合与当前绑定不一致".into());
    }
    references(&response.resources)?;
    let changed: Vec<_> = response
        .resources
        .iter()
        .filter(|remote| {
            !state.resources.values().any(|local| {
                local.account_id == cfg.account_id
                    && local.collection_url == url
                    && local.href == remote.href
                    && local.etag.as_deref() == Some(remote.etag.as_str())
                    && local.baseline.is_some()
            })
        })
        .cloned()
        .collect();
    let bodies = dav.multiget(url, &changed).map_err(text)?;
    validate_resources(&bodies)?;
    let mut remote: BTreeMap<_, _> = bodies.into_iter().map(|r| (r.href.clone(), r)).collect();
    for reference in &response.resources {
        if !remote.contains_key(&reference.href) {
            let known = state
                .resources
                .values()
                .find(|r| {
                    r.account_id == cfg.account_id
                        && r.collection_url == url
                        && r.href == reference.href
                        && r.etag.as_deref() == Some(reference.etag.as_str())
                })
                .ok_or("资源基线在拉取期间发生变化")?;
            remote.insert(
                reference.href.clone(),
                CalendarResource {
                    href: reference.href.clone(),
                    etag: reference.etag.clone(),
                    ics: known.baseline.clone().ok_or("缓存缺少服务端基线")?,
                },
            );
        }
    }
    // 到这里清单、正文及解析全部成功，才允许缺席删除和 token 推进。
    let remote: Vec<_> = remote.into_values().collect();
    transaction(|state| {
        check_binding(state, cfg, binding)?;
        apply_download(state, cfg, url, &remote, response.full, &response.deleted)?;
        let collection = state.collections.get_mut(url).ok_or("集合已移除")?;
        collection.writable = capabilities.privileges.writable();
        collection.full_scan |= response.full;
        collection.sync_token = response.token;
        collection.last_sync = Some(chrono::Utc::now().to_rfc3339());
        collection.error = None;
        super::projection::refresh(state)
    })
}

fn has_pending(state: &State, id: &str) -> bool {
    state.pending.iter().any(|p| p.resource_id == id)
}
fn paused(state: &State, id: &str) -> bool {
    state
        .conflicts
        .iter()
        .any(|c| c.resource_id == id && !c.resolved)
}
fn apply_download(
    state: &mut State,
    cfg: &Config,
    collection: &str,
    remote: &[CalendarResource],
    full: bool,
    deleted: &[String],
) -> Result<(), String> {
    let present: BTreeSet<_> = remote.iter().map(|r| r.href.as_str()).collect();
    for remote in remote {
        let existing = state
            .resources
            .values()
            .find(|r| {
                r.account_id == cfg.account_id
                    && r.collection_url == collection
                    && r.href == remote.href
            })
            .map(|r| r.id.clone());
        if let Some(id) = existing {
            accept_remote(
                state,
                cfg,
                &id,
                Some(remote.ics.clone()),
                Some(remote.etag.clone()),
            )?;
        } else {
            let id = organizer::new_id();
            state.resources.insert(
                id.clone(),
                Resource {
                    id,
                    account_id: cfg.account_id.clone(),
                    collection_url: collection.into(),
                    href: remote.href.clone(),
                    etag: Some(remote.etag.clone()),
                    local_ics: Some(remote.ics.clone()),
                    baseline: Some(remote.ics.clone()),
                    revision: 0,
                },
            );
        }
    }
    let missing: Vec<_> = state
        .resources
        .values()
        .filter(|r| {
            r.account_id == cfg.account_id
                && r.collection_url == collection
                && r.baseline.is_some()
                && !present.contains(r.href.as_str())
                && (full || deleted.contains(&r.href))
        })
        .map(|r| r.id.clone())
        .collect();
    for id in missing {
        accept_remote(state, cfg, &id, None, None)?;
    }
    Ok(())
}
fn accept_remote(
    state: &mut State,
    cfg: &Config,
    id: &str,
    content: Option<String>,
    etag: Option<String>,
) -> Result<(), String> {
    if paused(state, id)
        || state
            .pending
            .iter()
            .any(|p| p.resource_id == id && p.state == SendState::Unknown)
    {
        return Ok(());
    }
    let resource = state.resources.get(id).ok_or("资源已移除")?.clone();
    if has_pending(state, id) {
        if resource.baseline == content {
            // 正文没有变化但 ETag 可能已更新；仅推进尚未发送操作的条件版本。
            let current = state.resources.get_mut(id).unwrap();
            current.etag = etag.clone();
            for change in state
                .pending
                .iter_mut()
                .filter(|p| p.resource_id == id && p.state == SendState::Ready)
            {
                change.base_etag = etag.clone();
                change.baseline = content.clone();
            }
        } else {
            archive_conflict(state, cfg, id, content, etag)?;
        }
    } else {
        let current = state.resources.get_mut(id).unwrap();
        current.local_ics = content.clone();
        current.baseline = content;
        current.etag = etag;
        // 删除墓碑留存资源身份，后续同 href 重新出现仍可复用 ID。
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct Remote {
    content: Option<String>,
    etag: Option<String>,
}
fn read_remote(dav: &DavClient, resource: &Resource) -> Result<Remote, String> {
    match dav.get(&resource.href) {
        Ok(response) => {
            if response.final_url != resource.href {
                return Err("固定资源地址发生重定向，不能确认写入结果".into());
            }
            let etag = response.etag.ok_or("GET 未返回 ETag，不能确认资源版本")?;
            dav::require_strong_etag(&etag).map_err(text)?;
            validate_ics(&response.body, false)?;
            Ok(Remote {
                content: Some(response.body),
                etag: Some(etag),
            })
        }
        Err(error) if error.status == Some(404) => Ok(Remote {
            content: None,
            etag: None,
        }),
        Err(error) => Err(text(error)),
    }
}

/// 仅删除这个已发送 operation；请求期间的新 revision 内容/删除意图保持不变。
fn acknowledge(state: &mut State, sent: &PendingChange, remote: Remote) -> Result<(), String> {
    if !state
        .pending
        .iter()
        .any(|p| p.id == sent.id && p.revision == sent.revision)
    {
        return Err("待确认操作已变化，不能确认其他 revision".into());
    }
    let current = state
        .resources
        .get_mut(&sent.resource_id)
        .ok_or("待确认资源已移除")?;
    current.baseline = remote.content.clone();
    current.etag = remote.etag.clone();
    if current.revision == sent.revision {
        current.local_ics = remote.content.clone();
    }
    state.pending.retain(|p| p.id != sent.id);
    for next in state.pending.iter_mut().filter(|p| {
        p.resource_id == sent.resource_id
            && p.revision > sent.revision
            && p.state == SendState::Ready
    }) {
        // 后继编辑以此 operation 的旧基线发起；仅在基线链吻合时重新设置条件版本。
        if next.base_etag == sent.base_etag && next.baseline == sent.baseline {
            next.base_etag = remote.etag.clone();
            next.baseline = remote.content.clone();
        }
    }
    Ok(())
}
fn commit_ack(
    cfg: &Config,
    binding: &Binding,
    sent: &PendingChange,
    remote: Remote,
) -> Result<(), String> {
    transaction(|state| {
        check_binding(state, cfg, binding)?;
        acknowledge(state, sent, remote)?;
        super::projection::refresh(state)
    })
}

fn reconcile(dav: &DavClient, cfg: &Config, binding: &Binding, id: &str) -> Result<(), String> {
    let state = read_state()?;
    check_binding(&state, cfg, binding)?;
    let Some(sent) = state
        .pending
        .iter()
        .find(|p| p.id == id && p.state == SendState::Unknown)
        .cloned()
    else {
        return Ok(());
    };
    if paused(&state, &sent.resource_id) {
        return Ok(());
    }
    let resource = state
        .resources
        .get(&sent.resource_id)
        .ok_or("结果未知操作引用了缺失资源")?;
    check_target(resource, binding)?;
    let remote = read_remote(dav, resource)?;
    if remote.content == sent.content {
        return commit_ack(cfg, binding, &sent, remote);
    }
    // 固定 href 仍处于原基线，重试原来的条件写入是安全的。创建的 404 也属于此情形。
    if remote.content == sent.baseline && remote.etag == sent.base_etag {
        return transaction(|state| {
            check_binding(state, cfg, binding)?;
            if let Some(change) = state.pending.iter_mut().find(|p| p.id == sent.id) {
                change.state = SendState::Ready;
            }
            Ok(())
        });
    }
    transaction(|state| {
        check_binding(state, cfg, binding)?;
        archive_conflict(state, cfg, &sent.resource_id, remote.content, remote.etag)?;
        super::projection::refresh(state)
    })
}

fn archive_conflict(
    state: &mut State,
    cfg: &Config,
    id: &str,
    remote: Option<String>,
    etag: Option<String>,
) -> Result<(), String> {
    let resource = state.resources.get(id).ok_or("冲突资源已移除")?;
    let saved = account(state, &cfg.account_id)?;
    // 关闭期间只允许保留待处理版本；账号的自动 Server 策略也不能丢弃离线编辑。
    let policy = if saved.two_way && state.binding.as_ref().is_some_and(|b| !b.switching) {
        saved.conflict_policy
    } else {
        ConflictPolicy::Ask
    };
    let conflict = Conflict {
        id: organizer::new_id(),
        resource_id: id.into(),
        baseline: resource.baseline.clone(),
        local: resource.local_ics.clone(),
        remote,
        remote_etag: etag,
        revision: resource.revision,
        resolved: policy != ConflictPolicy::Ask,
    };
    state.conflicts.push(conflict.clone());
    match policy {
        ConflictPolicy::Ask => {
            // 已通过 GET 确认冲突，旧 Unknown 不再代表未完成对账，但资源保持暂停。
            for pending in state.pending.iter_mut().filter(|p| p.resource_id == id) {
                pending.state = SendState::Ready;
                pending.error = Some("资源存在冲突，等待选择保留版本".into());
            }
        }
        ConflictPolicy::Server => choose_conflict(state, &conflict, false)?,
        ConflictPolicy::Client => choose_conflict(state, &conflict, true)?,
    }
    Ok(())
}
fn choose_conflict(
    state: &mut State,
    conflict: &Conflict,
    prefer_local: bool,
) -> Result<(), String> {
    let resource = state
        .resources
        .get_mut(&conflict.resource_id)
        .ok_or("冲突资源已移除")?;
    if resource.revision != conflict.revision {
        return Err("冲突后本机 revision 已变化，请重新检查版本".into());
    }
    resource.baseline = conflict.remote.clone();
    resource.etag = conflict.remote_etag.clone();
    state
        .pending
        .retain(|p| p.resource_id != conflict.resource_id);
    if prefer_local {
        if resource.local_ics != conflict.remote {
            state.pending.push(PendingChange {
                id: organizer::new_id(),
                resource_id: resource.id.clone(),
                revision: resource.revision,
                base_etag: resource.etag.clone(),
                baseline: resource.baseline.clone(),
                content: resource.local_ics.clone(),
                state: SendState::Ready,
                attempts: 0,
                error: None,
            });
        }
    } else {
        resource.local_ics = conflict.remote.clone();
    }
    Ok(())
}
/// 选择只在仓库中排队；下一次 sync 会重新拉取/检查远端，并始终使用条件写入。
pub fn resolve_conflict(id: &str, prefer_local: bool) -> Result<(), String> {
    transaction(|state| {
        let index = state
            .conflicts
            .iter()
            .position(|c| c.id == id && !c.resolved)
            .ok_or("冲突已解决或不存在")?;
        let conflict = state.conflicts[index].clone();
        let resource = state
            .resources
            .get(&conflict.resource_id)
            .ok_or("冲突资源已移除")?;
        let binding = state.binding.as_ref().ok_or("同步绑定已移除")?;
        let cfg = account(state, &resource.account_id)?;
        if !binding_matches_config(binding, cfg) || binding.switching {
            return Err("同步绑定正在切换或配置已变化".into());
        }
        check_target(resource, binding)?;
        choose_conflict(state, &conflict, prefer_local)?;
        state.conflicts[index].resolved = true;
        super::projection::refresh(state)
    })
}

fn prepare_send(
    state: &mut State,
    cfg: &Config,
    binding: &Binding,
    id: &str,
) -> Result<Option<(Resource, PendingChange)>, String> {
    if paused(state, id) {
        return Ok(None);
    }
    let Some(change) = state
        .pending
        .iter()
        .filter(|p| p.resource_id == id)
        .min_by_key(|p| p.revision)
        .cloned()
    else {
        return Ok(None);
    };
    if change.state == SendState::Unknown {
        return Ok(None);
    }
    let resource = state.resources.get(id).ok_or("上传资源已移除")?.clone();
    check_write(state, cfg, binding, &resource)?;
    if change.content.is_none() && change.baseline.is_none() && change.base_etag.is_none() {
        // 尚未创建的本机资源被删除：只取消本机队列，不发送无条件 DELETE。
        state.pending.retain(|p| p.id != change.id);
        if !has_pending(state, id) && resource.local_ics.is_none() {
            state.resources.remove(id);
        }
        return Ok(None);
    }
    if change.baseline.is_some() != change.base_etag.is_some() {
        return Err("待上传操作的基线与 ETag 不一致，已阻止写入".into());
    }
    if let Some(etag) = &change.base_etag {
        dav::require_strong_etag(etag).map_err(text)?;
    }
    if let Some(content) = change.content.as_ref().or(change.baseline.as_ref()) {
        validate_ics(content, true)?;
        validate_component_target(state, &resource, content)?;
    }
    let pending = state
        .pending
        .iter_mut()
        .find(|p| p.id == change.id)
        .unwrap();
    pending.state = SendState::Unknown;
    pending.attempts = pending.attempts.checked_add(1).ok_or("重试计数已耗尽")?;
    pending.error = None;
    Ok(Some((resource, pending.clone())))
}
fn upload_resource(
    dav: &DavClient,
    cfg: &Config,
    binding: &Binding,
    id: &str,
) -> Result<(), String> {
    for attempt in 0..CONFLICT_ATTEMPTS {
        let prepared = transaction(|state| prepare_send(state, cfg, binding, id));
        let (resource, sent) = match prepared {
            Ok(Some(pair)) => pair,
            Ok(None) => return Ok(()),
            Err(error) => {
                let state = read_state()?;
                if let Some(change) = state
                    .pending
                    .iter()
                    .filter(|p| p.resource_id == id)
                    .min_by_key(|p| p.revision)
                {
                    record_pending_error(cfg, binding, &change.id, &error)?;
                }
                return Err(error);
            }
        };
        // Unknown 已落盘后再读一次开关/目标；关闭发生在此间时允许保留 Unknown 下轮 GET 对账。
        let latest = read_state()?;
        if let Err(error) = check_write(&latest, cfg, binding, &resource) {
            record_pending_error(cfg, binding, &sent.id, &error)?;
            return Err(error);
        }
        let result = match (&sent.content, &sent.base_etag) {
            (Some(ics), Some(etag)) => dav.update(&resource.href, etag, ics),
            (Some(ics), None) => dav.create(&resource.href, ics),
            (None, Some(etag)) => dav.delete(&resource.href, etag),
            (None, None) => return Err("删除缺少强 ETag，已阻止写入".into()),
        };
        match result {
            Ok(response) => {
                let confirmed = if sent.content.is_none() {
                    Ok(Remote {
                        content: None,
                        etag: None,
                    })
                } else {
                    read_remote(dav, &resource).and_then(|remote| {
                        if remote.content.is_none() {
                            return Err("PUT 后资源不存在，需继续对账".into());
                        }
                        if let Some(etag) = &response.etag {
                            dav::require_strong_etag(etag).map_err(text)?;
                            if remote.etag.as_ref() != Some(etag) {
                                return Err(
                                    "PUT 后服务端版本再次变化，保留 Unknown 等待对账".into()
                                );
                            }
                        } else if remote.content != sent.content {
                            return Err(
                                "PUT 未返回 ETag 且 GET 内容与提交内容不同，保留 Unknown 等待对账"
                                    .into(),
                            );
                        }
                        Ok(remote)
                    })
                };
                match confirmed {
                    Ok(remote) => return commit_ack(cfg, binding, &sent, remote),
                    Err(error) => {
                        record_pending_error(cfg, binding, &sent.id, &error)?;
                        return Err(error);
                    }
                }
            }
            Err(error) if error.status == Some(412) => {
                let remote = match read_remote(dav, &resource) {
                    Ok(remote) => remote,
                    Err(error) => {
                        record_pending_error(cfg, binding, &sent.id, &error)?;
                        return Err(error);
                    }
                };
                // DELETE 的已不存在及恰好相同内容可直接确认，不凭 LAST-MODIFIED 猜胜者。
                if remote.content == sent.content {
                    return commit_ack(cfg, binding, &sent, remote);
                }
                let policy = transaction(|state| {
                    let policy = check_binding(state, cfg, binding)?.conflict_policy;
                    archive_conflict(state, cfg, id, remote.content, remote.etag)?;
                    super::projection::refresh(state)?;
                    Ok(policy)
                })?;
                if policy != ConflictPolicy::Client {
                    return Ok(());
                }
                if attempt + 1 == CONFLICT_ATTEMPTS {
                    let error = "服务端持续变化，已达到本轮 412 条件重试上限";
                    let state = read_state()?;
                    if let Some(change) = state.pending.iter().find(|p| p.resource_id == id) {
                        record_pending_error(cfg, binding, &change.id, error)?;
                    }
                    return Err(error.into());
                }
            }
            Err(error) => {
                let error = describe_dav_error(&error);
                record_pending_error(cfg, binding, &sent.id, &error)?;
                return Err(error);
            }
        }
    }
    Ok(())
}
fn describe_dav_error(error: &DavError) -> String {
    match error.status {
        Some(status) => format!("{error}（HTTP {status}）；保留队列供下次对账"),
        None => format!("{error}；结果未知，保留固定 href 待 GET 对账"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::Provider;
    fn fixture() -> (State, Config, Binding) {
        let cfg = Config {
            provider: Provider::Custom,
            account_id: "account".into(),
            username: "user".into(),
            server: Some("https://example.test/".into()),
            event_collection: Some("https://example.test/calendar/".into()),
            todo_collection: Some("https://example.test/tasks/".into()),
            two_way: true,
            ..Config::default()
        };
        let binding = Binding {
            account_id: cfg.account_id.clone(),
            generation: 7,
            event_collection: cfg.event_collection.clone().unwrap(),
            todo_collection: cfg.todo_collection.clone().unwrap(),
            switching: false,
        };
        let mut state = State {
            accounts: vec![cfg.clone()],
            binding: Some(binding.clone()),
            ..State::default()
        };
        state.collections.insert(
            binding.todo_collection.clone(),
            CollectionState {
                url: binding.todo_collection.clone(),
                todos: true,
                writable: true,
                full_scan: true,
                ..CollectionState::default()
            },
        );
        state.resources.insert(
            "resource".into(),
            Resource {
                id: "resource".into(),
                account_id: cfg.account_id.clone(),
                collection_url: binding.todo_collection.clone(),
                href: "https://example.test/tasks/fixed.ics".into(),
                etag: Some("\"old\"".into()),
                baseline: Some("base".into()),
                local_ics: Some("submitted".into()),
                revision: 1,
            },
        );
        state.pending.push(PendingChange {
            id: "operation".into(),
            resource_id: "resource".into(),
            revision: 1,
            base_etag: Some("\"old\"".into()),
            baseline: Some("base".into()),
            content: Some("submitted".into()),
            state: SendState::Unknown,
            attempts: 1,
            error: None,
        });
        (state, cfg, binding)
    }
    struct TestRepository(std::path::PathBuf);
    impl TestRepository {
        fn new(cfg: &Config) -> Self {
            let root = std::env::temp_dir().join(format!("daozai-sync-{}", organizer::new_id()));
            std::fs::create_dir_all(&root).unwrap();
            TEST_ROOT.with(|slot| *slot.borrow_mut() = Some(root.clone()));
            transaction(|state| {
                state.accounts = vec![cfg.clone()];
                state
                    .todos
                    .push(crate::model::TodoItem::new("初始化前本机待办"));
                Ok(())
            })
            .unwrap();
            Self(root)
        }
    }
    impl Drop for TestRepository {
        fn drop(&mut self) {
            TEST_ROOT.with(|slot| *slot.borrow_mut() = None);
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[derive(Clone, Debug)]
    struct Request {
        method: String,
        path: String,
        headers: BTreeMap<String, String>,
        body: String,
    }
    #[derive(Default)]
    struct FakeState {
        resources: BTreeMap<String, (String, u64)>,
        requests: Vec<Request>,
        fail_tasks_listing: bool,
        supports_sync: bool,
        fail_multiget: bool,
        deleted: Vec<String>,
        sync_version: u64,
        drop_next_put_reply: bool,
        always_precondition_failed: bool,
        after_put: Option<Box<dyn FnOnce() + Send>>,
    }
    struct FakeDav {
        base: String,
        data: std::sync::Arc<std::sync::Mutex<FakeState>>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }
    fn xml(value: &str) -> String {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }
    fn multistatus(entries: String) -> String {
        format!(
            r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">{entries}</d:multistatus>"#
        )
    }
    fn properties(path: &str, props: &str) -> String {
        format!("<d:response><d:href>{}</d:href><d:propstat><d:prop>{props}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>", xml(path))
    }
    fn request(stream: &mut std::net::TcpStream) -> Request {
        use std::io::Read;
        let mut bytes = Vec::new();
        let header_end = loop {
            let mut byte = [0u8; 1];
            stream.read_exact(&mut byte).unwrap();
            bytes.push(byte[0]);
            assert!(bytes.len() < 128 * 1024);
            if bytes.ends_with(b"\r\n\r\n") {
                break bytes.len();
            }
        };
        let header = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
        let mut lines = header.split("\r\n");
        let first = lines.next().unwrap().split_whitespace().collect::<Vec<_>>();
        let headers: BTreeMap<_, _> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().into()))
            .collect();
        let size: usize = headers
            .get("content-length")
            .map(|value: &String| value.parse().unwrap())
            .unwrap_or(0);
        assert!(size < 1024 * 1024);
        let mut body = vec![0u8; size];
        stream.read_exact(&mut body).unwrap();
        Request {
            method: first[0].into(),
            path: first[1].into(),
            headers,
            body: String::from_utf8(body).unwrap(),
        }
    }
    fn fake_response(data: &mut FakeState, req: &Request) -> Option<(u16, Option<String>, String)> {
        let empty = |code| Some((code, None, String::new()));
        match req.method.as_str() {
            "PROPFIND" if req.headers.get("depth").is_some_and(|v| v == "0") => {
                let props = r#"<d:resourcetype><d:collection/><c:calendar/></d:resourcetype><c:supported-calendar-component-set><c:comp name="VEVENT"/><c:comp name="VTODO"/></c:supported-calendar-component-set><d:current-user-privilege-set><d:privilege><d:all/></d:privilege></d:current-user-privilege-set>"#;
                let sync = if data.supports_sync {
                    "<d:supported-report-set><d:supported-report><d:report><d:sync-collection/></d:report></d:supported-report></d:supported-report-set>"
                } else {
                    ""
                };
                Some((
                    207,
                    None,
                    multistatus(properties(&req.path, &format!("{props}{sync}"))),
                ))
            }
            "PROPFIND" => {
                if req.path == "/tasks/" && data.fail_tasks_listing {
                    return empty(503);
                }
                let mut entries = properties(
                    &req.path,
                    "<d:resourcetype><d:collection/></d:resourcetype>",
                );
                for (path, (_, version)) in data
                    .resources
                    .iter()
                    .filter(|(path, _)| path.starts_with(&req.path))
                {
                    entries.push_str(&properties(
                        path,
                        &format!("<d:resourcetype/><d:getetag>&quot;{version}&quot;</d:getetag>"),
                    ));
                }
                Some((207, None, multistatus(entries)))
            }
            "REPORT" => {
                let doc = roxmltree::Document::parse(&req.body).unwrap();
                let mut entries = String::new();
                if doc.root_element().tag_name().name() == "sync-collection" {
                    data.sync_version += 1;
                    entries.push_str(&format!(
                        "<d:sync-token>token-{}</d:sync-token>",
                        data.sync_version
                    ));
                    for (path, (_, version)) in data
                        .resources
                        .iter()
                        .filter(|(path, _)| path.starts_with(&req.path))
                    {
                        entries.push_str(&properties(
                            path,
                            &format!("<d:getetag>&quot;{version}&quot;</d:getetag>"),
                        ));
                    }
                    for path in data
                        .deleted
                        .iter()
                        .filter(|path| path.starts_with(&req.path))
                    {
                        entries.push_str(&format!("<d:response><d:href>{}</d:href><d:status>HTTP/1.1 404 Gone</d:status></d:response>", xml(path)));
                    }
                    return Some((207, None, multistatus(entries)));
                }
                if data.fail_multiget {
                    return empty(503);
                }
                for node in doc
                    .descendants()
                    .filter(|n| n.is_element() && n.tag_name().name() == "href")
                {
                    let href = node.text().unwrap();
                    let url = url::Url::parse(href).unwrap();
                    if let Some((ics, version)) = data.resources.get(url.path()) {
                        entries.push_str(&properties(url.path(), &format!("<d:getetag>&quot;{version}&quot;</d:getetag><c:calendar-data>{}</c:calendar-data>", xml(ics))));
                    } else {
                        return empty(404);
                    }
                }
                Some((207, None, multistatus(entries)))
            }
            "GET" => match data.resources.get(&req.path) {
                Some((ics, version)) => Some((200, Some(format!("\"{version}\"")), ics.clone())),
                None => empty(404),
            },
            "PUT" => {
                if data.always_precondition_failed {
                    if let Some((_, version)) = data.resources.get_mut(&req.path) {
                        *version += 1;
                    }
                    return empty(412);
                }
                let existing = data.resources.get(&req.path);
                let conditional = match existing {
                    Some((_, version)) => {
                        req.headers.get("if-match") == Some(&format!("\"{version}\""))
                    }
                    None => req
                        .headers
                        .get("if-none-match")
                        .is_some_and(|value| value == "*"),
                };
                if !conditional {
                    return empty(412);
                }
                let version = existing.map_or(1, |(_, version)| version + 1);
                data.resources
                    .insert(req.path.clone(), (req.body.clone(), version));
                if let Some(hook) = data.after_put.take() {
                    hook();
                }
                if std::mem::take(&mut data.drop_next_put_reply) {
                    return None;
                }
                Some((204, Some(format!("\"{version}\"")), String::new()))
            }
            "DELETE" => {
                let Some((_, version)) = data.resources.get(&req.path) else {
                    return empty(404);
                };
                if req.headers.get("if-match") != Some(&format!("\"{version}\"")) {
                    return empty(412);
                }
                data.resources.remove(&req.path);
                empty(204)
            }
            _ => empty(405),
        }
    }
    impl FakeDav {
        fn new() -> Self {
            use std::io::Write;
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/", listener.local_addr().unwrap());
            let data = std::sync::Arc::new(std::sync::Mutex::new(FakeState::default()));
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let worker_data = data.clone();
            let worker_stop = stop.clone();
            let worker = std::thread::spawn(move || {
                for connection in listener.incoming() {
                    if worker_stop.load(std::sync::atomic::Ordering::Acquire) {
                        break;
                    }
                    let mut stream = connection.unwrap();
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                        .unwrap();
                    let req = request(&mut stream);
                    let response = {
                        let mut data = worker_data.lock().unwrap();
                        data.requests.push(req.clone());
                        fake_response(&mut data, &req)
                    };
                    if let Some((status, etag, body)) = response {
                        let etag = etag.map(|v| format!("ETag: {v}\r\n")).unwrap_or_default();
                        let header = format!("HTTP/1.1 {status} Fake\r\nContent-Length: {}\r\nContent-Type: application/xml; charset=utf-8\r\n{etag}Connection: close\r\n\r\n", body.len());
                        stream.write_all(header.as_bytes()).unwrap();
                        stream.write_all(body.as_bytes()).unwrap();
                    }
                }
            });
            Self {
                base,
                data,
                stop,
                worker: Some(worker),
            }
        }
        fn config(&self) -> Config {
            Config {
                provider: Provider::Custom,
                username: "fake-user".into(),
                account_id: "fake-account".into(),
                server: Some(self.base.clone()),
                two_way: false,
                event_collection: Some(format!("{}events/", self.base)),
                todo_collection: Some(format!("{}tasks/", self.base)),
                ..Config::default()
            }
        }
        fn task(title: &str) -> String {
            CalendarDocument::new_task(chrono::Utc::now())
                .unwrap()
                .to_ics()
                .replace(
                    "STATUS:NEEDS-ACTION",
                    &format!("SUMMARY:{title}\r\nSTATUS:NEEDS-ACTION"),
                )
        }
        fn initialize(&self) -> (TestRepository, Config, String) {
            let cfg = self.config();
            let repo = TestRepository::new(&cfg);
            let ics = Self::task("服务端任务");
            self.data
                .lock()
                .unwrap()
                .resources
                .insert("/tasks/remote.ics".into(), (ics, 1));
            initialize(
                &cfg,
                cfg.event_collection.as_ref().unwrap(),
                cfg.todo_collection.as_ref().unwrap(),
                "fake-password",
                read_state().unwrap().generation,
            )
            .unwrap();
            let state = read_state().unwrap();
            let id = state.resources.keys().next().unwrap().clone();
            (repo, state.accounts[0].clone(), id)
        }
    }
    impl Drop for FakeDav {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Release);
            if let Ok(url) = url::Url::parse(&self.base) {
                let _ = std::net::TcpStream::connect(("127.0.0.1", url.port().unwrap()));
            }
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
        }
    }
    #[test]
    fn fake_dav_initialization_is_read_only_and_failure_preserves_snapshot() {
        let server = FakeDav::new();
        let cfg = server.config();
        let _repo = TestRepository::new(&cfg);
        server.data.lock().unwrap().fail_tasks_listing = true;
        let before = read_state().unwrap();
        assert!(initialize(
            &cfg,
            cfg.event_collection.as_ref().unwrap(),
            cfg.todo_collection.as_ref().unwrap(),
            "fake-password",
            before.generation
        )
        .is_err());
        let after = read_state().unwrap();
        assert_eq!(after.generation, before.generation);
        assert_eq!(after.todos[0].text, "初始化前本机待办");
        assert!(after.binding.is_none());
        assert!(!after.accounts[0].two_way);
        {
            let mut data = server.data.lock().unwrap();
            data.fail_tasks_listing = false;
            data.resources.insert(
                "/tasks/broken.ics".into(),
                ("BEGIN:VCALENDAR\r\nBROKEN\r\nEND:VCALENDAR\r\n".into(), 1),
            );
        }
        assert!(initialize(
            &cfg,
            cfg.event_collection.as_ref().unwrap(),
            cfg.todo_collection.as_ref().unwrap(),
            "fake-password",
            before.generation
        )
        .is_err());
        assert_eq!(read_state().unwrap().generation, before.generation);
        assert_eq!(read_state().unwrap().todos[0].text, "初始化前本机待办");
        assert!(server
            .data
            .lock()
            .unwrap()
            .requests
            .iter()
            .all(|r| r.method != "PUT" && r.method != "DELETE"));
    }
    #[test]
    fn fake_dav_successful_init_reopen_and_conditional_roundtrip() {
        let server = FakeDav::new();
        let (_repo, cfg, id) = server.initialize();
        let initial = read_state().unwrap();
        assert_eq!(initial.todos.len(), 1);
        assert!(initial
            .todos
            .iter()
            .all(|todo| todo.text != "初始化前本机待办"));
        assert!(initial.pending.is_empty());
        assert!(initial.conflicts.is_empty());
        assert!(server
            .data
            .lock()
            .unwrap()
            .requests
            .iter()
            .all(|r| r.method != "PUT" && r.method != "DELETE"));
        let local = FakeDav::task("本机修改");
        transaction(|state| organizer::stage(state, &id, Some(local.clone()))).unwrap();
        set_enabled(&cfg.account_id, false).unwrap();
        let state = read_state().unwrap();
        initialize(
            &state.accounts[0],
            cfg.event_collection.as_ref().unwrap(),
            cfg.todo_collection.as_ref().unwrap(),
            "fake-password",
            state.generation,
        )
        .unwrap();
        assert_eq!(read_state().unwrap().pending.len(), 1); // 重开不再次清队列。
        sync_account(&cfg, "fake-password").unwrap();
        let state = read_state().unwrap();
        assert!(state.pending.is_empty());
        assert_eq!(state.resources[&id].baseline.as_ref(), Some(&local));
        transaction(|state| organizer::stage(state, &id, None)).unwrap();
        sync_account(&cfg, "fake-password").unwrap();
        assert!(read_state().unwrap().pending.is_empty());
        let data = server.data.lock().unwrap();
        assert!(!data.resources.contains_key("/tasks/remote.ics"));
        let writes: Vec<_> = data
            .requests
            .iter()
            .filter(|r| r.method == "PUT" || r.method == "DELETE")
            .collect();
        assert_eq!(writes.len(), 2);
        assert!(writes.iter().all(|r| r.headers.contains_key("if-match")));
    }
    #[test]
    fn fake_dav_partial_download_does_not_advance_token_or_apply_deletion() {
        let server = FakeDav::new();
        let (_repo, cfg, id) = server.initialize();
        {
            let mut data = server.data.lock().unwrap();
            data.supports_sync = true;
            data.resources.insert(
                "/tasks/deleted-later.ics".into(),
                (FakeDav::task("稍后删除"), 1),
            );
        }
        sync_account(&cfg, "fake-password").unwrap();
        let before = read_state().unwrap();
        let url = cfg.todo_collection.as_ref().unwrap();
        let token = before.collections[url].sync_token.clone();
        assert!(token.is_some());
        let removed_id = before
            .resources
            .values()
            .find(|r| r.href.ends_with("deleted-later.ics"))
            .unwrap()
            .id
            .clone();
        {
            let mut data = server.data.lock().unwrap();
            data.resources.get_mut("/tasks/remote.ics").unwrap().1 += 1;
            data.resources.remove("/tasks/deleted-later.ics");
            data.deleted.push("/tasks/deleted-later.ics".into());
            data.fail_multiget = true;
        }
        assert!(sync_account(&cfg, "fake-password").is_err());
        let after = read_state().unwrap();
        assert_eq!(after.collections[url].sync_token, token);
        assert!(after.collections[url].error.is_some());
        assert!(after.resources[&removed_id].local_ics.is_some());
        assert_eq!(after.resources[&id].etag, before.resources[&id].etag);
        server.data.lock().unwrap().fail_multiget = false;
        sync_account(&cfg, "fake-password").unwrap();
        let final_state = read_state().unwrap();
        assert_ne!(final_state.collections[url].sync_token, token);
        assert!(final_state.resources[&removed_id].local_ics.is_none());
        assert!(final_state.collections[url].error.is_none());
    }
    #[test]
    fn fake_dav_lost_put_reply_is_reconciled_without_repeating_create() {
        let server = FakeDav::new();
        let (_repo, cfg, _) = server.initialize();
        let content = FakeDav::task("离线新任务");
        let id = transaction(|state| {
            organizer::create_resource(
                state,
                cfg.todo_collection.as_ref().unwrap(),
                content.clone(),
            )
        })
        .unwrap();
        server.data.lock().unwrap().drop_next_put_reply = true;
        assert!(sync_account(&cfg, "fake-password").is_err());
        assert!(read_state()
            .unwrap()
            .pending
            .iter()
            .any(|p| p.resource_id == id && p.state == SendState::Unknown));
        sync_account(&cfg, "fake-password").unwrap();
        assert!(read_state().unwrap().pending.is_empty());
        assert_eq!(
            read_state().unwrap().resources[&id].baseline.as_ref(),
            Some(&content)
        );
        let data = server.data.lock().unwrap();
        let puts: Vec<_> = data.requests.iter().filter(|r| r.method == "PUT").collect();
        assert_eq!(puts.len(), 1);
        assert_eq!(
            puts[0].headers.get("if-none-match").map(String::as_str),
            Some("*")
        );
    }
    #[test]
    fn fake_dav_edit_during_put_retains_new_revision_and_uses_new_etag() {
        let server = FakeDav::new();
        let (repo, cfg, id) = server.initialize();
        let sent = FakeDav::task("正在发送");
        let next = FakeDav::task("发送期间再次编辑");
        transaction(|state| organizer::stage(state, &id, Some(sent.clone()))).unwrap();
        let root = repo.0.clone();
        let hook_id = id.clone();
        let next_for_hook = next.clone();
        server.data.lock().unwrap().after_put = Some(Box::new(move || {
            organizer::store::transaction_at(&root, |state| {
                organizer::stage(state, &hook_id, Some(next_for_hook))
            })
            .unwrap();
        }));
        sync_account(&cfg, "fake-password").unwrap();
        let state = read_state().unwrap();
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending[0].revision, 2);
        assert_eq!(state.resources[&id].local_ics.as_ref(), Some(&next));
        assert_eq!(state.resources[&id].baseline.as_ref(), Some(&sent));
        assert_eq!(state.pending[0].base_etag.as_deref(), Some("\"2\""));
        sync_account(&cfg, "fake-password").unwrap();
        assert!(read_state().unwrap().pending.is_empty());
        let data = server.data.lock().unwrap();
        assert_eq!(data.resources["/tasks/remote.ics"].0, next);
        let conditions: Vec<_> = data
            .requests
            .iter()
            .filter(|r| r.method == "PUT")
            .map(|r| r.headers["if-match"].as_str())
            .collect();
        assert_eq!(conditions, vec!["\"1\"", "\"2\""]);
    }
    #[test]
    fn fake_dav_client_policy_bounds_412_and_preserves_archives() {
        let server = FakeDav::new();
        let (_repo, mut cfg, id) = server.initialize();
        cfg.conflict_policy = ConflictPolicy::Client;
        transaction(|state| {
            state.accounts[0].conflict_policy = ConflictPolicy::Client;
            organizer::stage(state, &id, Some(FakeDav::task("冲突本机版本")))
        })
        .unwrap();
        server.data.lock().unwrap().always_precondition_failed = true;
        assert!(sync_account(&cfg, "fake-password").is_err());
        let state = read_state().unwrap();
        assert_eq!(state.conflicts.len(), CONFLICT_ATTEMPTS);
        assert!(state.conflicts.iter().all(|c| c.resolved));
        assert_eq!(state.pending.len(), 1);
        assert!(state.pending[0].error.is_some());
        assert_eq!(
            server
                .data
                .lock()
                .unwrap()
                .requests
                .iter()
                .filter(|r| r.method == "PUT")
                .count(),
            CONFLICT_ATTEMPTS
        );
    }
    #[test]
    fn disabled_server_policy_never_replaces_pending_local() {
        let (mut state, cfg, _) = fixture();
        state.accounts[0].two_way = false;
        state.accounts[0].conflict_policy = ConflictPolicy::Server;
        state.pending[0].state = SendState::Ready;
        accept_remote(
            &mut state,
            &cfg,
            "resource",
            Some("remote".into()),
            Some("\"remote\"".into()),
        )
        .unwrap();
        assert_eq!(
            state.resources["resource"].local_ics.as_deref(),
            Some("submitted")
        );
        assert_eq!(state.pending.len(), 1);
        assert!(!state.conflicts[0].resolved);
    }
    fn writable_fixture() -> (State, Config, Binding) {
        let (mut state, cfg, binding) = fixture();
        let baseline = FakeDav::task("base");
        let local = baseline.replace("SUMMARY:base", "SUMMARY:submitted");
        let resource = state.resources.get_mut("resource").unwrap();
        resource.baseline = Some(baseline.clone());
        resource.local_ics = Some(local.clone());
        state.pending[0].baseline = Some(baseline);
        state.pending[0].content = Some(local);
        (state, cfg, binding)
    }
    #[test]
    fn reply_confirms_only_sent_revision_and_advances_successor_etag() {
        let (mut state, _, _) = writable_fixture();
        let sent = state.pending[0].clone();
        let next = FakeDav::task("new edit");
        organizer::stage(&mut state, "resource", Some(next.clone())).unwrap();
        acknowledge(
            &mut state,
            &sent,
            Remote {
                content: Some("normalized submitted".into()),
                etag: Some("\"new\"".into()),
            },
        )
        .unwrap();
        assert_eq!(
            state.resources["resource"].local_ics.as_deref(),
            Some(next.as_str())
        );
        assert_eq!(
            state.resources["resource"].baseline.as_deref(),
            Some("normalized submitted")
        );
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending[0].revision, 2);
        assert_eq!(state.pending[0].base_etag.as_deref(), Some("\"new\""));
        assert_eq!(state.pending[0].content.as_ref(), Some(&next));
    }
    #[test]
    fn edit_during_delete_becomes_conditional_create() {
        let (mut state, _, _) = writable_fixture();
        state.pending[0].content = None;
        state.resources.get_mut("resource").unwrap().local_ics = None;
        let sent = state.pending[0].clone();
        let restored = FakeDav::task("restored");
        organizer::stage(&mut state, "resource", Some(restored.clone())).unwrap();
        acknowledge(
            &mut state,
            &sent,
            Remote {
                content: None,
                etag: None,
            },
        )
        .unwrap();
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending[0].content.as_ref(), Some(&restored));
        assert!(state.pending[0].baseline.is_none());
        assert!(state.pending[0].base_etag.is_none());
    }
    #[test]
    fn local_new_then_deleted_is_cancelled_without_delete() {
        let (mut state, cfg, binding) = fixture();
        let resource = state.resources.get_mut("resource").unwrap();
        resource.local_ics = None;
        resource.baseline = None;
        resource.etag = None;
        let change = &mut state.pending[0];
        change.content = None;
        change.baseline = None;
        change.base_etag = None;
        change.state = SendState::Ready;
        assert!(prepare_send(&mut state, &cfg, &binding, "resource")
            .unwrap()
            .is_none());
        assert!(state.pending.is_empty());
        assert!(!state.resources.contains_key("resource"));
    }
    #[test]
    fn closing_switching_wrong_generation_and_other_provider_prevent_writes() {
        let (mut state, cfg, binding) = fixture();
        let resource = state.resources["resource"].clone();
        assert!(check_write(&state, &cfg, &binding, &resource).is_ok());
        state.accounts[0].two_way = false;
        assert!(check_write(&state, &cfg, &binding, &resource).is_err());
        // 已发结果在关闭期间仍可确认。
        assert!(check_binding(&state, &cfg, &binding).is_ok());
        state.accounts[0].two_way = true;
        state.binding.as_mut().unwrap().switching = true;
        assert!(check_write(&state, &cfg, &binding, &resource).is_err());
        state.binding.as_mut().unwrap().switching = false;
        state.binding.as_mut().unwrap().generation += 1;
        assert!(check_binding(&state, &cfg, &binding).is_err());
        state.binding = Some(binding.clone());
        state.accounts[0].provider = Provider::Exchange;
        assert!(check_write(&state, &cfg, &binding, &resource).is_err());
    }
    #[test]
    fn disabled_download_preserves_pending_local_and_archives_three_versions() {
        let (mut state, mut cfg, _) = fixture();
        cfg.two_way = false;
        state.accounts[0].two_way = false;
        state.pending[0].state = SendState::Ready;
        accept_remote(
            &mut state,
            &cfg,
            "resource",
            Some("server edit".into()),
            Some("\"server\"".into()),
        )
        .unwrap();
        assert_eq!(
            state.resources["resource"].local_ics.as_deref(),
            Some("submitted")
        );
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.conflicts.len(), 1);
        let conflict = &state.conflicts[0];
        assert_eq!(conflict.baseline.as_deref(), Some("base"));
        assert_eq!(conflict.local.as_deref(), Some("submitted"));
        assert_eq!(conflict.remote.as_deref(), Some("server edit"));
        assert!(!conflict.resolved);
    }
    #[test]
    fn client_conflict_uses_latest_remote_etag_and_keeps_archive() {
        let (mut state, cfg, _) = fixture();
        state.accounts[0].conflict_policy = ConflictPolicy::Client;
        archive_conflict(
            &mut state,
            &cfg,
            "resource",
            Some("server".into()),
            Some("\"latest\"".into()),
        )
        .unwrap();
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending[0].state, SendState::Ready);
        assert_eq!(state.pending[0].base_etag.as_deref(), Some("\"latest\""));
        assert_eq!(state.pending[0].baseline.as_deref(), Some("server"));
        assert_eq!(state.pending[0].content.as_deref(), Some("submitted"));
        assert!(state.conflicts[0].resolved);
        assert_eq!(state.conflicts[0].remote.as_deref(), Some("server"));
    }
    #[test]
    fn server_conflict_keeps_local_archive_and_removes_queue() {
        let (mut state, cfg, _) = fixture();
        state.accounts[0].conflict_policy = ConflictPolicy::Server;
        archive_conflict(&mut state, &cfg, "resource", None, None).unwrap();
        assert!(state.pending.is_empty());
        assert!(state.resources["resource"].local_ics.is_none());
        assert_eq!(state.conflicts[0].local.as_deref(), Some("submitted"));
    }
    #[test]
    fn partial_listing_never_implies_deletion_and_unknown_is_not_overwritten() {
        let (mut state, cfg, binding) = fixture();
        state.pending.clear();
        apply_download(&mut state, &cfg, &binding.todo_collection, &[], false, &[]).unwrap();
        assert!(state.resources["resource"].baseline.is_some());
        apply_download(&mut state, &cfg, &binding.todo_collection, &[], true, &[]).unwrap();
        assert!(state.resources["resource"].baseline.is_none());
        let (mut state, cfg, _) = fixture();
        accept_remote(
            &mut state,
            &cfg,
            "resource",
            Some("server edit".into()),
            Some("\"server\"".into()),
        )
        .unwrap();
        assert_eq!(
            state.resources["resource"].local_ics.as_deref(),
            Some("submitted")
        );
        assert!(state.conflicts.is_empty());
    }
    #[test]
    fn multiple_uid_resources_are_retained_but_never_sent_as_one_object() {
        let first = FakeDav::task("第一组");
        let second = FakeDav::task("第二组");
        let start = second.find("BEGIN:VTODO").unwrap();
        let end = second.find("END:VCALENDAR").unwrap();
        let multiple = first.replace(
            "END:VCALENDAR",
            &format!("{}END:VCALENDAR", &second[start..end]),
        );
        assert!(validate_ics(&multiple, false).is_ok());
        assert!(validate_ics(&multiple, true).is_err());
        let (mut state, _, _) = fixture();
        let resource = state.resources["resource"].clone();
        assert!(validate_component_target(&state, &resource, &first).is_ok());
        state
            .collections
            .get_mut(&resource.collection_url)
            .unwrap()
            .todos = false;
        assert!(validate_component_target(&state, &resource, &first).is_err());
    }
    #[test]
    fn wrong_collection_and_weak_etag_are_rejected_before_sending() {
        let (mut state, cfg, binding) = fixture();
        let resource = state.resources.get_mut("resource").unwrap();
        resource.href = "https://example.test/calendar/stolen.ics".into();
        assert!(check_write(&state, &cfg, &binding, &state.resources["resource"]).is_err());
        state.resources.get_mut("resource").unwrap().href =
            "https://example.test/tasks/fixed.ics".into();
        state.pending[0].state = SendState::Ready;
        state.pending[0].base_etag = Some("W/\"weak\"".into());
        assert!(prepare_send(&mut state, &cfg, &binding, "resource").is_err());
        assert_eq!(state.pending[0].state, SendState::Ready);
    }
}
