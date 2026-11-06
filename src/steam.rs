//! Steam account binding, Web API and achievement synchronization.
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

const OPENID: &str = "https://steamcommunity.com/openid/login";
const NS: &str = "http://specs.openid.net/auth/2.0";
const BASE_ID: u64 = 76561197960265728;
pub const NOTICE: &str = "Steam 成就已解锁";

#[derive(Clone, PartialEq, Eq)]
pub enum Action {
    Local(String),
    Login(String),
    Cancel,
    Disconnect,
    Refresh,
    WatchGame(Option<u32>),
    Options { notify: bool, fullscreen: bool },
}
impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Local(_) => "SteamLocal([redacted])",
            Self::Login(_) => "SteamLogin([redacted])",
            Self::Cancel => "SteamCancel",
            Self::Disconnect => "SteamDisconnect",
            Self::Refresh => "SteamRefresh",
            Self::WatchGame(_) => "SteamWatchGame",
            Self::Options { .. } => "SteamOptions",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub steamid: String,
    pub notify: bool,
    pub fullscreen: bool,
    pub watch_app: Option<u32>,
}
impl Config {
    pub fn load() -> Option<Self> {
        load_config(&crate::model::config_dir().join("steam.json"))
    }
}
fn load_config(path: &Path) -> Option<Config> {
    let v: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let steamid = v["steamid"].as_str()?.to_owned();
    valid_id(&steamid).then(|| Config {
        steamid,
        notify: v["notify"].as_bool().unwrap_or(true),
        fullscreen: v["fullscreen"].as_bool().unwrap_or(false),
        watch_app: v["watch_app"]
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
            .filter(|id| *id > 0),
    })
}
fn save_config(config: Option<&Config>) -> Result<(), String> {
    let root = crate::model::config_dir();
    std::fs::create_dir_all(&root).map_err(|_| "无法创建 Steam 数据目录")?;
    let v = config
        .map(|c| json!({"steamid":c.steamid,"notify":c.notify,"fullscreen":c.fullscreen,"watch_app":c.watch_app}))
        .unwrap_or(Value::Null);
    crate::model::atomic_write(&root.join("steam.json"), v.to_string().as_bytes())
        .map_err(|_| "无法保存 Steam 设置".into())
}
pub fn valid_id(value: &str) -> bool {
    value.parse::<u64>().ok().is_some_and(|id| {
        id > BASE_ID && id <= BASE_ID + u32::MAX as u64 && id.to_string() == value
    })
}
pub fn valid_key(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn parse_app_id(input: &str) -> Option<u32> {
    let input = input.trim();
    if let Ok(id) = input.parse::<u32>() {
        return (id > 0).then_some(id);
    }
    let url = url::Url::parse(input).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(
            url.host_str(),
            Some("store.steampowered.com" | "steamcommunity.com")
        )
    {
        return None;
    }
    let mut parts = url.path_segments()?;
    if !matches!(parts.next(), Some("app" | "stats")) {
        return None;
    }
    parts.next()?.parse::<u32>().ok().filter(|id| *id > 0)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Game {
    pub id: u32,
    pub name: String,
    pub icon: Option<std::path::PathBuf>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Player {
    pub steamid: String,
    pub name: String,
    pub avatar: Option<String>,
    pub online: u32,
    pub public: bool,
    pub game: Option<Game>,
}
impl Player {
    pub fn status(&self) -> &'static str {
        match self.online {
            0 => "离线或隐身",
            1 => "在线",
            2 => "忙碌",
            3 => "离开",
            4 => "休眠",
            5 => "想交易",
            6 => "想玩游戏",
            _ => "状态未知",
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub player: Option<Player>,
    pub unlocked: usize,
    pub total: usize,
    pub note: String,
    pub at: String,
    pub achievement_game: Option<Game>,
    pub achievements_at: String,
    pub achievement_icons: Vec<String>,
    pub recent_achievements: Vec<Achievement>,
    pub local_status: &'static str,
    pub player_stale: bool,
}

fn tracking_game(config: &Config, player: Option<&Player>, local_app: Option<u32>) -> Option<Game> {
    let playing = player.and_then(|p| p.game.as_ref());
    match config.watch_app.or(local_app) {
        Some(id) => Some(playing.filter(|g| g.id == id).cloned().unwrap_or(Game {
            id,
            name: format!("游戏 {id}"),
            icon: None,
        })),
        None => playing.cloned(),
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Achievement {
    pub api_name: String,
    pub name: String,
    pub description: String,
    pub unlocked: bool,
    pub unlocktime: u64,
    pub icon: Option<String>,
}

fn recent_unlocked(items:&[Achievement])->Vec<Achievement> {
    let mut recent=items.iter().filter(|item|item.unlocked && item.unlocktime>0).cloned().collect::<Vec<_>>();
    recent.sort_by(|a,b|b.unlocktime.cmp(&a.unlocktime).then_with(||a.api_name.cmp(&b.api_name)));
    recent.truncate(4);
    recent
}
#[derive(Clone, Debug)]
pub enum Event {
    LoginUrl(String),
    Connected(Config),
    Snapshot {
        steamid: String,
        snapshot: Snapshot,
    },
    Unlocked {
        steamid: String,
        game: Game,
        achievement: Achievement,
    },
    Error(String),
    Disconnected(Option<String>),
    Cancelled,
}

fn parse_player(v: &Value, id: &str) -> Result<Player, String> {
    let player = v["response"]["players"]
        .as_array()
        .and_then(|a| a.iter().find(|p| p["steamid"].as_str() == Some(id)))
        .ok_or("Steam 没有返回这个账号，请重新连接")?;
    let game = player["gameid"]
        .as_str()
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|id| *id > 0)
        .map(|id| Game {
            id,
            name: player["gameextrainfo"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("游戏 {id}")),
            icon: None,
        });
    Ok(Player {
        steamid: id.into(),
        name: player["personaname"]
            .as_str()
            .unwrap_or("Steam 玩家")
            .chars()
            .take(100)
            .collect(),
        avatar: player["avatarmedium"].as_str().map(str::to_owned),
        online: player["personastate"].as_u64().unwrap_or(0) as u32,
        public: player["communityvisibilitystate"].as_u64() == Some(3),
        game,
    })
}
fn parse_achievements(v: &Value) -> Result<Vec<Achievement>, String> {
    let stats = &v["playerstats"];
    if stats["success"].as_bool() != Some(true) {
        return Err("成就不可读取：请检查 Steam 游戏详情隐私设置，或确认该游戏支持成就".into());
    }
    let items = stats["achievements"]
        .as_array()
        .ok_or("Steam 尚未返回成就列表，该游戏可能没有可用成就")?;
    items
        .iter()
        .map(|a| {
            let api = a["apiname"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 256)
                .ok_or("Steam 成就数据格式异常")?;
            let achieved = a["achieved"]
                .as_u64()
                .filter(|n| *n <= 1)
                .ok_or("Steam 成就状态异常")?;
            Ok(Achievement {
                api_name: api.into(),
                name: a["name"]
                    .as_str()
                    .unwrap_or(api)
                    .chars()
                    .take(160)
                    .collect(),
                description: a["description"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(300)
                    .collect(),
                unlocked: achieved == 1,
                unlocktime: a["unlocktime"].as_u64().unwrap_or(0),
                icon: None,
            })
        })
        .collect()
}
fn steam_agent() -> Result<ureq::Agent, String> {
    let builder = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout(Duration::from_secs(12))
        .redirects(0);
    // Match the browser's Windows trust store. Keep normal certificate and hostname validation;
    // a Steam accelerator may terminate TLS locally using a CA installed by the user.
    #[cfg(windows)]
    let builder = builder.tls_connector(Arc::new(
        native_tls::TlsConnector::new()
            .map_err(|_| "无法初始化 Windows 安全连接，请重启岛仔后重试")?,
    ));
    Ok(builder.build())
}

fn network_reason(error: &ureq::Error) -> &'static str {
    if let ureq::Error::Transport(transport) = error {
        if transport.kind() == ureq::ErrorKind::Dns {
            return "DNS 解析失败";
        }
        let mut source = std::error::Error::source(transport);
        while let Some(cause) = source {
            if cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::TimedOut)
            {
                return "连接超时";
            }
            // Classify internally; never expose transport errors, URLs, signatures or API keys.
            let description = cause.to_string().to_lowercase();
            if description.contains("certificate")
                || description.contains("证书")
                || description.contains("tls")
                || description.contains("ssl")
                || description.contains("schannel")
            {
                return "TLS／证书连接失败";
            }
            source = cause.source();
        }
    }
    "网络连接失败"
}

fn openid_request_error(error: &ureq::Error) -> String {
    match error {
        ureq::Error::Status(status, _) => {
            format!("Steam 登录核验返回 HTTP {status}，请检查 Steam 加速／代理设置后重试")
        }
        _ => format!(
            "Steam 登录核验失败（{}），请检查 Steam 加速／代理设置后重试",
            network_reason(error)
        ),
    }
}

fn parse_verification(body: &str) -> Result<bool, String> {
    let mut response = BTreeMap::new();
    for line in body.lines().filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or("Steam 登录核验响应格式无效")?;
        if response.insert(name, value).is_some() {
            return Err("Steam 登录核验响应包含重复字段".into());
        }
    }
    if response.get("ns") != Some(&NS) {
        return Err("Steam 登录核验响应类型无效".into());
    }
    match response.get("is_valid") {
        Some(&"true") => Ok(true),
        Some(&"false") => Ok(false),
        _ => Err("Steam 登录核验响应缺少有效结果".into()),
    }
}

struct Api {
    agent: Result<ureq::Agent, String>,
}
impl Api {
    fn new() -> Self {
        Self {
            agent: steam_agent(),
        }
    }
    fn get(&self, method: &str, key: &str, params: &[(&str, &str)]) -> Result<Value, String> {
        let mut req = self
            .agent
            .as_ref()
            .map_err(Clone::clone)?
            .get(&format!("https://api.steampowered.com/{method}/"))
            .query("key", key);
        for (k, v) in params {
            req = req.query(k, v);
        }
        let response = req.call().map_err(|e| match e {
            ureq::Error::Status(400, _) if method.starts_with("ISteamUserStats/") => {
                "这个游戏暂时无法读取成就，请确认它支持 Steam 成就并已上传游戏数据".to_string()
            }
            ureq::Error::Status(400, _) => "Steam 请求参数无效，请重新连接账号".to_string(),
            ureq::Error::Status(401 | 403, _) => {
                "Steam 拒绝访问，请检查 API Key 和资料隐私设置".to_string()
            }
            ureq::Error::Status(429, _) => "Steam 请求过于频繁，正在延长刷新间隔".to_string(),
            _ => format!(
                "Steam 连接失败（{}），稍后自动重试；请检查网络或加速设置",
                network_reason(&e)
            ),
        })?;
        // Never include ureq errors or request URLs in messages: the query contains a secret.
        serde_json::from_reader(response.into_reader().take(2 * 1024 * 1024))
            .map_err(|_| "Steam 返回的数据无法读取".into())
    }
    fn player(&self, key: &str, id: &str) -> Result<Player, String> {
        parse_player(
            &self.get("ISteamUser/GetPlayerSummaries/v2", key, &[("steamids", id)])?,
            id,
        )
    }
    fn achievements(
        &self,
        key: &str,
        id: &str,
        game: u32,
    ) -> Result<(Option<String>, Vec<Achievement>), String> {
        let value = self.get(
            "ISteamUserStats/GetPlayerAchievements/v1",
            key,
            &[
                ("steamid", id),
                ("appid", &game.to_string()),
                ("l", "schinese"),
            ],
        )?;
        let title = value["playerstats"]["gameName"]
            .as_str()
            .filter(|name| !name.is_empty())
            .map(|name| name.chars().take(160).collect());
        Ok((title, parse_achievements(&value)?))
    }
}

/// Keep last reported unlock states/timestamps; switching targets or restarting seeds silently.
#[derive(Default)]
struct Tracker {
    seen: BTreeMap<String, BTreeMap<String, u64>>,
    active: Option<String>,
    local: BTreeMap<String, u64>,
    local_relocked: BTreeMap<String, u64>,
    local_sampled: bool,
    online_sampled: bool,
    icons: BTreeMap<String, String>,
}
impl Tracker {
    fn load(path: &Path) -> Self {
        let seen = std::fs::read(path)
            .ok()
            .filter(|v| v.len() < 2 * 1024 * 1024)
            .and_then(|b| {
                let value: Value = serde_json::from_slice(&b).ok()?;
                if value["version"].as_u64() == Some(2) {
                    serde_json::from_value(value["games"].clone()).ok()
                } else {
                    serde_json::from_value::<BTreeMap<String, BTreeSet<String>>>(value)
                        .ok()
                        .map(|games| {
                            games
                                .into_iter()
                                .map(|(game, names)| {
                                    (game, names.into_iter().map(|name| (name, 0)).collect())
                                })
                                .collect()
                        })
                }
            })
            .unwrap_or_default();
        Self {
            seen,
            ..Self::default()
        }
    }

    fn observe(&mut self, id: &str, game: u32, items: &[Achievement]) -> Vec<Achievement> {
        self.observe_source(id, game, items, false)
    }

    fn observe_source(
        &mut self,
        id: &str,
        game: u32,
        items: &[Achievement],
        local: bool,
    ) -> Vec<Achievement> {
        let key = format!("{id}:{game}");
        if self.active.as_ref() != Some(&key) {
            self.active = Some(key.clone());
            self.local.clear();
            self.local_relocked.clear();
            self.icons.clear();
            self.local_sampled = false;
            self.online_sampled = false;
        }
        let sampled = if local {
            &mut self.local_sampled
        } else {
            &mut self.online_sampled
        };
        let silent = !std::mem::replace(sampled, true);
        if !self.seen.contains_key(&key) && self.seen.len() >= 32 {
            if let Some(old) = self.seen.keys().find(|k| **k != key).cloned() {
                self.seen.remove(&old);
            }
        }
        let seen = self.seen.entry(key).or_default();
        let mut new = Vec::new();
        for a in items {
            if local {
                let time = if a.unlocked { a.unlocktime } else { 0 };
                let old = self.local.insert(a.api_name.clone(), time).unwrap_or(0);
                if !a.unlocked {
                    if old > 0 {
                        self.local_relocked.insert(a.api_name.clone(), old);
                        if seen.get(&a.api_name).is_some_and(|time| *time <= old) {
                            seen.remove(&a.api_name);
                        }
                    }
                    continue;
                }
                if old == a.unlocktime && !silent {
                    continue;
                }
            } else {
                let cached = self.local.get(&a.api_name).copied().unwrap_or(0);
                // Steam's Web API can lag behind a local unlock or still report a local relock.
                if (!a.unlocked && cached > 0)
                    || (a.unlocked
                        && self
                            .local_relocked
                            .get(&a.api_name)
                            .is_some_and(|time| a.unlocktime <= *time))
                {
                    continue;
                }
            }
            if !a.unlocked {
                seen.remove(&a.api_name);
                continue;
            }
            let previous = seen.get(&a.api_name).copied();
            let changed = previous.map_or(true, |time| time > 0 && a.unlocktime > time);
            seen.insert(a.api_name.clone(), previous.unwrap_or(0).max(a.unlocktime));
            if changed && !silent {
                new.push(a.clone());
            }
        }
        new.sort_by_key(|a| a.unlocktime);
        new
    }

    fn unlocked(&self) -> usize {
        self.active
            .as_ref()
            .and_then(|key| self.seen.get(key))
            .map_or(0, BTreeMap::len)
    }
    fn save(&self, path: &Path) -> Result<(), String> {
        crate::model::atomic_write(
            path,
            serde_json::to_string(&json!({"version":2,"games":self.seen}))
                .map_err(|_| "无法编码成就缓存")?
                .as_bytes(),
        )
        .map_err(|_| "无法保存成就去重记录，请检查数据目录权限".into())
    }
}
fn interval(failures: u32, gaming: bool) -> Duration {
    Duration::from_secs(if failures == 0 {
        if gaming {
            30
        } else {
            60
        }
    } else {
        (30u64.saturating_mul(1u64 << failures.min(5))).min(900)
    })
}

#[cfg(windows)]
mod secrets {
    fn entry() -> Result<keyring::Entry, String> {
        let dir = crate::model::config_dir().to_string_lossy().to_lowercase();
        let digest = ring::digest::digest(&ring::digest::SHA256, dir.as_bytes());
        let scope = digest
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        keyring::Entry::new("Daozai.Steam", &scope).map_err(|_| "Windows 凭据管理器不可用".into())
    }
    pub fn save(key: &str) -> Result<(), String> {
        entry()?
            .set_password(key)
            .map_err(|_| "无法把 API Key 保存到 Windows 凭据管理器".into())
    }
    pub fn load() -> Result<String, String> {
        entry()?
            .get_password()
            .map_err(|_| "没有保存 Steam API Key，请重新连接".into())
    }
    pub fn delete() -> Result<(), String> {
        match entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err("无法删除 Steam API Key，请重试清除本地连接".into()),
        }
    }
}
#[cfg(not(windows))]
mod secrets {
    pub fn save(_: &str) -> Result<(), String> {
        Err("Steam 凭据存储目前仅支持 Windows".into())
    }
    pub fn load() -> Result<String, String> {
        Err("Steam 凭据存储目前仅支持 Windows".into())
    }
    pub fn delete() -> Result<(), String> {
        Ok(())
    }
}
#[cfg(windows)]
fn steam_dword(key: windows::core::PCWSTR, value: windows::core::PCWSTR) -> Option<u32> {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let mut number = 0u32;
    let mut size = 4;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some((&mut number as *mut u32).cast()),
            Some(&mut size),
        )
    };
    (status.is_ok() && number != 0).then_some(number)
}

