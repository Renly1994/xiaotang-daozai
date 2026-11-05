//! 账号密码：%USERPROFILE%\claude-hub\secrets.json。
//! 文件里只有密文，格式与 Electron safeStorage 相同（"v10" + 12 字节随机数 + AES-256-GCM 密文）。
//! 密钥由 Windows DPAPI 保护，只有当前 Windows 用户能解开：装过桌面工具 claude-hub 时用它的
//! （%APPDATA%\claude-hub\Local State），否则第一次保存时生成一把（secrets.key）。
//! 只在用户点“显示 / 复制”时解密，明文不写日志、不落盘。
use super::store;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use base64::Engine;
use serde_json::{Map, Value};
use std::path::PathBuf;

fn file() -> PathBuf {
    store::hub_dir().join("secrets.json")
}

fn local_states() -> Vec<PathBuf> {
    let roaming = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    ["claude-hub", "ClaudeHub"].iter().map(|n| roaming.join(n).join("Local State")).filter(|p| p.is_file()).collect()
}

#[cfg(windows)]
fn dpapi(data: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        let r = if encrypt {
            CryptProtectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        } else {
            CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        };
        r.ok()?;
        let out = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData as _));
        Some(out)
    }
}
#[cfg(not(windows))]
fn dpapi(_: &[u8], _: bool) -> Option<Vec<u8>> {
    None
}

fn key_file() -> PathBuf {
    store::hub_dir().join("secrets.key")
}

fn own_key() -> Option<[u8; 32]> {
    let raw = base64::engine::general_purpose::STANDARD.decode(std::fs::read(key_file()).ok()?).ok()?;
    dpapi(&raw, false)?.try_into().ok()
}

fn create_own_key() -> Option<[u8; 32]> {
    let mut key = [0u8; 32];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut key).ok()?;
    let wrapped = dpapi(&key, true)?;
    store::write_atomic(&key_file(), base64::engine::general_purpose::STANDARD.encode(wrapped).as_bytes()).ok()?;
    Some(key)
}

/// 可用的 AES 密钥：claude-hub 的（可能有安装版和开发版两份），以及自己的
fn keys() -> Vec<[u8; 32]> {
    local_states()
        .into_iter()
        .filter_map(|p| {
            let v = store::read_json(&p)?;
            let raw = base64::engine::general_purpose::STANDARD.decode(v["os_crypt"]["encrypted_key"].as_str()?).ok()?;
            let key = dpapi(raw.strip_prefix(b"DPAPI")?, false)?;
            key.try_into().ok()
        })
        .chain(own_key())
        .collect()
}

fn open(key: &[u8; 32], blob: &[u8]) -> Option<String> {
    let body = blob.strip_prefix(b"v10")?;
    if body.len() < 12 + 16 {
        return None;
    }
    let (nonce, data) = body.split_at(12);
    let k = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).ok()?);
    let mut buf = data.to_vec();
    let plain = k.open_in_place(Nonce::try_assume_unique_for_key(nonce).ok()?, Aad::empty(), &mut buf).ok()?;
    String::from_utf8(plain.to_vec()).ok()
}

fn seal(key: &[u8; 32], text: &str) -> Option<Vec<u8>> {
    let mut nonce = [0u8; 12];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut nonce).ok()?;
    let k = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).ok()?);
    let mut buf = text.as_bytes().to_vec();
    k.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut buf).ok()?;
    let mut out = b"v10".to_vec();
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&buf);
    Some(out)
}

