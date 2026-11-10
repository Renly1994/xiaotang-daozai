use super::ical::*;
use super::projection::*;
use crate::organizer::{self, Binding, Resource, State};
use chrono::TimeZone;

fn utc_timestamp(value: &str) -> i64 {
    chrono::NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ")
        .unwrap()
        .and_utc()
        .timestamp()
}

fn reminder_document(kind: &str, body: &str) -> String {
    format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//测试//ZH\r\nBEGIN:{kind}\r\nUID:reminder-test\r\nDTSTAMP:20261108T020000Z\r\n{body}END:{kind}\r\nEND:VCALENDAR\r\n")
}

fn projected_task(body: &str) -> TaskItem {
    let document = CalendarDocument::parse(&reminder_document("VTODO", body)).unwrap();
    match document.items().unwrap().remove(0) {
        CalendarItem::Task(task) => task,
        _ => panic!("应为待办"),
    }
}

fn display_alarm(trigger: &str) -> String {
    format!("BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:测试提醒\r\n{trigger}\r\nEND:VALARM\r\n")
}

fn display_event(uid: &str, start: &str) -> crate::mail::Meeting {
    crate::mail::Meeting {
        all_day: false,
        title: "测试日程".into(),
        start: utc_timestamp(start),
        end: None,
        at: String::new(),
        uid: uid.into(),
    }
}

fn fixture() -> State {
    let collection = "https://example.test/cal/";
    let mut state = State {
        binding: Some(Binding {
            account_id: "account".into(),
            generation: 1,
            event_collection: collection.into(),
            todo_collection: collection.into(),
            switching: false,
        }),
        ..Default::default()
    };
    state.accounts.push(crate::calendar::Config {
        provider: crate::calendar::Provider::Custom,
        account_id: "account".into(),
        two_way: true,
        event_collection: Some(collection.into()),
        todo_collection: Some(collection.into()),
        ..Default::default()
    });
    let raw="BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//TEST//ZH\r\nBEGIN:VTODO\r\nUID:task\r\nDTSTAMP:20261108T020000Z\r\nSUMMARY:原任务\r\nDUE;VALUE=DATE:20261112\r\nSTATUS:IN-PROCESS\r\nPERCENT-COMPLETE:40\r\nRELATED-TO;RELTYPE=PARENT:parent\r\nX-APPLE-SORT-ORDER:100\r\nX-UNKNOWN;PARAM=KEEP:value\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:提醒一\r\nTRIGGER;VALUE=DATE-TIME:20261109T010000Z\r\nEND:VALARM\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:提醒二\r\nTRIGGER;VALUE=DATE-TIME:20261110T010000Z\r\nEND:VALARM\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
    state.resources.insert(
        "resource".into(),
        Resource {
            id: "resource".into(),
            account_id: "account".into(),
            collection_url: collection.into(),
            href: format!("{collection}task.ics"),
            etag: Some("\"one\"".into()),
            local_ics: Some(raw.into()),
            baseline: Some(raw.into()),
            revision: 0,
        },
    );
    refresh(&mut state).unwrap();
    state
}
#[test]
fn day_and_week_triggers_follow_calendar_dates_across_dst() {
    for (start, expected_day, expected_clock) in [
        ("20261101T090000", "20261031T130000Z", "20261031T140000Z"),
        ("20260308T090000", "20260307T140000Z", "20260307T130000Z"),
    ] {
        let body = format!(
            "DTSTART;TZID=America/New_York:{start}\r\n{}{}",
            display_alarm("TRIGGER:-P1D"),
            display_alarm("TRIGGER:-PT24H")
        );
        let task = projected_task(&body);
        let start = task.start.as_ref();
        assert_eq!(
            alarm_timestamp(&task.common.alarms[0], start, None),
            Some(utc_timestamp(expected_day))
        );
        assert_eq!(
            alarm_timestamp(&task.common.alarms[1], start, None),
            Some(utc_timestamp(expected_clock))
        );
    }
    let task = projected_task(&format!(
        "DTSTART;TZID=America/New_York:20261107T090000\r\n{}{}",
        display_alarm("TRIGGER:-P1W"),
        display_alarm("TRIGGER:-PT168H")
    ));
    assert_eq!(
        alarm_timestamp(&task.common.alarms[0], task.start.as_ref(), None),
        Some(utc_timestamp("20261031T130000Z"))
    );
    assert_eq!(
        alarm_timestamp(&task.common.alarms[1], task.start.as_ref(), None),
        Some(utc_timestamp("20261031T140000Z"))
    );
    let task = projected_task(&format!(
        "DTSTART;TZID=America/New_York:20261101T090000\r\n{}",
        display_alarm("TRIGGER:-P1DT2H")
    ));
    assert_eq!(
        task_alarm_timestamps(&task),
        vec![utc_timestamp("20261031T110000Z")]
    );
}

