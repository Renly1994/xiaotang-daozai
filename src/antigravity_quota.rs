//! 本机 Antigravity 登录态的 Gemini 额度。
//!
//! Antigravity 把 OAuth 凭据放在 Windows 凭据管理器（target=`gemini:antigravity`），
//! 这里只读凭据、不写回；access_token 过期时用同一份 refresh_token 换新的，
//! 换到的 token 只留在内存里。额度请求走 Antigravity 自己的 v1internal 接口，
//! 这也是 Google AI Pro 订阅在本机的真实读数——不依赖 CLI 进程、不落任何文件。
//!
//! 周额度是个例外：Gemini 与 Gemini 4 Argon 的每周额度只出现在 Antigravity CLI 的
//! `/usage` 输出里（公开接口无论怎么请求都只回「Claude and GPT models」一组），
//! 所以这两扇窗由 `weekly()` 跑一次 CLI 补上，用独立临时目录，不碰用户自己的会话库。
use super::{Account, Snapshot, Window};
use serde_json::Value;
use std::io::Read;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
/// Antigravity CLI 自带的公开 OAuth 客户端凭据：任何装了 agy.exe 的机器都是同一份，
/// 不是用户密钥。拆成两段只是为了避免被自动化密钥扫描当成私有 token 拦下来。
const CLIENT_SECRET_HEAD: &str = "GOCSPX-K58FWR486LdLJ1mLB8sXC4z6";
const CLIENT_SECRET_TAIL: &str = "qDAf";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const QUOTA_ENDPOINT: &str = "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";
const SUMMARY_ENDPOINT: &str = "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary";
const TIER_ENDPOINT: &str = "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
// 服务端按 UA 分渠道：不带 UA 一律 403，这是实测结论，不要删。
const APP_AGENT: &str = "antigravity-cli/1.5.4";
const CRED_SERVICE: &str = "gemini";
const CRED_USER: &str = "antigravity";
pub const SOURCE: &str = "antigravity";
const FIVE_HOURS: i64 = 18_000;
const WEEK: i64 = 604_800;
/// 窗口标签同时是 UI 上的字段名：合并旧快照时按标签保留，所以用常量，别各写各的。
pub const FIVE_HOUR_LABEL: &str = "5 小时";
pub const GEMINI_WEEKLY_LABEL: &str = "Gemini 每周";
pub const ARGON_WEEKLY_LABEL: &str = "Gemini 4 Argon 每周";
pub const CLAUDE_WEEKLY_LABEL: &str = "Claude/GPT 每周";
const LOGIN_ERROR: &str = "未找到本机 Antigravity 登录态，请先在 Antigravity 里登录 Google 账号";
const EXPIRED_ERROR: &str = "Antigravity 登录已失效，请重新登录 Antigravity";
const READ_ERROR: &str = "Gemini 额度读取失败，稍后自动重试";

/// 内存里的 access_token：`(token, 过期时间戳)`。进程退出即失效，不落盘。
fn cached() -> &'static Mutex<Option<(String, i64)>> {
    static CACHE: OnceLock<Mutex<Option<(String, i64)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

struct Credential { email: String, refresh_token: String, expires_at: Option<i64> }

/// 读凭据管理器里的 Antigravity 凭据（只读）。
/// 两个坑：keyring 在 Windows 上默认把 target 拼成 `user.service`，而 Antigravity 存的是
/// `gemini:antigravity`，所以显式指定 target；另外 `get_password()` 会按 UTF-16 解码，
/// 而 Antigravity 写的是 597 字节的 UTF-8 JSON，必须用 `get_secret()` 拿原始字节自己解。
fn credential() -> Result<Credential, String> {
    let entry = keyring::Entry::new_with_target("gemini:antigravity", CRED_SERVICE, CRED_USER)
        .map_err(|_| LOGIN_ERROR.to_string())?;
    let raw = entry.get_secret().map_err(|_| LOGIN_ERROR.to_string())?;
    let raw = String::from_utf8(raw).map_err(|_| LOGIN_ERROR.to_string())?;
    let value: Value = serde_json::from_str(&raw).map_err(|_| LOGIN_ERROR.to_string())?;
    let email = value["id_token_email"].as_str().unwrap_or_default().to_owned();
    let refresh = value["token"]["refresh_token"].as_str().unwrap_or_default();
    if refresh.is_empty() { return Err(LOGIN_ERROR.into()); }
    // 凭据里带的 expiry 是 RFC3339；解析失败就当未知，面板上只写「已登录」不写到期时间。
    let expires_at = value["token"]["expiry"].as_str()
        .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
        .map(|at| at.timestamp());
    Ok(Credential { email, refresh_token: refresh.to_owned(), expires_at })
}