fn read_all() -> Map<String, Value> {
    store::read_json(&file()).and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

fn valid(name: &str) -> bool {
    let Some((kind, id)) = name.split_once(':') else { return false };
    matches!(kind, "acc" | "key") && store::valid_name(id)
}

pub fn has(name: &str) -> bool {
    read_all().get(name).is_some_and(|v| v.is_string())
}

/// 解出明文；没有保存或解不开时返回 None
pub fn get(name: &str) -> Option<String> {
    if !valid(name) {
        return None;
    }
    let blob = base64::engine::general_purpose::STANDARD.decode(read_all().get(name)?.as_str()?).ok()?;
    keys().iter().find_map(|k| open(k, &blob))
}

/// 加密保存；空值表示删除。
pub fn set(name: &str, value: &str) -> Result<(), String> {
    if !valid(name) {
        return Err("无效的名字".into());
    }
    let mut all = read_all();
    if value.is_empty() {
        all.remove(name);
    } else {
        let key = keys().into_iter().next().or_else(create_own_key).ok_or("加密失败")?;
        let blob = seal(&key, value).ok_or("加密失败")?;
        all.insert(name.into(), Value::String(base64::engine::general_purpose::STANDARD.encode(blob)));
    }
    let raw = serde_json::to_vec_pretty(&Value::Object(all)).map_err(|_| "保存失败".to_string())?;
    store::write_atomic(&file(), &raw).map_err(|e| format!("保存失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    /// 只打印每一步成没成，不打印密钥或密码
    #[test]
    #[ignore = "按需核对本机密码能否解开"]
    fn real_key_steps() {
        for p in local_states() {
            let v = store::read_json(&p).unwrap_or_default();
            let raw = v["os_crypt"]["encrypted_key"].as_str().map(|s| base64::engine::general_purpose::STANDARD.decode(s));
            let bytes = raw.and_then(|r| r.ok()).unwrap_or_default();
            let key = bytes.strip_prefix(b"DPAPI").and_then(|b| dpapi(b, false));
            println!("{} top_keys={:?} os_crypt_keys={:?} raw_len={} dpapi_prefix={} key_len={:?}", p.display(), v.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()), v["os_crypt"].as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()), bytes.len(), bytes.starts_with(b"DPAPI"), key.as_ref().map(|k| k.len()));
            if let Some(Ok(k)) = key.map(<[u8; 32]>::try_from) {
                if let Some(sample) = std::env::var_os("HUB_SAMPLE") {
                    let text = std::fs::read_to_string(sample).unwrap_or_default();
                    let blob = base64::engine::general_purpose::STANDARD.decode(text.trim()).unwrap_or_default();
                    println!("  electron_fresh_blob_opens_as_probe={}", open(&k, &blob).as_deref() == Some("probe"));
                }
                for (name, value) in read_all() {
                    let blob = base64::engine::general_purpose::STANDARD.decode(value.as_str().unwrap_or_default()).unwrap_or_default();
                    println!("  {name} blob_len={} prefix_ok={} opens={}", blob.len(), blob.starts_with(b"v10"), open(&k, &blob).is_some());
                }
            }
        }
    }
    #[test]
    fn electron_format_round_trips() {
        let key = [7u8; 32];
        let blob = seal(&key, "示例密码-123").unwrap();
        assert!(blob.starts_with(b"v10"));
        assert_eq!(open(&key, &blob).as_deref(), Some("示例密码-123"));
        assert!(open(&[8u8; 32], &blob).is_none(), "换了密钥不能解开");
        assert!(!valid("acc:../x") && valid("acc:work-2"));
    }
    #[cfg(windows)]
    #[test]
    fn works_without_claude_hub_installed() {
        let root = std::env::temp_dir().join(format!("daozai-secrets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let old = (std::env::var_os("CLAUDE_HUB_DIR"), std::env::var_os("APPDATA"));
        std::env::set_var("CLAUDE_HUB_DIR", &root);
        std::env::set_var("APPDATA", root.join("roaming"));
        set("acc:demo", "示例密码").unwrap();
        let wrapped = std::fs::read(key_file()).unwrap();
        assert_eq!(get("acc:demo").as_deref(), Some("示例密码"));
        set("acc:other", "另一个").unwrap();
        assert_eq!(std::fs::read(key_file()).unwrap(), wrapped, "已有密钥时不能重新生成");
        assert!(!String::from_utf8_lossy(&std::fs::read(file()).unwrap()).contains("示例密码"));
        match old.0 { Some(v) => std::env::set_var("CLAUDE_HUB_DIR", v), None => std::env::remove_var("CLAUDE_HUB_DIR") }
        match old.1 { Some(v) => std::env::set_var("APPDATA", v), None => std::env::remove_var("APPDATA") }
        let _ = std::fs::remove_dir_all(root);
    }
}