#[cfg(windows)]
fn active_account() -> Option<u32> {
    use windows::core::w;
    steam_dword(
        w!("Software\\Valve\\Steam\\ActiveProcess"),
        w!("ActiveUser"),
    )
}

#[cfg(windows)]
fn local_game(account: u32) -> Option<u32> {
    use windows::core::w;
    (active_account() == Some(account))
        .then(|| steam_dword(w!("Software\\Valve\\Steam"), w!("RunningAppID")))
        .flatten()
}

#[cfg(not(windows))]
fn local_game(_: u32) -> Option<u32> {
    None
}

#[cfg(windows)]
fn local_id() -> Result<String, String> {
    active_account()
        .map(|account| (BASE_ID + u64::from(account)).to_string())
        .ok_or_else(|| "未检测到本机已登录账号。请先启动并登录 Steam，或使用浏览器登录".into())
}
#[cfg(not(windows))]
fn local_id() -> Result<String, String> {
    Err("本机 Steam 检测仅支持 Windows，请使用浏览器登录".into())
}

fn callback_fields(url: &url::Url, expected: &str) -> Result<BTreeMap<String, String>, String> {
    let mut fields = BTreeMap::new();
    for (k, v) in url.query_pairs() {
        if fields.insert(k.into_owned(), v.into_owned()).is_some() {
            return Err("Steam 登录回调参数重复".into());
        }
    }
    if fields.get("openid.mode").map(String::as_str) == Some("cancel") {
        return Err("已取消 Steam 登录".into());
    }
    let get = |name: &str| fields.get(name).map(String::as_str).unwrap_or("");
    if get("openid.ns") != NS
        || get("openid.mode") != "id_res"
        || get("openid.op_endpoint") != OPENID
        || get("openid.return_to") != expected
    {
        return Err("Steam 登录回调校验失败，请重新登录".into());
    }
    let claimed = get("openid.claimed_id");
    if claimed != get("openid.identity")
        || !claimed
            .strip_prefix("https://steamcommunity.com/openid/id/")
            .is_some_and(valid_id)
    {
        return Err("Steam 登录身份无效".into());
    }
    let signed: BTreeSet<_> = get("openid.signed").split(',').collect();
    if ![
        "op_endpoint",
        "claimed_id",
        "identity",
        "return_to",
        "response_nonce",
        "assoc_handle",
    ]
    .iter()
    .all(|k| signed.contains(k))
    {
        return Err("Steam 登录签名字段不完整".into());
    }
    let nonce = get("openid.response_nonce");
    let time = nonce
        .get(..20)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .ok_or("Steam 登录时间无效")?;
    if (chrono::Utc::now().timestamp() - time.timestamp()).abs() > 300 {
        return Err("Steam 登录已过期，请重试".into());
    }
    Ok(fields)
}
fn login(generation: u64, epoch: &AtomicU64, emit: &impl Fn(Event)) -> Result<String, String> {
    use ring::rand::{SecureRandom, SystemRandom};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|_| "无法创建 Steam 本机登录回调，请检查防火墙")?;
    listener
        .set_nonblocking(true)
        .map_err(|_| "无法启动 Steam 登录回调")?;
    let mut random = [0u8; 32];
    SystemRandom::new()
        .fill(&mut random)
        .map_err(|_| "无法安全地启动 Steam 登录")?;
    let token = random
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let authority = listener
        .local_addr()
        .map_err(|_| "无法读取登录回调端口")?
        .to_string();
    let expected = format!("http://{authority}/steam/callback/{token}");
    let mut target = url::Url::parse(OPENID).unwrap();
    target.query_pairs_mut().extend_pairs([
        ("openid.ns", NS),
        ("openid.mode", "checkid_setup"),
        ("openid.return_to", &expected),
        ("openid.realm", &format!("http://{authority}/")),
        (
            "openid.identity",
            "http://specs.openid.net/auth/2.0/identifier_select",
        ),
        (
            "openid.claimed_id",
            "http://specs.openid.net/auth/2.0/identifier_select",
        ),
    ]);
    emit(Event::LoginUrl(target.into()));
    let until = Instant::now() + Duration::from_secs(180);
    while Instant::now() < until && epoch.load(Ordering::SeqCst) == generation {
        let (mut stream, _) = match listener.accept() {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(30));
                continue;
            }
            Err(_) => return Err("无法接收 Steam 登录结果".into()),
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
        let mut request = Vec::new();
        let mut buf = [0u8; 1024];
        while request.len() < 16384
            && Instant::now() < until
            && epoch.load(Ordering::SeqCst) == generation
            && !request.windows(4).any(|w| w == b"\r\n\r\n")
        {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => request.extend_from_slice(&buf[..n]),
            }
        }
        let raw = String::from_utf8_lossy(&request);
        let line = raw.lines().next().unwrap_or("");
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let path = parts.next().unwrap_or("");
        let host_ok = raw.lines().skip(1).any(|s| {
            s.split_once(':')
                .is_some_and(|(k, v)| k.eq_ignore_ascii_case("host") && v.trim() == authority)
        });
        let parsed = url::Url::parse(&format!("http://{authority}{path}"));
        if method != "GET"
            || !host_ok
            || parsed.as_ref().is_err()
            || parsed
                .as_ref()
                .is_ok_and(|u| u.path() != format!("/steam/callback/{token}"))
        {
            let _ = stream.write_all(
                b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            continue;
        }
        let fields = callback_fields(&parsed.unwrap(), &expected);
        let verified = fields.and_then(|mut fields| {
            fields.insert("openid.mode".into(), "check_authentication".into());
            let form = fields
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect::<Vec<_>>();
            let response = steam_agent()?
                .post(OPENID)
                .send_form(&form)
                .map_err(|error| openid_request_error(&error))?;
            let mut body = String::new();
            response
                .into_reader()
                .take(16384)
                .read_to_string(&mut body)
                .map_err(|_| "Steam 登录核验响应无效")?;
            if !parse_verification(&body)? {
                return Err("Steam 未认可这次登录，请重试".into());
            }
            Ok(fields["openid.claimed_id"]
                .rsplit('/')
                .next()
                .unwrap()
                .to_owned())
        });
        let message = if verified.is_ok() {
            "Steam 账号已验证，请返回岛仔查看连接结果。"
        } else {
            "Steam 登录没有完成，请返回岛仔重试。"
        };
        let body=format!("<!doctype html><html lang=\"zh-CN\"><meta charset=\"utf-8\"><title>岛仔 · Steam</title><p>{message}</p></html>");
        let _=write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Security-Policy: default-src 'none'\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
        return verified;
    }
    Err(if epoch.load(Ordering::SeqCst) != generation {
        "已取消 Steam 登录"
    } else {
        "Steam 登录超时，请重试"
    }
    .into())
}

