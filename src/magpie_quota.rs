//! magpie 本机额度：只读本机服务的缓存条目，不触发厂商请求。
//! 服务地址固定为回环地址，专用无代理 agent，且不发送任何转发头。
//! 协议来源：https://github.com/Renly1994/magpie/blob/5df7ce8（`GET /v1/magpie/quotas`）。
use super::{relay, Account, Amount, Config, Provider, Snapshot, Window};
use serde_json::Value;
use std::{
    collections::HashSet,
    io::Read,
    path::PathBuf,
    sync::mpsc,
    time::Duration,
};

/// 本机服务地址写死：不开放用户输入，也不经过中转站的 HTTPS 校验。
pub(crate) const BASE: &str = "http://127.0.0.1:3425";
const PATH: &str = "/v1/magpie/quotas?kept=1";
const NOT_RUNNING: &str = "未检测到本机 magpie，请先启动 magpie";
const MAX: u64 = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(5);

fn agent() -> ureq::Agent {
    // 公共 agent 会读取系统代理；本机回环请求必须直连。ureq 不会自行添加转发头。
    ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .redirects(0)
        .timeout(TIMEOUT)
        .timeout_connect(TIMEOUT)
        .build()
}

fn text(v: &Value, key: &str) -> Option<String> {
    v[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.chars().count() <= 256)
        .map(str::to_owned)
}

fn number(v: &Value) -> Option<f64> {
    super::number(v).filter(|n| n.is_finite())
}

fn source_string(a: &Account) -> String {
    a.source.to_string_lossy().into_owned()
}

/// 余额类条目（kind=balance）在界面上使用余额布局，而不是额度环。
pub(crate) fn balance_account(a: &Account) -> bool {
    a.provider == Provider::Magpie
        && source_string(a)
            .splitn(4, '|')
            .nth(2)
            .is_some_and(|kind| kind == "balance")
}

/// 条目标识串 `provider|user|kind|plan`；刷新时据此从列表中找回条目。
fn identity(q: &Value) -> Option<(String, String, String, String)> {
    let provider = text(q, "provider")?;
    let user = text(q, "user")?;
    let kind = text(q, "kind")?;
    if !matches!(kind.as_str(), "subscription" | "plan" | "balance") {
        return None;
    }
    Some((provider, user, kind, text(q, "plan").unwrap_or_default()))
}

fn key(provider: &str, user: &str, kind: &str, plan: &str) -> String {
    format!("{provider}|{user}|{kind}|{plan}")
}

fn account_id(key: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, key.as_bytes());
    format!(
        "Magpie-{}",
        digest.as_ref()[..16].iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}

fn clean(error: &str) -> String {
    error.chars().filter(|c| !c.is_control()).take(160).collect()
}

/// 条目级错误必须让刷新失败，才能保留上次数据并显示陈旧提示。
fn entry_error(q: &Value) -> Option<String> {
    for field in ["error", "readError"] {
        let value = &q[field];
        if value.is_null() || value == &Value::Bool(false) {
            continue;
        }
        let detail = match value {
            Value::String(s) => s.trim().to_owned(),
            Value::Object(o) => o
                .get("message")
                .or_else(|| o.get("error"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned(),
            _ => String::new(),
        };
        return Some(if detail.is_empty() {
            "magpie 未能读取该账号额度".into()
        } else {
            format!("magpie 未能读取该账号额度：{}", clean(&detail))
        });
    }
    None
}

fn read_at(q: &Value) -> i64 {
    text(q, "readAt")
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|at| at.timestamp())
        .filter(|at| *at > 0)
        .unwrap_or_else(|| chrono::Utc::now().timestamp())
}

fn money(raw: &str) -> (Option<f64>, String) {
    let unit = if raw.contains('$') {
        "USD"
    } else if raw.contains('¥') || raw.contains('￥') {
        "CNY"
    } else if raw.contains('€') {
        "EUR"
    } else if raw.contains('£') {
        "GBP"
    } else {
        "余额"
    };
    let digits = raw
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect::<String>();
    let remaining = digits.parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0);
    (remaining, unit.into())
}

