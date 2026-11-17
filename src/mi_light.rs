//! Mi home desk lamp bar 2 (xiaomi.light.bar2) cloud control.
//!
//! Firmware closes the LAN miIO port, so the only path is the MIoT cloud
//! (api.io.mi.com): RC4-encrypted body + sha1 signature, keys come from the
//! login session (serviceToken + ssecurity). All network work runs on a
//! background thread; the UI thread only reads snapshots.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ring::digest::{digest, SHA256};
use serde_json::{json, Value};

const UA: &str = "Android-7.1.1-1.0.0-ONEPLUS A3010-136-abcdefgh APP/xiaomi.smarthome APPV/62830";
const HOST: &str = "https://api.io.mi.com/app";
const DEVICE_LIST: &str = "/home/device_list";
const SIID: u32 = 2;
const PIID_ON: u32 = 1;
const PIID_BRIGHT: u32 = 2;
const PIID_CT: u32 = 3;
pub const CT_MIN: u32 = 2700;
pub const CT_MAX: u32 = 6500;
const IDLE_STOP_MS: u64 = 15000;

#[derive(Clone, PartialEq, Debug)]
pub struct State {
    pub logged: bool,
    pub online: bool,
    pub loaded: bool,
    pub on: bool,
    pub bright: u32,
    pub ct: u32,
    pub busy: bool,
    pub note: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            logged: false,
            online: false,
            loaded: false,
            on: false,
            bright: 60,
            ct: 4000,
            busy: false,
            note: String::new(),
        }
    }
}

enum Cmd {
    Power(bool),
    Bright(u32),
    Ct(u32),
    Login,
    Retry,
}

static TX: OnceLock<Sender<Cmd>> = OnceLock::new();
static ACTIVE: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Called from the paint path while the panel is visible.
pub fn touch() {
    ACTIVE.store(now_ms(), Ordering::Relaxed);
}

fn active() -> bool {
    now_ms().saturating_sub(ACTIVE.load(Ordering::Relaxed)) < IDLE_STOP_MS
}

fn send(c: Cmd) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(c);
    }
}

pub fn request_power(on: bool) {
    send(Cmd::Power(on));
}
pub fn request_bright(v: u32) {
    send(Cmd::Bright(v));
}
pub fn request_ct(v: u32) {
    send(Cmd::Ct(v));
}
pub fn request_login() {
    send(Cmd::Login);
}
pub fn request_reload() {
    send(Cmd::Retry);
}
fn sha1(d: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut m = d.to_vec();
    let bits = (d.len() as u64) * 8;
    m.push(0x80);
    while m.len() % 64 != 56 { m.push(0); }
    m.extend_from_slice(&bits.to_be_bytes());
    for ch in m.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 { let b = &ch[i * 4..i * 4 + 4]; w[i] = u32::from_be_bytes([b[0], b[1], b[2], b[3]]); }
        for i in 16..80 { w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1); }
        let (mut a, mut b, mut c, mut e, mut f) = (h[0], h[1], h[2], h[3], h[4]);
        for i in 0..80 {
            let (g, k) = if i < 20 { ((b & c) | ((!b) & e), 0x5A827999u32) } else if i < 40 { (b ^ c ^ e, 0x6ED9EBA1) } else if i < 60 { ((b & c) | (b & e) | (c & e), 0x8F1BBCDC) } else { (b ^ c ^ e, 0xCA62C1D6) };
            let t = a.rotate_left(5).wrapping_add(g).wrapping_add(f).wrapping_add(k).wrapping_add(w[i]);
            f = e; e = c; c = b.rotate_left(30); b = a; a = t;
        }
        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b); h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(e); h[4] = h[4].wrapping_add(f);
    }
    let mut o = [0u8; 20];
    for i in 0..5 { o[i * 4..i * 4 + 4].copy_from_slice(&h[i].to_be_bytes()); }
    o
}
struct Rc4 { s: [u8; 256], i: u8, j: u8 }
impl Rc4 {
    fn new(key: &[u8]) -> Self {
        let mut s = [0u8; 256];
        for k in 0..256 { s[k] = k as u8; }
        let mut j = 0u8;
        for k in 0..256 { j = j.wrapping_add(s[k]).wrapping_add(key[k % key.len()]); s.swap(k, j as usize); }
        Self { s, i: 0, j: 0 }
    }
    fn apply(&mut self, d: &mut [u8]) {
        for b in d.iter_mut() {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.s[self.i as usize]);
            self.s.swap(self.i as usize, self.j as usize);
            let k = self.s[(self.s[self.i as usize].wrapping_add(self.s[self.j as usize])) as usize];
            *b ^= k;
        }
    }
}

