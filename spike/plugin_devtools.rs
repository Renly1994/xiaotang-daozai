//! 本地插件监视、脚本日志和离线试跑。

use super::*;
use std::{cell::RefCell, collections::VecDeque, rc::Rc, sync::Mutex, time::SystemTime};

const POLL: Duration = Duration::from_millis(200);
const DEBOUNCE: Duration = Duration::from_millis(160);
const CALL_LINES: usize = 20;
const LINE_CHARS: usize = 200;
const HISTORY_LINES: usize = 100;

#[derive(Clone, Default)]
pub(super) struct ScriptLogs(Arc<Mutex<VecDeque<String>>>);

impl ScriptLogs {
    pub(super) fn append(&self, id: &str, lines: Vec<String>) {
        if lines.is_empty() {
            return;
        }
        let mut history = self.0.lock().unwrap_or_else(|error| error.into_inner());
        history.extend(lines.into_iter().map(|line| format!("{id}  {line}")));
        while history.len() > HISTORY_LINES {
            history.pop_front();
        }
    }

    #[cfg(windows)]
    pub(super) fn snapshot(&self) -> Vec<String> {
        let history = self.0.lock().unwrap_or_else(|error| error.into_inner());
        history.iter().rev().cloned().collect()
    }
}

#[derive(Default)]
pub(super) struct DevTools {
    next_poll: Option<Instant>,
    watches: BTreeMap<String, WatchedPackage>,
    pub(super) logs: ScriptLogs,
}

struct WatchedPackage {
    files: Vec<PathBuf>,
    stamps: Vec<Option<(SystemTime, u64)>>,
    changed: Option<Instant>,
}

impl WatchedPackage {
    fn new(record: &Record, package: &Package) -> Self {
        let base = record.origin.parent().unwrap_or_else(|| Path::new("."));
        let manifest = serde_json::to_value(&package.manifest).unwrap_or(Value::Null);
        let mut files = Self::files(base, &record.origin, &package.manifest.entry, &manifest);
        files.extend(package.canvas_images.keys().map(|name| base.join(name)));
        files.extend(package.font_assets.keys().map(|name| base.join(name)));
        let stamps = Self::read_stamps(&files);
        Self {
            files,
            stamps,
            changed: None,
        }
    }

    fn files(base: &Path, origin: &Path, entry: &str, manifest: &Value) -> Vec<PathBuf> {
        let mut files = vec![origin.to_owned(), base.join(entry)];
        // 只监视已校验清单声明的包内数据文件。
        if let Some(data) = manifest["data"].as_array() {
            files.extend(
                data.iter()
                    .filter_map(Value::as_str)
                    .filter(|name| valid_data_name(name))
                    .map(|name| base.join(name)),
            );
        }
        files
    }

    fn read_stamps(files: &[PathBuf]) -> Vec<Option<(SystemTime, u64)>> {
        files
            .iter()
            .map(|path| {
                let meta = fs::symlink_metadata(path).ok()?;
                if !meta.is_file() {
                    return None;
                }
                Some((meta.modified().ok()?, meta.len()))
            })
            .collect()
    }

    fn poll(&mut self, now: Instant) -> bool {
        let stamps = Self::read_stamps(&self.files);
        if self.stamps != stamps {
            self.stamps = stamps;
            self.changed = Some(now);
        }
        if self
            .changed
            .is_some_and(|changed| now.duration_since(changed) >= DEBOUNCE)
        {
            self.changed = None;
            return true;
        }
        false
    }
}

fn valid_data_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".json") else {
        return false;
    };
    let first = stem
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let device_number = first
        .strip_prefix("COM")
        .or_else(|| first.strip_prefix("LPT"));
    !stem.trim().is_empty()
        && !name.starts_with(['.', ' '])
        && !name
            .chars()
            .any(|ch| ch.is_control() || "/\\:*?\"<>|".contains(ch))
        && !matches!(first.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !device_number.is_some_and(|number| matches!(number.as_bytes(), [b'1'..=b'9']))
        && !name.eq_ignore_ascii_case("plugin.json")
}

impl LocalPlugins {
    pub(crate) fn watch_files_enabled(&self) -> bool {
        self.devtools.next_poll.is_some()
    }

