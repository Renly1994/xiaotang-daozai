//! daozai-bridge：浏览器扩展的 Native Messaging 宿主（独立精简版）。
//!
//! 只做「接收 Chrome/Edge 下载事件 → 翻译成 island_status.json 条目」这一件事，
//! 不链接 GUI 栈。
//!
//! 协议：stdin/stdout 上跑 Native Messaging（4 字节小端长度 + UTF-8 JSON），
//! 写入 island_status.json，与状态总线其他写入者共用同一把文件锁（island_status.lock 首字节）。
//!
//! 消息类型：
//! - `dl`（下载事件，state = progress/complete/cancel）
//! - `status`（通用状态透传，白名单字段）
//! - `ping`（保活）
//!
//! 配置：读 `%APPDATA%\Daozai\bridge.json`（与主程序设置页共用），
//! `done_seconds`（完成提示时长）、`min_progress_mb`（进度门槛）。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use std::os::windows::io::AsRawHandle;
use windows::core::HSTRING;
use windows::Win32::Storage::FileSystem::{
    LockFileEx, MoveFileExW, UnlockFile, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    MOVEFILE_REPLACE_EXISTING,
};
use windows::Win32::System::IO::OVERLAPPED;

/// 进行中条目的自动消失时限（秒）：桥进程挂了也不会留僵尸胶囊，活着就靠覆写续期。
const PROGRESS_SECONDS: f64 = 15.0;
/// 单条消息上限（字节）：浏览器侧出了问题也不会把内存顶爆。
const MAX_MESSAGE: usize = 1 << 20;
/// 配置 / 状态文件的上限（字节），与主程序一致。
const MAX_JSON_BYTES: u64 = 1_048_576;

fn main() {
    std::process::exit(run_host() as i32);
}

/// 宿主主循环：读到 stdin 断开（浏览器回收 Service Worker）为止，返回退出码。
fn run_host() -> u32 {
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    let mut speed: HashMap<String, (u64, Instant)> = HashMap::new();
    let mut head = [0u8; 4];
    let mut body = Vec::new();
    loop {
        if reader.read_exact(&mut head).is_err() {
            break; // 端口断开，正常退出
        }
        let len = u32::from_le_bytes(head) as usize;
        if len == 0 || len > MAX_MESSAGE {
            break;
        }
        body.clear();
        body.resize(len, 0);
        if reader.read_exact(&mut body).is_err() {
            break;
        }
        let reply = match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(msg) => handle(msg, &mut speed),
            Err(error) => serde_json::json!({ "ok": false, "error": error.to_string() }),
        };
        if write_frame(&mut writer, &reply).is_err() {
            break;
        }
    }
    0
}

/// 处理一条消息并返回给浏览器的回执。
fn handle(msg: serde_json::Value, speed: &mut HashMap<String, (u64, Instant)>) -> serde_json::Value {
    let ok = (|| {
        match msg.get("t").and_then(|t| t.as_str()) {
            Some("dl") => handle_download(&msg, speed, &prefs_cached()),
            Some("status") => handle_status(&msg),
            Some("ping") => {}
            _ => return Err(format!("unknown t={:?}", msg.get("t"))),
        }
        Ok(())
    })();
    match ok {
        Ok(()) => serde_json::json!({ "ok": true }),
        Err(error) => serde_json::json!({ "ok": false, "error": error }),
    }
}

/// 用户偏好（bridge.json，主程序设置 → 浏览器下载页写入）。5 秒缓存。
fn prefs_cached() -> BridgePrefs {
    use std::sync::Mutex;
    static CACHE: Mutex<Option<(Instant, BridgePrefs)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    if let Some((at, prefs)) = cache.as_ref() {
        if at.elapsed().as_secs() < 5 {
            return prefs.clone();
        }
    }
    let prefs = BridgePrefs::load();
    *cache = Some((Instant::now(), prefs.clone()));
    prefs
}

