#![cfg(windows)]

use daozai::calendar::{self, Config, Provider};
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct TestData {
    path: std::path::PathBuf,
    previous: Option<std::ffi::OsString>,
}

impl TestData {
    fn new(label: &str, blocked: bool) -> Self {
        let id = format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let path = std::env::temp_dir().join(format!("daozai-credential-{label}-{id}"));
        if blocked { std::fs::write(&path, "不是目录".as_bytes()).unwrap(); }
        else { std::fs::create_dir_all(&path).unwrap(); }
        let previous = std::env::var_os("DAOZAI_DATA_DIR");
        std::env::set_var("DAOZAI_DATA_DIR", &path);
        Self { path, previous }
    }

    fn account(&self, provider: Provider) -> Config {
        Config { provider, username: format!("test-{}-{}", provider.key(), self.path.file_name().unwrap().to_string_lossy()), ..Config::default() }
    }
}

impl Drop for TestData {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous { std::env::set_var("DAOZAI_DATA_DIR", previous); }
        else { std::env::remove_var("DAOZAI_DATA_DIR"); }
        // organizer 快照目录也只在本测试的唯一临时路径内清理。
        if self.path.is_dir() { let _ = std::fs::remove_dir_all(&self.path); }
        else { let _ = std::fs::remove_file(&self.path); }
    }
}

#[test]
fn failed_configuration_preserves_old_credentials() {
    let _guard = ENV_LOCK.lock().unwrap();
    let data = TestData::new("blocked", true);
    let old = data.account(Provider::Feishu);
    let new = Config { username: format!("{}-new", old.username), ..old.clone() };
    calendar::secrets::save(&old, "旧密码").unwrap();
    assert!(calendar::save_connection(&new, "新密码", Some(&old)).is_err());
    assert_eq!(calendar::secrets::load(&old).unwrap(), "旧密码");
    assert!(calendar::secrets::load(&new).is_err());
    assert!(calendar::save_connection(&old, "更新后的密码", Some(&old)).is_err());
    assert_eq!(calendar::secrets::load(&old).unwrap(), "旧密码");
    calendar::secrets::delete(&old);
    calendar::secrets::delete(&new);
}

#[test]
fn calendar_platform_connections_are_independent() {
    let _guard = ENV_LOCK.lock().unwrap();
    let data = TestData::new("platforms", false);
    let legacy = data.account(Provider::Feishu);
    // 模拟升级前仅有一个平台，凭据保存在旧键；迁移不应先填 ID 而丢失密码引用。
    calendar::secrets::save(&legacy, "飞书测试密码").unwrap();
    std::fs::write(data.path.join("calendar.json"), calendar::encode_config(&legacy)).unwrap();
    let ding = data.account(Provider::DingTalk);
    calendar::save_connection(&ding, "钉钉测试密码", Some(&legacy)).unwrap();
    let ding = calendar::load_config_strict().unwrap().unwrap();
    assert!(!ding.account_id.is_empty());
    assert_eq!(calendar::secrets::load(&legacy).unwrap(), "飞书测试密码");
    assert_eq!(calendar::load_configs_strict().unwrap().len(), 2);
    calendar::save_config(&legacy).unwrap();
    let feishu = calendar::load_config_strict().unwrap().unwrap();
    assert!(!feishu.account_id.is_empty());
    assert_eq!(calendar::secrets::load(&feishu).unwrap(), "飞书测试密码");
    assert!(calendar::secrets::load(&legacy).is_err());
    calendar::select_provider(Provider::WeCom).unwrap();
    assert_eq!(calendar::load_config_strict().unwrap(), None);
    assert_eq!(calendar::load_configs_strict().unwrap().len(), 2);
    calendar::select_provider(Provider::Feishu).unwrap();
    calendar::forget_config().unwrap();
    assert_eq!(calendar::load_config_strict().unwrap(), None);
    assert_eq!(calendar::load_configs_strict().unwrap(), vec![ding.clone()]);
    calendar::secrets::delete(&feishu);
    calendar::secrets::delete(&ding);
}

