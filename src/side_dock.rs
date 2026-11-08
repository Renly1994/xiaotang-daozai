//! 左右停靠：岛体贴在屏幕左 / 右边缘，收起时是竖向胶囊，展开后面板从边缘滑出。
//!
//! 收起栏和展开页面均按竖向排列。

use crate::app::{App, Env, Phase, BAR_H_MIN, BAR_W_MIN, PILL_GAP};
use crate::model::{Dock, IslandShape};
use crate::paint::{self, Card, H};
use crate::theme::{self, W};
use eframe::egui::{Color32, Pos2, Rect, Vec2};

/// 侧边停靠时的透明画布（窗口）大小，逻辑点。
pub const SIDE_W: f32 = 500.0;
pub const SIDE_H: f32 = 600.0;
/// 收起时竖向胶囊的厚度。
pub const SIDE_BAR_W: f32 = 36.0;
pub const SIDE_PLAYER_W: f32 = 156.0;
pub const SIDE_MIN_W: u16 = 144;
pub const SIDE_MAX_W: u16 = 200;
pub const SIDE_PLAYER_H: f32 = 460.0;
const SIDE_LYRIC_H: f32 = 166.0;
/// 岛体中心能离开屏幕竖直中线的预留：展开面板（最高约 236）与肩部、阴影留在屏幕内。
pub const SIDE_HALF_EXTENT: f32 = 150.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SideItem {
    Clock, Battery, Music, Lyric, Focus, FocusTimer, Bell, Pet, Shortcut(usize),
    /// AI 额度：账号下标（None 为「+N」）与高度。
    Quota(usize, f32),
}

impl SideItem {
    pub(crate) fn height(self) -> f32 {
        match self { Self::Clock => 32.0, Self::Battery => 20.0, Self::Music => 18.0, Self::Lyric => SIDE_LYRIC_H, Self::Focus => 22.0, Self::FocusTimer => 38.0, Self::Bell => 22.0, Self::Quota(_, h) => h, Self::Pet => 32.0, Self::Shortcut(_) => 28.0 }
    }

    fn page(self) -> Option<usize> {
        match self {
            Self::Clock => Some(crate::app::CALENDAR_PAGE),
            Self::Music | Self::Lyric => Some(0),
            Self::Focus | Self::FocusTimer => Some(1),
            Self::Bell => Some(5),
            Self::Quota(..) => Some(6),
            Self::Shortcut(page) => Some(page),
            _ => None,
        }
    }
}

impl App {
    pub(crate) fn dock(&self) -> Dock { self.view_dock.unwrap_or(self.settings.dock) }
    pub(crate) fn side(&self) -> bool { self.dock().is_side() }
    pub(crate) fn is_side_player(&self) -> bool { self.side() && self.page() == 0 }
    pub(crate) fn side_player_is_folded(&self) -> bool { self.is_side_player() && self.side_player_folded }

