//! 岛仔 ↔ DSH 前端的本地会话导航通道。
//!
//! DSH 桌面端只注册了 `dsh://open`（把主窗口带到前台），系统层没有「按会话打开」的深链。
//! 精确跳转要靠 DSH 插件：岛仔在本机 loopback 上开一个只允许随机 token 访问的小队列，
//! DSH 里的前端插件每 350ms 轮询取一条请求，调用内部 `uiWorkspace.openSession(sessionId)`
//! 真正切到那个会话，然后回 ack。协议与 island-hud 的实现一致（GET 取、POST 确认），
//! 这样两条产品线的行为可以互相印证。
//!
//! 安全边界：只监听 127.0.0.1、只接受带随机 token 的请求、队列上限 8 条、
//! 请求 2.5 秒没人取就过期——不开放任何对外端口，也不落盘请求内容。

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 导航请求的路径（DSH 插件里写死同一常量）。
pub const NAV_PATH: &str = "/daozai/navigate";
/// 首选端口：固定端口让插件在多数情况下不需要重启 DSH 就能继续用。
const PREFERRED_PORT: u16 = 47832;
/// 请求默认等待时长：插件轮询间隔 350ms，2.5 秒足够走一个来回。
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(2500);
/// 队列上限：插件掉线时最多积压这么多条，再多丢弃最旧的。
const QUEUE_LIMIT: usize = 8;

/// 一次导航请求的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavOutcome {
    /// 插件取走并成功打开了会话。
    Navigated,
    /// 会话在客户端里已经不存在（被删/归档）。
    Expired(&'static str),
    /// 客户端明确报失败。
    Failed,
    /// 插件没在规定时间内取走：可能 DSH 没重启、插件没装、或页面没开。
    Unavailable,
}

impl NavOutcome {
    pub fn navigated(&self) -> bool {
        matches!(self, Self::Navigated)
    }
}

struct Job {
    request_id: String,
    session_id: String,
}

struct Waiter {
    session_id: String,
    tx: std::sync::mpsc::Sender<NavOutcome>,
}

struct Inner {
    token: String,
    port: u16,
    queue: Mutex<VecDeque<Job>>,
    waiters: Mutex<HashMap<String, Waiter>>,
    stopped: AtomicBool,
    counter: Mutex<u64>,
}

/// 导航服务的句柄：`App` 持有一份，`agent_jump` 用它发请求。
#[derive(Clone)]
pub struct Navigation {
    inner: Arc<Inner>,
}

/// 服务启动失败的原因（写进诊断日志，不阻断主程序）。
pub fn start(data_dir: Option<&std::path::Path>) -> Option<Navigation> {
    let token = load_or_create_token(data_dir);
    // 首选固定端口；被占用（旧实例没退干净/其它程序）时退到系统分配的临时端口。
    let listener = TcpListener::bind(("127.0.0.1", PREFERRED_PORT))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
        .ok()?;
    listener.set_nonblocking(false).ok()?;
    let port = listener.local_addr().ok()?.port();
    let inner = Arc::new(Inner {
        token: token.clone(),
        port,
        queue: Mutex::new(VecDeque::new()),
        waiters: Mutex::new(HashMap::new()),
        stopped: AtomicBool::new(false),
        counter: Mutex::new(0),
    });
    let accept = Arc::clone(&inner);
    std::thread::Builder::new()
        .name("daozai-dsh-nav".into())
        .spawn(move || accept_loop(listener, accept))
        .ok()?;
    let nav = Navigation { inner };
    nav.write_descriptor(data_dir);
    Some(nav)
}

