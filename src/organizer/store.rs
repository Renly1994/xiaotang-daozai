//! 不可变 UTF-8 JSON 快照与 CURRENT 单提交点。
use super::model::{State, SCHEMA};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

static WRITER: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize)]
struct Pointer {
    schema: u32,
    generation: u64,
    file: String,
    sha256: String,
}

fn checksum(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn io_error(action: &str, e: impl std::fmt::Display) -> String {
    format!("{action}：{e}；同步已停止，请检查 organizer 数据目录")
}

/// 文件不存在才表示没有旧数据；权限错误、损坏和非 UTF-8 都必须中止迁移。
fn optional_read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error("读取旧数据失败", e)),
    }
}

fn migrate(root: &Path) -> Result<State, String> {
    let mut state = State::default();
    if let Some(raw) = optional_read(&root.join("calendar.json"))? {
        (state.accounts, state.active_provider) = crate::calendar::decode_configs_strict(&raw)?;
        // 无凭据的纯迁移不能先变更 keyring 引用，旧账号在重新保存连接时稳定化。
        for account in &mut state.accounts {
            account.two_way = false;
        }
    }
    if let Some(raw) = optional_read(&root.join("local-calendar.json"))? {
        state.local_events = crate::local_calendar::decode(&raw)?;
    }
    if let Some(raw) = optional_read(&root.join("todo.txt"))? {
        let text = std::str::from_utf8(&raw).map_err(|e| io_error("待办文件不是 UTF-8", e))?;
        state.todos = crate::model::TodoList::parse_strict(text)?.items;
    }
    state.migrated = true;
    Ok(state)
}

fn lock_directory(root: &Path) -> Result<fs::File, String> {
    fs::create_dir_all(root).map_err(|e| io_error("创建事务目录失败", e))?;
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    options
        .open(root.join("LOCK"))
        .map_err(|e| io_error("事务仓库正被其他进程占用", e))
}

fn load(root: &Path) -> Result<State, String> {
    let dir = root.join("organizer");
    let raw = match fs::read(dir.join("CURRENT")) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let leftovers = fs::read_dir(&dir)
                .map_err(|e| io_error("检查迁移状态失败", e))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| io_error("检查迁移状态失败", e))?
                .iter()
                .any(|entry| entry.file_name() != "LOCK");
            if leftovers {
                return Err("CURRENT 缺失且存在快照或迁移残留；为避免重新导入已抛弃的数据，同步已停止，请手动恢复 CURRENT".into());
            }
            let mut state = migrate(root)?;
            commit(&dir, &mut state)?;
            return Ok(state);
        }
        Err(e) => return Err(io_error("读取 CURRENT 失败", e)),
    };
    let pointer: Pointer = serde_json::from_slice(&raw).map_err(|e| io_error("CURRENT 损坏", e))?;
    if pointer.schema != SCHEMA || pointer.file != format!("state-{}.json", pointer.generation) {
        return Err("CURRENT 的 schema 或快照引用无效；同步已停止，请手动恢复".into());
    }
    let bytes =
        fs::read(dir.join(&pointer.file)).map_err(|e| io_error("CURRENT 引用的快照无法读取", e))?;
    if checksum(&bytes) != pointer.sha256 {
        return Err("快照校验失败；同步已停止，请手动恢复".into());
    }
    let state: State = serde_json::from_slice(&bytes).map_err(|e| io_error("事务快照损坏", e))?;
    if state.schema != SCHEMA || state.generation != pointer.generation || !state.migrated {
        return Err("事务快照的 schema、generation 或迁移记录无效；同步已停止".into());
    }
    validate(&state)?;
    Ok(state)
}

fn validate(state: &State) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    if state
        .todos
        .iter()
        .any(|todo| todo.id.is_empty() || !ids.insert(&todo.id))
    {
        return Err("待办稳定 ID 缺失或重复；事务未提交".into());
    }
    for (id, resource) in &state.resources {
        if id != &resource.id || id.is_empty() {
            return Err("资源 ID 不一致；事务未提交".into());
        }
    }
    if state
        .pending
        .iter()
        .any(|change| !state.resources.contains_key(&change.resource_id))
    {
        return Err("上传队列引用缺失资源；事务未提交".into());
    }
    Ok(())
}

fn commit(dir: &Path, state: &mut State) -> Result<(), String> {
    let previous = state.generation;
    commit_from(dir, state, previous)
}

