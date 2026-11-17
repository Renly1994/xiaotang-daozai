//! 蓝牙电量角标：在展开面板顶栏右上角常驻显示已连接蓝牙设备的电量。
//!
//! 数据来自 `bt::Service`（只读系统蓝牙设备属性），不建立连接、不联网。
//! 开关 `show_bt_battery` 决定角标是否显示；扫描调度在 main.rs 中按
//! 「连接提醒」或「本角标」任一开启时运行。

use crate::app::Env;
use crate::paint::{Card, H};
use crate::theme::{self, W};
use eframe::egui::Color32;

/// 角标右边界：设置齿轮中心在 x=394、半径 13（左缘 381），留出余量。
const RIGHT: f32 = 372.0;
const CENTER_Y: f32 = 18.0;

/// 电量配色：低电量红、偏低黄、其余绿，与系统 HUD 一致。
pub(crate) fn battery_color(level: u8) -> Color32 {
    if level <= 20 { theme::RED } else if level <= 50 { theme::YELLOW } else { theme::GREEN }
}

/// 从已连接设备里挑一个最值得展示的：有电量的音频设备优先，其次任意有电量的设备。
pub(crate) fn pick_device<'a>(devices: &'a [crate::bt::Device]) -> Option<&'a crate::bt::Device> {
    use crate::bt::DeviceKind::*;
    fn audio(kind: crate::bt::DeviceKind) -> bool {
        matches!(kind, Earbuds | Headphones | Bone | Speaker)
    }
    devices.iter().find(|d| d.battery.is_some() && audio(d.kind))
        .or_else(|| devices.iter().find(|d| d.battery.is_some()))
}

impl Card<'_> {
    /// 在顶栏右上角画蓝牙电量角标：设备图标 + 电量条 + 百分比。
    /// 返回是否实际绘制（供测试与命中判断）。
    pub(crate) fn bt_battery_badge(&self, env: &Env) -> bool {
        let Some(device) = pick_device(&env.bt) else { return false };
        let Some(level) = device.battery else { return false };
        let color = battery_color(level);

        let pct = format!("{level}%");
        let pct_w = self.text_w(&pct, 10.5, W::Semibold);
        // 布局从右往左：[百分比][电量条][图标]
        let pct_x = RIGHT - pct_w;
        let bar_w = 22.0;
        let bar_x = pct_x - bar_w - 6.0;
        let icon_cx = bar_x - 12.0;

        self.circle(icon_cx, CENTER_Y, 10.0, Color32::from_gray(38));
        crate::extras::device_icon(self, device.kind, icon_cx - 7.5, CENTER_Y - 7.5, 15.0);

        self.fill(bar_x, CENTER_Y - 3.0, bar_w, 6.0, 3.0, Color32::from_gray(60));
        let filled = bar_w * level as f32 / 100.0;
        if filled > 0.0 { self.fill(bar_x, CENTER_Y - 3.0, filled, 6.0, 3.0, color); }

        self.text(RIGHT, CENTER_Y + 3.5, &pct, 10.5, W::Semibold, color, H::Right);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bt::{Device, DeviceKind};

    fn device(name: &str, kind: DeviceKind, battery: Option<u8>) -> Device {
        Device { id: name.into(), name: name.into(), battery, kind }
    }

    #[test]
    fn battery_color_marks_low_levels() {
        assert_eq!(battery_color(5), theme::RED);
        assert_eq!(battery_color(20), theme::RED);
        assert_eq!(battery_color(21), theme::YELLOW);
        assert_eq!(battery_color(50), theme::YELLOW);
        assert_eq!(battery_color(51), theme::GREEN);
        assert_eq!(battery_color(100), theme::GREEN);
    }

    #[test]
    fn picks_audio_device_with_battery_first() {
        let devices = [
            device("键盘", DeviceKind::Keyboard, Some(90)),
            device("AirPods", DeviceKind::Earbuds, Some(80)),
        ];
        assert_eq!(pick_device(&devices).unwrap().name, "AirPods");
    }

    #[test]
    fn falls_back_to_any_device_with_battery() {
        let devices = [
            device("键盘", DeviceKind::Keyboard, Some(90)),
            device("耳机", DeviceKind::Headphones, None),
        ];
        assert_eq!(pick_device(&devices).unwrap().name, "键盘");
        assert!(pick_device(&[]).is_none());
        assert!(pick_device(&[device("耳机", DeviceKind::Headphones, None)]).is_none());
    }
}