use ring::rand::{SecureRandom, SystemRandom};

/// RC4 keyed by a base64 password (ssecurity / snonce); the first 1024
/// keystream bytes are dropped, like the python client.
fn rc4(pw: &str, buf: &[u8]) -> Option<Vec<u8>> {
    let key = B64.decode(pw.trim()).ok()?;
    if key.is_empty() {
        return None;
    }
    let mut c = Rc4::new(&key);
    let mut drop = [0u8; 1024];
    c.apply(&mut drop);
    let mut out = buf.to_vec();
    c.apply(&mut out);
    Some(out)
}

fn rc4_encrypt(password_b64: &str, payload: &str) -> Option<String> {
    rc4(password_b64, payload.as_bytes()).map(|v| B64.encode(v))
}

fn rc4_decrypt(password_b64: &str, payload_b64: &str) -> Option<String> {
    let raw = B64.decode(payload_b64.trim()).ok()?;
    String::from_utf8(rc4(password_b64, &raw)?).ok()
}

/// base64(random 8 bytes ++ minutes since epoch, minimal length).
fn gen_nonce() -> String {
    let mut buf = [0u8; 16];
    let mut r = [0u8; 8];
    if SystemRandom::new().fill(&mut r).is_err() {
        r = now_ms().to_be_bytes();
    }
    buf[..8].copy_from_slice(&r);
    let mins = now_ms() / 60_000;
    let n = ((64 - mins.leading_zeros() as usize) + 7) / 8;
    let full = mins.to_be_bytes();
    buf[8..8 + n].copy_from_slice(&full[8 - n..]);
    B64.encode(&buf[..8 + n])
}

/// base64(sha256(ssecurity ++ nonce)): the per-request rc4 key.
fn signed_nonce(ssecurity: &str, nonce: &str) -> Option<String> {
    let a = B64.decode(ssecurity.trim()).ok()?;
    let b = B64.decode(nonce.trim()).ok()?;
    let mut m = a;
    m.extend_from_slice(&b);
    Some(B64.encode(digest(&SHA256, &m)))
}

/// base64(sha1(METHOD&/path&k=v...&snonce)); parameter order is kept as given.
fn gen_enc_signature(url: &str, method: &str, snonce: &str, params: &[(String, String)]) -> String {
    let tail = url
        .split_once("com")
        .map(|(_, r)| r)
        .unwrap_or(url)
        .replace("/app/", "/");
    let mut parts: Vec<String> = Vec::with_capacity(params.len() + 3);
    parts.push(method.to_uppercase());
    parts.push(tail);
    for (k, v) in params {
        parts.push(format!("{k}={v}"));
    }
    parts.push(snonce.to_string());
    B64.encode(sha1(parts.join("&").as_bytes()))
}

/// Credentials of a signed-in Mi Home account: uid, service token and
/// ssecurity (base64) -- all the MIoT cloud needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub uid: String,
    pub token: String,
    pub ssecurity: String,
}

/// <data dir>\mi_session.json (DAOZAI_DATA_DIR overrides the data dir).
pub fn session_path() -> PathBuf {
    crate::model::config_dir().join("mi_session.json")
}

