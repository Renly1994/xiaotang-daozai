//! Typed, bounded external-device output. No filesystem, process or HID access.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

fn default_fresh_seconds() -> u16 { 90 }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct PluginWirelessDevice {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub vendor_id: u16,
    pub product_id: u16,
    #[serde(default)]
    pub usb_instance: Option<String>,
    #[serde(default)]
    pub percent: Option<u8>,
    #[serde(default)]
    pub charging: Option<bool>,
    #[serde(default)]
    pub approximate: bool,
    #[serde(default)]
    pub online: bool,
    #[serde(default)]
    pub fresh: bool,
    #[serde(default)]
    pub measured_at_utc: Option<String>,
    #[serde(default)]
    pub connection_checked_at_utc: Option<String>,
    #[serde(default)]
    pub charging_checked_at_utc: Option<String>,
    #[serde(default = "default_fresh_seconds")]
    pub fresh_for_seconds: u16,
}

fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= limit && !value.chars().any(char::is_control)
}

fn source_time(value: Option<&str>, now: DateTime<Utc>) -> Result<(), &'static str> {
    if let Some(value) = value {
        let at = DateTime::parse_from_rfc3339(value).map_err(|_| "设备来源时间必须是 RFC3339")?;
        if value.len() > 64 || (at.with_timezone(&Utc) - now).num_milliseconds() > 5_000 {
            return Err("设备来源时间异常或超前");
        }
    }
    Ok(())
}

pub(super) fn validate(devices: &[PluginWirelessDevice], now: DateTime<Utc>) -> Result<(), &'static str> {
    if devices.len() > 64 { return Err("无线来源最多返回 64 台设备"); }
    let mut ids = BTreeSet::new();
    for device in devices {
        if !text(&device.id, 128) || !ids.insert(device.id.as_str()) || !text(&device.name, 80)
            || !matches!(device.kind.as_str(), "mouse" | "keyboard" | "gamepad" | "headset")
            || device.vendor_id == 0 || device.product_id == 0
            || device.percent.is_some_and(|percent| percent > 100)
            || !(30..=180).contains(&device.fresh_for_seconds)
            || device.usb_instance.as_ref().is_some_and(|value| !text(value, 512)) {
            return Err("无线设备的标识、类型、电量或有效期无效");
        }
        source_time(device.measured_at_utc.as_deref(), now)?;
        source_time(device.connection_checked_at_utc.as_deref(), now)?;
        source_time(device.charging_checked_at_utc.as_deref(), now)?;
        if device.percent.is_some() && device.measured_at_utc.is_none()
            || device.online && device.connection_checked_at_utc.is_none()
            || device.charging.is_some() && device.charging_checked_at_utc.is_none()
            || device.fresh && (!device.online || device.percent.is_none()) {
            return Err("电量、连接和充电状态需要各自真实来源时间");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> PluginWirelessDevice {
        serde_json::from_value(json!({"id":"keyboard-1","name":"Test keyboard","kind":"keyboard",
            "vendorId":2821,"productId":6862,"percent":95,"online":true,"fresh":true,
            "measuredAtUtc":"2026-11-28T04:00:00Z","connectionCheckedAtUtc":"2026-11-28T04:00:00Z",
            "charging":null,"freshForSeconds":150})).unwrap()
    }
    fn now() -> DateTime<Utc> { "2026-11-28T04:00:00Z".parse().unwrap() }
    #[test]
    fn wireless_output_preserves_source_time_and_nullable_charge() {
        let device=fixture(); assert!(validate(&[device.clone()],now()).is_ok());
        assert_eq!(device.measured_at_utc.as_deref(),Some("2026-11-28T04:00:00Z"));
        assert_eq!(device.charging,None);
        let mut old=device;old.fresh=false;old.measured_at_utc=Some("2026-11-28T03:55:00Z".into());
        assert!(validate(&[old],now()).is_ok(),"old evidence is retained, not renewed; native presentation ages it");
    }
    #[test]
    fn wireless_output_rejects_unsupported_or_unproven_values() {
        let original=serde_json::to_value(fixture()).unwrap();
        for (key,value) in [("percent",json!(101)),("kind",json!("receiver")),("freshForSeconds",json!(181)),
            ("id",json!("bad\nidentity")),("measuredAtUtc",json!("2026-11-28T04:00:06Z")),
            ("connectionCheckedAtUtc",json!(null)),("measuredAtUtc",json!(null))] {
            let mut record=original.clone();record[key]=value;
            let record=serde_json::from_value::<PluginWirelessDevice>(record);
            assert!(record.is_err()||validate(&[record.unwrap()],now()).is_err(),"{key}");
        }
        let mut charge=fixture();charge.charging=Some(true);
        assert!(validate(&[charge],now()).is_err());
    }
    #[test]
    fn wireless_output_rejects_duplicate_or_unbounded_lists() {
        assert!(validate(&[fixture(),fixture()],now()).is_err());
        assert!(validate(&vec![fixture();65],now()).is_err());
    }
}
