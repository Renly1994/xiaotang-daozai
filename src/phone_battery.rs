//! 手机连接（Phone Link）电量：读取 Windows 开始菜单小组件的卡片数据。
//!
//! 手机连接会把已配对手机的电量写进 StartMenuCompanion.json，
//! 本模块只读本地文件，不联网、不依赖手机连接的私有接口。
//! 卡片里只有电量百分比和连接状态，没有充电标志：默认按电量涨跌推测，
//! 也可由外部自动化写 phone_charging.txt 精确覆盖。
//! 卡片里的中文是 JSON 的 \uXXXX 转义，判断文案前统一解码。

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::{path::{Path, PathBuf}, time::{Duration, Instant, SystemTime}};

const RELATIVE: &str = r"Packages\Microsoft.YourPhone_8wekyb3d8bbwe\LocalState\StartMenu\StartMenuCompanion.json";
/// 卡片刷新很频繁，10 秒读一次足够；文件一有变化则最快 1 秒内重读，
/// 这样手机断开后电量环能及时收起，而不是等满一个轮询周期。
const INTERVAL: Duration = Duration::from_secs(10);
const MIN_RECHECK: Duration = Duration::from_secs(1);
/// 卡片没有明确连接状态时，超过 2 小时没更新才视为手机已断开（兜底）。
const STALE: Duration = Duration::from_secs(2 * 60 * 60);
/// 明确仍连接、但卡片这一帧没写电量数值时的宽限，避免刷新瞬间闪烁。
const MISSING_GRACE: Duration = Duration::from_secs(3 * 60);
/// 充电中若电量长时间不再上涨，退出充电态，避免拔电后一直显示绿色。
const CHARGE_IDLE: Duration = Duration::from_secs(20 * 60);
/// 低于这个电量就停止充电，多半是随手拔线，不作为“充电上限”样本。
const MIN_SAMPLE: u8 = 60;
/// 每台设备保存最近几次充电会话的峰值，用来推断它的充电上限。
const SAMPLES_KEEP: usize = 5;

#[derive(Default)]
pub struct Reader {
    checked: Option<Instant>,
    last_modified: Option<SystemTime>,
    /// 卡片读不到电量数值的开始时刻（用于宽限）。
    missing_since: Option<Instant>,
    level: Option<u8>,
    charging: bool,
    charged_at: Option<Instant>,
    device: String,
    /// 本次充电会话到达过的最高电量；判定一次充电结束时记为样本。
    peak: u8,
    /// 本次充电会话是否已经涨过学到的上限：手机临时设成“本次充满”“充到明天”时，
    /// 电量会继续往上走，这时不能再用“停在上限”的短确认，回到默认的观望时间。
    exceeded: bool,
    /// 每台手机的充电上限记录，首次用到时从磁盘读。
    book: Option<Book>,
}

impl Reader {
    /// 返回 (电量百分比, 是否充电)；手机断开或数据不可用时返回 (None, false)。
    /// 卡片没有充电标志时按电量变化推测，或由外部文件精确覆盖。
    pub fn poll(&mut self) -> (Option<u8>, bool) {
        let path = card_path();
        let modified = path.as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|meta| meta.modified().ok());
        let changed = modified != self.last_modified;
        // 先限制最高重读频率，再看是否需要提前重读：
        // 文件改动（连接/断开、卡片刷新）后 1 秒内就能反映，稳定时每 10 秒一次。
        if self.checked.is_some_and(|at| at.elapsed() < MIN_RECHECK) {
            return (self.level, self.charging);
        }
        if !changed && self.checked.is_some_and(|at| at.elapsed() < INTERVAL) {
            return (self.level, self.charging);
        }
        self.checked = Some(Instant::now());
        self.last_modified = modified;