fn handle_download(msg: &serde_json::Value, speed: &mut HashMap<String, (u64, Instant)>, prefs: &BridgePrefs) {
    // 每个下载用自身 id 跟踪速度（speed 表），状态文件里只留「一条」聚合条目（固定 key "dl"），
    // 多下载并发时 text 尾部带 (+N) 角标、进度只看最新一条。
    let own = msg.get("id").and_then(|v| v.as_u64()).map(|n| n.to_string()).unwrap_or_else(|| {
        msg.get("name").and_then(|v| v.as_str()).unwrap_or("x").to_string()
    });
    let own_key = format!("dl-{own}");
    let name = short_name(msg.get("name").and_then(|v| v.as_str()).unwrap_or("下载"));
    let state = msg.get("state").and_then(|v| v.as_str()).unwrap_or("progress");

    match state {
        "cancel" => {
            speed.remove(&own_key);
            if !speed.is_empty() {
                return; // 还有别的活跃下载，聚合条目交给它们续写。
            }
            let item = serde_json::json!({
                "id": "dl", "text": format!("已取消下载 · {name}"),
                "seconds": 5.0, "color": "amber", "ttl": unix_secs() + 5.0,
            });
            set_status("dl", item);
        }
        "complete" => {
            speed.remove(&own_key);
            let done_seconds = prefs.done_seconds as f64;
            let mut item = serde_json::json!({
                "id": "dl", "text": format!("下载完成 · {name}"),
                "seconds": done_seconds, "color": "green",
                "ttl": unix_secs() + done_seconds,
            });
            let path = msg.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if !path.is_empty() && Path::new(path).is_file() {
                item["click"] = serde_json::json!(format!("file:{path}"));
            }
            set_status("dl", item);
        }
        _ => {
            let received = msg.get("received").and_then(|v| v.as_u64()).unwrap_or(0);
            let total = msg.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
            let threshold = prefs.min_progress_mb as u64 * 1024 * 1024;
            if threshold > 0 && total > 0 && total < threshold {
                return; // 小文件不打扰
            }
            let now = Instant::now();
            let mut detail = match total {
                0 => fmt_bytes(received),
                _ => format!("{} / {}", fmt_bytes(received), fmt_bytes(total)),
            };
            if let Some((prev, at)) = speed.get(&own_key) {
                if now > *at {
                    let rate = (received.saturating_sub(*prev)) as f64 / now.duration_since(*at).as_secs_f64();
                    if rate > 0.0 {
                        detail.push_str(&format!(" · {}/s", fmt_bytes(rate as u64)));
                    }
                }
            }
            speed.insert(own_key.clone(), (received, now));
            let active = speed.len();
            let label = if active > 1 {
                format!("下载 · {name} (+{})", active)
            } else {
                format!("下载 · {name}")
            };
            let mut item = serde_json::json!({
                "id": "dl", "text": label,
                "detail": detail, "seconds": PROGRESS_SECONDS, "color": "blue",
                "ttl": unix_secs() + PROGRESS_SECONDS,
            });
            if total > 0 {
                item["progress"] = serde_json::json!((received as f64 / total as f64).clamp(0.0, 1.0));
            }
            set_status("dl", item);
        }
    }
}

/// 通用状态透传的白名单字段，未知字段一律丢弃。
const STATUS_FIELDS: [&str; 9] = [
    "id", "text", "detail", "seconds", "progress", "color", "click", "ttl", "priority",
];

fn handle_status(msg: &serde_json::Value) {
    let mut item = serde_json::Map::new();
    for field in STATUS_FIELDS {
        if let Some(value) = msg.get(field).filter(|v| !v.is_null()) {
            item.insert(field.into(), value.clone());
        }
    }
    let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
    if text.is_empty() {
        return;
    }
    let key = item.get("id").and_then(|v| v.as_str()).unwrap_or(text).to_string();
    set_status(&key, serde_json::Value::Object(item));
}

