//! 天气页与天气设置：实况 + 四日预报，图标全部用面板矢量绘制。
use crate::app::{App, Cmd, Env};
use crate::paint::{self, Card, H};
use crate::theme::{self, W};
use crate::weather::{self, Kind};
use eframe::egui::{self, Color32};

const SUN: Color32 = Color32::from_rgb(255, 198, 88);
const CLOUD: Color32 = Color32::from_rgb(206, 214, 228);
const RAIN: Color32 = Color32::from_rgb(116, 176, 255);
const SNOW: Color32 = Color32::from_rgb(228, 238, 255);
const BOLT: Color32 = Color32::from_rgb(255, 214, 92);
const WARN: Color32 = Color32::from_rgb(255, 166, 77);

/// 常驻胶囊上迷你天气图标的边长。
const PILL_ICON: f32 = 15.0;

/// 胶囊上的温度文字：取整并带度符，负零归零。
fn pill_temp_text(snapshot: &crate::weather::Snapshot) -> String {
    format!("{}°", snapshot.temp.round() as i32)
}

fn sun(c: &Card, cx: f32, cy: f32, s: f32) {
    c.circle(cx, cy, s * 0.40, SUN);
    for i in 0..8 {
        let angle = i as f32 * std::f32::consts::TAU / 8.0;
        c.seg((cx + angle.cos() * s * 0.58, cy + angle.sin() * s * 0.58), (cx + angle.cos() * s * 0.90, cy + angle.sin() * s * 0.90), 1.7, paint::alpha(SUN, 0.9));
    }
}

/// 弯月：外圆减去偏右的内圆，用一条闭合多边形画出来，不依赖背景色。
fn moon(c: &Card, cx: f32, cy: f32, s: f32) {
    let (outer, inner, offset) = (0.62 * s, 0.55 * s, 0.30 * s);
    let x = (offset * offset + outer * outer - inner * inner) / (2.0 * offset);
    let y = (outer * outer - x * x).max(0.0).sqrt();
    let mut points = Vec::with_capacity(40);
    let (top, bottom) = (y.atan2(x), (-y).atan2(x));
    let steps = 18;
    for i in 0..=steps {
        let angle = top + (bottom + std::f32::consts::TAU - top) * i as f32 / steps as f32;
        points.push((cx + angle.cos() * outer, cy + angle.sin() * outer));
    }
    let (inner_bottom, inner_top) = ((-y).atan2(x - offset), y.atan2(x - offset) - std::f32::consts::TAU);
    for i in 0..=steps {
        let angle = inner_bottom + (inner_top - inner_bottom) * i as f32 / steps as f32;
        points.push((cx + offset + angle.cos() * inner, cy + angle.sin() * inner));
    }
    c.poly(&points, SNOW);
}

fn cloud(c: &Card, cx: f32, cy: f32, s: f32, color: Color32) {
    c.circle(cx - s * 0.36, cy + s * 0.04, s * 0.32, color);
    c.circle(cx, cy - s * 0.18, s * 0.42, color);
    c.circle(cx + s * 0.38, cy + s * 0.02, s * 0.31, color);
    c.fill(cx - s * 0.42, cy + s * 0.02, s * 0.84, s * 0.34, s * 0.17, color);
}

