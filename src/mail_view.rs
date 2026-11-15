//! 邮件阅读层的共享规则：显示模式、排版令牌、账号标签展示与配色。
//!
//! 列表、悬停预览、详情卡、全文窗口与「完整排版」网页都从这里取值，
//! 保证同一封邮件在四个界面里是同一套字号、间距与账号称呼。

use crate::mail;
use eframe::egui::Color32;
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// 正文显示模式：用户可在设置里选，也能在阅读窗口临时切换。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// 尽量保留邮件自己的字号、颜色、表格。
    Fidelity,
    /// 统一版心、字号阶梯、行高与段距，布局表摊平。
    #[default]
    Comfort,
    /// 去装饰，只留标题、正文、链接与图片。
    Minimal,
}

impl ViewMode {
    pub fn from_u8(v: u8) -> Self {
        match v { 0 => Self::Fidelity, 2 => Self::Minimal, _ => Self::Comfort }
    }
    pub fn as_u8(self) -> u8 {
        match self { Self::Fidelity => 0, Self::Comfort => 1, Self::Minimal => 2 }
    }
    pub fn label(self) -> &'static str {
        match self { Self::Fidelity => "原样", Self::Comfort => "舒适", Self::Minimal => "极简" }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Self::Fidelity => "保留邮件原本的表格、字号与配色",
            Self::Comfort => "统一字号与留白，读起来更轻松",
            Self::Minimal => "去掉装饰，只留正文、链接与图片",
        }
    }
}

/// 「完整排版」网页主题。
///
/// 存储值是追加式的：0/1/2 是早期版本已有的 纸张白/柔和米/深色，
/// 3 是后加的「跟随岛仔」。老配置读出来必须还是原来的主题，禁止重排。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WebTheme {
    Paper,
    Sepia,
    Dark,
    /// 跟随岛仔外观（设置里的深/浅色）：深色→深色，浅色→纸张白。
    /// 只是「意图」，不能进渲染——使用点必须先 [`WebTheme::resolve`]。
    #[default]
    Follow,
}

impl WebTheme {
    pub fn from_u8(v: u8) -> Self {
        match v { 1 => Self::Sepia, 2 => Self::Dark, 3 => Self::Follow, _ => Self::Paper }
    }
    pub fn as_u8(self) -> u8 {
        match self { Self::Paper => 0, Self::Sepia => 1, Self::Dark => 2, Self::Follow => 3 }
    }
    pub fn label(self) -> &'static str {
        match self { Self::Follow => "跟随岛仔", Self::Paper => "纸张白", Self::Sepia => "柔和米", Self::Dark => "深色" }
    }
    /// 把「跟随岛仔」解析成具体主题；其余选项原样返回。
    /// 所有把主题交给渲染层的入口都要先过这一步。
    pub fn resolve(self, dark: bool) -> Self {
        match self {
            Self::Follow => if dark { Self::Dark } else { Self::Paper },
            other => other,
        }
    }
    pub fn palette(self) -> (&'static str, &'static str, &'static str, &'static str, &'static str) {
        // (页面底, 卡片底, 正文字, 边框, 链接)
        // Follow 走到这里说明上游漏了 resolve；按浅色兜底，不 panic。
        match self.resolve(false) {
            Self::Paper => ("#f2f3f5", "#ffffff", "#1f2328", "#d6dbe3", "#0b57d0"),
            Self::Sepia => ("#efebe2", "#fbfaf7", "#26221c", "#ddd5c4", "#8a5a20"),
            Self::Dark  => ("#151518", "#1c1c20", "#e6e6ea", "#34343a", "#7fc4ff"),
            Self::Follow => unreachable!("resolve 之后不再有 Follow"),
        }
    }
}

