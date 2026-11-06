//! 【新增】设备管理页的电量诊断与本地反馈；未知设备也能导出接口结构供后续适配。
use crate::{
    app::{Cmd, Env},
    hid,
};
use eframe::egui;
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct State {
    snapshot: Arc<hid::Snapshot>,
    host: (Option<u8>, bool),
    bluetooth: Vec<crate::bt::Device>,
    report: String,
    note: String,
    last_request: Option<f64>,
    now: f64,
}
impl State {
    pub fn sync(&mut self, env: &Env) {
        self.snapshot = env.hid.clone();
        self.host = (env.battery, env.charging);
        self.bluetooth = env.bt.clone();
        self.now = env.t;
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, enabled: &mut bool, pending: &mut Vec<Cmd>) {
        ui.label(egui::RichText::new("设备电量与 HID 反馈").strong());
        ui.label("查看本机、蓝牙及 HID 外设。读不到电量时，可生成兼容性报告帮助维护者适配。");
        ui.checkbox(enabled, "启用 HID 外设电量采集");
        ui.small("2.4G 默认 5s，可在上方逐设备调整；其他 HID 每分钟刷新。未知设备仅采集接口结构。");
        ui.add_space(5.0);
        let host = self
            .host
            .0
            .map(|v| {
                format!(
                    "{v}% · {}",
                    if self.host.1 {
                        "外部电源"
                    } else {
                        "电池供电"
                    }
                )
            })
            .unwrap_or_else(|| "未提供电池信息".into());
        ui.label(format!("本机：{host}"));
        for device in &self.bluetooth {
            ui.label(format!(
                "蓝牙 · {}：{}",
                device.name,
                device
                    .battery
                    .map(|b| format!("{b}%"))
                    .unwrap_or_else(|| "电量未知".into())
            ));
        }
        ui.add_space(4.0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let stale = self.snapshot.stale_at(now);
        if let Some(at) = chrono::DateTime::from_timestamp(self.snapshot.collected_at as i64, 0)
            .filter(|_| self.snapshot.collected_at > 0)
        {
            ui.small(format!(
                "最近采集：{}{}",
                at.with_timezone(&chrono::Local).format("%H:%M:%S"),
                if stale { " · 已过期" } else { "" }
            ));
        }
        if self.snapshot.generation == 0 {
            ui.small(if *enabled {
                "正在获取 HID 接口…"
            } else {
                "采集已关闭，可点击刷新查看接口信息。"
            });
        }
        if let Some(error) = self.snapshot.error {
            ui.small(error.label());
        }
        egui::ScrollArea::vertical()
            .id_salt("hid-device-list")
            .max_height(180.0)
            .show(ui, |ui| {
                for device in &self.snapshot.devices {
                    let status = if self.snapshot.device_stale_at(&device.id, now) {
                        "读数已过期，请刷新".into()
                    } else {
                        match device.battery {
                            Some(b) => format!(
                                "{}%{}",
                                b.percent,
                                if b.charging == Some(true) {
                                    " · 充电标志"
                                } else {
                                    ""
                                }
                            ),
                            None => device
                                .failure
                                .map_or("电量未知", hid::Failure::label)
                                .into(),
                        }
                    };
                    let slot = device
                        .slot
                        .map(|s| format!(" · 槽位 {s}"))
                        .unwrap_or_default();
                    ui.label(format!("{}{}：{}", device.name, slot, status));
                    ui.small(format!(
                        "{} · {:04X}:{:04X} · {}",
                        device.transport,
                        device.descriptor.vid,
                        device.descriptor.pid,
                        device.protocol
                    ));
                    ui.separator();
                }
            });
        let ready = self.last_request.is_none_or(|at| self.now - at >= 5.0);
        ui.horizontal_wrapped(|ui|{
            if ui.add_enabled(ready,egui::Button::new("刷新设备")).clicked(){pending.push(Cmd::HidRefresh);self.last_request=Some(self.now);self.note="已请求刷新，结果会自动更新。".into();}
            if ui.add_enabled(self.snapshot.generation>0,egui::Button::new("生成反馈报告")).clicked(){
                // 【新增】对当前快照按白名单生成报告；只有用户点击复制/导出才离开内存。
                self.report=hid::feedback(&self.snapshot);self.note="报告已生成。请先预览，再复制或导出，并在反馈时补充设备型号、连接方式和问题现象。".into();
            }
        });
        ui.small("报告不包含设备名称、序列号、设备路径、MAC 或键鼠输入，不会自动上传。");
        if !self.report.is_empty() {
            egui::CollapsingHeader::new("报告预览")
                .id_salt("hid-feedback-preview")
                .show(ui, |ui| {
                    // 长报告只预览前 32 KiB，避免文本布局拖慢设置页；导出保留完整内容。
                    let end = self
                        .report
                        .char_indices()
                        .map(|(i, _)| i)
                        .take_while(|i| *i <= 32768)
                        .last()
                        .unwrap_or(0);
                    let mut preview = if self.report.len() > 32768 {
                        &self.report[..end]
                    } else {
                        self.report.as_str()
                    };
                    egui::ScrollArea::vertical()
                        .id_salt("hid-report-scroll")
                        .max_height(200.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut preview)
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(f32::INFINITY)
                                    .interactive(false),
                            );
                        });
                    if self.report.len() > 32768 {
                        ui.small("预览已截断，导出 JSON 可查看完整报告。");
                    }
                });
            ui.horizontal_wrapped(|ui| {
                if ui.button("复制报告").clicked() {
                    pending.push(Cmd::Copy(self.report.clone()));
                }
                if ui.button("导出 JSON").clicked() {
                    match hid::export_report(&self.report) {
                        Ok(path) => {
                            self.note =
                                "已导出到本地 diagnostics 目录，可将 JSON 附加到反馈。".into();
                            pending.push(Cmd::Reveal(path));
                        }
                        Err(_) => {
                            self.note = "导出失败，请检查本地数据目录权限，或使用复制报告。".into()
                        }
                    }
                }
                if ui.button("打开反馈页面").clicked() {
                    pending.push(Cmd::OpenUrl(feedback_url()));
                }
            });
        }
        if !self.note.is_empty() {
            ui.add_space(4.0);
            ui.label(&self.note);
        }
    }
}

