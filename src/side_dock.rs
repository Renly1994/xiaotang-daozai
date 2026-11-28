//! 左右停靠：岛体贴在屏幕左 / 右边缘，收起时是竖向胶囊，展开后面板从边缘滑出。
//!
//! 展开后的页面内容与顶部停靠完全相同（419 × 140 的横向面板），只是锚点换到侧边；
//! 收起态换成竖排：时间（时 / 分两行）、电池、播放状态、专注倒计时、通知铃铛。
//! AI 额度竖排：环形额度一行一个，余额类图标在上、金额在下。
//! 吉祥物和岛上歌词是横向内容，侧边停靠时不显示。

use crate::app::resident_navigation::Destination;
use crate::app::{App, Env, Phase, BAR_H_MIN, BAR_W_MIN, PILL_GAP, PW};
use crate::model::{Dock, IslandShape, ResidentDate};
use crate::paint::{self, Card, H};
use crate::resident_date::ResidentDay;
use crate::theme::{self, size, W};
use eframe::egui::{Color32, Pos2, Rect, Vec2};

/// 侧边停靠时的透明画布（窗口）大小，逻辑点。
pub const SIDE_W: f32 = 500.0;
pub const SIDE_H: f32 = 600.0;
/// 收起时竖向胶囊的厚度。
pub const SIDE_BAR_W: f32 = 36.0;
#[cfg(feature="sdk-js")]
const PINNED_HEIGHT: f32 = 42.0;
/// 岛体中心能离开屏幕竖直中线的预留：展开面板（最高约 236）与肩部、阴影留在屏幕内。
pub const SIDE_HALF_EXTENT: f32 = 150.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SideItem {
    Clock, Date(f32), Weather, Battery, Music, Focus, Bell, Wireless, Performance(f32), Shortcut(usize),
    #[cfg(feature="sdk-js")]
    Pinned(usize),
    /// AI 额度：账号下标（None 为「+N」）与高度。
    Quota(usize, f32),
}

impl SideItem {
    pub(crate) fn height(self, clock_seconds: bool) -> f32 {
        match self {
            #[cfg(feature="sdk-js")]
            Self::Pinned(_) => PINNED_HEIGHT,
            Self::Shortcut(_)=>28.0,
            Self::Performance(h) | Self::Date(h) => h, Self::Clock => if clock_seconds {46.0}else{32.0}, Self::Weather => 40.0, Self::Wireless=>37.0, Self::Battery => 20.0, Self::Music => 18.0, Self::Focus => 22.0, Self::Bell => 22.0, Self::Quota(_, h) => h }
    }
}

impl App {
    pub(crate) fn dock(&self) -> Dock { self.settings.dock }
    pub(crate) fn side(&self) -> bool { self.settings.dock.is_side() }

    /// 竖向胶囊里依次显示的内容。
    pub(crate) fn side_items(&self) -> Vec<SideItem> {
        let mut items = Vec::new();
        if self.resident_on(crate::app::CALENDAR_PAGE) {items.push(SideItem::Clock);}
        match if self.resident_on(crate::app::CALENDAR_PAGE){self.settings.resident_date}else{ResidentDate::Off} {
            ResidentDate::Off=>{},
            ResidentDate::Gregorian=>items.push(SideItem::Date(20.0)),
            ResidentDate::LunarOnly | ResidentDate::Lunar=>items.push(SideItem::Date(44.0)),
        }
        if self.weather_pill().is_some() {items.push(SideItem::Weather);}
        if !self.wireless_resident().is_empty(){items.push(SideItem::Wireless);}
        if self.resident_on(crate::app::BATTERY_PAGE) && self.resident_power.0.is_some() { items.push(SideItem::Battery); }
        if self.performance_on_bar() {items.push(SideItem::Performance(self.settings.performance_bar.side_height(self.settings.performance_metrics)));}
        if self.resident_on(crate::app::MUSIC_PAGE)&&self.media.is_music() { items.push(SideItem::Music); }
        if self.resident_on(crate::app::FOCUS_PAGE)&&self.pomo.running { items.push(SideItem::Focus); }
        items.extend(self.quota_side_items().into_iter().map(|(index, h)| SideItem::Quota(index, h)));
        if self.notification_bell().is_some() { items.push(SideItem::Bell); }
        #[cfg(feature="sdk-js")]
        items.extend((0..self.pinned_displays.len()).map(SideItem::Pinned));
        let represented=|item:&SideItem| match item {
            SideItem::Clock|SideItem::Date(_)=>Some(crate::app::CALENDAR_PAGE),SideItem::Weather=>Some(crate::app::WEATHER_PAGE),
            SideItem::Battery=>Some(crate::app::BATTERY_PAGE),SideItem::Music=>Some(crate::app::MUSIC_PAGE),SideItem::Focus=>Some(crate::app::FOCUS_PAGE),
            SideItem::Bell=>Some(crate::app::MAIL_PAGE),SideItem::Wireless=>Some(crate::app::BLUETOOTH_PAGE),SideItem::Performance(_)=>Some(crate::app::PERFORMANCE_PAGE),
            SideItem::Quota(..)=>Some(crate::app::QUOTA_PAGE),SideItem::Shortcut(p)=>Some(*p),
            #[cfg(feature="sdk-js")] SideItem::Pinned(i)=>self.pinned_displays.get(*i).and_then(|d|self.status_pages.iter().position(|(id,_)|id==&d.plugin)).map(|i|crate::app::PAGE_NAMES.len()+i),
        };
        for &page in &self.settings.page_order {if self.resident_on(page)&&!items.iter().any(|item|represented(item)==Some(page)){items.push(SideItem::Shortcut(page));}}
        #[cfg(feature="sdk-js")]
        for (i,(id,_)) in self.status_pages.iter().enumerate(){let page=crate::app::PAGE_NAMES.len()+i;if self.plugin_resident(id)&&!items.iter().any(|item|represented(item)==Some(page)){items.push(SideItem::Shortcut(page));}}
        let order=self.component_keys();items.sort_by_key(|item|represented(item).and_then(|page|order.iter().position(|k|*k==self.component_key(page))).unwrap_or(usize::MAX));
        items
    }