    /// 竖向胶囊里依次显示的内容。
    pub(crate) fn side_items(&self) -> Vec<SideItem> {
        use crate::bar_layout as bar;
        let mut items = Vec::new();
        let page_on = |page| self.settings.page_on(page, self.dock());
        let pinned = |page: usize| page_on(page) && self.settings.page_resident(page);
        let order = bar::normalize_order(&self.settings.resident_order).unwrap_or_else(|| {
            if self.settings.show_battery { bar::DEFAULT_ORDER.to_vec() }
            else { vec![bar::QUOTA, bar::PET, bar::CLOCK, bar::BATTERY, bar::MUSIC, bar::FOCUS, bar::WATER, bar::SHELF, bar::CLIPBOARD, bar::TODO, bar::MAIL, bar::STEAM] }
        });
        for slot in order {
            match slot {
                bar::CLOCK if self.clock_on_dock() => items.push(SideItem::Clock),
                bar::BATTERY if self.battery_on_dock() && self.resident_power.0.is_some() => items.push(SideItem::Battery),
                bar::PET if self.settings.pet && (!self.settings.split_docks || self.settings.dock == self.dock()) => items.push(SideItem::Pet),
                bar::MUSIC if page_on(0) && (self.media.is_music() || pinned(0)) => {
                    items.push(SideItem::Music);
                    if self.island_lyrics_available() { items.push(SideItem::Lyric); }
                }
                bar::FOCUS if page_on(1) && self.pomo.running => items.push(SideItem::FocusTimer),
                bar::FOCUS if pinned(1) => items.push(SideItem::Focus),
                bar::QUOTA if page_on(6) => {
                    let quotas = self.quota_side_items();
                    if quotas.is_empty() && pinned(6) { items.push(SideItem::Shortcut(6)); }
                    items.extend(quotas.into_iter().map(|(index, height)| SideItem::Quota(index, height)));
                }
                bar::STEAM if page_on(8) && (self.steam.badge().is_some() || !self.steam.installed_games.is_empty() || pinned(8)) => items.push(SideItem::Shortcut(8)),
                bar::WATER if pinned(2) => items.push(SideItem::Shortcut(2)),
                bar::SHELF if pinned(3) => items.push(SideItem::Shortcut(3)),
                bar::CLIPBOARD if pinned(9) => items.push(SideItem::Shortcut(9)),
                bar::TODO if pinned(4) => items.push(SideItem::Shortcut(4)),
                bar::MAIL if pinned(5) => items.push(SideItem::Shortcut(5)),
                _ => {}
            }
        }
        if self.t < self.notification_bell_until && (self.settings.split_docks || page_on(5)) { items.push(SideItem::Bell); }
        if items.is_empty() && self.settings.split_docks {
            if let Some(page) = self.settings.page_order.iter().copied().find(|&page| page_on(page)) {
                items.push(match page { 0 => SideItem::Music, 1 => SideItem::Focus, _ => SideItem::Shortcut(page) });
            }
        }
        items
    }

    /// 收起时竖向胶囊的长度。
    pub(crate) fn side_bar_len(&self) -> f32 {
        14.0 + self.side_items().iter().map(|i| i.height()).sum::<f32>()
    }

    /// 收起态尺寸（屏幕宽 × 高）。极简模式是贴边的一小段竖线。
    pub(crate) fn side_base_size(&self, minimal: bool) -> (f32, f32) {
        let lyrics = self.island_lyrics_available();
        if self.side_player_is_folded() {
            return (SIDE_BAR_W, if lyrics { (SIDE_LYRIC_H + 34.0).min(self.expanded_height() - 32.0) } else { 72.0 });
        }
        if minimal && !lyrics { (BAR_H_MIN, BAR_W_MIN) } else { (SIDE_BAR_W, self.side_bar_len().min(self.expanded_height() - 32.0)) }
    }

    fn side_bar_scale(&self, height: f32, total: f32) -> f32 {
        (height / (total + 14.0)).min(1.0)
    }

    pub(crate) fn side_hover_page(&self, win: Rect, cursor: Pos2) -> Option<usize> {
        let rect = self.island_rect(win, self.w.pos, self.h.pos);
        if rect.width() < 16.0 { return None; }
        let items = self.side_items();
        let total = items.iter().map(|item| item.height()).sum();
        let scale = self.side_bar_scale(rect.height(), total);
        let y = (cursor.y - rect.center().y) / scale + total / 2.0;
        let mut bottom = 0.0;
        items.into_iter().find_map(|item| {
            let top = bottom;
            bottom += item.height();
            (top <= y && y < bottom).then(|| item.page()).flatten()
        }).filter(|&page| self.settings.page_on(page, self.dock())
            && (page != crate::app::CALENDAR_PAGE || self.settings.calendar_resident))
    }

    fn side_gap(&self) -> f32 {
        if self.settings.island_shape == IslandShape::Pill { PILL_GAP } else { 0.0 }
    }

    /// 宽 `w`、高 `h` 的岛体在画布 `win` 里的位置：顶部停靠水平居中贴顶，
    /// 左右停靠贴边、竖直居中。
    pub(crate) fn island_rect(&self, win: Rect, w: f32, h: f32) -> Rect {
        let gap = self.side_gap();
        match self.dock() {
            Dock::Top => Rect::from_min_size(Pos2::new(win.center().x - w / 2.0, win.top() + gap), Vec2::new(w, h)),
            Dock::Left => Rect::from_min_size(Pos2::new(win.left() + gap, win.center().y - h / 2.0), Vec2::new(w, h)),
            Dock::Right => Rect::from_min_size(Pos2::new(win.right() - gap - w, win.center().y - h / 2.0), Vec2::new(w, h)),
        }
    }

