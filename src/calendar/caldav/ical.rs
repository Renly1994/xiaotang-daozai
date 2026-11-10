//! 无损 iCalendar 资源及有限、显式的编辑接口。
//!
//! `parse` 只建立文档树；`items` 读取 typed projection；`patch_event` /
//! `patch_task` 原子地修改指定根条目。序列化始终使用 CRLF 和 75 octet UTF-8 折行。
//! 未修改的合法原始折行、属性顺序、参数和未知组件均保留。此层不展开重复规则，
//! 不模拟 VTIMEZONE，也不承担 ETag、持久化或网络同步。

use chrono::{DateTime, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

pub type Result<T> = std::result::Result<T, IcalError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IcalError(pub String);

impl fmt::Display for IcalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for IcalError {}

fn error(message: impl Into<String>) -> IcalError {
    IcalError(message.into())
}

/// 参数保留原始拼写和编码，同时提供 RFC 6868 解码后的值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parameter {
    pub name: String,
    pub values: Vec<String>,
    pub raw: String,
}

/// 属性值仍是 ICS 编码；TEXT 使用 `text()` 解码，不能对所有属性统一解码。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentLine {
    name: String,
    parameters: Vec<Parameter>,
    value: String,
    unfolded: String,
    physical: Vec<String>,
}

impl ContentLine {
    fn parse(unfolded: String, physical: Vec<String>) -> Result<Self> {
        let colon = delimiters(&unfolded, ':')?
            .into_iter()
            .next()
            .ok_or_else(|| error("ICS 内容行缺少冒号"))?;
        let head = &unfolded[..colon];
        let parts = split_quoted(head, ';')?;
        let name = parts[0];
        check_token(name)?;
        let mut parameters = Vec::new();
        for part in &parts[1..] {
            let (name, value) = part
                .split_once('=')
                .ok_or_else(|| error("ICS 参数缺少等号"))?;
            check_token(name)?;
            let values = split_quoted(value, ',')?
                .into_iter()
                .map(|value| {
                    let value = if value.starts_with('"') && value.ends_with('"') {
                        let inner = &value[1..value.len() - 1];
                        if inner.contains('"') {
                            return Err(error("ICS 参数内部的引号必须使用 RFC 6868 转义"));
                        }
                        inner
                    } else if value.contains('"') {
                        return Err(error("ICS 参数的引号位置无效"));
                    } else {
                        value
                    };
                    Ok(decode_parameter(value))
                })
                .collect::<Result<Vec<_>>>()?;
            parameters.push(Parameter {
                name: name.to_owned(),
                values,
                raw: (*part).to_owned(),
            });
        }
        let value = unfolded[colon + 1..].to_owned();
        Ok(Self {
            name: name.to_owned(),
            parameters,
            value,
            unfolded,
            physical,
        })
    }

    fn generated(name: &str, parameters: &[Parameter], value: &str) -> Result<Self> {
        check_token(name)?;
        check_value(value)?;
        let mut line = name.to_owned();
        for parameter in parameters {
            line.push(';');
            line.push_str(&parameter.raw);
        }
        line.push(':');
        line.push_str(value);
        Self::parse(line, Vec::new())
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn parameters(&self) -> &[Parameter] {
        &self.parameters
    }
    pub fn value(&self) -> &str {
        &self.value
    }
    pub fn unfolded(&self) -> &str {
        &self.unfolded
    }
    pub fn original_lines(&self) -> &[String] {
        &self.physical
    }
    pub fn parameter(&self, name: &str) -> Option<&str> {
        self.parameters
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))?
            .values
            .first()
            .map(String::as_str)
    }
    pub fn text(&self) -> String {
        unescape_text(&self.value)
    }
    fn single_parameter(&self, name: &str) -> Result<Option<&str>> {
        let mut parameters = self
            .parameters
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case(name));
        let Some(parameter) = parameters.next() else {
            return Ok(None);
        };
        if parameters.next().is_some() || parameter.values.len() != 1 {
            return Err(error(format!(
                "{} 的 {name} 参数必须是单值且不能重复",
                self.name
            )));
        }
        Ok(parameter.values.first().map(String::as_str))
    }

    fn replace(&self, value: &str, semantic: &[&str], additions: &[Parameter]) -> Result<Self> {
        let mut parameters: Vec<_> = self
            .parameters
            .iter()
            .filter(|p| {
                !semantic
                    .iter()
                    .any(|name| p.name.eq_ignore_ascii_case(name))
            })
            .cloned()
            .collect();
        parameters.extend_from_slice(additions);
        Self::generated(&self.name, &parameters, value)
    }

    fn write_to(&self, output: &mut String) {
        if !self.physical.is_empty() && self.physical.iter().all(|line| line.len() <= 75) {
            for line in &self.physical {
                output.push_str(line);
                output.push_str("\r\n");
            }
        } else {
            output.push_str(&fold_line(&self.unfolded));
        }
    }
}

fn check_token(value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(error("ICS 属性、参数或组件名称无效"));
    }
    Ok(())
}
fn check_value(value: &str) -> Result<()> {
    if value
        .chars()
        .any(|ch| ch == '\r' || ch == '\n' || (ch.is_control() && ch != '\t'))
    {
        return Err(error("ICS 内容值包含未转义的换行或控制字符"));
    }
    Ok(())
}

// 只扫描参数头；冒号之后的属性值可以包含任意引号。
fn delimiters(value: &str, delimiter: char) -> Result<Vec<usize>> {
    let mut quoted = false;
    let mut result = Vec::new();
    for (index, ch) in value.char_indices() {
        if ch == '"' {
            quoted = !quoted;
        } else if ch == delimiter && !quoted {
            result.push(index);
            if delimiter == ':' {
                return Ok(result);
            }
        }
    }
    if quoted {
        return Err(error("ICS 参数的引号未闭合"));
    }
    Ok(result)
}
fn split_quoted(value: &str, delimiter: char) -> Result<Vec<&str>> {
    let mut previous = 0;
    let mut parts = Vec::new();
    for index in delimiters(value, delimiter)? {
        parts.push(&value[previous..index]);
        previous = index + delimiter.len_utf8();
    }
    parts.push(&value[previous..]);
    Ok(parts)
}
fn decode_parameter(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '^' {
            match chars.next() {
                Some('n' | 'N') => output.push('\n'),
                Some('\'') => output.push('"'),
                Some('^') => output.push('^'),
                Some(next) => {
                    output.push('^');
                    output.push(next);
                }
                None => output.push('^'),
            }
        } else {
            output.push(ch);
        }
    }
    output
}
fn parameter(name: &str, value: &str) -> Result<Parameter> {
    check_token(name)?;
    check_value(value)?;
    let encoded = value.replace('^', "^^").replace('"', "^'");
    let encoded = if encoded.contains([':', ';', ','])
        || encoded.starts_with(' ')
        || encoded.ends_with(' ')
    {
        format!("\"{encoded}\"")
    } else {
        encoded
    };
    Ok(Parameter {
        name: name.into(),
        values: vec![value.into()],
        raw: format!("{name}={encoded}"),
    })
}