fn icon(c: &Card, cx: f32, cy: f32, size: f32, kind: Kind) {
    let s = size / 2.0;
    match kind {
        Kind::Sun => sun(c, cx, cy, s),
        Kind::Moon => moon(c, cx, cy, s * 1.05),
        Kind::Partly => { sun(c, cx + s * 0.30, cy - s * 0.30, s * 0.60); cloud(c, cx - s * 0.10, cy + s * 0.22, s * 0.90, CLOUD); }
        Kind::Cloud => cloud(c, cx, cy, s * 0.98, CLOUD),
        Kind::Fog => {
            cloud(c, cx, cy - s * 0.14, s * 0.90, CLOUD);
            for i in 0..3 { c.fill(cx - s * 0.58, cy + s * 0.30 + i as f32 * s * 0.16, s * 1.16, 1.4, 0.7, paint::alpha(CLOUD, 0.72)); }
        }
        Kind::Drizzle => {
            cloud(c, cx, cy - s * 0.16, s * 0.90, CLOUD);
            for i in 0..3 {
                let x = cx - s * 0.32 + i as f32 * s * 0.32;
                c.seg((x, cy + s * 0.32), (x - s * 0.06, cy + s * 0.50), 1.5, RAIN);
            }
        }
        Kind::Rain => {
            cloud(c, cx, cy - s * 0.16, s * 0.90, CLOUD);
            for i in 0..3 {
                let x = cx - s * 0.32 + i as f32 * s * 0.32;
                c.seg((x, cy + s * 0.30), (x - s * 0.12, cy + s * 0.64), 1.8, RAIN);
            }
        }
        Kind::Shower => {
            cloud(c, cx, cy - s * 0.16, s * 0.90, CLOUD);
            for i in 0..2 {
                let x = cx - s * 0.18 + i as f32 * s * 0.38;
                c.seg((x, cy + s * 0.32), (x - s * 0.12, cy + s * 0.60), 1.9, RAIN);
            }
            c.circle(cx + s * 0.34, cy + s * 0.68, s * 0.08, RAIN);
        }
        Kind::Snow => {
            cloud(c, cx, cy - s * 0.16, s * 0.90, CLOUD);
            for i in 0..3 {
                let x = cx - s * 0.30 + i as f32 * s * 0.30;
                c.circle(x, cy + s * 0.42 + (i % 2) as f32 * s * 0.14, s * 0.10, SNOW);
            }
        }
        Kind::Thunder => {
            cloud(c, cx, cy - s * 0.18, s * 0.90, CLOUD);
            c.poly(&[
                (cx - s * 0.04, cy + s * 0.26), (cx + s * 0.18, cy + s * 0.26), (cx + s * 0.02, cy + s * 0.56),
                (cx + s * 0.18, cy + s * 0.56), (cx - s * 0.12, cy + s * 0.88), (cx - s * 0.01, cy + s * 0.54),
                (cx - s * 0.16, cy + s * 0.54),
            ], BOLT);
        }
    }
}

impl App {
    /// 后台取数的轮询与自动刷新；数据过期就重新拉一次。
    pub(crate) fn weather_tick(&mut self) {
        self.weather.poll();
        // 单元测试不联网：数据由测试直接塞进状态。
        if cfg!(test) { return; }
        let city = self.settings.weather_city.trim().to_string();
        if self.weather.city_changed(&city) {
            self.weather.refresh(&city, self.t, true);
        } else if self.weather.stale(chrono::Utc::now().timestamp()) {
            self.weather.refresh(&city, self.t, false);
        }
    }

    /// 常驻胶囊上的天气徽标：有快照、天气页可见且非极简时才占位。
    pub(crate) fn weather_pill(&self) -> Option<&crate::weather::Snapshot> {
        self.weather.snapshot.as_ref()
            .filter(|_| self.settings.visible_pages[8] && !self.effective_minimal())
    }

    /// 温度文字比基础槽宽时需要补的宽度（负数会被调用方忽略）。
    pub(crate) fn weather_pill_extra(&self, ctx: &egui::Context) -> Option<f32> {
        let snapshot = self.weather_pill()?;
        let text = pill_temp_text(snapshot);
        let text_w = ctx.fonts(|fonts| fonts.layout_no_wrap(text, theme::font(theme::size::PILL_CLOCK, W::Bold), theme::WHITE).size().x).ceil();
        let chip = PILL_ICON + 4.0 + text_w + 2.0;
        Some(chip - crate::bar_layout::WEATHER_WIDTH)
    }

