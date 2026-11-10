//! CalDAV 设置与完整资源编辑器。网络工作全部交给 Calendar Service。
use crate::app::{App, Cmd};
use crate::calendar::{
    self,
    caldav::{
        dav,
        ical::{
            Alarm, AlarmEdit, AlarmRelation, AlarmTrigger, CalendarDate, CalendarDocument,
            CalendarItem, CommonPatch, DisplayAlarm, EditCapabilities, EventPatch, IcalDuration,
            Patch, TaskPatch, TaskStateChange, TaskStatus,
        },
        projection,
    },
    Config, ConflictPolicy, Provider,
};
use crate::organizer;
use chrono::{NaiveDate, NaiveDateTime, Utc};
use eframe::egui;

#[derive(Default)]
pub(crate) struct State {
    pub initializing: bool,
    pub bound: bool,
    pub snapshot: Option<organizer::State>,
    config: Option<Config>,
    event_path: String,
    todo_path: String,
    policy: ConflictPolicy,
    collections: Vec<dav::Collection>,
    discovering: Option<Config>,
    confirmation: Option<Initialization>,
    operation: Option<(String, u64)>,
    editor: Option<Editor>,
    note: String,
    error: bool,
    store_error: String,
    recovery_candidates: Vec<std::path::PathBuf>,
    recovery_preview: Option<(organizer::store::RecoveryPreview, String)>,
    recovery_confirm: bool,
    recovered: bool,
    browser_open: bool,
    browser_search: String,
    browser_kind: u8,
    browser_status: u8,
    browser_collection: String,
}

struct Initialization {
    config: Config,
    events: String,
    todos: String,
    generation: u64,
    event_count: usize,
    todo_count: usize,
    completed_count: usize,
    repeating_count: usize,
}

impl State {
    fn request_initialization(
        &mut self,
        config: Config,
        events: String,
        todos: String,
        current: &organizer::State,
    ) -> Result<(), String> {
        if config.provider != Provider::Custom {
            return Err("只有 CalDAV 可以请求双向初始化".into());
        }
        self.confirmation = Some(initialization(config, events, todos, current));
        Ok(())
    }
    fn cancel_initialization(&mut self) {
        self.confirmation = None;
    }

    /// None 忽略；Some(true) 完成当前操作；Some(false) 只刷新常规同步状态。
    fn classify_result(
        &self,
        account: &str,
        generation: u64,
        success: bool,
        current: &organizer::State,
    ) -> Option<bool> {
        let operation = if let Some((id, minimum)) = &self.operation {
            if id != account
                || (generation != 0 && generation < *minimum)
                || success && generation <= *minimum
            {
                return None;
            }
            true
        } else {
            false
        };
        if !operation
            && !current
                .binding
                .as_ref()
                .is_some_and(|binding| binding.account_id == account)
        {
            return None;
        }
        if generation != 0 && generation > current.generation {
            return None;
        }
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| generation != 0 && generation < snapshot.generation)
        {
            return None;
        }
        Some(operation)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DateInput {
    // 空值由 value 表达，类型本身不会把缺失日期变成今天。
    kind: u8,
    value: String,
    tzid: String,
}
impl DateInput {
    fn from_date(date: Option<&CalendarDate>) -> Self {
        match date {
            None => Self::default(),
            Some(CalendarDate::Date(day)) => Self {
                kind: 0,
                value: day.format("%Y-%m-%d").to_string(),
                tzid: String::new(),
            },
            Some(CalendarDate::Utc(at)) => Self {
                kind: 1,
                value: at.format("%Y-%m-%d %H:%M:%S").to_string(),
                tzid: String::new(),
            },
            Some(CalendarDate::Zoned { local, tzid }) => Self {
                kind: 2,
                value: local.format("%Y-%m-%d %H:%M:%S").to_string(),
                tzid: tzid.clone(),
            },
            Some(CalendarDate::Floating(at)) => Self {
                kind: 3,
                value: at.format("%Y-%m-%d %H:%M:%S").to_string(),
                tzid: String::new(),
            },
        }
    }
    fn parse(&self) -> Result<Option<CalendarDate>, String> {
        let value = self.value.trim();
        if value.is_empty() {
            return Ok(None);
        }
        if self.kind == 0 {
            return NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map(CalendarDate::Date)
                .map(Some)
                .map_err(|_| "日期请使用 YYYY-MM-DD".into());
        }
        let at = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
            .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M"))
            .map_err(|_| "时间请使用 YYYY-MM-DD HH:MM[:SS]".to_owned())?;
        Ok(Some(match self.kind {
            1 => CalendarDate::Utc(at.and_utc()),
            2 if self.tzid.trim().is_empty() => {
                return Err("TZID 不能为空，例如 Asia/Shanghai".into())
            }
            2 => CalendarDate::Zoned {
                local: at,
                tzid: self.tzid.trim().into(),
            },
            _ => CalendarDate::Floating(at),
        }))
    }
    fn draw(&mut self, ui: &mut egui::Ui, label: &str) {
        ui.push_id(label, |ui| {
            ui.label(label);
            egui::ComboBox::from_id_salt("type")
                .selected_text(date_kind(self.kind))
                .show_ui(ui, |ui| {
                    for kind in 0..4 {
                        ui.selectable_value(&mut self.kind, kind, date_kind(kind));
                    }
                });
            ui.add(
                egui::TextEdit::singleline(&mut self.value)
                    .desired_width(f32::INFINITY)
                    .hint_text(if self.kind == 0 {
                        "YYYY-MM-DD（留空表示无日期）"
                    } else {
                        "YYYY-MM-DD HH:MM:SS（可留空）"
                    }),
            );
            if self.kind == 2 {
                ui.add(
                    egui::TextEdit::singleline(&mut self.tzid)
                        .hint_text("TZID，例如 Asia/Shanghai"),
                );
            }
        });
    }
}
fn date_kind(kind: u8) -> &'static str {
    match kind {
        0 => "DATE · 全天日期",
        1 => "UTC · 世界时",
        2 => "TZID · 指定时区",
        _ => "floating · 浮动时间",
    }
}

#[derive(Clone)]
struct AlarmInput {
    index: Option<usize>,
    original: Option<DisplayAlarm>,
    kind: u8,
    value: String,
    description: String,
    removed: bool,
}
impl AlarmInput {
    fn from_alarm(alarm: &Alarm) -> Option<Self> {
        if !alarm.editable {
            return None;
        }
        let trigger = alarm.trigger.as_ref().ok()?.clone();
        let (kind, value) = match &trigger {
            AlarmTrigger::Absolute(at) => (2, at.format("%Y-%m-%d %H:%M:%S").to_string()),
            AlarmTrigger::Relative { duration, related } => (
                if *related == AlarmRelation::End { 1 } else { 0 },
                duration.as_str().to_owned(),
            ),
        };
        let description = alarm.description.clone().unwrap_or_default();
        Some(Self {
            index: Some(alarm.index),
            original: Some(DisplayAlarm {
                trigger,
                description: description.clone(),
            }),
            kind,
            value,
            description,
            removed: false,
        })
    }
    fn parse(&self) -> Result<DisplayAlarm, String> {
        let trigger = if self.kind == 2 {
            let at = NaiveDateTime::parse_from_str(self.value.trim(), "%Y-%m-%d %H:%M:%S")
                .or_else(|_| NaiveDateTime::parse_from_str(self.value.trim(), "%Y-%m-%d %H:%M"))
                .map_err(|_| "绝对提醒请填写 UTC 时间 YYYY-MM-DD HH:MM[:SS]".to_owned())?;
            AlarmTrigger::Absolute(at.and_utc())
        } else {
            AlarmTrigger::Relative {
                duration: IcalDuration::parse(self.value.trim()).map_err(failure)?,
                related: if self.kind == 1 {
                    AlarmRelation::End
                } else {
                    AlarmRelation::Start
                },
            }
        };
        Ok(DisplayAlarm {
            trigger,
            description: self.description.clone(),
        })
    }
}

struct BrowserRow {
    resource_id: String,
    task: bool,
    title: String,
    uid: String,
    collection: String,
    href: String,
    status: String,
    editable: bool,
    error: Option<String>,
}

fn browser_rows(state: &organizer::State) -> Vec<BrowserRow> {
    let mut rows = Vec::new();
    for resource in state.resources.values() {
        let Some(raw) = &resource.local_ics else {
            continue;
        };
        let collection = resource.collection_url.clone();
        let href = resource.href.clone();
        let document = match CalendarDocument::parse(raw) {
            Ok(document) => document,
            Err(error) => {
                rows.push(BrowserRow {
                    resource_id: resource.id.clone(),
                    task: false,
                    title: "无法解析的同步资源".into(),
                    uid: String::new(),
                    collection,
                    href,
                    status: "原始 ICS 保留".into(),
                    editable: false,
                    error: Some(failure(error)),
                });
                continue;
            }
        };
        let items = match document.items() {
            Ok(items) => items,
            Err(error) => {
                rows.push(BrowserRow {
                    resource_id: resource.id.clone(),
                    task: false,
                    title: "无法读取资源组件".into(),
                    uid: String::new(),
                    collection,
                    href,
                    status: "原始 ICS 保留".into(),
                    editable: false,
                    error: Some(failure(error)),
                });
                continue;
            }
        };
        for item in items {
            let task = matches!(&item, CalendarItem::Task(_));
            let status = match &item {
                CalendarItem::Event(event) => match event.status.as_ref() {
                    Some(crate::calendar::caldav::ical::EventStatus::Tentative) => {
                        "TENTATIVE".into()
                    }
                    Some(crate::calendar::caldav::ical::EventStatus::Cancelled) => {
                        "CANCELLED".into()
                    }
                    Some(crate::calendar::caldav::ical::EventStatus::Other(value)) => value.clone(),
                    _ => "CONFIRMED".into(),
                },
                CalendarItem::Task(task) => task
                    .status
                    .as_ref()
                    .map(|status| status.as_str().to_owned())
                    .unwrap_or_else(|| "NEEDS-ACTION".into()),
            };
            rows.push(BrowserRow {
                resource_id: resource.id.clone(),
                task,
                title: item
                    .common()
                    .title
                    .clone()
                    .unwrap_or_else(|| "（无标题）".into()),
                uid: item.common().uid.clone(),
                collection: collection.clone(),
                href: href.clone(),
                status,
                editable: true,
                error: None,
            });
        }
    }
    rows.sort_by_key(|row| (row.task, row.title.to_lowercase(), row.uid.clone()));
    rows
}

