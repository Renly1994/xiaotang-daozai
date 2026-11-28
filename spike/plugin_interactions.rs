//! 脚本按钮、专注事件和提醒共用插件的串行执行队列。

use super::*;
use crate::{
    paint::Card,
    plugin_canvas::{live::Hit, Kind, Node},
    plugin_canvas_ui::{LiveOptions, LiveResult},
    plugin_status::valid_text,
    plugin_status_ui::{self, ClickTarget, Interaction, InteractionResponse},
};
use eframe::egui;
use std::collections::{BTreeSet, VecDeque};
use url::{Host, Url};

fn apply_multiline_draft(status: &mut PluginStatus, form: &Values) {
    for field in status.fields.iter_mut().filter(|field| field.multiline) {
        if let Some(value) = form.get(&field.key) {
            field.value = Some(value.clone());
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum EventName {
    #[serde(rename = "focus.finished")]
    FocusFinished,
}

#[derive(Clone, Serialize)]
pub(super) struct FocusFinished {
    name: EventName,
    id: String,
    at: i64,
    minutes: u32,
    #[serde(skip)]
    sequence: u64,
}

impl FocusFinished {
    fn new(at: i64, minutes: u32) -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
        Self {
            name: EventName::FocusFinished,
            id: format!("{at}-{sequence}"),
            at,
            minutes,
            sequence,
        }
    }
}

pub(super) enum Call {
    Refresh,
    Action { id: String, form: Values },
    Event(FocusFinished),
}

impl Call {
    pub(super) fn prepare(self, input: &mut Value) -> &'static str {
        match self {
            Self::Refresh => "JSON.stringify(refresh(__input))",
            Self::Action { id, form } => {
                input["action"] = json!({ "id": id, "form": form });
                "JSON.stringify(action(__input))"
            }
            Self::Event(event) => {
                input["event"] = json!(event);
                "JSON.stringify(event(__input))"
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ScriptNotice {
    id: String,
    title: String,
    body: Option<String>,
}

impl ScriptNotice {
    pub(super) fn valid(&self) -> bool {
        valid_text(&self.id, 128)
            && valid_text(&self.title, 40)
            && self.body.as_deref().is_none_or(|body| {
                body.chars().count() <= 120 && body.chars().all(|ch| ch == '\n' || !ch.is_control())
            })
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub(super) struct NoticeHistory {
    ids: BTreeSet<String>,
    last_ms: Option<i64>,
}

impl NoticeHistory {
    pub(super) fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    #[cfg(windows)]
    fn accept(&mut self, id: &str, now_ms: i64) -> bool {
        if self
            .last_ms
            .is_some_and(|last| now_ms.saturating_sub(last) < 60_000)
            || !self.ids.insert(id.to_owned())
        {
            return false;
        }
        self.last_ms = Some(now_ms);
        true
    }
}

#[derive(Default)]
pub(super) struct InteractionState {
    pub(super) revision: u64,
    pub(super) forms: BTreeMap<String, Values>,
    pub(super) notices: BTreeMap<String, ScriptNotice>,
    calls: BTreeMap<String, VecDeque<Call>>,
    last_event: u64,
}

impl InteractionState {
    pub(super) fn has_call(&self, id: &str) -> bool {
        self.calls.contains_key(id)
    }

    pub(super) fn pop_call(&mut self, id: &str) -> Call {
        let Some(queue) = self.calls.get_mut(id) else {
            return Call::Refresh;
        };
        let call = queue.pop_front().unwrap();
        if queue.is_empty() {
            self.calls.remove(id);
        }
        call
    }

    pub(super) fn forget(&mut self, id: &str) {
        self.calls.remove(id);
        self.forms.remove(id);
        self.notices.remove(id);
    }

    pub(super) fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.calls.retain(|id, _| keep(id));
        self.forms.retain(|id, _| keep(id));
        self.notices.retain(|id, _| keep(id));
    }
}

impl Manifest {
    pub(super) fn has_capability(&self, name: &str) -> bool {
        self.capabilities
            .iter()
            .any(|capability| capability == name)
    }

    fn open_action_url(&self, raw: &str) -> Option<Url> {
        // 打开链接沿用联网取数的域名白名单；只声明 hosts 的清单也可以用。
        #[cfg(windows)]
        let network = self.network.as_ref()?;
        #[cfg(not(windows))]
        return None;
        if raw.len() > 2048
            || raw.chars().any(|ch| ch.is_control() || ch == '\\')
            || !raw.get(..8)?.eq_ignore_ascii_case("https://")
            || raw[8..].split(['/', '?', '#']).next().is_none_or(|authority| {
                authority.is_empty() || authority.contains('@')
            })
        {
            return None;
        }
        let url = Url::parse(raw).ok()?;
        let Host::Domain(host) = url.host()? else {
            return None;
        };
        (url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443)
            && network.hosts.iter().any(|allowed| allowed.eq_ignore_ascii_case(host)))
        .then_some(url)
    }

    pub(super) fn check_action_links(&self, status: &PluginStatus) -> Result<()> {
        let links = status.actions.iter().filter_map(|action| action.open.as_deref())
            .chain(status.items.iter().filter_map(|item| item.open.as_deref()));
        for link in links {
            if self.open_action_url(link).is_none() {
                return Err(fail("动作链接须为清单 network.hosts 中的 HTTPS 地址"));
            }
        }
        Ok(())
    }

    pub(super) fn check_canvas_actions(&self, nodes: &[Node]) -> Result<()> {
        for node in nodes {
            if let Some(hit) = &node.hit {
                if !self.has_capability("action") {
                    return Err(fail("画布的点击区域需要声明 action"));
                }
                if let Hit::Open(url) = hit {
                    if self.open_action_url(url).is_none() {
                        return Err(fail("动作链接须为清单 network.hosts 中的 HTTPS 地址"));
                    }
                }
            }
            if let Kind::Group { group, .. } = &node.kind {
                self.check_canvas_actions(group)?;
            }
        }
        Ok(())
    }
}

impl Record {
    pub(super) fn owner_on(&self, tasks_on: bool, calendar_on: bool, quota_on: bool) -> bool {
        let Some(package) = &self.package else {
            return false;
        };
        match package.manifest.owner.as_str() {
            "tasks" => tasks_on && !package.manifest.is_retired_lunar_preview(),
            "calendar" => calendar_on,
            "quota" => quota_on,
            "wireless" => self.source_grant().is_some(),
            "status" => {
                !self.status_page_disabled
                    && (self.source_kind().is_none() || self.source_grant().is_some())
            }
            _ => false,
        }
    }
}

impl LocalPlugins {
    pub(crate) fn focus_finished(
        &mut self,
        at: i64,
        minutes: u32,
        tasks_on: bool,
        calendar_on: bool,
        quota_on: bool,
    ) {
        self.queue_event(
            FocusFinished::new(at, minutes),
            tasks_on,
            calendar_on,
            quota_on,
        );
    }

    fn queue_event(
        &mut self,
        event: FocusFinished,
        tasks_on: bool,
        calendar_on: bool,
        quota_on: bool,
    ) {
        if event.sequence <= self.interactions.last_event {
            return;
        }
        self.interactions.last_event = event.sequence;
        for (id, record) in &self.records {
            if record.enabled
                && record.owner_on(tasks_on, calendar_on, quota_on)
                && record
                    .package
                    .as_ref()
                    .and_then(|package| package.manifest.events.as_ref())
                    .is_some_and(|events| events.contains(&event.name))
            {
                self.interactions
                    .calls
                    .entry(id.clone())
                    .or_default()
                    .push_back(Call::Event(event.clone()));
            }
        }
    }

    pub(crate) fn status_height(&self, id: &str) -> f32 {
        let layout = self.status_layout(id);
        let status = self.status_page(id).and_then(|(_, status, _)| status);
        let draft = status
            .zip(self.interactions.forms.get(id))
            .filter(|(status, _)| status.fields.iter().any(|field| field.multiline))
            .map(|(status, form)| {
                let mut status = status.clone();
                apply_multiline_draft(&mut status, form);
                status
            });
        let status = draft.as_ref().or(status);
        let default_controls = layout.is_none()
            && status.is_none_or(|status| status.canvas.is_none())
            && self
                .records
                .get(id)
                .and_then(|record| record.package.as_ref())
                .is_some_and(|package| package.manifest.has_capability("action"));
        plugin_status_ui::status_page_height(layout, status)
            + if default_controls {
                plugin_status_ui::DEFAULT_FORM_HEIGHT + status.map_or(0.0, plugin_status_ui::extra_fields_height)
            } else {
                0.0
            }
    }

    pub(crate) fn draw_status_controls(
        &mut self,
        c: &Card,
        id: &str,
        backdrop: plugin_status_ui::StatusBackdrop,
        privacy_blocked: bool,
    ) -> InteractionResponse {
        let Some((_, status, health)) = self.status_page(id) else {
            return InteractionResponse::default();
        };
        let layout = self.status_layout(id);
        let record = &self.records[id];
        let package = record.package.as_ref().unwrap();
        let logs = &self.devtools.logs;
        let revision = self.status.get(id).and_then(|state| state.as_ref().ok()).map_or(0, |state| state.0);
        let enabled = self.action_available(id);
        let pet_sheets = self.appearance_pet();
        let mut live = LiveResult::default();
        let mut paint_canvas = |card: &Card<'_>, canvas: &Arc<Canvas>, area, theme| {
            let ctx = card.ui.ctx();
            let options = LiveOptions {
                now: chrono::Utc::now(),
                revision,
                allow_action: enabled,
                interactive: card.live,
            };
            live = crate::plugin_canvas_ui::draw_live_with_pet(card, id, canvas, area, theme, options, pet_sheets.clone(), |name, pixels, fit| {
                package.canvas_images.get(name)?.texture(ctx, name, pixels, fit)
            });
            if live.timed_out && !record.from_market {
                let key = egui::Id::new(("plugin-canvas-budget-log", id));
                let report = ctx.data_mut(|data| {
                    let reported = data.get_temp::<bool>(key).unwrap_or(false);
                    data.insert_temp(key, true);
                    !reported
                });
                if report {
                    logs.append(id, vec!["画布绘制超过 4 毫秒，已回退到缓存帧".to_owned()]);
                }
            }
        };
        let (mut response, action) = if let Some(controls) = status.filter(|status| {
            health == crate::plugin_status::StatusHealth::Ready && status.has_interactions()
        }) {
            let mut status = controls.clone();
            let layout = layout.cloned();
            let form = self
                .interactions
                .forms
                .entry(id.to_owned())
                .or_insert_with(|| {
                    status
                        .fields
                        .iter()
                        .map(|field| (field.key.clone(), field.value.clone().unwrap_or_default()))
                        .collect()
                });
            apply_multiline_draft(&mut status, form);
            let mut interaction = Interaction::new(id, revision, form, enabled);
            plugin_status_ui::draw_interactive_status_page(
                c, id, &status, layout.as_ref(), backdrop, privacy_blocked, &mut interaction,
                Some(&mut paint_canvas),
            );
            interaction.finish()
        } else {
            plugin_status_ui::draw_status_page(
                c, id, status, health, layout, backdrop, privacy_blocked, Some(&mut paint_canvas),
            );
            (InteractionResponse::default(), None)
        };
        response.canvas_animating = live.animating;
        response.canvas_scroll = live.scroll_area;
        response.canvas_repaint = live.repaint_after;
        if let Some(action) = action.or_else(|| live.hit.map(ClickTarget::Canvas)) {
            self.submit_action(c.ui.ctx(), id, revision, action);
        }
        response
    }

    fn action_available(&self, id: &str) -> bool {
        !self.jobs.contains_key(id) && !self.interactions.has_call(id)
    }

    fn submit_action(
        &mut self,
        ctx: &eframe::egui::Context,
        id: &str,
        revision: u64,
        target: ClickTarget,
    ) -> bool {
        let Some((_, Some(status), StatusHealth::Ready)) = self.status_page(id) else {
            return false;
        };
        if !matches!(self.status.get(id), Some(Ok((current, _))) if *current == revision) {
            return false;
        }
        let (action, open, copy) = match target {
            ClickTarget::Button(index) => {
                let Some(button) = status.actions.get(index) else {
                    return false;
                };
                (Some(button.id.as_str()), button.open.as_deref(), button.copy.as_deref())
            }
            ClickTarget::Item(index) => {
                let Some(item) = status.items.get(index) else {
                    return false;
                };
                let action = item.action.as_deref();
                (action, item.open.as_deref(), item.copy.as_deref().filter(|_| action.is_none()))
            }
            ClickTarget::CopyItem(index) => {
                (None, None, status.items.get(index).filter(|item| item.action.is_some()).and_then(|item| item.copy.as_deref()))
            }
            ClickTarget::Canvas(ref hit) => {
                let package = self.records[id].package.as_ref().unwrap();
                if status.canvas.is_none() || !package.manifest.has_capability("action") {
                    return false;
                }
                match hit {
                    Hit::Action(action) => (Some(action.as_str()), None, None),
                    Hit::Open(url) => (None, Some(url.as_str()), None),
                    Hit::Copy(text) => (None, None, Some(text.as_str())),
                }
            }
        };
        if let Some(raw) = open {
            let manifest = &self.records[id].package.as_ref().unwrap().manifest;
            let Some(url) = manifest.open_action_url(raw) else {
                return false;
            };
            ctx.open_url(eframe::egui::OpenUrl::new_tab(url));
            return true;
        }
        if let Some(text) = copy {
            ctx.copy_text(text.to_owned());
            return true;
        }
        let Some(action) = action.filter(|_| self.action_available(id)) else {
            return false;
        };
        let action = action.to_owned();
        let values = self.interactions.forms.get(id);
        let form = status
            .fields
            .iter()
            .map(|field| {
                let value = values
                    .and_then(|values| values.get(&field.key))
                    .or(field.value.as_ref())
                    .cloned()
                    .unwrap_or_default();
                (field.key.clone(), value)
            })
            .collect();
        self.interactions
            .calls
            .entry(id.to_owned())
            .or_default()
            .push_back(Call::Action { id: action, form });
        true
    }

    #[cfg(windows)]
    pub(crate) fn take_script_notices(
        &mut self,
        enabled: bool,
        now_ms: i64,
    ) -> Vec<crate::plugin_notice::Event> {
        use crate::plugin_notice::{Completion, Event};
        let pending = std::mem::take(&mut self.interactions.notices);
        if !enabled {
            return Vec::new();
        }
        pending
            .into_iter()
            .filter_map(|(id, notice)| {
                let mut record = self.records.get(&id)?.clone();
                if !record.notice_history.accept(&notice.id, now_ms) {
                    return None;
                }
                let name = record.package.as_ref()?.manifest.name.clone();
                let data = serde_json::to_vec(&record).ok()?;
                // 提醒回执和私有数据共用可重启读取的原子快照。
                if data.len() > 2 * 1024 * 1024
                    || crate::model::atomic_write(&self.path(&id), &data).is_err()
                {
                    return None;
                }
                self.records.insert(id.clone(), record);
                Some(Event::Completed(Completion {
                    source: format!("sdk:{id}"),
                    name,
                    id: notice.id,
                    session: None,
                    markdown: notice.body.unwrap_or_default(),
                    title: Some(notice.title),
                    action_label: None,
                }))
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "plugin_interactions_tests.rs"]
mod tests;
