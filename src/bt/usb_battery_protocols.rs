//! 2.4G 接收器的只读电量查询。
//!
//! CompX 帧格式和回包样本参考 HaloBattery（MIT）与 python-pulsar-mouse-tool（MIT）。
//! https://github.com/HeyOkay/HaloBattery/blob/15a73ac4d88435a771b0060b28171733a43dab06/providers/compx.py
//! https://github.com/andrewrabert/python-pulsar-mouse-tool
//!
//! 完整许可声明见 THIRD_PARTY_NOTICES.txt。

mod compx_keyboard;
mod feature_mice;
mod hyperx;
mod inphic;
mod mchose;
mod steelseries;
mod xm2we;

use super::compat::BatteryProtocol;
use super::usb_hid::{self, Handle};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

const COMPX_REPORT_LEN: usize = 17;
const COMPX_QUERY: [u8; 2] = [0x08, 0x04];
const HIDPP_REPORT_LEN: usize = 20;
const HIDPP_LONG_REPORT: u8 = 0x11;
const HIDPP_UNIFIED_BATTERY: [u8; 2] = [0x10, 0x04];
const HIDPP_REQUEST_TIMEOUT: Duration = Duration::from_millis(140);
// 0 保留给设备事件，3–6 避开 G HUB 和 HaloBattery 已知的软件 ID。
static HIDPP_SOFTWARE_ID_SEQUENCE: AtomicU8 = AtomicU8::new(0);

pub(super) fn read(instance: &str, protocol: BatteryProtocol) -> Option<(u8, Option<bool>)> {
    match protocol {
        BatteryProtocol::CompX => {
            read_vendor_collection(instance, 0xff02, COMPX_REPORT_LEN, read_compx)
        }
        BatteryProtocol::CompXKeyboard => compx_keyboard::read(instance),
        BatteryProtocol::InphicIn10 => inphic::read_in10(instance),
        BatteryProtocol::LogitechHidpp20 => {
            read_vendor_collection(instance, 0xff00, HIDPP_REPORT_LEN, read_hidpp)
        }
        BatteryProtocol::EndgameXm2we => xm2we::read(instance),
        BatteryProtocol::HyperXCloudIiCore => hyperx::read_cloud_ii_core(instance),
        BatteryProtocol::HyperXCloudIiiS => hyperx::read_cloud_iii_s(instance),
        BatteryProtocol::HyperXCloudAlpha2 => hyperx::read_cloud_alpha_2(instance),
        BatteryProtocol::SteelSeriesNova7 => steelseries::read_nova_7(instance),
        BatteryProtocol::SteelSeriesNovaElite => steelseries::read_nova_elite(instance),
        BatteryProtocol::MouseFeature83 => feature_mice::read_report_83(instance),
        BatteryProtocol::Vaxee4k => feature_mice::read_vaxee(instance),
        BatteryProtocol::MchoseM7Ultra => mchose::read_m7_ultra(instance),
        BatteryProtocol::MchoseA5ProMax => mchose::read_a5_pro_max(instance),
    }
}

fn read_vendor_collection(
    instance: &str,
    page: u16,
    report_len: usize,
    read: fn(&mut Handle) -> Option<(u8, Option<bool>)>,
) -> Option<(u8, Option<bool>)> {
    let candidates: Vec<_> = usb_hid::collections(instance)
        .into_iter()
        .filter(|collection| {
            collection.usage_page == page
                && collection.usage == 0x0002
                && collection.input_len >= report_len
                && collection.output_len >= report_len
        })
        .collect();
    // ATK A9 SE 会同时报告 17 和 20 字节的集合，只有前者接受同一 CompX 请求。
    let prefer_exact = page == 0xff02 && candidates.iter().any(|c| c.output_len == report_len);
    let mut candidates = candidates
        .iter()
        .filter(|c| !prefer_exact || c.output_len == report_len);
    let collection = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }
    read(&mut collection.open()?)
}

fn compx_checksum(bytes: &[u8]) -> u8 {
    bytes
        .iter()
        .fold(0x55_u8, |sum, &byte| sum.wrapping_sub(byte))
}

fn compx_request() -> [u8; COMPX_REPORT_LEN] {
    let mut report = [0; COMPX_REPORT_LEN];
    report[..2].copy_from_slice(&COMPX_QUERY);
    report[16] = compx_checksum(&report[..16]);
    report
}

