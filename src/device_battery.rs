//! 【新增】将三类电量源接入同一视图；显示偏好与采集、通知分别管理。
use crate::{
    app::{App, Env},
    bt::DeviceKind,
    model::Settings,
    paint::{Card, H},
    theme::{self, W},
};
use eframe::egui::{self, Color32};

pub(crate) const PAGE: usize = 8;
pub(crate) const HEIGHT: f32 = 232.0;
const ROWS: usize = 4;

#[derive(Clone, Debug)]
pub(crate) struct Device {
    pub id: String,
    pub name: String,
    pub source: &'static str,
    pub kind: DeviceKind,
    pub percent: Option<u8>,
    pub charging: bool,
    pub status: &'static str,
}
impl Device {
    pub fn name<'a>(&'a self, settings: &'a Settings) -> &'a str {
        settings
            .battery_devices
            .get(&self.id)
            .map(|(name, _)| name.trim())
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.name)
    }
    pub fn visible(&self, settings: &Settings) -> bool {
        settings.battery_devices.get(&self.id).map_or(
            self.id != "host" || self.percent.is_some(),
            |(_, visible)| *visible,
        )
    }
    pub fn level(&self) -> String {
        self.percent
            .map_or_else(|| self.status.into(), |p| format!("{p}%"))
    }
}