        let Some(path) = path else { self.clear(); return (None, false); };
        let Ok(text) = std::fs::read_to_string(&path) else { self.clear(); return (None, false); };
        // 中文文案在 JSON 里是 \uXXXX，解码后再判连接状态与电量。
        let plain = decode_escapes(&text);
        self.evaluate(&plain, fresh(&path, STALE))
    }

    /// 按卡片文本更新状态，与文件读取 / 节流解耦，
    /// 这样「已连接 → 断开」的状态切换能用单测直接覆盖。
    fn evaluate(&mut self, plain: &str, is_fresh: bool) -> (Option<u8>, bool) {
        let level = parse_level(plain);
        // 连接态必须先于「沿用缓存」判断：断开时移除电量数值，避免断开后仍显示旧电量。
        if !card_usable(plain, level, is_fresh) {
            self.clear();
            return (None, false);
        }
        let Some(level) = level else {
            // 明确仍连接但这一帧没写电量：给 3 分钟宽限，之后收起。
            let since = *self.missing_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= MISSING_GRACE {
                self.clear();
                return (None, false);
            }
            return (self.level, self.charging);
        };
        self.missing_since = None;
        let was_charging = self.charging;
        self.device = parse_device(plain).unwrap_or_default();
        let device = self.device.clone();
        let profile = self.book().profile(&device);
        let mut inferred = false;
        // 卡片没有充电字段：优先用外部精确状态，否则按电量涨跌推测；
        // 电量长期不涨时自动退出充电态，避免拔电后一直保持绿色。
        self.charging = match override_charging() {
            // 精确覆盖由外部负责，不做超时回落；清掉计时器，删文件后立刻回到推测。
            Some(state) => { self.charged_at = None; state },
            None => {
                inferred = true;
                let rising = self.level.is_some_and(|old| level > old);
                if rising { self.charged_at = Some(Instant::now()); }
                let next = next_charging(self.level, level, self.charging);
                if next && !was_charging { self.peak = level; self.exceeded = false; }
                if next && level > self.peak { self.peak = level; }
                // 涨过学到的上限：这台机器这次是要充满（临时设成“本次充满”“充到明天”），
                // 本会话不再用短确认；真正的上限对不对，等会话结束按峰值判断。
                if next && profile.limit().is_some_and(|limit| level > limit) { self.exceeded = true; }
                // 没有上涨记录也视为超时，避免覆盖文件撤销后一直保持绿色。
                let wait = idle_timeout(level, profile.limit(), profile.count(), profile.is_manual(), self.exceeded);
                let idle = self.charged_at.map_or(true, |at| at.elapsed() >= wait);
                if next && idle { false } else { next }
            },
        };
        // 一次充电会话结束（拔线、掉电，或判定停在该机的上限）时按本次峰值结账：
        // 停在上限附近就记一票，下次这个电量只需很短的确认；停得更高说明上次学错了，
        // 样本作废重学；充到 100% 的会话不记（满电本身有单独的 60 秒判定）。
        if inferred && !self.charging && was_charging && self.peak >= MIN_SAMPLE && self.peak < 100 {
            let peak = self.peak;
            match profile.limit() {
                Some(limit) if peak > limit + 1 => self.book().forget(&device),
                _ => self.book().remember(&device, peak),
            }
        }
        self.level = Some(level);
        (self.level, self.charging)
    }

    /// 每台手机的充电记录，首次用到时才读盘（文件不存在就是空表）。
    fn book(&mut self) -> &mut Book {
        self.book.get_or_insert_with(Book::load)
    }

    /// 断开 / 数据失效时清空所有缓存，避免 UI 继续显示上一次的电量。
    fn clear(&mut self) {
        self.level = None;
        self.charging = false;
        self.charged_at = None;
        self.missing_since = None;
        self.device.clear();
        self.peak = 0;
        self.exceeded = false;
    }

    pub fn device(&self) -> &str { &self.device }
}

/// 可选精确充电状态：外部自动化把 `1` / `0` 写进
/// `%APPDATA%\Daozai\phone_charging.txt` 即可覆盖推测值
/// （例如手机端 Tasker 在插拔充电器时同步该文件）。
/// 文件不存在、写着 auto 或其它内容时返回 None，沿用推测逻辑。
fn override_charging() -> Option<bool> {
    let base = std::env::var_os("APPDATA")?;
    let path = PathBuf::from(base).join("Daozai").join("phone_charging.txt");
    parse_override(&std::fs::read_to_string(path).ok()?)
}