#[test]
fn task_end_alarms_derive_duration_with_calendar_day_semantics() {
    let alarm = display_alarm("TRIGGER;RELATED=END:-PT15M");
    let day = projected_task(&format!(
        "DTSTART;TZID=America/New_York:20261031T090000\r\nDURATION:P1D\r\n{alarm}"
    ));
    let hours = projected_task(&format!(
        "DTSTART;TZID=America/New_York:20261031T090000\r\nDURATION:PT24H\r\n{alarm}"
    ));
    assert_eq!(
        task_alarm_timestamps(&day),
        vec![utc_timestamp("20261101T134500Z")]
    );
    assert_eq!(
        task_alarm_timestamps(&hours),
        vec![utc_timestamp("20261101T124500Z")]
    );
    let due = projected_task(&format!(
        "DUE;TZID=America/New_York:20261101T090000\r\n{alarm}"
    ));
    assert_eq!(
        task_alarm_timestamps(&due),
        vec![utc_timestamp("20261101T134500Z")]
    );
    let missing_start = projected_task(&format!("DURATION:P1D\r\n{alarm}"));
    assert!(task_alarm_timestamps(&missing_start).is_empty());
    let conflicting_end = projected_task(&format!(
        "DTSTART:20261031T130000Z\r\nDUE:20261101T140000Z\r\nDURATION:P1D\r\n{alarm}"
    ));
    assert!(task_alarm_timestamps(&conflicting_end).is_empty());
}

#[test]
fn event_instances_keep_source_timezone_for_relative_day_and_duration_end() {
    let raw = reminder_document("VEVENT", &format!("DTSTART;TZID=America/New_York:20261031T090000\r\nDURATION:P1D\r\nRRULE:FREQ=DAILY;COUNT=2\r\n{}{}", display_alarm("TRIGGER:-P1D"), display_alarm("TRIGGER;RELATED=END:-PT15M")));
    let items = CalendarDocument::parse(&raw).unwrap().items().unwrap();
    assert_eq!(
        event_reminder_timestamps(&items, &display_event("reminder-test", "20261101T140000Z")),
        vec![
            utc_timestamp("20261031T130000Z"),
            utc_timestamp("20261102T134500Z")
        ]
    );
    // DTEND 为重复实例定义精确时长；原系列跨回拨的 25 小时仍应保持 25 小时。
    let raw = reminder_document("VEVENT", &format!("DTSTART;TZID=America/New_York:20261031T090000\r\nDTEND;TZID=America/New_York:20261101T090000\r\nRRULE:FREQ=DAILY;COUNT=2\r\n{}", display_alarm("TRIGGER;RELATED=END:-PT15M")));
    let items = CalendarDocument::parse(&raw).unwrap().items().unwrap();
    assert_eq!(
        event_reminder_timestamps(&items, &display_event("reminder-test", "20261101T140000Z")),
        vec![utc_timestamp("20261102T144500Z")]
    );
}

