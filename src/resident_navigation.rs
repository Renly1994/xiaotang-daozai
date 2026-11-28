//! Resident components declare their destination beside their actual painted hit region.
//! Click and hover use the same destination, including account/device/detail selection.
use crate::{
    app::{App, Env, BLUETOOTH_PAGE, PERFORMANCE_PAGE, QUOTA_PAGE},
    paint::Card,
    performance::View,
};
use eframe::egui::{self, Id, Rect, Response};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Destination {
    Page(usize),
    Performance(View),
    Wireless(String),
    Quota(String),
    #[cfg(feature = "sdk-js")]
    Plugin(String),
}

impl Destination {
    fn page(&self, app: &App) -> Option<usize> {
        Some(match self {
            Self::Page(page) => *page,
            Self::Performance(_) => PERFORMANCE_PAGE,
            Self::Wireless(_) => BLUETOOTH_PAGE,
            Self::Quota(_) => QUOTA_PAGE,
            #[cfg(feature = "sdk-js")]
            Self::Plugin(id) => {
                crate::app::PAGE_NAMES.len()
                    + app
                        .status_pages
                        .iter()
                        .position(|(source, _)| source == id)?
            }
        })
    }
}

#[derive(Clone, Default)]
struct Links {
    regions: Vec<(Id, Rect, Destination)>,
    clicked: Option<Destination>,
}

#[derive(Default)]
pub(crate) struct Navigation {
    hovered: Option<(Id, Destination)>,
    since: Option<f64>,
}

fn key() -> Id {
    Id::new("resident-page-links")
}

impl Card<'_> {
    /// Use instead of `hit` for a component which opens a page. No hover map is needed.
    pub(crate) fn page_link(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        id: impl std::hash::Hash,
        destination: Destination,
    ) -> Response {
        let live = self.live && self.painter.opacity() >= 0.8;
        let response = if live {
            self.hit(x, y, w, h, id)
        } else {
            self.ghost().hit(x, y, w, h, id)
        };
        let clicked = response.clicked();
        if live && response.interact_rect.is_positive() {
            self.ui.ctx().data_mut(|data| {
                let links = data.get_temp_mut_or_default::<Links>(key());
                links
                    .regions
                    .push((response.id, response.interact_rect, destination.clone()));
                if clicked {
                    links.clicked = Some(destination);
                }
            });
        }
        response
    }
}

impl App {
    pub(crate) fn hover_module(&mut self, ctx: &egui::Context, env: &Env, inside: bool) {
        // Take last frame's painted regions. Hidden, clipped and ghost components never register.
        let links = ctx
            .data_mut(|data| data.remove_temp::<Links>(key()))
            .unwrap_or_default();
        let allowed = !self.open
            && self.progress() < 0.1
            && inside
            && !self.hover_lock
            && !self.drag_hot
            && (self.notice.is_none() || self.compact_notices())
            && !self.is_editing()
            && self.position_drag.is_none()
            && !env.fullscreen_hidden
            && !env.screen_locked
            && !env.primary_down.unwrap_or(false)
            && !ctx.input(|i| i.pointer.any_down());
        let target = if allowed {
            env.cursor
                .and_then(|p| {
                    links
                        .regions
                        .into_iter()
                        .rev()
                        .find(|(_, rect, _)| rect.contains(p))
                })
                .map(|(id, _, destination)| (id, destination))
        } else {
            None
        };
        if target != self.resident_navigation.hovered {
            self.resident_navigation.hovered = target;
            self.resident_navigation.since = Some(env.t);
        }
        let delay = self.settings.hover_page_delay_ms as f64 / 1000.0;
        if let Some((_, destination)) = self.resident_navigation.hovered.clone() {
            if self
                .resident_navigation
                .since
                .is_some_and(|since| env.t - since >= delay)
            {
                if let Some(page) = self.prepare_destination(destination) {
                    self.go(page, 1.0);
                    // Select the destination's final canvas before starting the opening spring.
                    self.page_height.snap(self.expanded_height());
                }
            }
        }
    }

    pub(crate) fn module_hover_ready(&self, t: f64) -> bool {
        let Some((_, destination)) = &self.resident_navigation.hovered else {
            // Crossing a gap after a component must not immediately open the old page.
            return self.resident_navigation.since.is_none_or(|since| {
                t - since >= self.settings.hover_page_delay_ms as f64 / 1000.0
            });
        };
        destination
            .page(self)
            .is_some_and(|page| self.module_on(page))
            && self
                .resident_navigation
                .since
                .is_some_and(|since| t - since >= self.settings.hover_page_delay_ms as f64 / 1000.0)
    }

    pub(crate) fn clicked_module(&mut self, ctx: &egui::Context) {
        let destination =
            ctx.data_mut(|data| data.get_temp_mut_or_default::<Links>(key()).clicked.take());
        if let Some(destination) = destination {
            if let Some(page) = self.prepare_destination(destination) {
                self.show_page(page);
            }
        }
    }

    fn prepare_destination(&mut self, destination: Destination) -> Option<usize> {
        let page = destination.page(self)?;
        match destination {
            Destination::Page(_) => {}
            Destination::Performance(view) => {
                self.performance.view = Some(view);
                self.performance.list_first = 0;
            }
            Destination::Wireless(id) => {
                self.wireless_focus = Some(id);
            }
            Destination::Quota(id) => {
                self.select_quota_account(id);
            }
            #[cfg(feature = "sdk-js")]
            Destination::Plugin(_) => {}
        };
        if page == crate::app::WEATHER_PAGE {
            self.weather.solar_open = false;
        }
        Some(page)
    }

    pub(crate) fn page_battery(&self, c: &Card, env: &Env) {
        use crate::{
            paint::{self, H},
            theme::{self, W},
        };
        paint::battery(c, 57.0, 77.0, env.battery, env.charging, 2.2);
        let value = env
            .battery
            .map_or("暂无电量数据".into(), |pct| format!("{pct}%"));
        c.centered_text(106.0, 68.0, &value, 26.0, W::Bold, theme::WHITE, H::Left);
        c.centered_text(
            106.0,
            98.0,
            if env.battery.is_none() {
                "未检测到本机电池"
            } else if env.charging {
                "正在充电"
            } else if env.battery == Some(100) {
                "电量已充满"
            } else {
                "未在充电"
            },
            12.0,
            W::Regular,
            theme::GRAY,
            H::Left,
        );
    }
}

#[cfg(test)]
pub(crate) fn registered(ctx: &egui::Context) -> Vec<(Rect, Destination)> {
    ctx.data(|data| {
        data.get_temp::<Links>(key())
            .unwrap_or_default()
            .regions
            .into_iter()
            .map(|(_, rect, destination)| (rect, destination))
            .collect()
    })
}
