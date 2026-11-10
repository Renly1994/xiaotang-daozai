//! Encrypted WebDAV settings snapshots. No mail content or sync credentials leave the device.
use base64::{engine::general_purpose::STANDARD, Engine};
use ring::{
    aead, digest, pbkdf2,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    num::NonZeroU32,
    path::Path,
    time::Duration,
};

const LIMIT: usize = 2 * 1024 * 1024;
const ITERATIONS: u32 = 600_000;
const AAD: &[u8] = b"Daozai.WebDAV.v1";
#[cfg(test)]
thread_local! {static FAIL_AFTER_SETTINGS:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};}
#[cfg(test)]
pub(crate) fn fail_next_restore_for_test(){FAIL_AFTER_SETTINGS.with(|flag|flag.set(true));}
// Explicit allowlist: unknown future fields, meeting details and device permissions stay local.
const SYNC_KEYS: &[&str] = &[
    "skin","island_shape","dock","glass_blur","island_size","island_offset","settings_theme",
    "pet","minimal","browser_minimal","visible_pages","page_order","resident_order",
    "clock_period","clock_12h","show_battery","sound","lyrics_enabled","island_lyrics",
    "lyric_delay_ms","hide_fullscreen","focus_minutes","rest_minutes","water_minutes",
    "serving_ml","water_start","water_end","mail_code","mail_code_hide_seconds","mail_interval",
];

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Config {
    pub folder: String,
    pub username: String,
    pub automatic: bool,
    pub mail_passwords: bool,
}
impl Config {
    pub fn load() -> Self {
        fs::read(crate::model::config_dir().join("cloud-sync.json"))
            .ok()
            .and_then(|v| serde_json::from_slice(&v).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) -> Result<(), String> {
        let root = crate::model::config_dir();
        fs::create_dir_all(&root).map_err(|_| "无法创建配置目录")?;
        crate::model::atomic_write(
            &root.join("cloud-sync.json"),
            &serde_json::to_vec(self).map_err(|_| "无法保存同步设置")?,
        )
        .map_err(|_| "无法保存同步设置".into())
    }
    pub fn target(&self) -> Result<String, String> {
        let mut url = url::Url::parse(self.folder.trim())
            .map_err(|_| "请填写完整的 HTTP 或 HTTPS WebDAV 文件夹地址")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("WebDAV 仅支持 HTTP 或 HTTPS，地址中不能包含账号、密码或查询参数".into());
        }
        if self.username.trim().is_empty()
            || self.username.contains(':')
            || self.username.chars().any(char::is_control)
        {
            return Err("请填写有效的 WebDAV 用户名".into());
        }
        let path = format!("{}/", url.path().trim_end_matches('/'));
        url.set_path(&path);
        url.join("daozai-sync-v1.json")
            .map(|u| u.to_string())
            .map_err(|_| "WebDAV 地址无效".into())
    }
    pub fn identity(&self) -> Result<String, String> {
        Ok(hash(
            format!("{}\0{}", self.target()?, self.username.trim()).as_bytes(),
        ))
    }
}

