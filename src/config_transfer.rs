//! 配置导入/导出：将岛上的各项配置打包、加密、传输。
//!
//! 加密算法：AES-256-GCM + PBKDF2-SHA256 派生密钥。
//! 字符串格式：`DZ1:` + base64(salt(16) || nonce(12) || ciphertext+tag)。
//! 密码可留空（使用固定密钥），此时不需要密码也能解密。

use base64::Engine;
use ring::aead::{AES_256_GCM, LessSafeKey, Nonce, UnboundKey};
use ring::pbkdf2;
use std::num::NonZeroU32;
use std::time::{SystemTime, UNIX_EPOCH};

const HEADER: &[u8; 3] = b"DZ1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const PBKDF2_ITER: NonZeroU32 = unsafe { NonZeroU32::new_unchecked(100_000) };

// 固定密钥（密码为空时使用）；再与 salt 混合避免直接识别。
const FALLBACK_KEY: &[u8; 32] = b"daozai-config-export-v1-fallback";

// --------------------------------------------------------------------- 数据结构

/// 导出包：把岛上的各项配置打包成 JSON，再加密传输。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ExportBundle {
    pub version: u32,
    /// 导出时的 unix timestamp (秒)。
    pub exported_at: u64,
    pub settings: crate::model::Settings,
    pub mail_accounts: Vec<crate::mail::Account>,
    pub calendar_configs: Vec<crate::calendar::Config>,
    pub quota_config: crate::quota::Config,
}

impl ExportBundle {
    /// 从当前运行的岛仔收集全部配置。
    pub fn collect() -> Self {
        Self {
            version: 1,
            exported_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            settings: crate::model::Settings::load(),
            mail_accounts: crate::mail::load_accounts(),
            calendar_configs: crate::calendar::load_configs(),
            quota_config: crate::quota::Config::load(),
        }
    }

    /// 把包里的各项配置写回到岛上（不覆盖密码 / 凭据）。
    pub fn apply(&self) -> Result<(), String> {
        self.settings.save();

        // 邮件账号（密码仍在 Windows 凭据管理器）
        let accounts_json =
            serde_json::to_vec_pretty(&self.mail_accounts).map_err(|e| format!("序列化邮件账号失败: {e}"))?;
        crate::model::atomic_write(
            &crate::model::config_dir().join("mail-accounts.json"),
            &accounts_json,
        )
        .map_err(|e| format!("保存邮件账号失败: {e}"))?;

        // 日历配置
        let cal_path = crate::model::config_dir().join("calendar.json");
        let cal_json = if self.calendar_configs.len() <= 1 {
            self.calendar_configs
                .first()
                .map(|c| serde_json::to_vec_pretty(c))
                .transpose()
                .map_err(|e| format!("序列化日历配置失败: {e}"))?
                .unwrap_or_default()
        } else {
            let json = serde_json::json!({
                "accounts": self
                    .calendar_configs
                    .iter()
                    .filter_map(|c| serde_json::to_value(c).ok())
                    .collect::<Vec<_>>()
            });
            serde_json::to_vec_pretty(&json).unwrap_or_default()
        };
        crate::model::atomic_write(&cal_path, &cal_json)
            .map_err(|e| format!("保存日历配置失败: {e}"))?;

        // AI 额度
        self.quota_config.save();

        Ok(())
    }
}

// --------------------------------------------------------------------- UI 状态

/// 配置导入/导出弹窗状态（由 App 持有）。
#[derive(Clone, Debug, Default)]
pub struct Dialog {
    /// `true` = 导出, `false` = 导入。
    pub export: bool,
    pub password: String,
    /// 导出成功后的加密字符串。
    pub encrypted: String,
    /// 导入时粘贴的加密字符串。
    pub import_input: String,
    /// 过程中的提示 / 错误（空串 = 无提示）。
    pub message: String,
    /// 0 = 等待操作, 1 = 导出成功可复制, 2 = 导入成功。
    pub stage: u8,
}

impl Dialog {
    pub fn open_export() -> Self {
        Self { export: true, ..Default::default() }
    }
    pub fn open_import() -> Self {
        Self { export: false, ..Default::default() }
    }

    /// 执行导出：读取当前配置、加密、存入自身 `encrypted` 字段。
    pub fn do_export(&mut self) -> Result<(), String> {
        let bundle = ExportBundle::collect();
        let s = encrypt(&bundle, &self.password)?;
        self.encrypted = format!("DZ1:{}", s);
        self.message = "导出成功！复制下方字符串到另一台电脑，粘贴即可导入。".into();
        self.stage = 1;
        Ok(())
    }

    /// 执行导入：解密粘贴的字符串、覆盖岛上配置。
    pub fn do_import(&mut self) -> Result<(), String> {
        let input = self.import_input.trim().to_string();
        if input.is_empty() {
            return Err("请粘贴要导入的配置字符串".into());
        }
        // 允许用户粘贴时带/不带 "DZ1:" 前缀。
        let stripped = input.strip_prefix("DZ1:").unwrap_or(&input);
        let bundle = decrypt(stripped, &self.password)?;
        bundle.apply()?;
        self.message = "导入成功！部分模块可能需要重启岛仔才能完全生效。".into();
        self.stage = 2;
        Ok(())
    }
}