    /// 收起时竖向胶囊的长度。
    pub(crate) fn side_bar_len(&self) -> f32 {
        14.0 + self.side_items().iter().map(|i| i.height(self.settings.clock_seconds)).sum::<f32>()
    }

    /// 收起态尺寸（屏幕宽 × 高）。极简模式是贴边的一小段竖线。
    pub(crate) fn side_base_size(&self, minimal: bool) -> (f32, f32) {
        if minimal { (BAR_H_MIN, BAR_W_MIN) } else { (SIDE_BAR_W, self.side_bar_len().min(self.resident_limit())) }
    }

    fn side_gap(&self) -> f32 {
        if self.settings.island_shape == IslandShape::Pill { PILL_GAP } else { 0.0 }
    }

    /// 宽 `w`、高 `h` 的岛体在画布 `win` 里的位置：上下停靠水平居中贴边，
    /// 左右停靠贴边、竖直居中。
    pub(crate) fn island_rect(&self, win: Rect, w: f32, h: f32) -> Rect {
        let gap = self.side_gap();
        match self.dock() {
            Dock::Top => Rect::from_min_size(Pos2::new(win.center().x - w / 2.0, win.top() + gap), Vec2::new(w, h)),
            Dock::Bottom => Rect::from_min_size(Pos2::new(win.center().x - w / 2.0, win.bottom() - gap - h), Vec2::new(w, h)),
            Dock::Left => Rect::from_min_size(Pos2::new(win.left() + gap, win.center().y - h / 2.0), Vec2::new(w, h)),
            Dock::Right => Rect::from_min_size(Pos2::new(win.right() - gap - w, win.center().y - h / 2.0), Vec2::new(w, h)),
        }
    }

    /// 展开面板（页面内容）的左上角。
    pub(crate) fn content_origin(&self, win: Rect) -> Pos2 {
        let height=if self.dock()==Dock::Bottom {self.h.pos}else{self.page_height.pos};
        self.island_rect(win, PW, height).min
    }

    /// 侧边停靠的命中区域：收起时是胶囊外扩几点并一直延伸到屏幕边缘，展开时是整个面板。
    pub(crate) fn side_hit_rect(&self, win: Rect, p: f32, page_height: f32) -> Rect {
        let rect = if p > 0.04 || self.open {
            self.island_rect(win, PW, page_height).expand2(Vec2::new(4.0, 2.0))
        } else {
            let (w, h) = (self.w.pos, self.h.pos);
            self.island_rect(win, w, h).expand2(Vec2::new(4.0, 6.0))
        };
        // 光标推到屏幕最边上也要能唤出岛（胶囊离边缘有一小段空隙）。
        match self.dock() {
            Dock::Left => Rect::from_min_max(Pos2::new(win.left(), rect.top()), rect.max),
            Dock::Right => Rect::from_min_max(rect.min, Pos2::new(win.right(), rect.bottom())),
            Dock::Top | Dock::Bottom => rect,
        }
    }

