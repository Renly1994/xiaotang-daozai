//! 宿主内置的接收器表与插件集市提供的扩展数据。
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

const PACKAGE_ID: &str = "daozai.wireless-receivers";
#[cfg(windows)]
const BUILTIN: &[u8] = include_bytes!("../../assets/builtin-usb-receivers.json");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BatteryProtocol {
    CompX,
    LogitechHidpp20,
    EndgameXm2we,
    HyperXCloudIiCore,
    HyperXCloudIiiS,
    HyperXCloudAlpha2,
    SteelSeriesNova7,
    SteelSeriesNovaElite,
    MouseFeature83,
    Vaxee4k,
    MchoseM7Ultra,
    MchoseA5ProMax,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    id: String,
    api_version: u32,
    version: String,
    receivers: Vec<Receiver>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Receiver {
    vid: String,
    pid: String,
    name: String,
    #[serde(default, deserialize_with = "present_protocol")]
    battery_protocol: Option<String>,
}

fn present_protocol<'de, D: serde::Deserializer<'de>>(
    value: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(value).map(Some)
}

pub(crate) struct Package {
    version: String,
    table: ReceiverTable,
}

impl Package {
    pub(crate) fn parse(raw: &[u8]) -> Result<Self, &'static str> {
        let manifest: Manifest = serde_json::from_slice(raw).map_err(|_| "兼容包格式无效")?;
        if manifest.id != PACKAGE_ID {
            return Err("此文件不是岛仔无线设备兼容包");
        }
        if !matches!(manifest.api_version, 1 | 2) {
            return Err("当前岛仔版本不支持此兼容包接口");
        }
        if manifest.api_version == 1
            && manifest
                .receivers
                .iter()
                .any(|receiver| receiver.battery_protocol.is_some())
        {
            return Err("兼容包接口版本 1 不支持电量协议");
        }
        if !valid_text(&manifest.version, 32) {
            return Err("兼容包版本无效");
        }
        Ok(Self {
            version: manifest.version,
            table: ReceiverTable::new(manifest.receivers)?,
        })
    }

    #[cfg(any(test, all(windows, feature = "sdk-js")))]
    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    #[cfg(all(windows, feature = "sdk-js"))]
    pub(crate) fn receivers(&self) -> impl ExactSizeIterator<Item = ((u16, u16), &str)> {
        self.table
            .0
            .iter()
            .map(|(&id, info)| (id, info.name.as_str()))
    }

    pub(super) fn receiver_name(&self, hardware: &str) -> Option<&str> {
        self.table.receiver_name(hardware)
    }

    pub(super) fn battery_protocol(&self, hardware: &str) -> Option<BatteryProtocol> {
        self.table.battery_protocol(hardware)
    }
}

struct ReceiverInfo {
    name: String,
    battery_protocol: Option<BatteryProtocol>,
}

pub(super) struct ReceiverTable(BTreeMap<(u16, u16), ReceiverInfo>);

impl ReceiverTable {
    fn new(rows: Vec<Receiver>) -> Result<Self, &'static str> {
        let mut receivers = BTreeMap::new();
        for receiver in rows {
            let vid = usb_id(&receiver.vid)
                .filter(|id| *id != 0)
                .ok_or("接收器 VID 无效")?;
            let pid = usb_id(&receiver.pid).ok_or("接收器 PID 无效")?;
            if !valid_text(&receiver.name, 64) {
                return Err("接收器名称无效");
            }
            let battery_protocol = match receiver.battery_protocol.as_deref() {
                Some("compx") => Some(BatteryProtocol::CompX),
                Some("logitech-hidpp-2") => Some(BatteryProtocol::LogitechHidpp20),
                Some("endgame-xm2we") => Some(BatteryProtocol::EndgameXm2we),
                Some("hyperx-cloud-ii-core") => Some(BatteryProtocol::HyperXCloudIiCore),
                Some("hyperx-cloud-iii-s") => Some(BatteryProtocol::HyperXCloudIiiS),
                Some("hyperx-cloud-alpha-2") => Some(BatteryProtocol::HyperXCloudAlpha2),
                Some("steelseries-nova-7") => Some(BatteryProtocol::SteelSeriesNova7),
                Some("steelseries-nova-elite") => Some(BatteryProtocol::SteelSeriesNovaElite),
                Some("mouse-feature-83") => Some(BatteryProtocol::MouseFeature83),
                Some("vaxee-4k") => Some(BatteryProtocol::Vaxee4k),
                Some("mchose-m7-ultra") => Some(BatteryProtocol::MchoseM7Ultra),
                Some("mchose-a5-pro-max") => Some(BatteryProtocol::MchoseA5ProMax),
                Some(id)
                    if id.is_empty()
                        || id.len() > 32
                        || !id.bytes().all(|byte| {
                            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                        }) =>
                {
                    return Err("接收器电量协议名称无效");
                }
                _ => None,
            };
            let info = ReceiverInfo {
                name: receiver.name,
                battery_protocol,
            };
            if receivers.insert((vid, pid), info).is_some() {
                return Err("兼容包有重复的接收器 VID/PID");
            }
        }
        Ok(Self(receivers))
    }

    pub(super) fn receiver_name(&self, hardware: &str) -> Option<&str> {
        self.receiver(hardware)
            .map(|receiver| receiver.name.as_str())
    }

    pub(super) fn battery_protocol(&self, hardware: &str) -> Option<BatteryProtocol> {
        self.receiver(hardware)?.battery_protocol
    }

    fn receiver(&self, hardware: &str) -> Option<&ReceiverInfo> {
        let (vid, pid) = hardware.strip_prefix("VID_")?.split_once("&PID_")?;
        self.0.get(&(usb_id(vid)?, usb_id(pid)?))
    }
}