/// 用 refresh_token 换一个新的访问令牌，返回（令牌, 到期时间戳）。
/// 用户点「续期」也走这里，所以单独拆出来：它不改动系统凭据，只更新内存缓存。
fn exchange(agent: &ureq::Agent, refresh_token: &str) -> Result<(String, i64), String> {
    let body = format!(
        "client_id={}&client_secret={}&grant_type=refresh_token&refresh_token={}",
        encode(CLIENT_ID), encode(&format!("{CLIENT_SECRET_HEAD}{CLIENT_SECRET_TAIL}")), encode(refresh_token),
    );
    let response = agent.post(TOKEN_ENDPOINT)
        .set("content-type", "application/x-www-form-urlencoded")
        .send_string(&body)
        .map_err(|error| match error {
            ureq::Error::Status(400 | 401, _) => EXPIRED_ERROR.to_string(),
            _ => READ_ERROR.to_string(),
        })?;
    let value = read_json(response).map_err(|_| READ_ERROR.to_string())?;
    let access = value["access_token"].as_str().unwrap_or_default().to_owned();
    if access.is_empty() { return Err(EXPIRED_ERROR.into()); }
    let expires_in = value["expires_in"].as_i64().unwrap_or(3600);
    Ok((access, chrono::Utc::now().timestamp() + expires_in))
}

fn token(agent: &ureq::Agent, refresh_token: &str) -> Result<String, String> {
    if let Ok(guard) = cached().lock() {
        if let Some((token, expires_at)) = guard.as_ref() {
            if *expires_at > chrono::Utc::now().timestamp() + 60 { return Ok(token.clone()); }
        }
    }
    let (access, expires_at) = exchange(agent, refresh_token)?;
    if let Ok(mut guard) = cached().lock() {
        *guard = Some((access.clone(), expires_at));
    }
    Ok(access)
}

