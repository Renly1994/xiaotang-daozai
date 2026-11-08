//! 同步主数据与事务快照。ICS 正文是主数据，界面对象只是投影。
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SCHEMA: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub schema: u32,
    pub generation: u64,
    pub previous_generation: Option<u64>,
    pub accounts: Vec<crate::calendar::Config>,
    pub active_provider: crate::calendar::Provider,
    pub local_events: Vec<crate::local_calendar::Entry>,
    pub todos: Vec<crate::model::TodoItem>,
    pub binding: Option<Binding>,
    pub collections: BTreeMap<String, CollectionState>,
    pub resources: BTreeMap<String, Resource>,
    pub pending: Vec<PendingChange>,
    pub conflicts: Vec<Conflict>,
    pub metadata: BTreeMap<String, LocalMetadata>,
    pub migrated: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            generation: 0,
            previous_generation: None,
            accounts: vec![],
            active_provider: Default::default(),
            local_events: vec![],
            todos: vec![],
            binding: None,
            collections: BTreeMap::new(),
            resources: BTreeMap::new(),
            pending: vec![],
            conflicts: vec![],
            metadata: BTreeMap::new(),
            migrated: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Binding {
    pub account_id: String,
    pub generation: u64,
    pub event_collection: String,
    pub todo_collection: String,
    /// 初始化/模式切换先阻止新写请求；已发请求仍须确认结果。
    pub switching: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct CollectionState {
    pub url: String,
    pub events: bool,
    pub todos: bool,
    pub writable: bool,
    pub full_scan: bool,
    pub sync_token: Option<String>,
    pub last_sync: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resource {
    pub id: String,
    pub account_id: String,
    pub collection_url: String,
    pub href: String,
    pub etag: Option<String>,
    /// None 是本机删除意图，不等于已确认的服务端删除。
    pub local_ics: Option<String>,
    pub baseline: Option<String>,
    pub revision: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SendState {
    Ready,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingChange {
    pub id: String,
    pub resource_id: String,
    pub revision: u64,
    pub base_etag: Option<String>,
    pub baseline: Option<String>,
    pub content: Option<String>,
    pub state: SendState,
    pub attempts: u32,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Conflict {
    pub id: String,
    pub resource_id: String,
    pub baseline: Option<String>,
    pub local: Option<String>,
    pub remote: Option<String>,
    pub remote_etag: Option<String>,
    pub revision: u64,
    /// 自动选择也保留被替换内容供用户检查。
    pub resolved: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LocalMetadata {
    pub reminded: bool,
    pub reminded_on: Option<String>,
    pub previous_status: Option<String>,
    pub previous_percent: Option<u8>,
}

pub fn resource_key(account: &str, collection: &str, href: &str) -> String {
    // JSON tuple 避免 URL/账号里分隔符导致碰撞。
    serde_json::to_string(&(account, collection, href)).expect("字符串可以序列化")
}
