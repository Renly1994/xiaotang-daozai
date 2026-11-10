//! 天气：按城市（留空时按网络出口定位）读取 Open-Meteo 的实况与四日预报。
//! 只保留最近一次结果用于离线展示，不读取、也不上传任何账号信息。
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

/// 自动刷新的间隔（秒）：Open-Meteo 的实况本身每 15 分钟更新一次。
pub const INTERVAL: i64 = 900;

/// 天气现象；决定画太阳、云、雨还是雪。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Sun, Moon, Partly, Cloud, Fog, Drizzle, Rain, Shower, Snow, Thunder }

/// WMO 天气代码 → 图标类型（`day` 决定太阳还是月亮）。
pub fn kind(code: u8, day: bool) -> Kind {
    match code {
        0 => if day { Kind::Sun } else { Kind::Moon },
        1 | 2 => Kind::Partly,
        3 => Kind::Cloud,
        45 | 48 => Kind::Fog,
        51 | 53 | 55 | 56 | 57 => Kind::Drizzle,
        61 | 63 | 65 | 66 | 67 => Kind::Rain,
        71 | 73 | 75 | 77 => Kind::Snow,
        80 | 81 | 82 => Kind::Shower,
        85 | 86 => Kind::Snow,
        95 | 96 | 99 => Kind::Thunder,
        _ => Kind::Cloud,
    }
}