#[derive(Default)]
pub(crate) struct State {
    pub devices: Vec<Device>,
    pub first: usize,
}
impl State {
    /// 【新增】每帧更新读数；过期、停采、休眠和断开均不把旧电量当作当前值。
    pub fn sync(&mut self, env: &Env, settings: &Settings) {
        let mut devices = vec![Device {
            id: "host".into(),
            name: "本机".into(),
            source: "本机",
            kind: DeviceKind::Other,
            percent: env.battery.filter(|p| *p <= 100),
            charging: env.charging,
            status: "未提供电池信息",
        }];
        for d in &env.bt {
            devices.push(Device {
                id: format!("bt:{}", d.id),
                name: d.name.clone(),
                source: "蓝牙",
                kind: d.kind,
                percent: d.battery.filter(|p| *p <= 100),
                charging: false,
                status: "电量未知",
            });
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        for d in &env.hid.devices {
            let stale = env.hid.device_stale_at(&d.id, now);
            // 【新增】未识别的普通 HID 接口留在反馈详情，避免键鼠的每个集合都变成电池。
            if d.protocol == "unknown"
                && d.battery.is_none()
                && !d
                    .descriptor
                    .fields
                    .iter()
                    .any(|f| f.battery_usage().is_some())
            {
                continue;
            }
            let percent = d
                .battery
                .filter(|_| settings.hid_battery && !stale && d.failure.is_none())
                .map(|b| b.percent)
                .filter(|p| *p <= 100);
            let kind = match (d.descriptor.usage_page, d.descriptor.usage) {
                (1, 2) => DeviceKind::Mouse,
                (1, 6) => DeviceKind::Keyboard,
                (1, 4 | 5) => DeviceKind::Gamepad,
                _ if d.protocol == "compx-17" => DeviceKind::Mouse,
                _ => DeviceKind::Other,
            };
            let name = d
                .slot
                .map_or_else(|| d.name.clone(), |slot| format!("{} · {slot}", d.name));
            devices.push(Device {
                id: format!("hid:{}", d.id),
                name,
                source: d.transport,
                kind,
                percent,
                charging: percent.is_some() && d.battery.is_some_and(|b| b.charging == Some(true)),
                status: if !settings.hid_battery {
                    "采集已关闭"
                } else if stale {
                    "读数已过期"
                } else {
                    d.failure.map_or("电量未知", crate::hid::Failure::label)
                },
            });
        }
        self.devices = devices;
        self.first = self
            .first
            .min(self.visible_count(settings).saturating_sub(ROWS));
    }
    pub fn selected(&self, settings: &Settings) -> Option<&Device> {
        if settings.battery_device.is_empty() {
            // 【修改】自动选择优先本机，其次蓝牙和 HID；台式机也能直接显示外设电量。
            self.devices.iter().find(|d| d.percent.is_some())
        } else {
            // 固定选择断开时隐藏指示，不偷偷改为另一台设备。
            self.devices
                .iter()
                .find(|d| d.id == settings.battery_device)
        }
    }
    pub fn power(&self, settings: &Settings) -> (Option<u8>, bool) {
        self.selected(settings)
            .map_or((None, false), |d| (d.percent, d.charging))
    }
    pub fn visible_count(&self, settings: &Settings) -> usize {
        self.devices.iter().filter(|d| d.visible(settings)).count()
    }
    pub fn ui(&self, ui: &mut egui::Ui, settings: &mut Settings) {
        crate::extras::setting_row(
            ui,
            "岛上电量",
            "与「页面管理 → 电量」同步，控制岛上的电量显示。",
            &mut settings.show_battery,
        );
        // 【新增】开启常驻时同时开启总开关；关闭总开关仍可暂时隐藏所有电量显示。
        let was_pinned = settings.battery_pinned;
        crate::extras::setting_row(
            ui,
            "常驻岛上",
            "无设备或电量未知时，也保留一个灰色电量环。",
            &mut settings.battery_pinned,
        );
        if settings.battery_pinned && !was_pinned {
            settings.show_battery = true;
        }
        ui.add_space(5.0);
        let selected = if settings.battery_device.is_empty() {
            "自动选择（本机优先）".into()
        } else {
            self.selected(settings)
                .map(|d| format!("{} · {}", d.name(settings), d.source))
                .unwrap_or_else(|| {
                    format!(
                        "{} · 已断开",
                        settings
                            .battery_devices
                            .get(&settings.battery_device)
                            .map(|(name, _)| name.as_str())
                            .filter(|n| !n.is_empty())
                            .unwrap_or("所选设备")
                    )
                })
        };
        ui.horizontal_wrapped(|ui| {
            ui.label("岛上显示");
            egui::ComboBox::from_id_salt("island-battery-device")
                .selected_text(selected)
                .truncate()
                .width(270.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut settings.battery_device,
                        String::new(),
                        "自动选择（本机优先）",
                    );
                    for d in &self.devices {
                        let label = format!("{} · {} · {}", d.name(settings), d.source, d.level());
                        ui.selectable_value(&mut settings.battery_device, d.id.clone(), label);
                    }
                });
        });
        ui.separator();
        ui.label(egui::RichText::new("设备电量页面").strong());
        ui.small("勾选要在岛内列表显示的设备；修改名称后自动保存，留空恢复原名。无电池的本机默认不显示。 ");
        for d in &self.devices {
            ui.push_id(&d.id, |ui| {
                let mut visible = d.visible(settings);
                let mut name = settings
                    .battery_devices
                    .get(&d.id)
                    .map(|(n, _)| n.clone())
                    .unwrap_or_default();
                let mut changed = false;
                ui.horizontal(|ui| {
                    changed |= ui
                        .checkbox(&mut visible, "")
                        .on_hover_text("在设备电量页面显示")
                        .changed();
                    ui.vertical(|ui| {
                        changed |= ui
                            .add(
                                egui::TextEdit::singleline(&mut name)
                                    .hint_text(&d.name)
                                    .desired_width(210.0)
                                    .char_limit(48),
                            )
                            .changed();
                        ui.horizontal_wrapped(|ui| {
                            ui.small(format!("{} · {}", d.source, d.level()));
                            // 【新增】2.4G 的采样间隔单独配置；不改变其他 HID / 蓝牙 / 本机的轮询。
                            if d.source == "2.4G" {
                                refresh_interval_ui(ui, settings, &d.id);
                            }
                        });
                    });
                });
                if changed {
                    // 【新增】配置使用 JSON 转义名称；反馈生成器仍只接收 HID 白名单数据。
                    settings
                        .battery_devices
                        .insert(d.id.clone(), (name, visible));
                }
            });
            ui.add_space(5.0);
        }
        // 保存过的偏好在断开期间也可调整，重连不会丢失别名或开关。
        let offline: Vec<_> = settings
            .battery_devices
            .keys()
            .filter(|id| !self.devices.iter().any(|d| &d.id == *id))
            .cloned()
            .collect();
        if !offline.is_empty() {
            egui::CollapsingHeader::new("已断开设备的偏好").show(ui, |ui| {
                for id in offline {
                    ui.push_id(&id, |ui| {
                        let (name, visible) = settings.battery_devices.get_mut(&id).unwrap();
                        ui.horizontal(|ui| {
                            ui.checkbox(visible, "");
                            ui.add(
                                egui::TextEdit::singleline(name)
                                    .hint_text("已断开的设备")
                                    .desired_width(200.0)
                                    .char_limit(48),
                            );
                        });
                    });
                }
            });
        }
    }
}

