//! 面板 IPC 契约：消息信封、状态序列化、命令解析与校验。
//!
//! 全部是纯函数，不碰 WebView2 和 App，方便单测。

use serde_json::{json, Value};

/// 协议版本。前端发来的消息版本不符一律丢弃。
pub const VERSION: u64 = 1;

/// 统一信封：`{v, type, seq, payload}`。
pub struct Envelope;

impl Envelope {
    /// 把下行消息包成统一信封。名字不叫 `new`：它返回的是 `Value` 而不是自建类型。
    pub fn wrap(kind: &str, payload: &impl serde::Serialize) -> Value {
        json!({ "v": VERSION, "type": kind, "payload": payload })
    }
}

#[derive(Clone, Debug, Default)]
pub struct StateAccount {
    pub email: String,
    pub note: String,
    pub tag: String,
    pub color: [u8; 3],
    pub provider: String,
}

#[derive(Clone, Debug, Default)]
pub struct StateItem {
    pub uid: u32,
    pub account: String,
    pub from: String,
    pub subject: String,
    pub time: String,
    pub unread: bool,
    pub code: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub accounts: Vec<StateAccount>,
    pub folder: String,
    pub unread_only: bool,
    pub ready: bool,
    pub syncing: bool,
    /// 当前是否放行远程图片（阅读页的「显示图片」开关状态）。
    pub remote_images: bool,
    /// 岛仔当前外观：深色 = true。外壳（列表/工具栏）跟着它换深、浅两套配色，
    /// 正文主题另外由「完整视图主题」决定（可在设置里选「跟随岛仔」）。
    pub theme_dark: bool,
    pub items: Vec<StateItem>,
    pub notice: String,
}

impl State {
    /// 内容指纹：面板每帧都会调一次 [`State`] 构建，先用指纹比一下，
    /// 没变就直接跳过 JSON 序列化（一百封邮件约 20KB，60fps 下这笔开销不小）。
    ///
    /// 覆盖所有会进入 `to_value()` 的字段，漏一个就会出现「界面不刷新」。
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.folder.hash(&mut hasher);
        self.unread_only.hash(&mut hasher);
        self.ready.hash(&mut hasher);
        self.syncing.hash(&mut hasher);
        self.remote_images.hash(&mut hasher);
        self.theme_dark.hash(&mut hasher);
        self.notice.hash(&mut hasher);
        self.accounts.len().hash(&mut hasher);
        for account in &self.accounts {
            account.email.hash(&mut hasher);
            account.note.hash(&mut hasher);
            account.tag.hash(&mut hasher);
            account.color.hash(&mut hasher);
            account.provider.hash(&mut hasher);
        }
        self.items.len().hash(&mut hasher);
        for item in &self.items {
            item.uid.hash(&mut hasher);
            item.account.hash(&mut hasher);
            item.from.hash(&mut hasher);
            item.subject.hash(&mut hasher);
            item.time.hash(&mut hasher);
            item.unread.hash(&mut hasher);
            item.code.hash(&mut hasher);
        }
        hasher.finish()
    }

    pub fn to_value(&self) -> Value {
        let accounts: Vec<Value> = self
            .accounts
            .iter()
            .map(|a| {
                json!({
                    "email": a.email,
                    "note": a.note,
                    "tag": a.tag,
                    "color": format!("#{:02x}{:02x}{:02x}", a.color[0], a.color[1], a.color[2]),
                    "provider": a.provider,
                })
            })
            .collect();
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|i| {
                json!({
                    "uid": i.uid,
                    "account": i.account,
                    "from": i.from,
                    "subject": i.subject,
                    "time": i.time,
                    "unread": i.unread,
                    "code": i.code,
                })
            })
            .collect();
        json!({
            "accounts": accounts,
            "folder": self.folder,
            "unread_only": self.unread_only,
            "ready": self.ready,
            "syncing": self.syncing,
            "remote_images": self.remote_images,
            "theme_dark": self.theme_dark,
            "items": items,
            "notice": self.notice,
        })
    }
}

