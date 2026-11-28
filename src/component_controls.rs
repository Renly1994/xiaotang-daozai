//! 页面、常驻图标和设置入口共用的控制层；插件以稳定 ID 保存偏好。
use super::*;

pub(crate) fn icon_control(
    ui: &mut egui::Ui,
    pin: bool,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    let label = if pin {
        if selected {
            "取消岛体常驻"
        } else {
            "常驻到岛体"
        }
    } else if selected {
        "显示左侧设置入口"
    } else {
        "隐藏左侧设置入口"
    };
    let response = ui.add_enabled(
        enabled,
        egui::Button::new("")
            .frame(false)
            .min_size(Vec2::splat(28.0)),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, selected, label)
    });
    let color = if !enabled {
        ui.visuals().weak_text_color()
    } else if pin && selected {
        theme::SOFT_GREEN
    } else {
        ui.visuals().text_color()
    };
    let c = Card::new(ui, response.rect.min, ui.clip_rect()).ghost();
    if pin {
        paint::pin(&c, 14., 14., 14., color);
    } else {
        let points: Vec<_> = (0..=24)
            .map(|i| {
                let a = i as f32 / 24. * std::f32::consts::TAU;
                (14. + 8. * a.cos(), 14. + 4.5 * a.sin())
            })
            .collect();
        c.path(&points, 1.4, color);
        c.ring(14., 14., 2., 1.3, color);
    }
    if (pin && !selected) || (!pin && selected) {
        c.seg((7., 7.), (21., 21.), 1.5, color);
    }
    response
        .on_hover_text(label)
        .on_disabled_hover_text("先开启该功能的后台运行")
}

pub(crate) fn page_key(page: usize) -> String {
    format!("page:{}", PAGE_SHORTCUT_KEYS[page])
}
pub(crate) fn native_slot(page: usize) -> Option<usize> {
    Some(match page {
        MUSIC_PAGE => bar::MUSIC,
        FOCUS_PAGE => bar::FOCUS,
        CALENDAR_PAGE => bar::CLOCK,
        BATTERY_PAGE => bar::BATTERY,
        WEATHER_PAGE => bar::WEATHER,
        PERFORMANCE_PAGE => bar::PERFORMANCE,
        BLUETOOTH_PAGE => bar::BLUETOOTH,
        QUOTA_PAGE => bar::QUOTA,
        _ => return None,
    })
}