fn balance_snapshot(q: &Value, at: i64, plan: &str) -> Result<Snapshot, String> {
    let raw = text(q, "balance").ok_or("magpie 未返回该账号余额")?;
    let (remaining, unit) = money(&raw);
    Ok(Snapshot {
        plan: plan.into(),
        at,
        relay: Some(relay::Quota {
            source: "Magpie".into(),
            amounts: vec![relay::Amount {
                scope: relay::Scope::Account,
                label: "账户余额".into(),
                unit,
                remaining,
                limit: None,
                used: None,
                unlimited: false,
            }],
            expires_at: None,
        }),
        ..Default::default()
    })
}

fn window(w: &Value) -> Result<Option<Window>, String> {
    if w["unlimited"].as_bool() == Some(true) {
        // 不限额无法画成有限进度，也不猜成 100%。
        return Ok(None);
    }
    let Some(remaining) = number(&w["remaining"]) else {
        // 仅文本窗口没有数值，沿用现有 provider 的做法：不伪造百分比。
        return Ok(None);
    };
    if !(0.0..=100.0).contains(&remaining) {
        return Err("magpie 返回的额度百分比无效".into());
    }
    let amount = match (number(&w["limit"]), number(&w["amount"])) {
        (Some(limit), Some(used)) if limit >= 0.0 => Some(Amount::new((limit - used).max(0.0), limit)),
        _ => None,
    };
    Ok(Some(Window {
        label: text(w, "name").unwrap_or_else(|| "额度".into()),
        seconds: None,
        remaining: remaining as f32,
        reset: super::reset(&w["resetsAt"]),
        amount,
    }))
}

fn quota_snapshot(q: &Value, kind: &str, name: &str) -> Result<Snapshot, String> {
    let at = read_at(q);
    let plan = text(q, "plan").unwrap_or_default();
    if kind == "balance" {
        return balance_snapshot(q, at, &plan);
    }
    let mut snapshot = Snapshot { plan, at, ..Default::default() };
    for w in q["windows"].as_array().into_iter().flatten() {
        if let Some(window) = window(w)? {
            snapshot.windows.push(window);
        }
    }
    if snapshot.windows.is_empty() {
        return Err(format!("magpie 未返回 {name} 的可显示额度周期"));
    }
    Ok(snapshot)
}

#[derive(Debug)]
struct Entry {
    key: String,
    alias: String,
    snapshot: Result<Snapshot, String>,
}

fn entries(value: &Value) -> Result<Vec<Entry>, String> {
    if value["object"].as_str() != Some("list") {
        return Err("magpie 返回了未知的额度列表".into());
    }
    let rows = value["data"].as_array().ok_or("magpie 未返回额度条目")?;
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for q in rows {
        let Some((provider, user, kind, plan)) = identity(q) else { continue; };
        let key = key(&provider, &user, &kind, &plan);
        if !seen.insert(key.clone()) {
            continue;
        }
        let name = text(q, "name").unwrap_or_else(|| provider.clone());
        let snapshot = match entry_error(q) {
            Some(error) => Err(error),
            None => quota_snapshot(q, &kind, &name),
        };
        entries.push(Entry {
            key,
            alias: format!("magpie · {name}({user})"),
            snapshot,
        });
    }
    Ok(entries)
}

fn transport(error: ureq::Error) -> (String, u64) {
    match error {
        ureq::Error::Status(403, _) => ("magpie 拒绝了本机请求，请确认没有通过代理访问".into(), 60),
        ureq::Error::Status(code, _) => (format!("magpie 额度服务返回 {code}"), 60),
        _ => (NOT_RUNNING.into(), 30),
    }
}