/// 前端 → Rust 的命令。
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Ready,
    Refresh,
    Folder { trash: bool },
    Filter { unread_only: bool },
    Open { uid: u32, account: String },
    Close,
    Flag { uid: u32, account: String, unread: bool },
    Move { uid: u32, account: String, to_trash: bool },
    Copy { text: String },
    OpenLink { url: String },
    /// 阅读页的「显示远程图片」开关。
    Remote { on: bool },
    /// 「信任这个发件人」：写进白名单，以后它的邮件自动放行远程图。
    /// 目标邮件取 [`super::current`]，前端不用（也不该）自己报 uid。
    Trust,
    /// 「浏览器打开完整排版」：面板外的 Edge 窗口，和详情卡的按钮同一套实现。
    External { uid: u32, account: String },
    Settings,
}

impl Command {
    /// 解析并校验一条消息。任何不合法都返回错误，调用方只记日志。
    pub fn parse(raw: &str) -> Result<Command, String> {
        let value: Value = serde_json::from_str(raw).map_err(|e| format!("不是合法 JSON：{e}"))?;
        let version = value.get("v").and_then(Value::as_u64).unwrap_or(0);
        if version != VERSION {
            return Err(format!("协议版本不符：{version}"));
        }
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| "缺少 type".to_string())?;
        let payload = value.get("payload").cloned().unwrap_or(Value::Null);
        match kind {
            "ready" => Ok(Command::Ready),
            "refresh" => Ok(Command::Refresh),
            "close" => Ok(Command::Close),
            "settings" => Ok(Command::Settings),
            "folder" => {
                let name = payload.get("name").and_then(Value::as_str).unwrap_or("inbox");
                Ok(Command::Folder { trash: name == "trash" })
            }
            "filter" => Ok(Command::Filter {
                unread_only: payload.get("unread_only").and_then(Value::as_bool).unwrap_or(false),
            }),
            "open" => Ok(Command::Open {
                uid: uid_of(&payload)?,
                account: account_of(&payload)?,
            }),
            "flag" => Ok(Command::Flag {
                uid: uid_of(&payload)?,
                account: account_of(&payload)?,
                unread: payload.get("unread").and_then(Value::as_bool).unwrap_or(false),
            }),
            "move" => Ok(Command::Move {
                uid: uid_of(&payload)?,
                account: account_of(&payload)?,
                to_trash: payload.get("to").and_then(Value::as_str).unwrap_or("trash") == "trash",
            }),
            "copy" => {
                let text = payload.get("text").and_then(Value::as_str).unwrap_or_default();
                if text.is_empty() || text.chars().count() > 64 {
                    return Err("复制内容为空或过长".into());
                }
                Ok(Command::Copy { text: text.to_string() })
            }
            "open_link" => {
                let url = payload.get("url").and_then(Value::as_str).unwrap_or_default();
                // 只放行三种协议，杜绝 file:/javascript: 之类的本地入口。
                if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:")) {
                    return Err(format!("链接协议不允许：{}", url.chars().take(32).collect::<String>()));
                }
                Ok(Command::OpenLink { url: url.to_string() })
            }
            "trust" => Ok(Command::Trust),
            "remote" => Ok(Command::Remote {
                on: payload.get("on").and_then(Value::as_bool).unwrap_or(false),
            }),
            "external" => Ok(Command::External {
                uid: uid_of(&payload)?,
                account: account_of(&payload)?,
            }),
            other => Err(format!("未知命令：{other}")),
        }
    }
}

/// 把「正文里点了一个链接」包装成前端命令 JSON。
///
/// WebView2 的导航拦截只能拿到原始 URI；让它走和页面同一条 `Command::parse`
/// 入口，才不会出现绕过协议白名单的旁路。
pub fn link_command(url: &str) -> String {
    Envelope::wrap("open_link", &json!({ "url": url })).to_string()
}