/// 排版令牌：替换散落在 mail_reader.rs 里的魔法数字。
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub body_size: f32,
    pub line_body: f32,
    pub line_heading: f32,
    pub gap_same: f32,
    pub gap_level: f32,
    pub gap_block: f32,
    pub image_gap: f32,
    pub decoration: bool,
    pub uniform_size: bool,
    pub uniform_color: bool,
}

pub fn tokens(mode: ViewMode) -> Tokens {
    match mode {
        ViewMode::Fidelity => Tokens {
            body_size: 15.0, line_body: 1.55, line_heading: 1.5,
            gap_same: 3.0, gap_level: 6.0, gap_block: 10.0, image_gap: 4.0,
            decoration: true, uniform_size: false, uniform_color: false,
        },
        ViewMode::Comfort => Tokens {
            body_size: 14.5, line_body: 1.65, line_heading: 1.3,
            gap_same: 10.0, gap_level: 16.0, gap_block: 24.0, image_gap: 8.0,
            decoration: true, uniform_size: false, uniform_color: false,
        },
        ViewMode::Minimal => Tokens {
            body_size: 14.5, line_body: 1.7, line_heading: 1.35,
            gap_same: 9.0, gap_level: 14.0, gap_block: 20.0, image_gap: 8.0,
            decoration: false, uniform_size: true, uniform_color: true,
        },
    }
}

/// 服务商的两字代号，供无备注的账号使用。
fn short_provider(p: mail::Provider) -> &'static str {
    use mail::Provider::*;
    match p {
        Qq => "QQ", Mail163 => "163", Mail126 => "126", Exmail => "企邮",
        ICloud => "苹果", Gmail => "谷歌", Microsoft => "微软", Custom => "自定",
    }
}

// ---------------------------------------------------------------------------
// 账号标签配色：一组饱和度/明度接近的柔和色，深色界面上清晰但不刺眼。
// 用户可在设置里为每个邮箱指定颜色，覆盖写进 mail-tag-colors.json。
// ---------------------------------------------------------------------------
pub const PALETTE: [Color32; 8] = [
    Color32::from_rgb(108, 168, 224), // 雾蓝
    Color32::from_rgb( 96, 186, 170), // 青绿
    Color32::from_rgb(168, 150, 220), // 藕紫
    Color32::from_rgb(224, 166, 116), // 暖橙
    Color32::from_rgb(216, 140, 156), // 玫红
    Color32::from_rgb(108, 196, 208), // 湖蓝
    Color32::from_rgb(152, 190, 132), // 苔绿
    Color32::from_rgb(160, 170, 190), // 银灰
];

pub const PALETTE_NAMES: [&str; 8] = ["雾蓝", "青绿", "藕紫", "暖橙", "玫红", "湖蓝", "苔绿", "银灰"];

fn colors_path() -> std::path::PathBuf {
    crate::model::config_dir().join("mail-tag-colors.json")
}

/// 进程内缓存：列表每帧都要取色，不能每次读盘。
fn color_cache() -> &'static RwLock<HashMap<String, usize>> {
    static CACHE: OnceLock<RwLock<HashMap<String, usize>>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let map = std::fs::read_to_string(colors_path())
            .ok()
            .and_then(|s| serde_json::from_str::<HashMap<String, usize>>(&s).ok())
            .unwrap_or_default();
        RwLock::new(map)
    })
}

/// 某个邮箱是否由用户手工指定了色号；None 表示「按邮箱自动」。
pub fn color_override(account: &str) -> Option<usize> {
    color_cache()
        .read()
        .ok()
        .and_then(|map| map.get(&account.to_ascii_lowercase()).copied())
        .filter(|i| *i < PALETTE.len())
}

/// 设置或清除账号标签颜色；None 恢复按邮箱哈希自动取色。
pub fn set_color_override(account: &str, index: Option<usize>) {
    let key = account.to_ascii_lowercase();
    let snapshot = {
        let Ok(mut map) = color_cache().write() else { return };
        match index {
            Some(i) if i < PALETTE.len() => { map.insert(key, i); }
            _ => { map.remove(&key); }
        }
        map.clone()
    };
    if let Ok(text) = serde_json::to_string(&snapshot) {
        let _ = std::fs::write(colors_path(), text);
    }
}