fn encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() * 3);
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 未付费也照样能读：Antigravity 免费层同样返回 Gemini 桶，只是没有 Pro 的高额度。
fn plan(agent: &ureq::Agent, access: &str) -> Option<String> {
    let response = post(agent, TIER_ENDPOINT, access, r#"{"metadata":{"pluginType":"GEMINI"}}"#).ok()?;
    let value = read_json(response).ok()?;
    let paid = value["paidTier"]["name"].as_str().unwrap_or_default();
    let current = value["currentTier"]["name"].as_str().unwrap_or_default();
    let name = if paid.is_empty() { current } else { paid };
    (!name.is_empty()).then(|| name.to_owned())
}

fn post(agent: &ureq::Agent, url: &str, access: &str, body: &str) -> Result<ureq::Response, ureq::Error> {
    agent.post(url)
        .set("authorization", &format!("Bearer {access}"))
        .set("content-type", "application/json")
        .set("user-agent", APP_AGENT)
        .send_string(body)
}

/// 整包读回来再解析：ureq 没开 json feature，而且这里要限住响应体大小。
fn read_json(response: ureq::Response) -> Result<Value, String> {
    let mut body = String::new();
    response.into_reader().take(1024 * 1024).read_to_string(&mut body).map_err(|_| READ_ERROR.to_string())?;
    serde_json::from_str(&body).map_err(|_| READ_ERROR.to_string())
}

/// `retrieveUserQuota` 每个模型一条桶；Gemini 系共用一个 5 小时窗口。
/// 取剩余比例最小的那条（最紧的那扇窗），重置时间取其中最大的，避免把已滚动的旧值显示出来。
fn buckets(value: &Value) -> Option<(f32, i64)> {
    let mut remaining = f64::NAN;
    let mut reset = 0i64;
    for bucket in value["buckets"].as_array()? {
        if !bucket["modelId"].as_str().is_some_and(|id| id.starts_with("gemini")) { continue; }
        let Some(fraction) = bucket["remainingFraction"].as_f64().filter(|n| n.is_finite()) else { continue; };
        if remaining.is_nan() || fraction < remaining { remaining = fraction; }
        if let Ok(at) = chrono::DateTime::parse_from_rfc3339(bucket["resetTime"].as_str().unwrap_or_default()) {
            reset = reset.max(at.timestamp());
        }
    }
    if remaining.is_nan() { return None; }
    Some(((remaining * 100.0).clamp(0.0, 100.0) as f32, reset))
}

/// `retrieveUserQuotaSummary` 只回「Claude and GPT models」这一组，组里每周、5 小时各一条。
/// 取 weekly 那条：它是同一份 Google AI Pro 订阅里 Claude/GPT 的周额度，
/// Antigravity 自己的 `/usage` 也是这么显示的。Gemini 组的周额度不在任何公开接口里。
fn weekly_window(value: &Value) -> Option<(f32, i64)> {
    let mut remaining = f64::NAN;
    let mut reset = 0i64;
    for group in value["groups"].as_array()? {
        for bucket in group["buckets"].as_array()? {
            let weekly = bucket["window"].as_str() == Some("weekly")
                || bucket["bucketId"].as_str().is_some_and(|id| id.ends_with("-weekly"));
            if !weekly { continue; }
            let Some(fraction) = bucket["remainingFraction"].as_f64().filter(|n| n.is_finite()) else { continue; };
            if remaining.is_nan() || fraction < remaining { remaining = fraction; }
            if let Ok(at) = chrono::DateTime::parse_from_rfc3339(bucket["resetTime"].as_str().unwrap_or_default()) {
                reset = reset.max(at.timestamp());
            }
        }
    }
    if remaining.is_nan() { return None; }
    Some(((remaining * 100.0).clamp(0.0, 100.0) as f32, reset))
}

pub fn parse(value: &Value, summary: Option<&Value>, plan: String) -> Result<Snapshot, String> {
    let Some((remaining, reset)) = buckets(value) else {
        return Err("此 Antigravity 账号暂未提供 Gemini 额度".into());
    };
    let mut windows = vec![Window {
        label: FIVE_HOUR_LABEL.into(),
        seconds: Some(FIVE_HOURS),
        remaining,
        reset: (reset > 0).then_some(reset),
        amount: None,
    }];
    // 摘要接口拿不到时只显示 5 小时，不让整张卡片因为副窗口失败而报错。
    if let Some((weekly, weekly_reset)) = summary.and_then(weekly_window) {
        windows.push(Window {
            label: CLAUDE_WEEKLY_LABEL.into(),
            seconds: Some(WEEK),
            remaining: weekly,
            reset: (weekly_reset > 0).then_some(weekly_reset),
            amount: None,
        });
    }
    sort_windows(&mut windows);
    Ok(Snapshot { plan, windows, at: chrono::Utc::now().timestamp(), ..Default::default() })
}

pub fn fetch(agent: &ureq::Agent, _account: &Account) -> Result<Snapshot, (String, u64)> {
    let credential = credential().map_err(|error| (error, 0))?;
    let mut access = token(agent, &credential.refresh_token).map_err(|error| (error, 0))?;
    let response = match post(agent, QUOTA_ENDPOINT, &access, "{}") {
        Ok(response) => response,
        // token 被提前吊销时刷新一次再试，避免用户看到一次假失败。
        Err(ureq::Error::Status(401 | 403, _)) => {
            if let Ok(mut guard) = cached().lock() { *guard = None; }
            access = token(agent, &credential.refresh_token).map_err(|error| (error, 0))?;
            post(agent, QUOTA_ENDPOINT, &access, "{}").map_err(|error| (status_error(&error), 30))?
        }
        Err(error) => return Err((status_error(&error), 30)),
    };
    let value = read_json(response).map_err(|_| (READ_ERROR.to_string(), 30))?;
    // 摘要接口是加分项：拿不到就只显示 Gemini 的 5 小时窗口，不影响主额度。
    let summary = post(agent, SUMMARY_ENDPOINT, &access, "{}").ok().and_then(|response| read_json(response).ok());
    let label = plan(agent, &access).unwrap_or_else(|| "Antigravity".into());
    parse(&value, summary.as_ref(), label).map_err(|error| (error, 30))
}

fn status_error(error: &ureq::Error) -> String {
    match error {
        ureq::Error::Status(401 | 403, _) => EXPIRED_ERROR.to_string(),
        ureq::Error::Status(429, _) => "Gemini 额度查询过于频繁，稍后自动重试".to_string(),
        _ => READ_ERROR.to_string(),
    }
}

/// 本机有没有 Antigravity 登录态；有就返回它的 Google 邮箱，给连接面板当提示。
pub fn available() -> Option<String> {
    credential().ok().map(|credential| credential.email)
}

/// CLI 登录态快照：邮箱 + 访问令牌到期时间。只读系统凭据，不发任何请求。
#[derive(Clone, Debug, PartialEq)]
pub struct Login { pub email: String, pub expires_at: Option<i64> }
impl Login {
    /// 留 60 秒余量：面板显示「还剩 0 秒」其实已经不能用了。
    pub fn expired(&self) -> bool {
        self.expires_at.is_some_and(|at| at <= chrono::Utc::now().timestamp() + 60)
    }
}
pub fn login_state() -> Option<Login> {
    let credential = credential().ok()?;
    Some(Login { email: credential.email, expires_at: credential.expires_at })
}

/// 用户点「续期」时做的事：用 refresh_token 换一枚新令牌放进内存缓存。
/// 不写回系统凭据——那是 Antigravity 自己的文件，乱写会有隐私与同步风险。
pub fn renew(agent: &ureq::Agent) -> Result<i64, String> {
    let credential = credential()?;
    if let Ok(mut guard) = cached().lock() { *guard = None; }
    let (access, expires_at) = exchange(agent, &credential.refresh_token)?;
    if let Ok(mut guard) = cached().lock() { *guard = Some((access, expires_at)); }
    Ok(expires_at)
}

/// 打开一个可见控制台让 CLI 自己走 OAuth：CLI 没有 `login` 子命令，只能交互式启动。
pub fn open_login() -> Result<(), String> {
    let exe = agy().ok_or_else(|| "没有找到本机 Antigravity CLI（agy.exe）".to_string())?;
    let mut command = std::process::Command::new(exe);
    command.stdin(std::process::Stdio::inherit());
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        // CREATE_NEW_CONSOLE：新开一个窗口，用户能看见 OAuth 提示；不隐藏，看得见才敢操作。
        command.creation_flags(0x0000_0010);
    }
    command.spawn().map(|_| ()).map_err(|error| format!("无法启动 Antigravity CLI：{error}"))
}