fn list_with(agent: &ureq::Agent, base: &str) -> Result<Vec<Entry>, (String, u64)> {
    let url = format!("{}{PATH}", base.trim_end_matches('/'));
    let response = agent
        .get(&url)
        .set("Accept", "application/json")
        .call()
        .map_err(transport)?;
    if !(200..300).contains(&response.status()) {
        return Err((format!("magpie 额度服务返回 {}", response.status()), 60));
    }
    let mut raw = Vec::new();
    response
        .into_reader()
        .take(MAX + 1)
        .read_to_end(&mut raw)
        .map_err(|_| ("读取 magpie 额度响应失败".to_string(), 30))?;
    if raw.len() as u64 > MAX {
        return Err(("magpie 额度响应过大".into(), 60));
    }
    let value: Value =
        serde_json::from_slice(&raw).map_err(|_| ("magpie 未返回有效的额度数据".to_string(), 60))?;
    entries(&value).map_err(|error| (error, 60))
}

fn fetch_with(agent: &ureq::Agent, base: &str, source: &str) -> Result<Snapshot, (String, u64)> {
    let entry = list_with(agent, base)?
        .into_iter()
        .find(|entry| entry.key == source)
        .ok_or_else(|| ("magpie 未返回该条目，请重新连接".to_string(), 60))?;
    entry.snapshot.map_err(|error| (error, 60))
}

pub(super) fn fetch(account: &Account) -> Result<Snapshot, (String, u64)> {
    fetch_with(&agent(), BASE, &source_string(account))
}

#[derive(Debug)]
pub(crate) struct Connected {
    entries: Vec<Entry>,
}

pub(crate) struct Connect {
    rx: mpsc::Receiver<Result<Connected, String>>,
    pub target: Option<Account>,
    cancelled: bool,
}

impl Connected {
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

impl Connect {
    pub fn start(target: Option<Account>) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = list_with(&agent(), BASE)
                .map(|entries| Connected { entries })
                .map_err(|error| error.0);
            let _ = tx.send(result);
        });
        Self { rx, target, cancelled: false }
    }
    pub fn poll(&self) -> Option<Result<Connected, String>> {
        self.rx.try_recv().ok()
    }
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }
    pub fn cancelled(&self) -> bool {
        self.cancelled
    }
    #[cfg(test)]
    pub(crate) fn pending(target: Option<Account>) -> (Self, mpsc::Sender<Result<Connected, String>>) {
        let (tx, rx) = mpsc::channel();
        (Self { rx, target, cancelled: false }, tx)
    }
}

pub(crate) struct Applied {
    pub selected: String,
    pub rows: Vec<(String, Result<Snapshot, String>)>,
    pub notes: Vec<String>,
}

