//! 账号岛（多账号模式，默认关闭）：多账号切换、会话列表同步和官方额度。
//! 数据在 %USERPROFILE%\claude-hub，格式与独立的桌面工具 claude-hub 相同，但不依赖它。
//!
//! 规则：
//! - 不读取、复制、上传、记录任何凭证、令牌或钥匙串内容；额度只来自官方 `claude -p /usage`
//!   和 Codex 自己会话记录里的 rate_limits，不调用任何内部网页接口。
//! - 会话列表只读写桌面端的 local_*.json 列表条目，写前整份备份、可撤销；桌面端开着时不写。
//! - 日志只记文件名、数量和结果，不记会话内容。
//! - 不做“额度用完自动轮换账号”：切换只在用户点按钮后进行。
pub mod codex;
pub mod desktop;
pub mod heat;
pub mod login;
pub mod proc;
pub mod secrets;
mod service;
pub mod store;
pub mod tasks;
pub mod usage;

pub use service::{AccountView, Alert, Ev, Phase, Req, Service, View};
pub use store::Group;