pub struct Service {
    tx: mpsc::Sender<(u64, Action)>,
    rx: mpsc::Receiver<(u64, Event)>,
    epoch: Arc<AtomicU64>,
}
impl Service {
    pub fn start(ctx: eframe::egui::Context) -> Self {
        let (tx, requests) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(0));
        let cancel = epoch.clone();
        std::thread::spawn(move || worker(requests, events, cancel, ctx));
        Self { tx, rx, epoch }
    }
    pub fn send(&self, action: Action) {
        let generation = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.tx.send((generation, action));
    }
    pub fn try_recv(&self) -> Option<Event> {
        while let Ok((generation, event)) = self.rx.try_recv() {
            if generation == self.epoch.load(Ordering::SeqCst) {
                return Some(event);
            }
        }
        None
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }
}

type AchievementSample = Result<(Option<String>, Vec<Achievement>), String>;

struct OnlinePoll {
    generation: u64,
    player: Option<Result<Player, String>>,
    achievements: Option<(u32, AchievementSample)>,
}

fn worker(
    requests: mpsc::Receiver<(u64, Action)>,
    events: mpsc::Sender<(u64, Event)>,
    epoch: Arc<AtomicU64>,
    ctx: eframe::egui::Context,
) {
    let api = Arc::new(Api::new());
    let root = crate::model::config_dir();
    let cache = root.join("steam-achievements.json");
    let mut tracker = Tracker::load(&cache);
    let mut config = Config::load();
    let mut key = secrets::load().unwrap_or_default();
    let mut generation = 0;
    let mut next = Instant::now();
    let mut failures = 0u32;
    let mut online = None::<mpsc::Receiver<OnlinePoll>>;
    let mut local = crate::steam_cache::LocalCache::default();
    let mut local_due = Instant::now();
    let mut local_app = None;
    let mut snapshot = Snapshot::default();
    let mut last_snapshot = None;
    let mut player_note = String::new();
    let mut achievement_game = None;
    let mut achievement_due = Instant::now();
    let mut achievement_failures = 0u32;
    let mut achievement_note = String::new();
    let mut achievement_title = None;
    loop {
        let emit = |generation, event| {
            if epoch.load(Ordering::SeqCst) == generation {
                let _ = events.send((generation, event));
                ctx.request_repaint();
            }
        };
        match requests.recv_timeout(Duration::from_secs(1)) {
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok((g, action)) => {
                generation = g;
                if epoch.load(Ordering::SeqCst) != g {
                    continue;
                }
                let browser = matches!(&action, Action::Login(_));
                match action {
                    Action::Cancel => {
                        if let Some(c) = &config {
                            emit(g, Event::Connected(c.clone()));
                        }
                        emit(g, Event::Cancelled);
                        next = Instant::now() + Duration::from_secs(30);
                    }
                    Action::Disconnect => {
                        // Persist disabled state before deleting credentials; a failed deletion never restarts polling.
                        match save_config(None) {
                            Err(error) => emit(g, Event::Error(error)),
                            Ok(()) => {
                                config = None;
                                key.clear();
                                tracker = Tracker::default();
                                snapshot = Snapshot::default();
                                last_snapshot = None;
                                achievement_game = None;
                                local.reset_game();
                                let error =
                                    secrets::delete()
                                        .err()
                                        .or_else(|| {
                                            match std::fs::remove_file(&cache) {
                                        Ok(()) => None,
                                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                                        Err(_) => Some(
                                            "连接已停止，但成就缓存删除失败，请重试清除本地连接"
                                                .into(),
                                        ),
                                    }
                                        });
                                emit(g, Event::Disconnected(error));
                            }
                        }
                    }
                    Action::Options { notify, fullscreen } => {
                        if let Some(c) = &config {
                            let mut updated = c.clone();
                            updated.notify = notify;
                            updated.fullscreen = fullscreen;
                            match save_config(Some(&updated)) {
                                Ok(()) => {
                                    config = Some(updated.clone());
                                    emit(g, Event::Connected(updated));
                                }
                                Err(e) => emit(g, Event::Error(e)),
                            }
                        }
                    }
                    Action::Refresh => {
                        next = Instant::now();
                        achievement_due = next;
                        local_due = next;
                        last_snapshot = None;
                    }
                    Action::WatchGame(app) => {
                        if let Some(c) = &config {
                            if app == Some(0) {
                                emit(g, Event::Error("游戏 AppID 必须大于 0".into()));
                                continue;
                            }
                            let mut updated = c.clone();
                            updated.watch_app = app;
                            match save_config(Some(&updated)) {
                                Ok(()) => {
                                    config = Some(updated.clone());
                                    next = Instant::now();
                                    local_due = next;
                                    last_snapshot = None;
                                    emit(g, Event::Connected(updated));
                                }
                                Err(error) => emit(g, Event::Error(error)),
                            }
                        }
                    }
                    Action::Local(input) | Action::Login(input) => {
                        let supplied = input.trim();
                        if !valid_key(supplied) {
                            emit(
                                g,
                                Event::Error(
                                    "API Key 应为 32 位十六进制字符，请从 Steam 官方页面复制"
                                        .into(),
                                ),
                            );
                            continue;
                        }
                        let result = (|| {
                            let id = if browser {
                                login(g, &epoch, &|e| emit(g, e))?
                            } else {
                                local_id()?
                            };
                            if epoch.load(Ordering::SeqCst) != g {
                                return Err("已取消 Steam 连接".into());
                            }
                            let player = api.player(supplied, &id)?;
                            if epoch.load(Ordering::SeqCst) != g {
                                return Err("已取消 Steam 连接".into());
                            }
                            let updated = Config {
                                steamid: id,
                                notify: config.as_ref().map_or(true, |c| c.notify),
                                fullscreen: config.as_ref().is_some_and(|c| c.fullscreen),
                                watch_app: config.as_ref().and_then(|c| c.watch_app),
                            };
                            let old = secrets::load().ok();
                            secrets::save(supplied)?;
                            if let Err(e) = save_config(Some(&updated)) {
                                if let Some(old) = old {
                                    let _ = secrets::save(&old);
                                } else {
                                    let _ = secrets::delete();
                                }
                                return Err(e);
                            }
                            key = supplied.into();
                            config = Some(updated.clone());
                            tracker.active = None;
                            achievement_game = None;
                            local.reset_game();
                            failures = 0;
                            next = Instant::now();
                            local_due = next;
                            player_note.clear();
                            last_snapshot = None;
                            snapshot = Snapshot {
                                player: Some(player),
                                ..Snapshot::default()
                            };
                            emit(g, Event::Connected(updated));
                            Ok(())
                        })();
                        if let Err(e) = result {
                            emit(g, Event::Error(e));
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if epoch.load(Ordering::SeqCst) != generation {
            continue;
        }
        let mut online_sample = None;
        if let Some(receiver) = &online {
            match receiver.try_recv() {
                Ok(sample) => {
                    online = None;
                    if sample.generation == generation {
                        online_sample = Some(sample);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => online = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let Some(c) = &config else { continue };
        let now = Instant::now();
        let local_tick = now >= local_due;
        let account = c
            .steamid
            .parse::<u64>()
            .ok()
            .and_then(|id| id.checked_sub(BASE_ID))
            .and_then(|id| u32::try_from(id).ok())
            .filter(|id| *id != 0);
        if local_tick {
            local_app = account.and_then(local_game);
            local_due = now + Duration::from_secs(1);
        }
        let mut sample = online_sample;
        let player_result = sample.as_mut().and_then(|s| s.player.take());
        let player_observed = player_result.is_some();
        if let Some(result) = player_result {
            match result {
                Ok(player) => {
                    snapshot.player = Some(player);
                    snapshot.player_stale = false;
                    snapshot.at = chrono::Local::now().format("%H:%M:%S").to_string();
                    player_note.clear();
                    failures = 0;
                }
                Err(error) => {
                    snapshot.player_stale = true;
                    player_note = error;
                    failures = failures.saturating_add(1);
                }
            }
        }
        if let Some(game) = snapshot.player.as_mut().and_then(|player| player.game.as_mut()) {
            game.icon = local.game_icon(game.id);
        }
        let mut watched = tracking_game(c, snapshot.player.as_ref(), local_app);
        if player_observed {
            next = now + interval(failures, watched.is_some());
        }
        let watched_id = watched.as_ref().map(|game| game.id);
        let target_changed = achievement_game != watched_id;
        if target_changed {
            achievement_game = watched_id;
            achievement_due = now;
            achievement_failures = 0;
            achievement_note.clear();
            achievement_title = None;
            snapshot.achievements_at.clear();
            snapshot.achievement_icons.clear();
            snapshot.recent_achievements.clear();
            snapshot.local_status = "";
            snapshot.total = 0;
            tracker.active = None;
            local.reset_game();
        }
        let mut notices = Vec::new();
        let mut observed = false;
        if let Some(game) = &mut watched {
            if game.icon.is_none() {
                game.icon = local.game_icon(game.id);
            }
            if let Some(account) = account.filter(|_| local_tick || target_changed) {
                let (status, items) = local.poll(account, game.id);
                snapshot.local_status = status;
                if let Some(mut items) = items {
                    for item in &mut items {
                        if let Some(icon) = &item.icon {
                            tracker.icons.insert(item.api_name.clone(), icon.clone());
                        } else {
                            item.icon
                                .clone_from(&tracker.icons.get(&item.api_name).cloned());
                        }
                    }
                    snapshot.total = snapshot.total.max(items.len());
                    snapshot.recent_achievements=recent_unlocked(&items);
                    snapshot.achievement_icons=items.iter().filter_map(|item|item.icon.clone()).collect();
                    snapshot.achievement_icons.sort_unstable();
                    snapshot.achievement_icons.dedup();
                    notices.extend(tracker.observe_source(&c.steamid, game.id, &items, true));
                    // observe_source seeds a new game by clearing transient metadata.
                    // Restore its icons for subsequent online samples, which omit image URLs.
                    for item in &items {
                        if let Some(icon) = &item.icon {
                            tracker.icons.insert(item.api_name.clone(), icon.clone());
                        }
                    }
                    observed = true;
                }
            }
            if let Some((_, result)) = sample
                .and_then(|s| s.achievements)
                .filter(|(id, _)| *id == game.id)
            {
                match result {
                    Ok((title, mut items)) => {
                        for item in &mut items {
                            if item.icon.is_none() {
                                item.icon
                                    .clone_from(&tracker.icons.get(&item.api_name).cloned());
                            }
                        }
                        snapshot.total = snapshot.total.max(items.len());
                        snapshot.recent_achievements=recent_unlocked(&items);
                        notices.extend(tracker.observe(&c.steamid, game.id, &items));
                        if title.is_some() {
                            achievement_title = title;
                        }
                        achievement_note.clear();
                        achievement_failures = 0;
                        observed = true;
                    }
                    Err(error) => {
                        achievement_note = error;
                        achievement_failures = achievement_failures.saturating_add(1);
                    }
                }
                achievement_due = now + interval(achievement_failures, true);
            }
            if let Some(title) = &achievement_title {
                game.name.clone_from(title);
            }
        }
        if observed {
            snapshot.achievements_at = chrono::Local::now().format("%H:%M:%S").to_string();
            if let Err(error) = tracker.save(&cache) {
                achievement_note = error;
            }
        }
        snapshot.unlocked = tracker.unlocked().min(snapshot.total);
        snapshot.achievement_game.clone_from(&watched);
        snapshot.note = if key.is_empty() {
            "没有保存 Steam API Key，请重新连接以恢复在线同步".into()
        } else if !player_note.is_empty() {
            player_note.clone()
        } else if watched.is_some() {
            achievement_note.clone()
        } else if let Some(player) = &snapshot.player {
            if player.public {
                "没有检测到正在玩的游戏；可在下方指定游戏跟踪成就".into()
            } else {
                "资料或游戏详情未公开，可能无法获取当前游戏与成就".into()
            }
        } else {
            String::new()
        };
        if last_snapshot.as_ref() != Some(&snapshot) {
            emit(
                generation,
                Event::Snapshot {
                    steamid: c.steamid.clone(),
                    snapshot: snapshot.clone(),
                },
            );
            last_snapshot = Some(snapshot.clone());
        }
        if let Some(game) = watched.filter(|_| c.notify) {
            notices.sort_by_key(|achievement| achievement.unlocktime);
            for achievement in notices {
                emit(
                    generation,
                    Event::Unlocked {
                        steamid: c.steamid.clone(),
                        game: game.clone(),
                        achievement,
                    },
                );
            }
        }
        let player_due = now >= next;
        let game_due = watched_id.filter(|_| now >= achievement_due);
        if online.is_none() && !key.is_empty() && (player_due || game_due.is_some()) {
            // Slow API requests must not block the local cache check or delay its notifications.
            let (tx, rx) = mpsc::channel();
            let (api, key, steamid) = (Arc::clone(&api), key.clone(), c.steamid.clone());
            online = Some(rx);
            std::thread::spawn(move || {
                let player = player_due.then(|| api.player(&key, &steamid));
                let achievements = game_due.map(|id| (id, api.achievements(&key, &steamid, id)));
                let _ = tx.send(OnlinePoll {
                    generation,
                    player,
                    achievements,
                });
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn openid_verification_response_is_strict_and_cannot_accept_an_error_page() {
        assert_eq!(
            parse_verification(&format!("ns:{NS}\nis_valid:true\n")).unwrap(),
            true
        );
        assert_eq!(
            parse_verification(&format!("ns:{NS}\nis_valid:false\n")).unwrap(),
            false
        );
        for body in [
            "is_valid:true",
            "<html>is_valid:true</html>",
            "ns:wrong\nis_valid:true",
            "ns:x\nis_valid:true\nis_valid:false",
        ] {
            assert!(parse_verification(body).is_err());
        }
    }
    #[test]
    fn openid_network_errors_expose_status_without_request_secrets() {
        let secret = "sensitive-signature-and-account";
        let response = ureq::Response::new(403, "Forbidden", secret).unwrap();
        let message = openid_request_error(&ureq::Error::Status(403, response));
        assert!(message.contains("403"));
        assert!(!message.contains(secret));
        let invalid = ureq::get(&format!("invalid-url-{secret}"))
            .call()
            .unwrap_err();
        assert!(!openid_request_error(&invalid).contains(secret));
    }
    #[test]
    #[ignore = "只探测 Steam 官方核验端点；不包含账号、签名或 API Key，按需联网执行"]
    fn steam_system_trust_reaches_the_openid_verifier() {
        let response = steam_agent()
            .unwrap()
            .post(OPENID)
            .send_form(&[("openid.ns", NS), ("openid.mode", "check_authentication")])
            .unwrap_or_else(|e| panic!("{}", openid_request_error(&e)));
        assert_eq!(response.status(), 200);
        let mut body = String::new();
        response
            .into_reader()
            .take(16384)
            .read_to_string(&mut body)
            .unwrap();
        assert!(
            !parse_verification(&body).unwrap(),
            "无签名的探测请求不能通过身份验证"
        );
    }
    const ID: &str = "76561198000000001";
    fn a(name: &str, unlocked: bool) -> Achievement {
        Achievement {
            api_name: name.into(),
            name: name.into(),
            description: String::new(),
            unlocked,
            unlocktime: 123,
            icon: None,
        }
    }
    #[test]
    fn steam_ids_are_canonical_individual_accounts() {
        assert!(valid_id(ID));
        for value in [
            "0",
            "76561197960265728",
            "076561198000000001",
            "76561197960265728x",
            "999999999999999999",
        ] {
            assert!(!valid_id(value));
        }
    }
    #[test]
    fn keys_validate_without_printing_secrets() {
        let key = "abcdef0123456789abcdef0123456789";
        assert!(valid_key(key));
        assert!(!valid_key("bad"));
        assert!(!format!("{:?}", Action::Login(key.into())).contains(key));
    }
    #[test]
    fn player_identity_must_match_and_game_id_cannot_overflow() {
        let v = json!({"response":{"players":[{"steamid":ID,"gameid":"570","gameextrainfo":"Dota 2","personastate":1,"communityvisibilitystate":3}]}});
        let p = parse_player(&v, ID).unwrap();
        assert_eq!(p.game.unwrap().id, 570);
        assert!(parse_player(&v, "76561198000000002").is_err());
    }
    #[test]
    fn unavailable_achievements_are_not_reported_as_zero() {
        assert!(parse_achievements(&json!({"playerstats":{"success":false}})).is_err());
        assert!(parse_achievements(&json!({"playerstats":{"success":true}})).is_err());
        assert!(
            parse_achievements(&json!({"playerstats":{"success":true,"achievements":[]}}))
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn baseline_is_silent_new_unlocks_are_ordered_and_deduped() {
        let mut t = Tracker::default();
        assert!(t
            .observe(ID, 570, &[a("old", true), a("new", false)])
            .is_empty());
        assert_eq!(
            t.observe(ID, 570, &[a("old", true), a("new", true)]).len(),
            1
        );
        assert!(t.observe(ID, 570, &[a("new", true)]).is_empty());
        assert!(t.observe(ID, 570, &[a("new", false)]).is_empty());
        assert_eq!(
            t.observe(ID, 570, &[a("new", true)]).len(),
            1,
            "已观察到反解锁后，再次解锁应重新提醒"
        );
    }
    #[test]
    fn restart_and_game_or_account_switch_seed_silently() {
        let mut t = Tracker::default();
        t.observe(ID, 570, &[a("a", false)]);
        t.observe(ID, 730, &[a("b", true)]);
        assert!(t.observe(ID, 570, &[a("a", true)]).is_empty());
        assert!(t
            .observe("76561198000000002", 570, &[a("a", true)])
            .is_empty());
        t.active = None;
        assert!(t.observe(ID, 570, &[a("c", true)]).is_empty());
    }
    #[test]
    fn empty_first_response_is_a_valid_baseline() {
        let mut t = Tracker::default();
        t.observe(ID, 570, &[]);
        assert_eq!(t.observe(ID, 570, &[a("new", true)]).len(), 1);
    }
    #[test]
    fn local_unlock_and_relock_survive_delayed_online_samples() {
        let mut t = Tracker::default();
        let locked = a("test", false);
        let unlocked = a("test", true);
        assert!(t
            .observe_source(ID, 570, &[locked.clone()], true)
            .is_empty());
        assert!(t.observe(ID, 570, &[locked.clone()]).is_empty());
        assert_eq!(
            t.observe_source(ID, 570, &[unlocked.clone()], true).len(),
            1
        );
        assert!(t.observe(ID, 570, &[locked.clone()]).is_empty());
        assert!(t.observe(ID, 570, &[unlocked.clone()]).is_empty());
        assert_eq!(t.unlocked(), 1);
        assert!(t.observe_source(ID, 570, &[locked], true).is_empty());
        assert!(t.observe(ID, 570, &[unlocked.clone()]).is_empty());
        assert_eq!(t.unlocked(), 0);
        assert_eq!(t.observe_source(ID, 570, &[unlocked], true).len(), 1);
    }

    #[test]
    fn delayed_local_baseline_is_silent_and_online_fallback_can_detect_changes() {
        let mut t = Tracker::default();
        assert!(t.observe(ID, 570, &[a("local", false)]).is_empty());
        assert!(t
            .observe_source(ID, 570, &[a("local", true)], true)
            .is_empty());
        assert_eq!(t.observe(ID, 570, &[a("web", true)]).len(), 1);
        assert!(t
            .observe_source(ID, 570, &[a("local", true), a("web", true)], true)
            .is_empty());
        assert_eq!(t.unlocked(), 2);
    }
    #[test]
    fn steam_selected_game_is_tracked_even_without_a_running_game() {
        let mut config = Config {
            steamid: ID.into(),
            notify: true,
            fullscreen: false,
            watch_app: None,
        };
        let mut player = Player {
            steamid: ID.into(),
            name: "Test".into(),
            avatar: None,
            online: 1,
            public: true,
            game: None,
        };
        assert!(tracking_game(&config, Some(&player), None).is_none());
        config.watch_app = Some(570);
        assert_eq!(tracking_game(&config, Some(&player), None).unwrap().id, 570);
        player.game = Some(Game {
            id: 730,
            name: "Another game".into(),
            icon: None,
        });
        assert_eq!(
            tracking_game(&config, Some(&player), Some(730)).unwrap().id,
            570
        );
        config.watch_app = None;
        assert_eq!(tracking_game(&config, Some(&player), None).unwrap().id, 730);
        assert_eq!(tracking_game(&config, None, Some(730)).unwrap().id, 730);
    }
    #[test]
    fn steam_app_id_accepts_numbers_and_official_game_links() {
        for input in [
            "570",
            " 570 ",
            "https://store.steampowered.com/app/570/Dota_2/",
            "https://steamcommunity.com/stats/570/achievements",
        ] {
            assert_eq!(parse_app_id(input), Some(570));
        }
        for input in [
            "0",
            "-1",
            "4294967296",
            "https://evil.example/app/570",
            "https://store.steampowered.com.evil/app/570",
            "not a game",
        ] {
            assert!(parse_app_id(input).is_none());
        }
    }
    #[test]
    fn steam_new_unlock_timestamp_rearms_without_an_intermediate_locked_sample() {
        let mut t = Tracker::default();
        let old = a("same", true);
        t.observe(ID, 570, &[old.clone()]);
        let mut new = old.clone();
        new.unlocktime += 60;
        assert_eq!(t.observe(ID, 570, &[new.clone()]).len(), 1);
        assert!(t.observe(ID, 570, &[new.clone()]).is_empty());
        assert!(t.observe(ID, 570, &[]).is_empty());
        assert!(
            t.observe(ID, 570, &[new.clone()]).is_empty(),
            "缺失列表项不能被误当作反解锁"
        );
        assert!(t.observe(ID, 570, &[old]).is_empty());
        assert!(
            t.observe(ID, 570, &[new]).is_empty(),
            "过期快照不能倒退已知解锁时间并制造重复提醒"
        );
    }
    #[test]
    fn steam_legacy_cache_migrates_and_still_seeds_silently() {
        let file =
            std::env::temp_dir().join(format!("daozai-steam-legacy-{}.json", std::process::id()));
        std::fs::write(&file, json!({format!("{ID}:570"):["old"]}).to_string()).unwrap();
        let mut tracker = Tracker::load(&file);
        assert_eq!(tracker.seen.len(), 1);
        assert!(tracker.observe(ID, 570, &[a("old", true)]).is_empty());
        tracker.save(&file).unwrap();
        let stored: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(stored["version"], 2);
        assert_eq!(Tracker::load(&file).seen.len(), 1);
        std::fs::remove_file(file).unwrap();
    }
    #[test]
    fn backoff_is_bounded_and_idle_is_cheaper() {
        assert_eq!(interval(0, true).as_secs(), 30);
        assert_eq!(interval(0, false).as_secs(), 60);
        assert!(interval(10, true).as_secs() <= 900);
        assert!(interval(2, true) > interval(1, true));
    }
    #[test]
    fn service_discards_results_from_cancelled_generation() {
        let (tx, _requests) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(0));
        let service = Service { tx, rx, epoch };
        service.send(Action::Cancel);
        events
            .send((0, Event::Error("stale result".into())))
            .unwrap();
        events.send((1, Event::Cancelled)).unwrap();
        assert!(matches!(service.try_recv(), Some(Event::Cancelled)));
        assert!(service.try_recv().is_none());
    }
    fn callback(expected: &str) -> url::Url {
        let mut u = url::Url::parse(expected).unwrap();
        let identity = format!("https://steamcommunity.com/openid/id/{ID}");
        let nonce = chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%SZnonce")
            .to_string();
        u.query_pairs_mut().extend_pairs([
            ("openid.ns", NS),
            ("openid.mode", "id_res"),
            ("openid.op_endpoint", OPENID),
            ("openid.return_to", expected),
            ("openid.identity", &identity),
            ("openid.claimed_id", &identity),
            ("openid.response_nonce", &nonce),
            ("openid.assoc_handle", "handle"),
            (
                "openid.signed",
                "op_endpoint,claimed_id,identity,return_to,response_nonce,assoc_handle",
            ),
        ]);
        u
    }
    #[test]
    fn openid_rejects_duplicate_or_mismatched_return_and_identity() {
        let expected = "http://127.0.0.1:1234/steam/callback/random";
        let u = callback(expected);
        assert!(callback_fields(&u, expected).is_ok());
        assert!(callback_fields(&u, "http://127.0.0.1:1234/wrong").is_err());
        let mut dup = u;
        dup.query_pairs_mut()
            .append_pair("openid.claimed_id", "attacker");
        assert!(callback_fields(&dup, expected).is_err());
    }
    #[test]
    fn openid_requires_all_identity_fields_to_be_signed() {
        let expected = "http://127.0.0.1:1234/callback";
        let mut u = callback(expected);
        let pairs = u
            .query_pairs()
            .filter(|(k, _)| k != "openid.signed")
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect::<Vec<_>>();
        u.set_query(None);
        u.query_pairs_mut()
            .extend_pairs(pairs)
            .append_pair("openid.signed", "identity");
        assert!(callback_fields(&u, expected).is_err());
    }
    #[test]
    fn config_and_cache_do_not_contain_key() {
        let path = std::env::temp_dir().join(format!("daozai-steam-test-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        let file = path.join("cache.json");
        let mut t = Tracker::default();
        t.observe(ID, 570, &[a("seen", true)]);
        t.save(&file).unwrap();
        let mut loaded = Tracker::load(&file);
        assert!(loaded
            .observe(ID, 570, &[a("seen", true), a("offline", true)])
            .is_empty());
        std::fs::remove_file(file).unwrap();
        std::fs::remove_dir(path).unwrap();
    }
}
