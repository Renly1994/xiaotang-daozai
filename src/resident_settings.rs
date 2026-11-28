use crate::app::{
    component_controls::{native_slot, page_key},
    BLUETOOTH_PAGE, PAGE_NAMES, QUOTA_PAGE,
};
use crate::{
    bar_layout::*,
    paint::{self, Card, H},
    resident_date::ResidentDay,
    theme::{self, W},
};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Vec2};

pub(crate) fn preview_layout(order: &[usize], quotas: usize, performance_width: f32) -> Layout {
    let mut layout = Layout::new(true, quotas.max(1), true, true, false, true, true);
    layout.add_performance(performance_width);
    layout.reorder(order);
    layout
}

impl crate::app::App {
    pub(crate) fn resident_order_settings(&mut self, ui: &mut egui::Ui) {
        let mut order = self.component_keys();
        ui.horizontal(|ui| {
            ui.label(RichText::new("拖动岛上的图标").color(ui.visuals().text_color()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("恢复默认").clicked() {
                    self.settings.resident_order.clear();
                    self.settings.island_order.clear();
                    order = self.component_keys();
                    self.settings.save();
                }
            });
        });
        ui.add_space(12.0);
        let indices = self.quota_bar_indices();
        let count = indices.len();
        let weather_snapshot = self.weather_pill().cloned();
        let performance_enabled = self.module_on(crate::app::PERFORMANCE_PAGE)
            && self
                .settings
                .performance_bar
                .width(self.settings.performance_metrics)
                > 0.0;
        let performance_width = if performance_enabled {
            self.settings
                .performance_bar
                .width(self.settings.performance_metrics)
        } else {
            20.0
        };
        let mut layout = preview_layout(&DEFAULT_ORDER, count, performance_width);
        for (position, extra) in self.quota_balance_layout(ui.ctx()) {
            layout.balance_tail(position, extra);
        }
        let now = chrono::Local::now();
        let clock_text = self
            .settings
            .clock_text(&now.format("%H:%M:%S").to_string());
        let clock = self.settings.show_clock.then_some(clock_text.as_str());
        let day = ResidentDay::new(self.settings.resident_date, now.date_naive());
        if let Some(day) = &day {
            layout.set_clock_width(day.group_width(ui.ctx(), clock));
        } else {
            if self.settings.clock_period != 0 {
                layout.widen_clock(40.0);
            }
            if self.settings.clock_seconds {
                layout.widen_clock(24.0);
            }
        }
        if let Some(extra) = self.weather_pill_extra(ui.ctx()) {
            layout.widen(WEATHER, extra);
        }
        // Keep the upstream live island preview; extend its slots with the shared page registry.
        let wireless_width = self.wireless_bar_width();
        layout.append_bluetooth(wireless_width.max(22.0));
        let mut groups: Vec<(String, Vec<usize>, Option<usize>)> = Vec::new();
        for page in 0..PAGE_NAMES.len() {
            let slots = if let Some(slot) = native_slot(page) {
                if page == QUOTA_PAGE {
                    layout.quota_slots().collect()
                } else {
                    vec![slot]
                }
            } else {
                let slot = layout.x.len();
                layout.append_page(page);
                vec![slot]
            };
            groups.push((page_key(page), slots, Some(page)));
        }
        groups.push(("pet".into(), vec![PET], None));
        #[cfg(feature = "sdk-js")]
        for (id, _, _, _) in self.plugins.status_page_preferences() {
            let slot = layout.x.len();
            layout.append_page(usize::MAX);
            if let Some(display) = self.pinned_displays.iter().find(|d| d.plugin == id) {
                layout.widen(slot, (display.width(156.0) - 22.0).max(0.0));
            }
            groups.push((format!("plugin:{id}"), vec![slot], None));
        }
        layout.order_components(
            &groups
                .iter()
                .map(|(key, slots, _)| (key.clone(), slots.clone()))
                .collect::<Vec<_>>(),
            &order,
        );
        groups.sort_by_key(|(key, _, _)| {
            order
                .iter()
                .position(|value| value == key)
                .unwrap_or(usize::MAX)
        });
        let (stage, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 100.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(stage, 10.0, Color32::from_rgb(19, 25, 36));
        let scale = ((stage.width() - 28.0) / layout.width).min(1.0);
        let origin = Pos2::new(stage.center().x - layout.width * scale / 2.0, stage.top());
        // 预览跟随当前的皮肤与形状。
        let pill = self.settings.island_shape == crate::model::IslandShape::Pill;
        let origin = if pill {
            origin + Vec2::new(0.0, 4.0)
        } else {
            origin
        };
        let body = Rect::from_min_size(origin, Vec2::new(layout.width * scale, 30.0 * scale));
        let fill = self.preview_fill();
        if pill {
            ui.painter().rect_filled(body, 15.0 * scale, fill);
        } else {
            crate::shape::fill(
                ui.painter(),
                body,
                8.0 * scale,
                10.0 * scale,
                12.0 * scale,
                fill,
                1.0 / ui.ctx().pixels_per_point(),
            );
        }
        if self.effective_skin().is_glass() {
            let rim = egui::Stroke::new(1.0_f32, Color32::from_white_alpha(60));
            if pill {
                ui.painter()
                    .rect_stroke(body, 15.0 * scale, rim, egui::StrokeKind::Inside);
            } else {
                let edge = Rect::from_min_max(
                    body.min + Vec2::new(0.5, 0.0),
                    body.max - Vec2::new(0.5, 0.5),
                );
                ui.painter().add(egui::Shape::line(
                    crate::shape::outline(edge, 8.0 * scale, 10.0 * scale, 12.0 * scale - 0.5),
                    rim,
                ));
            }
        }
        let content_from = crate::skin_paint::shape_count(ui.ctx(), ui.layer_id());
        let mut card = Card::new(ui, origin, stage);
        card.s = scale;
        let cx = layout.width / 2.0;
        let pointer = ui.ctx().pointer_interact_pos();
        let dragged = egui::DragAndDrop::payload::<String>(ui.ctx());
        let mut movement = None;
        let mut keep_colors = Vec::new();
        for (key, slots, page) in &groups {
            let left = cx
                + slots
                    .iter()
                    .filter_map(|&slot| layout.x[slot].map(|x| x - layout.widths[slot] / 2.0))
                    .fold(f32::INFINITY, f32::min);
            let right = cx
                + slots
                    .iter()
                    .filter_map(|&slot| layout.x[slot].map(|x| x + layout.widths[slot] / 2.0))
                    .fold(f32::NEG_INFINITY, f32::max);
            let width = right - left;
            let x = (left + right) / 2.0;
            let slot = slots[0];
            let label = if let Some(page) = page {
                PAGE_NAMES[*page].to_owned()
            } else if key == "pet" {
                "吉祥物".into()
            } else {
                #[cfg(feature = "sdk-js")]
                {
                    let id = key.strip_prefix("plugin:").unwrap_or("");
                    self.plugins
                        .status_page_preferences()
                        .find(|(plugin, _, _, _)| *plugin == id)
                        .map(|(_, name, _, _)| name.to_owned())
                        .unwrap_or_else(|| "插件".into())
                }
                #[cfg(not(feature = "sdk-js"))]
                {
                    "组件".into()
                }
            };
            let rect = card.rect(left - 5.0, 1.0, width + 10.0, 28.0);
            let response = ui
                .interact(
                    rect,
                    egui::Id::new(("component-order", key)),
                    egui::Sense::click_and_drag(),
                )
                .on_hover_cursor(egui::CursorIcon::Grab);
            response.dnd_set_drag_payload(key.clone());
            if let (Some(from), Some(point)) = (dragged.as_deref(), pointer) {
                if from != key && rect.contains(point) && ui.input(|i| i.pointer.primary_down()) {
                    if let Some(index) = order.iter().position(|value| value == key) {
                        movement = Some((from.clone(), index));
                    }
                }
            }
            let is_dragged = dragged.as_deref() == Some(key);
            let dx = if is_dragged {
                pointer
                    .map(|p| {
                        ((p.x - origin.x) / scale - (left + width / 2.0))
                            .clamp(-left, layout.width - left - width)
                    })
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            if is_dragged {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            } else if response.hovered() {
                ui.painter()
                    .rect_filled(rect, 6.0, Color32::from_white_alpha(18));
            }
            let active = if let Some(page) = page {
                self.resident_on(*page)
                    && match slot {
                        CLOCK => clock.is_some() || day.is_some(),
                        BATTERY => self.resident_power.0.is_some(),
                        WEATHER => weather_snapshot.is_some(),
                        MUSIC => self.media.is_music(),
                        FOCUS => self.pomo.running,
                        QUOTA => count > 0,
                        PERFORMANCE => self.performance_on_bar(),
                        BLUETOOTH => wireless_width > 0.0,
                        _ => true,
                    }
            } else if key == "pet" {
                self.settings.pet
            } else {
                #[cfg(feature = "sdk-js")]
                {
                    let id = key.strip_prefix("plugin:").unwrap_or("");
                    self.plugins.status_pages().any(|(plugin, _)| plugin == id)
                        && self.plugin_resident(id)
                }
                #[cfg(not(feature = "sdk-js"))]
                {
                    false
                }
            };
            let c = card
                .shifted(dx, 0.0)
                .faded(if active || is_dragged { 1.0 } else { 0.38 })
                .ghost();
            if key.starts_with("plugin:") {
                #[cfg(feature = "sdk-js")]
                {
                    let id = key.strip_prefix("plugin:").unwrap();
                    if let Some(display) = self.pinned_displays.iter().find(|d| d.plugin == id) {
                        let from = crate::skin_paint::shape_count(ui.ctx(), ui.layer_id());
                        let _ = display.draw(&c, x, 15.0, width, false, self.pinned_canvas_theme());
                        keep_colors.push((
                            from,
                            crate::skin_paint::shape_count(ui.ctx(), ui.layer_id()),
                        ));
                    } else if let Some(icon) = self.plugins.navigation_icon(id) {
                        crate::plugin_canvas_ui::draw_navigation_icon(
                            &c.painter,
                            c.rect(x - 8., 7., 16., 16.),
                            icon.commands(),
                            theme::WHITE,
                        );
                    } else {
                        paint::page_shortcut_icon(&c, usize::MAX, x, 15., theme::WHITE);
                    }
                }
            } else if page.is_some_and(|page| native_slot(page).is_none()) {
                self.component_symbol(&c, x, 15.0, page.unwrap(), theme::WHITE);
            } else {
                match slot {
                    BLUETOOTH if wireless_width > 0.0 => self.draw_wireless_bar(&c, cx, &layout),
                    BLUETOOTH => {
                        paint::page_shortcut_icon(&c, BLUETOOTH_PAGE, x, 15.0, theme::WHITE)
                    }
                    PERFORMANCE if performance_enabled => self.paint_performance_bar(&c, x),
                    PERFORMANCE => {
                        c.arc(x, 15.0, 7.0, 2.25, 7.18, 2.0, theme::GRAY);
                        c.seg((x, 15.0), (x + 3.0, 11.5), 1.8, theme::WHITE);
                    }
                    BATTERY => paint::battery(
                        &c,
                        x,
                        15.0,
                        self.resident_power.0,
                        self.resident_power.1,
                        1.0,
                    ),
                    CLOCK => {
                        if let Some(day) = &day {
                            day.draw_group(&c, x, clock);
                        } else {
                            paint::clock(
                                &c,
                                x - paint::clock_width(&c, &clock_text, theme::size::PILL_CLOCK)
                                    / 2.0,
                                21.0,
                                &clock_text,
                                theme::size::PILL_CLOCK,
                            );
                        }
                    }
                    WEATHER => {
                        if let Some(snapshot) = weather_snapshot.as_ref() {
                            self.paint_pill_weather(&c, x, &layout, snapshot);
                        } else {
                            crate::weather_ui::icon(
                                &c,
                                x,
                                15.0,
                                16.0,
                                crate::weather::Kind::Partly,
                            );
                        }
                    }
                    PET => self.pet.draw(&c, x, self.t, self.skin_palette()),
                    MUSIC => {
                        paint::playback_bars(&c, x, 15.0, self.t, self.media.playing, theme::WHITE)
                    }
                    FOCUS => crate::focus_bar::draw(
                        &c,
                        x,
                        15.0,
                        self.t,
                        &self.pomo,
                        self.phase_secs(self.pomo.phase),
                        false,
                    ),
                    QUOTA if count > 0 => self.draw_quota_bar(&c, cx, &layout),
                    _ => {
                        c.ring(x, 15.0, 10.0, 2.0, theme::GRAY);
                        c.text(x, 18.0, "AI", 7.0, W::Bold, theme::WHITE, H::Center);
                    }
                }
            }
            if is_dragged {
                ui.painter().line_segment(
                    [
                        Pos2::new(rect.center().x, body.bottom() + 6.0),
                        Pos2::new(rect.center().x, body.bottom() + 12.0),
                    ],
                    egui::Stroke::new(2.0, theme::BLUE),
                );
            }
            response.on_hover_text(label);
        }
        if let Some(palette) = self.skin_palette() {
            let end = crate::skin_paint::shape_count(ui.ctx(), ui.layer_id());
            let mut ranges = Vec::new();
            let mut at = content_from;
            for (from, to) in keep_colors {
                if from > at {
                    ranges.push((at, from));
                }
                at = at.max(to);
            }
            if end > at {
                ranges.push((at, end));
            }
            crate::skin_paint::recolor_layer(ui.ctx(), ui.layer_id(), &ranges, &palette);
        }
        if let Some((slot, index)) = movement {
            if let Some(from) = order.iter().position(|value| *value == slot) {
                order.remove(from);
                order.insert(index, slot);
                self.settings.island_order = order;
                self.settings.save();
            }
        }
        ui.painter().text(
            Pos2::new(stage.center().x, stage.bottom() - 22.0),
            egui::Align2::CENTER_CENTER,
            "按住图标左右拖动",
            theme::font(11.0, W::Regular),
            Color32::from_gray(160),
        );
        ui.add_space(8.0);
        ui.label(
            RichText::new("灰色图标为暂未显示的组件；位置会保留。")
                .size(11.0)
                .color(ui.visuals().weak_text_color()),
        );
    }
}