    #[cfg(windows)]
    pub(super) fn set_watch_files(&mut self, enabled: bool) {
        if self.watch_files_enabled() == enabled {
            return;
        }
        self.devtools.next_poll = enabled.then(Instant::now);
        self.devtools.watches.clear();
        if enabled {
            self.poll_local_reloads(Instant::now());
        }
    }

    pub(super) fn poll_local_reloads(&mut self, now: Instant) {
        if self.devtools.next_poll.is_none_or(|next| now < next) {
            return;
        }
        self.devtools.next_poll = Some(now + POLL);
        self.devtools.watches.retain(|id, _| {
            self.records
                .get(id)
                .is_some_and(|record| !record.from_market && record.package.is_some())
        });
        let mut changed = Vec::new();
        for (id, record) in &self.records {
            let Some(package) = record.package.as_ref().filter(|_| !record.from_market) else {
                continue;
            };
            let watch = self
                .devtools
                .watches
                .entry(id.clone())
                .or_insert_with(|| WatchedPackage::new(record, package));
            if watch.poll(now) {
                changed.push(id.clone());
            }
        }
        for id in changed {
            match self.reload_local(&id) {
                Ok(()) => {
                    let record = &self.records[&id];
                    let package = record.package.as_ref().unwrap();
                    self.devtools
                        .watches
                        .insert(id, WatchedPackage::new(record, package));
                }
                Err(error) => {
                    let message = format!("自动重载失败：{error}");
                    self.errors.insert(id.clone(), message.clone());
                    self.devtools.logs.append(&id, vec![message]);
                }
            }
        }
    }