    /// 展开面板（页面内容）的左上角。
    pub(crate) fn content_origin(&self, win: Rect) -> Pos2 {
        self.island_rect(win, self.expanded_width(), self.expanded_height()).min
    }

    /// 侧边停靠的命中区域：收起时是胶囊外扩几点并一直延伸到屏幕边缘，展开时是整个面板。
    pub(crate) fn side_hit_rect(&self, win: Rect, p: f32) -> Rect {
        let rect = if p > 0.04 || self.open {
            self.island_rect(win, self.expanded_width(), self.expanded_height()).expand2(Vec2::new(4.0, 2.0))
        } else {
            let (w, h) = (self.w.pos, self.h.pos);
            self.island_rect(win, w, h).expand2(Vec2::new(4.0, 6.0))
        };
        // 光标推到屏幕最边上也要能唤出岛（胶囊离边缘有一小段空隙）。
        match self.dock() {
            Dock::Left => Rect::from_min_max(Pos2::new(win.left(), rect.top()), rect.max),
            Dock::Right => Rect::from_min_max(rect.min, Pos2::new(win.right(), rect.bottom())),
            Dock::Top => rect,
        }
    }

    /// 毛玻璃取样点（画布坐标）：岛体外侧上下左右各取几处估计身后明暗。
    pub fn backdrop_points(&self) -> Vec<(f32, f32)> {
        match self.dock() {
            Dock::Top => {
                let cx = crate::app::WIN_W / 2.0;
                let bottom = crate::app::WIN_H - 6.0;
                vec![(cx - 270.0, 6.0), (cx - 270.0, 26.0), (cx + 270.0, 6.0), (cx + 270.0, 26.0), (cx - 130.0, bottom), (cx, bottom), (cx + 130.0, bottom)]
            }
            dock => {
                let (near, far) = if dock == Dock::Left { (6.0, SIDE_W - 6.0) } else { (SIDE_W - 6.0, 6.0) };
                let mid = if dock == Dock::Left { 26.0 } else { SIDE_W - 26.0 };
                let cy = SIDE_H / 2.0;
                vec![(near, cy - 170.0), (mid, cy - 170.0), (near, cy + 170.0), (mid, cy + 170.0), (far, cy - 60.0), (far, cy), (far, cy + 60.0)]
            }
        }
    }