    /// 毛玻璃取样点（画布坐标）：岛体外侧上下左右各取几处估计身后明暗。
    pub fn backdrop_points(&self, horizontal_width: f32) -> Vec<(f32, f32)> {
        match self.dock() {
            Dock::Top | Dock::Bottom => {
                let cx = horizontal_width / 2.0;
                let bottom = crate::app::WIN_H - 6.0;
                let y=|v| if self.dock()==Dock::Bottom {crate::app::WIN_H-v} else {v};
                vec![(cx - 270.0, y(6.0)), (cx - 270.0, y(26.0)), (cx + 270.0, y(6.0)), (cx + 270.0, y(26.0)), (cx - 130.0, y(bottom)), (cx, y(bottom)), (cx + 130.0, y(bottom))]
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
    pub(crate) fn side_bar(&mut self, c: &Card, width: f32, height: f32, env: &Env, resident_day: Option<&ResidentDay>) {
        let items = self.side_items();
        let total: f32 = items.iter().map(|i| i.height(self.settings.clock_seconds)).sum();
        let cx = width / 2.0;
        let fitted=c.scaled(cx,height/2.0,(self.resident_limit()/(total+14.0)).min(1.0));
        let c=&fitted;
        let mut y = (height - total) / 2.0;
        for item in items {
            let h = item.height(self.settings.clock_seconds);
            let mid = y + h / 2.0;
            match item {
                SideItem::Shortcut(page)=>self.resident_shortcut(c,cx,mid,page),
                SideItem::Clock => {
                    let text=self.settings.clock_text(&env.clock);
                    let (hour, minute) = side_clock(&text);
                    let top=mid-if self.settings.clock_seconds {8.0}else{1.0};
                    c.text(cx, top, &hour, 13.5, W::Bold, theme::WHITE, H::Center);
                    c.text(cx, top + 14.0, &minute, 13.5, W::Bold, theme::WHITE, H::Center);
                    if self.settings.clock_seconds {c.text(cx,top+28.0,text.rsplit(':').next().unwrap_or("--"),12.0,W::Bold,theme::GRAY,H::Center);}
                }
                SideItem::Date(_) => if let Some(day)=resident_day {day.draw_side(c,cx,mid);},
                SideItem::Weather => self.draw_side_weather(c,cx,mid),
                SideItem::Wireless=>self.draw_wireless_side(c,cx,mid),
                SideItem::Battery => paint::battery(c, cx, mid, env.battery, env.charging, 0.72),
                SideItem::Music => paint::playback_bars(c, cx, mid, self.t, self.media.playing, theme::WHITE),
                SideItem::Focus => {
                    let minutes = (self.pomo.remaining / 60.0).ceil().max(0.0) as u32;
                    let color = if self.pomo.phase == Phase::Focus { theme::RED_TOP } else { theme::GREEN };
                    c.circle(cx, mid - 5.0, 3.0, color);
                    c.text(cx, mid + 9.0, &format!("{minutes}′"), size::TINY, W::Semibold, theme::WHITE, H::Center);
                }
                SideItem::Performance(h) => self.side_performance(c,cx,mid,h),
                SideItem::Quota(index, h) => self.draw_quota_side(c, cx, mid, h, index),
                #[cfg(feature="sdk-js")]
                SideItem::Pinned(index) => if let Some(display) = self.pinned_displays.get(index) {
                    let (_, repaint_after, animating) =
                        display.draw(c, cx, mid, display.width(width), true, self.pinned_canvas_theme());
                    self.pinned_frame.include(repaint_after, animating);
                },
                SideItem::Bell => {
                    if let Some(at)=self.notification_bell(){
                        paint::bell(&c.scaled(cx, mid, 1.1), cx, mid, theme::WHITE, paint::bell_swing(self.t - at));
                        c.circle(cx + 4.0, mid - 4.5, 2.0, Color32::from_rgb(255, 72, 72));
                    }
                }
            }
            let page=match item {
                SideItem::Clock | SideItem::Date(_)=>Some(crate::app::CALENDAR_PAGE),
                SideItem::Battery=>Some(crate::app::BATTERY_PAGE),
                SideItem::Music=>Some(crate::app::MUSIC_PAGE),
                SideItem::Focus=>Some(crate::app::FOCUS_PAGE),
                SideItem::Bell=>Some(crate::app::MAIL_PAGE),
                _=>None,
            };
            if let Some(page)=page {c.page_link(cx-17.0,y,34.0,h,("side-page",page,std::mem::discriminant(&item)),Destination::Page(page));}
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
