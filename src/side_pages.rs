//! Side dock layout for the shared island components.

use crate::{
    app::{App, Cmd, Env, CALENDAR_PAGE},
    paint::{self, Card, H},
    theme::{self, size, W},
};
use eframe::egui;

pub(crate) const COLUMN_W: f32 = crate::side_dock::SIDE_PLAYER_W;

pub(crate) fn todo_list_card<'a>(c: &Card<'a>) -> Card<'a> {
    c.placed((196.0, 31.0), (10.0, 101.0), 136.0 / 181.0)
}

pub(crate) fn form_title(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).font(theme::font(14.0, W::Bold)).color(theme::WHITE));
}

pub(crate) fn form_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).font(theme::font(10.5, W::Regular)).color(theme::GRAY));
}

pub(crate) fn form_button(text: &str, selected: bool) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(text).font(theme::font(10.5, W::Semibold)).color(theme::WHITE))
        .min_size(egui::vec2(0.0, 22.0))
        .fill(if selected { egui::Color32::from_rgb(28, 65, 100) } else { theme::BTN })
        .corner_radius(egui::CornerRadius::same(8))
}

impl App {
    pub(crate) fn side_panel(&mut self, c: &Card, env: &Env, cmds: &mut Vec<Cmd>) {
        if self.is_side_player() {
            self.side_music_panel(c, env, cmds);
            return;
        }
        let width = self.expanded_width();
        let height = self.expanded_height();
        let scale = (width / COLUMN_W).min(1.0);
        let left = (width - COLUMN_W * scale) / 2.0;
        let column = c.placed((0.0, 0.0), (left, 0.0), scale);
        let available = self.page_on_dock(self.page());
        let editing = self.calendar.draft.is_some() || self.task_dialog.is_some();

        if available && !editing { self.page_title(self.page(), &column); }
        if available && self.page() == 8 { self.draw_steam_avatar(&column); }
        self.page_gear(&column, COLUMN_W - 16.0, if available { self.page_settings_tab() } else { 10 });
        if self.page() != 8 && self.page_on_dock(8) && (self.steam.badge().is_some() || !self.steam.installed_games.is_empty() || self.settings.page_resident(8)) {
            self.draw_steam_shortcut(&column, COLUMN_W - 46.0, 18.0);
        }
        let clock = self.settings.clock_text(&env.clock);
        let clock_x = COLUMN_W - 12.0 - paint::clock_width(&column, &clock, size::CLOCK);
        let date_left = if self.battery_on_dock() && env.battery.is_some() {
            paint::battery(&column, 17.0, 42.0, env.battery, env.charging, 0.64);
            27.0
        } else { 12.0 };
        if self.clock_on_dock() {
            let date = column.fit(&env.date, size::DATE, W::Regular, (clock_x - date_left - 5.0).max(8.0));
            column.centered_text(date_left, 42.0, &date, size::DATE, W::Regular, theme::GRAY, H::Left);
            paint::clock(&column, clock_x, 46.0, &clock, size::CLOCK);
        }
        self.dock_drag_handle(&column, (8.0, 31.0, COLUMN_W-16.0, 21.0), cmds);

        let body = column.rect(0.0, 54.0, COLUMN_W, (height / scale - 92.0).max(1.0));
        let clip = body.intersect(c.painter.clip_rect());
        let mut ui = egui::Ui::new(
            c.ui.ctx().clone(), egui::Id::new("side-page-body"),
            egui::UiBuilder::new().max_rect(body).layer_id(c.painter.layer_id()).style(c.ui.style().clone()),
        );
        ui.set_clip_rect(clip);
        ui.set_opacity(c.painter.opacity());
        let scroll = &mut ui.spacing_mut().scroll;
        scroll.floating = true;
        scroll.bar_width = 5.0;
        scroll.floating_width = 2.0;
        scroll.floating_allocated_width = 3.0;
        if !c.live { ui.disable(); }

        let page = self.page();
        let content_height = match page {
            1 => 282.0,
            2 if self.water_view == 1 => 276.0,
            2 => 243.0,
            3 => (self.shelf.items.len() as f32 * 94.0 + 38.0).max(152.0),
            9 => 232.0,
            4 => 279.0,
            5 => 274.0,
            6 => 270.0,
            CALENDAR_PAGE => 356.0,
            8 => 291.0,
            _ => 120.0,
        };
        let smooth_scroll = ui.input(|input| input.smooth_scroll_delta.y);
        egui::ScrollArea::vertical().id_salt((page, editing, self.subpage))
            .max_height(body.height()).auto_shrink([false, false])
            .show(&mut ui, |ui| {
                if available && editing {
                    egui::Frame::new().inner_margin(egui::Margin::symmetric(9, 2)).show(ui, |ui| {
                        ui.set_width((body.width() - 21.0).max(100.0));
                        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                        ui.spacing_mut().button_padding = egui::vec2(5.0, 2.0);
                        ui.style_mut().override_font_id = Some(theme::font(11.0, W::Regular));
                        ui.visuals_mut().extreme_bg_color = theme::CARD;
                        ui.visuals_mut().override_text_color = Some(theme::WHITE);
                        if self.calendar.draft.is_some() { self.side_calendar_editor(ui, cmds); }
                        else { self.side_task_editor(ui, cmds); }
                    });
                    return;
                }
                let (rect, _) = ui.allocate_exact_size(egui::vec2(body.width(), content_height * scale), egui::Sense::hover());
                let mut card = Card::new(ui, rect.min, ui.clip_rect());
                card.s = scale;
                card.live = c.live;
                if !available {
                    card.centered_text(COLUMN_W / 2.0, 48.0, "此位置没有可用页面", 13.0, W::Semibold, theme::GRAY, H::Center);
                    return;
                }
                self.scroll_side_list(&card, env);
                self.render_page(page, &card, env, cmds);
                if page == 4 && c.live && (self.adding || self.editing.is_some()) {
                    let list = todo_list_card(&card);
                    let (origin, input_scale) = (list.origin, list.s);
                    self.input_field_at(ui, origin, input_scale, cmds);
                }
            });
        if env.cursor.is_some_and(|cursor| clip.contains(cursor)) {
            let mail_scroll = page == 5 && !self.mail.recent.is_empty() && !self.mail.accounts.is_empty();
            ui.input_mut(|input| {
                // 嵌套列表或正文滚动已接收本次输入。
                if mail_scroll || (smooth_scroll != 0.0 && input.smooth_scroll_delta.y == 0.0) {
                    input.raw_scroll_delta.y = 0.0;
                }
            });
        }
        if !editing && available {
            self.nav_at(&column, COLUMN_W, height / scale - 22.0, height / scale - 9.0);
        }
    }