fn parse_override(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "charging" | "充电" => Some(true),
        "0" | "false" | "discharging" | "未充电" => Some(false),
        _ => None,
    }
}

/// 「手机连接」是否装在本机：只看包目录在不在，不依赖卡片是否已生成。
/// 没装的话设置页不允许开启手机电量。
pub fn installed() -> bool {
    #[cfg(test)]
    return false;
    #[cfg(not(test))]
    {
        let Some(base) = std::env::var_os("LOCALAPPDATA") else { return false; };
        std::path::Path::new(&base).join(r"Packages\Microsoft.YourPhone_8wekyb3d8bbwe").is_dir()
    }
}

/// 设置页引导用的即时快照：直接读卡片文件，返回 (是否已连接, 手机型号, 电量百分比)。
/// 不经 Reader 节流，每次打开设置页都能拿到最新状态。
pub fn snapshot() -> (bool, String, Option<u8>) {
    let Some(path) = card_path() else { return (false, String::new(), None); };
    let Ok(text) = std::fs::read_to_string(&path) else { return (false, String::new(), None); };
    let plain = decode_escapes(&text);
    let linked = parse_linked(&plain).unwrap_or(false);
    let device = parse_device(&plain).unwrap_or_default();
    let level = parse_level(&plain);
    (linked, device, level)
}

fn card_path() -> Option<PathBuf> {
    // 官方 UI 测试依赖「无电池设备」的确定性布局；测试构建不读真实手机连接数据，
    // 否则本机配对手机的电量会污染 resident_power，让布局/吉祥物断言失效。
    #[cfg(test)]
    return None;
    #[cfg(not(test))]
    {
        let base = std::env::var_os("LOCALAPPDATA")?;
        Some(PathBuf::from(base).join(RELATIVE))
    }
}

/// 文件在 max_age 内更新过才算新鲜。
fn fresh(path: &Path, max_age: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified()).ok()
        .and_then(|time| time.elapsed().ok())
        .is_some_and(|age| age <= max_age)
}