#[test]
fn stable_credentials_follow_identity_and_preserve_username_case() {
    let _guard = ENV_LOCK.lock().unwrap();
    let data = TestData::new("identity", false);
    let cfg = data.account(Provider::Custom);
    calendar::save_connection(&cfg, "初始密码", None).unwrap();
    let first = calendar::load_config_strict().unwrap().unwrap();
    let discovered = Config { href: Some("/calendar/".into()), ..first.clone() };
    calendar::save_config(&discovered).unwrap();
    let discovered = calendar::load_config_strict().unwrap().unwrap();
    assert_eq!(first.account_id, discovered.account_id);
    assert_eq!(calendar::secrets::load(&discovered).unwrap(), "初始密码");
    let changed = Config { username: discovered.username.to_ascii_uppercase(), ..discovered.clone() };
    calendar::save_connection(&changed, "大小写变更密码", Some(&discovered)).unwrap();
    let changed = calendar::load_config_strict().unwrap().unwrap();
    assert_ne!(first.account_id, changed.account_id);
    assert_eq!(calendar::secrets::load(&changed).unwrap(), "大小写变更密码");
    assert!(calendar::secrets::load(&first).is_err());
    let server = Config { server: Some("https://dav.example.invalid/".into()), ..changed.clone() };
    calendar::save_connection(&server, "新服务密码", Some(&changed)).unwrap();
    let server = calendar::load_config_strict().unwrap().unwrap();
    assert_ne!(changed.account_id, server.account_id);
    assert_eq!(calendar::secrets::load(&server).unwrap(), "新服务密码");
    assert!(calendar::secrets::load(&changed).is_err());
    calendar::secrets::delete(&server);
}

#[test]
fn migrated_credentials_do_not_fall_back_to_legacy_key() {
    let _guard = ENV_LOCK.lock().unwrap();
    let data = TestData::new("no-fallback", false);
    let legacy = data.account(Provider::Custom);
    calendar::secrets::save(&legacy, "旧键密码").unwrap();
    let migrated = Config { account_id: daozai::organizer::new_id(), ..legacy.clone() };
    assert!(calendar::secrets::load(&migrated).is_err());
    calendar::secrets::delete(&legacy);
}

#[test]
fn failed_snapshot_commit_restores_stable_credentials() {
    use std::os::windows::fs::OpenOptionsExt;
    let _guard = ENV_LOCK.lock().unwrap();
    let data = TestData::new("snapshot-failure", false);
    let cfg = data.account(Provider::Custom);
    calendar::save_connection(&cfg, "原密码", None).unwrap();
    let cfg = calendar::load_config_strict().unwrap().unwrap();
    // 允许读取 CURRENT，但不允许替换/删除，确保失败发生在新凭据已写入之后。
    let current = std::fs::OpenOptions::new().read(true).share_mode(1)
        .open(data.path.join("organizer").join("CURRENT")).unwrap();
    assert!(calendar::save_connection(&cfg, "提交失败的新密码", Some(&cfg)).is_err());
    assert_eq!(calendar::load_config_strict().unwrap(), Some(cfg.clone()));
    assert_eq!(calendar::secrets::load(&cfg).unwrap(), "原密码");
    drop(current);
    calendar::secrets::delete(&cfg);
}

#[test]
#[ignore = "仅由跨进程凭据测试作为子进程调用"]
fn credential_lock_holder_process() {
    use std::io::Write;
    use std::os::windows::fs::OpenOptionsExt;
    assert_eq!(std::env::var("DAOZAI_CREDENTIAL_LOCK_CHILD").as_deref(), Ok("1"));
    let root = std::env::var_os("LOCALAPPDATA").filter(|path| !path.is_empty()).expect("缺少 LOCALAPPDATA");
    let dir = std::path::PathBuf::from(root).join("Daozai");
    std::fs::create_dir_all(&dir).unwrap();
    let lock = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).share_mode(0)
        .open(dir.join("calendar-credentials.lock")).unwrap();
    println!("DAOZAI_CREDENTIAL_LOCK_READY");
    std::io::stdout().flush().unwrap();
    let mut release = String::new();
    std::io::stdin().read_line(&mut release).unwrap();
    drop(lock);
}