    fn scroll_side_list(&mut self, c: &Card, env: &Env) {
        if !c.live { return; }
        let Some(cursor) = env.cursor.filter(|cursor| c.painter.clip_rect().contains(*cursor)) else { return; };
        let y = c.to_local(cursor).y;
        let scroll = c.ui.input(|input| input.raw_scroll_delta.y);
        if scroll == 0.0 { return; }
        let keep_page = self.page() == CALENDAR_PAGE || self.page() == 9;
        let page = self.page();
        let (first, max) = match page {
            9 if (31.0..=218.0).contains(&y) => {
                let count = self.clipboard.lock().items.len();
                (self.clip_first, count.saturating_sub(crate::pages::CLIP_ROWS))
            }
            4 if (101.0..=240.0).contains(&y) => {
                (self.todo_first, self.todos.items.len().saturating_sub(3))
            }
            CALENDAR_PAGE if (166.0..=331.0).contains(&y) => {
                (self.calendar.view.first, self.calendar_scroll_max(env))
            }
            _ => return,
        };
        let first = first.min(max);
        let target = if scroll < 0.0 { (first + 1).min(max) } else { first.saturating_sub(1) };
        if first == target && !keep_page { return; }
        if self.wheel_step(scroll).is_some() {
            match page {
                9 => self.clip_first = target,
                4 => self.todo_first = target,
                _ => self.calendar.view.first = target,
            }
        }
        c.ui.input_mut(|input| {
            input.raw_scroll_delta.y = 0.0;
            input.smooth_scroll_delta.y = 0.0;
        });
    }
}