/// 重连即重新探测并合并：新增条目补建账号，消失的条目只提示、不自动删除。
pub(crate) fn apply_connection(
    config: &mut Config,
    target: Option<&Account>,
    connected: &Connected,
) -> Result<Applied, String> {
    if connected.entries.is_empty() {
        return Err("本机 magpie 没有返回任何额度条目".into());
    }
    for entry in &connected.entries {
        let source = PathBuf::from(&entry.key);
        if let Some(account) = config
            .accounts
            .iter_mut()
            .find(|a| a.provider == Provider::Magpie && a.source == source)
        {
            account.enabled = true;
        } else {
            let id = account_id(&entry.key);
            let pinned = config.accounts.is_empty();
            if pinned {
                config.selected = id.clone();
            }
            config.accounts.push(Account {
                id,
                provider: Provider::Magpie,
                alias: entry.alias.clone(),
                source,
                slot: None,
                enabled: true,
                pinned,
            });
        }
    }
    let present = connected.entries.iter().map(|entry| entry.key.as_str()).collect::<HashSet<_>>();
    let mut notes = Vec::new();
    for account in config.accounts.iter().filter(|a| a.provider == Provider::Magpie) {
        if !present.contains(source_string(account).as_str()) {
            notes.push(format!("「{}」已从 magpie 消失，账号保留但暂时无法刷新", account.alias));
        }
    }
    let selected = match target {
        Some(account) if present.contains(source_string(account).as_str()) => account.id.clone(),
        _ => config.selected.clone(),
    };
    config.selected = selected.clone();
    let rows = config
        .accounts
        .iter()
        .filter(|a| a.provider == Provider::Magpie)
        .filter_map(|account| {
            let entry = connected.entries.iter().find(|entry| entry.key == source_string(account))?;
            Some((account.id.clone(), entry.snapshot.clone()))
        })
        .collect();
    Ok(Applied { selected, rows, notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn quota(provider: &str, user: &str, kind: &str, name: &str) -> Value {
        json!({"provider":provider,"name":name,"user":user,"plan":"pro","kind":kind,"windows":[]})
    }

    fn server(body: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
            }
            stream
                .write_all(
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
                        .as_bytes(),
                )
                .unwrap();
            let _ = stream.write_all(&body);
            String::from_utf8_lossy(&request).into_owned()
        });
        (base, worker)
    }

    fn list(base: &str) -> Result<Vec<Entry>, (String, u64)> {
        list_with(&ureq::AgentBuilder::new().try_proxy_from_env(false).redirects(0).build(), base)
    }

    #[test]
    fn envelope_and_three_kinds_map_to_snapshots() {
        let value = json!({"object":"list","data":[
            {"provider":"openai","name":"ChatGPT","user":"alice","plan":"Plus","kind":"subscription","readAt":"2030-01-01T00:00:00Z","windows":[
                {"name":"5h","used":20.0,"remaining":80.0,"resetsAt":"2030-01-01T05:00:00Z"},
                {"name":"weekly","used":40,"remaining":60},
                {"name":"bonus","unlimited":true,"remaining":0},
                {"name":"按量","text":"按量计费","display":"按量计费"}
            ]},
            {"provider":"anthropic","name":"Claude","user":"bob","plan":"Max","kind":"plan","windows":[
                {"name":"credits","used":30,"remaining":70,"amount":30,"limit":100}
            ]},
            {"provider":"deepseek","name":"DeepSeek","user":"carol","kind":"balance","balance":"$4.20"}
        ]});
        let entries = entries(&value).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].key, "openai|alice|subscription|Plus");
        assert_eq!(entries[0].alias, "magpie · ChatGPT(alice)");
        let subscription = entries[0].snapshot.as_ref().unwrap();
        assert_eq!(subscription.plan, "Plus");
        assert_eq!(subscription.at, 1893456000);
        assert_eq!(subscription.windows.len(), 2, "不限额与仅文本窗口不伪造百分比");
        assert_eq!(subscription.windows[0].label, "5h");
        assert_eq!(subscription.windows[0].remaining, 80.0);
        assert!(subscription.windows[0].reset.is_some());
        assert!(subscription.windows[1].reset.is_none());
        let plan = entries[1].snapshot.as_ref().unwrap();
        assert_eq!(plan.windows[0].remaining, 70.0);
        assert_eq!(plan.windows[0].amount, Some(Amount::new(70.0, 100.0)));
        let balance = entries[2].snapshot.as_ref().unwrap();
        let relay = balance.relay.as_ref().unwrap();
        assert_eq!(relay.primary().unwrap().remaining, Some(4.2));
        assert_eq!(relay.primary().unwrap().unit, "USD");
    }

    #[test]
    fn malformed_or_unknown_entries_are_skipped_not_invented() {
        let value = json!({"object":"list","data":[
            {"provider":"openai","user":"alice","kind":"mystery","windows":[]},
            {"provider":"","user":"alice","kind":"plan"},
            {"provider":"openai","user":"alice","kind":"plan","windows":[]},
            {"provider":"openai","user":"alice","kind":"plan","windows":[]},
            {"provider":"openai","name":"Broken","user":"bob","kind":"plan","error":"provider said no","windows":[]}
        ]});
        let items = entries(&value).unwrap();
        assert_eq!(items.len(), 2, "未知 kind、缺少身份和重复条目都不建账号");
        assert!(items[0].snapshot.as_ref().unwrap_err().contains("可显示额度"));
        assert!(items[1].snapshot.as_ref().unwrap_err().contains("provider said no"));
        assert_eq!(items[1].alias, "magpie · Broken(bob)");
        assert_eq!(entries(&json!({"object":"usage","data":[]})).unwrap_err(), "magpie 返回了未知的额度列表");
        assert!(entries(&json!({"object":"list"})).is_err());
    }

    #[test]
    fn invalid_percentages_fail_instead_of_becoming_zero() {
        let value = json!({"object":"list","data":[quota("openai","alice","plan","ChatGPT")]});
        let mut value = value;
        value["data"][0]["windows"] = json!([{"name":"5h","remaining":140}]);
        let entries = entries(&value).unwrap();
        assert!(entries[0].snapshot.as_ref().unwrap_err().contains("百分比"));
    }

    #[test]
    fn request_is_local_read_only_and_has_no_forwarding_headers() {
        let body = json!({"object":"list","data":[quota("openai","alice","plan","ChatGPT")]}).to_string();
        let (base, worker) = server(body.into_bytes());
        let _ = list(&base).unwrap();
        let request = worker.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /v1/magpie/quotas?kept=1 http/1.1"), "{request}");
        for forbidden in ["forwarded:", "x-forwarded-for:", "x-real-ip:", "proxy-authorization:"] {
            assert!(!request.contains(forbidden), "本机请求不能带转发头：{forbidden}");
        }
    }

    #[test]
    fn local_request_ignores_environment_proxies() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let keys = ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"];
        let saved = keys.map(|key| (key, std::env::var_os(key)));
        for key in ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy"] {
            std::env::set_var(key, "http://127.0.0.1:9");
        }
        let body = json!({"object":"list","data":[quota("openai","alice","plan","ChatGPT")]}).to_string();
        let (base, worker) = server(body.into_bytes());
        // 使用生产 agent：若读取环境代理，请求会打到已关闭的 127.0.0.1:9 而失败。
        let result = list_with(&agent(), &base);
        let request = worker.join().unwrap();
        for (key, value) in saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        assert!(result.is_ok(), "本机 magpie 请求不得经过代理");
        assert!(request.starts_with("GET /v1/magpie/quotas?kept=1"));
    }

    #[test]
    fn entries_are_selected_by_identity_not_position() {
        let value = json!({"object":"list","data":[
            {"provider":"openai","name":"ChatGPT","user":"alice","plan":"Plus","kind":"subscription","windows":[{"name":"5h","remaining":90}]},
            {"provider":"openai","name":"ChatGPT","user":"bob","plan":"Plus","kind":"subscription","windows":[{"name":"5h","remaining":10}]}
        ]});
        let (base, worker) = server(value.to_string().into_bytes());
        let agent = ureq::AgentBuilder::new().try_proxy_from_env(false).redirects(0).build();
        let snapshot = fetch_with(&agent, &base, "openai|bob|subscription|Plus").unwrap();
        worker.join().unwrap();
        assert_eq!(snapshot.windows[0].remaining, 10.0);
        let (base, worker) = server(value.to_string().into_bytes());
        let error = fetch_with(&agent, &base, "openai|carol|subscription|Plus").unwrap_err();
        worker.join().unwrap();
        assert!(error.0.contains("未返回该条目"));
    }

    #[test]
    fn stopped_service_and_bad_responses_have_actionable_errors() {
        assert_eq!(list("http://127.0.0.1:1").unwrap_err(), (NOT_RUNNING.into(), 30));
        let (base, worker) = server(b"not-json".to_vec());
        assert!(list(&base).unwrap_err().0.contains("有效的额度数据"));
        worker.join().unwrap();
        let (base, worker) = server(vec![b' '; MAX as usize + 1]);
        assert!(list(&base).unwrap_err().0.contains("过大"));
        worker.join().unwrap();
    }

    #[test]
    fn reconnect_adds_new_entries_and_keeps_disappeared_ones() {
        let first = entries(&json!({"object":"list","data":[
            quota("openai","alice","plan","ChatGPT"),
            quota("deepseek","carol","balance","DeepSeek")
        ]})).unwrap();
        let mut config = Config::default();
        let applied = apply_connection(&mut config, None, &Connected { entries: first }).unwrap();
        assert_eq!(config.accounts.len(), 2);
        let alice = config.accounts.iter().find(|a|a.alias.contains("alice")).unwrap().clone();
        assert_eq!(alice.provider, Provider::Magpie);
        assert_eq!(alice.slot, None);
        assert_eq!(alice.source, PathBuf::from("openai|alice|plan|pro"));
        assert!(alice.pinned && config.selected == alice.id);
        assert_eq!(applied.rows.len(), 2);
        assert!(balance_account(&config.accounts.iter().find(|a|a.alias.contains("carol")).unwrap()));

        config.accounts.iter_mut().find(|a|a.id==alice.id).unwrap().alias = "我的主账号".into();
        let saved = alice.id.clone();
        let second = entries(&json!({"object":"list","data":[
            quota("openai","alice","plan","ChatGPT"),
            quota("anthropic","bob","plan","Claude")
        ]})).unwrap();
        let applied = apply_connection(&mut config, Some(&alice), &Connected { entries: second }).unwrap();
        assert_eq!(applied.selected, saved);
        assert_eq!(config.accounts.len(), 3, "消失的条目保留，新增条目补建");
        assert_eq!(config.accounts.iter().find(|a|a.id==saved).unwrap().alias, "我的主账号");
        assert_eq!(config.accounts.iter().find(|a|a.alias.contains("carol")).unwrap().source, PathBuf::from("deepseek|carol|balance|pro"));
        assert_eq!(applied.notes.len(), 1);
        assert!(applied.notes[0].contains("已从 magpie 消失"));
        assert_eq!(config, Config::from_json(&config.json()), "magpie 账号可以保存并读回");
    }

    #[test]
    fn empty_list_does_not_create_or_delete_accounts() {
        let mut config = Config::default();
        assert!(apply_connection(&mut config, None, &Connected { entries: vec![] }).is_err());
        assert!(config.accounts.is_empty());
    }

    #[test]
    fn cached_snapshot_stays_bound_to_provider_and_slot() {
        let account = Account{id:"Magpie-fixture".into(),provider:Provider::Magpie,alias:"magpie".into(),source:PathBuf::from("openai|alice|plan|"),slot:None,enabled:true,pinned:false};
        let cache = json!({&account.id:{"provider":"Magpie","slot":Value::Null,"source":"openai|alice|plan|pro","snapshot":{"plan":"pro","windows":[],"extra":[],"at":1900000000,"relay":{"source":"magpie","amounts":[],"expires_at":null}}}});
        assert!(account.cached_snapshot(&cache).is_some());
        let mut other = account.clone();
        other.provider = Provider::Codex;
        assert!(other.cached_snapshot(&cache).is_none());
    }

    #[test]
    fn connect_polls_and_reports_the_probe_failure() {
        let (mut connect, tx) = Connect::pending(None);
        assert!(connect.poll().is_none());
        tx.send(Err(NOT_RUNNING.into())).unwrap();
        assert_eq!(connect.poll().unwrap().unwrap_err(), NOT_RUNNING);
        assert!(!connect.cancelled());
        connect.cancel();
        assert!(connect.cancelled());
    }
}