/// 状态文件路径：与岛仔读的同一位置。
fn status_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        let dir = std::env::temp_dir().join(format!("daozai-bridge-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        return Some(dir.join("island_status.json"));
    }
    #[cfg(not(test))]
    {
        let base = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(base).join("Daozai").join("island_status.json"))
    }
}

/// 覆写一条状态（同 id 替换），写失败不影响主循环。
fn set_status(key: &str, item: serde_json::Value) {
    let done = with_status_lock(|| {
        let path = status_path()?;
        let mut list: Vec<serde_json::Value> = std::fs::read(&path)
            .ok()
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default();
        list.retain(|entry| {
            let id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let fallback = entry.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let same = if id.is_empty() { fallback == key } else { id == key };
            !same
        });
        list.push(item);
        atomic_write(&path, &serde_json::to_vec(&list).ok()?)
    })
    .is_some();
    if !done { eprintln!("bridge: set_status({key}) failed"); }
}

/// 临时文件 + 原子替换（MOVEFILE_REPLACE_EXISTING），岛仔只会看到完整 JSON。
fn atomic_write(path: &Path, bytes: &[u8]) -> Option<()> {
    let mut tmp = path.to_path_buf();
    tmp.set_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, bytes) { eprintln!("bridge: tmp write {e}"); return None; }
    let from = HSTRING::from(tmp.as_os_str());
    let to = HSTRING::from(path.as_os_str());
    unsafe {
        if let Err(e) = MoveFileExW(&from, &to, MOVEFILE_REPLACE_EXISTING) { eprintln!("bridge: move {e}"); return None; }
    }
    Some(())
}

/// 状态文件的写锁：与 python `StatusBus` 同一把（island_status.lock 首字节）。
fn with_status_lock<T>(op: impl FnOnce() -> T) -> Option<T> {
    static GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = GUARD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = status_path()?.with_extension("lock");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    // 必须「读写」打开：只写句柄上 LockFileEx 会报拒绝访问。
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).open(&path).ok()?;
    let handle = windows::Win32::Foundation::HANDLE(file.as_raw_handle());
    let mut overlapped = OVERLAPPED::default();
    for _ in 0..50 {
        let locked = unsafe {
            LockFileEx(handle, LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY, 0, 1, 0, &mut overlapped)
        };
        if locked.is_ok() {
            let out = op();
            unsafe { let _ = UnlockFile(handle, 0, 0, 1, 0); };
            return Some(out);
        }
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
    eprintln!("bridge: status lock busy");
    None
}

/// 帧编码：4 字节小端长度 + 内容。
fn write_frame(writer: &mut impl Write, value: &serde_json::Value) -> std::io::Result<()> {
    let raw = serde_json::to_vec(value)?;
    writer.write_all(&(raw.len() as u32).to_le_bytes())?;
    writer.write_all(&raw)?;
    writer.flush()
}

fn unix_secs() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn fmt_bytes(n: u64) -> String {
    const KB: u64 = 1 << 10;
    const MB: u64 = 1 << 20;
    const GB: u64 = 1 << 30;
    if n >= GB {
        format!("{:.1} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.1} KB", n as f64 / KB as f64)
    } else {
        format!("{n} B")
    }
}

fn short_name(name: &str) -> String {
    let base = name.rsplit(['\\', '/']).next().unwrap_or(name);
    let mut chars: Vec<char> = base.chars().collect();
    if chars.len() > 24 {
        chars.truncate(23);
        chars.push('…');
    }
    chars.into_iter().collect()
}

/// 桥偏好：与主程序 model::BridgePrefs 字段一致，独立定义避免引入整个 model 模块。
#[derive(Clone, Default)]
struct BridgePrefs {
    done_seconds: u32,
    min_progress_mb: u32,
}