    /// 常驻胶囊的天气徽标：小图标 + 温度，紧跟时钟；点击进入天气页。
    pub(crate) fn draw_pill_weather(&mut self, c: &Card, x: f32, layout: &crate::bar_layout::Layout) {
        let Some(snapshot) = self.weather_pill().cloned() else { return };
        self.paint_pill_weather(c, x, layout, &snapshot);
        let width = layout.widths[crate::bar_layout::WEATHER];
        let tip = format!("{} · {}\n更新于 {}", snapshot.place, weather::describe(snapshot.code), weather::clock(snapshot.at));
        // 用 bar_tooltip：常驻胶囊上的提示不能用 on_hover_text，悬浮层会盖住指针、抢走点击。
        if crate::quota_ui::bar_tooltip(c.hit(x - width / 2.0, 1.0, width, 28.0, "weather-pill"), tip).clicked() {
            self.show_page(8);
        }
    }

    /// 只画不响应命中，设置页的常驻预览复用。
    pub(crate) fn paint_pill_weather(&self, c: &Card, x: f32, layout: &crate::bar_layout::Layout, snapshot: &crate::weather::Snapshot) {
        // 吉祥物游到徽标附近时先淡出，避免遮挡。
        let alpha = layout.pet_clearance(crate::bar_layout::WEATHER);
        let faded = c.faded(alpha);
        let faded = if alpha < 0.8 { faded.ghost() } else { faded };
        let c = &faded;
        let width = layout.widths[crate::bar_layout::WEATHER];
        let left = x - width / 2.0;
        icon(c, left + 7.5, 15.0, PILL_ICON, weather::kind(snapshot.code, snapshot.day));
        // 超过半小时没更新就降为灰色，和额度徽标的过期处理一致。
        let stale = chrono::Utc::now().timestamp() - snapshot.at > weather::INTERVAL * 2;
        c.text(left + 16.0, 21.0, &pill_temp_text(snapshot), theme::size::PILL_CLOCK, W::Bold, if stale { theme::GRAY } else { theme::WHITE }, H::Left);
    }

    pub(crate) fn page_weather(&mut self, c: &Card, _env: &Env, _cmds: &mut Vec<Cmd>) {
        let loading = self.weather.loading;
        c.text(370.0, 23.0, if loading { "更新中" } else { "刷新" }, 10.5, W::Regular, if loading { theme::DIM } else { theme::GRAY }, H::Right);
        if c.hit(328.0, 10.0, 44.0, 22.0, "weather-refresh").on_hover_text("重新获取当地天气").clicked() && !loading {
            self.weather.refresh(&self.settings.weather_city, self.t, true);
        }

        let Some(snapshot) = self.weather.snapshot.clone() else {
            let (title, detail) = if loading {
                ("正在获取当地天气…", "首次定位需要几秒钟".to_string())
            } else if let Some(error) = &self.weather.error {
                ("天气暂时取不到", error.clone())
            } else {
                ("还没有天气数据", "点右上「刷新」重试".to_string())
            };
            c.text(209.5, 72.0, title, 13.0, W::Semibold, theme::WHITE, H::Center);
            c.text(209.5, 90.0, &c.fit(&detail, 10.5, W::Regular, 320.0), 10.5, W::Regular, theme::GRAY, H::Center);
            return;
        };

        // 左：当前实况。图标、温度、天气与城市自上而下排开。
        icon(c, 46.0, 61.0, 42.0, weather::kind(snapshot.code, snapshot.day));
        c.text(78.0, 74.0, &format!("{:.0}°", snapshot.temp), 30.0, W::Bold, theme::WHITE, H::Left);
        c.text(80.0, 92.0, weather::describe(snapshot.code), 12.0, W::Semibold, theme::WHITE, H::Left);
        c.text(80.0, 106.0, &c.fit(&snapshot.place, 10.5, W::Regular, 106.0), 10.5, W::Regular, theme::GRAY, H::Left);

        // 底部一行：体感、湿度、风速。
        let mut meta = Vec::new();
        if let Some(feels) = snapshot.feels { meta.push(format!("体感 {feels:.0}°")); }
        if let Some(humidity) = snapshot.humidity { meta.push(format!("湿度 {humidity}%")); }
        if let Some(wind) = snapshot.wind { meta.push(format!("风 {wind:.0} km/h")); }
        if !meta.is_empty() {
            c.text(20.0, 121.0, &c.fit(&meta.join(" · "), 9.5, W::Regular, 166.0), 9.5, W::Regular, theme::DIM, H::Left);
        }
        let age = chrono::Utc::now().timestamp() - snapshot.at;
        let offline = self.weather.error.is_some();
        let stamp = weather::clock(snapshot.at);
        let stale = offline || age > 3600;
        c.text(322.0, 23.0, &stamp, 9.5, W::Regular, if stale { WARN } else { theme::DIM }, H::Right);
        c.hit(296.0, 12.0, 28.0, 20.0, "weather-stamp").on_hover_text(format!(
            "上次更新 {stamp}{}",
            if offline { "，当前显示的是缓存" } else { "" }
        ));

        // 右：四日预报，等宽分栏。
        c.seg((196.0, 34.0), (196.0, 108.0), 0.8, Color32::from_white_alpha(16));
        let count = snapshot.days.len().clamp(1, 4);
        let span = (404.0 - 200.0) / count as f32;
        for (index, day) in snapshot.days.iter().take(4).enumerate() {
            let x = 200.0 + span * (index as f32 + 0.5);
            c.text(x, 45.0, &day.label, 9.5, W::Regular, if index == 0 { theme::WHITE } else { theme::GRAY }, H::Center);
            icon(c, x, 66.0, 24.0, weather::kind(day.code, true));
            c.text(x, 90.0, &format!("{:.0}°", day.high), 12.5, W::Semibold, theme::WHITE, H::Center);
            c.text(x, 103.0, &format!("{:.0}°", day.low), 10.5, W::Regular, theme::GRAY, H::Center);
            if day.rain >= 20 { c.text(x, 114.0, &format!("{}%", day.rain), 8.5, W::Regular, RAIN, H::Center); }
        }
    }

