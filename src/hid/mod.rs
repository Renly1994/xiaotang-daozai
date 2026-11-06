//! 【新增】与品牌解耦的 HID 电量快照、协议能力与本地反馈。
//! 平台层只负责 I/O，协议层只发送已知的电量查询；未知接口仅描述，不探测私有命令。

mod polling;
mod protocols;
mod service;
#[cfg(windows)]
mod windows;

pub use polling::{refresh_seconds, DEFAULT_REFRESH_SECS, MAX_REFRESH_SECS};
use serde_json::{json, Value};
pub use service::Service;

pub const MAX_INTERFACES: usize = 128;
pub const MAX_FIELDS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportType {
    Input,
    Output,
    Feature,
}
impl ReportType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Feature => "feature",
        }
    }
}

/// 【新增】只保存描述符的结构；这里没有任何按键值、序列号或私有原始输入。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub report_type: ReportType,
    pub report_id: u8,
    pub usage_page: u16,
    pub usage_min: u16,
    pub usage_max: u16,
    pub link_collection: u16,
    pub bit_size: u16,
    pub report_count: u16,
    pub logical_min: i32,
    pub logical_max: i32,
    pub absolute: bool,
}
impl Field {
    pub fn battery_usage(&self) -> Option<u16> {
        if self.report_type == ReportType::Output
            || !self.absolute
            || self.report_count != 1
            || self.bit_size == 0
            || self.bit_size > 32
        {
            return None;
        }
        let usage = match self.usage_page {
            0x06 => 0x20,
            0x85 => 0x64,
            _ => return None,
        };
        (self.usage_min == usage
            && self.usage_max == usage
            && self.logical_min >= 0
            && self.logical_max > self.logical_min)
            .then_some(usage)
    }
    pub fn percent(&self, value: u32) -> Option<u8> {
        self.battery_usage()?;
        let (min, max, value) = (
            i64::from(self.logical_min),
            i64::from(self.logical_max),
            i64::from(value),
        );
        if value < min || value > max {
            return None;
        }
        // 【新增】85:64 的单位本来就是百分比；不能按 8 位容器上限再次缩放。
        if self.usage_page == 0x85 {
            return (value <= 100).then_some(value as u8);
        }
        Some((((value - min) * 100 + (max - min) / 2) / (max - min)) as u8)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Descriptor {
    pub vid: u16,
    pub pid: u16,
    pub release: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub interface: i32,
    pub bus: String,
    pub input_len: u16,
    pub output_len: u16,
    pub feature_len: u16,
    /// Input / Output / Feature 顺序；仅描述符 ID 和能力数量，不含任何报告正文。
    pub report_ids: [Vec<u8>; 3],
    pub value_caps: [u16; 3],
    pub button_caps: [u16; 3],
    pub fields: Vec<Field>,
    pub fields_truncated: bool,
    pub error: Option<Failure>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Unsupported,
    Access,
    Io,
    Timeout,
    Offline,
    InvalidReply,
    Budget,
    Cancelled,
    Disabled,
}
impl Failure {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Access => "access_denied",
            Self::Io => "io_error",
            Self::Timeout => "timeout",
            Self::Offline => "offline",
            Self::InvalidReply => "invalid_reply",
            Self::Budget => "scan_budget",
            Self::Cancelled => "cancelled",
            Self::Disabled => "disabled",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Unsupported => "暂未适配电量协议",
            Self::Access => "无法访问接口",
            Self::Io => "设备通信失败",
            Self::Timeout => "未回复，设备可能休眠",
            Self::Offline => "接收器报告设备离线",
            Self::InvalidReply => "电量回复无效",
            Self::Budget => "本轮采集预算已用完",
            Self::Cancelled => "采集已停止",
            Self::Disabled => "HID 采集已关闭",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    pub percent: u8,
    pub charging: Option<bool>,
    pub millivolts: Option<u16>,
}

#[derive(Clone, Debug)]
pub struct Device {
    /// 【修改】接口路径的稳定本地散列（含接收器槽位），供显示偏好关联，不写入反馈文件。
    pub id: String,
    pub name: String,
    pub descriptor: Descriptor,
    pub protocol: &'static str,
    pub transport: &'static str,
    pub slot: Option<u8>,
    pub battery: Option<Battery>,
    pub failure: Option<Failure>,
}

/// 【新增】缓存读数保留实际采样时刻，不能因另一台设备刷新而假装重新采样。
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub collected_at: u64,
    pub interval_seconds: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub devices: Vec<Device>,
    pub collected_at: u64,
    pub generation: u64,
    pub error: Option<Failure>,
    pub truncated: bool,
    pub samples: std::collections::BTreeMap<String, Sample>,
}
impl Snapshot {
    /// 【新增】服务/驱动长期未返回时，界面不继续把旧读数显示成当前电量。
    pub fn stale_at(&self, now: u64) -> bool {
        self.collected_at > 0 && now.saturating_sub(self.collected_at) > 120
    }
    /// 【新增】自定义长间隔允许两个采样周期，后台整体失联仍按 120 秒判断。
    pub fn device_stale_at(&self, id: &str, now: u64) -> bool {
        if self.stale_at(now) {
            return true;
        }
        self.samples.get(id).is_some_and(|s| {
            s.collected_at > 0
                && now.saturating_sub(s.collected_at)
                    > u64::from(s.interval_seconds)
                        .saturating_mul(2)
                        .saturating_add(12)
                        .max(120)
        })
    }
}

/// 【新增】白名单序列化，而非直接序列化 Device；防止今后增加路径/名称字段时意外公开。
pub fn feedback(snapshot: &Snapshot) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let devices:Vec<Value>=snapshot.devices.iter().take(MAX_INTERFACES*7).enumerate().map(|(index,d)|{
        let fields:Vec<Value>=d.descriptor.fields.iter().take(MAX_FIELDS).map(|f|json!({
            "type":f.report_type.label(),"report_id":f.report_id,"usage_page":f.usage_page,
            "usage_min":f.usage_min,"usage_max":f.usage_max,"link_collection":f.link_collection,
            "bit_size":f.bit_size,"report_count":f.report_count,"logical_min":f.logical_min,
            "logical_max":f.logical_max,"absolute":f.absolute,
        })).collect();
        json!({"device":index+1,"vid":format!("{:04X}",d.descriptor.vid),"pid":format!("{:04X}",d.descriptor.pid),
            "release":d.descriptor.release,"usage_page":d.descriptor.usage_page,"usage":d.descriptor.usage,
            "interface":d.descriptor.interface,"bus":d.descriptor.bus,"report_bytes":{"input":d.descriptor.input_len,"output":d.descriptor.output_len,"feature":d.descriptor.feature_len},
            "report_ids":{"input":d.descriptor.report_ids[0],"output":d.descriptor.report_ids[1],"feature":d.descriptor.report_ids[2]},
            "value_cap_counts":d.descriptor.value_caps,"button_cap_counts":d.descriptor.button_caps,
            "fields":fields,"fields_truncated":d.descriptor.fields_truncated,"descriptor_error":d.descriptor.error.map(Failure::code),
            "protocol":d.protocol,"transport":d.transport,"receiver_slot":d.slot,
            "sampled_at_unix":snapshot.samples.get(&d.id).map_or(snapshot.collected_at,|s|s.collected_at),
            "refresh_seconds":snapshot.samples.get(&d.id).map(|s|s.interval_seconds),
            "stale":snapshot.device_stale_at(&d.id,now),
            "battery":d.battery.map(|b|json!({"percent":b.percent,"charging":b.charging,"millivolts":b.millivolts})),
            "failure":d.failure.map(Failure::code)})
    }).collect();
    serde_json::to_string_pretty(&json!({"schema":"daozai.hid-feedback","schema_version":1,
        "app_version":env!("CARGO_PKG_VERSION"),"platform":std::env::consts::OS,"architecture":std::env::consts::ARCH,
        "collected_at_unix":snapshot.collected_at,"stale":snapshot.stale_at(now),"scan_error":snapshot.error.map(Failure::code),"truncated":snapshot.truncated,
        "privacy":"No serial numbers, paths, names, MAC addresses, raw reports or keyboard/mouse input.","devices":devices})).unwrap_or_default()
}