/// WMO 天气代码 → 中文描述。
pub fn describe(code: u8) -> &'static str {
    match code {
        0 => "晴",
        1 => "晴间多云",
        2 => "多云",
        3 => "阴",
        45 | 48 => "雾",
        51 => "毛毛雨",
        53 => "小雨",
        55 => "细雨",
        56 | 57 | 66 | 67 => "冻雨",
        61 => "小雨",
        63 => "中雨",
        65 => "大雨",
        71 => "小雪",
        73 => "中雪",
        75 => "大雪",
        77 => "米雪",
        80 => "阵雨",
        81 => "强阵雨",
        82 => "暴雨",
        85 => "阵雪",
        86 => "强阵雪",
        95 => "雷阵雨",
        96 | 99 => "雷暴伴冰雹",
        _ => "多云",
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Day {
    pub label: String,
    pub high: f32,
    pub low: f32,
    pub code: u8,
    pub rain: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub place: String,
    pub temp: f32,
    pub feels: Option<f32>,
    pub humidity: Option<u32>,
    pub wind: Option<f32>,
    pub code: u8,
    pub day: bool,
    pub high: Option<f32>,
    pub low: Option<f32>,
    pub days: Vec<Day>,
    /// 取得这份数据的 Unix 时间（秒）。
    pub at: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Position { pub lat: f64, pub lon: f64, pub label: String }

fn json(response: ureq::Response) -> Result<Value, String> {
    serde_json::from_reader(response.into_reader().take(1024 * 1024)).map_err(|_| "天气响应格式无效".to_string())
}

fn get(agent: &ureq::Agent, url: &url::Url) -> Result<Value, String> {
    agent.get(url.as_str()).set("Accept", "application/json").call()
        .map_err(|error| match error { ureq::Error::Status(code, _) => format!("服务返回 {code}"), _ => "网络不可用".to_string() })
        .and_then(json)
}

/// 按网络出口定位；国内常见出口能拿到中文城市名。
pub fn locate(agent: &ureq::Agent) -> Result<Position, String> {
    let url = url::Url::parse("https://ipwho.is/?lang=zh-CN").map_err(|_| "定位服务地址无效")?;
    let value = get(agent, &url)?;
    if !value["success"].as_bool().unwrap_or(false) { return Err("自动定位失败，请在设置里填写城市".into()); }
    let lat = value["latitude"].as_f64().ok_or("定位结果缺少坐标")?;
    let lon = value["longitude"].as_f64().ok_or("定位结果缺少坐标")?;
    let label = value["city"].as_str().filter(|s| !s.trim().is_empty())
        .or_else(|| value["region"].as_str().filter(|s| !s.trim().is_empty()))
        .or_else(|| value["country"].as_str().filter(|s| !s.trim().is_empty()))
        .unwrap_or("当地").trim().to_string();
    Ok(Position { lat, lon, label })
}

/// 城市名 → 坐标（Open-Meteo 的地理编码，支持中英文）。
pub fn geocode(agent: &ureq::Agent, name: &str) -> Result<Position, String> {
    let mut url = url::Url::parse("https://geocoding-api.open-meteo.com/v1/search").map_err(|_| "定位服务地址无效")?;
    url.query_pairs_mut().append_pair("name", name).append_pair("count", "1").append_pair("language", "zh").append_pair("format", "json");
    let value = get(agent, &url)?;
    let first = value["results"].get(0).ok_or_else(|| format!("没找到「{name}」这个城市"))?;
    let lat = first["latitude"].as_f64().ok_or("定位结果缺少坐标")?;
    let lon = first["longitude"].as_f64().ok_or("定位结果缺少坐标")?;
    let label = first["name"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or(name).to_string();
    Ok(Position { lat, lon, label })
}

/// 拉一次天气：先定位，再取实况与四日预报。
pub fn fetch(agent: &ureq::Agent, city: &str) -> Result<Snapshot, String> {
    let city = city.trim();
    let position = if city.is_empty() { locate(agent)? } else { geocode(agent, city)? };
    let mut url = url::Url::parse("https://api.open-meteo.com/v1/forecast").map_err(|_| "天气服务地址无效")?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("latitude", &format!("{:.4}", position.lat));
        query.append_pair("longitude", &format!("{:.4}", position.lon));
        query.append_pair("current", "temperature_2m,relative_humidity_2m,apparent_temperature,is_day,weather_code,wind_speed_10m");
        query.append_pair("daily", "weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max");
        query.append_pair("timezone", "auto");
        query.append_pair("forecast_days", "4");
    }
    parse(&position.label, &get(agent, &url)?)
}

fn day_label(index: usize, date: &str) -> String {
    use chrono::Datelike;
    match index {
        0 => "今天".into(),
        1 => "明天".into(),
        _ => chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map(|d| format!("周{}", ["一", "二", "三", "四", "五", "六", "日"][d.weekday().num_days_from_monday() as usize]))
            .unwrap_or_else(|_| date.get(5..).unwrap_or("").to_string()),
    }
}

pub fn parse(place: &str, value: &Value) -> Result<Snapshot, String> {
    let current = &value["current"];
    let temp = current["temperature_2m"].as_f64().ok_or("天气数据缺少温度")? as f32;
    let daily = &value["daily"];
    let dates: Vec<&Value> = daily["time"].as_array().map(|items| items.iter().collect()).unwrap_or_default();
    let codes = daily["weather_code"].as_array();
    let highs = daily["temperature_2m_max"].as_array();
    let lows = daily["temperature_2m_min"].as_array();
    let rains = daily["precipitation_probability_max"].as_array();
    let mut days = Vec::new();
    for (index, date) in dates.iter().enumerate().take(4) {
        let date = date.as_str().unwrap_or_default();
        let pick = |list: Option<&Vec<Value>>| list.and_then(|items| items.get(index)).and_then(Value::as_f64);
        days.push(Day {
            label: day_label(index, date),
            high: pick(highs).unwrap_or_default() as f32,
            low: pick(lows).unwrap_or_default() as f32,
            code: codes.and_then(|items| items.get(index)).and_then(Value::as_u64).unwrap_or(2) as u8,
            rain: rains.and_then(|items| items.get(index)).and_then(Value::as_u64).unwrap_or(0) as u32,
        });
    }
    if days.is_empty() { return Err("天气数据不完整".into()); }
    Ok(Snapshot {
        place: place.to_string(),
        temp,
        feels: current["apparent_temperature"].as_f64().map(|n| n as f32),
        humidity: current["relative_humidity_2m"].as_u64().map(|n| n as u32),
        wind: current["wind_speed_10m"].as_f64().map(|n| n as f32),
        code: current["weather_code"].as_u64().unwrap_or(2) as u8,
        day: current["is_day"].as_u64().unwrap_or(1) != 0,
        high: days.first().map(|d| d.high),
        low: days.first().map(|d| d.low),
        days,
        at: chrono::Utc::now().timestamp(),
    })
}

/// `17:42` 形式的更新时间。
pub fn clock(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".into())
}

/// 页面与设置页共享的天气状态；拉取在后台线程，UI 只轮询结果。
#[derive(Default)]
pub struct State {
    pub snapshot: Option<Snapshot>,
    pub error: Option<String>,
    pub loading: bool,
    pub last_start: f64,
    /// 上一次请求用的城市；设置里改了城市就立即重取。
    last_city: Option<String>,
    rx: Option<Receiver<Result<Snapshot, String>>>,
    cache: Option<PathBuf>,
}

impl State {
    pub fn load() -> Self {
        if cfg!(test) { return Self::default(); }
        let path = crate::model::config_dir().join("weather.json");
        let snapshot = std::fs::read(&path).ok().and_then(|raw| serde_json::from_slice::<Snapshot>(&raw).ok());
        Self { snapshot, error: None, loading: false, last_start: 0.0, last_city: None, rx: None, cache: Some(path) }
    }

    /// 设置里的城市是否和上次请求不一致（首次启动也算）。
    pub fn city_changed(&self, city: &str) -> bool { self.last_city.as_deref() != Some(city.trim()) }

    pub fn refresh(&mut self, city: &str, t: f64, force: bool) {
        if self.loading { return; }
        // 失败后不要连点连试：成功过就等 30 秒，连续失败拉长到 2 分钟。
        let cooldown = if self.error.is_some() { 120.0 } else { 30.0 };
        if !force && self.last_start > 0.0 && t - self.last_start < cooldown { return; }
        let city = city.trim().to_string();
        self.last_city = Some(city.clone());
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.loading = true;
        self.last_start = t;
        std::thread::spawn(move || {
            let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(12)).build();
            let _ = tx.send(fetch(&agent, &city));
        });
    }

    /// 是否有新结果（外壳据此立即重绘）。
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false; };
        match rx.try_recv() {
            Ok(result) => {
                self.rx = None;
                self.loading = false;
                match result {
                    Ok(snapshot) => {
                        if let Some(path) = &self.cache {
                            if let Ok(raw) = serde_json::to_vec(&snapshot) { let _ = crate::model::atomic_write(path, &raw); }
                        }
                        self.snapshot = Some(snapshot);
                        self.error = None;
                    }
                    Err(error) => self.error = Some(error),
                }
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.rx = None;
                self.loading = false;
                self.error = Some("天气服务没有响应".into());
                true
            }
        }
    }

    /// 数据是否该刷新了（首次打开也算）。
    pub fn stale(&self, now: i64) -> bool { self.snapshot.as_ref().map_or(true, |s| now - s.at >= INTERVAL) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "current": {"temperature_2m": 9.5, "relative_humidity_2m": 58, "apparent_temperature": 5.6, "is_day": 0, "weather_code": 0, "wind_speed_10m": 13.9},
            "daily": {
                "time": ["2026-11-09", "2026-11-10", "2026-11-11", "2026-11-12"],
                "weather_code": [51, 2, 2, 61],
                "temperature_2m_max": [15.2, 14.5, 17.4, 19.2],
                "temperature_2m_min": [6.8, 4.7, 5.0, 8.2],
                "precipitation_probability_max": [60, 0, 0, 25]
            }
        })
    }

    #[test]
    fn parses_current_and_four_days() {
        let s = parse("杭州", &sample()).unwrap();
        assert_eq!(s.place, "杭州");
        assert_eq!(s.temp, 9.5);
        assert_eq!(s.feels, Some(5.6));
        assert_eq!(s.humidity, Some(58));
        assert_eq!(s.wind, Some(13.9));
        assert!(!s.day, "is_day=0 应识别为夜间");
        assert_eq!(s.high, Some(15.2));
        assert_eq!(s.low, Some(6.8));
        assert_eq!(s.days.len(), 4);
        assert_eq!(s.days[0].label, "今天");
        assert_eq!(s.days[1].label, "明天");
        assert_eq!(s.days[2].label, "周三");
        assert_eq!(s.days[3].rain, 25);
    }

    #[test]
    fn rejects_payload_without_temperature() {
        assert!(parse("杭州", &json!({"daily": {"time": ["2026-11-09"]}})).is_err());
        assert!(parse("杭州", &json!({"current": {"temperature_2m": 3.0}})).is_err(), "没有日预报也不应假装成功");
    }

    #[test]
    fn kind_and_text_follow_wmo_codes() {
        assert_eq!(kind(0, true), Kind::Sun);
        assert_eq!(kind(0, false), Kind::Moon);
        assert_eq!(kind(95, true), Kind::Thunder);
        assert_eq!(kind(75, true), Kind::Snow);
        assert_eq!(describe(0), "晴");
        assert_eq!(describe(63), "中雨");
        assert_eq!(describe(200), "多云", "未知代码退回多云");
    }

    #[test]
    fn stale_after_fifteen_minutes() {
        let mut state = State::default();
        assert!(state.stale(0), "还没有数据时应刷新");
        state.snapshot = Some(Snapshot { at: 1_000, ..Default::default() });
        assert!(!state.stale(1_000 + INTERVAL - 1));
        assert!(state.stale(1_000 + INTERVAL));
    }

    #[test]
    fn city_change_triggers_a_new_request() {
        let mut state = State::default();
        assert!(state.city_changed(""), "首次启动应请求一次");
        state.last_city = Some(String::new());
        assert!(!state.city_changed(""), "城市没变就不该重复请求");
        assert!(state.city_changed("杭州"));
        state.last_city = Some("杭州".into());
        assert!(!state.city_changed(" 杭州 "), "首尾空格不应算作换城市");
    }


    #[test]
    #[ignore="按需联网核验本机定位与天气"]
    fn weather_local_probe(){
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build();
        let auto = fetch(&agent, "").expect("自动定位失败");
        println!("auto_place={} temp={:.1} code={} days={:?}", auto.place, auto.temp, auto.code,
            auto.days.iter().map(|d| (&d.label, d.high, d.low, d.rain)).collect::<Vec<_>>());
        let named = fetch(&agent, "杭州").expect("按城市查询失败");
        println!("named_place={} temp={:.1}", named.place, named.temp);
    }

    #[test]
    fn clock_formats_local_time() {
        let text = clock(0);
        assert_eq!(text.len(), 5);
        assert!(text.contains(':'));
        let _ = clock(i64::MAX); // 极端时间只应退化成提示，不应崩溃
    }
}