#[test]
fn moved_exception_uses_its_own_alarms_and_resources_do_not_share_uid_matches() {
    let master_alarm = display_alarm("TRIGGER:-PT10M");
    let exception_alarm = display_alarm("TRIGGER:-PT45M");
    let raw = format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//测试//ZH\r\nBEGIN:VEVENT\r\nUID:shared\r\nDTSTAMP:20261108T020000Z\r\nDTSTART:20261030T090000Z\r\nDTEND:20261030T100000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\nSUMMARY:主系列\r\n{master_alarm}END:VEVENT\r\nBEGIN:VEVENT\r\nUID:shared\r\nDTSTAMP:20261108T020000Z\r\nRECURRENCE-ID:20261031T090000Z\r\nDTSTART:20261031T120000Z\r\nDTEND:20261031T130000Z\r\nSUMMARY:移动例外\r\n{exception_alarm}END:VEVENT\r\nEND:VCALENDAR\r\n");
    let items = CalendarDocument::parse(&raw).unwrap().items().unwrap();
    let moved = display_event("shared", "20261031T120000Z");
    assert_eq!(
        event_reminder_timestamps(&items, &moved),
        vec![utc_timestamp("20261031T111500Z")]
    );
    let no_exception_alarm = raw.replace(&exception_alarm, "");
    let without = CalendarDocument::parse(&no_exception_alarm)
        .unwrap()
        .items()
        .unwrap();
    assert!(event_reminder_timestamps(&without, &moved).is_empty());
    let other = reminder_document(
        "VEVENT",
        &format!(
            "DTSTART:20261031T120000Z\r\n{}",
            display_alarm("TRIGGER:-PT5M")
        ),
    )
    .replace("UID:reminder-test", "UID:shared");
    let mut state = fixture();
    let mut first = state.resources.remove("resource").unwrap();
    first.id = "first".into();
    first.local_ics = Some(raw.clone());
    first.baseline = Some(raw);
    let mut second = first.clone();
    second.id = "second".into();
    second.href = format!("{}second.ics", second.collection_url);
    second.local_ics = Some(other.clone());
    second.baseline = Some(other);
    state.resources.insert(first.id.clone(), first);
    state.resources.insert(second.id.clone(), second);
    refresh(&mut state).unwrap();
    let first_moved = state
        .local_events
        .iter()
        .find(|entry| {
            entry.resource_id.as_deref() == Some("first") && entry.event.start == moved.start
        })
        .unwrap();
    let second_event = state
        .local_events
        .iter()
        .find(|entry| entry.resource_id.as_deref() == Some("second"))
        .unwrap();
    assert_eq!(
        first_moved.sync_reminders,
        vec![utc_timestamp("20261031T111500Z")]
    );
    assert_eq!(
        second_event.sync_reminders,
        vec![utc_timestamp("20261031T115500Z")]
    );
}

#[test]
fn unknown_zones_ambiguous_times_and_complex_alarms_are_not_guessed() {
    let alarm = display_alarm("TRIGGER:-P1D");
    for start in [
        "DTSTART;TZID=Unknown/Office:20261102T090000",
        "DTSTART;TZID=America/New_York:20261101T013000",
        "DTSTART;TZID=America/New_York:20261102T013000",
        "DTSTART;TZID=America/New_York:20260309T023000",
    ] {
        let task = projected_task(&format!("{start}\r\n{alarm}"));
        assert!(
            task_alarm_timestamps(&task).is_empty(),
            "不能猜测提醒时间：{start}"
        );
    }
    let complex_alarm = alarm.replace("END:VALARM", "REPEAT:3\r\nDURATION:PT5M\r\nEND:VALARM");
    let raw = reminder_document(
        "VTODO",
        &format!("DTSTART:20261102T090000Z\r\n{complex_alarm}"),
    );
    let document = CalendarDocument::parse(&raw).unwrap();
    let CalendarItem::Task(task) = document.items().unwrap().remove(0) else {
        panic!("应为待办");
    };
    assert!(task_alarm_timestamps(&task).is_empty());
    assert_eq!(document.to_ics(), raw);
}

#[test]
fn date_utc_and_floating_anchors_keep_their_time_semantics() {
    let alarm = display_alarm("TRIGGER:-P1D");
    let utc = projected_task(&format!("DTSTART:20261110T090000Z\r\n{alarm}"));
    assert_eq!(
        task_alarm_timestamps(&utc),
        vec![utc_timestamp("20261109T090000Z")]
    );
    let all_day = projected_task(&format!("DTSTART;VALUE=DATE:20261110\r\n{alarm}"));
    let floating = projected_task(&format!("DTSTART:20261110T090000\r\n{alarm}"));
    let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 9).unwrap();
    for (task, hour) in [(all_day, 0), (floating, 9)] {
        let expected = chrono::Local
            .from_local_datetime(&day.and_hms_opt(hour, 0, 0).unwrap())
            .single()
            .unwrap()
            .timestamp();
        assert_eq!(task_alarm_timestamps(&task), vec![expected]);
    }
}