#[cfg(windows)]
pub(super) fn builtin_receivers() -> &'static ReceiverTable {
    static TABLE: std::sync::OnceLock<ReceiverTable> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let rows = serde_json::from_slice(BUILTIN).expect("内置接收器识别表应使用有效的 JSON");
        ReceiverTable::new(rows).expect("内置接收器识别表应包含有效的设备数据")
    })
}

fn usb_id(hex: &str) -> Option<u16> {
    if hex.len() != 4 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u16::from_str_radix(hex, 16).ok()
}

fn valid_text(text: &str, max: usize) -> bool {
    !text.is_empty()
        && text.trim() == text
        && text.chars().count() <= max
        && !text.chars().any(char::is_control)
}

/// 供 USB 枚举线程读取当前启用的插件数据。安装状态由集市保存。
#[derive(Clone, Default)]
pub struct Store {
    package: Arc<Mutex<Option<Arc<Package>>>>,
    #[cfg(windows)] component: Option<super::halo_component::Component>,
    usb: Arc<Mutex<Option<(std::time::Instant, Vec<String>)>>>,
}

impl Store {
    #[cfg(windows)]
    pub(crate) fn with_component(mut self, component: super::halo_component::Component) -> Self {
        self.component = Some(component);
        self
    }

    #[cfg(windows)]
    pub(super) fn component(&self) -> Option<&super::halo_component::Component> {
        self.component.as_ref()
    }

    #[cfg(any(test, all(windows, feature = "sdk-js")))]
    pub(crate) fn set(&self, package: Option<Package>) {
        *self.package.lock().unwrap_or_else(|error| error.into_inner()) = package.map(Arc::new);
    }