fn parse_compx(report: &[u8]) -> Option<(u8, Option<bool>)> {
    let report = report.get(..COMPX_REPORT_LEN)?;
    if !report.starts_with(&COMPX_QUERY)
        || report[16] != compx_checksum(&report[..16])
        || report[6] > 100
        || report == compx_request()
    {
        return None;
    }
    Some((report[6], Some(report[7] != 0)))
}

fn read_compx(handle: &mut Handle) -> Option<(u8, Option<bool>)> {
    let deadline = Instant::now() + Duration::from_millis(600);
    if !handle.write(&compx_request(), deadline) {
        return None;
    }
    while Instant::now() < deadline {
        if let Some(reading) = parse_compx(&handle.read(deadline)?) {
            return Some(reading);
        }
    }
    None
}

// HID++ 2.0 帧及功能寻址由罗技公开，统一电量字段参见 OpenLogi 的协议说明。
// https://github.com/Logitech/cpg-docs/blob/master/hidpp20/README.rst
// https://openlogi.org/hidpp/features/x1004-unified-battery
fn hidpp_request(slot: u8, feature: u8, function: u8, software_id: u8, params: &[u8]) -> [u8; 20] {
    let mut request = [0; HIDPP_REPORT_LEN];
    request[..4].copy_from_slice(&[
        HIDPP_LONG_REPORT,
        slot,
        feature,
        (function << 4) | software_id,
    ]);
    request[4..4 + params.len()].copy_from_slice(params);
    request
}

fn hidpp_payload<'a>(response: &'a [u8], request: &[u8; 20]) -> Option<&'a [u8]> {
    let response = response.get(..HIDPP_REPORT_LEN)?;
    response
        .starts_with(&request[..4])
        .then_some(&response[4..])
}

fn hidpp_error(response: &[u8], request: &[u8; 20]) -> bool {
    matches!(response, [HIDPP_LONG_REPORT, slot, 0xff, feature, command, _, ..]
        if *slot == request[1] && *feature == request[2] && *command == request[3])
}

fn query_hidpp(
    handle: &mut Handle,
    receiver_deadline: Instant,
    slot: u8,
    feature: u8,
    function: u8,
    params: &[u8],
) -> Option<[u8; 16]> {
    let deadline = receiver_deadline.min(Instant::now() + HIDPP_REQUEST_TIMEOUT);
    let software_id = 3 + (HIDPP_SOFTWARE_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed) & 0x03);
    let request = hidpp_request(slot, feature, function, software_id, params);
    if Instant::now() >= deadline || !handle.write(&request, deadline) {
        return None;
    }
    while Instant::now() < deadline {
        let response = handle.read(deadline)?;
        if hidpp_error(&response, &request) {
            return None;
        }
        if let Some(payload) = hidpp_payload(&response, &request) {
            return payload.try_into().ok();
        }
    }
    None
}

fn parse_hidpp_battery(capabilities: &[u8; 16], status: &[u8; 16]) -> Option<(u8, Option<bool>)> {
    if capabilities[1] & 0x02 == 0 || status[0] > 100 {
        return None;
    }
    let charging = match status[2] {
        0 | 3 => Some(false),
        1 | 2 | 4 => Some(true),
        _ => None,
    };
    Some((status[0], charging))
}

fn read_hidpp_slot(handle: &mut Handle, deadline: Instant, slot: u8) -> Option<(u8, Option<bool>)> {
    let feature = query_hidpp(handle, deadline, slot, 0, 0, &HIDPP_UNIFIED_BATTERY)?[0];
    if feature == 0 || feature == 0xff {
        return None;
    }
    let capabilities = query_hidpp(handle, deadline, slot, feature, 0, &[])?;
    if capabilities[1] & 0x02 == 0 {
        return None;
    }
    let status = query_hidpp(handle, deadline, slot, feature, 1, &[])?;
    parse_hidpp_battery(&capabilities, &status)
}