fn uid_of(payload: &Value) -> Result<u32, String> {
    payload
        .get("uid")
        .and_then(Value::as_u64)
        .and_then(|uid| u32::try_from(uid).ok())
        .ok_or_else(|| "缺少 uid".to_string())
}

fn account_of(payload: &Value) -> Result<String, String> {
    let account = payload.get("account").and_then(Value::as_str).unwrap_or_default();
    if account.is_empty() || account.chars().count() > 96 {
        return Err("账号字段为空或过长".into());
    }
    Ok(account.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_state() -> State {
        State {
            accounts: vec![StateAccount { email: "a@b.c".into(), note: "工作".into(), tag: "工作".into(), color: [1, 2, 3], provider: "QQ 邮箱".into() }],
            folder: "inbox".into(),
            unread_only: false,
            ready: true,
            syncing: false,
            remote_images: false,
            theme_dark: true,
            items: vec![StateItem { uid: 7, account: "a@b.c".into(), from: "GitHub".into(), subject: "验证码".into(), time: "09:41".into(), unread: true, code: Some("123456".into()) }],
            notice: String::new(),
        }
    }

    /// 指纹漏一个字段，界面上就会出现「点了没反应」；这里把每个会影响渲染的
    /// 字段单独改一次，指纹都必须跟着变。
    #[test]
    fn state_fingerprint_tracks_every_visible_field() {
        let base = sample_state();
        assert_eq!(base.fingerprint(), sample_state().fingerprint(), "内容相同指纹必须相同");
        let mutations: Vec<(&str, Box<dyn Fn(&mut State)>)> = vec![
            ("folder", Box::new(|s| s.folder = "trash".into())),
            ("unread_only", Box::new(|s| s.unread_only = true)),
            ("ready", Box::new(|s| s.ready = false)),
            ("syncing", Box::new(|s| s.syncing = true)),
            ("remote_images", Box::new(|s| s.remote_images = true)),
            ("theme_dark", Box::new(|s| s.theme_dark = false)),
            ("notice", Box::new(|s| s.notice = "已同步".into())),
            ("accounts", Box::new(|s| s.accounts[0].note = "个人".into())),
            ("account color", Box::new(|s| s.accounts[0].color = [9, 9, 9])),
            ("items", Box::new(|s| s.items[0].unread = false)),
            ("item subject", Box::new(|s| s.items[0].subject = "改了".into())),
            ("item code", Box::new(|s| s.items[0].code = None)),
            ("item order", Box::new(|s| s.items.push(StateItem { uid: 8, ..s.items[0].clone() }))),
        ];
        for (name, mutate) in mutations {
            let mut state = sample_state();
            mutate(&mut state);
            assert_ne!(base.fingerprint(), state.fingerprint(), "改 {name} 之后指纹没变，界面不会刷新");
        }
    }

    #[test]
    fn ready_and_refresh_parse() {
        assert_eq!(Command::parse(r#"{"v":1,"type":"ready"}"#).unwrap(), Command::Ready);
        assert_eq!(Command::parse(r#"{"v":1,"type":"refresh"}"#).unwrap(), Command::Refresh);
    }

    #[test]
    fn wrong_version_is_rejected() {
        assert!(Command::parse(r#"{"v":2,"type":"ready"}"#).is_err());
        assert!(Command::parse(r#"{"type":"ready"}"#).is_err());
    }

    #[test]
    fn open_requires_uid_and_account() {
        assert!(Command::parse(r#"{"v":1,"type":"open","payload":{"account":"a@b.c"}}"#).is_err());
        assert!(Command::parse(r#"{"v":1,"type":"open","payload":{"uid":3}}"#).is_err());
        assert_eq!(
            Command::parse(r#"{"v":1,"type":"open","payload":{"uid":3,"account":"a@b.c"}}"#).unwrap(),
            Command::Open { uid: 3, account: "a@b.c".into() }
        );
    }

    #[test]
    fn unsafe_link_schemes_are_rejected() {
        for url in ["file:///C:/Windows/System32/cmd.exe", "javascript:alert(1)", "data:text/html,x"] {
            let raw = format!(r#"{{"v":1,"type":"open_link","payload":{{"url":"{url}"}}}}"#);
            assert!(Command::parse(&raw).is_err(), "{url} 不该被放行");
        }
        let ok = r#"{"v":1,"type":"open_link","payload":{"url":"https://example.com/a"}}"#;
        assert_eq!(
            Command::parse(ok).unwrap(),
            Command::OpenLink { url: "https://example.com/a".into() }
        );
    }

    #[test]
    fn remote_toggle_parses() {
        assert_eq!(
            Command::parse(r#"{"v":1,"type":"remote","payload":{"on":true}}"#).unwrap(),
            Command::Remote { on: true }
        );
        // 缺字段按关闭处理，绝不默认放开远程图。
        assert_eq!(
            Command::parse(r#"{"v":1,"type":"remote"}"#).unwrap(),
            Command::Remote { on: false }
        );
    }

    #[test]
    fn external_open_requires_uid_and_account() {
        assert!(Command::parse(r#"{"v":1,"type":"external","payload":{"uid":1}}"#).is_err());
        assert_eq!(
            Command::parse(r#"{"v":1,"type":"external","payload":{"uid":9,"account":"a@b.c"}}"#).unwrap(),
            Command::External { uid: 9, account: "a@b.c".into() }
        );
    }

    #[test]
    fn trust_needs_no_payload_and_unknown_types_fail() {
        // 目标邮件由 Rust 侧的 current() 决定，前端不该能指定「信任谁」。
        assert!(matches!(Command::parse(r#"{"v":1,"type":"trust"}"#), Ok(Command::Trust)));
        assert!(matches!(Command::parse(r#"{"v":1,"type":"trust","payload":{}}"#), Ok(Command::Trust)));
        assert!(Command::parse(r#"{"v":1,"type":"trust_from","payload":{}}"#).is_err());
    }

    #[test]
    fn link_command_goes_through_same_whitelist() {
        let ok = link_command("https://example.com/x?y=1");
        assert!(matches!(Command::parse(&ok), Ok(Command::OpenLink { .. })));
        let bad = link_command("javascript:alert(1)");
        assert!(Command::parse(&bad).is_err());
        let local = link_command("file:///C:/Windows/System32/calc.exe");
        assert!(Command::parse(&local).is_err());
    }

    #[test]
    fn state_serializes_tags_and_colors() {
        let state = State {
            accounts: vec![StateAccount {
                email: "a@qq.com".into(),
                note: "工作号".into(),
                tag: "工作".into(),
                color: [108, 168, 224],
                provider: "QQ 邮箱".into(),
            }],
            folder: "inbox".into(),
            unread_only: true,
            ready: true,
            syncing: false,
            remote_images: false,
            theme_dark: true,
            items: vec![StateItem {
                uid: 7,
                account: "a@qq.com".into(),
                from: "GitHub".into(),
                subject: "登录验证码".into(),
                time: "11-03 09:41".into(),
                unread: true,
                code: Some("274630".into()),
            }],
            notice: String::new(),
        };
        let value = state.to_value();
        assert_eq!(value["accounts"][0]["color"], "#6ca8e0");
        assert_eq!(value["items"][0]["uid"], 7);
        assert_eq!(value["unread_only"], true);
        assert_eq!(value["remote_images"], false);
    }

    #[test]
    fn envelope_carries_version() {
        let value = Envelope::wrap("state", &json!({"x":1}));
        assert_eq!(value["v"], 1);
        assert_eq!(value["type"], "state");
    }
}