fn browser_row_matches(
    row: &BrowserRow,
    search: &str,
    kind: u8,
    status: u8,
    collection: &str,
) -> bool {
    let kind_match = kind == 0 || kind == if row.task { 2 } else { 1 };
    let status_match = match status {
        1 => row.status != "COMPLETED" && row.status != "CANCELLED",
        2 => row.status == "COMPLETED",
        3 => row.status == "CANCELLED",
        _ => true,
    };
    let collection_match = collection.is_empty() || collection == row.collection;
    let query = search.trim().to_lowercase();
    let text_match = query.is_empty()
        || [row.title.as_str(), row.uid.as_str(), row.href.as_str()]
            .into_iter()
            .any(|text| text.to_lowercase().contains(&query));
    kind_match && status_match && collection_match && text_match
}

struct Editor {
    resource_id: Option<String>,
    account_id: String,
    binding_generation: u64,
    collection: String,
    revision: u64,
    original_ics: Option<String>,
    document: CalendarDocument,
    item: CalendarItem,
    capabilities: EditCapabilities,
    title: String,
    description: String,
    location: String,
    categories: String,
    start: DateInput,
    end: DateInput,
    status: String,
    percent: String,
    priority: String,
    previous_percent: u8,
    alarms: Vec<AlarmInput>,
    delete_confirm: bool,
    error: String,
}
impl Editor {
    fn select(&mut self, item: CalendarItem) -> Result<(), String> {
        self.capabilities = self
            .document
            .edit_capabilities(item.common().index)
            .map_err(failure)?;
        let common = item.common();
        self.title = common.title.clone().unwrap_or_default();
        self.description = common.description.clone().unwrap_or_default();
        self.location = common.location.clone().unwrap_or_default();
        self.categories = common.categories.join(", ");
        self.alarms = common
            .alarms
            .iter()
            .filter_map(AlarmInput::from_alarm)
            .collect();
        let (start, end) = match &item {
            CalendarItem::Event(event) => (event.start.as_ref(), event.end.as_ref()),
            CalendarItem::Task(task) => (task.start.as_ref(), task.due.as_ref()),
        };
        self.start = DateInput::from_date(start);
        self.end = DateInput::from_date(end);
        if let CalendarItem::Task(task) = &item {
            if !matches!(task.status, Some(TaskStatus::Completed)) {
                self.previous_percent = task.percent_complete.unwrap_or(0);
            }
            self.status = task
                .status
                .as_ref()
                .map(|status| status.as_str().to_owned())
                .unwrap_or_else(|| "NEEDS-ACTION".into());
            self.percent = task
                .percent_complete
                .map(|value| value.to_string())
                .unwrap_or_default();
            self.priority = task
                .priority
                .map(|value| value.to_string())
                .unwrap_or_default();
        }
        self.item = item;
        self.delete_confirm = false;
        self.error.clear();
        Ok(())
    }
    fn alarm_edits(&self) -> Result<Vec<AlarmEdit>, String> {
        let mut changes = Vec::new();
        for alarm in &self.alarms {
            if alarm.removed {
                if let Some(index) = alarm.index {
                    changes.push(AlarmEdit::Remove { index });
                }
                continue;
            }
            let next = alarm.parse()?;
            match alarm.index {
                Some(index) if alarm.original.as_ref() != Some(&next) => {
                    changes.push(AlarmEdit::Update { index, alarm: next })
                }
                None => changes.push(AlarmEdit::Add(next)),
                _ => {}
            }
        }
        Ok(changes)
    }
    fn patched(&self) -> Result<CalendarDocument, String> {
        let common = self.item.common();
        let mut patch = CommonPatch {
            title: text_patch(&self.title, common.title.as_deref()),
            description: text_patch(&self.description, common.description.as_deref()),
            location: text_patch(&self.location, common.location.as_deref()),
            ..Default::default()
        };
        // 未触碰标签输入时，保留原来的多条 CATEGORIES 及参数。
        if self.categories != common.categories.join(", ") {
            let categories = self
                .categories
                .split([',', '，'])
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if categories != common.categories {
                patch.categories = if categories.is_empty() {
                    Patch::Remove
                } else {
                    Patch::Set(categories)
                };
            }
        }
        let mut document = self.document.clone();
        let alarms = self.alarm_edits()?;
        match &self.item {
            CalendarItem::Event(event) => {
                let start = date_patch(&self.start, event.start.as_ref())?;
                let end = date_patch(&self.end, event.end.as_ref())?;
                // 明确填写 DTEND 时才去掉互斥的 DURATION；仅修改标题保留 DURATION。
                let duration = if event.duration.is_some() && matches!(end, Patch::Set(_)) {
                    Patch::Remove
                } else {
                    Patch::Keep
                };
                document
                    .patch_event(
                        common.index,
                        &EventPatch {
                            common: patch,
                            start,
                            end,
                            duration,
                            alarms,
                        },
                        Utc::now(),
                    )
                    .map_err(failure)?;
            }
            CalendarItem::Task(task) => {
                let start = date_patch(&self.start, task.start.as_ref())?;
                let due = date_patch(&self.end, task.due.as_ref())?;
                let duration = if task.duration.is_some() && matches!(due, Patch::Set(_)) {
                    Patch::Remove
                } else {
                    Patch::Keep
                };
                let old_status = task
                    .status
                    .as_ref()
                    .map(TaskStatus::as_str)
                    .unwrap_or("NEEDS-ACTION");
                let state = if self.status == old_status {
                    Patch::Keep
                } else if self.status == "COMPLETED" {
                    Patch::Set(TaskStateChange::Complete)
                } else {
                    Patch::Set(TaskStateChange::Set(projection::status_from_str(
                        &self.status,
                    )))
                };
                let percent_complete = if self.status == "COMPLETED" && self.status != old_status {
                    Patch::Keep
                } else {
                    number_patch(&self.percent, task.percent_complete, 100, "进度")?
                };
                let priority = number_patch(&self.priority, task.priority, 9, "优先级")?;
                document
                    .patch_task(
                        common.index,
                        &TaskPatch {
                            common: patch,
                            start,
                            due,
                            duration,
                            state,
                            percent_complete,
                            priority,
                            alarms,
                        },
                        Utc::now(),
                    )
                    .map_err(failure)?;
            }
        }
        Ok(document)
    }
}
fn failure(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn text_patch(next: &str, old: Option<&str>) -> Patch<String> {
    if next == old.unwrap_or_default() {
        Patch::Keep
    } else if next.is_empty() {
        Patch::Remove
    } else {
        Patch::Set(next.to_owned())
    }
}
fn date_patch(
    input: &DateInput,
    old: Option<&CalendarDate>,
) -> Result<Patch<CalendarDate>, String> {
    if input == &DateInput::from_date(old) {
        return Ok(Patch::Keep);
    }
    let next = input.parse()?;
    Ok(if next.as_ref() == old {
        Patch::Keep
    } else {
        next.map(Patch::Set).unwrap_or(Patch::Remove)
    })
}
fn number_patch(next: &str, old: Option<u8>, max: u8, label: &str) -> Result<Patch<u8>, String> {
    let next = if next.trim().is_empty() {
        None
    } else {
        Some(
            next.trim()
                .parse::<u8>()
                .ok()
                .filter(|value| *value <= max)
                .ok_or_else(|| format!("{label}必须是 0–{max}，留空表示移除此属性"))?,
        )
    };
    Ok(if next == old {
        Patch::Keep
    } else {
        next.map(Patch::Set).unwrap_or(Patch::Remove)
    })
}

impl App {
    fn caldav_store_failure(&mut self, error: &str) {
        self.calendar.caldav.store_error = error.into();
        self.calendar.caldav.snapshot = None;
        self.calendar.caldav.bound = true;
        self.calendar.caldav.initializing = true;
        self.calendar.caldav.operation = None;
        self.calendar.caldav.editor = None;
        self.calendar.caldav.confirmation = None;
    }

    pub(crate) fn reload_organizer(&mut self) -> Result<(), String> {
        let state = match organizer::read_state() {
            Ok(state) => state,
            Err(error) => {
                self.caldav_store_failure(&error);
                return Err(error);
            }
        };
        if !self.calendar.caldav.store_error.is_empty() {
            self.calendar.caldav.initializing = false;
            self.calendar.caldav.operation = None;
            self.calendar.caldav.store_error.clear();
        }
        if let Some(id) = &self.editing_id {
            let previous = self.todos.items.iter().find(|item| &item.id == id);
            let updated = state.todos.iter().position(|item| &item.id == id);
            if previous != updated.map(|index| &state.todos[index]) {
                self.cancel_edit();
                self.say("待办已被后台更新，请重新打开后编辑");
            } else {
                self.editing = updated;
            }
        }
        self.todos.items = state.todos.clone();
        self.todos.baseline = Some(state.todos.clone());
        self.calendar.local = state.local_events.clone();
        self.calendar.local_error.clear();
        self.calendar.config = state
            .accounts
            .iter()
            .find(|cfg| cfg.provider == state.active_provider)
            .cloned();
        self.calendar.feeds.retain(|feed| {
            state
                .accounts
                .iter()
                .any(|cfg| cfg.provider == feed.config.provider)
        });
        for config in &state.accounts {
            if !self
                .calendar
                .feeds
                .iter()
                .any(|feed| feed.config.provider == config.provider)
            {
                self.calendar
                    .feeds
                    .push(crate::app::CalendarFeed::new(config.clone()));
            }
        }
        for feed in &mut self.calendar.feeds {
            if let Some(config) = state
                .accounts
                .iter()
                .find(|cfg| cfg.provider == feed.config.provider)
            {
                feed.config = config.clone();
            }
            if state
                .binding
                .as_ref()
                .is_some_and(|binding| binding.account_id == feed.config.account_id)
            {
                feed.events.clear();
                feed.month.events.clear();
            }
        }
        if state.binding.as_ref().is_some_and(|binding| {
            self.calendar
                .config
                .as_ref()
                .is_some_and(|cfg| cfg.account_id == binding.account_id)
        }) {
            self.calendar.events.clear();
            self.calendar.view.events.clear();
        }
        let cfg = state
            .accounts
            .iter()
            .find(|cfg| cfg.provider == Provider::Custom)
            .cloned();
        if self.calendar.caldav.config != cfg {
            self.calendar.caldav.event_path = cfg
                .as_ref()
                .and_then(|cfg| cfg.event_collection.clone())
                .unwrap_or_default();
            self.calendar.caldav.todo_path = cfg
                .as_ref()
                .and_then(|cfg| cfg.todo_collection.clone())
                .unwrap_or_default();
            self.calendar.caldav.policy = cfg
                .as_ref()
                .map(|cfg| cfg.conflict_policy)
                .unwrap_or_default();
            self.calendar.caldav.config = cfg;
        }
        self.calendar.caldav.bound = state.binding.is_some();
        self.calendar.caldav.snapshot = Some(state);
        Ok(())
    }

    fn caldav_form_config(&self) -> Result<Config, String> {
        if self.calendar.form.provider != Provider::Custom {
            return Err("只有 CalDAV 支持日程与待办双向同步".into());
        }
        let username = self.calendar.form.username.trim().to_owned();
        if username.is_empty() {
            return Err("请先填写共享用户名".into());
        }
        let server = calendar::normalize_server(&self.calendar.form.server)
            .ok_or("请填写有效的 CalDAV 服务器地址")?;
        let saved = self.calendar.caldav.config.as_ref().filter(|saved| {
            saved.username == username && saved.server.as_deref() == Some(server.as_str())
        });
        let mut cfg = saved.cloned().unwrap_or_else(|| Config {
            provider: Provider::Custom,
            username,
            server: Some(server),
            ..Default::default()
        });
        cfg.event_collection = nonempty(&self.calendar.caldav.event_path);
        cfg.todo_collection = nonempty(&self.calendar.caldav.todo_path);
        cfg.conflict_policy = self.calendar.caldav.policy;
        Ok(cfg)
    }
    fn caldav_password(&self, cfg: &Config) -> Result<String, String> {
        if !self.calendar.form.password.is_empty() {
            return Ok(self.calendar.form.password.clone());
        }
        let saved = self
            .calendar
            .caldav
            .config
            .as_ref()
            .filter(|saved| saved.account_id == cfg.account_id && !cfg.account_id.is_empty())
            .ok_or("请填写密码；保存后两类集合共享同一份 Windows 凭据")?;
        calendar::secrets::load(saved)
    }
    fn caldav_note(&mut self, result: Result<(), String>, success: &str) {
        self.calendar.caldav.error = result.is_err();
        self.calendar.caldav.note = result.err().unwrap_or_else(|| success.into());
    }

    pub(crate) fn draw_caldav_settings(&mut self, ui: &mut egui::Ui) {
        if self.calendar.caldav.snapshot.is_none() && self.calendar.caldav.store_error.is_empty() {
            if let Err(error) = self.reload_organizer() {
                self.caldav_note(Err(error), "");
            }
        }
        // 存储恢复属于本机全局数据，即使当前选择 Exchange / 其他平台也必须可见。
        if !self.calendar.caldav.store_error.is_empty() {
            self.draw_organizer_recovery(ui);
            return;
        }
        if self.calendar.caldav.recovered {
            ui.label("已从历史快照恢复为只读，原 CURRENT 已备份。双向同步已关闭，请重新启用并先核对服务端版本。");
            if ui.button("关闭恢复提示").clicked() {
                self.calendar.caldav.recovered = false;
            }
        }
        if self.calendar.form.provider != Provider::Custom {
            return;
        }
        ui.add_space(10.0);
        ui.heading("CalDAV 日程与待办");
        ui.label("使用上方同一份服务器、用户名和密码。密码留空时读取已保存的 Windows 凭据。");
        let frozen = self.calendar.caldav.initializing;
        let mut discover = false;
        let mut save_policy = false;
        let mut toggle = None;
        let mut confirm = false;
        let mut refresh = false;
        let mut open: Option<(Option<String>, bool)> = None;
        let mut conflict = None;
        ui.add_enabled_ui(!frozen, |ui| {
            ui.horizontal_wrapped(|ui| {
                discover = ui
                    .add_enabled(
                        self.calendar.caldav.discovering.is_none(),
                        egui::Button::new(if self.calendar.caldav.discovering.is_some() {
                            "正在发现集合…"
                        } else {
                            "发现集合（只读）"
                        }),
                    )
                    .clicked();
                refresh = ui.button("刷新本机状态").clicked();
            });
            collection_picker(
                ui,
                "日程集合 · VEVENT",
                &self.calendar.caldav.collections,
                "VEVENT",
                &mut self.calendar.caldav.event_path,
            );
            collection_picker(
                ui,
                "待办集合 · VTODO",
                &self.calendar.caldav.collections,
                "VTODO",
                &mut self.calendar.caldav.todo_path,
            );
            egui::CollapsingHeader::new("高级：直接填写 collection 路径").show(ui, |ui| {
                ui.label("接受服务端返回的绝对 URL 或相对于服务器地址的路径；可使用同一集合。");
                ui.label("VEVENT");
                ui.text_edit_singleline(&mut self.calendar.caldav.event_path);
                ui.label("VTODO");
                ui.text_edit_singleline(&mut self.calendar.caldav.todo_path);
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("账号冲突策略");
                egui::ComboBox::from_id_salt("caldav-conflict-policy")
                    .selected_text(policy_name(self.calendar.caldav.policy))
                    .show_ui(ui, |ui| {
                        for policy in [
                            ConflictPolicy::Ask,
                            ConflictPolicy::Server,
                            ConflictPolicy::Client,
                        ] {
                            ui.selectable_value(
                                &mut self.calendar.caldav.policy,
                                policy,
                                policy_name(policy),
                            );
                        }
                    });
                save_policy = ui.button("保存冲突策略").clicked();
            });
            let mut enabled = self
                .calendar
                .caldav
                .snapshot
                .as_ref()
                .and_then(|state| {
                    state.binding.as_ref().and_then(|binding| {
                        state
                            .accounts
                            .iter()
                            .find(|cfg| cfg.account_id == binding.account_id)
                    })
                })
                .is_some_and(|cfg| cfg.two_way);
            if ui.checkbox(&mut enabled, "双向同步日程和待办").changed() {
                toggle = Some(enabled);
            }
            if enabled && ui.button("应用集合选择（更换绑定需重新确认）").clicked()
            {
                toggle = Some(true);
            }
            ui.small("默认关闭。关闭后保留缓存、待上传内容和冲突，同步对象变为只读。");
        });
        if frozen {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("正在初始化或切换模式，日程和待办编辑已暂停…");
            });
        }
        if let Some(warning) = &self.calendar.caldav.confirmation {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.strong("首次开启 / 更换绑定：以服务端内容替换本机数据");
                ui.label(format!("将替换本机日程 {} 条，以及全部待办 {} 条（含已完成 {} 条、重复 {} 条）。", warning.event_count, warning.todo_count, warning.completed_count, warning.repeating_count));
                ui.label("这些本机内容不会上传，也不会与服务端合并。日程、待办两个集合均验证并完整读取成功后才一次替换；失败保留原数据。邮件邀请及其他只读日历不在替换范围内。");
                ui.label(format!("日程：{}\n待办：{}", warning.events, warning.todos));
                confirm = ui.add_enabled(!frozen, egui::Button::new("确认抛弃上述本机内容，从服务端初始化")).clicked();
            });
            if ui
                .add_enabled(!frozen, egui::Button::new("取消初始化"))
                .clicked()
            {
                self.calendar.caldav.cancel_initialization();
            }
        }
        if let Some(state) = &self.calendar.caldav.snapshot {
            ui.separator();
            for (name, task) in [("日程", false), ("待办", true)] {
                draw_statistics(ui, state, name, task);
            }
            if state.binding.is_some() {
                let writable = organizer::writable(state) && !frozen;
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(writable, egui::Button::new("新建日程"))
                        .clicked()
                    {
                        open = Some((None, false));
                    }
                    if ui
                        .add_enabled(writable, egui::Button::new("新建待办"))
                        .clicked()
                    {
                        open = Some((None, true));
                    }
                });
                ui.horizontal(|ui| {
                    ui.strong("同步资源");
                    ui.small(format!(
                        "{} 个资源",
                        state
                            .resources
                            .values()
                            .filter(|resource| resource.local_ics.is_some())
                            .count()
                    ));
                    if ui
                        .add_enabled(!frozen, egui::Button::new("浏览同步资源 / 完整编辑…"))
                        .clicked()
                    {
                        self.calendar.caldav.browser_open = true;
                    }
                });
            }
            for item in state.conflicts.iter().filter(|conflict| !conflict.resolved) {
                egui::CollapsingHeader::new(format!("资源冲突 · {}", item.resource_id))
                    .id_salt(&item.id)
                    .show(ui, |ui| {
                        conflict_body(ui, "上次同步基线", &item.baseline);
                        conflict_body(ui, "本机版本", &item.local);
                        conflict_body(ui, "服务端版本", &item.remote);
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .add_enabled(
                                    !frozen,
                                    egui::Button::new("保留服务端版本（含删除意图）"),
                                )
                                .clicked()
                            {
                                conflict = Some((item.id.clone(), false));
                            }
                            if ui
                                .add_enabled(
                                    !frozen,
                                    egui::Button::new("保留本机版本（含删除意图）"),
                                )
                                .clicked()
                            {
                                conflict = Some((item.id.clone(), true));
                            }
                        });
                    });
            }
        }
        if !self.calendar.caldav.note.is_empty() {
            if self.calendar.caldav.error {
                ui.colored_label(ui.visuals().error_fg_color, &self.calendar.caldav.note);
            } else {
                ui.label(&self.calendar.caldav.note);
            }
        }
        if refresh {
            let result = self.reload_organizer();
            self.caldav_note(result, "已刷新本机快照");
        }
        if discover {
            let result = self
                .caldav_form_config()
                .and_then(|cfg| self.caldav_password(&cfg).map(|password| (cfg, password)));
            match result {
                Ok((cfg, password)) => {
                    self.calendar.caldav.discovering = Some(cfg.clone());
                    self.pending.push(Cmd::CalendarDiscover(cfg, password));
                    self.caldav_note(Ok(()), "正在只读发现集合，不会上传或清空内容");
                }
                Err(error) => self.caldav_note(Err(error), ""),
            }
        }
        if save_policy {
            let result = (|| {
                let state = organizer::read_state()?;
                let mut cfg = state
                    .accounts
                    .into_iter()
                    .find(|cfg| cfg.provider == Provider::Custom)
                    .ok_or("请先保存 CalDAV 连接")?;
                cfg.conflict_policy = self.calendar.caldav.policy;
                calendar::save_config(&cfg)?;
                calendar::load_config_strict()?;
                self.reload_organizer()
            })();
            self.caldav_note(result, "已保存账号冲突策略");
        }
        if let Some(enabled) = toggle {
            let result = self.caldav_toggle(enabled);
            self.caldav_note(
                result,
                if enabled {
                    "请确认首次初始化范围"
                } else {
                    "正在关闭双向同步…"
                },
            );
        }
        if confirm {
            let result = self.caldav_initialize();
            self.caldav_note(result, "正在完整读取两个集合，成功后才替换本机内容…");
        }
        if let Some((id, local)) = conflict {
            self.pending.push(Cmd::CalendarConflict(id, local));
        }
        if let Some((id, task)) = open {
            self.open_caldav_editor(id.as_deref(), task);
        }
        self.draw_caldav_browser(ui.ctx());
    }

    fn draw_caldav_browser(&mut self, ctx: &egui::Context) {
        if !self.calendar.caldav.browser_open {
            return;
        }
        let Some(state) = self.calendar.caldav.snapshot.as_ref() else {
            self.calendar.caldav.browser_open = false;
            return;
        };
        let rows = browser_rows(state);
        let collections = rows
            .iter()
            .map(|row| row.collection.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let mut open = true;
        let mut edit = None;
        egui::Window::new("同步资源浏览器")
            .id(egui::Id::new("caldav-resource-browser"))
            .open(&mut open)
            .default_size(egui::vec2(760.0, 560.0))
            .min_size(egui::vec2(560.0, 360.0))
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("筛选");
                    ui.add_sized(
                        [220.0, 26.0],
                        egui::TextEdit::singleline(&mut self.calendar.caldav.browser_search)
                            .hint_text("标题、UID 或资源路径"),
                    );
                    egui::ComboBox::from_id_salt("caldav-browser-kind")
                        .selected_text(match self.calendar.caldav.browser_kind {
                            1 => "日程",
                            2 => "待办",
                            _ => "全部类型",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.calendar.caldav.browser_kind,
                                0,
                                "全部类型",
                            );
                            ui.selectable_value(&mut self.calendar.caldav.browser_kind, 1, "日程");
                            ui.selectable_value(&mut self.calendar.caldav.browser_kind, 2, "待办");
                        });
                    egui::ComboBox::from_id_salt("caldav-browser-status")
                        .selected_text(match self.calendar.caldav.browser_status {
                            1 => "未完成 / 进行中",
                            2 => "已完成",
                            3 => "已取消",
                            _ => "全部状态",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.calendar.caldav.browser_status,
                                0,
                                "全部状态",
                            );
                            ui.selectable_value(
                                &mut self.calendar.caldav.browser_status,
                                1,
                                "未完成 / 进行中",
                            );
                            ui.selectable_value(
                                &mut self.calendar.caldav.browser_status,
                                2,
                                "已完成",
                            );
                            ui.selectable_value(
                                &mut self.calendar.caldav.browser_status,
                                3,
                                "已取消",
                            );
                        });
                });
                ui.horizontal(|ui| {
                    ui.label("集合");
                    egui::ComboBox::from_id_salt("caldav-browser-collection")
                        .selected_text(if self.calendar.caldav.browser_collection.is_empty() {
                            "全部集合"
                        } else {
                            self.calendar.caldav.browser_collection.as_str()
                        })
                        .width(360.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.calendar.caldav.browser_collection,
                                String::new(),
                                "全部集合",
                            );
                            for collection in &collections {
                                ui.selectable_value(
                                    &mut self.calendar.caldav.browser_collection,
                                    collection.clone(),
                                    collection,
                                );
                            }
                        });
                    if ui.button("清除筛选").clicked() {
                        self.calendar.caldav.browser_search.clear();
                        self.calendar.caldav.browser_kind = 0;
                        self.calendar.caldav.browser_status = 0;
                        self.calendar.caldav.browser_collection.clear();
                    }
                });
                ui.separator();
                let visible = rows
                    .iter()
                    .filter(|row| {
                        browser_row_matches(
                            row,
                            &self.calendar.caldav.browser_search,
                            self.calendar.caldav.browser_kind,
                            self.calendar.caldav.browser_status,
                            &self.calendar.caldav.browser_collection,
                        )
                    })
                    .collect::<Vec<_>>();
                ui.horizontal(|ui| {
                    ui.strong(format!("显示 {} / {} 个组件", visible.len(), rows.len()));
                    ui.small("点击资源进入完整编辑；复杂字段和未知 ICS 属性仍会原样保留。");
                });
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if visible.is_empty() {
                            ui.add_space(24.0);
                            ui.centered_and_justified(|ui| {
                                ui.label("没有符合筛选条件的同步资源");
                            });
                        }
                        for (index, row) in visible.iter().enumerate() {
                            let id = row.resource_id.clone();
                            let task = row.task;
                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let type_label = if row.task { "待办" } else { "日程" };
                                    ui.colored_label(
                                        if row.task {
                                            egui::Color32::from_rgb(74, 170, 232)
                                        } else {
                                            egui::Color32::from_rgb(83, 190, 126)
                                        },
                                        type_label,
                                    );
                                    ui.strong(&row.title);
                                    ui.label(egui::RichText::new(&row.status).small());
                                    if row.editable && ui.button("完整编辑").clicked() {
                                        edit = Some((id.clone(), task));
                                    }
                                });
                                ui.small(format!(
                                    "UID：{}",
                                    if row.uid.is_empty() {
                                        "（无 UID）"
                                    } else {
                                        &row.uid
                                    }
                                ));
                                ui.small(format!("集合：{}", row.collection));
                                ui.small(format!("资源：{}", row.href));
                                if let Some(error) = &row.error {
                                    ui.colored_label(ui.visuals().warn_fg_color, error);
                                }
                            });
                            if index + 1 < visible.len() {
                                ui.add_space(4.0);
                            }
                        }
                    });
            });
        if !open {
            self.calendar.caldav.browser_open = false;
        }
        if let Some((id, task)) = edit {
            self.calendar.caldav.browser_open = false;
            self.open_caldav_editor(Some(&id), task);
        }
    }

    fn draw_organizer_recovery(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("organizer 存储异常 · 同步与编辑已停止");
        ui.colored_label(
            ui.visuals().error_fg_color,
            &self.calendar.caldav.store_error,
        );
        ui.label("不会自动回退历史数据。请先预览候选快照并核对内容与 SHA-256，确认后才能恢复。恢复会备份原 CURRENT，创建新的只读快照；保留资源、冲突和队列，重新启用前必须核对服务端。");
        let mut list = false;
        let mut retry = false;
        let mut selected = None;
        let mut restore = false;
        ui.horizontal_wrapped(|ui| {
            list = ui.button("列出 / 刷新历史快照").clicked();
            retry = ui.button("重试读取 CURRENT").clicked();
        });
        for path in &self.calendar.caldav.recovery_candidates {
            if let Some(file) = path.file_name().and_then(|file| file.to_str()) {
                if ui.button(format!("预览 {file}")).clicked() {
                    selected = Some(file.to_owned());
                }
            }
        }
        if let Some((preview, body)) = &self.calendar.caldav.recovery_preview {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.strong(format!(
                    "候选：{} · generation {}",
                    preview.file, preview.state.generation
                ));
                ui.label(egui::RichText::new(format!("SHA-256：{}", preview.sha256)).monospace());
                ui.label(format!(
                    "本机日程 {} · 全部待办 {} · 同步资源 {} · 队列 {} · 未解决冲突 {}",
                    preview.state.local_events.len(),
                    preview.state.todos.len(),
                    preview.state.resources.len(),
                    preview.state.pending.len(),
                    preview
                        .state
                        .conflicts
                        .iter()
                        .filter(|conflict| !conflict.resolved)
                        .count()
                ));
                for account in &preview.state.accounts {
                    ui.label(format!(
                        "账号：{} · {}",
                        account.provider.label(),
                        account.username
                    ));
                }
                egui::CollapsingHeader::new("查看候选完整内容（含基线与待上传内容）").show(
                    ui,
                    |ui| {
                        let mut text = body.as_str();
                        ui.add(
                            egui::TextEdit::multiline(&mut text)
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .desired_rows(14)
                                .interactive(false),
                        );
                    },
                );
                ui.checkbox(
                    &mut self.calendar.caldav.recovery_confirm,
                    "我已核对候选内容和 SHA-256，确认恢复后关闭双向同步，并在重新启用前对账",
                );
                restore = ui
                    .add_enabled(
                        self.calendar.caldav.recovery_confirm,
                        egui::Button::new("再次确认：备份 CURRENT 并恢复此快照为只读"),
                    )
                    .clicked();
            });
        }
        if !self.calendar.caldav.note.is_empty() {
            ui.label(&self.calendar.caldav.note);
        }
        let root = crate::model::config_dir();
        if list {
            self.calendar.caldav.recovery_preview = None;
            self.calendar.caldav.recovery_confirm = false;
            match organizer::store::recovery_candidates(&root) {
                Ok(paths) => {
                    let empty = paths.is_empty();
                    self.calendar.caldav.recovery_candidates = paths;
                    self.caldav_note(
                        Ok(()),
                        if empty {
                            "未找到可列出的历史快照；CURRENT 保持原状。"
                        } else {
                            "请选择一个历史快照进行只读预览。"
                        },
                    );
                }
                Err(error) => self.caldav_note(Err(error), ""),
            }
        }
        if let Some(file) = selected {
            self.calendar.caldav.recovery_preview = None;
            self.calendar.caldav.recovery_confirm = false;
            let result = organizer::store::recovery_preview_at(&root, &file).and_then(|preview| {
                let body = serde_json::to_string_pretty(&preview.state).map_err(failure)?;
                Ok((preview, body))
            });
            match result {
                Ok(preview) => {
                    self.calendar.caldav.recovery_preview = Some(preview);
                    self.caldav_note(Ok(()), "已完成只读预览，尚未修改 CURRENT。");
                }
                Err(error) => self.caldav_note(Err(error), ""),
            }
        }
        // 唯一恢复调用点：用户预览、勾选确认并再次点击恢复按钮之后。
        if restore && self.calendar.caldav.recovery_confirm {
            if let Some((preview, _)) = self.calendar.caldav.recovery_preview.take() {
                self.calendar.caldav.recovery_confirm = false;
                let result = organizer::store::recovery_restore_at(&root, &preview)
                    .and_then(|_| self.reload_organizer());
                if result.is_ok() {
                    self.calendar.caldav.recovery_candidates.clear();
                    self.calendar.caldav.recovered = true;
                }
                self.caldav_note(result, "已恢复为只读：原 CURRENT 已独立备份，双向同步已关闭；请重新启用并先核对服务端版本。未发送网络请求。");
            }
        }
        if retry {
            let result = self.reload_organizer();
            self.caldav_note(result, "CURRENT 已可读取；已重新加载本机状态。");
        }
    }

    fn caldav_toggle(&mut self, enabled: bool) -> Result<(), String> {
        let state = organizer::read_state()?;
        if !enabled {
            let binding = state.binding.as_ref().ok_or("当前没有同步绑定")?;
            self.calendar.caldav.operation = Some((binding.account_id.clone(), state.generation));
            self.calendar.caldav.initializing = true;
            self.calendar.caldav.confirmation = None;
            self.pending
                .push(Cmd::CalendarTwoWay(binding.account_id.clone(), false));
            return Ok(());
        }
        let cfg = self.caldav_form_config()?;
        let events = cfg
            .event_collection
            .clone()
            .ok_or("请选择日程 collection")?;
        let todos = cfg.todo_collection.clone().ok_or("请选择待办 collection")?;
        if state.binding.as_ref().is_some_and(|binding| {
            binding.account_id == cfg.account_id
                && paths_match(&cfg, &events, &binding.event_collection)
                && paths_match(&cfg, &todos, &binding.todo_collection)
        }) {
            self.calendar.caldav.operation = Some((cfg.account_id.clone(), state.generation));
            self.calendar.caldav.initializing = true;
            self.pending.push(Cmd::CalendarTwoWay(cfg.account_id, true));
            return Ok(());
        }
        self.caldav_password(&cfg)?;
        self.calendar
            .caldav
            .request_initialization(cfg, events, todos, &state)
    }
    fn caldav_initialize(&mut self) -> Result<(), String> {
        let warning = self
            .calendar
            .caldav
            .confirmation
            .take()
            .ok_or("请先确认初始化范围")?;
        let current = organizer::read_state()?;
        let cfg = self.caldav_form_config()?;
        if current.generation != warning.generation || cfg != warning.config {
            self.calendar.caldav.confirmation = Some(initialization(
                cfg.clone(),
                cfg.event_collection.clone().ok_or("请选择日程集合")?,
                cfg.todo_collection.clone().ok_or("请选择待办集合")?,
                &current,
            ));
            return Err("确认期间数据或配置发生变化，请核对最新数量和集合后再次确认".into());
        }
        let password = self.caldav_password(&cfg)?;
        let mut cfg = cfg;
        cfg.two_way = false;
        calendar::save_connection(&cfg, &password, self.calendar.caldav.config.as_ref())?;
        let saved = calendar::load_config_strict()?
            .filter(|cfg| cfg.provider == Provider::Custom)
            .ok_or("连接保存后未能重新读取")?;
        self.reload_organizer()?;
        let state = organizer::read_state()?;
        self.calendar.form.password.clear();
        self.calendar.test_password.clear();
        self.calendar.draft = None;
        self.calendar.caldav.editor = None;
        self.calendar.caldav.operation = Some((saved.account_id.clone(), state.generation));
        self.calendar.caldav.initializing = true;
        self.pending.push(Cmd::CalendarInitialize(
            saved,
            warning.events,
            warning.todos,
            password,
            state.generation,
        ));
        Ok(())
    }

    pub(crate) fn caldav_collections_result(
        &mut self,
        config: Config,
        result: Result<Vec<dav::Collection>, String>,
    ) {
        if self.calendar.caldav.discovering.as_ref() != Some(&config) {
            return;
        }
        self.calendar.caldav.discovering = None;
        if !self.caldav_form_config().is_ok_and(|current| {
            current.username == config.username && current.server == config.server
        }) {
            return;
        }
        match result {
            Ok(collections) => {
                self.calendar.caldav.collections = collections;
                self.caldav_note(
                    Ok(()),
                    "集合发现完成，请分别选择 VEVENT 和 VTODO 集合。未上传、未清空本机内容。",
                );
            }
            Err(error) => self.caldav_note(Err(error), ""),
        }
    }

    pub(crate) fn caldav_result(
        &mut self,
        account_id: String,
        generation: u64,
        result: Result<(), String>,
    ) {
        let current = match organizer::read_state() {
            Ok(state) => state,
            Err(error) => {
                self.caldav_store_failure(&error);
                self.caldav_note(Err(error), "");
                return;
            }
        };
        let Some(operation) =
            self.calendar
                .caldav
                .classify_result(&account_id, generation, result.is_ok(), &current)
        else {
            return;
        };
        if operation {
            self.calendar.caldav.initializing = false;
            self.calendar.caldav.operation = None;
        }
        let reload = self.reload_organizer();
        let success = result.is_ok() && reload.is_ok();
        if success {
            let at = calendar::now_label();
            if let Some(feed) = self
                .calendar
                .feeds
                .iter_mut()
                .find(|feed| feed.config.account_id == account_id)
            {
                feed.connected = true;
                feed.failures = 0;
                feed.error.clear();
                feed.last_sync = at.clone();
            }
            if self
                .calendar
                .config
                .as_ref()
                .is_some_and(|cfg| cfg.account_id == account_id)
            {
                self.calendar.connected = true;
                self.calendar.failures = 0;
                self.calendar.error.clear();
                self.calendar.last_sync = at;
            }
        }
        self.caldav_note(result.and(reload), "同步状态已更新");
        if operation && success {
            self.pending.push(Cmd::CalendarSync);
        }
    }

    pub(crate) fn open_caldav_editor(&mut self, resource_id: Option<&str>, task: bool) {
        self.settings_tab = 8;
        self.open_settings();
        let result = (|| {
            if self.calendar.caldav.initializing {
                return Err("初始化或模式切换期间暂停编辑".into());
            }
            self.reload_organizer()?;
            let state = self
                .calendar
                .caldav
                .snapshot
                .as_ref()
                .ok_or("未能读取 organizer")?;
            let binding = state.binding.as_ref().ok_or("请先初始化 CalDAV 双向同步")?;
            let (document, original_ics, revision, collection) = if let Some(id) = resource_id {
                let resource = state.resources.get(id).ok_or("同步资源已不存在")?;
                if resource.account_id != binding.account_id {
                    return Err("此资源不属于当前绑定账号".into());
                }
                let raw = resource.local_ics.clone().ok_or("此资源已标记删除")?;
                (
                    CalendarDocument::parse(&raw).map_err(failure)?,
                    Some(raw),
                    resource.revision,
                    resource.collection_url.clone(),
                )
            } else {
                if !organizer::writable(state) {
                    return Err("双向同步关闭时不可新建同步对象".into());
                }
                let now = Utc::now();
                let document = if task {
                    CalendarDocument::new_task(now)
                } else {
                    CalendarDocument::new_event(
                        CalendarDate::Floating(chrono::Local::now().naive_local()),
                        now,
                    )
                }
                .map_err(failure)?;
                (
                    document,
                    None,
                    0,
                    if task {
                        binding.todo_collection.clone()
                    } else {
                        binding.event_collection.clone()
                    },
                )
            };
            let item = document
                .items()
                .map_err(failure)?
                .into_iter()
                .find(|item| matches!(item, CalendarItem::Task(_)) == task)
                .ok_or("资源不包含所选组件类型")?;
            let previous_percent = resource_id
                .and_then(|id| state.metadata.get(&projection::item_id(id, item.common())))
                .and_then(|meta| meta.previous_percent)
                .unwrap_or(0);
            let mut editor = Editor {
                resource_id: resource_id.map(str::to_owned),
                account_id: binding.account_id.clone(),
                binding_generation: binding.generation,
                collection,
                revision,
                original_ics,
                document,
                capabilities: EditCapabilities {
                    text: false,
                    dates_and_alarms: false,
                    task_state: false,
                    reason: None,
                },
                item: item.clone(),
                title: String::new(),
                description: String::new(),
                location: String::new(),
                categories: String::new(),
                start: DateInput::default(),
                end: DateInput::default(),
                status: String::new(),
                percent: String::new(),
                priority: String::new(),
                previous_percent,
                alarms: Vec::new(),
                delete_confirm: false,
                error: String::new(),
            };
            editor.select(item)?;
            Ok(editor)
        })();
        match result {
            Ok(editor) => self.calendar.caldav.editor = Some(editor),
            Err(error) => self.caldav_note(Err(error), ""),
        }
    }

    pub(crate) fn draw_caldav_editor(&mut self, ctx: &egui::Context) {
        let Some(mut editor) = self.calendar.caldav.editor.take() else {
            return;
        };
        let mut open = true;
        let mut save = false;
        let mut delete = false;
        let children = self
            .calendar
            .caldav
            .snapshot
            .as_ref()
            .map(|state| child_tasks(state, &editor.item.common().uid))
            .unwrap_or_default();
        let writable = !self.calendar.caldav.initializing
            && self.calendar.caldav.snapshot.as_ref().is_some_and(|state| {
                organizer::writable(state)
                    && state.binding.as_ref().is_some_and(|binding| {
                        binding.account_id == editor.account_id
                            && binding.generation == editor.binding_generation
                    })
                    && !state.conflicts.iter().any(|conflict| {
                        editor.resource_id.as_deref() == Some(conflict.resource_id.as_str())
                            && !conflict.resolved
                    })
            });
        egui::Window::new(if matches!(editor.item, CalendarItem::Task(_)) {
            "CalDAV 完整待办编辑器"
        } else {
            "CalDAV 完整日程编辑器"
        })
        .id(egui::Id::new("caldav-full-editor"))
        .open(&mut open)
        .default_width(520.0)
        .resizable(true)
        .vscroll(true)
        .show(ctx, |ui| {
            if !writable {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "当前为只读：双向同步关闭、正在初始化、绑定变化或资源存在冲突。",
                );
            }
            ui.small(format!("UID：{}", editor.item.common().uid));
            ui.small(format!("集合：{}", editor.collection));
            if let Ok(items) = editor.document.items() {
                if items.len() > 1 {
                    ui.label(format!(
                        "完整资源包含 {} 个组件（主系列 / 例外）。删除将删除整个资源。",
                        items.len()
                    ));
                    let mut selected = None;
                    egui::ComboBox::from_id_salt("caldav-component")
                        .selected_text("切换组件（放弃当前未保存修改）")
                        .show_ui(ui, |ui| {
                            for item in items {
                                let label = format!(
                                    "{} · {:?}",
                                    item.common().title.as_deref().unwrap_or("（无标题）"),
                                    item.common().recurrence_id
                                );
                                if ui
                                    .selectable_label(
                                        item.common().index == editor.item.common().index,
                                        label,
                                    )
                                    .clicked()
                                {
                                    selected = Some(item);
                                }
                            }
                        });
                    if let Some(item) = selected {
                        if let Err(error) = editor.select(item) {
                            editor.error = error;
                        }
                    }
                }
            }
            if let Some(reason) = &editor.capabilities.reason {
                ui.colored_label(ui.visuals().warn_fg_color, reason);
            }
            ui.add_enabled_ui(writable, |ui| {
                ui.add_enabled_ui(editor.capabilities.text, |ui| {
                    ui.label("标题");
                    ui.text_edit_singleline(&mut editor.title);
                    ui.label("描述");
                    ui.add(
                        egui::TextEdit::multiline(&mut editor.description)
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                    ui.label("地点");
                    ui.text_edit_singleline(&mut editor.location);
                    ui.label("标签（逗号分隔）");
                    ui.text_edit_singleline(&mut editor.categories);
                });
                ui.add_enabled_ui(editor.capabilities.dates_and_alarms, |ui| {
                    editor.start.draw(ui, "开始 · DTSTART");
                    editor.end.draw(
                        ui,
                        if matches!(editor.item, CalendarItem::Task(_)) {
                            "截止 · DUE"
                        } else {
                            "结束 · DTEND（不包含此边界）"
                        },
                    );
                });
                let duration = match &editor.item {
                    CalendarItem::Event(item) => item.duration.as_ref(),
                    CalendarItem::Task(item) => item.duration.as_ref(),
                };
                if let Some(duration) = duration {
                    ui.small(format!(
                        "原始 DURATION：{}；明确填写结束 / 截止才会替换它。",
                        duration.as_str()
                    ));
                }
                if let CalendarItem::Task(task) = &editor.item {
                    ui.add_enabled_ui(editor.capabilities.task_state, |ui| {
                        let old = editor.status.clone();
                        egui::ComboBox::from_id_salt("caldav-task-status")
                            .selected_text(status_name(&editor.status))
                            .show_ui(ui, |ui| {
                                for status in
                                    ["NEEDS-ACTION", "IN-PROCESS", "COMPLETED", "CANCELLED"]
                                {
                                    ui.selectable_value(
                                        &mut editor.status,
                                        status.to_owned(),
                                        status_name(status),
                                    );
                                }
                            });
                        if editor.status != old {
                            if editor.status == "COMPLETED" {
                                editor.percent = "100".into();
                            } else if old == "COMPLETED" {
                                editor.percent = editor.previous_percent.to_string();
                            }
                        }
                        ui.label("进度 0–100（留空移除）");
                        ui.add_enabled(
                            editor.status != "COMPLETED",
                            egui::TextEdit::singleline(&mut editor.percent),
                        );
                    });
                    ui.label("优先级 0–9（1 最高，9 最低，0 未指定；留空移除）");
                    ui.text_edit_singleline(&mut editor.priority);
                    if let Some(at) = task.completed_at {
                        ui.small(format!("完成时刻（UTC）：{}", at.to_rfc3339()));
                    }
                    for relation in &task.relations {
                        ui.label(format!(
                            "关联 {}：{}",
                            relation.reltype.as_deref().unwrap_or("PARENT"),
                            relation.uid
                        ));
                    }
                    for child in &children {
                        ui.label(format!("子任务：{child}（不会级联完成或删除）"));
                    }
                    if let Some(order) = &task.sort_order {
                        ui.label(format!("X-APPLE-SORT-ORDER：{order}（保留原值）"));
                    }
                }
                draw_alarms(ui, &mut editor);
                save = ui.button("保存并排队同步").clicked();
                if editor.resource_id.is_some() {
                    ui.separator();
                    ui.checkbox(
                        &mut editor.delete_confirm,
                        "确认删除整个资源（含完整重复系列及全部例外），下次同步会删除远端资源",
                    );
                    delete = ui
                        .add_enabled(
                            editor.delete_confirm,
                            egui::Button::new("删除整个资源 / 系列"),
                        )
                        .clicked();
                }
            });
            if !editor.error.is_empty() {
                ui.colored_label(ui.visuals().error_fg_color, &editor.error);
            }
        });
        if save || delete {
            let result = self.caldav_save_editor(&editor, delete);
            match result {
                Ok(()) => {
                    self.caldav_note(
                        Ok(()),
                        if delete {
                            "已记录整资源删除意图，等待同步"
                        } else {
                            "已保存本机修改，等待同步"
                        },
                    );
                    return;
                }
                Err(error) => editor.error = error,
            }
        }
        if open {
            self.calendar.caldav.editor = Some(editor);
        }
    }

    fn caldav_save_editor(&mut self, editor: &Editor, delete: bool) -> Result<(), String> {
        if self.calendar.caldav.initializing {
            return Err("初始化或模式切换期间禁止保存".into());
        }
        let document = if delete {
            None
        } else {
            Some(editor.patched()?)
        };
        organizer::transaction(|state| {
            let binding = state
                .binding
                .as_ref()
                .ok_or("同步绑定已变化，请重新打开编辑器")?;
            if !organizer::writable(state)
                || binding.account_id != editor.account_id
                || binding.generation != editor.binding_generation
            {
                return Err("当前绑定或双向模式已变化，请重新打开编辑器".into());
            }
            if let Some(id) = &editor.resource_id {
                let resource = state
                    .resources
                    .get(id)
                    .ok_or("资源已被删除，请重新打开编辑器")?;
                if resource.revision != editor.revision || resource.local_ics != editor.original_ics
                {
                    return Err(
                        "编辑期间资源已被后台更新，已拒绝覆盖。请关闭并重新打开编辑器。".into(),
                    );
                }
                if resource.account_id != editor.account_id
                    || resource.collection_url != editor.collection
                {
                    return Err("资源目标集合发生变化，请重新打开编辑器".into());
                }
                if !delete {
                    if let CalendarItem::Task(task) = &editor.item {
                        if editor.status == "COMPLETED"
                            && !matches!(task.status, Some(TaskStatus::Completed))
                        {
                            let metadata = state
                                .metadata
                                .entry(projection::item_id(id, &task.common))
                                .or_default();
                            metadata.previous_status = task
                                .status
                                .as_ref()
                                .map(|status| status.as_str().to_owned());
                            metadata.previous_percent = task.percent_complete;
                        }
                    }
                }
                // 空 patch 不生成 revision 或重复上传。
                if !delete
                    && document
                        .as_ref()
                        .is_some_and(|next| next.to_ics() == editor.document.to_ics())
                {
                    return Ok(());
                }
                organizer::stage(state, id, document.as_ref().map(CalendarDocument::to_ics))?;
            } else {
                if delete {
                    return Err("尚未创建的资源不能删除".into());
                }
                if editor.collection != binding.event_collection
                    && editor.collection != binding.todo_collection
                {
                    return Err("新建目标集合已变化".into());
                }
                organizer::create_resource(
                    state,
                    &editor.collection,
                    document.as_ref().ok_or("缺少新建资源正文")?.to_ics(),
                )?;
            }
            projection::refresh(state)
        })?;
        self.reload_organizer()?;
        self.pending.push(Cmd::CalendarSync);
        Ok(())
    }
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}
fn policy_name(policy: ConflictPolicy) -> &'static str {
    match policy {
        ConflictPolicy::Ask => "Ask · 每次询问",
        ConflictPolicy::Server => "Server · 服务端优先",
        ConflictPolicy::Client => "Client · 本机优先",
    }
}
fn status_name(status: &str) -> String {
    match status {
        "NEEDS-ACTION" => "待处理 · NEEDS-ACTION".into(),
        "IN-PROCESS" => "进行中 · IN-PROCESS".into(),
        "COMPLETED" => "已完成 · COMPLETED".into(),
        "CANCELLED" => "已取消 · CANCELLED".into(),
        other => format!("未知状态 · {other}（原值保留）"),
    }
}
fn paths_match(config: &Config, path: &str, collection: &str) -> bool {
    url::Url::parse(&config.base())
        .ok()
        .and_then(|base| base.join(path).ok())
        .is_some_and(|url| url.as_str() == collection)
}
fn initialization(
    config: Config,
    events: String,
    todos: String,
    state: &organizer::State,
) -> Initialization {
    Initialization {
        config,
        events,
        todos,
        generation: state.generation,
        event_count: state.local_events.len(),
        todo_count: state.todos.len(),
        completed_count: state
            .todos
            .iter()
            .filter(|todo| todo.done_on.is_some())
            .count(),
        repeating_count: state
            .todos
            .iter()
            .filter(|todo| {
                todo.repeat != 0
                    || todo
                        .resource_id
                        .as_ref()
                        .and_then(|id| state.resources.get(id))
                        .and_then(|resource| resource.local_ics.as_deref())
                        .and_then(|raw| CalendarDocument::parse(raw).ok())
                        .is_some_and(|document| {
                            document.items().is_ok_and(|items| {
                                items.iter().any(|item| {
                                    matches!(item, CalendarItem::Task(_))
                                        && todo.resource_id.as_ref().is_some_and(|id| {
                                            projection::item_id(id, item.common()) == todo.id
                                        })
                                        && document
                                            .edit_capabilities(item.common().index)
                                            .is_ok_and(|cap| !cap.task_state)
                                })
                            })
                        })
            })
            .count(),
    }
}
fn collection_picker(
    ui: &mut egui::Ui,
    label: &str,
    collections: &[dav::Collection],
    component: &str,
    path: &mut String,
) {
    ui.label(label);
    let selected = collections
        .iter()
        .find(|collection| collection.url == *path)
        .map(collection_name)
        .unwrap_or_else(|| {
            if path.is_empty() {
                "请选择集合".into()
            } else {
                path.clone()
            }
        });
    egui::ComboBox::from_id_salt(label)
        .width(ui.available_width().min(420.0))
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for collection in collections
                .iter()
                .filter(|collection| collection.components.contains(component))
            {
                let label = format!(
                    "{} · {}",
                    collection_name(collection),
                    if collection.privileges.writable() {
                        "可写"
                    } else {
                        "只读 / 权限待验证"
                    }
                );
                ui.selectable_value(path, collection.url.clone(), label)
                    .on_hover_text(&collection.url);
            }
        });
}
fn collection_name(collection: &dav::Collection) -> String {
    collection
        .display_name
        .clone()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| collection.url.clone())
}
fn draw_statistics(ui: &mut egui::Ui, state: &organizer::State, label: &str, task: bool) {
    let collection = state.binding.as_ref().map(|binding| {
        if task {
            &binding.todo_collection
        } else {
            &binding.event_collection
        }
    });
    let ids = state
        .resources
        .values()
        .filter(|resource| {
            collection.is_some_and(|collection| collection == &resource.collection_url)
        })
        .filter(|resource| {
            resource
                .local_ics
                .as_ref()
                .or(resource.baseline.as_ref())
                .is_some_and(|raw| {
                    CalendarDocument::parse(raw)
                        .and_then(|document| document.items())
                        .is_ok_and(|items| {
                            items
                                .iter()
                                .any(|item| matches!(item, CalendarItem::Task(_)) == task)
                        })
                })
        })
        .map(|resource| resource.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let pending = state
        .pending
        .iter()
        .filter(|change| ids.contains(change.resource_id.as_str()))
        .collect::<Vec<_>>();
    let failures = pending
        .iter()
        .filter(|change| change.error.is_some())
        .count();
    let conflicts = state
        .conflicts
        .iter()
        .filter(|conflict| !conflict.resolved && ids.contains(conflict.resource_id.as_str()))
        .count();
    ui.label(format!(
        "{label} · {} 个资源 · 待上传 {} · 失败 {} · 冲突 {}",
        ids.len(),
        pending.len(),
        failures,
        conflicts
    ));
    if let Some(collection) = collection.and_then(|url| state.collections.get(url)) {
        if let Some(at) = &collection.last_sync {
            ui.small(format!("上次同步：{at}"));
        }
        if let Some(error) = &collection.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }
    for change in pending.into_iter().filter(|change| change.error.is_some()) {
        ui.colored_label(
            ui.visuals().error_fg_color,
            change.error.as_deref().unwrap_or_default(),
        );
    }
}
fn conflict_body(ui: &mut egui::Ui, label: &str, body: &Option<String>) {
    egui::CollapsingHeader::new(label).show(ui, |ui| match body {
        Some(body) => {
            let mut text = body.as_str();
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .code_editor()
                    .desired_rows(8)
                    .desired_width(f32::INFINITY)
                    .interactive(false),
            );
        }
        None => {
            ui.colored_label(ui.visuals().warn_fg_color, "删除意图 / 该版本不存在");
        }
    });
}
fn draw_alarms(ui: &mut egui::Ui, editor: &mut Editor) {
    ui.separator();
    ui.strong("提醒 · VALARM");
    for alarm in &editor.item.common().alarms {
        if !alarm.editable {
            ui.small(format!(
                "提醒 #{}：{} · {:?}（复杂提醒保留原文，只读）",
                alarm.index + 1,
                alarm.action.as_deref().unwrap_or("未知 ACTION"),
                alarm.trigger
            ));
        }
    }
    ui.add_enabled_ui(editor.capabilities.dates_and_alarms, |ui| {
        for (row, alarm) in editor.alarms.iter_mut().enumerate() {
            ui.push_id(("caldav-alarm", row), |ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.checkbox(
                        &mut alarm.removed,
                        if alarm.index.is_some() {
                            "删除此提醒"
                        } else {
                            "撤销此新增提醒"
                        },
                    );
                    ui.add_enabled_ui(!alarm.removed, |ui| {
                        egui::ComboBox::from_id_salt("trigger")
                            .selected_text(alarm_kind(alarm.kind))
                            .show_ui(ui, |ui| {
                                for kind in 0..3 {
                                    ui.selectable_value(&mut alarm.kind, kind, alarm_kind(kind));
                                }
                            });
                        ui.add(egui::TextEdit::singleline(&mut alarm.value).hint_text(
                            if alarm.kind == 2 {
                                "UTC：YYYY-MM-DD HH:MM:SS"
                            } else {
                                "RFC 5545 DURATION，例如 -PT15M、PT0S"
                            },
                        ));
                        ui.label("提醒正文");
                        ui.text_edit_singleline(&mut alarm.description);
                    });
                });
            });
        }
        if ui.button("添加单独提醒（保留其他全部 VALARM）").clicked() {
            let absolute = editor.start.value.trim().is_empty();
            editor.alarms.push(AlarmInput {
                index: None,
                original: None,
                kind: if absolute { 2 } else { 0 },
                value: if absolute {
                    (Utc::now() + chrono::Duration::hours(1))
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string()
                } else {
                    "-PT15M".into()
                },
                description: if editor.title.is_empty() {
                    "提醒".into()
                } else {
                    editor.title.clone()
                },
                removed: false,
            });
        }
    });
}
fn alarm_kind(kind: u8) -> &'static str {
    match kind {
        0 => "相对开始 · RELATED=START",
        1 => "相对结束 / 截止 · RELATED=END",
        _ => "绝对时刻 · UTC",
    }
}
fn child_tasks(state: &organizer::State, parent: &str) -> Vec<String> {
    let mut children = Vec::new();
    for resource in state.resources.values() {
        let Some(raw) = &resource.local_ics else {
            continue;
        };
        let Ok(document) = CalendarDocument::parse(raw) else {
            continue;
        };
        let Ok(items) = document.items() else {
            continue;
        };
        for item in items {
            if let CalendarItem::Task(task) = item {
                if task.relations.iter().any(|relation| {
                    relation.uid == parent
                        && relation
                            .reltype
                            .as_deref()
                            .is_none_or(|kind| kind.eq_ignore_ascii_case("PARENT"))
                }) {
                    children.push(format!(
                        "{} · {}",
                        task.common.title.as_deref().unwrap_or("（无标题）"),
                        task.common.uid
                    ));
                }
            }
        }
    }
    children
}