pub fn color_of(account: &str) -> Color32 {
    if let Some(i) = color_override(account) {
        return PALETTE[i];
    }
    let mut hash = 2166136261u32;
    for byte in account.to_ascii_lowercase().as_bytes() {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    PALETTE[(hash as usize) % PALETTE.len()]
}

/// 列表左侧的彩色标签：只放两字代号，永不截断。
pub fn chip(accounts: &[mail::Account], account: &str) -> (String, Color32) {
    let Some(a) = accounts.iter().find(|a| a.email.eq_ignore_ascii_case(account)) else {
        return ("邮箱".into(), color_of(account));
    };
    let note = a.note.trim();
    let label = if note.is_empty() {
        short_provider(a.provider).to_string()
    } else {
        note.chars().take(2).collect()
    };
    (label, color_of(account))
}

/// 预览卡专用：完整标签文字（备注全名，不截断）+ 完整邮箱地址。
/// 返回 (标签文字, 颜色, 完整邮箱, 服务商名)。
pub fn full_tag(accounts: &[mail::Account], account: &str) -> (String, Color32, String, String) {
    let a = accounts.iter().find(|a| a.email.eq_ignore_ascii_case(account));
    let provider = a.map(|a| a.provider.label().to_string()).unwrap_or_else(|| "邮箱".into());
    let note = a.map(|a| a.note.trim().to_string()).unwrap_or_default();
    let label = if note.is_empty() { provider.clone() } else { note };
    (label, color_of(account), account.to_string(), provider)
}

/// 中段省略，保留邮箱头部与域名。
pub fn shorten(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max { return text.to_string(); }
    if max < 4 { return chars[..max].iter().collect(); }
    let head = max / 2;
    let tail = max - head - 1;
    let left: String = chars[..head].iter().collect();
    let right: String = chars[chars.len() - tail..].iter().collect();
    format!("{left}…{right}")
}

/// 统一的「备注 · 邮箱」副行。空备注时只返回邮箱。
pub fn account_line(accounts: &[mail::Account], account: &str, max: usize) -> (Option<String>, String) {
    let note = accounts.iter()
        .find(|a| a.email.eq_ignore_ascii_case(account))
        .map(|a| a.note.trim().to_string())
        .unwrap_or_default();
    let email = shorten(account, max);
    (if note.is_empty() { None } else { Some(note) }, email)
}

/// 发件人域名，用于「始终信任此发件人」的键。
pub fn sender_domain(from_addr: &str) -> String {
    let addr = from_addr.trim().trim_start_matches('<').trim_end_matches('>');
    addr.rsplit('@').next().unwrap_or(addr).to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chip_never_exceeds_two_characters() {
        let accounts = vec![mail::Account {
            custom: None, email: "a@163.com".into(), provider: mail::Provider::Mail163,
            enabled: true, note: "岛仔演示账号".into(),
        }];
        let (label, _) = chip(&accounts, "a@163.com");
        assert_eq!(label.chars().count(), 2);
        assert_eq!(label, "岛仔");
    }

    #[test]
    fn chip_falls_back_to_provider_short_name() {
        let accounts = vec![mail::Account {
            custom: None, email: "b@qq.com".into(), provider: mail::Provider::Qq,
            enabled: true, note: String::new(),
        }];
        assert_eq!(chip(&accounts, "b@qq.com").0, "QQ");
    }

    #[test]
    fn full_tag_keeps_the_whole_note_and_address() {
        let accounts = vec![mail::Account {
            custom: None, email: "long.address.name@example.com".into(), provider: mail::Provider::Custom,
            enabled: true, note: "我的一个很长的工作邮箱备注".into(),
        }];
        let (label, _, email, provider) = full_tag(&accounts, "long.address.name@example.com");
        assert_eq!(label, "我的一个很长的工作邮箱备注", "预览标签不能截断备注");
        assert_eq!(email, "long.address.name@example.com", "预览要显示完整地址");
        assert!(!provider.is_empty());
    }

    #[test]
    fn palette_is_soft_and_has_editable_entries() {
        assert_eq!(PALETTE.len(), PALETTE_NAMES.len());
        for color in PALETTE {
            let luma = 0.299 * color.r() as f32 + 0.587 * color.g() as f32 + 0.114 * color.b() as f32;
            assert!(luma > 110.0 && luma < 220.0, "深色界面上要清晰但不刺眼: {color:?}");
        }
        // 未设置覆盖时仍按邮箱自动取色，且不同邮箱能取到不同色。
        assert!(color_override("no-such-account@example.com").is_none());
    }

    #[test]
    fn color_override_round_trips_in_memory() {
        let key = "color-override-test@example.com";
        set_color_override(key, None);
        assert_eq!(color_override(key), None);
        set_color_override(key, Some(3));
        assert_eq!(color_override(key), Some(3));
        assert_eq!(color_of(key), PALETTE[3]);
        set_color_override(key, None);
        assert_eq!(color_override(key), None, "清除后回到自动色");
    }

    #[test]
    fn same_note_on_two_accounts_gets_colors() {
        let mk = |email: &str| mail::Account {
            custom: None, email: email.into(), provider: mail::Provider::Mail163,
            enabled: true, note: "工作".into(),
        };
        let accounts = vec![mk("a@163.com"), mk("b@163.com")];
        let (_, c1) = chip(&accounts, "a@163.com");
        let (_, c2) = chip(&accounts, "b@163.com");
        assert_ne!(c1, Color32::TRANSPARENT);
        assert_ne!(c2, Color32::TRANSPARENT);
    }

    #[test]
    fn shorten_keeps_head_and_domain() {
        let s = shorten("someone@example.com", 16);
        assert!(s.contains('…'));
        assert!(s.ends_with("com"));
        assert!(s.chars().count() <= 16);
    }

    #[test]
    fn account_line_omits_separator_when_note_is_empty() {
        let accounts = vec![mail::Account {
            custom: None, email: "a@163.com".into(), provider: mail::Provider::Mail163,
            enabled: true, note: "  ".into(),
        }];
        let (note, email) = account_line(&accounts, "a@163.com", 40);
        assert!(note.is_none());
        assert_eq!(email, "a@163.com");
    }

    #[test]
    fn view_mode_round_trips() {
        for m in [ViewMode::Fidelity, ViewMode::Comfort, ViewMode::Minimal] {
            assert_eq!(ViewMode::from_u8(m.as_u8()), m);
        }
        assert_eq!(ViewMode::from_u8(99), ViewMode::Comfort);
        assert_eq!(ViewMode::default(), ViewMode::Comfort);
    }

    #[test]
    fn web_theme_round_trips_and_follow_resolves() {
        for t in [WebTheme::Follow, WebTheme::Paper, WebTheme::Sepia, WebTheme::Dark] {
            assert_eq!(WebTheme::from_u8(t.as_u8()), t);
        }
        assert_eq!(WebTheme::default(), WebTheme::Follow, "新装默认跟随岛仔外观");
        // Follow 只是意图，解析后必须是具体主题。
        assert_eq!(WebTheme::Follow.resolve(true), WebTheme::Dark);
        assert_eq!(WebTheme::Follow.resolve(false), WebTheme::Paper);
        assert_eq!(WebTheme::Sepia.resolve(true), WebTheme::Sepia);
        // palette 是渲染层的最后兜底：即使 Follow 漏进来也不能 panic。
        assert_eq!(WebTheme::Follow.palette(), WebTheme::Paper.palette());
    }
}