pub fn escape_text(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\\n")
        .replace(';', "\\;")
        .replace(',', "\\,")
}
pub fn unescape_text(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n' | 'N') => output.push('\n'),
                Some(next @ ('\\' | ';' | ',')) => output.push(next),
                Some(next) => {
                    output.push('\\');
                    output.push(next);
                }
                None => output.push('\\'),
            }
        } else {
            output.push(ch);
        }
    }
    output
}
/// 包括续行开头空格在内，每个物理行最多 75 octets。
pub fn fold_line(value: &str) -> String {
    let mut result = String::new();
    let mut octets = 0;
    for ch in value.chars() {
        if octets + ch.len_utf8() > 75 {
            result.push_str("\r\n ");
            octets = 1;
        }
        result.push(ch);
        octets += ch.len_utf8();
    }
    result.push_str("\r\n");
    result
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Property(ContentLine),
    Component(Component),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Component {
    name: String,
    begin: ContentLine,
    end: ContentLine,
    entries: Vec<Entry>,
}
impl Component {
    fn new(name: &str) -> Result<Self> {
        check_token(name)?;
        Ok(Self {
            name: name.into(),
            begin: ContentLine::generated("BEGIN", &[], name)?,
            end: ContentLine::generated("END", &[], name)?,
            entries: Vec::new(),
        })
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    pub fn properties<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a ContentLine> {
        self.entries.iter().filter_map(move |entry| match entry {
            Entry::Property(line) if line.name.eq_ignore_ascii_case(name) => Some(line),
            _ => None,
        })
    }
    pub fn property(&self, name: &str) -> Option<&ContentLine> {
        self.entries.iter().find_map(|entry| match entry {
            Entry::Property(line) if line.name.eq_ignore_ascii_case(name) => Some(line),
            _ => None,
        })
    }
    pub fn children(&self) -> impl Iterator<Item = &Component> {
        self.entries.iter().filter_map(|entry| match entry {
            Entry::Component(child) => Some(child),
            _ => None,
        })
    }
    fn value(&self, name: &str) -> Option<&str> {
        self.property(name).map(ContentLine::value)
    }
    fn text(&self, name: &str) -> Option<String> {
        self.property(name).map(ContentLine::text)
    }
    fn set(
        &mut self,
        name: &str,
        value: Option<&str>,
        semantic: &[&str],
        parameters: &[Parameter],
    ) -> Result<()> {
        let indexes: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| match entry {
                Entry::Property(line) if line.name.eq_ignore_ascii_case(name) => Some(index),
                _ => None,
            })
            .collect();
        if value.is_some() && indexes.len() > 1 {
            return Err(error(format!("{name} 出现多次，无法安全地局部修改")));
        }
        if let Some(value) = value {
            if let Some(&index) = indexes.first() {
                if let Entry::Property(old) = &self.entries[index] {
                    self.entries[index] =
                        Entry::Property(old.replace(value, semantic, parameters)?);
                }
            } else {
                self.entries.push(Entry::Property(ContentLine::generated(
                    name, parameters, value,
                )?));
            }
        } else {
            self.entries.retain(|entry| !matches!(entry, Entry::Property(line) if line.name.eq_ignore_ascii_case(name)));
        }
        Ok(())
    }
    fn write_to(&self, output: &mut String) {
        self.begin.write_to(output);
        for entry in &self.entries {
            match entry {
                Entry::Property(line) => line.write_to(output),
                Entry::Component(child) => child.write_to(output),
            }
        }
        self.end.write_to(output);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarDocument {
    root: Component,
    original: Option<String>,
}
impl CalendarDocument {
    /// 接受 CRLF 或 LF，拒绝孤立 CR、空内容行、孤立折行及组件不匹配。
    /// 输入本身始终通过 original_ics 保留，输出则规范化换行和超长物理行。
    pub fn parse(input: &str) -> Result<Self> {
        if input.contains('\0') {
            return Err(error("ICS 包含空字符"));
        }
        let normalized = input.replace("\r\n", "\n");
        if normalized.contains('\r') {
            return Err(error("ICS 包含孤立的 CR 换行"));
        }
        let normalized = normalized.strip_suffix('\n').unwrap_or(&normalized);
        let mut lines: Vec<(String, Vec<String>)> = Vec::new();
        for physical in normalized.split('\n') {
            check_value(physical)?;
            if physical.starts_with([' ', '\t']) {
                let (unfolded, raw) = lines
                    .last_mut()
                    .ok_or_else(|| error("ICS 折行没有前一内容行"))?;
                unfolded.push_str(&physical[1..]);
                raw.push(physical.into());
            } else {
                if physical.is_empty() {
                    return Err(error("ICS 包含空内容行"));
                }
                lines.push((physical.into(), vec![physical.into()]));
            }
        }
        let mut stack: Vec<Component> = Vec::new();
        let mut root = None;
        for (logical, physical) in lines {
            let line = ContentLine::parse(logical, physical)?;
            if line.name.eq_ignore_ascii_case("BEGIN") {
                if !line.parameters.is_empty() {
                    return Err(error("ICS 组件边界不能包含参数"));
                }
                if stack.len() >= 64 {
                    return Err(error("ICS 组件嵌套超过 64 层"));
                }
                let mut component = Component::new(&line.value)?;
                component.begin = line;
                stack.push(component);
            } else if line.name.eq_ignore_ascii_case("END") {
                let mut component = stack
                    .pop()
                    .ok_or_else(|| error("ICS 结束组件没有对应开始组件"))?;
                if !line.parameters.is_empty() || !component.name.eq_ignore_ascii_case(&line.value)
                {
                    return Err(error("ICS 开始组件与结束组件不匹配"));
                }
                component.end = line;
                if let Some(parent) = stack.last_mut() {
                    parent.entries.push(Entry::Component(component));
                } else if root.replace(component).is_some() {
                    return Err(error("ICS 只能包含一个 VCALENDAR 根组件"));
                }
            } else {
                stack
                    .last_mut()
                    .ok_or_else(|| error("ICS 属性位于组件之外"))?
                    .entries
                    .push(Entry::Property(line));
            }
        }
        if !stack.is_empty() {
            return Err(error("ICS 组件未结束"));
        }
        let root = root.ok_or_else(|| error("ICS 缺少 VCALENDAR 根组件"))?;
        if !root.name.eq_ignore_ascii_case("VCALENDAR") {
            return Err(error("ICS 根组件必须是 VCALENDAR"));
        }
        Ok(Self {
            root,
            original: Some(input.into()),
        })
    }
    pub fn root(&self) -> &Component {
        &self.root
    }
    /// 最初读取的完整输入；patch 后仍为基线输入，不代表当前内容。
    pub fn original_ics(&self) -> Option<&str> {
        self.original.as_deref()
    }
    pub fn to_ics(&self) -> String {
        let mut output = String::new();
        self.root.write_to(&mut output);
        output
    }
    /// index 是 VCALENDAR.entries 中的下标，不能持久保存或跨资源复用。
    pub fn items(&self) -> Result<Vec<CalendarItem>> {
        self.root
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| match entry {
                Entry::Component(c) if c.name.eq_ignore_ascii_case("VEVENT") => {
                    Some(project_event(index, c).map(CalendarItem::Event))
                }
                Entry::Component(c) if c.name.eq_ignore_ascii_case("VTODO") => {
                    Some(project_task(index, c).map(CalendarItem::Task))
                }
                _ => None,
            })
            .collect()
    }
    pub fn component(&self, index: usize) -> Result<&Component> {
        match self.root.entries.get(index) {
            Some(Entry::Component(c))
                if c.name.eq_ignore_ascii_case("VEVENT")
                    || c.name.eq_ignore_ascii_case("VTODO") =>
            {
                Ok(c)
            }
            _ => Err(error("指定下标不是日程或待办组件")),
        }
    }
    fn recurring(&self, component: &Component) -> bool {
        component.property("RRULE").is_some()
            || component.property("RDATE").is_some()
            || component.property("EXDATE").is_some()
            || component.property("RECURRENCE-ID").is_some()
            || self
                .root
                .children()
                .filter(|c| {
                    c.name.eq_ignore_ascii_case(&component.name)
                        && c.value("UID") == component.value("UID")
                })
                .count()
                > 1
    }
    pub fn edit_capabilities(&self, index: usize) -> Result<EditCapabilities> {
        let recurring = self.recurring(self.component(index)?);
        Ok(EditCapabilities {
            text: true,
            dates_and_alarms: !recurring,
            task_state: !recurring,
            reason: recurring
                .then(|| "重复对象暂不支持时间、提醒和任务状态修改，请在原客户端操作".into()),
        })
    }
    pub fn new_event(start: CalendarDate, now: DateTime<Utc>) -> Result<Self> {
        let mut document = Self::new_object("VEVENT", now)?;
        if let Entry::Component(c) = &mut document.root.entries[2] {
            set_date(c, "DTSTART", &Patch::Set(start))?;
        }
        document.validate_for_write()?;
        Ok(document)
    }
    pub fn new_task(now: DateTime<Utc>) -> Result<Self> {
        let mut document = Self::new_object("VTODO", now)?;
        if let Entry::Component(c) = &mut document.root.entries[2] {
            c.set("STATUS", Some("NEEDS-ACTION"), &[], &[])?;
        }
        Ok(document)
    }
    fn new_object(kind: &str, now: DateTime<Utc>) -> Result<Self> {
        static NEXT_UID: AtomicU64 = AtomicU64::new(0);
        let uid = format!(
            "{}-{}-{}-{}@daozai.local",
            now.timestamp(),
            now.timestamp_subsec_nanos(),
            std::process::id(),
            NEXT_UID.fetch_add(1, Ordering::Relaxed)
        );
        let mut root = Component::new("VCALENDAR")?;
        root.set("VERSION", Some("2.0"), &[], &[])?;
        root.set("PRODID", Some("-//Daozai//CalDAV 1.0//ZH-CN"), &[], &[])?;
        let mut component = Component::new(kind)?;
        component.set("UID", Some(&uid), &[], &[])?;
        component.set("DTSTAMP", Some(&format_utc(now)), &[], &[])?;
        root.entries.push(Entry::Component(component));
        Ok(Self {
            root,
            original: None,
        })
    }
    /// 校验可写资源的基础 RFC 约束；未知状态/属性和复杂提醒仍原样保留。
    pub fn validate_for_write(&self) -> Result<()> {
        if self.root.value("VERSION") != Some("2.0")
            || self.root.value("PRODID").is_none_or(str::is_empty)
        {
            return Err(error("ICS 写入需要 VERSION:2.0 和 PRODID"));
        }
        for item in self.items()? {
            let common = item.common();
            if common.uid.is_empty() || common.dtstamp.is_none() {
                return Err(error("日程或待办缺少 UID 或 UTC DTSTAMP"));
            }
            validate_component(self.component(common.index)?)?;
        }
        Ok(())
    }
    pub fn patch_event(
        &mut self,
        index: usize,
        patch: &EventPatch,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let original = self.component(index)?;
        if !original.name.eq_ignore_ascii_case("VEVENT") {
            return Err(error("指定组件不是 VEVENT"));
        }
        if self.recurring(original)
            && (!patch.start.is_keep()
                || !patch.end.is_keep()
                || !patch.duration.is_keep()
                || !patch.alarms.is_empty())
        {
            return Err(error("重复日程暂不支持修改时间或提醒，请在原客户端操作"));
        }
        if patch.is_empty() {
            return Ok(());
        }
        let mut candidate = self.clone();
        let c = candidate.component_mut(index)?;
        apply_common(c, &patch.common)?;
        set_date(c, "DTSTART", &patch.start)?;
        set_date(c, "DTEND", &patch.end)?;
        set_duration(c, &patch.duration)?;
        apply_alarms(c, &patch.alarms)?;
        touch(c, now)?;
        candidate.validate_for_write()?;
        *self = candidate;
        Ok(())
    }
    pub fn patch_task(
        &mut self,
        index: usize,
        patch: &TaskPatch,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let original = self.component(index)?;
        if !original.name.eq_ignore_ascii_case("VTODO") {
            return Err(error("指定组件不是 VTODO"));
        }
        if self.recurring(original)
            && (!patch.start.is_keep()
                || !patch.due.is_keep()
                || !patch.duration.is_keep()
                || !patch.state.is_keep()
                || !patch.percent_complete.is_keep()
                || !patch.alarms.is_empty())
        {
            return Err(error(
                "重复待办暂不支持完成、进度、时间或提醒修改，请在 Tasks.org 或原客户端操作",
            ));
        }
        if patch.is_empty() {
            return Ok(());
        }
        let mut candidate = self.clone();
        let c = candidate.component_mut(index)?;
        apply_common(c, &patch.common)?;
        set_date(c, "DTSTART", &patch.start)?;
        set_date(c, "DUE", &patch.due)?;
        set_duration(c, &patch.duration)?;
        set_number(c, "PRIORITY", &patch.priority, 9)?;
        validate_progress_patch(&patch.state, &patch.percent_complete)?;
        set_number(c, "PERCENT-COMPLETE", &patch.percent_complete, 100)?;
        apply_state(c, &patch.state, now)?;
        if !patch.percent_complete.is_keep()
            && c.value("STATUS") == Some("COMPLETED")
            && c.value("PERCENT-COMPLETE") != Some("100")
        {
            return Err(error("已完成任务的进度必须为 100，请先取消完成"));
        }
        apply_alarms(c, &patch.alarms)?;
        touch(c, now)?;
        candidate.validate_for_write()?;
        *self = candidate;
        Ok(())
    }
    fn component_mut(&mut self, index: usize) -> Result<&mut Component> {
        match self.root.entries.get_mut(index) {
            Some(Entry::Component(c)) => Ok(c),
            _ => Err(error("组件下标无效")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditCapabilities {
    pub text: bool,
    pub dates_and_alarms: bool,
    pub task_state: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalendarDate {
    Date(NaiveDate),
    Utc(DateTime<Utc>),
    Zoned { local: NaiveDateTime, tzid: String },
    Floating(NaiveDateTime),
}
impl CalendarDate {
    pub fn from_line(line: &ContentLine) -> Result<Self> {
        let value_type = line.single_parameter("VALUE")?;
        let tzid = line.single_parameter("TZID")?;
        if value_type.is_some_and(|value| value.eq_ignore_ascii_case("DATE")) {
            if tzid.is_some() || line.value.len() != 8 {
                return Err(error("DATE 不允许 TZID，且必须使用 YYYYMMDD"));
            }
            return parse_date(&line.value).map(Self::Date);
        }
        if value_type.is_some_and(|value| !value.eq_ignore_ascii_case("DATE-TIME")) {
            return Err(error("不支持此日期 VALUE 类型"));
        }
        if line.value.ends_with('Z') {
            if tzid.is_some() {
                return Err(error("UTC 日期时间不能同时使用 TZID"));
            }
            return parse_datetime(&line.value[..line.value.len() - 1])
                .map(|value| Self::Utc(value.and_utc()));
        }
        let local = parse_datetime(&line.value)?;
        match tzid {
            Some("") => Err(error("TZID 不能为空")),
            Some(tzid) => Ok(Self::Zoned {
                local,
                tzid: tzid.into(),
            }),
            None => Ok(Self::Floating(local)),
        }
    }
    /// DATE 没有唯一时刻；floating 必须由调用方提供时区；DST 歧义和缺失明确报错。
    /// 自定义 VTIMEZONE 保留在文档中，只有 chrono-tz 认识的 TZID 可以转换。
    pub fn to_utc(&self, floating_zone: Option<Tz>) -> Result<DateTime<Utc>> {
        let (local, zone) = match self {
            Self::Utc(value) => return Ok(*value),
            Self::Date(_) => return Err(error("全天日期不能隐式转换为 UTC 时刻")),
            Self::Floating(local) => (
                *local,
                floating_zone.ok_or_else(|| error("浮动时间需要明确的本地时区"))?,
            ),
            Self::Zoned { local, tzid } => (
                *local,
                tzid.parse::<Tz>()
                    .map_err(|_| error(format!("无法解析 TZID：{tzid}；原始 VTIMEZONE 已保留")))?,
            ),
        };
        match zone.from_local_datetime(&local) {
            LocalResult::Single(value) => Ok(value.with_timezone(&Utc)),
            LocalResult::Ambiguous(_, _) => {
                Err(error("该本地时间位于夏令时重复区间，请明确选择时刻"))
            }
            LocalResult::None => Err(error("该本地时间位于夏令时跳过区间，不存在对应时刻")),
        }
    }
    fn encoded(&self) -> Result<(String, Vec<Parameter>)> {
        Ok(match self {
            Self::Date(value) => (
                format!("{}", value.format("%Y%m%d")),
                vec![parameter("VALUE", "DATE")?],
            ),
            Self::Utc(value) => (format_utc(*value), vec![]),
            Self::Zoned { local, tzid } => {
                if tzid.is_empty() {
                    return Err(error("TZID 不能为空"));
                }
                (
                    format!("{}", local.format("%Y%m%dT%H%M%S")),
                    vec![parameter("TZID", tzid)?],
                )
            }
            Self::Floating(value) => (format!("{}", value.format("%Y%m%dT%H%M%S")), vec![]),
        })
    }
}
fn parse_date(value: &str) -> Result<NaiveDate> {
    if value.len() != 8 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(error("日期必须使用 YYYYMMDD"));
    }
    NaiveDate::parse_from_str(value, "%Y%m%d").map_err(|_| error("ICS 日期无效"))
}
fn parse_datetime(value: &str) -> Result<NaiveDateTime> {
    if value.len() != 15
        || value.as_bytes()[8] != b'T'
        || !value
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 8 || b.is_ascii_digit())
    {
        return Err(error("日期时间必须使用 YYYYMMDDTHHMMSS"));
    }
    NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").map_err(|_| error("ICS 日期时间无效"))
}
fn format_utc(value: DateTime<Utc>) -> String {
    format!("{}", value.format("%Y%m%dT%H%M%SZ"))
}
fn date_property(c: &Component, name: &str) -> Result<Option<CalendarDate>> {
    c.property(name).map(CalendarDate::from_line).transpose()
}
fn utc_property(c: &Component, name: &str) -> Result<Option<DateTime<Utc>>> {
    match date_property(c, name)? {
        Some(CalendarDate::Utc(value)) => Ok(Some(value)),
        None => Ok(None),
        _ => Err(error(format!("{name} 必须是 UTC 日期时间"))),
    }
}

/// 保存原始时长，秒数仅作名义时长投影，不能用于跨 DST 的日历日计算。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IcalDuration {
    raw: String,
    nominal_seconds: i64,
    whole_days: bool,
}
impl IcalDuration {
    pub fn parse(value: &str) -> Result<Self> {
        let (negative, body) = if let Some(v) = value.strip_prefix('-') {
            (true, v)
        } else {
            (false, value.strip_prefix('+').unwrap_or(value))
        };
        let body = body
            .strip_prefix('P')
            .ok_or_else(|| error("时长必须使用 RFC 5545 的 P 格式"))?;
        let mut seconds = 0i64;
        let mut number = String::new();
        let mut time = false;
        let mut rank = 0;
        let mut count = 0;
        let mut weeks = false;
        let mut whole_days = true;
        for ch in body.chars() {
            if ch.is_ascii_digit() {
                number.push(ch);
                continue;
            }
            if ch == 'T' && !time && number.is_empty() && !weeks {
                time = true;
                rank = 2;
                whole_days = false;
                continue;
            }
            let (next_rank, multiplier) = match (ch, time) {
                ('W', false) => {
                    weeks = true;
                    (1, 604800i64)
                }
                ('D', false) => (2, 86400),
                ('H', true) => (3, 3600),
                ('M', true) => (4, 60),
                ('S', true) => (5, 1),
                _ => return Err(error("ICS 时长包含不支持的单位或顺序")),
            };
            if number.is_empty() || next_rank <= rank || (weeks && count != 0) {
                return Err(error("ICS 时长单位重复或顺序无效"));
            }
            let n = number
                .parse::<i64>()
                .map_err(|_| error("ICS 时长数值过大"))?;
            seconds = seconds
                .checked_add(
                    n.checked_mul(multiplier)
                        .ok_or_else(|| error("ICS 时长溢出"))?,
                )
                .ok_or_else(|| error("ICS 时长溢出"))?;
            number.clear();
            rank = next_rank;
            count += 1;
        }
        if count == 0 || !number.is_empty() || (time && rank == 2) || (weeks && count > 1) {
            return Err(error("ICS 时长不完整或混用了周与其他单位"));
        }
        Ok(Self {
            raw: value.into(),
            nominal_seconds: if negative { -seconds } else { seconds },
            whole_days,
        })
    }
    pub fn from_seconds(seconds: i64) -> Self {
        Self {
            raw: format!(
                "{}PT{}S",
                if seconds < 0 { "-" } else { "" },
                seconds.unsigned_abs()
            ),
            nominal_seconds: seconds,
            whole_days: false,
        }
    }
    pub fn as_str(&self) -> &str {
        &self.raw
    }
    pub fn nominal_seconds(&self) -> i64 {
        self.nominal_seconds
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    NeedsAction,
    InProcess,
    Completed,
    Cancelled,
    Other(String),
}
impl TaskStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::NeedsAction => "NEEDS-ACTION",
            Self::InProcess => "IN-PROCESS",
            Self::Completed => "COMPLETED",
            Self::Cancelled => "CANCELLED",
            Self::Other(value) => value,
        }
    }
    fn parse(value: &str) -> Self {
        match value {
            "NEEDS-ACTION" => Self::NeedsAction,
            "IN-PROCESS" => Self::InProcess,
            "COMPLETED" => Self::Completed,
            "CANCELLED" => Self::Cancelled,
            _ => Self::Other(value.into()),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventStatus {
    Tentative,
    Confirmed,
    Cancelled,
    Other(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relation {
    pub uid: String,
    pub reltype: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommonItem {
    pub index: usize,
    pub uid: String,
    pub dtstamp: Option<DateTime<Utc>>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub categories: Vec<String>,
    pub url: Option<String>,
    pub recurrence_rules: Vec<String>,
    pub recurrence_id: Option<CalendarDate>,
    pub alarms: Vec<Alarm>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventItem {
    pub common: CommonItem,
    pub start: Option<CalendarDate>,
    /// DTEND 是不包含的结束边界。
    pub end: Option<CalendarDate>,
    pub duration: Option<IcalDuration>,
    pub status: Option<EventStatus>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskItem {
    pub common: CommonItem,
    pub start: Option<CalendarDate>,
    pub due: Option<CalendarDate>,
    pub duration: Option<IcalDuration>,
    pub status: Option<TaskStatus>,
    pub percent_complete: Option<u8>,
    pub priority: Option<u8>,
    pub completed_at: Option<DateTime<Utc>>,
    pub relations: Vec<Relation>,
    pub sort_order: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalendarItem {
    Event(EventItem),
    Task(TaskItem),
}
impl CalendarItem {
    pub fn common(&self) -> &CommonItem {
        match self {
            Self::Event(item) => &item.common,
            Self::Task(item) => &item.common,
        }
    }
}

fn project_common(index: usize, c: &Component) -> Result<CommonItem> {
    let uid = c.text("UID").ok_or_else(|| error("日程或待办缺少 UID"))?;
    let mut categories = Vec::new();
    for line in c.properties("CATEGORIES") {
        categories.extend(split_text_list(line.value()));
    }
    Ok(CommonItem {
        index,
        uid,
        dtstamp: utc_property(c, "DTSTAMP")?,
        title: c.text("SUMMARY"),
        description: c.text("DESCRIPTION"),
        location: c.text("LOCATION"),
        categories,
        url: c.value("URL").map(str::to_owned),
        recurrence_rules: c
            .properties("RRULE")
            .map(|line| line.value.clone())
            .collect(),
        recurrence_id: date_property(c, "RECURRENCE-ID")?,
        alarms: c
            .children()
            .filter(|child| child.name.eq_ignore_ascii_case("VALARM"))
            .enumerate()
            .map(|(index, alarm)| project_alarm(index, alarm))
            .collect(),
    })
}
fn project_event(index: usize, c: &Component) -> Result<EventItem> {
    Ok(EventItem {
        common: project_common(index, c)?,
        start: date_property(c, "DTSTART")?,
        end: date_property(c, "DTEND")?,
        duration: c.value("DURATION").map(IcalDuration::parse).transpose()?,
        status: c.value("STATUS").map(|value| match value {
            "TENTATIVE" => EventStatus::Tentative,
            "CONFIRMED" => EventStatus::Confirmed,
            "CANCELLED" => EventStatus::Cancelled,
            _ => EventStatus::Other(value.into()),
        }),
    })
}
fn project_task(index: usize, c: &Component) -> Result<TaskItem> {
    Ok(TaskItem {
        common: project_common(index, c)?,
        start: date_property(c, "DTSTART")?,
        due: date_property(c, "DUE")?,
        duration: c.value("DURATION").map(IcalDuration::parse).transpose()?,
        status: c.value("STATUS").map(TaskStatus::parse),
        percent_complete: number_property(c, "PERCENT-COMPLETE", 100)?,
        priority: number_property(c, "PRIORITY", 9)?,
        completed_at: utc_property(c, "COMPLETED")?,
        relations: c
            .properties("RELATED-TO")
            .map(|line| Relation {
                uid: line.text(),
                reltype: line.parameter("RELTYPE").map(str::to_owned),
            })
            .collect(),
        sort_order: c.value("X-APPLE-SORT-ORDER").map(str::to_owned),
    })
}
fn number_property(c: &Component, name: &str, maximum: u8) -> Result<Option<u8>> {
    c.value(name)
        .map(|value| {
            value
                .parse::<u8>()
                .ok()
                .filter(|value| *value <= maximum)
                .ok_or_else(|| error(format!("{name} 必须在 0 到 {maximum} 之间")))
        })
        .transpose()
}
fn split_text_list(value: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
        } else if ch == ',' {
            result.push(unescape_text(&value[start..index]));
            start = index + 1;
        }
    }
    result.push(unescape_text(&value[start..]));
    result
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlarmRelation {
    Start,
    End,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlarmTrigger {
    Relative {
        duration: IcalDuration,
        related: AlarmRelation,
    },
    Absolute(DateTime<Utc>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alarm {
    /// VALARM 中的下标，与父组件的 entries 下标不同。
    pub index: usize,
    pub action: Option<String>,
    pub description: Option<String>,
    /// 无法解释的提醒保留在文档树中，并在此返回中文错误；不影响其他投影。
    pub trigger: Result<AlarmTrigger>,
    pub editable: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayAlarm {
    pub trigger: AlarmTrigger,
    pub description: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlarmEdit {
    Add(DisplayAlarm),
    Update { index: usize, alarm: DisplayAlarm },
    Remove { index: usize },
}
fn parse_trigger(c: &Component) -> Result<AlarmTrigger> {
    let line = c
        .property("TRIGGER")
        .ok_or_else(|| error("提醒缺少 TRIGGER"))?;
    let value_type = line.single_parameter("VALUE")?;
    let relation = line.single_parameter("RELATED")?;
    if value_type.is_some_and(|value| value.eq_ignore_ascii_case("DATE-TIME")) {
        if relation.is_some() {
            return Err(error("绝对提醒不能使用 RELATED"));
        }
        return match CalendarDate::from_line(line)? {
            CalendarDate::Utc(value) => Ok(AlarmTrigger::Absolute(value)),
            _ => Err(error("绝对提醒必须使用 UTC 日期时间")),
        };
    }
    if value_type.is_some_and(|value| !value.eq_ignore_ascii_case("DURATION"))
        || line.single_parameter("TZID")?.is_some()
    {
        return Err(error("相对提醒的 VALUE 或 TZID 无效"));
    }
    let related = match relation {
        None | Some("START") => AlarmRelation::Start,
        Some("END") => AlarmRelation::End,
        _ => return Err(error("提醒 RELATED 必须是 START 或 END")),
    };
    Ok(AlarmTrigger::Relative {
        duration: IcalDuration::parse(line.value())?,
        related,
    })
}
fn project_alarm(index: usize, c: &Component) -> Alarm {
    let trigger = parse_trigger(c);
    let editable = c.value("ACTION") == Some("DISPLAY")
        && trigger.is_ok()
        && c.property("REPEAT").is_none()
        && c.property("DURATION").is_none()
        && c.properties("ACTION").count() == 1
        && c.properties("TRIGGER").count() == 1
        && c.properties("DESCRIPTION").count() <= 1;
    Alarm {
        index,
        action: c.value("ACTION").map(str::to_owned),
        description: c.text("DESCRIPTION"),
        trigger,
        editable,
    }
}

/// Keep、Set、Remove 分别表示未编辑、设置值、明确清除，避免表单覆盖未展示字段。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Patch<T> {
    #[default]
    Keep,
    Set(T),
    Remove,
}
impl<T> Patch<T> {
    pub fn is_keep(&self) -> bool {
        matches!(self, Self::Keep)
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommonPatch {
    pub title: Patch<String>,
    pub description: Patch<String>,
    pub location: Patch<String>,
    pub categories: Patch<Vec<String>>,
}
impl CommonPatch {
    fn is_empty(&self) -> bool {
        self.title.is_keep()
            && self.description.is_keep()
            && self.location.is_keep()
            && self.categories.is_keep()
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventPatch {
    pub common: CommonPatch,
    pub start: Patch<CalendarDate>,
    pub end: Patch<CalendarDate>,
    pub duration: Patch<IcalDuration>,
    /// 下标以修改前的 VALARM 列表为准，同一个下标不能在一次 patch 内修改两次。
    pub alarms: Vec<AlarmEdit>,
}
impl EventPatch {
    fn is_empty(&self) -> bool {
        self.common.is_empty()
            && self.start.is_keep()
            && self.end.is_keep()
            && self.duration.is_keep()
            && self.alarms.is_empty()
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskPatch {
    pub common: CommonPatch,
    pub start: Patch<CalendarDate>,
    pub due: Patch<CalendarDate>,
    pub duration: Patch<IcalDuration>,
    pub state: Patch<TaskStateChange>,
    pub percent_complete: Patch<u8>,
    pub priority: Patch<u8>,
    pub alarms: Vec<AlarmEdit>,
}
impl TaskPatch {
    fn is_empty(&self) -> bool {
        self.common.is_empty()
            && self.start.is_keep()
            && self.due.is_keep()
            && self.duration.is_keep()
            && self.state.is_keep()
            && self.percent_complete.is_keep()
            && self.priority.is_keep()
            && self.alarms.is_empty()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskStateChange {
    /// 同时写入 COMPLETED、UTC 完成时刻、100%。
    Complete,
    /// 恢复调用方保存的非完成状态/进度；未保存时使用 NEEDS-ACTION / 0。
    Reopen { previous: Option<(TaskStatus, u8)> },
    /// 显式改成四态中的非完成状态，保留已有进度，清除完成时刻。
    Set(TaskStatus),
}
fn apply_common(c: &mut Component, patch: &CommonPatch) -> Result<()> {
    for (name, patch) in [
        ("SUMMARY", &patch.title),
        ("DESCRIPTION", &patch.description),
        ("LOCATION", &patch.location),
    ] {
        match patch {
            Patch::Keep => {}
            Patch::Remove => c.set(name, None, &[], &[])?,
            Patch::Set(value) => c.set(name, Some(&escape_text(value)), &["VALUE"], &[])?,
        }
    }
    match &patch.categories {
        Patch::Keep => {}
        Patch::Remove => c.set("CATEGORIES", None, &[], &[])?,
        Patch::Set(values) => {
            // 多条 CATEGORIES 可能各带未知参数，禁止合并导致参数丢失。
            let value = values
                .iter()
                .map(|value| escape_text(value))
                .collect::<Vec<_>>()
                .join(",");
            if values.is_empty() {
                c.set("CATEGORIES", None, &[], &[])?;
            } else {
                c.set("CATEGORIES", Some(&value), &["VALUE"], &[])?;
            }
        }
    }
    Ok(())
}
fn set_date(c: &mut Component, name: &str, patch: &Patch<CalendarDate>) -> Result<()> {
    match patch {
        Patch::Keep => Ok(()),
        Patch::Remove => c.set(name, None, &[], &[]),
        Patch::Set(value) => {
            let (value, parameters) = value.encoded()?;
            c.set(name, Some(&value), &["VALUE", "TZID"], &parameters)
        }
    }
}
fn set_duration(c: &mut Component, patch: &Patch<IcalDuration>) -> Result<()> {
    match patch {
        Patch::Keep => Ok(()),
        Patch::Remove => c.set("DURATION", None, &[], &[]),
        Patch::Set(value) => {
            IcalDuration::parse(value.as_str())?;
            c.set("DURATION", Some(value.as_str()), &["VALUE"], &[])
        }
    }
}
fn set_number(c: &mut Component, name: &str, patch: &Patch<u8>, maximum: u8) -> Result<()> {
    match patch {
        Patch::Keep => Ok(()),
        Patch::Remove => c.set(name, None, &[], &[]),
        Patch::Set(value) => {
            if *value > maximum {
                return Err(error(format!("{name} 必须在 0 到 {maximum} 之间")));
            }
            c.set(name, Some(&format!("{value}")), &["VALUE"], &[])
        }
    }
}
fn validate_progress_patch(state: &Patch<TaskStateChange>, progress: &Patch<u8>) -> Result<()> {
    let expected = match state {
        Patch::Set(TaskStateChange::Complete | TaskStateChange::Set(TaskStatus::Completed)) => {
            Some(100)
        }
        Patch::Set(TaskStateChange::Reopen { previous }) => {
            Some(previous.as_ref().map_or(0, |(_, progress)| *progress))
        }
        _ => None,
    };
    if let Some(expected) = expected {
        if !progress.is_keep() && progress != &Patch::Set(expected) {
            return Err(error("完成或取消完成指定的进度与独立进度修改冲突"));
        }
    }
    Ok(())
}
fn apply_state(
    c: &mut Component,
    patch: &Patch<TaskStateChange>,
    now: DateTime<Utc>,
) -> Result<()> {
    let (status, percent, completed) = match patch {
        Patch::Keep => return Ok(()),
        Patch::Remove => return Err(error("任务状态不能直接清除，请选择四态之一")),
        Patch::Set(TaskStateChange::Complete) => (TaskStatus::Completed, Some(100), Some(now)),
        Patch::Set(TaskStateChange::Set(TaskStatus::Completed)) => {
            (TaskStatus::Completed, Some(100), Some(now))
        }
        Patch::Set(TaskStateChange::Set(status)) => (status.clone(), None, None),
        Patch::Set(TaskStateChange::Reopen { previous }) => {
            let (status, progress) = previous.clone().unwrap_or((TaskStatus::NeedsAction, 0));
            if status == TaskStatus::Completed || matches!(status, TaskStatus::Other(_)) {
                return Err(error("取消完成只能恢复受支持的非完成状态"));
            }
            if progress > 100 {
                return Err(error("恢复的任务进度必须在 0 到 100 之间"));
            }
            (status, Some(progress), None)
        }
    };
    if matches!(status, TaskStatus::Other(_)) {
        return Err(error("未知任务状态仅支持保留，不能主动写入"));
    }
    c.set("STATUS", Some(status.as_str()), &["VALUE"], &[])?;
    if let Some(progress) = percent {
        set_number(c, "PERCENT-COMPLETE", &Patch::Set(progress), 100)?;
    }
    c.set(
        "COMPLETED",
        completed.map(format_utc).as_deref(),
        &["VALUE", "TZID"],
        &[],
    )
}
fn touch(c: &mut Component, now: DateTime<Utc>) -> Result<()> {
    let timestamp = format_utc(now);
    c.set("DTSTAMP", Some(&timestamp), &["VALUE", "TZID"], &[])?;
    c.set("LAST-MODIFIED", Some(&timestamp), &["VALUE", "TZID"], &[])
}
fn set_alarm(c: &mut Component, alarm: &DisplayAlarm) -> Result<()> {
    c.set("ACTION", Some("DISPLAY"), &["VALUE"], &[])?;
    c.set(
        "DESCRIPTION",
        Some(&escape_text(&alarm.description)),
        &["VALUE"],
        &[],
    )?;
    let (value, parameters) = match &alarm.trigger {
        AlarmTrigger::Absolute(value) => {
            (format_utc(*value), vec![parameter("VALUE", "DATE-TIME")?])
        }
        AlarmTrigger::Relative { duration, related } => {
            IcalDuration::parse(duration.as_str())?;
            (
                duration.as_str().into(),
                vec![parameter(
                    "RELATED",
                    match related {
                        AlarmRelation::Start => "START",
                        AlarmRelation::End => "END",
                    },
                )?],
            )
        }
    };
    c.set(
        "TRIGGER",
        Some(&value),
        &["VALUE", "RELATED", "TZID"],
        &parameters,
    )
}
fn apply_alarms(c: &mut Component, edits: &[AlarmEdit]) -> Result<()> {
    let positions: Vec<_> = c
        .entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| match entry {
            Entry::Component(child) if child.name.eq_ignore_ascii_case("VALARM") => Some(index),
            _ => None,
        })
        .collect();
    let mut used = std::collections::BTreeSet::new();
    let mut removed = Vec::new();
    for edit in edits {
        let (index, replacement) = match edit {
            AlarmEdit::Add(alarm) => {
                let mut child = Component::new("VALARM")?;
                set_alarm(&mut child, alarm)?;
                c.entries.push(Entry::Component(child));
                continue;
            }
            AlarmEdit::Update { index, alarm } => (*index, Some(alarm)),
            AlarmEdit::Remove { index } => (*index, None),
        };
        if !used.insert(index) {
            return Err(error("一次修改不能重复操作同一提醒"));
        }
        let position = *positions.get(index).ok_or_else(|| error("提醒下标无效"))?;
        if let Entry::Component(child) = &mut c.entries[position] {
            if !project_alarm(index, child).editable {
                return Err(error("此提醒包含暂不支持的行为，请在原客户端编辑"));
            }
            if let Some(alarm) = replacement {
                set_alarm(child, alarm)?;
            } else {
                removed.push(position);
            }
        }
    }
    removed.sort_unstable();
    for position in removed.into_iter().rev() {
        c.entries.remove(position);
    }
    Ok(())
}

fn compatible_dates(start: &CalendarDate, end: &CalendarDate) -> Result<bool> {
    Ok(match (start, end) {
        (CalendarDate::Date(a), CalendarDate::Date(b)) => b > a,
        (CalendarDate::Utc(a), CalendarDate::Utc(b)) => b > a,
        (CalendarDate::Floating(a), CalendarDate::Floating(b)) => b > a,
        (
            CalendarDate::Zoned { local: a, tzid: za },
            CalendarDate::Zoned { local: b, tzid: zb },
        ) if za == zb => b > a,
        (
            CalendarDate::Utc(_) | CalendarDate::Zoned { .. },
            CalendarDate::Utc(_) | CalendarDate::Zoned { .. },
        ) => end.to_utc(None)? > start.to_utc(None)?,
        _ => {
            return Err(error(
                "开始和结束必须使用兼容的 DATE 或 DATE-TIME 类型及浮动时间语义",
            ))
        }
    })
}
fn validate_component(c: &Component) -> Result<()> {
    for name in [
        "UID",
        "DTSTAMP",
        "DTSTART",
        "DTEND",
        "DUE",
        "DURATION",
        "STATUS",
        "PERCENT-COMPLETE",
        "PRIORITY",
        "COMPLETED",
    ] {
        if c.properties(name).count() > 1 {
            return Err(error(format!("{name} 不能重复出现")));
        }
    }
    let event = c.name.eq_ignore_ascii_case("VEVENT");
    let start = date_property(c, "DTSTART")?;
    let end = date_property(c, if event { "DTEND" } else { "DUE" })?;
    let duration = c.value("DURATION").map(IcalDuration::parse).transpose()?;
    if event && start.is_none() {
        return Err(error("日程必须有 DTSTART"));
    }
    if duration.is_some() && end.is_some() {
        return Err(error("DURATION 不能与 DTEND 或 DUE 同时使用"));
    }
    if let Some(duration) = &duration {
        if start.is_none() || duration.nominal_seconds <= 0 {
            return Err(error("DURATION 必须为正数，且需要 DTSTART"));
        }
        if matches!(start, Some(CalendarDate::Date(_))) && !duration.whole_days {
            return Err(error("全天对象的 DURATION 只能使用日或周"));
        }
    }
    if let (Some(start), Some(end)) = (&start, &end) {
        if !compatible_dates(start, end)? {
            return Err(error(
                "结束或截止时间必须晚于开始时间；DTEND 为不包含的边界",
            ));
        }
    }
    for alarm in c
        .children()
        .filter(|child| child.name.eq_ignore_ascii_case("VALARM"))
    {
        // 只验证已支持的基础提醒，复杂提醒完整保留，不冒充支持其执行语义。
        let projected = project_alarm(0, alarm);
        if !projected.editable {
            continue;
        }
        if alarm.property("DESCRIPTION").is_none() {
            return Err(error("DISPLAY 提醒需要 DESCRIPTION"));
        }
        if let Ok(AlarmTrigger::Relative { related, .. }) = projected.trigger {
            match related {
                AlarmRelation::Start if start.is_none() => {
                    return Err(error("相对开始提醒需要 DTSTART"))
                }
                AlarmRelation::End if end.is_none() && !(start.is_some() && duration.is_some()) => {
                    return Err(error("相对结束提醒需要 DTEND、DUE，或 DTSTART 与 DURATION"))
                }
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 8, 2, 0, 0).unwrap()
    }
    fn date(value: &str) -> CalendarDate {
        CalendarDate::Date(parse_date(value).unwrap())
    }
    fn document(kind: &str, lines: &str) -> CalendarDocument {
        CalendarDocument::parse(&format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//测试//ZH\r\nBEGIN:{kind}\r\nUID:测试-001@example.net\r\nDTSTAMP:20261108T020000Z\r\n{lines}END:{kind}\r\nEND:VCALENDAR\r\n")).unwrap()
    }
    fn index(document: &CalendarDocument) -> usize {
        document.items().unwrap()[0].common().index
    }
    fn task(document: &CalendarDocument) -> TaskItem {
        match document.items().unwrap().remove(0) {
            CalendarItem::Task(task) => task,
            _ => panic!("应为待办"),
        }
    }

    #[test]
    fn lossless_tree_preserves_unknowns_timezones_and_multiple_alarms() {
        let input = concat!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//测试//ZH\r\n",
            "X-ROOT;X-PARAM=one,two:扩展\r\nBEGIN:VTIMEZONE\r\nTZID:Custom/Office\r\n",
            "BEGIN:STANDARD\r\nDTSTART:19700101T000000\r\nTZOFFSETFROM:+0800\r\nTZOFFSETTO:+0800\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n",
            "BEGIN:VTODO\r\nUID:one\r\nDTSTAMP:20261108T020000Z\r\n",
            "SUMMARY;LANGUAGE=zh-CN;X-Q=\"a:b;c,d\":原始\r\n 标题\r\n",
            "DTSTART;TZID=Custom/Office:20261110T090000\r\nRELATED-TO;RELTYPE=PARENT:parent\r\n",
            "X-APPLE-SORT-ORDER:1000\r\nX-UNKNOWN;X-CN=\"张三^'姓名\":未识别\r\n",
            "BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:提醒\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n",
            "BEGIN:VALARM\r\nACTION:AUDIO\r\nTRIGGER:-PT1H\r\nATTACH;VALUE=URI:sound.wav\r\nEND:VALARM\r\n",
            "END:VTODO\r\nEND:VCALENDAR\r\n");
        let mut doc = CalendarDocument::parse(input).unwrap();
        assert_eq!(doc.to_ics(), input);
        let item = task(&doc);
        assert_eq!(item.common.title.as_deref(), Some("原始标题"));
        assert_eq!(item.common.alarms.len(), 2);
        assert_eq!(item.relations[0].reltype.as_deref(), Some("PARENT"));
        assert_eq!(item.sort_order.as_deref(), Some("1000"));
        assert!(item.start.unwrap().to_utc(None).is_err());
        doc.patch_task(
            item.common.index,
            &TaskPatch {
                common: CommonPatch {
                    title: Patch::Set("新标题".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        let output = doc.to_ics();
        assert!(output.contains("SUMMARY;LANGUAGE=zh-CN;X-Q=\"a:b;c,d\":新标题\r\n"));
        for line in [
            "X-ROOT;X-PARAM=one,two:扩展",
            "X-UNKNOWN;X-CN=\"张三^'姓名\":未识别",
            "X-APPLE-SORT-ORDER:1000",
            "ATTACH;VALUE=URI:sound.wav",
            "TZID:Custom/Office",
        ] {
            assert!(output.contains(line), "未保留 {line}");
        }
        assert_eq!(doc.original_ics(), Some(input));
    }
    #[test]
    fn utf8_folding_counts_octets_including_continuation_space() {
        let title = format!("{}😀,;\\\n末尾", "中文标题".repeat(35));
        let mut doc = CalendarDocument::new_task(now()).unwrap();
        doc.patch_task(
            index(&doc),
            &TaskPatch {
                common: CommonPatch {
                    title: Patch::Set(title.clone()),
                    ..Default::default()
                },
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        let output = doc.to_ics();
        assert!(!output.replace("\r\n", "").contains(['\r', '\n']));
        assert!(output.split("\r\n").all(|line| line.len() <= 75));
        assert_eq!(
            task(&CalendarDocument::parse(&output).unwrap())
                .common
                .title,
            Some(title)
        );
        assert_eq!(CalendarDocument::parse(&output).unwrap().to_ics(), output);
    }
    #[test]
    fn date_kinds_and_dst_are_explicit() {
        for (line, expected) in [
            ("DTSTART;VALUE=DATE:20261110", 0),
            ("DTSTART:20261110T090000Z", 1),
            ("DTSTART;TZID=Asia/Shanghai:20261110T090000", 2),
            ("DTSTART:20261110T090000", 3),
        ] {
            let line = ContentLine::parse(line.into(), vec![]).unwrap();
            let value = CalendarDate::from_line(&line).unwrap();
            assert!(matches!(
                (value, expected),
                (CalendarDate::Date(_), 0)
                    | (CalendarDate::Utc(_), 1)
                    | (CalendarDate::Zoned { .. }, 2)
                    | (CalendarDate::Floating(_), 3)
            ));
        }
        let zoned = |s| CalendarDate::Zoned {
            local: parse_datetime(s).unwrap(),
            tzid: "America/New_York".into(),
        };
        assert!(zoned("20261101T013000")
            .to_utc(None)
            .unwrap_err()
            .0
            .contains("重复"));
        assert!(zoned("20260308T023000")
            .to_utc(None)
            .unwrap_err()
            .0
            .contains("不存在"));
        assert_eq!(
            CalendarDate::Zoned {
                local: parse_datetime("20261110T090000").unwrap(),
                tzid: "Asia/Shanghai".into()
            }
            .to_utc(None)
            .unwrap(),
            Utc.with_ymd_and_hms(2026, 11, 10, 1, 0, 0).unwrap()
        );
        assert!(date("20261110").to_utc(None).is_err());
        assert!(
            CalendarDate::Floating(parse_datetime("20261110T090000").unwrap())
                .to_utc(None)
                .is_err()
        );
    }
    #[test]
    fn completion_reopen_and_four_states_keep_authoritative_fields() {
        let mut doc = document("VTODO", "STATUS:IN-PROCESS\r\nPERCENT-COMPLETE:40\r\n");
        let i = index(&doc);
        doc.patch_task(
            i,
            &TaskPatch {
                state: Patch::Set(TaskStateChange::Complete),
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        let item = task(&doc);
        assert_eq!(item.status, Some(TaskStatus::Completed));
        assert_eq!(item.percent_complete, Some(100));
        assert_eq!(item.completed_at, Some(now()));
        doc.patch_task(
            i,
            &TaskPatch {
                common: CommonPatch {
                    title: Patch::Set("仅改标题".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        assert_eq!(task(&doc).completed_at, Some(now()));
        doc.patch_task(
            i,
            &TaskPatch {
                state: Patch::Set(TaskStateChange::Reopen {
                    previous: Some((TaskStatus::InProcess, 40)),
                }),
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        assert_eq!(task(&doc).status, Some(TaskStatus::InProcess));
        assert_eq!(task(&doc).percent_complete, Some(40));
        assert_eq!(task(&doc).completed_at, None);
        for status in [
            TaskStatus::Cancelled,
            TaskStatus::NeedsAction,
            TaskStatus::InProcess,
        ] {
            doc.patch_task(
                i,
                &TaskPatch {
                    state: Patch::Set(TaskStateChange::Set(status.clone())),
                    ..Default::default()
                },
                now(),
            )
            .unwrap();
            assert_eq!(task(&doc).status, Some(status));
            assert_eq!(task(&doc).percent_complete, Some(40));
        }
        doc.patch_task(
            i,
            &TaskPatch {
                state: Patch::Set(TaskStateChange::Reopen { previous: None }),
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        assert_eq!(task(&doc).percent_complete, Some(0));
    }
    #[test]
    fn unknown_status_is_preserved_and_not_generated() {
        let mut doc = document("VTODO", "STATUS:PENDING\r\nPERCENT-COMPLETE:17\r\n");
        let i = index(&doc);
        doc.patch_task(
            i,
            &TaskPatch {
                common: CommonPatch {
                    title: Patch::Set("等待条件".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        assert_eq!(task(&doc).status, Some(TaskStatus::Other("PENDING".into())));
        let baseline = doc.clone();
        assert!(doc
            .patch_task(
                i,
                &TaskPatch {
                    state: Patch::Set(TaskStateChange::Set(TaskStatus::Other("FAILED".into()))),
                    ..Default::default()
                },
                now()
            )
            .is_err());
        assert_eq!(doc, baseline);
    }
    #[test]
    fn recurrence_is_never_downgraded_or_completed() {
        for property in [
            "RRULE:FREQ=WEEKLY;BYDAY=MO,WE",
            "RDATE;VALUE=DATE:20261110",
            "EXDATE;VALUE=DATE:20261110",
            "RECURRENCE-ID;VALUE=DATE:20261110",
        ] {
            let mut doc = document("VTODO", &format!("{property}\r\n"));
            let i = index(&doc);
            let baseline = doc.clone();
            assert!(!doc.edit_capabilities(i).unwrap().task_state);
            assert!(doc
                .patch_task(
                    i,
                    &TaskPatch {
                        state: Patch::Set(TaskStateChange::Complete),
                        ..Default::default()
                    },
                    now()
                )
                .is_err());
            assert_eq!(doc, baseline);
            assert!(doc
                .patch_task(
                    i,
                    &TaskPatch {
                        due: Patch::Set(date("20261112")),
                        ..Default::default()
                    },
                    now()
                )
                .is_err());
            doc.patch_task(
                i,
                &TaskPatch {
                    common: CommonPatch {
                        title: Patch::Set("保留重复规则".into()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                now(),
            )
            .unwrap();
            assert!(doc.to_ics().contains(property));
        }
        let mut doc = document(
            "VEVENT",
            "DTSTART;VALUE=DATE:20261110\r\nRRULE:FREQ=DAILY\r\n",
        );
        assert!(doc
            .patch_event(
                index(&doc),
                &EventPatch {
                    start: Patch::Set(date("20261111")),
                    ..Default::default()
                },
                now()
            )
            .is_err());
    }
    #[test]
    fn alarm_edit_preserves_other_alarm_and_unknown_parameters() {
        let mut doc = document("VTODO", concat!("DTSTART:20261110T090000Z\r\nDUE:20261110T100000Z\r\n",
            "BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION;X-P=keep:原提醒\r\nTRIGGER;X-KEEP=ok:-PT10M\r\nX-ALARM:保留\r\nEND:VALARM\r\n",
            "BEGIN:VALARM\r\nACTION:AUDIO\r\nTRIGGER:-PT1H\r\nATTACH:sound.wav\r\nEND:VALARM\r\n"));
        let i = index(&doc);
        let alarm = DisplayAlarm {
            trigger: AlarmTrigger::Relative {
                duration: IcalDuration::parse("-PT15M").unwrap(),
                related: AlarmRelation::End,
            },
            description: "截止前提醒".into(),
        };
        doc.patch_task(
            i,
            &TaskPatch {
                alarms: vec![AlarmEdit::Update { index: 0, alarm }],
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        let output = doc.to_ics();
        assert!(output.contains("TRIGGER;X-KEEP=ok;RELATED=END:-PT15M"));
        assert!(output.contains("DESCRIPTION;X-P=keep:截止前提醒"));
        assert!(output.contains("X-ALARM:保留"));
        assert!(output.contains("ACTION:AUDIO\r\nTRIGGER:-PT1H\r\nATTACH:sound.wav"));
        assert_eq!(task(&doc).common.alarms.len(), 2);
        let baseline = doc.clone();
        assert!(doc
            .patch_task(
                i,
                &TaskPatch {
                    alarms: vec![AlarmEdit::Remove { index: 1 }],
                    ..Default::default()
                },
                now()
            )
            .is_err());
        assert_eq!(doc, baseline);
    }
    #[test]
    fn alarm_dependencies_and_date_duration_rules_are_atomic() {
        let mut doc = CalendarDocument::new_task(now()).unwrap();
        let i = index(&doc);
        let baseline = doc.clone();
        let relative = DisplayAlarm {
            trigger: AlarmTrigger::Relative {
                duration: IcalDuration::parse("-PT10M").unwrap(),
                related: AlarmRelation::Start,
            },
            description: "提醒".into(),
        };
        assert!(doc
            .patch_task(
                i,
                &TaskPatch {
                    alarms: vec![AlarmEdit::Add(relative.clone())],
                    ..Default::default()
                },
                now()
            )
            .is_err());
        assert_eq!(doc, baseline);
        doc.patch_task(
            i,
            &TaskPatch {
                start: Patch::Set(date("20261110")),
                alarms: vec![AlarmEdit::Add(relative)],
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        let baseline = doc.clone();
        assert!(doc
            .patch_task(
                i,
                &TaskPatch {
                    duration: Patch::Set(IcalDuration::parse("PT1H").unwrap()),
                    ..Default::default()
                },
                now()
            )
            .is_err());
        assert_eq!(doc, baseline);
        doc.patch_task(
            i,
            &TaskPatch {
                duration: Patch::Set(IcalDuration::parse("P1D").unwrap()),
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        assert!(doc
            .patch_task(
                i,
                &TaskPatch {
                    due: Patch::Set(date("20261112")),
                    ..Default::default()
                },
                now()
            )
            .is_err());
        doc.patch_task(
            i,
            &TaskPatch {
                due: Patch::Set(date("20261112")),
                duration: Patch::Remove,
                ..Default::default()
            },
            now(),
        )
        .unwrap();
    }
    #[test]
    fn all_day_end_is_exclusive_and_mixed_types_rejected() {
        let mut doc = CalendarDocument::new_event(date("20261110"), now()).unwrap();
        let i = index(&doc);
        let baseline = doc.clone();
        assert!(doc
            .patch_event(
                i,
                &EventPatch {
                    end: Patch::Set(date("20261110")),
                    ..Default::default()
                },
                now()
            )
            .is_err());
        assert_eq!(doc, baseline);
        assert!(doc
            .patch_event(
                i,
                &EventPatch {
                    end: Patch::Set(CalendarDate::Utc(now())),
                    ..Default::default()
                },
                now()
            )
            .is_err());
        doc.patch_event(
            i,
            &EventPatch {
                end: Patch::Set(date("20261111")),
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        assert!(doc.to_ics().contains("DTEND;VALUE=DATE:20261111"));
    }
    #[test]
    fn malformed_input_and_duration_grammar_are_rejected() {
        for bad in [
            " BEGIN:VCALENDAR\r\n",
            "BEGIN:VCALENDAR\r\nEND:VTODO\r\n",
            "BEGIN:VCALENDAR\r\n",
            "BEGIN:VCALENDAR\n\nEND:VCALENDAR\n",
            "BEGIN:VCALENDAR\rEND:VCALENDAR",
            "BEGIN:VCALENDAR\nX;P=\"bad:value\nEND:VCALENDAR",
            "BEGIN:VCALENDAR\nEND:VCALENDAR\nX:y\n",
        ] {
            assert!(CalendarDocument::parse(bad).is_err(), "不应接受 {bad:?}");
        }
        for bad in [
            "P",
            "PT",
            "P1Y",
            "P1M",
            "P1W1D",
            "P1DT",
            "PT1S1M",
            "PT1H1H",
            "PT99999999999999999999S",
            "P1D2W",
        ] {
            assert!(IcalDuration::parse(bad).is_err(), "不应接受 {bad}");
        }
        for good in ["P1W", "P2D", "P1DT2H30M4S", "-PT15M", "+PT0S"] {
            assert!(IcalDuration::parse(good).is_ok(), "应接受 {good}");
        }
    }
    #[test]
    fn categories_parameter_quoting_and_text_escaping() {
        let mut doc = document("VTODO", "CATEGORIES:学习,家庭\\,朋友,路径\\\\文件\r\nSUMMARY;CN=\"a:b;c,d\";X-P=a^^b^nc^'d:旧标题\r\n");
        assert_eq!(
            task(&doc).common.categories,
            vec!["学习", "家庭,朋友", "路径\\文件"]
        );
        let c = doc.component(index(&doc)).unwrap();
        let line = c.property("SUMMARY").unwrap();
        assert_eq!(line.parameter("CN"), Some("a:b;c,d"));
        assert_eq!(line.parameter("X-P"), Some("a^b\nc\"d"));
        let categories = vec!["有,逗号".into(), "有;分号".into(), "多\n行\\".into()];
        doc.patch_task(
            index(&doc),
            &TaskPatch {
                common: CommonPatch {
                    categories: Patch::Set(categories.clone()),
                    ..Default::default()
                },
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        assert_eq!(
            task(&CalendarDocument::parse(&doc.to_ics()).unwrap())
                .common
                .categories,
            categories
        );
    }
    #[test]
    fn new_objects_have_required_fields_and_distinct_uids() {
        let a = CalendarDocument::new_task(now()).unwrap();
        let b = CalendarDocument::new_task(now()).unwrap();
        assert_ne!(task(&a).common.uid, task(&b).common.uid);
        for doc in [
            a,
            b,
            CalendarDocument::new_event(date("20261110"), now()).unwrap(),
        ] {
            doc.validate_for_write().unwrap();
            assert_eq!(doc.root().value("VERSION"), Some("2.0"));
            assert!(doc.root().value("PRODID").is_some());
            assert_eq!(doc.items().unwrap()[0].common().dtstamp, Some(now()));
        }
    }
    #[test]
    fn lf_input_and_overlong_original_lines_normalize_safely() {
        let input = format!(
            "BEGIN:VCALENDAR\nVERSION:2.0\nPRODID:test\nX-LONG:{}\nEND:VCALENDAR\n",
            "中".repeat(60)
        );
        let doc = CalendarDocument::parse(&input).unwrap();
        let output = doc.to_ics();
        assert_eq!(doc.original_ics(), Some(input.as_str()));
        assert!(output.split("\r\n").all(|line| line.len() <= 75));
        assert_eq!(
            CalendarDocument::parse(&output)
                .unwrap()
                .root()
                .value("X-LONG"),
            Some("中".repeat(60).as_str())
        );
    }
    #[test]
    fn progress_changes_cannot_contradict_completion_or_reopen() {
        let mut doc = CalendarDocument::new_task(now()).unwrap();
        let i = index(&doc);
        let baseline = doc.clone();
        assert!(doc
            .patch_task(
                i,
                &TaskPatch {
                    state: Patch::Set(TaskStateChange::Complete),
                    percent_complete: Patch::Set(20),
                    ..Default::default()
                },
                now()
            )
            .is_err());
        assert_eq!(doc, baseline);
        doc.patch_task(
            i,
            &TaskPatch {
                state: Patch::Set(TaskStateChange::Complete),
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        let baseline = doc.clone();
        for progress in [Patch::Set(30), Patch::Remove] {
            assert!(doc
                .patch_task(
                    i,
                    &TaskPatch {
                        percent_complete: progress,
                        ..Default::default()
                    },
                    now()
                )
                .is_err());
            assert_eq!(doc, baseline);
        }
        assert!(doc
            .patch_task(
                i,
                &TaskPatch {
                    state: Patch::Set(TaskStateChange::Reopen {
                        previous: Some((TaskStatus::InProcess, 40))
                    }),
                    percent_complete: Patch::Set(10),
                    ..Default::default()
                },
                now()
            )
            .is_err());
        assert_eq!(doc, baseline);
        // 来自远端的状态矛盾不应因修改标题而被悄悄修复。
        let mut remote = document(
            "VTODO",
            "STATUS:COMPLETED\r\nPERCENT-COMPLETE:25\r\nCOMPLETED:20261107T010000Z\r\n",
        );
        remote
            .patch_task(
                index(&remote),
                &TaskPatch {
                    common: CommonPatch {
                        title: Patch::Set("保留远端原值".into()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                now(),
            )
            .unwrap();
        assert_eq!(task(&remote).percent_complete, Some(25));
    }
    #[test]
    fn alarm_batch_uses_original_indexes_and_supports_absolute_triggers() {
        let mut doc = CalendarDocument::new_task(now()).unwrap();
        let i = index(&doc);
        let alarm = |description: &str| DisplayAlarm {
            trigger: AlarmTrigger::Absolute(now()),
            description: description.into(),
        };
        doc.patch_task(
            i,
            &TaskPatch {
                alarms: vec![AlarmEdit::Add(alarm("第一")), AlarmEdit::Add(alarm("第二"))],
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        doc.patch_task(
            i,
            &TaskPatch {
                alarms: vec![
                    AlarmEdit::Remove { index: 0 },
                    AlarmEdit::Update {
                        index: 1,
                        alarm: alarm("更新第二"),
                    },
                    AlarmEdit::Add(alarm("第三")),
                ],
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        let projected = task(&doc).common.alarms;
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[0].description.as_deref(), Some("更新第二"));
        assert_eq!(projected[1].description.as_deref(), Some("第三"));
        assert_eq!(projected[0].trigger, Ok(AlarmTrigger::Absolute(now())));
        let baseline = doc.clone();
        assert!(doc
            .patch_task(
                i,
                &TaskPatch {
                    alarms: vec![
                        AlarmEdit::Remove { index: 0 },
                        AlarmEdit::Remove { index: 0 }
                    ],
                    ..Default::default()
                },
                now()
            )
            .is_err());
        assert_eq!(doc, baseline);
    }
    #[test]
    fn exception_sibling_protects_master_and_survives_text_patch() {
        let exception = "BEGIN:VTODO\r\nUID:测试-001@example.net\r\nDTSTAMP:20261108T020000Z\r\nRECURRENCE-ID;VALUE=DATE:20261109\r\nSUMMARY:例外实例\r\nEND:VTODO\r\n";
        let input = document("VTODO", "SUMMARY:主系列\r\n").to_ics().replace(
            "END:VCALENDAR\r\n",
            &format!("{exception}END:VCALENDAR\r\n"),
        );
        let mut doc = CalendarDocument::parse(&input).unwrap();
        let i = index(&doc);
        assert_eq!(doc.items().unwrap().len(), 2);
        assert!(!doc.edit_capabilities(i).unwrap().task_state);
        assert!(doc
            .patch_task(
                i,
                &TaskPatch {
                    state: Patch::Set(TaskStateChange::Complete),
                    ..Default::default()
                },
                now()
            )
            .is_err());
        doc.patch_task(
            i,
            &TaskPatch {
                common: CommonPatch {
                    title: Patch::Set("新主标题".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        assert!(doc.to_ics().contains(exception));
    }
    #[test]
    fn semantic_parameter_ambiguity_is_retained_but_not_projected() {
        for line in [
            "DTSTART;VALUE=DATE;VALUE=DATE:20261110",
            "DTSTART;VALUE=DATE,DATE-TIME:20261110",
            "DTSTART;TZID=Asia/Shanghai,UTC:20261110T090000",
        ] {
            let doc = document("VTODO", &format!("{line}\r\n"));
            assert!(doc.items().is_err());
            assert!(doc.to_ics().contains(line));
        }
        let doc = document("VTODO", "BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:提醒\r\nTRIGGER;RELATED=START;RELATED=END:-PT10M\r\nEND:VALARM\r\n");
        let alarm = &task(&doc).common.alarms[0];
        assert!(alarm.trigger.is_err());
        assert!(!alarm.editable);
    }
    #[test]
    fn event_text_patch_keeps_scheduling_extensions_and_opaque_components() {
        let retained =
            concat!("ORGANIZER;CN=组织者:mailto:owner@example.net\r\n",
            "ATTENDEE;PARTSTAT=ACCEPTED;CN=参会人:mailto:user@example.net\r\n",
            "SEQUENCE:7\r\nCREATED:20261101T010000Z\r\nURL:https://example.net/a:b\r\n",
            "CLASS:PRIVATE\r\nTRANSP:OPAQUE\r\nATTACH;FMTTYPE=text/plain:https://example.net/a\r\n",
            "RRULE:FREQ=MONTHLY;BYDAY=1MO\r\nEXDATE;VALUE=DATE:20261207\r\n",
            "BEGIN:X-VENDOR\r\nX-DATA;X-P=unknown:厂商数据\r\nEND:X-VENDOR\r\n");
        let mut doc = document(
            "VEVENT",
            &format!("DTSTART;VALUE=DATE:20261110\r\n{retained}"),
        );
        let i = index(&doc);
        doc.patch_event(
            i,
            &EventPatch {
                common: CommonPatch {
                    description: Patch::Set("修改说明".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        assert!(doc.to_ics().contains(retained));
    }
    #[test]
    fn invalid_ranges_duplicates_and_injection_leave_resource_unchanged() {
        let mut doc = document(
            "VTODO",
            "STATUS:NEEDS-ACTION\r\nSUMMARY;X-A=1:一\r\nSUMMARY;X-B=2:二\r\n",
        );
        let baseline = doc.clone();
        assert!(doc
            .patch_task(
                index(&doc),
                &TaskPatch {
                    common: CommonPatch {
                        title: Patch::Set("合并".into()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                now()
            )
            .is_err());
        assert_eq!(doc, baseline);
        let mut doc = CalendarDocument::new_task(now()).unwrap();
        let baseline = doc.clone();
        assert!(doc
            .patch_task(
                index(&doc),
                &TaskPatch {
                    priority: Patch::Set(10),
                    ..Default::default()
                },
                now()
            )
            .is_err());
        assert_eq!(doc, baseline);
        let bad_date = CalendarDate::Zoned {
            local: parse_datetime("20261110T090000").unwrap(),
            tzid: "UTC\r\nX-INJECT:bad".into(),
        };
        assert!(doc
            .patch_task(
                index(&doc),
                &TaskPatch {
                    start: Patch::Set(bad_date),
                    ..Default::default()
                },
                now()
            )
            .is_err());
        assert_eq!(doc, baseline);
    }
}