    pub(super) fn reload_local(&mut self, id: &str) -> Result<()> {
        let mut record = self.installed(id)?;
        let previous = &record.package.as_ref().unwrap().manifest;
        let package = Package::read(&record.origin)?;
        if package.manifest.id != id || previous.runtime != package.manifest.runtime {
            return Err(fail("已安装插件的 ID 和运行环境须保持一致"));
        }
        #[cfg(windows)]
        let previous_network = previous.network.clone();
        if previous.source_kind() != package.manifest.source_kind() {
            record.source_grant = None;
        }
        record.values = if record
            .package
            .as_ref()
            .is_some_and(|old_pkg| old_pkg.manifest.is_lyrics_style() && package.manifest.is_lyrics_style())
        {
            let old_pkg = record.package.as_ref().unwrap();
            old_pkg
                .migrate_lyrics_style_values(&package, &record.values)
                .ok_or_else(|| fail("新版本的歌词样式设置与已保存配置不兼容，已保留当前版本"))?
        } else {
            package.settings_values(&record.values)
        };
        record.package = Some(package);
        self.drafts.remove(id);
        #[cfg(windows)]
        let previous_secret = {
            let scope_changed = match (previous_network.as_ref(), record.network()) {
                (Some(before), Some(after)) => !before.same_authorization(after),
                (None, None) => false,
                _ => true,
            };
            let mut secret = None;
            if scope_changed || record.source_grant().is_none() {
                record.source_grant = None;
                if previous_network.is_some_and(|network| network.credential.is_some()) {
                    secret = source_https::load_secret(id)?;
                    source_https::delete_secret(id)?;
                }
            }
            secret
        };
        if let Err(error) = self.put(id, record) {
            #[cfg(windows)]
            if let Some(secret) = previous_secret {
                source_https::save_secret(id, &secret)?;
            }
            return Err(error);
        }
        self.forget_view(id);
        self.restore_pins();
        Ok(())
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MockInput {
    pub(super) settings: Option<Values>,
    #[serde(default)]
    pub(super) storage: Value,
    pub(super) now: Option<i64>,
    pub(super) sources: Option<BTreeMap<String, Value>>,
}

impl MockInput {
    pub(super) fn apply(&self, input: &mut Value) -> Option<bool> {
        if let Some(now) = self.now {
            input["now"] = json!(now);
        }
        let sources = self.sources.as_ref()?;
        input["sources"] = json!(sources);
        if input.get("source").is_some() {
            // 单来源插件仍从 source 取值。命名来源使用 sources。
            let source = sources.get("default").or_else(|| {
                if sources.len() == 1 {
                    sources.values().next()
                } else {
                    None
                }
            });
            if let Some(source) = source {
                input["source"] = source.clone();
            }
            return Some(source_ready(&input["source"]));
        }
        Some(sources.values().any(source_ready))
    }
}

fn source_ready(source: &Value) -> bool {
    source["status"].as_str() == Some("ready") && source["text"].is_string()
}

pub(super) fn run_script(
    source: &[u8],
    entry: &str,
    input: String,
    invocation: &str,
    lines: &mut Vec<String>,
) -> Result<String> {
    use rquickjs::{function::Rest, Coerced, Context, Error, FromJs, Function, Object, Runtime};

    let runtime = Runtime::new()?;
    runtime.set_memory_limit(4 * 1024 * 1024);
    runtime.set_max_stack_size(128 * 1024);
    let ctx = Context::full(&runtime)?;
    let captured = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&captured);
    let end = Instant::now() + Duration::from_millis(30);
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= end)));
    let output = ctx.with(|c| -> Result<String> {
        let console = Object::new(c.clone())?;
        console.set(
            "log",
            Function::new(
                c.clone(),
                move |Rest(arguments): Rest<rquickjs::Value<'_>>| {
                    if sink.borrow().len() == CALL_LINES {
                        return Ok(());
                    }
                    let mut line = String::new();
                    let mut remaining = LINE_CHARS;
                    for argument in arguments {
                        if !line.is_empty() {
                            line.push(' ');
                            remaining -= 1;
                        }
                        let text = Coerced::<String>::from_js(argument.ctx(), argument.clone())?.0;
                        for ch in text.chars().take(remaining) {
                            line.push(if ch.is_control() { ' ' } else { ch });
                            remaining -= 1;
                        }
                        if remaining == 0 {
                            break;
                        }
                    }
                    let mut sink = sink.borrow_mut();
                    if sink.len() < CALL_LINES {
                        sink.push(line.trim_end().to_owned());
                    }
                    Ok::<(), rquickjs::Error>(())
                },
            )?,
        )?;
        c.globals().set("console", console)?;
        let execute = || -> rquickjs::Result<String> {
            c.eval::<(), _>(source)?;
            c.globals().set("__input", c.json_parse(input)?)?;
            c.eval(invocation)
        };
        match execute() {
            Ok(raw) => Ok(raw),
            Err(Error::Exception) => {
                let thrown = c.catch();
                let detail = thrown
                    .as_exception()
                    .map(|exception| {
                        let message = exception.message().unwrap_or_default();
                        let location = exception.stack().as_deref().and_then(script_location);
                        match location {
                            Some((line, col)) => format!("{entry}:{line}:{col}：{message}"),
                            None => format!("{entry}：{message}"),
                        }
                    })
                    .unwrap_or_else(|| format!("{entry}：脚本抛出异常"));
                Err(fail(&detail))
            }
            Err(other) => Err(fail(&format!("{entry}：{other}"))),
        }
    });
    *lines = std::mem::take(&mut *captured.borrow_mut());
    output
}

fn script_location(stack: &str) -> Option<(u32, u32)> {
    stack.lines().find_map(|frame| {
        let (_, location) = frame.split_once("eval_script:")?;
        let (line, col) = location.trim_end_matches(')').split_once(':')?;
        Some((line.parse().ok()?, col.parse().ok()?))
    })
}

#[cfg(windows)]
pub(super) fn capability_report(manifest: &Value) -> String {
    let runtime = manifest["runtime"].as_str().unwrap_or("");
    let mut report = String::from("能力自检");
    for name in manifest["capabilities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let state = match capabilities::find(name) {
            Some(capability) if capability.runtime == "*" || capability.runtime == runtime => {
                match name {
                    "appearance.material" if !cfg!(feature = "sdk-material") => "未支持",
                    "settings"
                    | "card"
                    | "calendar.marker"
                    | "storage"
                    | "timer"
                    | "appearance.material"
                    | "mail.view"
                    | "lyrics.style"
                    | "navigation.shortcuts"
                    | "wireless.receivers" => "可用",
                    _ => "实验",
                }
            }
            _ => "未支持",
        };
        report.push_str(&format!("\n  {name} · {state}"));
    }
    report
}

#[cfg(all(test, windows))]
#[path = "plugin_devtools_tests.rs"]
mod tests;