fn refresh_interval_ui(ui: &mut egui::Ui, settings: &mut Settings, id: &str) {
    const PRESETS: [u32; 6] = [5, 10, 15, 30, 60, 120];
    let mut seconds = settings
        .hid_refresh_seconds
        .get(id)
        .copied()
        .unwrap_or(crate::hid::DEFAULT_REFRESH_SECS);
    let before = seconds;
    let custom_id = ui.id().with("hid-custom-interval");
    let mut custom = ui.ctx().data(|d| {
        d.get_temp::<bool>(custom_id)
            .unwrap_or(!PRESETS.contains(&seconds))
    });
    ui.small("刷新");
    egui::ComboBox::from_id_salt("hid-refresh-interval")
        .selected_text(if custom {
            "自定义".into()
        } else {
            format!("{seconds}s")
        })
        .width(72.0)
        .show_ui(ui, |ui| {
            for value in PRESETS {
                if ui
                    .selectable_label(!custom && seconds == value, format!("{value}s"))
                    .clicked()
                {
                    seconds = value;
                    custom = false;
                }
            }
            if ui.selectable_label(custom, "自定义").clicked() {
                custom = true;
            }
        });
    ui.ctx().data_mut(|d| d.insert_temp(custom_id, custom));
    if custom {
        ui.add(
            egui::DragValue::new(&mut seconds)
                .range(1..=crate::hid::MAX_REFRESH_SECS)
                .suffix("s")
                .speed(1.0),
        )
        .on_hover_text("输入 1–3600 秒，或拖动数值调整");
    }
    if seconds != before {
        settings
            .hid_refresh_seconds
            .insert(id.into(), crate::hid::refresh_seconds(seconds));
    }
}

impl App {
    /// 【新增】普通顶栏、歌词、侧边停靠和排列预览共享同一可见性判断。
    pub(crate) fn island_battery_visible(&self) -> bool {
        self.settings.show_battery
            && (self.settings.battery_pinned || self.resident_power.0.is_some())
    }
    /// 【新增】iOS 风格的多设备列表；四行可见，更多设备在内容区滚动。
    pub(crate) fn page_device_battery(&mut self, c: &Card) {
        let devices: Vec<_> = self
            .device_battery
            .devices
            .iter()
            .filter(|d| d.visible(&self.settings))
            .collect();
        if devices.is_empty() {
            c.centered_text(
                209.5,
                100.0,
                "暂无显示的设备",
                15.0,
                W::Bold,
                theme::WHITE,
                H::Center,
            );
            c.centered_text(
                209.5,
                126.0,
                "在设备与电量管理中选择设备",
                11.0,
                W::Regular,
                theme::GRAY,
                H::Center,
            );
            return;
        }
        let first = self
            .device_battery
            .first
            .min(devices.len().saturating_sub(ROWS));
        let list = c.clipped(39.0, 38.0, 340.0, 178.0);
        for (i, d) in devices.iter().skip(first).take(ROWS).enumerate() {
            let cy = 60.0 + i as f32 * 44.0;
            if d.id == "host" {
                list.stroke(48.0, cy - 8.0, 19.0, 13.0, 2.0, 1.3, theme::WHITE);
                list.seg((45.0, cy + 7.0), (70.0, cy + 7.0), 1.6, theme::WHITE);
            } else {
                crate::extras::device_icon(&list, d.kind, 47.0, cy - 10.0, 21.0);
            }
            list.centered_text(
                81.0,
                cy - 5.0,
                &list.fit(d.name(&self.settings), 12.0, W::Regular, 207.0),
                12.0,
                W::Regular,
                theme::WHITE,
                H::Left,
            );
            let subtitle = if d.percent.is_none() {
                d.status
            } else if d.charging {
                "正在充电"
            } else {
                d.source
            };
            list.centered_text(
                81.0,
                cy + 11.0,
                &list.fit(subtitle, 9.0, W::Regular, 207.0),
                9.0,
                W::Regular,
                theme::GRAY,
                H::Left,
            );
            list.centered_text(
                330.0,
                cy,
                &d.percent.map_or("—".into(), |p| format!("{p}%")),
                11.0,
                W::Regular,
                theme::WHITE,
                H::Right,
            );
            battery_glyph(&list, 341.0, cy, d.percent, d.charging);
            if i < ROWS - 1 && first + i + 1 < devices.len() {
                list.seg(
                    (81.0, cy + 22.0),
                    (369.0, cy + 22.0),
                    0.6,
                    Color32::from_white_alpha(26),
                );
            }
        }
        if devices.len() > ROWS {
            let track = 174.0;
            let thumb = track * ROWS as f32 / devices.len() as f32;
            let y = 40.0 + (track - thumb) * first as f32 / (devices.len() - ROWS) as f32;
            c.fill(375.0, y, 2.0, thumb, 1.0, Color32::from_white_alpha(80));
        }
    }
}