fn commit_from(dir: &Path, state: &mut State, previous: u64) -> Result<(), String> {
    validate(state)?;
    let (path, mut file) = loop {
        state.generation = state
            .generation
            .checked_add(1)
            .ok_or("快照 generation 已耗尽")?;
        let path = dir.join(format!("state-{}.json", state.generation));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => break (path, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue, // 未提交残留永远不被引用。
            Err(e) => return Err(io_error("创建不可变快照失败", e)),
        }
    };
    state.previous_generation = (previous != 0).then_some(previous);
    let bytes = serde_json::to_vec(state).map_err(|e| io_error("序列化事务失败", e))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| io_error("同步快照到磁盘失败", e))?;
    drop(file);
    let pointer = Pointer {
        schema: SCHEMA,
        generation: state.generation,
        file: path
            .file_name()
            .unwrap()
            .to_str()
            .ok_or("快照名称不是 UTF-8")?
            .to_owned(),
        sha256: checksum(&bytes),
    };
    let tmp = dir.join(format!("CURRENT-{}.tmp", super::new_id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| io_error("创建提交指针失败", e))?;
    let bytes = serde_json::to_vec(&pointer).map_err(|e| io_error("序列化提交指针失败", e))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| io_error("同步提交指针失败", e))?;
    drop(file);
    let result = replace_current(&tmp, &dir.join("CURRENT"));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|e| io_error("原子切换 CURRENT 失败", e))?;
    #[cfg(not(windows))]
    fs::File::open(dir)
        .and_then(|file| file.sync_all())
        .map_err(|e| io_error("同步事务目录失败", e))?;
    Ok(())
}

#[cfg(windows)]
fn replace_current(tmp: &Path, current: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        core::PCWSTR,
        Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        },
    };
    let tmp: Vec<u16> = tmp.as_os_str().encode_wide().chain(Some(0)).collect();
    let current: Vec<u16> = current.as_os_str().encode_wide().chain(Some(0)).collect();
    // 同一目录、同卷 rename；绝不启用 MOVEFILE_COPY_ALLOWED 的跨卷复制回退。
    unsafe {
        MoveFileExW(
            PCWSTR(tmp.as_ptr()),
            PCWSTR(current.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(std::io::Error::other)
}
#[cfg(not(windows))]
fn replace_current(tmp: &Path, current: &Path) -> std::io::Result<()> {
    fs::rename(tmp, current)
}

pub fn read_at(root: &Path) -> Result<State, String> {
    let _writer = WRITER
        .lock()
        .map_err(|_| "事务写入负责人异常，请重启后检查快照")?;
    let _file = lock_directory(&root.join("organizer"))?;
    load(root)
}

pub fn transaction_at<R>(
    root: &Path,
    change: impl FnOnce(&mut State) -> Result<R, String>,
) -> Result<R, String> {
    let _writer = WRITER
        .lock()
        .map_err(|_| "事务写入负责人异常，请重启后检查快照")?;
    let _file = lock_directory(&root.join("organizer"))?;
    let mut state = load(root)?;
    let result = change(&mut state)?;
    commit(&root.join("organizer"), &mut state)?;
    Ok(result)
}

/// 预览令牌绑定候选正文和当时的 CURRENT，确认期间任一变化均拒绝恢复。
#[derive(Clone, Debug)]
pub struct RecoveryPreview {
    pub file: String,
    pub sha256: String,
    pub state: State,
    current_sha256: Option<String>,
}

fn snapshot_generation(name: &str) -> Option<u64> {
    let number = name.strip_prefix("state-")?.strip_suffix(".json")?;
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let generation = number.parse::<u64>().ok()?;
    (generation != 0 && name == format!("state-{generation}.json")).then_some(generation)
}

fn snapshot_paths(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| io_error("读取恢复目录失败", e))? {
        let entry = entry.map_err(|e| io_error("读取恢复目录失败", e))?;
        let kind = entry
            .file_type()
            .map_err(|e| io_error("读取候选快照类型失败", e))?;
        if kind.is_file()
            && !kind.is_symlink()
            && entry
                .file_name()
                .to_str()
                .and_then(snapshot_generation)
                .is_some()
        {
            paths.push(entry.path());
        }
    }
    paths.sort_by_key(|path| {
        std::cmp::Reverse(
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(snapshot_generation)
                .unwrap_or(0),
        )
    });
    Ok(paths)
}

fn recovery_snapshot(dir: &Path, name: &str) -> Result<(State, String), String> {
    let generation = snapshot_generation(name)
        .ok_or("恢复只允许 organizer 目录内的 state-<正整数>.json 快照，禁止其他路径")?;
    let path = dir.join(name);
    let metadata = fs::symlink_metadata(&path).map_err(|e| io_error("读取恢复候选失败", e))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("恢复候选必须是 organizer 目录内的普通快照文件".into());
    }
    let bytes = fs::read(path).map_err(|e| io_error("读取恢复快照失败", e))?;
    let state: State =
        serde_json::from_slice(&bytes).map_err(|e| io_error("恢复候选 JSON 或 UTF-8 无效", e))?;
    if state.schema != SCHEMA || state.generation != generation || !state.migrated {
        return Err("恢复候选的 schema、generation 或 migrated 无效；CURRENT 未修改".into());
    }
    validate(&state)?;
    Ok((state, checksum(&bytes)))
}