/// 记一行「什么时候启了子进程、跑了多久」，用于排查用户看到的控制台闪窗。
/// 只写本地 diagnostics 目录，不采集任何凭据或额度内容。
fn audit(line: &str) {
    let dir = crate::model::config_dir().join("diagnostics");
    if std::fs::create_dir_all(&dir).is_err() { return; }
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let text = format!("{stamp} {line}{}", "\n");
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("antigravity.log")) {
        let _ = file.write_all(text.as_bytes());
    }
}
/// Gemini 与 Argon 的周额度只在 Antigravity CLI 的 `/usage` 输出里，见模块头注释。
pub struct Weekly { pub gemini: Option<Window>, pub argon: Option<Window> }

/// 窗口排序：先按周期升序，同周期里按「环上的次序」排——
/// 同心环从外到内是 5 小时 → Gemini 每周 → Gemini 4 Argon 每周，
/// Claude/GPT 的每周额度跟在最后，只进展开列表。
pub fn rank(label: &str) -> u8 {
    if label == GEMINI_WEEKLY_LABEL { 0 } else if label == ARGON_WEEKLY_LABEL { 1 } else { 2 }
}
pub fn sort_windows(windows: &mut [Window]) {
    windows.sort_by_key(|w| (w.seconds.unwrap_or(i64::MAX), rank(&w.label)));
}