#[test]
fn second_precision_is_preserved_and_hidden_alarms_survive_adding_a_reminder() {
    let raw = reminder_document(
        "VTODO",
        &format!(
            "DTSTART:20261110T090000Z\r\n{}",
            display_alarm("TRIGGER:-PT30S")
        ),
    );
    let task = projected_task(&format!(
        "DTSTART:20261110T090000Z\r\n{}",
        display_alarm("TRIGGER:-PT30S")
    ));
    assert_eq!(
        task_alarm_timestamps(&task),
        vec![utc_timestamp("20261110T085930Z")]
    );
    assert_eq!(local_reminder_text(utc_timestamp("20261110T085930Z")), None);
    let event_raw = raw.replace("VTODO", "VEVENT");
    let items = CalendarDocument::parse(&event_raw)
        .unwrap()
        .items()
        .unwrap();
    assert_eq!(
        event_reminder_timestamps(&items, &display_event("reminder-test", "20261110T090000Z")),
        vec![utc_timestamp("20261110T085930Z")]
    );
    let mut state = fixture();
    let resource = state.resources.get_mut("resource").unwrap();
    resource.local_ics = Some(raw.clone());
    resource.baseline = Some(raw);
    refresh(&mut state).unwrap();
    assert_eq!(state.todos[0].due, None);
    let old = state.todos.clone();
    let mut next = old.clone();
    next[0].due = Some("2026-11-11 12:00".into());
    save_todos(&mut state, Some(&old), &next).unwrap();
    let document =
        CalendarDocument::parse(state.resources["resource"].local_ics.as_deref().unwrap()).unwrap();
    assert_eq!(document.items().unwrap()[0].common().alarms.len(), 2);
    assert!(document.to_ics().contains("TRIGGER:-PT30S\r\n"));
}