impl App {
    pub(crate) fn resident_preference(&self, page: usize) -> bool {
        match page {
            CALENDAR_PAGE => self.settings.show_clock,
            BATTERY_PAGE => self.settings.show_battery,
            WEATHER_PAGE => self.settings.show_weather,
            PERFORMANCE_PAGE => self.settings.show_performance,
            BLUETOOTH_PAGE => self.settings.show_bluetooth,
            QUOTA_PAGE => self.quota.config.compact,
            _ => self
                .settings
                .resident_pages
                .get(page)
                .copied()
                .unwrap_or(false),
        }
    }
    pub(crate) fn resident_on(&self, page: usize) -> bool {
        self.module_on(page) && self.resident_preference(page)
    }
    pub(crate) fn set_resident_preference(&mut self, page: usize, on: bool) {
        match page {
            CALENDAR_PAGE => self.settings.show_clock = on,
            BATTERY_PAGE => self.settings.show_battery = on,
            WEATHER_PAGE => self.settings.show_weather = on,
            PERFORMANCE_PAGE => self.settings.show_performance = on,
            BLUETOOTH_PAGE => self.settings.show_bluetooth = on,
            QUOTA_PAGE => {
                self.quota.config.compact = on;
                self.quota.config.save();
            }
            _ => {
                if let Some(value) = self.settings.resident_pages.get_mut(page) {
                    *value = on;
                }
            }
        }
        self.settings.save();
    }
    #[cfg(feature = "sdk-js")]
    pub(crate) fn plugin_resident(&self, id: &str) -> bool {
        if self.plugins.has_pinned_surface(id) {
            self.plugins.pinned_enabled(id)
        } else {
            self.settings.resident_plugins.iter().any(|p| p == id)
        }
    }
    #[cfg(feature = "sdk-js")]
    pub(crate) fn set_plugin_resident(&mut self, id: &str, on: bool) {
        if self.plugins.has_pinned_surface(id) {
            if let Err(error) = self.plugins.set_pinned(id, on) {
                self.toast = Some((error.to_string(), self.t));
            }
        } else {
            self.settings.resident_plugins.retain(|p| p != id);
            if on {
                self.settings.resident_plugins.push(id.to_owned());
            }
            self.settings.save();
        }
    }
    pub(crate) fn component_keys(&self) -> Vec<String> {
        let legacy = if bar::valid_order(&self.settings.resident_order) {
            self.settings.resident_order.clone()
        } else {
            bar::DEFAULT_ORDER.to_vec()
        };
        let mut defaults = Vec::new();
        for slot in legacy {
            if slot == bar::PET {
                defaults.push("pet".into());
            } else if let Some(page) = (0..PAGE_NAMES.len()).find(|&p| native_slot(p) == Some(slot))
            {
                defaults.push(page_key(page));
            }
        }
        for &page in &self.settings.page_order {
            let key = page_key(page);
            if !defaults.contains(&key) {
                defaults.push(key);
            }
        }
        #[cfg(feature = "sdk-js")]
        for (id, _, _, _) in self.plugins.status_page_preferences() {
            defaults.push(format!("plugin:{id}"));
        }
        let mut result = Vec::new();
        for key in self.settings.island_order.iter().chain(&defaults) {
            if defaults.contains(key) && !result.contains(key) {
                result.push(key.clone());
            }
        }
        result
    }
    pub(crate) fn append_resident_fallbacks(&self, layout: &mut Layout) {
        for &page in &self.settings.page_order {
            if !self.resident_on(page) {
                continue;
            }
            let native = native_slot(page).is_some_and(|slot| layout.x[slot].is_some());
            let mail_bell = page == MAIL_PAGE && layout.x[bar::BELL].is_some();
            if !native && !mail_bell {
                layout.append_page(page);
            }
        }
    }
    pub(crate) fn finish_resident_layout(&self, layout: &mut Layout) {
        #[cfg(feature = "sdk-js")]
        for (index, (id, _)) in self.status_pages.iter().enumerate() {
            if self.plugin_resident(id) && !self.pinned_displays.iter().any(|d| d.plugin == *id) {
                layout.append_page(PAGE_NAMES.len() + index);
            }
        }
        let mut groups = Vec::new();
        for page in 0..PAGE_NAMES.len() {
            if let Some(slot) = native_slot(page) {
                let slots = if page == QUOTA_PAGE {
                    layout.quota_slots().collect()
                } else {
                    vec![slot]
                };
                groups.push((page_key(page), slots));
            }
        }
        groups.push(("pet".into(), vec![bar::PET]));
        groups.push(("notices".into(), vec![bar::BELL]));
        for &(slot, page) in &layout.shortcuts {
            groups.push((self.component_key(page), vec![slot]));
        }
        #[cfg(feature = "sdk-js")]
        for (slot, display) in layout.pinned_slots().zip(&self.pinned_displays) {
            groups.push((format!("plugin:{}", display.plugin), vec![slot]));
        }
        layout.order_components(&groups, &self.component_keys());
    }
    pub(crate) fn component_key(&self, page: usize) -> String {
        if page < PAGE_NAMES.len() {
            return page_key(page);
        }
        #[cfg(feature = "sdk-js")]
        if let Some((id, _)) = self.status_pages.get(page - PAGE_NAMES.len()) {
            return format!("plugin:{id}");
        }
        format!("unknown:{page}")
    }
    pub(crate) fn component_symbol(&self, c: &Card, x: f32, y: f32, page: usize, color: Color32) {
        #[cfg(feature = "sdk-js")]
        if let Some((id, _)) = page
            .checked_sub(PAGE_NAMES.len())
            .and_then(|i| self.status_pages.get(i))
        {
            if let Some(icon) = self.plugins.navigation_icon(id) {
                crate::plugin_canvas_ui::draw_navigation_icon(
                    &c.painter,
                    c.rect(x - 8., y - 8., 16., 16.),
                    icon.commands(),
                    color,
                );
                return;
            }
        }
        paint::page_shortcut_icon(c, page, x, y, color);
    }
    pub(crate) fn resident_shortcut(&self, c: &Card, x: f32, y: f32, page: usize) {
        self.component_symbol(c, x, y, page, theme::WHITE);
        let destination = if page < PAGE_NAMES.len() {
            Destination::Page(page)
        } else {
            #[cfg(feature = "sdk-js")]
            {
                let Some((id, _)) = self.status_pages.get(page - PAGE_NAMES.len()) else {
                    return;
                };
                Destination::Plugin(id.clone())
            }
            #[cfg(not(feature = "sdk-js"))]
            {
                return;
            }
        };
        let _ = crate::quota_ui::bar_tooltip(
            c.page_link(
                x - 11.,
                y - 14.,
                22.,
                28.,
                ("resident-page", self.component_key(page)),
                destination,
            ),
            self.navigation_page_name(page).into(),
        );
    }
}