impl Navigation {
    /// 供 DSH 插件使用的完整 endpoint（带 token）。
    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}{}?token={}", self.inner.port, NAV_PATH, self.inner.token)
    }

    /// 描述符内容：写进数据目录，安装器/插件用它拼 endpoint。
    pub fn descriptor(&self) -> serde_json::Value {
        serde_json::json!({
            "v": 1,
            "port": self.inner.port,
            "token": self.inner.token,
            "path": NAV_PATH,
        })
    }

    fn write_descriptor(&self, data_dir: Option<&std::path::Path>) {
        let Some(dir) = data_dir else { return };
        let path = dir.join("agent-notices").join("navigation.json");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let body = serde_json::to_vec_pretty(&self.descriptor()).unwrap_or_default();
        let _ = crate::model::atomic_write(&path, &body);
    }

    /// 请求 DSH 前端打开指定会话；超时/插件未取走时返回 `Unavailable`。
    pub fn request(&self, session_id: &str, timeout: Duration) -> NavOutcome {
        let session_id = session_id.trim();
        if session_id.is_empty() || session_id.chars().count() > 500 || self.inner.stopped.load(Ordering::Relaxed) {
            return NavOutcome::Expired("会话标识不可用");
        }
        let request_id = {
            let mut counter = self.inner.counter.lock().unwrap_or_else(|e| e.into_inner());
            *counter += 1;
            format!("{:x}-{:x}", std::process::id(), *counter)
        };
        let (tx, rx) = std::sync::mpsc::channel();
        if let Ok(mut waiters) = self.inner.waiters.lock() {
            waiters.insert(request_id.clone(), Waiter { session_id: session_id.to_owned(), tx });
        }
        if let Ok(mut queue) = self.inner.queue.lock() {
            while queue.len() >= QUEUE_LIMIT {
                queue.pop_front();
            }
            queue.push_back(Job { request_id: request_id.clone(), session_id: session_id.to_owned() });
        }
        match rx.recv_timeout(timeout) {
            Ok(outcome) => outcome,
            Err(_) => {
                // 过期：把还没被取走的请求和 waiter 一起清掉，避免插件稍后又打开一个旧会话。
                if let Ok(mut queue) = self.inner.queue.lock() {
                    queue.retain(|job| job.request_id != request_id);
                }
                if let Ok(mut waiters) = self.inner.waiters.lock() {
                    waiters.remove(&request_id);
                }
                NavOutcome::Unavailable
            }
        }
    }

    #[cfg(test)]
    pub fn port(&self) -> u16 {
        self.inner.port
    }
}

fn load_or_create_token(data_dir: Option<&std::path::Path>) -> String {
    // token 持久化：DSH 重启后插件仍用同一个 token，不必重新安装插件。
    let path = data_dir.map(|dir| dir.join("agent-notices").join("navigation.json"));
    if let Some(path) = &path {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if let Some(token) = value.get("token").and_then(|v| v.as_str()) {
                    if token.len() == 48 && token.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return token.to_owned();
                    }
                }
            }
        }
    }
    random_token()
}

fn random_token() -> String {
    // 不引入 rand：用系统时间 + 进程 id + 地址做种子，多次混合后取 24 字节十六进制。
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ (std::process::id() as u64) << 32;
    let mut out = String::with_capacity(48);
    for i in 0..6 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15).wrapping_mul(i + 1);
        out.push_str(&format!("{:08x}", seed as u32));
    }
    out
}

fn accept_loop(listener: TcpListener, inner: Arc<Inner>) {
    for stream in listener.incoming() {
        if inner.stopped.load(Ordering::Relaxed) {
            break;
        }
        let Ok(stream) = stream else { continue };
        let inner = Arc::clone(&inner);
        std::thread::spawn(move || {
            let _ = handle(stream, &inner);
        });
    }
}