#[test]
fn title_and_reminder_edits_keep_due_status_progress_and_extensions() {
    let mut state = fixture();
    let before = state.todos.clone();
    let mut next = before.clone();
    next[0].text = "只改标题".into();
    save_todos(&mut state, Some(&before), &next).unwrap();
    let raw = state.resources["resource"].local_ics.as_ref().unwrap();
    for field in [
        "DUE;VALUE=DATE:20261112",
        "STATUS:IN-PROCESS",
        "PERCENT-COMPLETE:40",
        "X-UNKNOWN;PARAM=KEEP:value",
        "RELATED-TO;RELTYPE=PARENT:parent",
        "X-APPLE-SORT-ORDER:100",
    ] {
        assert!(raw.contains(field), "字段丢失：{field}");
    }
    let before = state.todos.clone();
    let mut next = before.clone();
    next[0].due = None;
    save_todos(&mut state, Some(&before), &next).unwrap();
    let raw = state.resources["resource"].local_ics.as_ref().unwrap();
    assert!(raw.contains("DUE;VALUE=DATE:20261112"));
    assert_eq!(
        CalendarDocument::parse(raw).unwrap().items().unwrap()[0]
            .common()
            .alarms
            .len(),
        1
    );
}
#[test]
fn reminder_metadata_never_queues_upload_and_advances_to_next_alarm() {
    let mut state = fixture();
    let before = state.todos.clone();
    let first = before[0].due.clone();
    let mut next = before.clone();
    next[0].reminded = true;
    save_todos(&mut state, Some(&before), &next).unwrap();
    assert!(state.pending.is_empty());
    assert_eq!(state.resources["resource"].revision, 0);
    assert_ne!(state.todos[0].due, first);
    assert!(state.todos[0].due.is_some());
}
#[test]
fn completion_reopen_restores_progress_and_only_real_completion_time() {
    let mut state = fixture();
    let before = state.todos.clone();
    let mut next = before.clone();
    next[0].toggle("2026-11-08");
    save_todos(&mut state, Some(&before), &next).unwrap();
    let raw = state.resources["resource"].local_ics.as_ref().unwrap();
    let doc = CalendarDocument::parse(raw).unwrap();
    let CalendarItem::Task(task) = doc.items().unwrap().remove(0) else {
        panic!()
    };
    assert_eq!(task.status, Some(TaskStatus::Completed));
    assert_eq!(task.percent_complete, Some(100));
    assert!(task.completed_at.is_some());
    let before = state.todos.clone();
    let mut next = before.clone();
    next[0].toggle("2026-11-08");
    save_todos(&mut state, Some(&before), &next).unwrap();
    let doc =
        CalendarDocument::parse(state.resources["resource"].local_ics.as_ref().unwrap()).unwrap();
    let CalendarItem::Task(task) = doc.items().unwrap().remove(0) else {
        panic!()
    };
    assert_eq!(task.status, Some(TaskStatus::InProcess));
    assert_eq!(task.percent_complete, Some(40));
    assert!(task.completed_at.is_none());
}
#[test]
fn stale_list_edit_cannot_delete_or_overwrite_background_changes() {
    let mut state = State::default();
    let first = crate::model::TodoItem::new("第一项");
    state.todos = vec![first];
    let old = state.todos.clone();
    state.todos[0].text = "后台修改".into();
    let mut next = old.clone();
    next[0].text = "旧编辑器".into();
    assert!(save_todos(&mut state, Some(&old), &next).is_err());
    assert_eq!(state.todos[0].text, "后台修改");
    let old = state.todos.clone();
    state.todos.push(crate::model::TodoItem::new("后台新增"));
    let mut next = old.clone();
    next[0].text = "正常修改".into();
    save_todos(&mut state, Some(&old), &next).unwrap();
    assert!(state.todos.iter().any(|item| item.text == "后台新增"));
}
#[test]
fn new_synced_todo_keeps_top_position_with_stable_identity() {
    let mut state = fixture();
    let old = state.todos.clone();
    let mut next = old.clone();
    next.insert(0, crate::model::TodoItem::new("新待办"));
    save_todos(&mut state, Some(&old), &next).unwrap();
    assert_eq!(state.todos[0].text, "新待办");
    let id = state.todos[0].id.clone();
    refresh(&mut state).unwrap();
    assert_eq!(state.todos[0].id, id);
    assert_eq!(state.pending.len(), 1);
}
#[test]
fn disabled_objects_and_repeating_completion_are_explicitly_blocked() {
    let mut state = fixture();
    state.accounts[0].two_way = false;
    let old = state.todos.clone();
    let mut next = old.clone();
    next[0].text = "禁止写".into();
    assert!(save_todos(&mut state, Some(&old), &next).is_err());
    let mut state = fixture();
    let resource = state.resources.get_mut("resource").unwrap();
    let raw = resource
        .local_ics
        .as_ref()
        .unwrap()
        .replace("STATUS:IN-PROCESS", "RRULE:FREQ=DAILY\r\nSTATUS:IN-PROCESS");
    resource.local_ics = Some(raw.clone());
    resource.baseline = Some(raw);
    refresh(&mut state).unwrap();
    let old = state.todos.clone();
    let mut next = old.clone();
    next[0].toggle("2026-11-08");
    assert!(save_todos(&mut state, Some(&old), &next).is_err());
    assert!(state.pending.is_empty());
}
#[test]
fn full_import_preserves_uid_and_recurrence_and_deduplicates_collection_uid() {
    let mut state = fixture();
    let raw = state.resources["resource"]
        .local_ics
        .clone()
        .unwrap()
        .replace("UID:task", "UID:imported")
        .replace("STATUS:IN-PROCESS", "RRULE:FREQ=DAILY\r\nSTATUS:IN-PROCESS");
    let files = vec![("task.ics".into(), raw)];
    assert_eq!(import_resources(&mut state, &files).unwrap(), (1, 0));
    assert_eq!(import_resources(&mut state, &files).unwrap(), (0, 1));
    assert!(state.pending[0]
        .content
        .as_ref()
        .unwrap()
        .contains("RRULE:FREQ=DAILY"));
    assert!(state.pending[0]
        .content
        .as_ref()
        .unwrap()
        .contains("UID:imported"));
    let id = state.pending[0].resource_id.clone();
    assert!(organizer::stage(&mut state, &id, None).is_ok());
}