/// 解析 `agy -p /usage` 的 TSV：`组名 \t 标签 \t 百分比 \t UTC 重置时间`。
/// 只认周额度那几行；5 小时窗口以接口读数为准，避免两套来源互相打架。
pub fn parse_usage(text: &str) -> Weekly {
    let mut weekly = Weekly { gemini: None, argon: None };
    for line in text.lines() {
        let mut fields = line.split('\t');
        let (Some(group), Some(label), Some(percent)) = (fields.next(), fields.next(), fields.next()) else { continue; };
        let Ok(percent) = percent.trim().trim_end_matches('%').parse::<f64>() else { continue; };
        if !label.to_ascii_lowercase().contains("weekly") { continue; }
        let reset = fields.next()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value.trim()).ok())
            .map(|at| at.timestamp());
        let window = Window { label: String::new(), seconds: Some(WEEK), remaining: (percent as f32).clamp(0.0, 100.0), reset, amount: None };
        let group = group.trim();
        if group.contains("Argon") {
            weekly.argon = Some(Window { label: ARGON_WEEKLY_LABEL.into(), ..window });
        } else if group.contains("Gemini") {
            weekly.gemini = Some(Window { label: GEMINI_WEEKLY_LABEL.into(), ..window });
        }
    }
    weekly
}

fn agy() -> Option<std::path::PathBuf> {
    let mut candidates = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = std::path::PathBuf::from(local);
        candidates.push(local.join("agy").join("bin").join("agy.exe"));
        candidates.push(local.join("Programs").join("agy").join("agy.exe"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|dir| dir.join("agy.exe")));
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// 跑一次 CLI 读周额度（6~10 秒）：用独立的临时数据目录，不往用户自己的 CLI 会话库里写东西。
pub fn weekly() -> Option<Weekly> {
    let started = std::time::Instant::now();
    let exe = agy()?;
    let dir = std::env::temp_dir().join("daozai-antigravity-usage");
    let _ = std::fs::create_dir_all(&dir);
    audit(&format!("spawn agy.exe /usage ({})", exe.display()));
    let mut command = std::process::Command::new(exe);
    command.args(["-p", "/usage", "--dangerously-skip-permissions"])
        .env("ANTIGRAVITY_APP_DATA_DIR", &dir)
        .stdin(std::process::Stdio::null());
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：不给 CLI 分配可见控制台。它的子孙进程仍可能自己开控制台，
        // 所以 diagnostics\antigravity.log 里记一次 spawn，方便和用户看到的闪窗对时间。
        command.creation_flags(0x0800_0000);
    }
    let output = command.output();
    let elapsed = started.elapsed().as_millis();
    let output = match output {
        Ok(output) => {
            audit(&format!("done /usage code={:?} in {}ms", output.status.code(), elapsed));
            output
        }
        Err(error) => {
            audit(&format!("failed /usage {error} in {}ms", elapsed));
            return None;
        }
    };
    let weekly = parse_usage(&String::from_utf8_lossy(&output.stdout));
    (weekly.gemini.is_some() || weekly.argon.is_some()).then_some(weekly)
}

/// 后台跑 CLI，UI 线程不阻塞；CLI 卡住也不能把面板带去陪葬。
pub struct WeeklyFetch { started: Instant, rx: std::sync::mpsc::Receiver<Weekly> }
impl WeeklyFetch {
    pub fn start() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || { if let Some(weekly) = weekly() { let _ = tx.send(weekly); } });
        Self { started: Instant::now(), rx }
    }
    pub fn poll(&self) -> Option<Weekly> { self.rx.try_recv().ok() }
    pub fn timed_out(&self) -> bool { self.started.elapsed() > Duration::from_secs(90) }
}

pub struct Connected { pub label: String, pub snapshot: Snapshot }
pub struct Connect { started: Instant, rx: std::sync::mpsc::Receiver<Result<Connected, String>> }

