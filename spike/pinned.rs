use super::{fail, fs, valid_id, LocalPlugins, Path, Result, Surface};
use crate::{
    plugin_canvas::Canvas,
    plugin_canvas_ui::{self, CanvasTheme},
    plugin_status::{StatusCountdown, StatusCounter},
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

#[path = "pinned_payroll.rs"]
mod payroll;

const ORDER_LIMIT: u64 = 64 * 1024;
const ROTATION: Duration = Duration::from_secs(8);
const MASKED_VALUE: &str = "••••";
const ORDER_FILE: &str = "island-pinned.json";

#[derive(Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pinned {
    text: Option<String>,
    value: Option<PinnedValue>,
    icon: Option<PinnedIcon>,
    #[serde(default)]
    sensitive: bool,
    canvas: Option<Slides>,
}

#[derive(Clone, PartialEq)]
struct Slide {
    width: f32,
    canvas: Arc<Canvas>,
    fit_width: bool,
}

impl<'de> Deserialize<'de> for Slide {
    fn deserialize<D: serde::Deserializer<'de>>(input: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error;
        let mut value = serde_json::Value::deserialize(input)?;
        let fit_width = value.as_object_mut()
            .and_then(|fields| fields.remove("fit_width"))
            .map(|value| value.as_bool().ok_or_else(|| Error::custom("常驻画布 fit_width 须为布尔值")))
            .transpose()?
            .unwrap_or(false);
        let (width, canvas) = Canvas::parse_pinned(value).map_err(Error::custom)?;
        Ok(Self { width, canvas: Arc::new(canvas), fit_width })
    }
}

#[derive(Clone, PartialEq, Deserialize)]
#[serde(untagged)]
enum Slides {
    Single(Slide),
    Rotation(Vec<Slide>),
}