// --------------------------------------------------------------------- 密码与密钥

fn derive_key(password: &str, salt: &[u8; SALT_LEN]) -> [u8; 32] {
    let mut key = [0u8; 32];
    if password.is_empty() {
        key.copy_from_slice(FALLBACK_KEY);
        for i in 0..32 {
            key[i] ^= salt[i % SALT_LEN];
        }
    } else {
        pbkdf2::derive(
            pbkdf2::PBKDF2_HMAC_SHA256,
            PBKDF2_ITER,
            salt,
            password.as_bytes(),
            &mut key,
        );
    }
    key
}

fn pseudo_random_bytes(buf: &mut [u8]) {
    // 岛仔是单机桌面应用；导出场景下用时钟 + 简单 PRNG 生成 salt/nonce 足够。
    let mut state = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or_else(|_| 0xdeadbeef);
    for b in buf.iter_mut() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *b = (state >> 56) as u8;
    }
}

// --------------------------------------------------------------------- 加解密

fn make_cipher(key: &[u8; 32]) -> Result<LessSafeKey, String> {
    let unbound = UnboundKey::new(&AES_256_GCM, key).map_err(|e| format!("密钥初始化失败: {e}"))?;
    Ok(LessSafeKey::new(unbound))
}

/// 将导出包序列化为加密字符串。密码可留空（此时无需密码即可解密）。
pub fn encrypt(bundle: &ExportBundle, password: &str) -> Result<String, String> {
    let json = serde_json::to_vec(bundle).map_err(|e| format!("序列化配置失败: {e}"))?;

    let mut salt = [0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    pseudo_random_bytes(&mut salt);
    pseudo_random_bytes(&mut nonce_bytes);

    let key = derive_key(password, &salt);
    let cipher = make_cipher(&key)?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);

    // seal_in_place_append_tag 原地加密并追加 16 字节 tag；先复制 JSON 到缓冲区。
    let mut buffer = json;
    cipher
        .seal_in_place_append_tag(nonce, ring::aead::Aad::empty(), &mut buffer)
        .map_err(|e| format!("加密失败: {e}"))?;

    let mut out = Vec::with_capacity(HEADER.len() + SALT_LEN + NONCE_LEN + buffer.len());
    out.extend_from_slice(HEADER);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&buffer);

    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(out))
}

/// 解密字符串并还原导出包。密码需与导出时一致（导出时留空则密码也留空即可）。
pub fn decrypt(input: &str, password: &str) -> Result<ExportBundle, String> {
    let trimmed = input.trim();
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(trimmed.as_bytes())
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(trimmed.as_bytes()))
        .map_err(|_| "无法解析：不是有效的配置字符串".to_string())?;

    if raw.len() < HEADER.len() + SALT_LEN + NONCE_LEN + 16 {
        return Err("配置字符串长度不足，格式不正确".into());
    }
    if &raw[..HEADER.len()] != HEADER {
        return Err("配置字符串头部不匹配：这不是岛仔导出的配置".into());
    }

    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&raw[HEADER.len()..HEADER.len() + SALT_LEN]);
    let mut nonce_bytes = [0u8; NONCE_LEN];
    nonce_bytes.copy_from_slice(
        &raw[HEADER.len() + SALT_LEN..HEADER.len() + SALT_LEN + NONCE_LEN],
    );
    let mut ciphertext = raw[HEADER.len() + SALT_LEN + NONCE_LEN..].to_vec();

    let key = derive_key(password, &salt);
    let cipher = make_cipher(&key)?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);

    // open_in_place 去掉尾部 tag，原地还原 plaintext。
    let plaintext = cipher
        .open_in_place(nonce, ring::aead::Aad::empty(), &mut ciphertext)
        .map_err(|_| "密码错误或配置已损坏，解密失败".to_string())?;

    serde_json::from_slice(plaintext)
        .map_err(|e| format!("解密成功但无法解析配置：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_no_password() {
        let bundle = ExportBundle {
            version: 1,
            exported_at: 0,
            settings: crate::model::Settings::default(),
            mail_accounts: vec![],
            calendar_configs: vec![],
            quota_config: crate::quota::Config::default(),
        };
        let s = encrypt(&bundle, "").unwrap();
        let back = decrypt(&s, "").unwrap();
        assert_eq!(back.version, 1);
    }

    #[test]
    fn round_trip_with_password() {
        let bundle = ExportBundle {
            version: 1,
            exported_at: 12345,
            settings: crate::model::Settings::default(),
            mail_accounts: vec![],
            calendar_configs: vec![],
            quota_config: crate::quota::Config::default(),
        };
        let s = encrypt(&bundle, "hello world").unwrap();
        let back = decrypt(&s, "hello world").unwrap();
        assert_eq!(back.exported_at, 12345);
    }

    #[test]
    fn wrong_password_fails() {
        let bundle = ExportBundle {
            version: 1,
            exported_at: 0,
            settings: crate::model::Settings::default(),
            mail_accounts: vec![],
            calendar_configs: vec![],
            quota_config: crate::quota::Config::default(),
        };
        let s = encrypt(&bundle, "correct").unwrap();
        assert!(decrypt(&s, "wrong").is_err());
    }

    #[test]
    fn bad_header_fails() {
        assert!(decrypt("AAAA", "").is_err());
    }
}