#[test]
fn another_process_cannot_overwrite_credentials_during_save() {
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};
    let _guard = ENV_LOCK.lock().unwrap();
    let data = TestData::new("process-lock", false);
    calendar::save_connection(&data.account(Provider::Custom), "原密码", None).unwrap();
    let cfg = calendar::load_config_strict().unwrap().unwrap();
    let generation = daozai::organizer::read_state().unwrap().generation;
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "credential_lock_holder_process", "--nocapture", "--test-threads=1"])
        .env("DAOZAI_CREDENTIAL_LOCK_CHILD", "1")
        .env("DAOZAI_DATA_DIR", data.path.join("other-data-dir"))
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        assert_ne!(stdout.read_line(&mut line).unwrap(), 0, "子进程未能获取凭据保存锁");
        if line.contains("DAOZAI_CREDENTIAL_LOCK_READY") { break; }
        line.clear();
    }
    let result = calendar::save_connection(&cfg, "不应写入的新密码", Some(&cfg));
    writeln!(child.stdin.take().unwrap(), "release").unwrap();
    assert!(child.wait().unwrap().success());
    assert!(result.unwrap_err().contains("其他进程正在保存日历凭据"));
    assert_eq!(calendar::secrets::load(&cfg).unwrap(), "原密码");
    assert_eq!(daozai::organizer::read_state().unwrap().generation, generation);
    // 子进程退出后，锁应被释放，正常保存恢复可用。
    calendar::save_connection(&cfg, "后续成功密码", Some(&cfg)).unwrap();
    assert_eq!(calendar::secrets::load(&cfg).unwrap(), "后续成功密码");
    calendar::secrets::delete(&cfg);
}

#[test]
fn disconnect_keeps_binding_resources_and_pending_changes() {
    use daozai::organizer::{Binding, PendingChange, Resource, SendState};
    let _guard = ENV_LOCK.lock().unwrap();
    let data = TestData::new("disconnect", false);
    calendar::save_config(&data.account(Provider::Custom)).unwrap();
    let cfg = calendar::load_config_strict().unwrap().unwrap();
    daozai::organizer::transaction(|state| {
        let account = state.accounts.iter_mut().find(|account| account.account_id == cfg.account_id).unwrap();
        account.two_way = true;
        account.event_collection = Some("https://dav.example.invalid/events/".into());
        account.todo_collection = Some("https://dav.example.invalid/tasks/".into());
        state.binding = Some(Binding { account_id: cfg.account_id.clone(), generation: state.generation,
            event_collection: account.event_collection.clone().unwrap(), todo_collection: account.todo_collection.clone().unwrap(), switching: false });
        state.resources.insert("resource".into(), Resource { id: "resource".into(), account_id: cfg.account_id.clone(),
            collection_url: "https://dav.example.invalid/tasks/".into(), href: "https://dav.example.invalid/tasks/test.ics".into(),
            etag: Some("\"etag\"".into()), local_ics: Some("test-local".into()), baseline: Some("test-base".into()), revision: 1 });
        state.pending.push(PendingChange { id: "pending".into(), resource_id: "resource".into(), revision: 1,
            base_etag: Some("\"etag\"".into()), baseline: Some("test-base".into()), content: Some("test-local".into()),
            state: SendState::Ready, attempts: 0, error: None });
        Ok(())
    }).unwrap();
    calendar::forget_config().unwrap();
    let state = daozai::organizer::read_state().unwrap();
    assert!(state.accounts.is_empty());
    assert!(state.binding.unwrap().switching);
    assert_eq!(state.resources.len(), 1);
    assert_eq!(state.pending.len(), 1);
    assert_eq!(state.pending[0].content.as_deref(), Some("test-local"));
}

#[test]
fn corrupt_legacy_configuration_is_not_overwritten() {
    let _guard = ENV_LOCK.lock().unwrap();
    let data = TestData::new("corrupt", false);
    let path = data.path.join("calendar.json");
    let raw = br#"{"active":"feishu","accounts":[{"username":"test-valid"},{"username":23}]}"#;
    std::fs::write(&path, raw).unwrap();
    assert!(calendar::load_configs_strict().is_err());
    assert!(calendar::save_config(&data.account(Provider::DingTalk)).is_err());
    assert_eq!(std::fs::read(path).unwrap(), raw);
    assert!(!data.path.join("organizer").join("CURRENT").exists());
}