impl Slides {
    fn all(&self) -> &[Slide] {
        match self {
            Self::Single(slide) => std::slice::from_ref(slide),
            Self::Rotation(slides) => slides,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PinnedIcon {
    Clock,
    Chart,
    Coin,
    Calendar,
    Cup,
    Cloud,
}

#[derive(Clone, PartialEq, Deserialize)]
#[serde(untagged)]
enum PinnedValue {
    Text(String),
    Number(f64),
    Live(LiveValue),
}

#[derive(Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LiveValue {
    Counter(StatusCounter),
    Countdown(StatusCountdown),
}

fn valid_text(text: &str, max: usize) -> bool {
    !text.trim().is_empty() && text.chars().count() <= max && !text.chars().any(char::is_control)
}

impl Pinned {
    pub(super) fn valid(&self) -> bool {
        if let Some(slides) = &self.canvas {
            return !slides.all().is_empty() && self.text.is_none()
                && self.value.is_none() && self.icon.is_none();
        }
        self.text.as_deref().is_some_and(|text| valid_text(text, 12))
            && self.value.as_ref().is_none_or(|value| match value {
                PinnedValue::Text(text) => valid_text(text, 24),
                PinnedValue::Number(number) => number.is_finite(),
                PinnedValue::Live(LiveValue::Counter(counter)) => counter.valid(),
                PinnedValue::Live(LiveValue::Countdown(countdown)) => countdown.valid(),
            })
    }

    pub(super) fn validate_canvas(&self, package: &super::Package) -> Result<()> {
        if let Some(slides) = &self.canvas {
            if !package.manifest.has_capability("status.canvas") {
                return Err(fail("常驻画布需要声明 status.canvas"));
            }
            for slide in slides.all() {
                slide.canvas.validate_images(|name| package.canvas_images.contains_key(name))
                    .map_err(|error| fail(&error))?;
            }
        }
        Ok(())
    }

    fn reserve_width(&self, ui: &eframe::egui::Ui) -> f32 {
        if let Some(slides) = &self.canvas {
            let slides = slides.all();
            let declared = slides.iter().map(|slide| slide.width).fold(0.0, f32::max);
            if self.sensitive || slides.iter().any(|slide| !slide.fit_width) {
                return declared;
            }
            return slides.iter().map(|slide| {
                // 绑定值可在插件输出不变时继续增长。
                if slide.canvas.binds.is_empty() {
                    plugin_canvas_ui::pinned_content_width(ui, &slide.canvas, slide.width)
                } else {
                    slide.width
                }
            }).fold(0.0, f32::max);
        }
        if !self.sensitive && match &self.value {
            Some(PinnedValue::Live(LiveValue::Counter(counter))) => !counter.sensitive,
            Some(PinnedValue::Live(LiveValue::Countdown(_))) => true,
            _ => false,
        } {
            return 156.0;
        }
        let (mut display, _) = self.display_slide(chrono::Utc::now(), 0);
        display.measure_width(ui);
        display.width(156.0)
    }

    fn page_count(&self) -> usize {
        self.canvas.as_ref().map_or(1, |slides| slides.all().len())
    }

    #[cfg(test)]
    fn display(&self, now: chrono::DateTime<chrono::Utc>) -> (PinnedDisplay, Option<Duration>) {
        self.display_slide(now, 0)
    }

    fn display_slide(&self, now: chrono::DateTime<chrono::Utc>, index: usize) -> (PinnedDisplay, Option<Duration>) {
        let second = now.timestamp();
        let title = self.text.clone().unwrap_or_default();
        let hide = self.sensitive
            || matches!(&self.value,
            Some(PinnedValue::Live(LiveValue::Counter(counter))) if counter.sensitive);
        let (text, value, next) = if hide {
            if self.value.is_some() {
                (title, Some(MASKED_VALUE.into()), None)
            } else {
                (MASKED_VALUE.into(), None, None)
            }
        } else {
            let (value, next) = match &self.value {
                None => (None, None),
                Some(PinnedValue::Text(text)) => (Some(text.clone()), None),
                Some(PinnedValue::Number(number)) => (Some(number.to_string()), None),
                Some(PinnedValue::Live(LiveValue::Counter(counter))) => {
                    (Some(counter.value_at(second)), counter.repaint_at(second))
                }
                Some(PinnedValue::Live(LiveValue::Countdown(countdown))) => (
                    Some(countdown.value_at(second)),
                    countdown.repaint_at(second),
                ),
            };
            (title, value, next)
        };
        let display = PinnedDisplay {
            text,
            value,
            icon: self.icon,
            plugin: String::new(),
            images: BTreeMap::new(),
            slide: self.canvas.as_ref().filter(|_| !hide).map(|slides| slides.all()[index].clone()),
            slide_width: self.canvas.as_ref().map(|slides| {
                slides.all().iter().map(|slide| slide.width).fold(0.0, f32::max)
            }),
            elapsed: Duration::ZERO,
        };
        (
            display,
            next.map(|deadline| {
                Duration::from_millis((deadline * 1000 - now.timestamp_millis()).max(1) as u64)
            }),
        )
    }
}

pub(crate) struct PinnedDisplay {
    pub text: String,
    pub value: Option<String>,
    pub icon: Option<PinnedIcon>,
    pub(crate) plugin: String,
    images: super::canvas_images::CanvasImages,
    slide: Option<Slide>,
    slide_width: Option<f32>,
    elapsed: Duration,
}

impl PinnedDisplay {
    pub(crate) fn measure_width(&mut self, ui: &eframe::egui::Ui) {
        let now = chrono::Utc::now();
        if let Some(counter) = self.payroll_counter(now) {
            self.slide_width = Some(payroll::width(ui, counter, now));
        } else if let Some(slide) = &self.slide {
            if slide.fit_width {
                self.slide_width = Some(plugin_canvas_ui::pinned_content_width(ui, &slide.canvas, slide.width));
            }
        } else if self.slide_width.is_none() {
            use crate::theme::W;
            let card = crate::paint::Card::new(ui, eframe::egui::Pos2::ZERO, ui.clip_rect());
            let label = self.caption();
            self.slide_width = Some((card.text_w(&label, 11.0, W::Semibold)
                + if self.icon.is_some() { 20.0 } else { 4.0 }).clamp(24.0, 156.0));
        }
    }

    pub(crate) fn width(&self, available: f32) -> f32 {
        self.slide_width.map_or(available, |width| width.min(available))
    }

    pub(crate) fn caption(&self) -> String {
        self.value.as_ref().map_or_else(
            || self.text.clone(),
            |value| format!("{}  {value}", self.text),
        )
    }

    fn payroll_counter(&self, now: chrono::DateTime<chrono::Utc>) -> Option<&crate::plugin_canvas::live::Counter> {
        use crate::plugin_canvas::{live::Binding, Kind};
        if self.plugin != "daozai.live-pay" {
            return None;
        }
        let canvas = &self.slide.as_ref()?.canvas;
        let Binding::Counter { counter } = canvas.binds.get("cash")? else {
            return None;
        };
        (counter.decimals == 2
            && (0.0..=10_000_000.0).contains(&counter.sample(now.timestamp()).0)
            && canvas.nodes.iter().any(|node| {
                node.roll && matches!(&node.kind, Kind::Text { text, .. } if text == "{cash}")
            })).then_some(counter)
    }

    pub(crate) fn draw(
        &self,
        card: &crate::paint::Card,
        x: f32,
        y: f32,
        width: f32,
        vertical: bool,
        theme: CanvasTheme,
    ) -> (bool, Option<Duration>, bool) {
        self.draw_at(card, x, y, width, vertical, theme, chrono::Utc::now())
    }

    fn draw_at(
        &self,
        card: &crate::paint::Card,
        x: f32,
        y: f32,
        width: f32,
        vertical: bool,
        theme: CanvasTheme,
        now: chrono::DateTime<chrono::Utc>,
    ) -> (bool, Option<Duration>, bool) {
        use crate::{
            app,
            paint::{self, H},
            theme::{self, W},
        };
        if width <= 0.0 { return (false, None, false); }
        let payroll = self.payroll_counter(now);
        let height = if vertical && payroll.is_some() { 42.0 } else { 22.0 };
        let click = card.page_link(x - width / 2.0, y - height / 2.0, width, height,
            ("plugin-pinned", self.plugin.as_str()), crate::app::resident_navigation::Destination::Plugin(self.plugin.clone())).clicked();
        if let Some(counter) = payroll {
            let (repaint, animating) = payroll::draw(card, x, y, width, vertical, theme, counter, now);
            return (click, repaint, animating);
        }
        if let Some(slide) = &self.slide {
            let natural = if slide.fit_width { self.slide_width.unwrap_or(slide.width) } else { slide.width };
            let scrolling = slide.fit_width && plugin_canvas_ui::has_pinned_text_flow(&slide.canvas.nodes);
            let canvas_width = if !vertical || scrolling { natural.min(width) } else { natural };
            let card = if vertical { card.scaled(x, y, (width / canvas_width).min(1.0)) } else { card.clone() };
            let area = card.rect(x - canvas_width / 2.0, y - 11.0, canvas_width, 22.0);
            let (repaint, animating) =
                plugin_canvas_ui::draw_pinned(&card, &format!("{}:pinned", self.plugin), &slide.canvas, area, theme, self.elapsed, |name, pixels, fit| {
                    self.images.get(name)?.texture(card.ui.ctx(), name, pixels, fit)
                });
            return (click, repaint, animating);
        }
        let icon = self.icon.map(|icon| match icon {
            PinnedIcon::Clock => app::FOCUS_PAGE,
            PinnedIcon::Chart => app::PERFORMANCE_PAGE,
            PinnedIcon::Coin => app::QUOTA_PAGE,
            PinnedIcon::Calendar => app::CALENDAR_PAGE,
            PinnedIcon::Cup => app::WATER_PAGE,
            PinnedIcon::Cloud => app::WEATHER_PAGE,
        });
        if vertical {
            if let Some(page) = icon {
                paint::page_shortcut_icon(card, page, x, y - 13.0, theme::WHITE);
            }
            let title = card.fit(&self.text, 9.0, W::Regular, width - 6.0);
            let split = self.value.is_some() || icon.is_some();
            card.centered_text(
                x,
                y - if split { 1.0 } else { 0.0 },
                &title,
                9.0,
                W::Regular,
                theme::WHITE,
                H::Center,
            );
            if let Some(value) = &self.value {
                let value = card.fit(value, 9.0, W::Semibold, width - 6.0);
                card.centered_text(
                    x,
                    y + 11.0,
                    &value,
                    9.0,
                    W::Semibold,
                    theme::WHITE,
                    H::Center,
                );
            }
        } else {
            let card = card.clipped(x - width / 2.0, y - 11.0, width, 22.0);
            let size = 11.0;
            let inset = if icon.is_some() { 16.0 } else { 0.0 };
            let room = width - inset - 4.0;
            let caption = self.caption();
            let label = if card.text_w(&caption, size, W::Semibold) <= room + 0.01 {
                caption
            } else if let Some(value) = &self.value {
                let value = card.fit(value, size, W::Semibold, room * 0.66);
                let label = card.fit(
                    &self.text,
                    size,
                    W::Semibold,
                    room - card.text_w(&value, size, W::Semibold) - card.text_w("  ", size, W::Semibold),
                );
                format!("{label}  {value}")
            } else {
                card.fit(&self.text, size, W::Semibold, room)
            };
            let left = x - (card.text_w(&label, size, W::Semibold) + inset) / 2.0;
            if let Some(page) = icon {
                paint::page_shortcut_icon(&card, page, left + 5.0, y, theme::WHITE);
            }
            card.centered_text(
                left + inset,
                y,
                &label,
                size,
                W::Semibold,
                theme::WHITE,
                H::Left,
            );
        }
        (click, None, false)
    }
}

#[derive(Default)]
pub(super) struct PinnedState {
    pub(super) order: Vec<String>,
    pub(super) output: BTreeMap<String, Pinned>,
    started: BTreeMap<String, Instant>,
    reserve_width: Option<(f32, f32)>,
}

impl PinnedState {
    pub(super) fn open(dir: &Path) -> Self {
        let order = super::bounded(&dir.join(ORDER_FILE), ORDER_LIMIT)
            .ok()
            .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
            .unwrap_or_default();
        let mut clean = Vec::new();
        for id in order.into_iter().filter(|id| valid_id(id)) {
            if !clean.contains(&id) {
                clean.push(id);
            }
        }
        Self {
            order: clean,
            ..Self::default()
        }
    }

    fn save(&mut self, dir: &Path, order: Vec<String>) -> Result<()> {
        let encoded = serde_json::to_vec(&order)?;
        if encoded.len() as u64 > ORDER_LIMIT {
            return Err(fail("岛上常驻顺序文件超出大小限制"));
        }
        crate::model::atomic_write(&dir.join(ORDER_FILE), &encoded)?;
        self.order = order;
        self.reserve_width = None;
        Ok(())
    }

    pub(super) fn set_output(&mut self, id: &str, value: Option<&Pinned>) {
        if self.output.get(id) == value {
            return;
        }
        if let Some(value) = value {
            self.output.insert(id.into(), value.clone());
        } else {
            self.output.remove(id);
        }
        if self.order.iter().any(|pinned| pinned == id) {
            self.reserve_width = None;
        }
    }

    pub(super) fn retain_outputs(&mut self, mut keep: impl FnMut(&str) -> bool) {
        let count = self.output.len();
        self.output.retain(|id, _| keep(id));
        if self.output.len() != count {
            self.reserve_width = None;
        }
    }

    fn reserved_width(&mut self, ui: &eframe::egui::Ui) -> f32 {
        let scale = ui.ctx().pixels_per_point();
        if let Some((previous_scale, width)) = self.reserve_width {
            if scale == previous_scale {
                return width;
            }
        }
        let width = self.order.iter()
            .filter_map(|id| self.output.get(id))
            .map(|output| output.reserve_width(ui) + 12.0)
            .sum();
        self.reserve_width = Some((scale, width));
        width
    }

    fn displays(
        &mut self,
        suppressed: bool,
        instant: Instant,
        now: chrono::DateTime<chrono::Utc>,
    ) -> (Vec<PinnedDisplay>, Option<Duration>) {
        if suppressed {
            return (Vec::new(), None);
        }
        self.started.retain(|id, _| self.order.contains(id) && self.output.contains_key(id));
        let mut displays = Vec::new();
        let mut next = None;
        for id in &self.order {
            let Some(output) = self.output.get(id) else { continue };
            let elapsed = instant.saturating_duration_since(*self.started.entry(id.clone()).or_insert(instant));
            let turn = (elapsed.as_secs() / ROTATION.as_secs()) as usize;
            let count = output.page_count();
            let (mut display, update) = output.display_slide(now, turn % count);
            display.plugin.clone_from(id);
            let turn_elapsed = Duration::from_millis((elapsed.as_millis() % ROTATION.as_millis()) as u64);
            display.elapsed = ROTATION.mul_f64((turn / count) as f64) + turn_elapsed;
            let rotate = (count > 1).then(|| ROTATION - turn_elapsed);
            next = next.into_iter().chain(update).chain(rotate).min();
            displays.push(display);
        }
        (displays, next)
    }
}

impl LocalPlugins {
    pub(crate) fn pinned_reserved_width(&mut self, ui: &eframe::egui::Ui) -> f32 {
        self.pinned.reserved_width(ui)
    }

    pub(crate) fn pinned_displays(
        &mut self,
        suppressed: bool,
    ) -> (Vec<PinnedDisplay>, Option<Duration>) {
        let (mut displays, update) = self.pinned.displays(suppressed, Instant::now(), chrono::Utc::now());
        displays.retain_mut(|display| {
            if display.slide.is_some() {
                let Some(package) = self.records.get(&display.plugin).and_then(|record| record.package.as_ref()) else {
                    return false;
                };
                display.images.clone_from(&package.canvas_images);
            }
            true
        });
        (displays, update)
    }

    pub(super) fn pinned_name(&self, id: &str) -> Option<&str> {
        let package = self.records.get(id)?.package.as_ref()?;
        package
            .manifest
            .has_surface(Surface::IslandPinned)
            .then_some(package.manifest.name.as_str())
    }

    pub(crate) fn has_pinned_surface(&self,id:&str)->bool {self.pinned_name(id).is_some()}
    pub(crate) fn pinned_enabled(&self,id:&str)->bool {self.pinned.order.iter().any(|item|item==id)}
    pub(crate) fn set_pinned(&mut self, id: &str, enabled: bool) -> Result<()> {
        let mut order = self.pinned.order.clone();
        if enabled {
            if self.pinned_name(id).is_none() {
                return Err(fail("此插件未声明岛上常驻位"));
            }
            if order.iter().any(|item| item == id) {
                return Ok(());
            }
            order.push(id.to_owned());
        } else {
            order.retain(|item| item != id);
            if order == self.pinned.order {
                return Ok(());
            }
        }
        self.pinned.save(&self.dir, order)
    }

    pub(super) fn move_pinned(&mut self, from: usize, to: usize) -> Result<()> {
        let mut order = self.pinned.order.clone();
        order.swap(from, to);
        self.pinned.save(&self.dir, order)
    }

    pub(super) fn restore_pins(&mut self) {
        let mut order = self.pinned.order.clone();
        order.retain(|id| self.pinned_name(id).is_some());
        if order != self.pinned.order {
            if let Err(error) = self.pinned.save(&self.dir, order.clone()) {
                self.notice = error.to_string();
            }
            self.pinned.order = order;
            self.pinned.reserve_width = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Env, Options, TODO_PAGE, WIN_H, WIN_W};
    use eframe::egui;
    use serde_json::json;

    #[test]
    fn pinned_canvas_limits_size_and_rotates_its_slides() {
        let canvas = |width, height, count| json!({
            "width":width, "height":height,
            "nodes": vec![json!({"rect":[0,0,10,10]}); count]
        });
        for (width, height, count) in [(0,22,1), (157,22,1), (30,23,1), (30,22,61)] {
            assert!(Canvas::parse_pinned(canvas(width,height,count)).is_err());
        }
        assert!(Canvas::parse_pinned(canvas(156,22,60)).is_ok());
        assert!(Canvas::parse_pinned(json!({"width":30,"nodes":[{"rect":[0,0,10,10],"hit":{"action":"test"}}]})).is_err());
        let pinned: Pinned = serde_json::from_value(json!({"canvas":[canvas(70,22,1),canvas(80,22,2)]})).unwrap();
        assert!(pinned.valid());
        let mut state = PinnedState::default();
        state.order.push("test.canvas".into());
        state.output.insert("test.canvas".into(), pinned);
        state.order.push("test.other".into());
        state.output.insert("test.other".into(), serde_json::from_value(json!({"text":"在旁显示"})).unwrap());
        let instant = Instant::now();
        let now = chrono::Utc::now();
        let (mut first, next) = state.displays(false, instant, now);
        assert_eq!(first[1].text, "在旁显示");
        let first = first.remove(0);
        assert_eq!(first.width(156.0), 80.0);
        assert_eq!(first.slide.unwrap().width, 70.0);
        assert_eq!(next, Some(ROTATION));
        let second = state.displays(false, instant + ROTATION, now).0.remove(0);
        assert_eq!(second.slide.unwrap().width, 80.0);
        assert_eq!(second.elapsed, Duration::ZERO);
        let first_again = state.displays(false, instant + ROTATION * 2, now).0.remove(0);
        assert_eq!(first_again.elapsed, ROTATION);
    }

    #[test]
    fn pinned_fit_width_measures_text_with_room_for_graphics() {
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let width = |ui: &egui::Ui, value| {
            let pinned: Pinned = serde_json::from_value(value).unwrap();
            let mut display = pinned.display(chrono::Utc::now()).0;
            display.measure_width(ui);
            display.width(156.0)
        };
        let canvas = |label, fit_width| json!({"canvas":{
            "width":150, "fit_width":fit_width,
            "nodes":[{"circle":[9,11,5]},{"text":label,"at":[20,15],"size":11,"max_width":126}]
        }});
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let short = width(ui, canvas("假期", true));
                let long = width(ui, canvas("下周还有一个假期", true));
                assert!((35.0..58.0).contains(&short), "{short}");
                assert!(long > short + 30.0 && long < 150.0, "{long}");
                assert_eq!(width(ui, canvas("假期", false)), 150.0);
                assert!(width(ui, json!({"text":"假期"})) < short);
            });
        });
    }

    #[test]
    fn pinned_reservation_measures_short_content_and_reuses_the_result() -> Result<()> {
        let mut state = PinnedState::default();
        let id = "pinned.short";
        state.order.push(id.into());
        let short: Pinned = serde_json::from_value(json!({"canvas": {
            "width": 156, "fit_width": true, "nodes": [{"rect": [0, 1, 20, 20]}]
        }}))?;
        state.set_output(id, Some(&short));
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert_eq!(state.reserved_width(ui), 36.0);
                state.set_output(id, Some(&short.clone()));
                assert_eq!(state.reserve_width, Some((ui.ctx().pixels_per_point(), 36.0)));
                assert_eq!(state.reserved_width(ui), 36.0);
                assert!(state.started.is_empty());
                state.set_output(id, None);
                assert_eq!(state.reserve_width, None);
                assert_eq!(state.reserved_width(ui), 0.0);
            });
        });
        Ok(())
    }

    #[test]
    fn horizontal_pinned_canvas_keeps_font_size_when_the_slot_is_narrow() {
        let pinned: Pinned = serde_json::from_value(json!({"canvas":{
            "width":140,"nodes":[{"text":"常驻字号","at":[6,15],"size":12}]
        }})).unwrap();
        let display = pinned.display(chrono::Utc::now()).0;
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 36.0));
        let size = |width| {
            let mut found = None;
            for _ in 0..3 {
                let out = ctx.run(egui::RawInput { screen_rect: Some(bounds), ..Default::default() }, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let card = crate::paint::Card::new(ui, egui::Pos2::ZERO, bounds);
                        display.draw(&card, 100.0, 17.0, width, false,
                            CanvasTheme { palette: None, accent: None, background: egui::Color32::BLACK },);
                    });
                });
                found = out.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Text(t) if t.galley.text() == "常驻字号" => {
                        Some((t.galley.job.sections[0].format.font_id.size, shape.clip_rect))
                    }
                    _ => None,
                });
                if found.is_some() { break; }
            }
            found.expect("常驻字号样例已绘制")
        };
        let (wide_size, _) = size(140.0);
        let (narrow_size, clip) = size(72.0);
        assert_eq!(wide_size, 12.0);
        assert_eq!(narrow_size, 12.0);
        assert!(clip.width() <= 72.0);
    }

    #[test]
    fn pinned_canvas_demo_fits_top_and_side() -> Result<()> {
        use crate::skin_paint::{CREAM, GLASS_DARK};
        let fixture = Fixture::new()?;
        let mut host = LocalPlugins::open(fixture.0.join("installed"));
        let id = "example.canvas-pinned-demo";
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/canvas-pinned-demo/plugin.json");
        host.install(&path)?;
        host.enable(id, true)?;
        host.set_pinned(id, true)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let display = loop {
            host.tick(true, true, "2026-11-24");
            if let Some(display) = host.pinned_displays(false).0.into_iter().next() {
                break display;
            }
            assert!(Instant::now() < deadline, "等待常驻画布超时：{:?}", host.errors.get(id));
            std::thread::sleep(Duration::from_millis(2));
        };
        for (skin, palette) in [("cream", CREAM), ("glass-dark", GLASS_DARK)] {
            for (dock, w, h) in [("top", 117_u32, 30_u32), ("left", 44, 42), ("right", 44, 42)] {
                let ctx = egui::Context::default();
                crate::theme::load_fonts(&ctx);
                let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32, h as f32));
                let [r, g, b] = palette.paper;
                let color = egui::Color32::from_rgb(r, g, b);
                let mut raster = crate::preview_raster::Raster::new(w as usize, h as usize);
                for _ in 0..2 {
                    let out = ctx.run(egui::RawInput { screen_rect: Some(bounds), ..Default::default() }, |ctx| {
                        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                            let card = crate::paint::Card::new(ui, egui::Pos2::ZERO, bounds);
                            card.fill(0.0, 0.0, w as f32, h as f32, 10.0, color);
                            display.draw(&card, bounds.center().x, bounds.center().y,
                                display.width(w as f32 - 8.0), dock != "top",
                                CanvasTheme { palette: Some(palette), accent: None, background: color });
                        });
                    });
                    raster.update_textures(&out.textures_delta);
                    let primitives = ctx.tessellate(out.shapes, out.pixels_per_point);
                    assert!(primitives.iter().all(|primitive| bounds.contains_rect(primitive.clip_rect)));
                    if let Some(dir) = std::env::var_os("DAOZAI_CANVAS_SCREENSHOTS") {
                        raster.buf.fill([0.0; 4]);
                        raster.draw(&primitives, out.pixels_per_point);
                        let mut rgba = Vec::new();
                        raster.to_rgba8(&mut rgba);
                        image::RgbaImage::from_raw(w, h, rgba).unwrap().save(
                            Path::new(&dir).join(format!("C-canvas-pinned-{skin}-{dock}.png")))?;
                    }
                    raster.free_textures(&out.textures_delta);
                }
            }
        }
        Ok(())
    }

    struct Fixture(std::path::PathBuf);

    impl Fixture {
        fn new() -> Result<Self> {
            let path = std::env::temp_dir().join(format!(
                "daozai-pinned-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap()
            ));
            fs::create_dir_all(&path)?;
            Ok(Self(path))
        }

        fn install(&self, plugins: &mut LocalPlugins, id: &str) -> Result<()> {
            let package = self.0.join(id);
            fs::create_dir(&package)?;
            let sample =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/examples/payday/cover.png");
            fs::copy(sample, package.join("cover.png"))?;
            let manifest = package.join("plugin.json");
            fs::write(&manifest, json!({
                "id":id,"name":id,"author":"岛仔","version":"1.0.0","api":1,"runtime":"js",
                "entry":"main.js","cover":"cover.png","owner":"tasks","capabilities":["card","island.pinned"],
                "dataVersion":1,"permissions":{},"settings":[],
            }).to_string())?;
            fs::write(
                package.join("main.js"),
                "function refresh() { return {pinned:{text:'常驻示例',value:'25',icon:'clock'}}; }",
            )?;
            plugins.install(&manifest)?;
            plugins.enable(id, true)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct TestConfig(Option<std::ffi::OsString>);

    impl TestConfig {
        fn use_dir(path: &Path) -> Self {
            let old = std::env::var_os("DAOZAI_DATA_DIR");
            std::env::set_var("DAOZAI_DATA_DIR", path);
            Self(old)
        }
    }

    impl Drop for TestConfig {
        fn drop(&mut self) {
            if let Some(old) = &self.0 {
                std::env::set_var("DAOZAI_DATA_DIR", old);
            } else {
                std::env::remove_var("DAOZAI_DATA_DIR");
            }
        }
    }

    #[test]
    fn pinned_plugins_keep_their_order_and_mask_values() {
        let mut state = PinnedState::default();
        for id in ["one", "two", "three"] {
            state.order.push(id.into());
            state.output.insert(
                id.into(),
                serde_json::from_value(serde_json::json!({
                    "text":id,"value":"10000","sensitive":true,
                }))
                .unwrap(),
            );
        }
        let t = Instant::now();
        let now = chrono::Utc::now();
        for instant in [t, t + ROTATION] {
            let (displays, repaint) = state.displays(false, instant, now);
            assert_eq!(displays.iter().map(|d| d.text.as_str()).collect::<Vec<_>>(), ["one", "two", "three"]);
            assert_eq!(displays[0].caption(), "one  ••••");
            assert!(repaint.is_none());
        }
        assert!(state.displays(true, t + ROTATION, now).0.is_empty());
        state.output.remove("one");
        let (displays, _) = state.displays(false, t + ROTATION, now);
        assert_eq!(displays.iter().map(|d| d.text.as_str()).collect::<Vec<_>>(), ["two", "three"]);
    }

    #[test]
    fn pinned_counter_and_countdown_follow_second_boundaries() -> Result<()> {
        let now = chrono::DateTime::from_timestamp_millis(100_250).unwrap();
        let counter =
            json!({"start_value":100,"per_second":2,"start_at":100,"until":102,"decimals":0});
        let pinned: Pinned =
            serde_json::from_value(json!({"text":"工资","value":{"counter":counter}}))?;
        assert_eq!(pinned.display(now).0.value.as_deref(), Some("100"));
        assert_eq!(pinned.display(now).1, Some(Duration::from_millis(750)));
        let done = now + chrono::TimeDelta::seconds(2);
        assert_eq!(pinned.display(done).0.value.as_deref(), Some("104"));
        assert!(pinned.display(done).1.is_none());
        let mut counter = counter;
        counter["sensitive"] = json!(true);
        let hidden: Pinned =
            serde_json::from_value(json!({"text":"工资","value":{"counter":counter}}))?;
        assert_eq!(hidden.display(now).0.value.as_deref(), Some(MASKED_VALUE));
        assert!(hidden.display(now).1.is_none());
        let countdown: Pinned =
            serde_json::from_value(json!({"text":"下班","value":{"countdown":{
                "label":"距离下班","target_at":102,"format":"HH:mm:ss","done_label":"已下班"
            }}}))?;
        assert_eq!(countdown.display(now).0.value.as_deref(), Some("00:00:02"));
        assert_eq!(countdown.display(now).1, Some(Duration::from_millis(750)));
        assert_eq!(countdown.display(done).0.value.as_deref(), Some("已下班"));
        assert!(countdown.display(done).1.is_none());
        Ok(())
    }

    #[test]
    fn pinned_preferences_keep_all_five_plugins_across_restarts() -> Result<()> {
        let fixture = Fixture::new()?;
        let dir = fixture.0.join("records");
        let mut plugins = LocalPlugins::open(dir.clone());
        for id in ["pinned.one", "pinned.two", "pinned.three", "pinned.four", "pinned.five"] {
            fixture.install(&mut plugins, id)?;
            plugins.set_pinned(id, true)?;
        }
        assert_eq!(
            plugins.pinned.order,
            ["pinned.one", "pinned.two", "pinned.three", "pinned.four", "pinned.five"]
        );
        plugins.move_pinned(4, 0)?;
        let mut plugins = LocalPlugins::open(dir);
        assert_eq!(
            plugins.pinned.order,
            ["pinned.five", "pinned.two", "pinned.three", "pinned.four", "pinned.one"]
        );
        plugins.uninstall("pinned.four", false)?;
        assert_eq!(plugins.pinned.order, ["pinned.five", "pinned.two", "pinned.three", "pinned.one"]);
        plugins.pinned.output.insert(
            "pinned.one".into(),
            serde_json::from_value(json!({"text":"第一个插件"}))?,
        );
        assert_eq!(plugins.pinned_displays(false).0.len(), 1);
        plugins.enable("pinned.one", false)?;
        assert!(plugins.pinned_displays(false).0.is_empty());
        assert_eq!(plugins.pinned.order, ["pinned.five", "pinned.two", "pinned.three", "pinned.one"]);
        Ok(())
    }

    #[test]
    fn compatibility_reserves_wide_pins_before_the_first_close() -> Result<()> {
        use crate::app::PW;

        let _lock = crate::ui_tests::ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let fixture = Fixture::new()?;
        let _config = TestConfig::use_dir(&fixture.0.join("config"));
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let mut app = App::new(&ctx, Options { fetch_lyrics: false, ..Default::default() });
        app.settings.dock = crate::model::Dock::Top;
        app.settings.minimal = false;
        app.settings.browser_minimal = false;
        app.settings.pet = false;
        app.settings.skin = crate::model::Skin::Classic;
        let wide: Pinned = serde_json::from_value(json!({
            "text": "常驻宽组件", "value": "文字".repeat(12)
        }))?;
        for id in ["pinned.one", "pinned.two", "pinned.three", "pinned.four", "pinned.five"] {
            app.plugins.pinned.order.push(id.into());
            app.plugins.pinned.set_output(id, Some(&wide));
        }
        let frame = |app: &mut App, t, width| {
            let win = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, WIN_H));
            let _ = ctx.run(egui::RawInput { screen_rect: Some(win), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.frame(ui, win, &Env {
                        t,
                        clock: "12:30:00".into(),
                        today: "2026-11-25".into(),
                        ..Default::default()
                    });
                });
            });
        };
        app.open = true;
        app.w.snap(PW);
        let height = app.expanded_height();
        app.h.snap(height);

        app.render_compat_running = false;
        frame(&mut app, 0.00, WIN_W);
        assert_eq!(app.requested_horizontal_width(), WIN_W);
        app.render_compat_running = true;
        app.settings.island_offset = 60;
        frame(&mut app, 0.01, WIN_W);
        assert_eq!(app.requested_horizontal_width(), WIN_W);

        app.settings.island_offset = 0;
        app.w.snap(PW);
        app.w.pos -= 40.0;
        frame(&mut app, 0.02, WIN_W);
        assert_eq!(app.requested_horizontal_width(), WIN_W);
        app.w.snap(PW);
        app.h.snap(height);
        frame(&mut app, 0.04, WIN_W);
        assert!(app.open);
        assert!(app.pinned_displays.is_empty());
        assert!(app.plugins.pinned.started.is_empty());
        let requested = app.requested_horizontal_width();
        assert!(requested > WIN_W);
        let allocated = (requested / 64.0).ceil() * 64.0;

        // 鼠标继续留在岛外，跨过离开等待后按首个可见常驻帧核对宽档。
        frame(&mut app, 0.14, allocated);
        assert!(!app.open);
        frame(&mut app, 0.16, allocated);
        assert_eq!(app.pinned_displays.len(), 5);
        assert!(app.requested_horizontal_width() <= allocated);
        Ok(())
    }

    #[test]
    fn pinned_plugins_render_together_in_the_collapsed_island() -> Result<()> {
        let _lock = crate::ui_tests::ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let fixture = Fixture::new()?;
        let _config = TestConfig::use_dir(&fixture.0.join("config"));
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, Options { fetch_lyrics: false, ..Default::default() });
        app.settings.minimal = false;
        app.settings.browser_minimal = false;
        app.settings.pet = false;
        app.settings.skin = crate::model::Skin::Classic;
        app.plugins = LocalPlugins::open(fixture.0.join("records"));
        app.settings.visible_pages[TODO_PAGE] = true;
        for id in ["pinned.one", "pinned.two", "pinned.three", "pinned.four", "pinned.five"] {
            fixture.install(&mut app.plugins, id)?;
            app.plugins.set_pinned(id, true)?;
        }
        for (dock, bounds) in [
            (crate::model::Dock::Top, egui::vec2(WIN_W, WIN_H)),
            (crate::model::Dock::Left, egui::vec2(crate::side_dock::SIDE_W, crate::side_dock::SIDE_H)),
        ] {
            app.settings.dock = dock;
            let win = egui::Rect::from_min_size(egui::Pos2::ZERO, bounds);
            let mut captions = 0;
            for i in 1..60 {
                let frame = ctx.run(egui::RawInput { screen_rect: Some(win), ..Default::default() },
                    |ctx| egui::CentralPanel::default().show(ctx, |ui| {
                        app.frame(ui, win, &Env {
                            t: if dock.is_side() { 1.0 } else { 0.0 } + i as f64 / 60.0,
                            clock: "12:30:00".into(),
                            today: "2026-11-25".into(),
                            ..Default::default()
                        });
                    }).inner);
                captions = frame.shapes.iter().filter(|shape| {
                    matches!(&shape.shape, egui::Shape::Text(text)
                        if text.galley.text().starts_with("常驻"))
                }).count();
                if captions == 5 { break; }
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(captions, 5, "{dock:?}");
        }
        Ok(())
    }

    #[test]
    fn pinned_setting_adds_a_capsule_and_hides_on_owner_disable() -> Result<()> {
        let _lock = crate::ui_tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let fixture = Fixture::new()?;
        let _config = TestConfig::use_dir(&fixture.0.join("config"));
        let ctx = egui::Context::default();
        let mut app = App::new(
            &ctx,
            Options {
                fetch_lyrics: false,
                ..Default::default()
            },
        );
        app.settings.minimal = false;
        app.settings.browser_minimal = false;
        app.settings.pet = false;
        app.settings.dock = crate::model::Dock::Top;
        app.settings.skin = crate::model::Skin::Classic;
        app.settings.visible_pages[TODO_PAGE] = true;
        app.plugins = LocalPlugins::open(fixture.0.join("records"));
        let id = "pinned.example";
        fixture.install(&mut app.plugins, id)?;
        let settings_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(450.0, 350.0));
        let mut settings = |events| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(settings_rect),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .show(ctx, |ui| app.plugins.settings(ui, "tasks", true));
                },
            );
        };
        settings(vec![]);
        let pos = ctx
            .read_response(egui::Id::new((
                "settings-plugin-switch",
                "pinned.example:pinned",
            )))
            .unwrap()
            .rect
            .center();
        for pressed in [true, false] {
            settings(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
        }
        assert_eq!(app.plugins.pinned.order, [id]);
        let win = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(WIN_W, WIN_H));
        let frame = |app: &mut App, t| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(win),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        app.frame(
                            ui,
                            win,
                            &Env {
                                t,
                                clock: "12:30:00".into(),
                                today: "2026-11-23".into(),
                                ..Default::default()
                            },
                        );
                    });
                },
            )
        };
        let has_pinned = |output: egui::FullOutput| {
            output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("常驻示例"))
        })
        };
        let mut shown = false;
        for i in 1..60 {
            if has_pinned(frame(&mut app, i as f64 / 60.0)) {
                shown = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(shown, "收起的岛显示常驻内容");
        app.notice = Some(crate::extras::Alert {
            source: crate::extras::AlertSource::Internal,
            title: "测试弹窗".into(),
            body: String::new(),
            page: TODO_PAGE,
            at: 1.0,
            duration: 4.0,
            bluetooth: None,
            meeting: None,
            unread: None,
        });
        assert!(!has_pinned(frame(&mut app, 1.1)), "通知弹窗优先显示");
        assert!(app.notice.is_some());
        app.notice = None;
        let restored = (1..90).any(|index| has_pinned(frame(&mut app, 1.1 + index as f64 / 60.0)));
        assert!(restored, "弹窗退场后恢复常驻");
        app.settings.visible_pages[TODO_PAGE] = false;
        assert!(!has_pinned(frame(&mut app, 3.0)));
        Ok(())
    }

    include!("pinned_payroll_tests.rs");
}