fn battery_glyph(c: &Card, x: f32, cy: f32, percent: Option<u8>, charging: bool) {
    let col = if percent.is_some_and(|p| p <= 20) && !charging {
        theme::RED_TOP
    } else {
        theme::GREEN
    };
    c.stroke(
        x,
        cy - 4.5,
        22.0,
        9.0,
        2.0,
        0.8,
        Color32::from_white_alpha(140),
    );
    c.fill(
        x + 23.0,
        cy - 1.8,
        1.5,
        3.6,
        0.7,
        Color32::from_white_alpha(140),
    );
    if let Some(p) = percent {
        if p > 0 {
            c.fill(x + 1.6, cy - 2.9, 18.8 * p as f32 / 100.0, 5.8, 1.0, col);
        }
        if charging {
            // 【新增】矢量闪电限制在电池内部，不依赖字体的特殊符号字形。
            c.poly(
                &[
                    (x + 12.0, cy - 3.7),
                    (x + 8.7, cy + 0.6),
                    (x + 11.0, cy + 0.6),
                    (x + 10.0, cy + 3.7),
                    (x + 14.0, cy - 0.8),
                    (x + 11.6, cy - 0.8),
                ],
                theme::WHITE,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auto_uses_external_and_fixed_choice_never_switches_on_disconnect() {
        let mut state = State::default();
        let mut settings = Settings::default();
        let mut env = Env {
            bt: vec![crate::bt::Device {
                id: "a".into(),
                name: "耳机".into(),
                kind: DeviceKind::Headphones,
                battery: Some(71),
            }],
            ..Default::default()
        };
        state.sync(&env, &settings);
        assert_eq!(state.power(&settings), (Some(71), false));
        settings.battery_device = "bt:a".into();
        env.battery = Some(90);
        state.sync(&env, &settings);
        assert_eq!(state.power(&settings).0, Some(71));
        env.bt.clear();
        state.sync(&env, &settings);
        assert_eq!(state.power(&settings).0, None);
    }
    #[test]
    fn hid_stale_disabled_and_offline_are_not_current_readings() {
        let mut settings = Settings::default();
        let mut state = State::default();
        let d = crate::hid::Device {
            id: "a".into(),
            name: "鼠标".into(),
            descriptor: Default::default(),
            protocol: "compx-17",
            transport: "2.4G",
            slot: None,
            battery: Some(crate::hid::Battery {
                percent: 20,
                charging: Some(true),
                millivolts: None,
            }),
            failure: None,
        };
        let mut snapshot = crate::hid::Snapshot {
            devices: vec![d],
            generation: 1,
            ..Default::default()
        };
        let mut env = Env::default();
        env.hid = std::sync::Arc::new(snapshot.clone());
        state.sync(&env, &settings);
        assert_eq!(state.power(&settings), (Some(20), true));
        settings.hid_battery = false;
        state.sync(&env, &settings);
        assert_eq!(state.power(&settings).0, None);
        settings.hid_battery = true;
        snapshot.collected_at = 1;
        env.hid = std::sync::Arc::new(snapshot.clone());
        state.sync(&env, &settings);
        assert_eq!(state.power(&settings).0, None);
        snapshot.collected_at = 0;
        snapshot.devices[0].failure = Some(crate::hid::Failure::Offline);
        env.hid = std::sync::Arc::new(snapshot);
        state.sync(&env, &settings);
        assert_eq!(state.power(&settings).0, None);
    }
}