impl BridgePrefs {
    fn load() -> Self {
        #[cfg(test)]
        { return Self { done_seconds: 12, min_progress_mb: 0 }; }
        #[cfg(not(test))]
        {
            let path = config_dir().join("bridge.json");
            let Some(raw) = read_capped(&path) else { return Self::default_prefs() };
            let Ok(prefs) = serde_json::from_slice::<SerdePrefs>(&raw) else { return Self::default_prefs() };
            let mut p = Self { done_seconds: prefs.done_seconds.clamp(3, 60), min_progress_mb: prefs.min_progress_mb.min(500) };
            if p.done_seconds == 0 { p = Self::default_prefs(); }
            p
        }
    }
    fn default_prefs() -> Self {
        Self { done_seconds: 12, min_progress_mb: 0 }
    }
}

#[derive(serde::Deserialize)]
struct SerdePrefs {
    #[serde(default)]
    done_seconds: u32,
    #[serde(default)]
    min_progress_mb: u32,
}

/// 配置目录：与主程序 model::config_dir() 一致（DAOZAI_DATA_DIR 可覆盖）。
fn config_dir() -> PathBuf {
    if let Ok(root) = std::env::var("DAOZAI_DATA_DIR") {
        return PathBuf::from(root);
    }
    env_dir("APPDATA").join("Daozai")
}

fn env_dir(var: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_default()
}