fn feedback_url() -> String {
    let mut url =
        url::Url::parse("https://github.com/Renly1994/xiaotang-daozai/issues/new").unwrap();
    url.query_pairs_mut().append_pair("title","[HID 电量兼容] 请填写设备型号")
        .append_pair("body","设备品牌及型号：\n连接方式（2.4G / USB / 蓝牙）：\n固件版本及厂商软件：\n厂商软件显示的电量：\n问题现象及复现步骤：\n\n请附加“设置 → 设备与电量管理 → 设备与兼容性反馈”导出的 JSON；提交前请检查内容。\n");
    url.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn render(
        state: &mut State,
        enabled: &mut bool,
        pending: &mut Vec<Cmd>,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        // 用同一个上下文让按钮的按下/释放状态跨帧保存。
        UI_CONTEXT.with(|ctx| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(680.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| state.ui(ui, enabled, pending));
                },
            )
        })
    }
    thread_local! {static UI_CONTEXT:egui::Context=egui::Context::default();}
    fn click(state: &mut State, enabled: &mut bool, pending: &mut Vec<Cmd>, label: &str) {
        let output = render(state, enabled, pending, vec![]);
        let at = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == label => {
                    Some(s.shape.visual_bounding_rect().center())
                }
                _ => None,
            })
            .expect("反馈按钮应可见");
        for pressed in [true, false] {
            render(
                state,
                enabled,
                pending,
                vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
    }
    #[test]
    fn unsupported_device_can_generate_and_copy_feedback_without_bluetooth() {
        let device = hid::Device {
            id: "private-device-path".into(),
            name: "私人设备名称".into(),
            descriptor: hid::Descriptor {
                vid: 0x1234,
                pid: 0x5678,
                ..Default::default()
            },
            protocol: "unknown",
            transport: "HID",
            slot: None,
            battery: None,
            failure: Some(hid::Failure::Unsupported),
        };
        let mut state = State::default();
        state.sync(&Env {
            battery: Some(76),
            hid: Arc::new(hid::Snapshot {
                devices: vec![device],
                generation: 1,
                ..Default::default()
            }),
            ..Default::default()
        });
        let mut enabled = false;
        let mut pending = vec![];
        click(&mut state, &mut enabled, &mut pending, "生成反馈报告");
        assert!(!state.report.is_empty());
        click(&mut state, &mut enabled, &mut pending, "复制报告");
        let copied = pending
            .iter()
            .find_map(|c| {
                if let Cmd::Copy(text) = c {
                    Some(text)
                } else {
                    None
                }
            })
            .unwrap();
        assert!(copied.contains("1234"));
        assert!(copied.contains("unsupported"));
        assert!(!copied.contains("私人"));
        assert!(!copied.contains("private-device-path"));
    }
    #[test]
    fn feedback_link_opens_a_draft_without_device_payload() {
        let url = url::Url::parse(&super::feedback_url()).unwrap();
        assert_eq!(url.host_str(), Some("github.com"));
        assert!(url.path().ends_with("/issues/new"));
        assert!(url
            .query_pairs()
            .any(|(key, value)| key == "body" && value.contains("固件")));
        assert!(!url.as_str().contains("serial"));
    }
}