/// 把 JSON 里的 \uXXXX 还原成真实字符（中文文案全靠这个才能匹配）。
/// 非法转义、代理项等原样保留，不影响后续判断。
fn decode_escapes(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && bytes.get(i + 1) == Some(&b'u') && i + 6 <= bytes.len() {
            // get() 而不是直接切片：碰到非 UTF-8 边界的坏数据时安全跳过。
            if let Some(hex) = text.get(i + 2..i + 6) {
                if let Ok(code) = u32::from_str_radix(hex, 16) {
                    if let Some(ch) = char::from_u32(code) {
                        out.push(ch);
                        i += 6;
                        continue;
                    }
                }
            }
        }
        let ch = text[i..].chars().next().unwrap_or('\u{FFFD}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// 卡片是否可用：明确的连接文案优先；没有连接文案时，
/// 必须还能读到电量数值、且文件没过期，才允许兜底显示。
fn card_usable(plain: &str, level: Option<u8>, fresh: bool) -> bool {
    match parse_linked(plain) {
        Some(linked) => linked,
        None => level.is_some() && fresh,
    }
}

/// 卡片 speak 形如 `Xiaomi 17 Pro Max - 已连接到你的电脑`（原文是 \u 转义），
/// 断开后变成 `Xiaomi 17 Pro Max - 已从电脑断开连接`，且电量数值一并消失。
fn parse_linked(text: &str) -> Option<bool> {
    let lower = text.to_ascii_lowercase();
    let disconnected = [
        "未连接", "断开连接", "已断开", "连接已断开", "已从电脑断开",
        "disconnected", "not connected", "no phone connected", "offline",
    ].iter().any(|key| lower.contains(key));
    // "disconnected" / "not connected" 已被上面捕获；这里再排除一次更稳妥。
    let connected = lower.contains("已连接")
        || (lower.contains("connected") && !lower.contains("not connected") && !lower.contains("disconnected"));
    if disconnected { Some(false) }
    else if connected { Some(true) }
    else { None }
}

fn next_charging(previous: Option<u8>, level: u8, current: bool) -> bool {
    match previous {
        Some(old) if level > old => true,
        Some(old) if level < old => false,
        _ => current,
    }
}

// ---------------------------------------------------------------
// 充电上限：有些手机把充电目标设在 80%，到 80% 就不再涨；卡片里没有充电标志，
// 只能靠“电量长时间不涨”来判断。默认的 20 分钟等待对这类机器太长，所以按设备
// 记住它上次停在哪一档，下次到这个电量只要很短的确认时间。
// ---------------------------------------------------------------

/// `%APPDATA%\Daozai\phone-charge.json`，每台手机一条记录。
#[derive(Default, Clone, Serialize, Deserialize)]
struct Book {
    #[serde(default)]
    devices: BTreeMap<String, Profile>,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct Profile {
    /// 手动指定的充电上限（0 表示没指定）。写了就以它为准，不再靠样本推断。
    #[serde(default)]
    limit: u8,
    /// 最近几次充电会话的峰值电量，最多 [`SAMPLES_KEEP`] 个。
    #[serde(default)]
    samples: Vec<u8>,
    /// 当前上限被确认过几次（派生值，写进文件方便查看，加载后重算）。
    #[serde(default)]
    count: u32,
}

impl Profile {
    /// 手动值优先，否则用样本里反复出现的峰值。
    fn limit(&self) -> Option<u8> {
        if self.limit > 0 { Some(self.limit.min(100)) } else { infer(&self.samples).map(|(limit, _)| limit) }
    }
    fn is_manual(&self) -> bool { self.limit > 0 }
    fn count(&self) -> u32 {
        match self.limit() {
            Some(limit) => self.samples.iter().filter(|value| value.abs_diff(limit) <= 1).count() as u32,
            None => 0,
        }
    }
}

/// 从样本推断充电上限：相邻相差 1 的电量并成一档（手机上报会跳数），
/// 一档取簇内最小值（宁可比实际低一点），再挑出现次数最多的那档；并列时取更高的那档。
fn infer(samples: &[u8]) -> Option<(u8, u32)> {
    let mut values = samples.to_vec();
    values.sort_unstable();
    let mut best: Option<(u8, u32)> = None;
    let mut index = 0;
    while index < values.len() {
        let low = values[index];
        let mut end = index + 1;
        while end < values.len() && values[end].abs_diff(values[end - 1]) <= 1 { end += 1; }
        let count = (end - index) as u32;
        let better = best.map_or(true, |(current, times)| count > times || (count == times && low > current));
        if better { best = Some((low, count)); }
        index = end;
    }
    best
}

/// 到这个电量后还要盯多久才认定“停住了”。次数越多越短，手写上限时最短。
fn confirm_seconds(manual: bool, count: u32) -> u64 {
    if manual { return 60; }
    match count {
        0 | 1 => 600, // 第一次学到：比默认的 20 分钟短，但仍留足余量
        2 => 240,
        3 => 120,
        _ => 60,
    }
}

/// 本次采样允许“电量不涨”的时间：离学到的上限还远或没学过时，仍用默认的 20 分钟。
fn idle_timeout(level: u8, limit: Option<u8>, count: u32, manual: bool, exceeded: bool) -> Duration {
    if level >= 100 { return Duration::from_secs(60); } // 满了不用等
    // 这次要充满（电量已经越过常规上限）：退回默认的观望时间，不要提前判定停住。
    if exceeded { return CHARGE_IDLE; }
    let Some(limit) = limit else { return CHARGE_IDLE };
    if level + 1 < limit { return CHARGE_IDLE; }
    Duration::from_secs(confirm_seconds(manual, count))
}

fn book_path() -> Option<PathBuf> {
    Some(crate::model::config_dir().join("phone-charge.json"))
}

impl Book {
    fn load() -> Book {
        // 测试不读真实用户文件：既避免断言受本机记录影响，也不会因坏文件触发隔离改名。
        if cfg!(test) { return Book::default(); }
        let Some(path) = book_path() else { return Book::default(); };
        let Some(value) = crate::model::read_json_renaming_bad(&path, 1024 * 1024) else { return Book::default(); };
        let mut book: Book = serde_json::from_value(value).unwrap_or_default();
        for profile in book.devices.values_mut() {
            profile.samples.retain(|value| *value <= 100);
            if profile.samples.len() > SAMPLES_KEEP {
                let extra = profile.samples.len() - SAMPLES_KEEP;
                profile.samples.drain(0..extra);
            }
            profile.count = profile.count();
        }
        book
    }

    fn save(&self) {
        if cfg!(test) { return; }
        let Some(path) = book_path() else { return; };
        let mut copy = self.clone();
        for profile in copy.devices.values_mut() { profile.count = profile.count(); }
        let Ok(body) = serde_json::to_vec_pretty(&copy) else { return; };
        if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
        let _ = crate::model::atomic_write(&path, &body);
    }

    fn profile(&self, device: &str) -> Profile {
        self.devices.get(&device_key(device)).cloned().unwrap_or_default()
    }

    /// 记一次充电会话的峰值；同一档只会被多次累加，用来缩短确认时间。
    fn remember(&mut self, device: &str, peak: u8) {
        let profile = self.devices.entry(device_key(device)).or_default();
        profile.samples.push(peak);
        if profile.samples.len() > SAMPLES_KEEP { profile.samples.remove(0); }
        self.save();
    }

    /// 学错了（电量涨过了上限）：清掉样本重新学，保留手动上限。
    fn forget(&mut self, device: &str) {
        if let Some(profile) = self.devices.get_mut(&device_key(device)) {
            profile.samples.clear();
            self.save();
        }
    }
}

/// 设备名可能是空串（卡片解析不到），归到同一个桶里。
fn device_key(device: &str) -> String {
    let name = device.trim();
    if name.is_empty() { "未命名设备".to_string() } else { name.chars().take(64).collect() }
}

/// 卡片里电量写成 `"电池电量剩余 45%"` 或 JSON 转义的
/// `"\u7535\u6C60\u7535\u91CF\u5269\u4F59 45%"`，两种都要认。
/// 只有紧跟在电量关键字后面的百分比才算数，避免把布局里的百分数误当电量。
fn parse_level(text: &str) -> Option<u8> {
    let re = Regex::new(r"(\d{1,3})%").ok()?;
    for caps in re.captures_iter(text) {
        let whole = caps.get(0)?;
        let before = &text[whole.start().saturating_sub(80)..whole.start()];
        let battery = before.contains(r"\u7535\u6C60") || before.contains("电池")
            || before.contains(r"\u5269\u4F59") || before.contains("剩余")
            || before.contains(r"\u4E3A") || before.contains("电量");
        if !battery { continue; }
        if let Ok(value) = caps[1].parse::<u8>() {
            if value <= 100 { return Some(value); }
        }
    }
    None
}

fn parse_device(text: &str) -> Option<String> {
    let re = Regex::new(r#""text"\s*:\s*"([^"\\]{1,64})""#).ok()?;
    for caps in re.captures_iter(text) {
        let value = caps[1].to_owned();
        if !value.contains('%') && !value.trim().is_empty() { return Some(value); }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析转义与明文卡片() {
        let escaped = r#"{"tooltip":"\u7535\u6C60\u7535\u91CF\u5269\u4F59 45%","altText":"\u5F53\u524D\u7535\u6C60\u7535\u91CF\u4E3A 45%"}"#;
        assert_eq!(parse_level(escaped), Some(45));
        assert_eq!(parse_level(r#"{"text":"49%","tooltip":"电池电量剩余 49%"}"#), Some(49));
    }

    #[test]
    fn 无关百分比不被误判() {
        assert_eq!(parse_level(r#"{"width":"100%","clipMargin":"2px"}"#), None);
        assert_eq!(parse_level(r#"{"tooltip":"电池电量剩余 101%"}"#), None);
    }

    #[test]
    fn 电量上升视为充电() {
        assert!(next_charging(Some(45), 49, false));
        assert!(!next_charging(Some(80), 79, true));
        assert!(next_charging(Some(80), 80, true));
    }

    #[test]
    fn 充电上限按样本推断() {
        assert_eq!(infer(&[]), None);
        assert_eq!(infer(&[80]), Some((80, 1)));
        // ±1 视为同一档（手机上报会跳数）
        assert_eq!(infer(&[80, 80, 81]), Some((80, 3)));
        assert_eq!(infer(&[80, 80, 50]), Some((80, 2)));
        // 并列时取更高的那个
        assert_eq!(infer(&[65, 90]), Some((90, 1)));
    }

    #[test]
    fn 确认时间随次数递减且不越过总时长() {
        assert_eq!(confirm_seconds(false, 1), 600);
        assert_eq!(confirm_seconds(false, 2), 240);
        assert_eq!(confirm_seconds(false, 3), 120);
        assert_eq!(confirm_seconds(false, 9), 60);
        assert_eq!(confirm_seconds(true, 0), 60);
        // 还没学到上限：沿用默认的 20 分钟
        assert_eq!(idle_timeout(50, None, 0, false, false), CHARGE_IDLE);
        // 离上限还远：也是 20 分钟，避免边用边充被误判成停住
        assert_eq!(idle_timeout(60, Some(80), 4, false, false), CHARGE_IDLE);
        // 到了上限才缩短
        assert_eq!(idle_timeout(79, Some(80), 3, false, false), Duration::from_secs(120));
        // 这次要充满（已越过常规上限）：退回 20 分钟，不提前判停
        assert_eq!(idle_timeout(82, Some(80), 4, false, true), CHARGE_IDLE);
        // 满电不用等
        assert_eq!(idle_timeout(100, None, 0, false, false), Duration::from_secs(60));
    }

    #[test]
    fn 手动上限优先于样本() {
        let manual = Profile { limit: 85, samples: vec![80, 80], ..Default::default() };
        assert!(manual.is_manual());
        assert_eq!(manual.limit(), Some(85));
        assert_eq!(manual.count(), 0);
        let learned = Profile { samples: vec![80, 80, 80], ..Default::default() };
        assert!(!learned.is_manual());
        assert_eq!(learned.limit(), Some(80));
        assert_eq!(learned.count(), 3);
        // 空设备名归到同一个桶，不会每台都新建一条
        assert_eq!(device_key("  "), device_key(""));
    }

    #[test]
    fn 能取出设备名() {
        let text = r#"{"speak":"Xiaomi 17 Pro Max - connected","body":[{"text":"Xiaomi 17 Pro Max"},{"text":"49%"}]}"#;
        assert_eq!(parse_device(text).as_deref(), Some("Xiaomi 17 Pro Max"));
    }

    #[test]
    fn 精确充电状态覆盖值解析() {
        assert_eq!(parse_override("1"), Some(true));
        assert_eq!(parse_override(" charging\n"), Some(true));
        assert_eq!(parse_override("充电"), Some(true));
        assert_eq!(parse_override("0"), Some(false));
        assert_eq!(parse_override("未充电"), Some(false));
        assert_eq!(parse_override("auto"), None);
        assert_eq!(parse_override(""), None);
    }

    #[test]
    fn 连接状态优先判断() {
        // 转义文案先解码，再交给 parse_linked（poll 里就是这么做的）。
        let linked = decode_escapes(r#"{"speak":"Xiaomi - \u5DF2\u8FDE\u63A5\u5230\u4F60\u7684\u7535\u8111"}"#);
        let unlinked = decode_escapes(r#"{"speak":"Xiaomi - \u672A\u8FDE\u63A5"}"#);
        assert_eq!(parse_linked(&linked), Some(true));
        assert_eq!(parse_linked(&unlinked), Some(false));
        assert_eq!(parse_linked(r#"{"speak":"Phone - connected"}"#), Some(true));
        assert_eq!(parse_linked(r#"{"speak":"Phone - disconnected"}"#), Some(false));
        assert_eq!(parse_linked(r#"{"speak":"Phone - not connected"}"#), Some(false));
        assert_eq!(parse_linked(r#"{"text":"49%"}"#), None);
    }

    #[test]
    fn 快照在无卡片时返回未连接() {
        // 测试构建下 card_path 返回 None，快照应安全地给出「未连接」而非 panic。
        let (linked, device, level) = snapshot();
        assert!(!linked);
        assert!(device.is_empty());
        assert_eq!(level, None);
    }

    #[test]
    fn 解码中文转义文案() {
        // 真实断开卡片的 speak：\u5DF2\u4ECE\u7535\u8111\u65AD\u5F00\u8FDE\u63A5
        let raw = r#"{"speak":"Xiaomi 17 Pro Max - \u5DF2\u4ECE\u7535\u8111\u65AD\u5F00\u8FDE\u63A5"}"#;
        let plain = decode_escapes(raw);
        assert!(plain.contains("已从电脑断开连接"));
        assert_eq!(parse_linked(&plain), Some(false));
        // 断开卡片里没有电量数值
        assert_eq!(parse_level(&plain), None);
        assert!(!card_usable(&plain, None, true));
        // 即使残留旧百分比，明确断开也要收起
        assert!(!card_usable(&plain, Some(80), true));
    }

    #[test]
    fn 已连接卡片仍显示电量() {
        let plain = decode_escapes(r#"{"speak":"Xiaomi - \u5DF2\u8FDE\u63A5\u5230\u4F60\u7684\u7535\u8111","tooltip":"\u7535\u6C60\u7535\u91CF\u5269\u4F59 80%"}"#);
        assert_eq!(parse_linked(&plain), Some(true));
        assert_eq!(parse_level(&plain), Some(80));
        assert!(card_usable(&plain, Some(80), false));
    }

    #[test]
    fn 无连接文案时的新鲜度兜底() {
        // 手机息屏等场景：卡片没写连接状态但仍有电量，靠文件新鲜度兜底。
        assert!(card_usable(r#"{"tooltip":"电池电量剩余 49%"}"#, Some(49), true));
        assert!(!card_usable(r#"{"tooltip":"电池电量剩余 49%"}"#, Some(49), false));
        // 断开后的典型形态：既无连接文案也无电量数值，即便文件刚更新也不能沿用旧值。
        assert!(!card_usable(r#"{"text":"电池处于未知状态"}"#, None, true));
    }

    #[test]
    fn 运行中手机断开会清空已缓存电量() {
        // 用户遇到的场景：先连着手机（缓存 80%），再断开，卡片改为断开文案且不再写电量数值。
        let mut reader = Reader::default();
        let linked = decode_escapes(r#"{"speak":"Xiaomi - \u5DF2\u8FDE\u63A5\u5230\u4F60\u7684\u7535\u8111","text":"Xiaomi 17 Pro Max","tooltip":"\u7535\u6C60\u7535\u91CF\u5269\u4F59 80%"}"#);
        assert_eq!(reader.evaluate(&linked, true).0, Some(80));
        assert_eq!(reader.level, Some(80));

        let gone = decode_escapes(r#"{"speak":"Xiaomi 17 Pro Max - \u5DF2\u4ECE\u7535\u8111\u65AD\u5F00\u8FDE\u63A5","tooltip":"\u7535\u6C60\u5904\u4E8E\u672A\u77E5\u72B6\u6001"}"#);
        assert_eq!(reader.evaluate(&gone, true), (None, false));
        assert_eq!(reader.level, None);
        assert!(reader.device().is_empty());

        // 重新连上要能立刻恢复，不需要重启岛仔。
        assert_eq!(reader.evaluate(&linked, true).0, Some(80));
    }

    #[test]
    fn 连接正常但卡片暂无电量数值时不会误收起() {
        let mut reader = Reader::default();
        let linked = decode_escapes(r#"{"speak":"Xiaomi - \u5DF2\u8FDE\u63A5\u5230\u4F60\u7684\u7535\u8111","tooltip":"\u7535\u6C60\u7535\u91CF\u5269\u4F59 80%"}"#);
        assert_eq!(reader.evaluate(&linked, true).0, Some(80));
        let no_level = decode_escapes(r#"{"speak":"Xiaomi - \u5DF2\u8FDE\u63A5\u5230\u4F60\u7684\u7535\u8111"}"#);
        // 明确仍连接、只是这一帧没写电量：宽限期内沿用旧值，避免刷新闪烁。
        assert_eq!(reader.evaluate(&no_level, true).0, Some(80));
    }
}