    pub(crate) fn weather_settings(&mut self, ui: &mut egui::Ui) {
        use crate::extras::{settings_card, settings_control_row, settings_hint, settings_label};
        settings_label(ui, "位置");
        settings_card(ui, |ui| {
            settings_control_row(ui, "城市", 56.0, |ui| {
                let width = (ui.available_width() - 8.0).min(300.0);
                ui.add_sized([width, 30.0], egui::TextEdit::singleline(&mut self.settings.weather_city)
                    .vertical_align(egui::Align::Center).hint_text("留空则按网络出口自动定位，例如：杭州"))
            });
            settings_hint(ui, "填中文或英文城市名都行；改完点下面的「刷新天气」立即生效。留空则按当前网络出口定位。");
        });

        settings_label(ui, "当前");
        settings_card(ui, |ui| {
            let place = self.weather.snapshot.as_ref().map(|s| (s.place.clone(), s.temp, s.at));
            let text = match &place {
                Some((place, temp, _)) => format!("{place} · {temp:.0}°"),
                None if self.weather.loading => "正在定位…".to_string(),
                None => "尚未获取".to_string(),
            };
            ui.label(egui::RichText::new(text).size(13.0));
            if let Some((_, _, at)) = place {
                ui.label(egui::RichText::new(format!("更新于 {}", weather::clock(at))).size(11.5).color(ui.visuals().weak_text_color()));
            }
            if let Some(error) = &self.weather.error {
                ui.add(egui::Label::new(egui::RichText::new(error).size(11.5).color(WARN)).wrap());
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.weather.loading, egui::Button::new(if self.weather.loading { "正在获取…" } else { "刷新天气" })).clicked() {
                    self.weather.refresh(&self.settings.weather_city, self.t, true);
                }
                if ui.add_enabled(!self.weather.loading, egui::Button::new("自动定位")).on_hover_text("清空城市，按当前网络出口重新定位").clicked() {
                    self.settings.weather_city.clear();
                    self.weather.refresh("", self.t, true);
                }
            });
            settings_hint(ui, "数据来自 Open-Meteo（免费、无需密钥）；岛仔只上传城市名或由定位服务给出的坐标，不涉及账号信息。");
        });
    }
}