fn read_hidpp(handle: &mut Handle) -> Option<(u8, Option<bool>)> {
    let deadline = Instant::now() + Duration::from_millis(1_200);
    let mut reading = None;
    for slot in 1..=6 {
        if Instant::now() >= deadline {
            break;
        }
        if let Some(current) = read_hidpp_slot(handle, deadline, slot) {
            if reading.replace(current).is_some() {
                return None;
            }
        }
    }
    reading
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compx_reads_captured_receiver_and_charging_reports() {
        // HaloBattery 的 Pulsar X2 CrazyLight Mini 原机样本覆盖同一 CompX 帧格式。
        // https://github.com/HeyOkay/HaloBattery/blob/15a73ac4d88435a771b0060b28171733a43dab06/tests/test_pulsar.py#L104
        let receiver = [
            0x08, 0x04, 0, 0, 0, 2, 0x14, 0, 0x0d, 0xea, 0, 0, 0, 0, 0, 0, 0x3c,
        ];
        let charging = [
            0x08, 0x04, 0, 0, 0, 2, 0x19, 1, 0x0e, 0x52, 0, 0, 0, 0, 0, 0, 0xcd,
        ];
        // 3554:F505 接收器回包。
        // https://github.com/HeyOkay/HaloBattery/issues/323#issuecomment-6665645669
        let dragonfly = [
            0x08, 0x04, 0, 0, 0, 2, 0x19, 0, 0x0e, 0x46, 0, 0, 0, 0, 0, 0, 0xda,
        ];
        // ATK A9 SE 原装接收器，HaloBattery #347。
        let atk = [
            0x08, 0x04, 0, 0, 0, 2, 0x23, 0, 0x0e, 0x23, 0, 0, 0, 0, 0, 0, 0xf3,
        ];
        assert_eq!(
            compx_request(),
            [0x08, 0x04, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x49]
        );
        assert_eq!(parse_compx(&receiver), Some((20, Some(false))));
        assert_eq!(parse_compx(&charging), Some((25, Some(true))));
        assert_eq!(parse_compx(&dragonfly), Some((25, Some(false))));
        assert_eq!(parse_compx(&atk), Some((35, Some(false))));
        assert_eq!(parse_compx(&receiver[..16]), None);
        let mut invalid = receiver;
        invalid[16] ^= 0x01;
        assert_eq!(parse_compx(&invalid), None);
    }

    #[test]
    fn compx_distinguishes_real_zero_from_invalid_level() {
        let mut response = compx_request();
        assert_eq!(parse_compx(&response), None);
        response[5] = 2;
        response[16] = 0x47;
        assert_eq!(parse_compx(&response), Some((0, Some(false))));
        response[6] = 101;
        response[16] = 0xe2;
        assert_eq!(parse_compx(&response), None);
    }

    #[test]
    fn hidpp_requires_matching_slot_feature_function_and_software_id() {
        let request = hidpp_request(1, 0, 0, 0x0b, &HIDPP_UNIFIED_BATTERY);
        assert_eq!(request[..6], [0x11, 1, 0, 0x0b, 0x10, 0x04]);
        let mut response = request;
        response[4] = 7;
        assert_eq!(hidpp_payload(&response, &request).unwrap()[0], 7);
        assert_eq!(hidpp_payload(&response[..19], &request), None);
        for index in 0..4 {
            let mut other = response;
            other[index] ^= 0x01;
            assert_eq!(hidpp_payload(&other, &request), None);
        }
        assert!(hidpp_error(&[0x11, 1, 0xff, 0, 0x0b, 8], &request));
        assert!(!hidpp_error(&[0x11, 1, 0xff, 0, 0x0a, 8], &request));
    }

    #[test]
    fn hidpp_does_not_turn_coarse_level_or_error_into_zero() {
        let mut capabilities = [0; 16];
        let mut status = [0; 16];
        status[1] = 1;
        assert_eq!(parse_hidpp_battery(&capabilities, &status), None);
        capabilities[1] = 0x02;
        assert_eq!(
            parse_hidpp_battery(&capabilities, &status),
            Some((0, Some(false)))
        );
        status[0] = 101;
        assert_eq!(parse_hidpp_battery(&capabilities, &status), None);
        status[0] = 84;
        status[2] = 1;
        assert_eq!(
            parse_hidpp_battery(&capabilities, &status),
            Some((84, Some(true)))
        );
        status[2] = 7;
        assert_eq!(
            parse_hidpp_battery(&capabilities, &status),
            Some((84, None))
        );
    }
}