/// 【新增】用户主动导出时创建新文件，避免覆盖旧反馈；只保存已预览的白名单 JSON。
pub fn export_report(text: &str) -> Result<std::path::PathBuf, std::io::Error> {
    use std::io::Write;
    let directory = crate::model::config_dir().join("diagnostics");
    std::fs::create_dir_all(&directory)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = directory.join(format!("hid-feedback-{stamp}.json"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(text.as_bytes())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn field() -> Field {
        Field {
            report_type: ReportType::Feature,
            report_id: 1,
            usage_page: 6,
            usage_min: 32,
            usage_max: 32,
            link_collection: 0,
            bit_size: 8,
            report_count: 1,
            logical_min: 0,
            logical_max: 255,
            absolute: true,
        }
    }
    #[test]
    fn standard_battery_normalizes_without_clamping_or_guessing() {
        let mut f = field();
        assert_eq!(f.percent(128), Some(50));
        assert_eq!(f.percent(0), Some(0));
        assert_eq!(f.percent(255), Some(100));
        assert_eq!(f.percent(256), None);
        f.logical_max = 0;
        assert_eq!(f.percent(0), None);
        f.logical_max = 100;
        f.usage_page = 7;
        assert_eq!(f.percent(80), None);
        f = field();
        f.report_type = ReportType::Output;
        assert_eq!(f.battery_usage(), None);
        f = field();
        f.usage_max = 33;
        assert_eq!(f.battery_usage(), None);
    }
    #[test]
    fn battery_system_percent_is_not_scaled_by_storage_range() {
        let mut f = field();
        f.usage_page = 0x85;
        f.usage_min = 0x64;
        f.usage_max = 0x64;
        assert_eq!(f.percent(60), Some(60));
        assert_eq!(f.percent(100), Some(100));
        assert_eq!(f.percent(101), None);
    }
    #[test]
    fn stale_readings_expire_without_breaking_on_backwards_clock() {
        let snapshot = Snapshot {
            collected_at: 100,
            ..Default::default()
        };
        assert!(!snapshot.stale_at(99));
        assert!(!snapshot.stale_at(220));
        assert!(snapshot.stale_at(221));
    }
    #[test]
    fn device_age_respects_custom_interval_and_other_device_refreshes() {
        let mut snapshot = Snapshot {
            collected_at: 1000,
            ..Default::default()
        };
        snapshot.samples.insert(
            "slow".into(),
            Sample {
                collected_at: 800,
                interval_seconds: 300,
            },
        );
        snapshot.samples.insert(
            "old".into(),
            Sample {
                collected_at: 100,
                interval_seconds: 5,
            },
        );
        assert!(!snapshot.device_stale_at("slow", 1000));
        assert!(snapshot.device_stale_at("old", 1000));
        assert!(
            snapshot.device_stale_at("slow", 1121),
            "整个采集线程失联不能继续显示缓存"
        );
    }
    #[test]
    fn feedback_excludes_names_ids_and_raw_input() {
        let descriptor = Descriptor {
            vid: 0x3554,
            pid: 0xfb3e,
            fields: vec![field()],
            ..Default::default()
        };
        let snapshot = Snapshot {
            devices: vec![Device {
                id: "private-serial-path-MAC".into(),
                name: "Julian personal keyboard".into(),
                descriptor,
                protocol: "standard-hid",
                transport: "HID",
                slot: None,
                battery: None,
                failure: Some(Failure::Unsupported),
            }],
            ..Default::default()
        };
        let text = feedback(&snapshot);
        assert!(!text.contains("Julian"));
        assert!(!text.contains("private-serial"));
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["devices"][0]["failure"], "unsupported");
        assert_eq!(value["devices"][0]["vid"], "3554");
    }
}