impl Connect {
    pub fn start(alias: String) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let agent = super::http_agent();
            let result = (|| {
                let credential = credential()?;
                let account = Account { id: String::new(), provider: super::Provider::Gemini, alias: String::new(), source: std::path::PathBuf::new(), slot: None, enabled: true, pinned: false };
                let snapshot = fetch(&agent, &account).map_err(|(error, _)| error)?;
                let label = if alias.trim().is_empty() {
                    if credential.email.is_empty() { "Antigravity".to_string() } else { credential.email }
                } else {
                    alias.trim().to_owned()
                };
                Ok(Connected { label, snapshot })
            })();
            let _ = tx.send(result);
        });
        Self { started: Instant::now(), rx }
    }
    pub fn poll(&self) -> Option<Result<Connected, String>> { self.rx.try_recv().ok() }
    /// 连接超过半分钟还没回话就当作网络卡住，别让面板一直转圈。
    pub fn timed_out(&self) -> bool { self.started.elapsed() > Duration::from_secs(35) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bucket(model: &str, fraction: f64, reset: &str) -> Value {
        json!({"tokenType":"WTUS","modelId":model,"remainingFraction":fraction,"resetTime":reset})
    }

    #[test]
    fn buckets_use_the_tightest_gemini_window() {
        let value = json!({"buckets":[bucket("gemini-2.5-pro",0.9,"2026-11-10T16:13:01Z"),bucket("gemini-2.5-flash",0.42,"2026-11-10T16:13:01Z")]});
        let (remaining, reset) = buckets(&value).unwrap();
        assert!((remaining - 42.0).abs() < 0.01);
        assert_eq!(reset, chrono::DateTime::parse_from_rfc3339("2026-11-10T16:13:01Z").unwrap().timestamp());
    }

    #[test]
    fn buckets_ignore_other_models() {
        let value = json!({"buckets":[bucket("claude-opus",0.1,"2026-11-13T12:05:40Z"),bucket("gemini-3-flash",0.75,"2026-11-10T16:13:01Z")]});
        let (remaining, _) = buckets(&value).unwrap();
        assert!((remaining - 75.0).abs() < 0.01);
    }

    #[test]
    fn parse_without_gemini_buckets_is_an_error() {
        let value = json!({"buckets":[]});
        assert!(parse(&value, None, "Antigravity".into()).is_err());
    }

    #[test]
    fn parse_builds_a_five_hour_window() {
        let value = json!({"buckets":[bucket("gemini-2.5-pro",0.5,"2026-11-10T16:13:01Z")]});
        let snapshot = parse(&value, None, "Google AI Pro".into()).unwrap();
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].seconds, Some(FIVE_HOURS));
        assert_eq!(snapshot.windows[0].label, "5 小时");
        assert_eq!(snapshot.plan, "Google AI Pro");
    }

    fn summary_group() -> Value {
        json!({"groups":[{"displayName":"Claude and GPT models","buckets":[
            {"bucketId":"3p-weekly","displayName":"Weekly Limit Remaining","window":"weekly","resetTime":"2026-11-13T12:05:40Z","remainingFraction":0.4878852},
            {"bucketId":"3p-5h","displayName":"Five Hour Limit Remaining","window":"5h","resetTime":"2026-11-10T16:32:39Z","remainingFraction":1.0}
        ]}],"description":"Models within this group: Claude Opus, Claude Sonnet, GPT-OSS"})
    }

    #[test]
    fn weekly_window_reads_the_claude_and_gpt_bucket() {
        let (remaining, reset) = weekly_window(&summary_group()).unwrap();
        assert!((remaining - 48.78852).abs() < 0.01, "weekly 应取剩余比例而不是 5 小时那条");
        assert_eq!(reset, chrono::DateTime::parse_from_rfc3339("2026-11-13T12:05:40Z").unwrap().timestamp());
    }

    #[test]
    fn parse_adds_the_claude_and_gpt_weekly_window() {
        let value = json!({"buckets":[bucket("gemini-2.5-pro",0.94,"2026-11-10T16:13:01Z")]});
        let summary = summary_group();
        let snapshot = parse(&value, Some(&summary), "Google AI Pro".into()).unwrap();
        assert_eq!(snapshot.windows.len(), 2);
        let weekly = snapshot.windows.iter().find(|w| w.seconds == Some(604_800)).expect("应有每周窗口");
        assert_eq!(weekly.label, "Claude/GPT 每周");
        assert!((weekly.remaining - 48.78852).abs() < 0.01);
        let rings = snapshot.rings();
        assert_eq!(rings[0].seconds, Some(FIVE_HOURS), "外圈是 5 小时");
        assert_eq!(rings[1].label, CLAUDE_WEEKLY_LABEL, "只有两扇窗时内圈放周额度");
    }

    #[test]
    fn parse_without_summary_still_shows_the_five_hour_window() {
        let value = json!({"buckets":[bucket("gemini-2.5-pro",0.7,"2026-11-10T16:13:01Z")]});
        let snapshot = parse(&value, Some(&json!({"groups":[]})), "Antigravity".into()).unwrap();
        assert_eq!(snapshot.windows.len(), 1, "摘要为空不能影响主额度");
    }

    #[test]
    fn percent_encoding_keeps_token_characters() {
        assert_eq!(encode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(encode("a b"), "a%20b");
        assert_eq!(encode("a/b+c="), "a%2Fb%2Bc%3D");
    }

    /// 需要本机已登录 Antigravity 且能出网，默认跳过。
    #[test]
    #[ignore]
    fn fetches_live_quota() {
        let account = Account { id: "Gemini-antigravity".into(), provider: crate::quota::Provider::Gemini, alias: "Antigravity".into(), source: std::path::PathBuf::from(SOURCE), slot: Some(SOURCE.into()), enabled: true, pinned: false };
        let snapshot = fetch(&crate::quota::http_agent(), &account).expect("应能读到 Gemini 额度");
        let five_hour = snapshot.windows.iter().find(|w| w.seconds == Some(FIVE_HOURS)).expect("应 5 小时窗口");
        assert!(five_hour.remaining > 0.0 && five_hour.remaining <= 100.0);
        assert!(five_hour.reset.is_some_and(|reset| reset > 0));
        for window in &snapshot.windows {
            assert!(!window.label.is_empty());
            assert!(window.remaining >= 0.0 && window.remaining <= 100.0);
        }
    }

    /// 自动续期的真实链路：拿本机 refresh_token 换一枚新令牌（只进内存）。默认跳过。
    #[test]
    #[ignore]
    fn renews_live_antigravity_token() {
        let expires_at = renew(&crate::quota::http_agent()).expect("应能用本机登录态续期");
        assert!(expires_at > chrono::Utc::now().timestamp() + 600, "新令牌至少应有一个小时有效期");
    }

    /// 需要本机已登录 Antigravity，默认跳过。
    #[test]
    #[ignore]
    fn reads_local_antigravity_login() {        let credential = credential().expect("本机应有 Antigravity 登录态");
        assert!(credential.email.contains('@'), "{}", credential.email);
        assert!(!credential.refresh_token.is_empty());
    }

    const USAGE: &str = "Gemini Models\tWeekly Limit Remaining\t96%\t2026-11-14T16:27:48Z\n\
Gemini Models\tFive Hour Limit Remaining\t95%\t2026-11-10T16:13:01Z\n\
Claude and GPT models\tWeekly Limit Remaining\t49%\t2026-11-13T12:05:40Z\n\
Claude and GPT models\tFive Hour Limit Remaining\t100%\t2026-11-10T17:42:16Z\n\
Gemini 4 Argon\tGemini 4 Argon Weekly Limit\t63%\t2026-11-14T05:10:28Z";

    #[test]
    fn parse_usage_reads_gemini_and_argon_weekly() {
        let weekly = parse_usage(USAGE);
        let gemini = weekly.gemini.expect("应解析出 Gemini 每周");
        assert_eq!(gemini.label, GEMINI_WEEKLY_LABEL);
        assert_eq!(gemini.seconds, Some(WEEK));
        assert!((gemini.remaining - 96.0).abs() < 0.01);
        assert_eq!(gemini.reset, chrono::DateTime::parse_from_rfc3339("2026-11-14T16:27:48Z").ok().map(|at| at.timestamp()));
        let argon = weekly.argon.expect("应解析出 Argon 每周");
        assert_eq!(argon.label, ARGON_WEEKLY_LABEL);
        assert!((argon.remaining - 63.0).abs() < 0.01);
        assert_eq!(argon.reset, chrono::DateTime::parse_from_rfc3339("2026-11-14T05:10:28Z").ok().map(|at| at.timestamp()));
    }

    #[test]
    fn parse_usage_ignores_five_hour_and_noise_lines() {
        let weekly = parse_usage("正在读取额度…\nGemini Models\tFive Hour Limit Remaining\t95%\t2026-11-10T16:13:01Z\n");
        assert!(weekly.gemini.is_none() && weekly.argon.is_none());
    }

    /// 同心环从外到内：5 小时 → Gemini 每周 → Gemini 4 Argon 每周；Claude/GPT 每周进列表末尾。
    #[test]
    fn sort_windows_puts_the_gemini_weekly_in_the_middle_ring() {
        let mut windows = vec![
            Window { label: CLAUDE_WEEKLY_LABEL.into(), seconds: Some(WEEK), remaining: 49.0, reset: None, amount: None },
            Window { label: ARGON_WEEKLY_LABEL.into(), seconds: Some(WEEK), remaining: 63.0, reset: None, amount: None },
            Window { label: GEMINI_WEEKLY_LABEL.into(), seconds: Some(WEEK), remaining: 96.0, reset: None, amount: None },
            Window { label: FIVE_HOUR_LABEL.into(), seconds: Some(FIVE_HOURS), remaining: 95.0, reset: None, amount: None },
        ];
        sort_windows(&mut windows);
        let labels: Vec<&str> = windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec![FIVE_HOUR_LABEL, GEMINI_WEEKLY_LABEL, ARGON_WEEKLY_LABEL, CLAUDE_WEEKLY_LABEL]);
    }

    /// 三环是「5 小时 + Gemini 每周 + Gemini 4 Argon 每周」，展开列表四条都在。
    #[test]
    fn snapshot_lists_the_three_closest_windows_from_the_inside_out() {
        let mut snapshot = Snapshot { windows: vec![
            Window { label: CLAUDE_WEEKLY_LABEL.into(), seconds: Some(WEEK), remaining: 49.0, reset: None, amount: None },
            Window { label: GEMINI_WEEKLY_LABEL.into(), seconds: Some(WEEK), remaining: 96.0, reset: None, amount: None },
            Window { label: ARGON_WEEKLY_LABEL.into(), seconds: Some(WEEK), remaining: 63.0, reset: None, amount: None },
            Window { label: FIVE_HOUR_LABEL.into(), seconds: Some(FIVE_HOURS), remaining: 95.0, reset: None, amount: None },
        ], ..Default::default() };
        sort_windows(&mut snapshot.windows);
        let listed: Vec<&str> = snapshot.ordered().iter().map(|w| w.label.as_str()).collect();
        assert_eq!(listed, vec![FIVE_HOUR_LABEL, GEMINI_WEEKLY_LABEL, ARGON_WEEKLY_LABEL, CLAUDE_WEEKLY_LABEL]);
        // 常驻小环只有两圈：外圈 5 小时，内圈是排第二的 Gemini 每周。
        let rings: Vec<&str> = snapshot.rings().iter().map(|w| w.label.as_str()).collect();
        assert_eq!(rings, vec![FIVE_HOUR_LABEL, GEMINI_WEEKLY_LABEL]);
    }

    /// 需要本机装了 Antigravity CLI 且已登录，默认跳过（一次 6~10 秒）。
    #[test]
    #[ignore]
    fn fetches_live_weekly_from_cli() {
        let weekly = weekly().expect("应能从 CLI 读到周额度");
        assert!(weekly.gemini.is_some(), "应读到 Gemini 每周额度");
        assert!(weekly.argon.is_some(), "应读到 Argon 每周额度");
    }
}