pub(crate) fn secret_key(cfg: &Config, kind: &str) -> Result<String, String> {
    Ok(format!("cloud-sync-{}-{kind}", cfg.identity()?))
}
pub(crate) fn save_secret(cfg: &Config, kind: &str, value: &str) -> Result<(), String> {
    crate::mail::secrets::save(&secret_key(cfg, kind)?, value)
}
pub(crate) fn load_secret(cfg: &Config, kind: &str) -> Result<String, String> {
    crate::mail::secrets::load(&secret_key(cfg, kind)?)
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snapshot {
    version: u32,
    settings: BTreeMap<String, String>,
    accounts: serde_json::Value,
    passwords: BTreeMap<String, String>,
}
fn settings_map(raw: &str) -> BTreeMap<String, String> {
    raw.trim_start_matches('\u{feff}')
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(k, _)| SYNC_KEYS.contains(&k.trim()))
        .map(|(k, v)| (k.trim().into(), v.trim().into()))
        .collect()
}
fn read_optional(path: &Path, default: &[u8]) -> Result<Vec<u8>, String> {
    match fs::read(path) {
        Ok(v) if v.len() <= LIMIT => Ok(v),
        Ok(_) => Err("本机配置过大，未同步".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(default.to_vec()),
        Err(_) => Err("无法读取本机配置，未同步".into()),
    }
}
impl Snapshot {
    pub fn capture() -> Result<Self, String> { Self::capture_with_passwords(true) }
    pub fn capture_with_passwords(include_passwords:bool) -> Result<Self,String> {
        let root = crate::model::config_dir();
        let raw = read_optional(&root.join("settings.txt"), b"")?;
        let settings = settings_map(std::str::from_utf8(&raw).map_err(|_| "本机设置编码无效")?);
        let raw = read_optional(&root.join("mail-accounts.json"), b"[]")?;
        let accounts = serde_json::from_slice(&raw).map_err(|_| "本机邮箱配置无效")?;
        let mut snapshot = Self {
            version: 1,
            settings,
            accounts,
            passwords: BTreeMap::new(),
        };
        for account in snapshot.checked_accounts(false)? {
            if !include_passwords {continue;}
            let key = account.credential_key();
            if let Some(password)=crate::mail::secrets::load_optional(&key)? {
                snapshot.passwords.insert(key,password);
            }
        }
        snapshot.validate()?;
        Ok(snapshot)
    }
    fn scoped(mut self,include_passwords:bool)->Self {
        self.settings.retain(|key,_|SYNC_KEYS.contains(&key.as_str()));
        if !include_passwords {self.passwords.clear();}
        self
    }
    fn checked_accounts(
        &self,
        require_passwords: bool,
    ) -> Result<Vec<crate::mail::Account>, String> {
        let invalid = || "同步文件中的邮箱配置无效".to_string();
        let list = self.accounts.as_array().ok_or_else(invalid)?;
        if list.len() > 100 {
            return Err(invalid());
        }
        let accounts = crate::mail::decode_accounts(
            &serde_json::to_vec(&self.accounts).map_err(|_| invalid())?,
        );
        if accounts.len() != list.len() {
            return Err(invalid());
        }
        let mut keys = BTreeSet::new();
        let mut emails = BTreeSet::new();
        for (raw, account) in list.iter().zip(&accounts) {
            let provider = raw
                .get("provider")
                .and_then(|p| p.as_str())
                .ok_or_else(invalid)?;
            if provider != account.provider.key()
                || account.email.len() > 320
                || account.email.chars().any(char::is_control)
                || !account.email.contains('@')
                || account.endpoint().is_err()
                || !emails.insert(crate::mail::key(account))
                || !keys.insert(account.credential_key())
            {
                return Err(invalid());
            }
        }
        let _=require_passwords; // Missing credentials are a valid backup/restore state.
        if self.passwords.iter().any(|(key,password)|!keys.contains(key) || password.is_empty() || password.len()>4096) {
            return Err("同步文件中的邮箱授权码无效".into());
        }
        Ok(accounts)
    }
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("不支持此同步文件版本，请更新岛仔".into());
        }
        if self.settings.len() > 256
            || self.settings.iter().any(|(k, v)| {
                k.is_empty()
                    || k.contains(['\n', '\r', '='])
                    || v.contains(['\n', '\r'])
            })
        {
            return Err("同步设置格式无效".into());
        }
        self.checked_accounts(true)?;
        Ok(())
    }
    pub fn fingerprint(&self) -> Result<String, String> {
        Ok(hash(&serde_json::to_vec(self).map_err(|_| "无法编码配置")?))
    }
    /// Backup ciphertext first; rollback both files and keyring if any restore step fails.
    pub fn restore(&self,expected:&str,password:&str)->Result<bool,String> {
        self.restore_with_passwords(expected,password,true)
    }
    pub fn restore_with_passwords(&self, expected: &str, password: &str, include_passwords:bool) -> Result<bool, String> {
        self.validate()?;
        let previous = Self::capture()?;
        if Self::capture_with_passwords(include_passwords)?.fingerprint()? != expected {
            return Err("同步期间本机配置有修改，请再次同步".into());
        }
        let root = crate::model::config_dir();
        fs::create_dir_all(&root).map_err(|_| "无法创建配置目录")?;
        let backup = root.join("sync-backups");
        fs::create_dir_all(&backup).map_err(|_| "无法创建同步备份")?;
        let name = format!("{}.json", chrono::Utc::now().format("%Y%m%dT%H%M%S%.9f"));
        crate::model::atomic_write(&backup.join(name), &encrypt(&previous, password)?)
            .map_err(|_| "备份失败，未恢复配置")?;
        let paths = [root.join("settings.txt"), root.join("mail-accounts.json")];
        let old: Vec<Option<Vec<u8>>> = paths
            .iter()
            .map(|p| match fs::read(p) {
                Ok(v) => Ok(Some(v)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(_) => Err("无法读取本机备份".to_string()),
            })
            .collect::<Result<_, _>>()?;
        // Preserve every local field outside the explicit sync allowlist, including future fields.
        let mut settings=BTreeMap::<String,String>::new();
        let existing = String::from_utf8(old[0].clone().unwrap_or_default())
            .map_err(|_| "本机设置编码无效")?;
        for line in existing.lines() {
            if let Some((k, v)) = line.split_once('=') {
                settings.insert(k.trim().into(), v.trim().into());
            }
        }
        for (key,value) in &self.settings {
            if SYNC_KEYS.contains(&key.as_str()) {settings.insert(key.clone(),value.clone());}
        }
        let text: String = settings.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
        let accounts = serde_json::to_vec(&self.accounts).map_err(|_| "邮箱配置编码失败")?;
        let incoming=if include_passwords {self.passwords.clone()}else{BTreeMap::new()};
        // Include credentials not present in the old account list, too: rollback must restore their exact state.
        let previous_credentials=incoming.keys().map(|key|Ok((key.clone(),crate::mail::secrets::load_optional(key)?)))
            .collect::<Result<BTreeMap<_,_>,String>>()?;
        let mail_changed=crate::mail::decode_accounts(&old[1].clone().unwrap_or_else(||b"[]".to_vec()))
            !=self.checked_accounts(false)? || incoming.iter().any(|(key,value)|previous_credentials.get(key).and_then(|v|v.as_ref())!=Some(value));
        if Self::capture_with_passwords(include_passwords)?.fingerprint()? != expected {
            return Err("同步期间本机配置有修改，请再次同步".into());
        }
        // Recheck raw files as well: local-only edits must not be lost during backup encryption.
        for (path,before) in paths.iter().zip(&old) {
            let now=match fs::read(path){Ok(value)=>Some(value),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>None,Err(_)=>return Err("无法检查本机配置".into())};
            if &now!=before{return Err("同步期间本机配置有修改，请再次同步".into());}
        }
        let result = (|| {
            for (key, value) in &incoming {
                crate::mail::secrets::save(key, value)
                    .map_err(|_| "无法恢复邮箱授权码".to_string())?;
            }
            if old[0].as_deref()!=Some(text.as_bytes()) {
                crate::model::atomic_write(&paths[0], text.as_bytes())
                    .map_err(|_| "无法恢复通用设置".to_string())?;
            }
            #[cfg(test)]
            if FAIL_AFTER_SETTINGS.with(|flag|flag.replace(false)){return Err("模拟恢复中断".into());}
            if old[1].as_deref()!=Some(accounts.as_slice()) {
                crate::model::atomic_write(&paths[1], &accounts)
                    .map_err(|_| "无法恢复邮箱设置".to_string())?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            let mut rollback_ok = true;
            for (key,value) in &previous_credentials {
                if let Some(value) = value {
                    rollback_ok &= crate::mail::secrets::save(key, value).is_ok();
                } else {
                    rollback_ok &= crate::mail::secrets::delete_checked(key).is_ok();
                }
            }
            for (path, value) in paths.iter().zip(old) {
                rollback_ok &= match value {
                    Some(value) => crate::model::atomic_write(path, &value).is_ok(),
                    None => !path.exists() || fs::remove_file(path).is_ok(),
                };
            }
            return Err(if rollback_ok {
                error
            } else {
                "恢复失败且回滚未完成，请从 sync-backups 中恢复加密备份".into()
            });
        }
        // Keep removed accounts' credentials for rollback; they are no longer monitored or uploaded.
        Ok(mail_changed)
    }
}
fn hash(bytes: &[u8]) -> String {
    digest::digest(&digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    iterations: u32,
    salt: String,
    nonce: String,
    ciphertext: String,
}
fn key(password: &str, salt: &[u8]) -> Result<aead::LessSafeKey, String> {
    let mut bytes = [0u8; 32];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(ITERATIONS).unwrap(),
        salt,
        password.as_bytes(),
        &mut bytes,
    );
    aead::UnboundKey::new(&aead::AES_256_GCM, &bytes)
        .map(aead::LessSafeKey::new)
        .map_err(|_| "无法初始化加密".into())
}
fn encrypt(snapshot: &Snapshot, password: &str) -> Result<Vec<u8>, String> {
    if password.chars().count() < 12 {
        return Err("同步密码至少需要 12 个字符".into());
    }
    let rng = SystemRandom::new();
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    rng.fill(&mut salt)
        .and_then(|_| rng.fill(&mut nonce))
        .map_err(|_| "无法生成加密随机数")?;
    let mut body = serde_json::to_vec(snapshot).map_err(|_| "配置编码失败")?;
    if body.len() > LIMIT / 2 {
        return Err("配置过大，未上传".into());
    }
    key(password, &salt)?
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(AAD),
            &mut body,
        )
        .map_err(|_| "加密失败")?;
    serde_json::to_vec(&Envelope {
        version: 1,
        iterations: ITERATIONS,
        salt: STANDARD.encode(salt),
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(body),
    })
    .map_err(|_| "加密文件编码失败".into())
}
fn decrypt(raw: &[u8], password: &str) -> Result<Snapshot, String> {
    if raw.len() > LIMIT {
        return Err("云端文件过大".into());
    }
    let envelope: Envelope = serde_json::from_slice(raw).map_err(|_| "云端同步文件格式无效")?;
    if envelope.version != 1 || envelope.iterations != ITERATIONS {
        return Err("不支持此同步文件版本或加密参数".into());
    }
    let decode = |s: &str| {
        STANDARD
            .decode(s)
            .map_err(|_| "云端加密文件损坏".to_string())
    };
    let salt = decode(&envelope.salt)?;
    let nonce = decode(&envelope.nonce)?;
    let nonce: [u8; 12] = nonce.try_into().map_err(|_| "云端加密文件损坏")?;
    if salt.len() != 16 {
        return Err("云端加密文件损坏".into());
    }
    let mut ciphertext = decode(&envelope.ciphertext)?;
    let plain = key(password, &salt)?
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(AAD),
            &mut ciphertext,
        )
        .map_err(|_| "同步密码不正确，或云端文件已损坏")?;
    let snapshot: Snapshot = serde_json::from_slice(plain).map_err(|_| "云端配置内容无效")?;
    snapshot.validate()?;
    Ok(snapshot)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Sync,
    Upload,
    Download,
}
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Upload,
    Download,
    Equal,
    Conflict,
}
fn decide(local: &str, remote: Option<&str>, base: Option<&str>, mode: Mode) -> Decision {
    if mode == Mode::Upload {
        return Decision::Upload;
    }
    if mode == Mode::Download {
        return if remote.is_some() {
            Decision::Download
        } else {
            Decision::Conflict
        };
    }
    match remote {
        Some(remote) if remote == local => Decision::Equal,
        None if base.is_none() => Decision::Upload,
        Some(remote) if base == Some(remote) => Decision::Upload,
        Some(_) if base == Some(local) => Decision::Download,
        _ => Decision::Conflict,
    }
}
pub(crate) struct Outcome {
    pub snapshot: Option<Snapshot>,
    pub fingerprint: String,
    pub note: String,
}
fn network_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(401 | 403, _) => "WebDAV 登录失败，请检查用户名和应用密码".into(),
        ureq::Error::Status(404, _) => "WebDAV 文件夹不存在，请先在云盘中创建文件夹".into(),
        ureq::Error::Status(409 | 412, _) => "云端配置已变化或文件夹不存在，请重新同步".into(),
        ureq::Error::Status(code, _) => format!("WebDAV 请求失败（HTTP {code}）"),
        _ => "无法连接 WebDAV，请检查网络、地址和证书".into(),
    }
}
pub(crate) fn run(
    cfg: &Config,
    webdav_password: &str,
    password: &str,
    local: Snapshot,
    base: Option<String>,
    mode: Mode,
) -> Result<Outcome, String> {
    let local=local.scoped(cfg.mail_passwords);
    transfer_scoped(
        &cfg.target()?,
        cfg.username.trim(),
        webdav_password,
        password,
        local,
        base,
        mode,
        cfg.mail_passwords,
    )
}
#[cfg(test)]
fn transfer(target:&str,username:&str,webdav_password:&str,password:&str,local:Snapshot,base:Option<String>,mode:Mode)->Result<Outcome,String>{
    transfer_scoped(target,username,webdav_password,password,local,base,mode,true)
}
fn transfer_scoped(
    target: &str,
    username: &str,
    webdav_password: &str,
    password: &str,
    local: Snapshot,
    base: Option<String>,
    mode: Mode,
    include_passwords:bool,
) -> Result<Outcome, String> {
    if password.chars().count() < 12 {
        return Err("同步密码至少需要 12 个字符".into());
    }
    let auth = format!(
        "Basic {}",
        STANDARD.encode(format!("{username}:{webdav_password}"))
    );
    // Do not forward Authorization through redirects, including to a different origin.
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(30))
        .build();
    let remote = match agent.get(target).set("Authorization", &auth).call() {
        Ok(response) => {
            let etag = response.header("ETag").map(str::to_string);
            let mut raw = Vec::new();
            response
                .into_reader()
                .take((LIMIT + 1) as u64)
                .read_to_end(&mut raw)
                .map_err(|_| "云端文件读取失败")?;
            Some((decrypt(&raw, password)?.scoped(include_passwords), etag))
        }
        Err(ureq::Error::Status(404, _)) => None,
        Err(error) => return Err(network_error(error)),
    };
    let local_hash = local.fingerprint()?;
    let remote_hash = remote.as_ref().map(|(s, _)| s.fingerprint()).transpose()?;
    match decide(&local_hash, remote_hash.as_deref(), base.as_deref(), mode) {
        Decision::Equal => Ok(Outcome {
            snapshot: None,
            fingerprint: local_hash,
            note: "无需同步，配置已是最新".into(),
        }),
        Decision::Download => {
            let (snapshot, _) = remote.ok_or("云端还没有配置")?;
            Ok(Outcome {
                fingerprint: snapshot.fingerprint()?,
                snapshot: Some(snapshot),
                note: if include_passwords {"已恢复云端配置和可用邮箱授权码"}else{"已恢复云端配置，未更改本机邮箱授权码"}.into(),
            })
        }
        Decision::Conflict => Err(
            "本机和云端尚未对齐，或两端都有修改。请选择「用本机覆盖云端」或「用云端恢复本机」"
                .into(),
        ),
        Decision::Upload => {
            let mut request = agent
                .put(target)
                .set("Authorization", &auth)
                .set("Content-Type", "application/json");
            if let Some((_, etag)) = &remote {
                let etag = etag
                    .as_deref()
                    .filter(|tag| {
                        !tag.starts_with("W/") && tag.starts_with('"') && tag.ends_with('"')
                    })
                    .ok_or("服务器未提供强 ETag，无法安全更新配置")?;
                request = request.set("If-Match", etag);
            } else {
                request = request.set("If-None-Match", "*");
            }
            let response = request
                .send_bytes(&encrypt(&local, password)?)
                .map_err(network_error)?;
            if !(200..300).contains(&response.status()) {
                return Err("WebDAV 未接受配置上传".into());
            }
            Ok(Outcome {
                snapshot: None,
                fingerprint: local_hash,
                note: if include_passwords {"配置和可用邮箱授权码已加密上传"}else{"配置已加密上传，未上传邮箱授权码"}.into(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Snapshot {
        Snapshot {
            version: 1,
            settings: BTreeMap::from([("skin".into(), "cream".into())]),
            accounts: serde_json::json!([]),
            passwords: BTreeMap::new(),
        }
    }
    #[test]
    fn encryption_authenticates_password_and_tampering() {
        let s = sample();
        let password = "correct horse battery staple";
        let raw = encrypt(&s, password).unwrap();
        assert!(decrypt(&raw, password).unwrap() == s);
        assert!(decrypt(&raw, "wrong password").is_err());
        assert!(!String::from_utf8_lossy(&raw).contains("cream"));
        let mut envelope: Envelope = serde_json::from_slice(&raw).unwrap();
        let mut body = STANDARD.decode(&envelope.ciphertext).unwrap();
        body[0] ^= 1;
        envelope.ciphertext = STANDARD.encode(body);
        assert!(decrypt(&serde_json::to_vec(&envelope).unwrap(), password).is_err());
        assert_ne!(raw, encrypt(&s, password).unwrap());
    }
    #[test]
    fn reconciliation_never_silently_overwrites_conflicts() {
        assert_eq!(decide("a", None, None, Mode::Sync), Decision::Upload);
        assert_eq!(decide("a", Some("b"), None, Mode::Sync), Decision::Conflict);
        assert_eq!(
            decide("b", Some("a"), Some("a"), Mode::Sync),
            Decision::Upload
        );
        assert_eq!(
            decide("a", Some("b"), Some("a"), Mode::Sync),
            Decision::Download
        );
        assert_eq!(
            decide("b", Some("c"), Some("a"), Mode::Sync),
            Decision::Conflict
        );
        assert_eq!(decide("a", None, Some("a"), Mode::Sync), Decision::Conflict);
        assert_eq!(decide("a", Some("a"), None, Mode::Sync), Decision::Equal);
    }
    #[test]
    fn rejects_unsafe_urls_and_does_not_sync_device_paths() {
        let mut cfg = Config {
            folder: "https://dav.example.com/folder".into(),
            username: "user".into(),
            automatic: false,
            mail_passwords: false,
        };
        assert_eq!(
            cfg.target().unwrap(),
            "https://dav.example.com/folder/daozai-sync-v1.json"
        );
        for address in [
            "http://100.107.108.80:6086/",
            "http://192.168.68.100:6086/Daozai/",
            "http://istoreos.shrimp-kanyu.ts.net:6086/",
            "https://192.168.68.100:6086/",
            "http://[::1]:6086/",
        ] {
            cfg.folder = address.into();
            assert_eq!(cfg.target().unwrap(), format!("{address}daozai-sync-v1.json"));
        }
        for bad in [
            "ftp://example.com/",
            "https://user:pass@example.com/",
            "https://example.com/?token=x",
        ] {
            cfg.folder = bad.into();
            assert!(cfg.target().is_err());
        }
        assert_eq!(
            settings_map("display_id=abc\navoid_apps=[]\nskin=cream\n"),
            sample().settings
        );
    }
    #[test]
    fn cloud_sync_allowlist_excludes_meetings_permissions_and_future_fields() {
        let fields=settings_map("skin=cream\nignored_mail_events=private-meeting-title\nmail_sync_operations=1\ncalendar_auto_sync=1\nread_notifications=1\nfuture_secret=private\n");
        assert_eq!(fields,sample().settings);
        let mut legacy=sample();
        legacy.settings.insert("mail_sync_operations".into(),"1".into());
        legacy.settings.insert("ignored_mail_events".into(),"private".into());
        legacy.accounts=serde_json::json!([{"email":"test@example.invalid","provider":"qq"}]);
        legacy.passwords.insert("test@example.invalid".into(),"not-for-upload".into());
        let filtered=legacy.scoped(false);
        assert_eq!(filtered.settings,sample().settings);
        assert!(filtered.passwords.is_empty());
    }
    #[test]
    fn invalid_mail_bundles_and_settings_are_rejected() {
        let mut s = sample();
        s.accounts = serde_json::json!([{"email":"me@example.com","provider":"custom"}]);
        assert!(s.validate().is_err());
        s.accounts = serde_json::json!([{"email":"me@qq.com","provider":"qq"}]);
        assert!(s.validate().is_ok());
        s.passwords.insert("me@qq.com".into(), "secret".into());
        assert!(s.validate().is_ok());
        s.settings
            .insert("skin".into(), "cream\nmail_enabled=1".into());
        assert!(s.validate().is_err());
    }
    fn server(responses: Vec<(String, Vec<u8>)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}/sync.json", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut headers = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap();
                        }
                    }
                    headers.push_str(&line);
                }
                let mut request_body = vec![0; length];
                reader.read_exact(&mut request_body).unwrap();
                if !request_body.is_empty() {
                    assert!(decrypt(&request_body, "test-password-123").is_ok());
                }
                requests.push(headers);
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
            requests
        });
        (address, thread)
    }
    #[test]
    fn webdav_create_and_update_use_conditional_writes() {
        let (url, thread) = server(vec![
            ("404 Not Found".into(), vec![]),
            ("201 Created".into(), vec![]),
        ]);
        assert!(transfer(
            &url,
            "user",
            "app-secret",
            "test-password-123",
            sample(),
            None,
            Mode::Sync
        )
        .is_ok());
        let requests = thread.join().unwrap();
        assert!(requests[1]
            .to_ascii_lowercase()
            .contains("if-none-match: *"));
        assert!(requests[0]
            .to_ascii_lowercase()
            .contains("authorization: basic "));
        let old = sample();
        let base = old.fingerprint().unwrap();
        let mut new = sample();
        new.settings.insert("skin".into(), "classic".into());
        let (url, thread) = server(vec![
            (
                "200 OK\r\nETag: \"revision-1\"".into(),
                encrypt(&old, "test-password-123").unwrap(),
            ),
            ("412 Precondition Failed".into(), vec![]),
        ]);
        assert!(transfer(
            &url,
            "user",
            "app-secret",
            "test-password-123",
            new,
            Some(base),
            Mode::Sync
        )
        .is_err());
        assert!(thread.join().unwrap()[1]
            .to_ascii_lowercase()
            .contains("if-match: \"revision-1\""));
    }
    #[test]
    fn webdav_download_and_wrong_password_never_upload() {
        let mut remote = sample();
        remote.settings.insert("skin".into(), "classic".into());
        let (url, thread) = server(vec![(
            "200 OK".into(),
            encrypt(&remote, "test-password-123").unwrap(),
        )]);
        let local = sample();
        let base = local.fingerprint().unwrap();
        let outcome = transfer(
            &url,
            "user",
            "app-secret",
            "test-password-123",
            local,
            Some(base),
            Mode::Sync,
        )
        .unwrap();
        assert!(outcome.snapshot.unwrap() == remote);
        assert_eq!(thread.join().unwrap().len(), 1);
        let (url, thread) = server(vec![(
            "200 OK".into(),
            encrypt(&sample(), "different-password").unwrap(),
        )]);
        assert!(transfer(
            &url,
            "user",
            "app-secret",
            "test-password-123",
            sample(),
            None,
            Mode::Upload
        )
        .is_err());
        assert_eq!(thread.join().unwrap().len(), 1);
    }
}