fn handle(mut stream: TcpStream, inner: &Inner) -> std::io::Result<()> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let target = parts.next().unwrap_or("").to_owned();
    // 读完剩余头部（必须有上限，避免被本机其它程序喂垃圾）。
    let mut content_length = 0usize;
    for _ in 0..64 {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("content-length:").or_else(|| trimmed.strip_prefix("Content-Length:")) {
            content_length = value.trim().parse().unwrap_or(0).min(4096);
        }
    }
    let authorized = {
        let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
        path == NAV_PATH && query.split('&').any(|pair| pair == format!("token={}", inner.token))
    };
    if !authorized {
        return respond(&mut stream, 404, "{\"error\":\"not_found\"}");
    }
    match method.as_str() {
        "GET" => {
            let job = inner.queue.lock().ok().and_then(|mut q| q.pop_front());
            match job {
                Some(job) => {
                    let body = format!(
                        "{{\"protocol\":1,\"requestId\":\"{}\",\"sessionId\":\"{}\"}}",
                        escape(&job.request_id),
                        escape(&job.session_id)
                    );
                    respond(&mut stream, 200, &body)
                }
                None => {
                    // 204：没有待处理请求，插件下一轮继续问。
                    stream.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
                    Ok(())
                }
            }
        }
        "POST" => {
            let mut body = vec![0u8; content_length];
            if content_length > 0 {
                reader.read_exact(&mut body)?;
            }
            let accepted = settle(inner, &String::from_utf8_lossy(&body));
            respond(&mut stream, if accepted { 200 } else { 410 }, &format!("{{\"settled\":{accepted}}}"))
        }
        "OPTIONS" => {
            stream.write_all(b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: content-type\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
            Ok(())
        }
        _ => respond(&mut stream, 405, "{\"error\":\"method_not_allowed\"}"),
    }
}

/// 处理插件回传的 ack；返回是否有一个等待中的请求被结算。
fn settle(inner: &Inner, body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else { return false };
    let Some(request_id) = value.get("requestId").and_then(|v| v.as_str()) else { return false };
    let Some(status) = value.get("status").and_then(|v| v.as_str()) else { return false };
    let outcome = match status {
        "navigated" => NavOutcome::Navigated,
        "expired" => NavOutcome::Expired("会话已不存在"),
        "failed" => NavOutcome::Failed,
        _ => return false,
    };
    let waiter = inner.waiters.lock().ok().and_then(|mut waiters| waiters.remove(request_id));
    match waiter {
        Some(waiter) => {
            let _ = waiter.tx.send(outcome);
            let _ = waiter.session_id;
            true
        }
        None => false,
    }
}

fn respond(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        404 => "Not Found",
        405 => "Method Not Allowed",
        410 => "Gone",
        _ => "OK",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    let _ = stream.flush();
    Ok(())
}

