//! 日程、待办、账号和同步队列的唯一事务仓库。
pub mod model;
pub mod store;
pub use model::*;

pub fn new_id() -> String {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut bytes = [0u8; 16];
    if SystemRandom::new().fill(&mut bytes).is_ok() {
        return bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    }
    // 系统熵异常时仍保证本进程内身份不重用。
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{:x}-{:x}-{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

pub fn read_state() -> Result<State, String> {
    store::read_at(&crate::model::config_dir())
}
pub fn transaction<R>(change: impl FnOnce(&mut State) -> Result<R, String>) -> Result<R, String> {
    store::transaction_at(&crate::model::config_dir(), change)
}

pub fn save_todos(
    before: Option<&[crate::model::TodoItem]>,
    items: &[crate::model::TodoItem],
) -> Result<Vec<crate::model::TodoItem>, String> {
    transaction(|state| crate::calendar::caldav::projection::save_todos(state, before, items))
}
pub fn save_events(entries: &[crate::local_calendar::Entry]) -> Result<(), String> {
    transaction(|state| crate::calendar::caldav::projection::save_events(state, entries))
}

pub fn writable(state: &State) -> bool {
    state.binding.as_ref().is_some_and(|binding| {
        !binding.switching
            && state.accounts.iter().any(|account| {
                account.account_id == binding.account_id
                    && account.two_way
                    && account.provider.two_way_capable()
            })
    })
}

/// 同一资源可有后续 revision；已发出的版本留在队列中等待确认。
pub fn stage(state: &mut State, id: &str, content: Option<String>) -> Result<(), String> {
    if !writable(state) {
        return Err("双向同步已关闭或正在切换，此同步对象暂为只读".into());
    }
    if state
        .conflicts
        .iter()
        .any(|conflict| conflict.resource_id == id && !conflict.resolved)
    {
        return Err("此资源存在冲突，请先选择保留本机版本或服务端版本".into());
    }
    let resource = state
        .resources
        .get_mut(id)
        .ok_or("同步资源已变化，请重新打开编辑器")?;
    for raw in [
        resource.local_ics.as_deref(),
        resource.baseline.as_deref(),
        content.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        crate::calendar::caldav::sync::validate_resource_for_write(raw)?;
    }
    resource.revision = resource
        .revision
        .checked_add(1)
        .ok_or("本机 revision 已耗尽")?;
    resource.local_ics = content.clone();
    state
        .pending
        .retain(|change| change.resource_id != id || change.state == SendState::Unknown);
    state.pending.push(PendingChange {
        id: new_id(),
        resource_id: id.into(),
        revision: resource.revision,
        base_etag: resource.etag.clone(),
        baseline: resource.baseline.clone(),
        content,
        state: SendState::Ready,
        attempts: 0,
        error: None,
    });
    Ok(())
}

pub fn create_resource(state: &mut State, collection: &str, ics: String) -> Result<String, String> {
    let binding = state.binding.as_ref().ok_or("尚未初始化同步")?;
    let id = new_id();
    let collection_url = url::Url::parse(collection).map_err(|_| "集合地址无效")?;
    let href = collection_url
        .join(&format!("{id}.ics"))
        .map_err(|_| "无法创建资源地址")?
        .to_string();
    state.resources.insert(
        id.clone(),
        Resource {
            id: id.clone(),
            account_id: binding.account_id.clone(),
            collection_url: collection.into(),
            href,
            etag: None,
            local_ics: None,
            baseline: None,
            revision: 0,
        },
    );
    stage(state, &id, Some(ics))?;
    Ok(id)
}
