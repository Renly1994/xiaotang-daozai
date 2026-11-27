//! CompX 2.4G 无线键盘接收器（3554:FA09，如前行者 GS98 / 狼蛛 F75 等）的只读电量查询。
//!
//! 20 字节 0x13 帧格式参考 aula-f75-hub（MIT），并由 Windows 11 实机抓包验证。
//! https://github.com/bakiakyol/aula-f75-hub/blob/main/docs/protocol.md
//!
//! 完整许可声明见 THIRD_PARTY_NOTICES.txt。

use super::super::usb_hid::{self, Handle};
use std::time::{Duration, Instant};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

const REPORT_LEN: usize = 20;
const REPORT_ID: u8 = 0x13;
const CMD_ONLINE: u8 = 0x07;
const CMD_BATTERY: u8 = 0x4a;
const ONLINE_TIMEOUT: Duration = Duration::from_millis(400);
const BATTERY_TIMEOUT: Duration = Duration::from_millis(800);

pub(super) fn read(instance: &str) -> Option<(u8, Option<bool>)> {
    let mut matches = usb_hid::collections(instance).into_iter().filter(|c| {
        (c.usage_page, c.usage) == (0xff02, 0x0002)
            && c.input_len == REPORT_LEN
            && c.output_len == REPORT_LEN
    });
    let collection = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let mut handle = collection.open()?;
    if query_online(&mut handle) != Some(true) {
        return None;
    }
    // 固件在响应 0x4A 配置查询期间可能丢失同时刻的按键抬起包，按键按下时跳过本轮读取。
    if any_keyboard_key_pressed() {
        return None;
    }
    query_battery(&mut handle)
}

fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |sum, &byte| sum.wrapping_add(byte))
}

fn request(command: u8) -> [u8; REPORT_LEN] {
    let mut frame = [0u8; REPORT_LEN];
    frame[0] = REPORT_ID;
    frame[1] = command;
    frame[2] = 0x01;
    frame[19] = checksum(&frame[..19]);
    frame
}

fn valid_frame(report: &[u8], command: u8) -> Option<&[u8]> {
    let frame = report.get(..REPORT_LEN)?;
    if report[REPORT_LEN..].iter().any(|&byte| byte != 0) {
        return None;
    }
    if frame[0] != REPORT_ID
        || (frame[1] & 0x7f) != command
        || (frame[2] & 0x80) != 0
        || (frame[4] & 0x0f) == 0
        || frame[19] != checksum(&frame[..19])
    {
        return None;
    }
    Some(frame)
}

fn parse_online(report: &[u8]) -> Option<bool> {
    let frame = valid_frame(report, CMD_ONLINE)?;
    Some(frame[5] != 0)
}

fn parse_battery(report: &[u8]) -> Option<(u8, Option<bool>)> {
    let frame = valid_frame(report, CMD_BATTERY)?;
    let payload_len = frame[4] & 0x0f;
    let level = frame[5];
    if !(1..=100).contains(&level) {
        return None;
    }
    let charging = if payload_len >= 2 {
        match frame[6] {
            0x01 => Some(false),
            0x02 | 0x10 => Some(true),
            _ => None,
        }
    } else {
        None
    };
    Some((level, charging))
}

fn query_online(handle: &mut Handle) -> Option<bool> {
    let deadline = Instant::now() + ONLINE_TIMEOUT;
    if !handle.write(&request(CMD_ONLINE), deadline) {
        return None;
    }
    while Instant::now() < deadline {
        if let Some(online) = parse_online(&handle.read(deadline)?) {
            return Some(online);
        }
    }
    None
}

fn query_battery(handle: &mut Handle) -> Option<(u8, Option<bool>)> {
    let deadline = Instant::now() + BATTERY_TIMEOUT;
    if !handle.write(&request(CMD_BATTERY), deadline) {
        return None;
    }
    while Instant::now() < deadline {
        if let Some(reading) = parse_battery(&handle.read(deadline)?) {
            return Some(reading);
        }
    }
    None
}

fn any_keyboard_key_pressed() -> bool {
    for vk in 0x08..=0xfe {
        let state = unsafe { GetAsyncKeyState(vk) } as u16;
        if (state & 0x8000) != 0 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compx_keyboard_formats_requests_and_parses_captured_responses() {
        let req_online = request(CMD_ONLINE);
        assert_eq!(req_online[0], 0x13);
        assert_eq!(req_online[1], 0x07);
        assert_eq!(req_online[2], 0x01);
        assert_eq!(req_online[19], 0x1b);

        let req_batt = request(CMD_BATTERY);
        assert_eq!(req_batt[1], 0x4a);
        assert_eq!(req_batt[19], 0x5e);

        // Windows 11 实测前行者 GS98 抓包样本。
        let online = [
            0x13, 0x07, 0x01, 0x00, 0x01, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x1d,
        ];
        let offline = [
            0x13, 0x07, 0x01, 0x00, 0x01, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x1c,
        ];
        let batt_95 = [
            0x13, 0x4a, 0x01, 0x00, 0x02, 0x5f, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xc0,
        ];
        let charging_100 = [
            0x13, 0x4a, 0x01, 0x00, 0x02, 0x64, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xd4,
        ];

        assert_eq!(parse_online(&online), Some(true));
        assert_eq!(parse_online(&offline), Some(false));
        assert_eq!(parse_battery(&batt_95), Some((95, Some(false))));
        assert_eq!(parse_battery(&charging_100), Some((100, Some(true))));

        let mut bad_crc = batt_95;
        bad_crc[19] ^= 0x01;
        assert_eq!(parse_battery(&bad_crc), None);

        let mut err_bit = batt_95;
        err_bit[2] = 0x81;
        err_bit[19] = checksum(&err_bit[..19]);
        assert_eq!(parse_battery(&err_bit), None);

        let mut zero_level = batt_95;
        zero_level[5] = 0;
        zero_level[19] = checksum(&zero_level[..19]);
        assert_eq!(parse_battery(&zero_level), None);

        let mut over_level = batt_95;
        over_level[5] = 101;
        over_level[19] = checksum(&over_level[..19]);
        assert_eq!(parse_battery(&over_level), None);

        let push_0a = [
            0x13, 0x0a, 0x01, 0x00, 0x02, 0x02, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x23,
        ];
        assert_eq!(parse_online(&push_0a), None);
        assert_eq!(parse_battery(&push_0a), None);

        let psd_05 = [
            0x13, 0x05, 0x01, 0x00, 0x0a, 0x03, 0, 0, 0, 0x01, 0xf4, 0x01, 0, 0, 0, 0, 0, 0, 0,
            0x1c,
        ];
        assert_eq!(parse_battery(&psd_05), None);
    }
}