fn read_capped(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_JSON_BYTES { return None; }
    std::fs::read(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 字节数格式化覆盖各档位() {
        assert_eq!(fmt_bytes(999), "999 B");
        assert_eq!(fmt_bytes(2048), "2.0 KB");
        assert_eq!(fmt_bytes(60 * 1024 * 1024), "60.0 MB");
        assert_eq!(fmt_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn 文件名取最后一段并截断() {
        assert_eq!(short_name(r"C:\Users\cay\Downloads\demo.zip"), "demo.zip");
        let long = "很长的文件名这是一段超过二十四个字符的下载文件名.bin";
        assert_eq!(short_name(long).chars().count(), 24);
        assert!(short_name(long).ends_with('…'));
    }

    fn read_test_status(id: &str) -> Option<serde_json::Value> {
        let path = status_path()?;
        let list: Vec<serde_json::Value> = std::fs::read(&path).ok().and_then(|raw| serde_json::from_slice(&raw).ok())?;
        list.into_iter().find(|entry| entry.get("id").and_then(|v| v.as_str()) == Some(id))
    }

    #[test]
    fn 下载消息翻译成状态总线条目() {
        let mut speed = HashMap::new();
        let prefs = BridgePrefs::default_prefs();
        let msg = serde_json::json!({
            "t":"dl","id":17,"name":"C:\\x\\演示文件.zip","state":"progress",
            "received":60*1024*1024,"total":100*1024*1024
        });
        handle_download(&msg, &mut speed, &prefs);
        assert!(speed.contains_key("dl-17"));
        let entry = read_test_status("dl").expect("progress 应写入聚合条目 dl");
        assert_eq!(entry.get("color").and_then(|v| v.as_str()), Some("blue"));
        assert_eq!(entry.get("text").and_then(|v| v.as_str()), Some("下载 · 演示文件.zip"));
        let tmp = std::env::temp_dir().join("daozai-bridge-bin-test.txt");
        std::fs::write(&tmp, b"x").unwrap();
        let msg = serde_json::json!({
            "t":"dl","id":17,"name":"note.txt","state":"complete","path":tmp.to_string_lossy()
        });
        handle_download(&msg, &mut speed, &prefs);
        assert!(speed.is_empty());
        let entry = read_test_status("dl").expect("complete 应覆盖聚合条目");
        assert_eq!(entry.get("color").and_then(|v| v.as_str()), Some("green"));
        assert!(entry.get("click").is_some());
    }

    #[test]
    fn 下载条目必须带ttl() {
        let mut speed = HashMap::new();
        let before = unix_secs();
        handle_download(&serde_json::json!({"t":"dl","id":18,"name":"a.zip","state":"progress","received":10,"total":100}), &mut speed, &BridgePrefs::default_prefs());
        let entry = read_test_status("dl").unwrap();
        let ttl = entry.get("ttl").and_then(|v| v.as_f64()).unwrap();
        assert!((before + 10.0..=before + 20.0).contains(&ttl));
    }

    #[test]
    fn 多下载聚合角标且不冲掉其他条目() {
        let mut speed = HashMap::new();
        let prefs = BridgePrefs::default_prefs();
        handle_download(&serde_json::json!({"t":"dl","id":1,"name":"a.bin","state":"progress","received":100,"total":1000}), &mut speed, &prefs);
        handle_download(&serde_json::json!({"t":"dl","id":2,"name":"b.bin","state":"progress","received":200,"total":2000}), &mut speed, &prefs);
        set_status("微信 · 张三", serde_json::json!({"text": "微信 · 张三"}));
        handle_download(&serde_json::json!({"t":"dl","id":3,"name":"c.bin","state":"progress","received":300,"total":3000}), &mut speed, &prefs);
        let entry = read_test_status("dl").unwrap();
        let text = entry.get("text").and_then(|v| v.as_str()).unwrap_or("");
        assert!(text.contains("c.bin") && text.contains("(+3)"), "应显示最新 + 角标：{text}");
        let raw = std::fs::read(status_path().unwrap()).unwrap();
        let list: Vec<serde_json::Value> = serde_json::from_slice(&raw).unwrap();
        assert_eq!(list.iter().filter(|e| e.get("id").and_then(|v| v.as_str()) == Some("dl")).count(), 1);
        assert!(list.iter().any(|e| e.get("text").and_then(|v| v.as_str()) == Some("微信 · 张三")));
    }

    #[test]
    fn 取消下载显示已取消且不残留历史() {
        let mut speed = HashMap::new();
        let prefs = BridgePrefs::default_prefs();
        handle_download(&serde_json::json!({"t":"dl","id":1,"name":"a.bin","state":"progress","received":100,"total":1000}), &mut speed, &prefs);
        handle_download(&serde_json::json!({"t":"dl","id":2,"name":"b.bin","state":"progress","received":200,"total":2000}), &mut speed, &prefs);
        handle_download(&serde_json::json!({"t":"dl","id":1,"name":"a.bin","state":"cancel"}), &mut speed, &prefs);
        let entry = read_test_status("dl").unwrap();
        assert!(entry.get("text").and_then(|v| v.as_str()).unwrap_or("").contains("b.bin"));
        handle_download(&serde_json::json!({"t":"dl","id":2,"name":"b.bin","state":"cancel"}), &mut speed, &prefs);
        let entry = read_test_status("dl").unwrap();
        assert_eq!(entry.get("color").and_then(|v| v.as_str()), Some("amber"));
        assert!(entry.get("text").and_then(|v| v.as_str()).unwrap_or("").contains("已取消下载"));
    }

    #[test]
    fn 帧编码解码往返一致() {
        let value = serde_json::json!({ "ok": true, "中文": "字段" });
        let mut buffer: Vec<u8> = Vec::new();
        write_frame(&mut buffer, &value).unwrap();
        let len = u32::from_le_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
        assert_eq!(len + 4, buffer.len());
        let back: serde_json::Value = serde_json::from_slice(&buffer[4..]).unwrap();
        assert_eq!(back, value);
    }

    #[test]
    fn 单测写状态不污染用户真实文件() {
        let mut speed = HashMap::new();
        handle_download(&serde_json::json!({"t":"dl","id":99,"name":"x.zip","state":"progress","received":1,"total":2}), &mut speed, &BridgePrefs::default_prefs());
        let real = std::env::var_os("APPDATA").map(|base| PathBuf::from(base).join("Daozai").join("island_status.json"));
        if let Some(path) = real.filter(|p| p.exists()) {
            let raw = std::fs::read_to_string(&path).unwrap_or_default();
            assert!(!raw.contains("x.zip"));
        }
    }
}