fn field(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

impl Session {
    pub fn load() -> Result<Session, String> {
        Self::load_from(&session_path())
    }

    pub fn load_from(path: &std::path::Path) -> Result<Session, String> {
        let raw =
            std::fs::read(path).map_err(|_| format!("未找到米家登录态：{}", path.display()))?;
        let mut body: &[u8] = &raw;
        if body.starts_with(b"\xef\xbb\xbf") {
            body = &body[3..];
        }
        let v: Value = serde_json::from_slice(body)
            .map_err(|_| format!("米家登录态格式错误：{}", path.display()))?;
        let s = Session {
            uid: field(&v, "userId"),
            token: field(&v, "serviceToken"),
            ssecurity: field(&v, "ssecurity"),
        };
        if s.uid.is_empty() || s.token.is_empty() || s.ssecurity.is_empty() {
            return Err(format!("米家登录态字段缺失：{}", path.display()));
        }
        if B64
            .decode(s.ssecurity.as_str())
            .map_or(true, |k| k.is_empty())
        {
            return Err(format!("米家登录态 ssecurity 无效：{}", path.display()));
        }
        Ok(s)
    }
}

// ---------- 灯设备发现 ----------

/// <data dir>\mi_device.json：这台电脑当前绑定的灯（did + model）。
pub fn device_path() -> PathBuf {
    crate::model::config_dir().join("mi_device.json")
}

/// 已缓存的设备 DID；没有就是空串，交给云端发现。
fn cached_did() -> String {
    let Ok(raw) = std::fs::read(device_path()) else { return String::new() };
    let mut body: &[u8] = &raw;
    if body.starts_with(b"\xef\xbb\xbf") {
        body = &body[3..];
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(v) => field(&v, "did"),
        Err(_) => String::new(),
    }
}

fn save_did(did: &str, model: &str) {
    let body = json!({"did": did, "model": model});
    let Ok(text) = serde_json::to_string(&body) else { return };
    let dir = crate::model::config_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(device_path(), text);
}

/// 灯类设备的模型名特征：挂灯、台灯、吸顶灯都以 light 开头或含 light。
fn looks_like_lamp(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.contains("light") || m.contains("lamp")
}

/// 让用户的米家账号自己报出灯具，彻底替代旧版写死的作者设备。
fn discover_did(sess: &Session) -> Result<(String, String), String> {
    let body = json!({"getVirtualModel": false, "getHuamiDevices": 0});
    let v = post(sess, DEVICE_LIST, &body)?;
    let list = v
        .get("result")
        .and_then(|r| r.get("list"))
        .and_then(|l| l.as_array())
        .cloned()
        .unwrap_or_default();
    if list.is_empty() {
        return Err("米家账号下没有设备，无法绑定灯具".into());
    }
    for item in &list {
        let did = item
            .get("did")
            .map(|d| match d {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .unwrap_or_default();
        let model = item.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
        let name = item.get("name").and_then(|m| m.as_str()).unwrap_or("").to_string();
        if did.is_empty() {
            continue;
        }
        if looks_like_lamp(&model) {
            save_did(&did, &model);
            return Ok((did, if name.is_empty() { model } else { name }));
        }
    }
    Err("米家账号下没有找到灯具设备，请在米家 App 里添加一盏灯后重试".into())
}

/// 当前要控制的灯：优先用缓存，没有就现场发现并落盘。
fn resolve_did(sess: &Session) -> Result<String, String> {
    let cached = cached_did();
    if !cached.is_empty() {
        return Ok(cached);
    }
    discover_did(sess).map(|(did, _)| did)
}

// ---------- MIoT cloud client ----------

const PROP_GET: &str = "/miotspec/prop/get";
const PROP_SET: &str = "/miotspec/prop/set";
const SYNC_EVERY_MS: u64 = 12000;

static STATE: OnceLock<std::sync::Mutex<State>> = OnceLock::new();
static DEVICE_ID: OnceLock<String> = OnceLock::new();

fn device_id() -> String {
    DEVICE_ID
        .get_or_init(|| {
            let mut r = [0u8; 8];
            if SystemRandom::new().fill(&mut r).is_err() {
                r = now_ms().to_be_bytes();
            }
            let hex: String = r.iter().map(|b| format!("{b:02X}")).collect();
            format!("wb_{hex}")
        })
        .clone()
}

fn cookies(s: &Session) -> String {
    format!(
        "userId={}; yetAnotherServiceToken={}; serviceToken={}; locale=zh_CN; timezone=Asia/Shanghai; is_daylight=0; dst_offset=0; channel=MI_APP_STORE; sdkVersion=3.8.6; deviceId={}",
        s.uid, s.token, s.token, device_id()
    )
}

fn cloud_error(code: i64, msg: &str) -> String {
    let msg = msg.trim_matches('"');
    if code == 401 || code == 403 {
        format!("auth: 米家登录态失效（code={code}），请重新登录米家")
    } else if msg.is_empty() {
        format!("米家云返回 code={code}")
    } else {
        format!("米家云返回 code={code}：{msg}")
    }
}

/// One signed, RC4-encrypted round trip against api.io.mi.com.
/// `code != 0` comes back as an error; auth failures carry the "auth:" prefix
/// so the UI can ask for a fresh login instead of just retrying.
fn post(sess: &Session, path: &str, body: &Value) -> Result<Value, String> {
    let url = format!("{HOST}{path}");
    let jar = cookies(sess);
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .build();
    let mut last = String::from("米家云请求失败");
    for _ in 0..3 {
        let nonce = gen_nonce();
        let snonce = match signed_nonce(&sess.ssecurity, &nonce) {
            Some(v) => v,
            None => return Err("auth: 米家登录态 ssecurity 无效，请重新登录米家".into()),
        };
        let data = serde_json::to_string(body).map_err(|e| e.to_string())?;
        let mut params: Vec<(String, String)> = vec![("data".to_string(), data)];
        let hash = gen_enc_signature(&url, "POST", &snonce, &params);
        params.push(("rc4_hash__".to_string(), hash));
        for it in params.iter_mut() {
            it.1 = match rc4_encrypt(&snonce, &it.1) {
                Some(v) => v,
                None => return Err("请求加密失败".into()),
            };
        }
        let sig = gen_enc_signature(&url, "POST", &snonce, &params);
        params.push(("signature".to_string(), sig));
        params.push(("ssecurity".to_string(), sess.ssecurity.clone()));
        params.push(("_nonce".to_string(), nonce));
        let form: Vec<(&str, &str)> = params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let reply = agent
            .post(&url)
            .set("User-Agent", UA)
            .set("Accept-Encoding", "identity")
            .set("x-xiaomi-protocal-flag-cli", "PROTOCAL-HTTP2")
            .set("Content-Type", "application/x-www-form-urlencoded")
            .set("MIOT-ENCRYPT-ALGORITHM", "ENCRYPT-RC4")
            .set("Cookie", &jar)
            .send_form(&form);
        match reply {
            Ok(r) => {
                let text = r.into_string().map_err(|e| e.to_string())?;
                let plain = match rc4_decrypt(&snonce, text.trim()) {
                    Some(v) => v,
                    None => return Err("米家云响应解密失败".into()),
                };
                let v: Value = serde_json::from_str(&plain)
                    .map_err(|_| format!("米家云响应解析失败：{}", plain.chars().take(120).collect::<String>()))?;
                let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-999);
                if code == 0 {
                    return Ok(v);
                }
                let msg = match v.get("message") {
                    Some(m) => m.to_string(),
                    None => String::new(),
                };
                return Err(cloud_error(code, &msg));
            }
            Err(ureq::Error::Status(code, _)) => last = format!("米家云 HTTP {code}"),
            Err(e) => last = format!("米家云请求失败：{e}"),
        }
        std::thread::sleep(Duration::from_millis(600));
    }
    Err(last)
}

/// Reads on / brightness / colour temperature in one batched prop/get.
fn prop_get(sess: &Session, did: &str) -> Result<(bool, u32, u32), String> {
    let body = json!({
        "params": [
            {"did": did, "siid": SIID, "piid": PIID_ON},
            {"did": did, "siid": SIID, "piid": PIID_BRIGHT},
            {"did": did, "siid": SIID, "piid": PIID_CT}
        ],
        "datasource": 2
    });
    let v = post(sess, PROP_GET, &body)?;
    let list = match v.get("result").and_then(|r| r.as_array()) {
        Some(l) => l,
        None => return Err("米家云未返回属性数据".into()),
    };
    let (mut on, mut bright, mut ct) = (None, None, None);
    for it in list {
        let code = it.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = match it.get("message") {
                Some(m) => m.to_string(),
                None => String::new(),
            };
            return Err(cloud_error(code, &msg));
        }
        let piid = it.get("piid").and_then(|p| p.as_u64()).unwrap_or(0);
        let val = it.get("value").map(val_i64).unwrap_or(0);
        match piid as u32 {
            PIID_ON => on = Some(val != 0),
            PIID_BRIGHT => bright = Some(val.clamp(0, 100) as u32),
            PIID_CT => ct = Some(val.clamp(CT_MIN as i64, CT_MAX as i64) as u32),
            _ => {}
        }
    }
    match (on, bright, ct) {
        (Some(o), Some(b), Some(c)) => Ok((o, b, c)),
        _ => Err("米家云返回的属性不完整".into()),
    }
}

/// MIoT returns a real JSON bool for switch properties and plain numbers for
/// the rest; accept both (plus numeric strings) instead of assuming integers.
fn val_i64(v: &Value) -> i64 {
    match v {
        Value::Bool(b) => i64::from(*b),
        Value::Number(n) => n.as_i64().unwrap_or(0),
        Value::String(s) => match s.trim() {
            "true" => 1,
            "false" => 0,
            other => other.parse().unwrap_or(0),
        },
        _ => 0,
    }
}

/// Writes a single property through prop/set. The switch property only takes a
/// JSON bool: the cloud turns an integer 1/0 into `false`, i.e. it turns the
/// lamp off no matter what we asked for.
fn prop_set(sess: &Session, did: &str, piid: u32, value: Value) -> Result<(), String> {
    let body = json!({
        "params": [{"did": did, "siid": SIID, "piid": piid, "value": value}]
    });
    let v = post(sess, PROP_SET, &body)?;
    if let Some(list) = v.get("result").and_then(|r| r.as_array()) {
        for it in list {
            let code = it.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
            if code != 0 {
                let msg = match it.get("message") {
                    Some(m) => m.to_string(),
                    None => String::new(),
                };
                return Err(cloud_error(code, &msg));
            }
        }
    }
    Ok(())
}

fn state_with(f: impl FnOnce(&mut State)) {
    if let Some(m) = STATE.get() {
        if let Ok(mut g) = m.lock() {
            f(&mut g);
        }
    }
}

/// Snapshot for the UI thread; never blocks on the network.
pub fn snapshot() -> State {
    match STATE.get() {
        Some(m) => match m.lock() {
            Ok(g) => g.clone(),
            Err(_) => State::default(),
        },
        None => State::default(),
    }
}

fn set_note(note: String) {
    let auth = note.starts_with("auth");
    state_with(|s| {
        s.busy = false;
        s.note = note;
        if auth {
            s.logged = false;
            s.online = false;
            s.loaded = false;
        }
    });
}

/// Pulls the real device state; the lamp is the only source of truth.
fn sync(sess: &Session) {
    state_with(|s| s.busy = true);
    let did = match resolve_did(sess) {
        Ok(did) => did,
        Err(e) => {
            state_with(|s| {
                s.loaded = false;
                s.busy = false;
            });
            set_note(e);
            return;
        }
    };
    match prop_get(sess, &did) {
        Ok((on, bright, ct)) => state_with(|s| {
            s.logged = true;
            s.online = true;
            s.loaded = true;
            s.busy = false;
            s.on = on;
            s.bright = bright;
            s.ct = ct;
            s.note.clear();
        }),
        Err(e) => {
            // 设备被移出账号或换了灯泡：清掉缓存，下次自动重新发现，而不是一直报错。
            let lower = e.to_ascii_lowercase();
            if e.contains("不存在") || lower.contains("not exist") || lower.contains("device not") {
                let _ = std::fs::remove_file(device_path());
            }
            state_with(|s| {
                s.online = false;
                s.loaded = false;
            });
            set_note(e);
        }
    }
}

/// Applies one queued command, then re-reads the lamp so the UI never lies.
fn run_cmd(sess: &Session, cmd: &Cmd) {
    // 登录/重试由 worker 自己处理，这里只管真正要下发的三条指令。
    let step = {
        let did = match resolve_did(sess) {
            Ok(did) => did,
            Err(e) => {
                set_note(e);
                return;
            }
        };
        match cmd {
            Cmd::Power(on) => prop_set(sess, &did, PIID_ON, json!(*on)),
            Cmd::Bright(v) => {
                let v = (*v).min(100);
                prop_set(sess, &did, PIID_BRIGHT, json!(v as i64))
            }
            Cmd::Ct(v) => {
                let v = (*v).clamp(CT_MIN, CT_MAX);
                prop_set(sess, &did, PIID_CT, json!(v as i64))
            }
            Cmd::Login | Cmd::Retry => Ok(()),
        }
    };
    apply(sess, step);
}

fn apply(sess: &Session, step: Result<(), String>) {
    match step {
        Ok(()) => {
            state_with(|s| {
                s.online = true;
                s.note.clear();
            });
            sync(sess);
        }
        Err(e) => set_note(e),
    }
}

use eframe::egui;

/// Boots the background worker; safe to call on every frame.
pub fn start(ctx: &egui::Context) {
    let (tx, rx) = mpsc::channel::<Cmd>();
    if TX.set(tx).is_err() {
        return;
    }
    STATE.get_or_init(|| std::sync::Mutex::new(State::default()));
    let ctx = ctx.clone();
    let _ = std::thread::Builder::new()
        .name("mi-light".to_string())
        .spawn(move || worker(rx, ctx));
}

fn load_session() -> Option<Session> {
    match Session::load() {
        Ok(s) => {
            state_with(|s| {
                s.logged = true;
                s.note.clear();
            });
            Some(s)
        }
        Err(e) => {
            set_note(e);
            None
        }
    }
}

/// Only runs while the panel is visible (see `touch`); otherwise it just
/// parks on the channel so an idle desk lamp costs no CPU.
fn worker(rx: Receiver<Cmd>, ctx: egui::Context) {
    let mut sess = load_session();
    if let Some(s) = &sess {
        sync(s);
    }
    let mut synced = Instant::now();
    ctx.request_repaint();
    loop {
        let wait = if active() {
            Duration::from_millis(700)
        } else {
            Duration::from_secs(3)
        };
        match rx.recv_timeout(wait) {
            Ok(Cmd::Login) | Ok(Cmd::Retry) => {
                sess = load_session();
                if let Some(s) = &sess {
                    sync(s);
                }
                synced = Instant::now();
                ctx.request_repaint();
            }
            Ok(cmd) => {
                if sess.is_none() {
                    sess = load_session();
                }
                match &sess {
                    Some(s) => run_cmd(s, &cmd),
                    None => set_note("未找到米家登录态，请先登录米家".to_string()),
                }
                synced = Instant::now();
                ctx.request_repaint();
            }
            Err(RecvTimeoutError::Timeout) => {
                if active() && synced.elapsed() >= Duration::from_millis(SYNC_EVERY_MS) {
                    synced = Instant::now();
                    if sess.is_none() {
                        sess = load_session();
                    }
                    if let Some(s) = &sess {
                        sync(s);
                        ctx.request_repaint();
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