fn recovery_current(dir: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(dir.join("CURRENT")) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error("读取待备份 CURRENT 失败", error)),
    }
}

/// 只列出历史文件，不修改 CURRENT、不自动选择或恢复任何候选。
pub fn recovery_candidates(root: &Path) -> Result<Vec<PathBuf>, String> {
    let _writer = WRITER
        .lock()
        .map_err(|_| "事务写入负责人异常，请重启后检查快照")?;
    let dir = root.join("organizer");
    let _file = lock_directory(&dir)?;
    snapshot_paths(&dir)
}

/// 只读预览。调用方显示摘要和 sha256，用户再次确认后才允许调用恢复。
pub fn recovery_preview_at(root: &Path, file: &str) -> Result<RecoveryPreview, String> {
    let _writer = WRITER
        .lock()
        .map_err(|_| "事务写入负责人异常，请重启后检查快照")?;
    let dir = root.join("organizer");
    let _file = lock_directory(&dir)?;
    let (state, sha256) = recovery_snapshot(&dir, file)?;
    let current_sha256 = recovery_current(&dir)?.as_deref().map(checksum);
    Ok(RecoveryPreview {
        file: file.into(),
        sha256,
        state,
        current_sha256,
    })
}

/// 仅用于明确确认后的本地恢复；没有网络、凭据或隐式回退操作。
/// 原 CURRENT 单独备份，新快照始终关闭双向，全部队列标记结果未知以先行对账。
pub fn recovery_restore_at(root: &Path, preview: &RecoveryPreview) -> Result<State, String> {
    let _writer = WRITER
        .lock()
        .map_err(|_| "事务写入负责人异常，请重启后检查快照")?;
    let dir = root.join("organizer");
    let _file = lock_directory(&dir)?;
    let (mut state, sha256) = recovery_snapshot(&dir, &preview.file)?;
    if sha256 != preview.sha256 {
        return Err("恢复候选在预览后发生变化，请重新预览并确认；CURRENT 未修改".into());
    }
    let current = recovery_current(&dir)?;
    if current.as_deref().map(checksum) != preview.current_sha256 {
        return Err("CURRENT 在预览后发生变化，请重新检查并确认；CURRENT 未修改".into());
    }
    let source_generation = state.generation;
    let maximum = snapshot_paths(&dir)?
        .iter()
        .filter_map(|path| path.file_name()?.to_str().and_then(snapshot_generation))
        .max()
        .unwrap_or(source_generation);
    // 新 generation 高于目录中的全部历史和未提交残留，绝不覆盖旧快照。
    let next_generation = maximum
        .checked_add(1)
        .ok_or("快照 generation 已耗尽，无法恢复")?;
    for account in &mut state.accounts {
        account.two_way = false;
    }
    if let Some(binding) = &mut state.binding {
        binding.switching = true;
        binding.generation = binding
            .generation
            .max(maximum)
            .checked_add(1)
            .ok_or("绑定 generation 已耗尽，无法恢复")?;
    }
    for collection in state.collections.values_mut() {
        collection.sync_token = None;
        collection.full_scan = false;
    }
    for change in &mut state.pending {
        change.state = super::model::SendState::Unknown;
    }
    validate(&state)?;
    // 先持久备份原始指针；失败不得切换 CURRENT。缺失指针保存独立标记。
    let backup = dir.join(format!(
        "CURRENT-recovery-{}.{}",
        super::new_id(),
        if current.is_some() { "bak" } else { "missing" }
    ));
    let mut backup_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(backup)
        .map_err(|e| io_error("备份原 CURRENT 失败", e))?;
    let original = current
        .as_deref()
        .unwrap_or(b"CURRENT was missing before explicit recovery\n");
    backup_file
        .write_all(original)
        .and_then(|_| backup_file.sync_all())
        .map_err(|e| io_error("同步 CURRENT 备份失败", e))?;
    drop(backup_file);
    state.generation = next_generation - 1;
    commit_from(&dir, &mut state, source_generation)?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("daozai-organizer-{}", crate::organizer::new_id()));
        fs::create_dir_all(&path).unwrap();
        path
    }
    #[test]
    fn migration_is_atomic_strict_and_only_runs_once() {
        let path = root();
        fs::write(
            path.join("todo.txt"),
            "r31|2026-11-07|@due:2026-11-08 09:00|2026-11-07|准备 | 资料\nx|2026-11-01|已完成\n",
        )
        .unwrap();
        fs::write(
            path.join("calendar.json"),
            br#"{"provider":"custom","username":"Test"}"#,
        )
        .unwrap();
        let first = read_at(&path).unwrap();
        assert_eq!(first.todos.len(), 2);
        assert_eq!(first.todos[0].repeat, 31);
        assert_eq!(first.todos[0].text, "准备 | 资料");
        assert!(!first.accounts[0].two_way);
        fs::write(path.join("todo.txt"), "不得再次导入").unwrap();
        transaction_at(&path, |state| {
            state.todos.clear();
            Ok(())
        })
        .unwrap();
        assert!(read_at(&path).unwrap().todos.is_empty());
        fs::write(path.join("organizer/CURRENT"), b"broken").unwrap();
        assert!(read_at(&path).is_err());
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn failed_migration_does_not_commit_empty_data() {
        let path = root();
        fs::write(path.join("local-calendar.json"), b"not json").unwrap();
        assert!(read_at(&path).is_err());
        assert!(!path.join("organizer/CURRENT").exists());
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn malformed_todo_reminder_aborts_migration_and_can_be_repaired() {
        let path = root();
        fs::write(
            path.join("todo.txt"),
            "o|-|保留的任务\no|-|@due:2026-11-08 09:00|采购牛奶\n",
        )
        .unwrap();
        assert!(read_at(&path).unwrap_err().contains("第 2 行"));
        assert!(!path.join("organizer/CURRENT").exists());
        assert_eq!(fs::read_dir(path.join("organizer")).unwrap().count(), 1);
        fs::write(path.join("todo.txt"), "\u{feff}o|-|保留的任务\no|-|@due:2026-11-08 09:00|0|采购牛奶\nr127|-|每周 | 汇总\nx|2026-11-07|已完成\n纯文本任务\n").unwrap();
        let state = read_at(&path).unwrap();
        assert_eq!(state.todos.len(), 5);
        assert_eq!(state.todos[1].text, "采购牛奶");
        assert_eq!(state.todos[1].due.as_deref(), Some("2026-11-08 09:00"));
        assert_eq!(state.todos[2].text, "每周 | 汇总");
        assert_eq!(state.todos[2].repeat, 127);
        assert!(state.todos[3].done_on.is_some());
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn failed_transaction_and_orphan_cannot_replace_current() {
        let path = root();
        let first = read_at(&path).unwrap();
        assert!(transaction_at::<()>(&path, |state| {
            state.todos.push(crate::model::TodoItem::new("不应出现"));
            Err("注入失败".into())
        })
        .is_err());
        assert_eq!(read_at(&path).unwrap().generation, first.generation);
        fs::write(path.join("organizer/state-2.json"), b"uncommitted").unwrap();
        transaction_at(&path, |state| {
            state.todos.push(crate::model::TodoItem::new("已提交"));
            Ok(())
        })
        .unwrap();
        let latest = read_at(&path).unwrap();
        assert_eq!(latest.generation, 3);
        assert_eq!(latest.todos.len(), 1);
        fs::remove_file(path.join("organizer/CURRENT")).unwrap();
        assert!(read_at(&path).is_err());
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn recovery_preview_never_automatically_replaces_corrupt_current() {
        let path = root();
        let state = read_at(&path).unwrap();
        let broken = b"CURRENT broken; do not silently fall back";
        fs::write(path.join("organizer/CURRENT"), broken).unwrap();
        assert!(read_at(&path).is_err());
        let candidates = recovery_candidates(&path).unwrap();
        assert_eq!(candidates.len(), 1);
        let preview =
            recovery_preview_at(&path, &format!("state-{}.json", state.generation)).unwrap();
        assert_eq!(preview.state.generation, state.generation);
        assert_eq!(preview.sha256.len(), 64);
        assert_eq!(fs::read(path.join("organizer/CURRENT")).unwrap(), broken);
        assert!(read_at(&path).is_err());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn recovery_rejects_invalid_or_changed_candidates_without_changing_current() {
        let path = root();
        let original = read_at(&path).unwrap();
        let broken = b"CURRENT stays unchanged";
        fs::write(path.join("organizer/CURRENT"), broken).unwrap();
        for name in [
            "../state-1.json",
            "organizer/state-1.json",
            "C:\\state-1.json",
            "state-01.json",
            "state-0.json",
            "state-bad.json",
        ] {
            assert!(recovery_preview_at(&path, name).is_err());
        }
        let mut valid = original.clone();
        valid.generation = 2;
        let mut invalid_schema = valid.clone();
        invalid_schema.schema = SCHEMA + 1;
        let mut invalid_generation = valid.clone();
        invalid_generation.generation = 3;
        let mut unmigrated = valid.clone();
        unmigrated.migrated = false;
        let mut invalid_ids = valid.clone();
        let mut todo = crate::model::TodoItem::new("无效 ID");
        todo.id.clear();
        invalid_ids.todos.push(todo);
        for candidate in [invalid_schema, invalid_generation, unmigrated, invalid_ids] {
            fs::write(
                path.join("organizer/state-2.json"),
                serde_json::to_vec(&candidate).unwrap(),
            )
            .unwrap();
            assert!(recovery_preview_at(&path, "state-2.json").is_err());
            assert_eq!(fs::read(path.join("organizer/CURRENT")).unwrap(), broken);
        }
        fs::write(
            path.join("organizer/state-2.json"),
            serde_json::to_vec(&valid).unwrap(),
        )
        .unwrap();
        let preview = recovery_preview_at(&path, "state-2.json").unwrap();
        valid.todos.push(crate::model::TodoItem::new("预览后改变"));
        fs::write(
            path.join("organizer/state-2.json"),
            serde_json::to_vec(&valid).unwrap(),
        )
        .unwrap();
        assert!(recovery_restore_at(&path, &preview)
            .unwrap_err()
            .contains("候选在预览后发生变化"));
        assert_eq!(fs::read(path.join("organizer/CURRENT")).unwrap(), broken);
        let preview = recovery_preview_at(&path, "state-2.json").unwrap();
        fs::write(path.join("organizer/CURRENT"), b"another CURRENT").unwrap();
        assert!(recovery_restore_at(&path, &preview)
            .unwrap_err()
            .contains("CURRENT 在预览后发生变化"));
        assert_eq!(
            fs::read(path.join("organizer/CURRENT")).unwrap(),
            b"another CURRENT"
        );
        assert_eq!(fs::read_dir(path.join("organizer")).unwrap().count(), 4); // LOCK、CURRENT、两个旧快照；未创建备份或新快照。
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn explicit_recovery_creates_new_readonly_snapshot_and_preserves_queue_and_current_backup() {
        use super::super::model::{
            Binding, CollectionState, Conflict, LocalMetadata, PendingChange, Resource, SendState,
        };
        let path = root();
        read_at(&path).unwrap();
        transaction_at(&path, |state| {
            state.accounts.push(crate::calendar::Config {
                provider: crate::calendar::Provider::Custom,
                username: "隔离测试".into(),
                account_id: "account".into(),
                two_way: true,
                server: Some("https://example.test/".into()),
                event_collection: Some("https://example.test/events/".into()),
                todo_collection: Some("https://example.test/tasks/".into()),
                ..Default::default()
            });
            state.binding = Some(Binding {
                account_id: "account".into(),
                generation: 3,
                event_collection: "https://example.test/events/".into(),
                todo_collection: "https://example.test/tasks/".into(),
                switching: false,
            });
            state.todos.push(crate::model::TodoItem::new("必须保留"));
            let raw = crate::calendar::caldav::ical::CalendarDocument::new_task(chrono::Utc::now())
                .unwrap()
                .to_ics();
            state.resources.insert(
                "resource".into(),
                Resource {
                    id: "resource".into(),
                    account_id: "account".into(),
                    collection_url: "https://example.test/tasks/".into(),
                    href: "https://example.test/tasks/one.ics".into(),
                    etag: Some("\"etag\"".into()),
                    local_ics: Some(raw.clone()),
                    baseline: Some(raw.clone()),
                    revision: 4,
                },
            );
            state.pending.push(PendingChange {
                id: "change".into(),
                resource_id: "resource".into(),
                revision: 4,
                base_etag: Some("\"etag\"".into()),
                baseline: Some(raw.clone()),
                content: Some(raw.clone()),
                state: SendState::Ready,
                attempts: 2,
                error: Some("保留错误详情".into()),
            });
            state.conflicts.push(Conflict {
                id: "conflict".into(),
                resource_id: "resource".into(),
                baseline: Some(raw.clone()),
                local: Some(raw),
                remote: None,
                remote_etag: None,
                revision: 4,
                resolved: false,
            });
            state.metadata.insert(
                "resource".into(),
                LocalMetadata {
                    reminded: true,
                    previous_percent: Some(37),
                    ..Default::default()
                },
            );
            state.collections.insert(
                "https://example.test/tasks/".into(),
                CollectionState {
                    url: "https://example.test/tasks/".into(),
                    todos: true,
                    writable: true,
                    full_scan: true,
                    sync_token: Some("old-token".into()),
                    ..Default::default()
                },
            );
            Ok(())
        })
        .unwrap();
        let before = read_at(&path).unwrap();
        let file = format!("state-{}.json", before.generation);
        let original_snapshot = fs::read(path.join("organizer").join(&file)).unwrap();
        fs::write(path.join("organizer/state-7.json"), b"uncommitted orphan").unwrap();
        let original_current = b"damaged original CURRENT bytes\xff";
        fs::write(path.join("organizer/CURRENT"), original_current).unwrap();
        let preview = recovery_preview_at(&path, &file).unwrap();
        let restored = recovery_restore_at(&path, &preview).unwrap();
        assert_eq!(restored.generation, 8);
        assert_eq!(restored.previous_generation, Some(before.generation));
        assert!(restored.accounts.iter().all(|account| !account.two_way));
        let binding = restored.binding.as_ref().unwrap();
        assert!(binding.switching);
        assert!(binding.generation > before.binding.as_ref().unwrap().generation);
        assert!(restored
            .collections
            .values()
            .all(|collection| !collection.full_scan && collection.sync_token.is_none()));
        assert_eq!(restored.pending[0].state, SendState::Unknown);
        assert_eq!(restored.pending[0].content, before.pending[0].content);
        assert_eq!(restored.pending[0].attempts, 2);
        assert_eq!(restored.pending[0].error, before.pending[0].error);
        assert_eq!(restored.todos, before.todos);
        assert_eq!(
            serde_json::to_value(&restored.resources).unwrap(),
            serde_json::to_value(&before.resources).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&restored.conflicts).unwrap(),
            serde_json::to_value(&before.conflicts).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&restored.metadata).unwrap(),
            serde_json::to_value(&before.metadata).unwrap()
        );
        assert_eq!(
            fs::read(path.join("organizer").join(&file)).unwrap(),
            original_snapshot
        );
        assert_eq!(
            fs::read(path.join("organizer/state-7.json")).unwrap(),
            b"uncommitted orphan"
        );
        let backups = fs::read_dir(path.join("organizer"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("CURRENT-recovery-")
            })
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(backups[0].path()).unwrap(), original_current);
        assert_eq!(read_at(&path).unwrap().generation, restored.generation);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn invalid_utf8_missing_target_and_checksum_failure_stop_reads() {
        let path = root();
        fs::write(path.join("todo.txt"), [0xff]).unwrap();
        assert!(read_at(&path).is_err());
        fs::remove_file(path.join("todo.txt")).unwrap();
        let state = read_at(&path).unwrap();
        fs::write(
            path.join(format!("organizer/state-{}.json", state.generation)),
            b"{}",
        )
        .unwrap();
        assert!(read_at(&path).is_err());
        fs::remove_dir_all(path).unwrap();
    }
}
