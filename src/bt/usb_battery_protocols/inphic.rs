//! 英菲克 IN10（2.4G 8K 接收器）的只读被动电量监听。
//!
//! 帧格式和设备类型码参考 InphicBatteryTray（MIT），并由 Windows 11 实机抓包验证。
//! https://github.com/Tan-TanZi/InphicBatteryTray
//!
//! 完整许可声明见 THIRD_PARTY_NOTICES.txt。

use super::super::usb_hid;
use std::time::{Duration, Instant};

const REPORT_LEN: usize = 5;
const REPORT_ID: u8 = 0x03;
// 鼠标在线时约每 2 秒主动广播一次电量，3 秒窗口可稳定捕获一帧。
const PASSIVE_TIMEOUT: Duration = Duration::from_millis(3_000);

pub(super) fn read_in10(instance: &str) -> Option<(u8, Option<bool>)> {
    let mut matches = usb_hid::collections(instance).into_iter().filter(|c| {
        (c.interface, c.usage_page, c.usage) == (Some(2), 0x000a, 0x0000)
            && c.input_len == REPORT_LEN
    });
    let collection = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let mut handle = collection.open()?;
    let deadline = Instant::now() + PASSIVE_TIMEOUT;
    while Instant::now() < deadline {
        let reply = handle.read(deadline)?;
        if let Some(reading) = parse_in10(&reply) {
            return Some(reading);
        }
    }
    None
}

fn parse_in10(report: &[u8]) -> Option<(u8, Option<bool>)> {
    if report.len() != REPORT_LEN {
        return None;
    }
    match report {
        [REPORT_ID, 0x73 | 0x90 | 0x93 | 0x95 | 0x98 | 0x99, 0x40 | 0x41, charge, level @ 0..=100] =>
        {
            let charging = match *charge {
                1 => Some(false),
                2 | 3 => Some(true),
                _ => None,
            };
            Some((*level, charging))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inphic_in10_parses_captured_reports_and_rejects_noise() {
        // Windows 11 实测 1D57:FA65 抓包样本（50%，未充电）。
        assert_eq!(
            parse_in10(&[0x03, 0x93, 0x40, 0x01, 0x32]),
            Some((50, Some(false)))
        );
        // 充电与不同设备类型码。
        assert_eq!(
            parse_in10(&[0x03, 0x93, 0x41, 0x02, 0x64]),
            Some((100, Some(true)))
        );
        assert_eq!(
            parse_in10(&[0x03, 0x98, 0x40, 0x03, 0x50]),
            Some((80, Some(true)))
        );
        assert_eq!(parse_in10(&[0x03, 0x95, 0x40, 0x00, 0x00]), Some((0, None)));

        assert_eq!(parse_in10(&[0x04, 0x93, 0x40, 0x01, 0x32]), None);
        assert_eq!(parse_in10(&[0x03, 0x80, 0x40, 0x01, 0x32]), None);
        assert_eq!(parse_in10(&[0x03, 0x93, 0x20, 0x01, 0x32]), None);
        assert_eq!(parse_in10(&[0x03, 0x93, 0x40, 0x01, 0x65]), None);
        assert_eq!(parse_in10(&[0x03, 0x93, 0x40, 0x01]), None);
        assert_eq!(parse_in10(&[0x03, 0x93, 0x40, 0x01, 0x32, 0x00]), None);
    }
}