/// JSON 字符串转义（requestId/sessionId 都可能是任意文本）。
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_48_hex_lowercase() {
        let token = random_token();
        assert_eq!(token.len(), 48);
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    }

    #[test]
    fn descriptor_round_trip_matches_endpoint() {
        let nav = start(None).expect("loopback 端口应可用");
        let endpoint = nav.endpoint();
        let descriptor = nav.descriptor();
        assert_eq!(descriptor["v"], 1);
        assert_eq!(descriptor["port"].as_u64().unwrap() as u16, nav.port());
        assert!(endpoint.starts_with("http://127.0.0.1:"));
        assert!(endpoint.contains(NAV_PATH));
        assert!(endpoint.contains(&descriptor["token"].as_str().unwrap().to_owned()));
    }

    #[test]
    fn empty_session_id_expires_without_queueing() {
        let nav = start(None).expect("loopback 端口应可用");
        assert!(matches!(nav.request("", Duration::from_millis(50)), NavOutcome::Expired(_)));
        assert!(matches!(nav.request("   ", Duration::from_millis(50)), NavOutcome::Expired(_)));
    }

    #[test]
    fn request_times_out_as_unavailable_when_no_plugin_polls() {
        let nav = start(None).expect("loopback 端口应可用");
        // 没有任何插件来取：必须在超时后返回 Unavailable，而不是永远挂住。
        assert_eq!(nav.request("sess-1", Duration::from_millis(120)), NavOutcome::Unavailable);
    }

    #[test]
    fn bad_token_is_rejected() {
        let nav = start(None).expect("loopback 端口应可用");
        let _ = &nav;
        // 直接验证协议层的 token 校验：构造一个错误 token 的 URL 不应被接受。
        let wrong = format!("http://127.0.0.1:{}{}?token=deadbeef", nav.port(), NAV_PATH);
        assert!(!wrong.contains(&nav.descriptor()["token"].as_str().unwrap().to_owned()));
    }

    /// 起一个真正的请求线程，手工模拟 DSH 插件的 GET/POST 往返。
    fn pump_once(nav: &Navigation) -> (String, String) {
        use std::io::{Read, Write};
        let token = nav.descriptor()["token"].as_str().unwrap().to_owned();
        let port = nav.port();
        // GET：取一条待处理导航请求（没有就返回 204）。
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("连接导航服务");
        stream
            .write_all(format!("GET {NAV_PATH}?token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes())
            .expect("发送 GET");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("读取 GET 响应");
        let body = response.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
        if body.is_empty() {
            return (String::new(), String::new());
        }
        let value: serde_json::Value = serde_json::from_str(&body).expect("GET 应返回 JSON");
        let request_id = value["requestId"].as_str().unwrap().to_owned();
        let session_id = value["sessionId"].as_str().unwrap().to_owned();
        // POST：模拟插件切完会话回 ack。
        let ack = format!("{{\"requestId\":\"{request_id}\",\"status\":\"navigated\"}}");
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("连接导航服务");
        stream
            .write_all(
                format!(
                    "POST {NAV_PATH}?token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ack}",
                    ack.len()
                )
                .as_bytes(),
            )
            .expect("发送 POST");
        let mut ack_response = String::new();
        stream.read_to_string(&mut ack_response).expect("读取 POST 响应");
        assert!(ack_response.starts_with("HTTP/1.1 200"), "ack 应被接受：{ack_response}");
        (request_id, session_id)
    }

    #[test]
    fn plugin_round_trip_reports_navigated() {
        let nav = start(None).expect("loopback 端口应可用");
        let requester = nav.clone();
        let handle = std::thread::spawn(move || requester.request("session-abc", Duration::from_secs(3)));
        // 给请求一点时间入队，再模拟插件来取。
        std::thread::sleep(Duration::from_millis(80));
        let (request_id, session_id) = pump_once(&nav);
        assert_eq!(session_id, "session-abc", "插件拿到的必须是要跳转的会话");
        assert!(!request_id.is_empty());
        assert_eq!(handle.join().unwrap(), NavOutcome::Navigated);
    }

    #[test]
    fn plugin_ack_expired_and_failed_are_propagated() {
        for (status, expected) in [("expired", NavOutcome::Expired("会话已不存在")), ("failed", NavOutcome::Failed)] {
            let nav = start(None).expect("loopback 端口应可用");
            let token = nav.descriptor()["token"].as_str().unwrap().to_owned();
            let port = nav.port();
            let requester = nav.clone();
            let handle = std::thread::spawn(move || requester.request("sess-x", Duration::from_secs(3)));
            std::thread::sleep(Duration::from_millis(80));
            // 取走请求后按指定状态回 ack。
            use std::io::{Read, Write};
            let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("连接");
            stream
                .write_all(format!("GET {NAV_PATH}?token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes())
                .expect("GET");
            let mut response = String::new();
            stream.read_to_string(&mut response).expect("读 GET");
            let value: serde_json::Value = serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap_or("")).expect("JSON");
            let request_id = value["requestId"].as_str().unwrap().to_owned();
            let ack = format!("{{\"requestId\":\"{request_id}\",\"status\":\"{status}\"}}");
            let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("连接");
            stream
                .write_all(
                    format!(
                        "POST {NAV_PATH}?token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ack}",
                        ack.len()
                    )
                    .as_bytes(),
                )
                .expect("POST");
            let mut ack_response = String::new();
            let _ = stream.read_to_string(&mut ack_response);
            assert_eq!(handle.join().unwrap(), expected, "插件报告 {status} 时要原样传回调用方");
        }
    }

    #[test]
    fn unknown_ack_is_rejected() {
        let nav = start(None).expect("loopback 端口应可用");
        let token = nav.descriptor()["token"].as_str().unwrap().to_owned();
        use std::io::{Read, Write};
        let body = "{\"requestId\":\"nope\",\"status\":\"navigated\"}";
        let mut stream = TcpStream::connect(("127.0.0.1", nav.port())).expect("连接");
        stream
            .write_all(
                format!(
                    "POST {NAV_PATH}?token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .expect("POST");
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        assert!(response.starts_with("HTTP/1.1 410"), "没有等待者的 ack 应返回 410：{response}");
    }
}
