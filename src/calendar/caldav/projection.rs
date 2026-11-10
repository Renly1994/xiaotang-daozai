//! 完整资源到旧界面的投影；只把用户真正修改的字段合回 ICS。
use super::ical::*;
use crate::organizer::{self, State};
use chrono::{DateTime, Duration, Local, NaiveDateTime, Offset, TimeZone, Utc};
use std::result::Result;

fn failure(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub(crate) fn timestamp(date: &CalendarDate) -> Option<i64> {
    match date {
        CalendarDate::Date(day) => Local
            .from_local_datetime(&day.and_hms_opt(0, 0, 0)?)
            .single()
            .map(|at| at.timestamp()),
        CalendarDate::Floating(local) => Local
            .from_local_datetime(local)
            .single()
            .map(|at| at.timestamp()),
        _ => date.to_utc(None).ok().map(|at| at.timestamp()),
    }
}

pub(crate) fn item_id(resource: &str, common: &CommonItem) -> String {
    format!("{resource}:{}:{:?}", common.uid, common.recurrence_id)
}

// RFC 5545：先偏移周/日（本地日历），再偏移小时/分钟/秒（精确时间）。
// nominal_seconds 仅用来提取时分秒，不能把 P1D 当成 PT24H。
fn duration_parts(duration: &IcalDuration) -> Option<(Duration, Duration)> {
    let raw = duration.as_str();
    let sign = if raw.starts_with('-') { -1i64 } else { 1 };
    let body = raw.trim_start_matches(['-', '+']).strip_prefix('P')?;
    let date = body.split('T').next()?;
    let days = if date.is_empty() {
        0
    } else if let Some(weeks) = date.strip_suffix('W') {
        weeks.parse::<i64>().ok()?.checked_mul(7)?
    } else {
        date.strip_suffix('D')?.parse::<i64>().ok()?
    }
    .checked_mul(sign)?;
    let seconds = duration
        .nominal_seconds()
        .checked_sub(days.checked_mul(86400)?)?;
    Some((Duration::try_days(days)?, Duration::try_seconds(seconds)?))
}

fn shift_local<T: TimeZone>(
    local: NaiveDateTime,
    zone: T,
    days: Duration,
    seconds: Duration,
) -> Option<NaiveDateTime> {
    // 原始锚点及日历偏移后的时刻都必须唯一，不能选择 earliest 来猜测。
    zone.from_local_datetime(&local).single()?;
    let shifted = local.checked_add_signed(days)?;
    let at = zone
        .from_local_datetime(&shifted)
        .single()?
        .checked_add_signed(seconds)?
        .with_timezone(&zone);
    let result = at
        .naive_utc()
        .checked_add_signed(Duration::seconds(i64::from(
            at.offset().fix().local_minus_utc(),
        )))?;
    // CalendarDate::Zoned/Floating 无法记录 DST fold；结果有歧义时暂不提醒。
    (zone.from_local_datetime(&result).single()?.timestamp() == at.timestamp()).then_some(result)
}

fn shift_date(anchor: &CalendarDate, duration: &IcalDuration) -> Option<CalendarDate> {
    let (days, seconds) = duration_parts(duration)?;
    match anchor {
        CalendarDate::Utc(at) => Some(CalendarDate::Utc(
            at.checked_add_signed(days)?.checked_add_signed(seconds)?,
        )),
        CalendarDate::Zoned { local, tzid } => Some(CalendarDate::Zoned {
            local: shift_local(*local, tzid.parse::<chrono_tz::Tz>().ok()?, days, seconds)?,
            tzid: tzid.clone(),
        }),
        CalendarDate::Floating(local) => Some(CalendarDate::Floating(shift_local(
            *local, Local, days, seconds,
        )?)),
        CalendarDate::Date(day) => {
            let local = shift_local(day.and_hms_opt(0, 0, 0)?, Local, days, seconds)?;
            Some(if seconds == Duration::zero() {
                CalendarDate::Date(local.date())
            } else {
                CalendarDate::Floating(local)
            })
        }
    }
}

pub(crate) fn alarm_timestamp(
    alarm: &Alarm,
    start: Option<&CalendarDate>,
    end: Option<&CalendarDate>,
) -> Option<i64> {
    // REPEAT/DURATION、非 DISPLAY、未知 TRIGGER 等只保留原文，不推测触发方式。
    if !alarm.editable || alarm.action.as_deref() != Some("DISPLAY") {
        return None;
    }
    match alarm.trigger.as_ref().ok()? {
        AlarmTrigger::Absolute(at) => Some(at.timestamp()),
        AlarmTrigger::Relative { duration, related } => timestamp(&shift_date(
            match related {
                AlarmRelation::Start => start?,
                AlarmRelation::End => end?,
            },
            duration,
        )?),
    }
}

fn duration_end(start: Option<&CalendarDate>, duration: &IcalDuration) -> Option<CalendarDate> {
    let start = start?;
    if duration.nominal_seconds() <= 0
        || (matches!(start, CalendarDate::Date(_)) && duration.as_str().contains('T'))
    {
        return None;
    }
    shift_date(start, duration)
}

fn task_end(task: &TaskItem) -> Option<CalendarDate> {
    match (&task.due, &task.duration) {
        (Some(due), None) => Some(due.clone()),
        (None, Some(duration)) => duration_end(task.start.as_ref(), duration),
        // DUE 与 DURATION 互斥，缺少结束锚点时也不能自行假设。
        _ => None,
    }
}

pub(crate) fn task_alarm_timestamps(task: &TaskItem) -> Vec<i64> {
    let end = task_end(task);
    let mut alarms = task
        .common
        .alarms
        .iter()
        .filter_map(|alarm| alarm_timestamp(alarm, task.start.as_ref(), end.as_ref()))
        .collect::<Vec<_>>();
    alarms.sort_unstable();
    alarms.dedup();
    alarms
}

// 从展示实例的 Unix 时间恢复源 DTSTART 的时间语义，禁止丢掉 TZID 后按 UTC 加日。
fn instance_start(template: &CalendarDate, at: i64) -> Option<CalendarDate> {
    let utc = DateTime::from_timestamp(at, 0)?;
    let date = match template {
        CalendarDate::Utc(_) => CalendarDate::Utc(utc),
        CalendarDate::Zoned { tzid, .. } => CalendarDate::Zoned {
            local: utc
                .with_timezone(&tzid.parse::<chrono_tz::Tz>().ok()?)
                .naive_local(),
            tzid: tzid.clone(),
        },
        CalendarDate::Floating(_) => {
            CalendarDate::Floating(utc.with_timezone(&Local).naive_local())
        }
        CalendarDate::Date(_) => CalendarDate::Date(utc.with_timezone(&Local).date_naive()),
    };
    (timestamp(&date)? == at).then_some(date)
}

fn event_end(item: &EventItem, start: &CalendarDate) -> Option<CalendarDate> {
    match (&item.end, &item.duration) {
        (None, Some(duration)) => duration_end(Some(start), duration),
        (Some(CalendarDate::Date(end)), None) => {
            let CalendarDate::Date(original_start) = item.start.as_ref()? else {
                return None;
            };
            let days = end.signed_duration_since(*original_start).num_days();
            if days <= 0 {
                return None;
            }
            shift_date(start, &IcalDuration::parse(&format!("P{days}D")).ok()?)
        }
        (Some(end), None) => {
            // DTEND 的重复实例沿用原对象的精确时长；DURATION 才保持名义日历时长。
            let seconds = timestamp(end)?.checked_sub(timestamp(item.start.as_ref()?)?)?;
            if seconds <= 0 {
                return None;
            }
            shift_date(start, &IcalDuration::from_seconds(seconds))
        }
        _ => None,
    }
}

pub(crate) fn event_reminder_timestamps(
    items: &[CalendarItem],
    event: &crate::mail::Meeting,
) -> Vec<i64> {
    // items 只能来自当前资源；相同 UID 的其他资源不能参与例外或 master 匹配。
    let candidates = items
        .iter()
        .filter_map(|item| match item {
            CalendarItem::Event(item) if item.common.uid == event.uid => Some(item),
            _ => None,
        })
        .collect::<Vec<_>>();
    let exceptions = candidates
        .iter()
        .copied()
        .filter(|item| {
            item.common.recurrence_id.is_some()
                && item.start.as_ref().and_then(timestamp) == Some(event.start)
        })
        .collect::<Vec<_>>();
    let selected = match exceptions.as_slice() {
        [item] => Some(*item),
        [] => {
            let masters = candidates
                .iter()
                .copied()
                .filter(|item| item.common.recurrence_id.is_none())
                .collect::<Vec<_>>();
            match masters.as_slice() {
                [item] => Some(*item),
                _ => None,
            }
        }
        // 无法区分同一实际开始时刻的多个例外，不能误取主系列提醒。
        _ => None,
    };
    let Some(item) = selected else {
        return Vec::new();
    };
    let start = item
        .start
        .as_ref()
        .and_then(|template| instance_start(template, event.start));
    let end = start.as_ref().and_then(|start| event_end(item, start));
    let mut reminders = item
        .common
        .alarms
        .iter()
        .filter_map(|alarm| alarm_timestamp(alarm, start.as_ref(), end.as_ref()))
        .collect::<Vec<_>>();
    reminders.sort_unstable();
    reminders.dedup();
    reminders
}
// 旧 TodoItem.due 只有本地分钟文本，不能编码秒或 DST fold。
// 此类精确提醒仍在 ICS 中保留；不能截断秒或把第二个 01:30 提前到第一个 01:30。
pub(crate) fn local_reminder_text(at: i64) -> Option<String> {
    if at % 60 != 0 {
        return None;
    }
    let local = DateTime::from_timestamp(at, 0)?.with_timezone(&Local);
    if Local
        .from_local_datetime(&local.naive_local())
        .single()?
        .timestamp()
        != at
    {
        return None;
    }
    Some(format!("{}", local.format("%Y-%m-%d %H:%M")))
}

fn reminder_key(id: &str, due: &str) -> String {
    format!("reminder:{id}:{due}")
}

pub fn refresh(state: &mut State) -> Result<(), String> {
    if state.binding.is_none() {
        return Ok(());
    }
    let mut events = Vec::new();
    let mut todos = Vec::new();
    for resource in state.resources.values() {
        let Some(raw) = &resource.local_ics else {
            continue;
        };
        let document = CalendarDocument::parse(raw).map_err(failure)?;
        let items = document.items().map_err(failure)?;
        let now = Utc::now();
        // 仅显示窗口展开；完整资源及系列身份一直保存在仓库，绝不上传展开后的内容。
        let expanded = crate::calendar::events_from_ics(
            raw,
            now - chrono::Duration::days(366),
            now + chrono::Duration::days(732),
        );
        match expanded {
            Ok(list) => {
                for event in list {
                    let sync_reminders = event_reminder_timestamps(&items, &event);
                    events.push(crate::local_calendar::Entry {
                        event,
                        reminder: None,
                        resource_id: Some(resource.id.clone()),
                        sync_reminders,
                    });
                }
            }
            Err(error)
                if items
                    .iter()
                    .any(|item| matches!(item, CalendarItem::Event(_))) =>
            {
                return Err(error)
            }
            Err(_) => {}
        }
        for item in items {
            let CalendarItem::Task(task) = item else {
                continue;
            };
            let id = item_id(&resource.id, &task.common);
            let status = task
                .status
                .as_ref()
                .map(TaskStatus::as_str)
                .unwrap_or("NEEDS-ACTION");
            let done_on = if status == "COMPLETED" {
                task.completed_at
                    .map(|at| at.with_timezone(&Local).format("%Y-%m-%d").to_string())
                    .or_else(|| Some("已完成".into()))
            } else {
                None
            };
            let mut alarms = task_alarm_timestamps(&task)
                .into_iter()
                .filter_map(local_reminder_text)
                .collect::<Vec<_>>();
            alarms.sort();
            alarms.dedup();
            let due = alarms.into_iter().find(|due| {
                !state
                    .metadata
                    .get(&reminder_key(&id, due))
                    .is_some_and(|meta| meta.reminded)
            });
            todos.push(crate::model::TodoItem {
                id,
                resource_id: Some(resource.id.clone()),
                sync_status: Some(status.into()),
                text: task.common.title.unwrap_or_else(|| "（无标题待办）".into()),
                done_on,
                due,
                repeat: 0,
                reminded: false,
                reminded_on: None,
            });
        }
    }
    let order: std::collections::HashMap<_, _> = state
        .todos
        .iter()
        .enumerate()
        .map(|(i, item)| (item.id.clone(), i))
        .collect();
    todos.sort_by_key(|item| order.get(&item.id).copied().unwrap_or(usize::MAX));
    state.todos = todos;
    state.local_events = events;
    Ok(())
}

pub fn save_events(
    state: &mut State,
    entries: &[crate::local_calendar::Entry],
) -> Result<(), String> {
    if state.binding.is_some() {
        if entries != state.local_events {
            return Err("同步日程请使用 CalDAV 编辑器；关闭双向后为只读".into());
        }
    } else {
        state.local_events = entries.to_vec();
    }
    Ok(())
}

pub fn save_todos(
    state: &mut State,
    before: Option<&[crate::model::TodoItem]>,
    items: &[crate::model::TodoItem],
) -> Result<Vec<crate::model::TodoItem>, String> {
    let baseline = before.unwrap_or(&state.todos).to_vec();
    let mut ids = std::collections::HashSet::new();
    if items
        .iter()
        .any(|item| item.id.is_empty() || !ids.insert(&item.id))
    {
        return Err("待办 ID 无效，请重新载入".into());
    }
    let mut requested_order: std::collections::HashMap<String, usize> = items
        .iter()
        .enumerate()
        .map(|(index, item)| (item.id.clone(), index))
        .collect();
    let mut merged = state.todos.clone();
    for old in &baseline {
        let next = items.iter().find(|item| item.id == old.id);
        if next == Some(old) {
            continue;
        }
        let current = state.todos.iter().find(|item| item.id == old.id);
        if current != Some(old) {
            return Err("待办已被后台更新，请重新打开后编辑".into());
        }
        if let Some(resource_id) = &old.resource_id {
            let metadata_only = next.is_some_and(|next| {
                next.text == old.text
                    && next.done_on == old.done_on
                    && next.due == old.due
                    && next.repeat == old.repeat
            });
            if metadata_only {
                if let Some(due) = &old.due {
                    state
                        .metadata
                        .entry(reminder_key(&old.id, due))
                        .or_default()
                        .reminded = next.is_some_and(|next| next.reminded);
                }
            } else {
                let resource = state
                    .resources
                    .get(resource_id)
                    .ok_or("待办资源已删除")?
                    .clone();
                let mut document =
                    CalendarDocument::parse(resource.local_ics.as_deref().ok_or("待办资源已删除")?)
                        .map_err(failure)?;
                let task = document
                    .items()
                    .map_err(failure)?
                    .into_iter()
                    .find_map(|item| match item {
                        CalendarItem::Task(task)
                            if item_id(resource_id, &task.common) == old.id =>
                        {
                            Some(task)
                        }
                        _ => None,
                    })
                    .ok_or("待办组件已变化")?;
                if let Some(next) = next {
                    if next.repeat != old.repeat {
                        return Err(
                            "重复 VTODO 的跨客户端完成语义尚未验证，暂不支持修改重复规则".into(),
                        );
                    }
                    let mut patch = TaskPatch::default();
                    if next.text != old.text {
                        patch.common.title = Patch::Set(next.text.clone());
                    }
                    if next.done_on != old.done_on {
                        if matches!(
                            task.status,
                            Some(TaskStatus::Cancelled | TaskStatus::Other(_))
                        ) {
                            return Err("已取消或未知状态的待办请在完整编辑器中显式选择状态".into());
                        }
                        if next.done_on.is_some() {
                            let metadata = state.metadata.entry(old.id.clone()).or_default();
                            metadata.previous_status = task
                                .status
                                .as_ref()
                                .map(|status| status.as_str().to_owned());
                            metadata.previous_percent = task.percent_complete;
                            patch.state = Patch::Set(TaskStateChange::Complete);
                        } else {
                            let previous = state.metadata.get(&old.id).and_then(|meta| {
                                Some((
                                    status_from_str(meta.previous_status.as_deref()?),
                                    meta.previous_percent.unwrap_or(0),
                                ))
                            });
                            patch.state = Patch::Set(TaskStateChange::Reopen { previous });
                        }
                    }
                    if next.due != old.due {
                        let end = task_end(&task);
                        for alarm in &task.common.alarms {
                            let old_due = alarm_timestamp(alarm, task.start.as_ref(), end.as_ref())
                                .and_then(local_reminder_text);
                            if old_due.is_some() && old_due == old.due && alarm.editable {
                                patch.alarms.push(AlarmEdit::Remove { index: alarm.index });
                            }
                        }
                        if let Some(due) = &next.due {
                            patch
                                .alarms
                                .push(AlarmEdit::Add(absolute_alarm(due, &next.text)?));
                        }
                    }
                    document
                        .patch_task(task.common.index, &patch, Utc::now())
                        .map_err(failure)?;
                    organizer::stage(state, resource_id, Some(document.to_ics()))?;
                } else {
                    // 一个资源的主系列和例外必须整体删除；列表不能偷偷删除其中一个组件。
                    if document.items().map_err(failure)?.len() != 1 {
                        return Err(
                            "此待办属于多组件资源，请在 CalDAV 完整编辑器确认删除整个资源".into(),
                        );
                    }
                    organizer::stage(state, resource_id, None)?;
                }
            }
        }
        merged.retain(|item| item.id != old.id);
        if let Some(next) = next {
            merged.push(next.clone());
        }
    }
    for item in items
        .iter()
        .filter(|item| !baseline.iter().any(|old| old.id == item.id))
    {
        if state.todos.iter().any(|current| current.id == item.id) {
            return Err("待办 ID 已存在，请重新载入".into());
        }
        if state.binding.is_some() {
            if item.repeat != 0 {
                return Err("重复 VTODO 编辑暂未开放，请先创建普通待办".into());
            }
            let mut document = CalendarDocument::new_task(Utc::now()).map_err(failure)?;
            let index = document.items().map_err(failure)?[0].common().index;
            let mut patch = TaskPatch {
                common: CommonPatch {
                    title: Patch::Set(item.text.clone()),
                    ..Default::default()
                },
                ..Default::default()
            };
            if let Some(due) = &item.due {
                patch
                    .alarms
                    .push(AlarmEdit::Add(absolute_alarm(due, &item.text)?));
            }
            document
                .patch_task(index, &patch, Utc::now())
                .map_err(failure)?;
            let collection = state.binding.as_ref().unwrap().todo_collection.clone();
            let resource_id = organizer::create_resource(state, &collection, document.to_ics())?;
            let mut local = item.clone();
            local.id = item_id(&resource_id, document.items().map_err(failure)?[0].common());
            if let Some(index) = requested_order.get(&item.id).copied() {
                requested_order.insert(local.id.clone(), index);
            }
            local.resource_id = Some(resource_id);
            merged.insert(0, local);
        } else {
            merged.insert(0, item.clone());
        }
    }
    // 本机列表顺序由提交方决定；未涉及的后台新增条目保留。
    merged.sort_by_key(|item| requested_order.get(&item.id).copied().unwrap_or(usize::MAX));
    state.todos = merged;
    refresh(state)?;
    Ok(state.todos.clone())
}

fn absolute_alarm(due: &str, title: &str) -> Result<DisplayAlarm, String> {
    let local = chrono::NaiveDateTime::parse_from_str(due, "%Y-%m-%d %H:%M")
        .map_err(|_| "提醒日期时间无效")?;
    let at = Local
        .from_local_datetime(&local)
        .single()
        .ok_or("提醒时间不存在或有歧义")?
        .with_timezone(&Utc);
    Ok(DisplayAlarm {
        trigger: AlarmTrigger::Absolute(at),
        description: title.into(),
    })
}

pub fn import_resources(
    state: &mut State,
    files: &[(String, String)],
) -> Result<(usize, usize), String> {
    if !organizer::writable(state) {
        return Err("双向同步已关闭或正在切换，暂不能导入".into());
    }
    let binding = state.binding.clone().ok_or("尚未初始化同步")?;
    let mut added = 0;
    let mut skipped = 0;
    for (name, raw) in files {
        let document = CalendarDocument::parse(raw).map_err(|e| format!("{name}：{e}"))?;
        document
            .validate_for_write()
            .map_err(|e| format!("{name}：{e}"))?;
        let items = document.items().map_err(failure)?;
        let first = items
            .first()
            .ok_or_else(|| format!("{name}：没有 VEVENT 或 VTODO"))?;
        let event = matches!(first, CalendarItem::Event(_));
        let uid = &first.common().uid;
        if items.iter().any(|item| {
            matches!(item, CalendarItem::Event(_)) != event || &item.common().uid != uid
        }) {
            return Err(format!(
                "{name}：同步导入要求一个文件只包含同一 UID 的一个系列及例外，请按完整资源分别导出"
            ));
        }
        let collection = if event {
            &binding.event_collection
        } else {
            &binding.todo_collection
        };
        let mut known = false;
        for resource in state
            .resources
            .values()
            .filter(|resource| &resource.collection_url == collection)
        {
            if let Some(raw) = resource
                .local_ics
                .as_deref()
                .or(resource.baseline.as_deref())
            {
                let current = CalendarDocument::parse(raw).map_err(failure)?;
                if current
                    .items()
                    .map_err(failure)?
                    .iter()
                    .any(|item| &item.common().uid == uid)
                {
                    known = true;
                    break;
                }
            }
        }
        if known {
            skipped += 1;
            continue;
        }
        organizer::create_resource(state, collection, document.to_ics())?;
        added += 1;
    }
    refresh(state)?;
    Ok((added, skipped))
}

pub(crate) fn status_from_str(status: &str) -> TaskStatus {
    match status {
        "NEEDS-ACTION" => TaskStatus::NeedsAction,
        "IN-PROCESS" => TaskStatus::InProcess,
        "COMPLETED" => TaskStatus::Completed,
        "CANCELLED" => TaskStatus::Cancelled,
        other => TaskStatus::Other(other.into()),
    }
}