    pub(crate) fn package(&self) -> Option<Arc<Package>> {
        self.package
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// Reuse the USB worker's complete inventory; plugins do not enumerate hardware on UI frames.
    pub(crate) fn set_usb_instances(&self, instances: Vec<String>, now: std::time::Instant) {
        *self.usb.lock().unwrap_or_else(|error|error.into_inner())=Some((now,instances));
    }
    pub(crate) fn usb_instances_at(&self, now: std::time::Instant)->Vec<String> {
        self.usb.lock().unwrap_or_else(|error|error.into_inner()).as_ref()
            .filter(|(at,_)|now.saturating_duration_since(*at)<=std::time::Duration::from_secs(6))
            .map(|(_,instances)|instances.clone()).unwrap_or_default()
    }
    pub(crate) fn clear_usb_instances(&self) {
        *self.usb.lock().unwrap_or_else(|error|error.into_inner())=None;
    }
}

#[cfg(all(windows, test))]
pub(super) fn builtin_data() -> Vec<serde_json::Value> {
    serde_json::from_slice(BUILTIN).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validates_receiver_extension_data() {
        let valid = json!({
            "id": PACKAGE_ID,
            "api_version": 1,
            "version": "1.0.3",
            "receivers": [{"vid": "046D", "pid": "c542", "name": "罗技 Nano 接收器"}]
        });
        let package = Package::parse(&serde_json::to_vec(&valid).unwrap()).unwrap();
        assert_eq!(package.version(), "1.0.3");
        assert_eq!(
            package.receiver_name("VID_046D&PID_C542"),
            Some("罗技 Nano 接收器")
        );
        assert_eq!(package.receiver_name("VID_046D&PID_C542&MI_00"), None);
        assert_eq!(package.battery_protocol("VID_046D&PID_C542"), None);

        for (field, value) in [
            ("id", json!("example.receiver")),
            ("api_version", json!(3)),
            ("version", json!(" ")),
            ("cover_png", json!("封面由插件包提供")),
            (
                "receivers",
                json!([{"vid": "0000", "pid": "0001", "name": "接收器"}]),
            ),
            (
                "receivers",
                json!([{"vid": "046D", "pid": "C54", "name": "接收器"}]),
            ),
            (
                "receivers",
                json!([{"vid": "046D", "pid": "C542", "name": " "}]),
            ),
            (
                "receivers",
                json!([
                    {"vid": "046D", "pid": "C542", "name": "接收器"},
                    {"vid": "046d", "pid": "c542", "name": "重复接收器"}
                ]),
            ),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(
                Package::parse(&serde_json::to_vec(&invalid).unwrap()).is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn validates_receiver_battery_protocols() {
        let data = json!({
            "id": PACKAGE_ID,
            "api_version": 2,
            "version": "1.1.0",
            "receivers": [
                {"vid": "3554", "pid": "f58a", "name": "VXE 接收器", "battery_protocol": "compx"},
                {"vid": "046D", "pid": "C539", "name": "罗技接收器", "battery_protocol": "logitech-hidpp-2"},
                {"vid": "046D", "pid": "C542", "name": "罗技 Nano 接收器"}
            ]
        });
        let parse = |data: &serde_json::Value| Package::parse(&serde_json::to_vec(data).unwrap());
        let package = parse(&data).unwrap();
        assert_eq!(
            package.receiver_name("VID_3554&PID_F58A"),
            Some("VXE 接收器")
        );
        assert_eq!(
            package.battery_protocol("VID_3554&PID_F58A"),
            Some(BatteryProtocol::CompX)
        );
        assert_eq!(
            package.battery_protocol("VID_046D&PID_C539"),
            Some(BatteryProtocol::LogitechHidpp20)
        );
        assert_eq!(package.battery_protocol("VID_046D&PID_C542"), None);
        assert_eq!(package.battery_protocol("VID_3554&PID_F58A&MI_00"), None);

        for (name, protocol) in [
            ("endgame-xm2we", BatteryProtocol::EndgameXm2we),
            ("hyperx-cloud-ii-core", BatteryProtocol::HyperXCloudIiCore),
            ("hyperx-cloud-iii-s", BatteryProtocol::HyperXCloudIiiS),
            ("hyperx-cloud-alpha-2", BatteryProtocol::HyperXCloudAlpha2),
            ("steelseries-nova-7", BatteryProtocol::SteelSeriesNova7),
            (
                "steelseries-nova-elite",
                BatteryProtocol::SteelSeriesNovaElite,
            ),
            ("mouse-feature-83", BatteryProtocol::MouseFeature83),
            ("vaxee-4k", BatteryProtocol::Vaxee4k),
            ("mchose-m7-ultra", BatteryProtocol::MchoseM7Ultra),
            ("mchose-a5-pro-max", BatteryProtocol::MchoseA5ProMax),
        ] {
            let mut receiver = data.clone();
            receiver["receivers"][0]["battery_protocol"] = json!(name);
            assert_eq!(
                parse(&receiver)
                    .unwrap()
                    .battery_protocol("VID_3554&PID_F58A"),
                Some(protocol),
                "{name}"
            );
        }

        let mut invalid = data.clone();
        invalid["api_version"] = json!(1);
        assert!(parse(&invalid).is_err());

        let mut future = data.clone();
        future["receivers"][0]["battery_protocol"] = json!("future-reader");
        let future = parse(&future).unwrap();
        assert_eq!(
            future.receiver_name("VID_3554&PID_F58A"),
            Some("VXE 接收器")
        );
        assert_eq!(future.battery_protocol("VID_3554&PID_F58A"), None);
        assert_eq!(
            future.battery_protocol("VID_046D&PID_C539"),
            Some(BatteryProtocol::LogitechHidpp20)
        );

        for value in [json!(""), json!("send raw hex!"), serde_json::Value::Null] {
            let mut invalid = data.clone();
            invalid["receivers"][0]["battery_protocol"] = value;
            assert!(parse(&invalid).is_err());
        }

        let mut duplicate = data;
        duplicate["receivers"][2]["vid"] = json!("3554");
        duplicate["receivers"][2]["pid"] = json!("F58A");
        assert!(parse(&duplicate).is_err());
    }
}