#[cfg(test)]
mod tests {
    use super::*;

    const TASK: &str = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//UI Tests//ZH\r\n",
        "BEGIN:VTODO\r\nUID:ui-task\r\nDTSTAMP:20261108T000000Z\r\n",
        "SUMMARY:旧标题\r\nDESCRIPTION;X-PARAM=retain:原始描述\r\n",
        "DTSTART;TZID=Asia/Shanghai:20261109T090000\r\nDUE;TZID=Asia/Shanghai:20261109T100000\r\n",
        "STATUS:IN-PROCESS\r\nPERCENT-COMPLETE:37\r\nRELATED-TO;RELTYPE=PARENT:parent\r\nX-APPLE-SORT-ORDER:42.5\r\n",
        "X-UNKNOWN;X-PARAM=retain:不要丢失\r\n",
        "BEGIN:VALARM\r\nACTION:AUDIO\r\nTRIGGER:-PT1H\r\nATTACH:ding.wav\r\nEND:VALARM\r\n",
        "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nDESCRIPTION:第一个可编辑提醒\r\nEND:VALARM\r\n",
        "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT5M\r\nDESCRIPTION:第二个可编辑提醒\r\nEND:VALARM\r\n",
        "END:VTODO\r\nEND:VCALENDAR\r\n"
    );

    fn editor(raw: &str) -> Editor {
        let document = CalendarDocument::parse(raw).unwrap();
        let item = document.items().unwrap().remove(0);
        let capabilities = document.edit_capabilities(item.common().index).unwrap();
        let mut editor = Editor {
            resource_id: Some("resource".into()),
            account_id: "account".into(),
            binding_generation: 1,
            collection: "https://example.test/tasks/".into(),
            revision: 2,
            original_ics: Some(raw.into()),
            document,
            item: item.clone(),
            capabilities,
            title: String::new(),
            description: String::new(),
            location: String::new(),
            categories: String::new(),
            start: DateInput::default(),
            end: DateInput::default(),
            status: String::new(),
            percent: String::new(),
            priority: String::new(),
            previous_percent: 0,
            alarms: Vec::new(),
            delete_confirm: false,
            error: String::new(),
        };
        editor.select(item).unwrap();
        editor
    }

    fn browser_row(
        task: bool,
        title: &str,
        uid: &str,
        collection: &str,
        href: &str,
        status: &str,
        error: Option<&str>,
    ) -> BrowserRow {
        BrowserRow {
            resource_id: "resource".into(),
            task,
            title: title.into(),
            uid: uid.into(),
            collection: collection.into(),
            href: href.into(),
            status: status.into(),
            editable: error.is_none(),
            error: error.map(str::to_owned),
        }
    }

    #[test]
    fn browser_filters_match_type_status_collection_and_case_insensitive_text() {
        let event = browser_row(
            false,
            "团队周会",
            "event-1",
            "https://example.test/events/",
            "https://example.test/events/1.ics",
            "CONFIRMED",
            None,
        );
        let completed = browser_row(
            true,
            "提交报销",
            "task-1",
            "https://example.test/tasks/",
            "https://example.test/tasks/1.ics",
            "COMPLETED",
            None,
        );
        let pending = browser_row(
            true,
            "准备周报",
            "task-2",
            "https://example.test/tasks/",
            "https://example.test/tasks/2.ics",
            "IN-PROCESS",
            None,
        );
        let broken = browser_row(
            false,
            "无法解析的同步资源",
            "",
            "https://example.test/events/",
            "https://example.test/events/broken.ics",
            "原始 ICS 保留",
            Some("ICS 错误"),
        );

        assert!(browser_row_matches(&event, "团队", 1, 0, ""));
        assert!(!browser_row_matches(&event, "团队", 2, 0, ""));
        assert!(browser_row_matches(
            &pending,
            "TASK-2",
            2,
            1,
            "https://example.test/tasks/"
        ));
        assert!(!browser_row_matches(&completed, "", 2, 1, ""));
        assert!(browser_row_matches(&completed, "", 2, 2, ""));
        assert!(!browser_row_matches(&pending, "", 2, 3, ""));
        assert!(browser_row_matches(
            &broken,
            "BROKEN.ICS",
            0,
            0,
            "https://example.test/events/"
        ));
        assert!(!browser_row_matches(&broken, "", 0, 2, ""));
    }

    #[test]
    fn title_edit_preserves_state_dates_unknown_fields_and_all_alarms() {
        let mut editor = editor(TASK);
        assert_eq!(editor.patched().unwrap().to_ics(), editor.document.to_ics());
        editor.title = "新标题".into();
        let document = editor.patched().unwrap();
        let CalendarItem::Task(task) = document.items().unwrap().remove(0) else {
            panic!("期望 VTODO")
        };
        assert_eq!(task.common.title.as_deref(), Some("新标题"));
        assert_eq!(task.status, Some(TaskStatus::InProcess));
        assert_eq!(task.percent_complete, Some(37));
        assert_eq!(task.common.alarms.len(), 3);
        assert_eq!(task.start, editor.start.parse().unwrap());
        assert_eq!(task.relations[0].uid, "parent");
        assert_eq!(task.sort_order.as_deref(), Some("42.5"));
        let raw = document.to_ics();
        assert!(raw.contains("X-UNKNOWN;X-PARAM=retain:不要丢失\r\n"));
        assert!(raw.contains("DESCRIPTION;X-PARAM=retain:原始描述\r\n"));
        assert!(raw.contains("ACTION:AUDIO\r\nTRIGGER:-PT1H\r\nATTACH:ding.wav"));
    }

    #[test]
    fn alarm_edits_use_original_valarm_indices_with_complex_alarm_before_them() {
        let mut editor = editor(TASK);
        assert_eq!(editor.alarms[0].index, Some(1));
        editor.alarms[0].removed = true;
        editor.alarms[1].value = "-PT2M".into();
        editor.alarms.push(AlarmInput {
            index: None,
            original: None,
            kind: 2,
            value: "2026-11-09 00:30:00".into(),
            description: "新增独立提醒".into(),
            removed: false,
        });
        let changes = editor.alarm_edits().unwrap();
        assert!(matches!(changes[0], AlarmEdit::Remove { index: 1 }));
        assert!(matches!(changes[1], AlarmEdit::Update { index: 2, .. }));
        let document = editor.patched().unwrap();
        let task = document.items().unwrap().remove(0);
        let alarms = &task.common().alarms;
        assert_eq!(alarms.len(), 3);
        assert_eq!(alarms[0].action.as_deref(), Some("AUDIO"));
        assert_eq!(alarms[1].description.as_deref(), Some("第二个可编辑提醒"));
        assert_eq!(alarms[2].description.as_deref(), Some("新增独立提醒"));
    }

    #[test]
    fn recurring_task_keeps_text_editable_but_rejects_state_and_date_changes() {
        let raw = TASK.replace(
            "STATUS:IN-PROCESS",
            "RRULE:FREQ=WEEKLY\r\nSTATUS:IN-PROCESS",
        );
        let mut editor = editor(&raw);
        assert!(!editor.capabilities.task_state);
        assert!(!editor.capabilities.dates_and_alarms);
        editor.title = "只改标题".into();
        assert!(editor
            .patched()
            .unwrap()
            .to_ics()
            .contains("RRULE:FREQ=WEEKLY"));
        editor.status = "COMPLETED".into();
        assert!(editor.patched().is_err());
    }

    #[test]
    fn initialization_counts_all_local_items_and_cancel_does_not_change_the_snapshot() {
        let mut current = organizer::State {
            generation: 42,
            ..Default::default()
        };
        current.local_events.push(crate::local_calendar::Entry {
            event: crate::mail::Meeting {
                all_day: false,
                title: "本机日程".into(),
                start: 0,
                end: Some(3600),
                at: String::new(),
                uid: "event".into(),
            },
            reminder: None,
            resource_id: None,
            sync_reminders: Vec::new(),
        });
        let mut completed = crate::model::TodoItem::new("完成且重复");
        completed.done_on = Some("2026-11-08".into());
        completed.repeat = 31;
        current.todos.push(completed);
        current.todos.push(crate::model::TodoItem::new("普通待办"));
        let raw = TASK.replace(
            "STATUS:IN-PROCESS",
            "RRULE:FREQ=WEEKLY\r\nSTATUS:IN-PROCESS",
        );
        let task = CalendarDocument::parse(&raw)
            .unwrap()
            .items()
            .unwrap()
            .remove(0);
        let mut repeated = crate::model::TodoItem::new("服务端重复待办");
        repeated.id = projection::item_id("resource", task.common());
        repeated.resource_id = Some("resource".into());
        current.todos.push(repeated);
        current.resources.insert(
            "resource".into(),
            organizer::Resource {
                id: "resource".into(),
                account_id: "account".into(),
                collection_url: "https://example.test/tasks/".into(),
                href: "https://example.test/tasks/task.ics".into(),
                etag: None,
                local_ics: Some(raw),
                baseline: None,
                revision: 1,
            },
        );
        let before = serde_json::to_value(&current).unwrap();
        let mut ui = State {
            snapshot: Some(current.clone()),
            ..Default::default()
        };
        let config = Config {
            provider: Provider::Custom,
            ..Default::default()
        };
        ui.request_initialization(config, "/events/".into(), "/tasks/".into(), &current)
            .unwrap();
        let warning = ui.confirmation.as_ref().unwrap();
        assert_eq!(
            (
                warning.event_count,
                warning.todo_count,
                warning.completed_count,
                warning.repeating_count
            ),
            (1, 3, 1, 2)
        );
        assert_eq!(warning.generation, 42);
        assert!(!ui.initializing);
        assert!(ui.operation.is_none());
        ui.cancel_initialization();
        assert!(ui.confirmation.is_none());
        assert!(!ui.initializing);
        assert!(ui.operation.is_none());
        assert_eq!(serde_json::to_value(&current).unwrap(), before);
        assert_eq!(
            serde_json::to_value(ui.snapshot.as_ref().unwrap()).unwrap(),
            before
        );
    }

    #[test]
    fn only_custom_provider_can_request_two_way_initialization() {
        let current = organizer::State::default();
        for provider in Provider::ALL {
            let mut state = State::default();
            let result = state.request_initialization(
                Config {
                    provider,
                    ..Default::default()
                },
                "/events/".into(),
                "/tasks/".into(),
                &current,
            );
            assert_eq!(result.is_ok(), provider == Provider::Custom);
            assert_eq!(state.confirmation.is_some(), provider == Provider::Custom);
            assert!(!state.initializing);
            assert!(state.operation.is_none());
        }
    }

    #[test]
    fn stale_generation_results_cannot_unfreeze_or_replace_a_newer_operation() {
        let mut current = organizer::State {
            generation: 55,
            ..Default::default()
        };
        current.binding = Some(organizer::Binding {
            account_id: "new-account".into(),
            generation: 4,
            event_collection: "/events/".into(),
            todo_collection: "/tasks/".into(),
            switching: false,
        });
        let mut cached = current.clone();
        cached.generation = 52;
        let state = State {
            initializing: true,
            operation: Some(("new-account".into(), 50)),
            snapshot: Some(cached),
            note: "正在执行新的初始化".into(),
            ..Default::default()
        };
        for (account, generation, success) in [
            ("new-account", 49, false),
            ("new-account", 50, true),
            ("new-account", 51, false),
            ("old-account", 55, true),
            ("new-account", 56, true),
        ] {
            assert_eq!(
                state.classify_result(account, generation, success, &current),
                None
            );
            assert!(state.initializing);
            assert_eq!(state.operation.as_ref(), Some(&("new-account".into(), 50)));
            assert_eq!(state.snapshot.as_ref().unwrap().generation, 52);
            assert_eq!(state.note, "正在执行新的初始化");
        }
        assert_eq!(
            state.classify_result("new-account", 55, true, &current),
            Some(true)
        );
        let idle = State {
            snapshot: state.snapshot.clone(),
            ..Default::default()
        };
        assert_eq!(
            idle.classify_result("new-account", 51, true, &current),
            None
        );
        assert_eq!(
            idle.classify_result("new-account", 55, true, &current),
            Some(false)
        );
    }

    #[test]
    fn date_input_preserves_all_four_time_types_without_implicit_conversion() {
        let local = NaiveDate::from_ymd_opt(2026, 11, 8)
            .unwrap()
            .and_hms_opt(9, 30, 17)
            .unwrap();
        for date in [
            CalendarDate::Date(local.date()),
            CalendarDate::Utc(local.and_utc()),
            CalendarDate::Zoned {
                local,
                tzid: "Asia/Shanghai".into(),
            },
            CalendarDate::Floating(local),
        ] {
            let input = DateInput::from_date(Some(&date));
            assert_eq!(input.parse().unwrap(), Some(date.clone()));
            assert_eq!(date_patch(&input, Some(&date)).unwrap(), Patch::Keep);
        }
        assert_eq!(DateInput::default().parse().unwrap(), None);
    }
}