    /// 竖向胶囊的内容。`c` 的原点是胶囊左上角。
    pub(crate) fn side_bar(&mut self, c: &Card, width: f32, height: f32, env: &Env) {
        if self.side_player_is_folded() {
            let center = width / 2.0;
            let lyrics = self.island_lyrics_available();
            let icon_y = if lyrics { 18.0 } else { height / 2.0 };
            let hit = c.hit(0.0, 0.0, width, height, "side-music-unfold").on_hover_text("展开音乐播放器");
            let (hover, _) = c.hover_t(&hit);
            c.circle(center, icon_y, 12.0, Color32::from_white_alpha((hover*24.0) as u8));
            paint::note(c, center, icon_y, 15.0, theme::WHITE);
            if lyrics {
                self.draw_side_lyric(c, center, 34.0, height - 6.0, self.media.position as f32, true);
            }
            if hit.clicked() { self.show_page(0); }
            return;
        }
        let items = self.side_items();
        let total: f32 = items.iter().map(|i| i.height()).sum();
        let cx = width / 2.0;
        let fitted = c.scaled(cx, height / 2.0, self.side_bar_scale(height, total));
        let c=&fitted;
        let mut y = (height - total) / 2.0;
        for item in items {
            let h = item.height();
            let mid = y + h / 2.0;
            match item {
                SideItem::Clock => {
                    let (hour, minute) = side_clock(&self.settings.clock_text(&env.clock));
                    c.text(cx, mid - 1.0, &hour, 13.5, W::Bold, theme::WHITE, H::Center);
                    c.text(cx, mid + 13.0, &minute, 13.5, W::Bold, theme::WHITE, H::Center);
                }
                SideItem::Battery => paint::battery(c, cx, mid, env.battery, env.charging, 0.72),
                SideItem::Music if self.media.is_music() => paint::playback_bars(c, cx, mid, self.t, self.media.playing, theme::WHITE),
                SideItem::Music => paint::note(c, cx, mid, 13.0, theme::WHITE),
                SideItem::Lyric => self.draw_side_lyric(c, cx, y + 5.0, y + h - 5.0, self.media.position as f32, true),
                SideItem::FocusTimer => {
                    let seconds = self.pomo.remaining.ceil().max(0.0) as u32;
                    let color = if self.pomo.phase == Phase::Focus { theme::RED_TOP } else { theme::GREEN };
                    c.circle(cx, mid - 13.0, 3.0, color);
                    c.centered_text(cx, mid - 2.0, &format!("{:02}′", seconds / 60), 11.0, W::Bold, theme::WHITE, H::Center);
                    c.centered_text(cx, mid + 10.0, &format!("{:02}″", seconds % 60), 11.0, W::Bold, theme::WHITE, H::Center);
                }
                SideItem::Focus => crate::focus_bar::shortcut(c, cx, mid),
                SideItem::Pet => self.pet.draw(&c.shifted(0.0, mid - 15.0), cx, self.t),
                SideItem::Shortcut(page) => match page {
                    2 => paint::resident_water(c, cx, mid),
                    3 => paint::resident_shelf(c, cx, mid),
                    9 => paint::resident_clipboard(c, cx, mid),
                    4 => paint::resident_todo(c, cx, mid),
                    5 => paint::resident_mail(c, cx, mid),
                    6 => paint::target(c, cx, mid, theme::SOFT_GREEN),
                    8 => {
                        let icon = self.steam.game().and_then(|game|game.icon.as_deref())
                            .or_else(||self.steam.installed_games.first().and_then(|game|game.icon.as_deref()))
                            .or_else(||self.steam.badge().and_then(|(_,game)|game.icon.as_deref()));
                        if let Some(texture) = self.steam_images.get(c.ui.ctx(), icon, None) {
                            c.painter.image(texture, c.rect(cx - 9.0, mid - 9.0, 18.0, 18.0),
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                        } else {
                            crate::extras::device_icon(c, crate::bt::DeviceKind::Gamepad, cx - 9.0, mid - 9.0, 18.0);
                        }
                    }
                    _ => {}
                },
                SideItem::Quota(index, h) => self.draw_quota_side(c, cx, mid, h, index),
                SideItem::Bell => {
                    paint::bell(&c.scaled(cx, mid, 1.1), cx, mid, theme::WHITE, paint::bell_swing(self.t - self.notification_bell_at));
                    c.circle(cx + 4.0, mid - 4.5, 2.0, Color32::from_rgb(255, 72, 72));
                }
            }
            if !matches!(item, SideItem::Quota(..)) {
                if let Some(page) = item.page().filter(|&page| self.settings.page_on(page, self.dock())) {
                    if c.hit(cx - 17.0, y, 34.0, h, ("side-shortcut", page, matches!(item, SideItem::Lyric)))
                        .on_hover_text(crate::app::PAGE_NAMES[page]).clicked()
                    {
                        self.show_page(page);
                    }
                }
            }
            y += h;
        }
    }
}

/// 竖排时间：「09:41」→ 「09」「41」；12 小时制去掉上午 / 下午等文字。
pub(crate) fn side_clock(text: &str) -> (String, String) {
    let digits: String = text.chars().filter(|c| c.is_ascii_digit() || *c == ':').collect();
    match digits.split_once(':') {
        Some((h, m)) if !h.is_empty() => (h.to_string(), m.chars().take(2).collect()),
        _ => ("--".into(), "--".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clock_splits_into_two_lines() {
        assert_eq!(side_clock("09:41"), ("09".into(), "41".into()));
        assert_eq!(side_clock("下午 9:05"), ("9".into(), "05".into()));
        assert_eq!(side_clock("--:--"), ("--".into(), "--".into()));
    }
}
