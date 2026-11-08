//! 五个页面：音乐 → 专注 → 饮水 → 置物 → 待办。
//! 坐标全部是面板局部逻辑点（419×140），文字 y 是基线。

use crate::anim::{self, lerp, smoothstep, Spring};
use crate::app::{App, Cmd, Env, Phase, PH};
use crate::mail;
use crate::paint::{self, capsule, mix, Card, H};
use crate::smtc::Transport;
use crate::theme::{self, size, W};
use eframe::egui::{self, Color32, Pos2, Rect};
use std::f32::consts::{FRAC_PI_2, TAU};

/// 剪贴板分栏一屏显示的条数。
pub(crate) const CLIP_ROWS: usize = 4;

fn mmss(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn music_time(media: &crate::smtc::NowPlaying, position: f64) -> (String, String, Color32) {
    let has_music = !media.title.is_empty();
    let remaining = if has_music && media.duration <= 0.0 { "--:--".to_owned() }
        else { format!("-{}", mmss((media.duration-position).max(0.0))) };
    let color = if has_music { theme::WHITE } else { Color32::from_rgb(78,78,82) };
    (mmss(position), remaining, color)
}

/// 由标题生成稳定的封面渐变色。
fn cover_colors(title: &str) -> (Color32, Color32) {
    let mut h: u64 = 1469598103934665603;
    for b in title.bytes() {
        h = (h ^ b as u64).wrapping_mul(1099511628211);
    }
    let palette = [
        (Color32::from_rgb(255, 94, 98), Color32::from_rgb(255, 153, 102)),
        (Color32::from_rgb(86, 204, 242), Color32::from_rgb(47, 128, 237)),
        (Color32::from_rgb(168, 85, 247), Color32::from_rgb(236, 72, 153)),
        (Color32::from_rgb(52, 211, 153), Color32::from_rgb(16, 120, 200)),
        (Color32::from_rgb(251, 191, 36), Color32::from_rgb(239, 68, 68)),
    ];
    palette[(h % palette.len() as u64) as usize]
}

impl App {
    pub(crate) fn shelf_columns_for_width(width:f32)->usize {
        ((width-72.0)/68.0).floor().max(1.0) as usize
    }

    pub(crate) fn shelf_rows_for_height(height:f32)->usize {
        if height>=350.0 {3}else if height>=272.0 {2}else{1}
    }

    pub(crate) fn shelf_page_capacity(&self)->usize {
        if self.side() { return 5; }
        Self::shelf_columns_for_width(self.expanded_width())*Self::shelf_rows_for_height(self.expanded_height())
    }

    pub(crate) fn shelf_list_bounds(&self)->Rect {
        if self.side() {Rect::from_min_size(Pos2::new(10.0,28.0),egui::vec2(136.0,self.expanded_height()-46.0))}
        else {Rect::from_min_size(Pos2::new(36.0,28.0),egui::vec2(self.expanded_width()-72.0,self.expanded_height()-46.0))}
    }

    pub(crate) fn shelf_last_page_start(&self)->usize {
        let (width,height)=self.settings.dock_size(self.dock());
        let capacity=Self::shelf_columns_for_width(width)*Self::shelf_rows_for_height(height);
        let count=self.shelf.items.len();
        if count<=capacity {0}else{((count-1)/capacity)*capacity}
    }

    // ================= 音乐 =================
    pub(crate) fn page_music(&mut self, c: &Card, _env: &Env, cmds: &mut Vec<Cmd>) {
        let media = self.media.clone();
        let has_music = !media.title.is_empty();
        let lyrics = has_music && self.show_lyrics;
        let (art_y, art_size) = if lyrics { (23.0, 44.0) } else { (13.0, 62.0) };
        self.music_cover(c, &media.title, 44.0, art_y, art_size);
        if has_music {
            let (x, width, title_y, artist_y, title_size) = if lyrics {
                (99.0, 123.0, 41.0, 57.0, 12.0)
            } else {
                (121.0, 220.0, 53.0, 67.0, 14.0)
            };
            c.text(x, title_y, &c.fit(&media.title, title_size, W::Bold, width), title_size, W::Bold, theme::WHITE, H::Left);
            c.text(x, artist_y, &c.fit(&media.artist, size::BODY, W::Regular, width), size::BODY, W::Regular, theme::GRAY, H::Left);
        } else {
            c.text(121.0, 47.0, "暂无音乐", 14.0, W::Bold, theme::WHITE, H::Left);
        }
        if lyrics { self.draw_music_lyrics(c); }

        let track = if lyrics { (44.0, 176.0, 82.0) } else { (96.0, 221.0, 92.6) };
        let position = self.music_seek(c, &media, track, false, cmds);
        let (elapsed, remaining, color) = music_time(&media, position);
        let (y, font, right) = if lyrics { (100.0, 10.0, 220.0) } else { (97.0, 15.0, 375.0) };
        c.text(44.0, y, &elapsed, font, W::Regular, color, H::Left);
        c.text(right, y, &remaining, font, W::Regular, color, H::Right);

        let controls = [74.0, 162.0, 210.5, 258.0, 367.0].map(|x| (x, 118.0));
        self.music_controls(c, &media, controls, false, cmds);
        if has_music { self.music_island_lyrics(c, 279.0, 108.0); }
    }

    fn music_cover(&self, c: &Card, title: &str, x: f32, y: f32, width: f32) {
        if let Some((image, texture)) = &self.music_artwork {
            let [w, h] = image.size;
            let (uw, vh) = if w > h { (h as f32 / w as f32, 1.0) } else { (1.0, w as f32 / h as f32) };
            let uv = Rect::from_center_size(Pos2::new(0.5, 0.5), egui::vec2(uw, vh));
            c.painter.add(egui::epaint::RectShape::filled(c.rect(x, y, width, width), 10.0*c.s, Color32::WHITE).with_texture(texture.id(), uv));
        } else if !title.is_empty() {
            let (a, b) = cover_colors(title);
            let cover = c.clipped(x, y, width, width);
            cover.vgrad(x, y, width, width, 10.0, a, b);
            paint::note(&cover, x + width/2.0, y + width/2.0, width*0.39, Color32::from_white_alpha(235));
        } else {
            c.fill(x, y, width, width, if width < 50.0 { 8.0 } else { 13.0 }, Color32::from_rgb(25,25,27));
            paint::note(c, x + width/2.0, y + width/2.0, width*26.0/62.0, Color32::from_rgb(110,110,114));
        }
    }

    fn music_seek(&mut self, c: &Card, media: &crate::smtc::NowPlaying,
        (start, length, cross): (f32, f32, f32), vertical: bool, cmds: &mut Vec<Cmd>) -> f64
    {
        let has_music = !media.title.is_empty();
        let bar = if vertical {
            c.hit_drag(cross - 12.0, start - 4.0, 24.0, length + 8.0, "m-seek")
        } else {
            c.hit_drag(start - 4.0, cross - 7.0, length + 8.0, 14.0, "m-seek")
        };
        let seek = if has_music && media.duration > 0.0 && (bar.is_pointer_button_down_on() || bar.clicked() || bar.drag_stopped()) {
            bar.interact_pointer_pos().or_else(|| c.ui.input(|input| input.pointer.interact_pos())).map(|pointer| {
                let local = c.to_local(pointer);
                let along = if vertical { local.y } else { local.x };
                f64::from(((along - start) / length).clamp(0.0, 1.0)) * media.duration
            })
        } else { None };
        let position = seek.unwrap_or(media.position);
        let played = if media.duration > 0.0 { length * (position / media.duration).clamp(0.0, 1.0) as f32 } else { 0.0 };
        let hover = if has_music { c.hover_t(&bar).0 } else { 0.0 };
        let thickness = lerp(3.5, 5.0, hover);
        let fill = |span, color| {
            if vertical { c.fill(cross - thickness/2.0, start, thickness, span, thickness/2.0, color); }
            else { c.fill(start, cross - thickness/2.0, span, thickness, thickness/2.0, color); }
        };
        let head = start + played;
        let (head_x, head_y) = if vertical { (cross, head) } else { (head, cross) };
        fill(length, if has_music { Color32::from_gray(90) } else { theme::TRACK });
        if self.settings.skin == crate::model::Skin::Fluid {
            let hot = if bar.is_pointer_button_down_on() { 1.0 } else if media.playing { 0.62 } else { 0.45 };
            if has_music && played > 0.0 {
                fill(played, self.fluid.tint_color(0.78));
                paint::chase(c, start, head, cross, thickness, self.fluid.tint_color(0.30), hot, vertical);
            }
            if c.live {
                let point = c.pos(head_x, head_y) - self.fluid_origin;
                self.fluid.set_seek((has_music && played > 0.0).then_some((point.x, point.y)), hot);
            }
        } else {
            if played > 0.0 { fill(played.max(thickness), theme::WHITE); }
            if hover > 0.01 { c.faded(hover).circle(head_x, head_y, 4.5, theme::WHITE); }
        }
        if bar.clicked() || bar.drag_stopped() {
            if let Some(position) = seek { cmds.push(Cmd::Media(Transport::Seek(position))); }
        }
        position
    }

    fn music_controls(&mut self, c: &Card, media: &crate::smtc::NowPlaying, positions: [(f32, f32); 5],
        vertical: bool, cmds: &mut Vec<Cmd>)
    {
        let has_music = !media.title.is_empty();
        let ink = if has_music { theme::WHITE } else { Color32::from_rgb(88,88,92) };
        let playing = c.ui.ctx().animate_bool_with_time(egui::Id::new("np-playing"), media.playing, 0.22);
        let [(words_x, words_y), (prev_x, prev_y), (play_x, play_y), (next_x, next_y), (volume_x, volume_y)] = positions;
        let button = |x, y, width, id| {
            let response = c.hit(x-width/2.0, y-12.0, width, 24.0, id);
            let (hover, pressed) = c.hover_t(&response);
            if has_music && hover > 0.01 { c.faded(hover).circle(x, y, 12.5, Color32::from_white_alpha(20)); }
            (response, c.scaled(x, y, 1.0-0.1*pressed))
        };

        if vertical && self.show_lyrics { c.fill(words_x-12.0, words_y-12.0, 24.0, 24.0, 7.0, theme::BTN); }
        let (words, words_card) = button(words_x, words_y, 26.0, "m-lyr");
        words_card.text(words_x, words_y+5.0, "词", 14.0, W::Semibold,
            if !has_music { ink } else if self.show_lyrics { theme::WHITE } else { theme::GRAY }, H::Center);
        let (prev, prev_card) = button(prev_x, prev_y, 30.0, "m-prev");
        paint::skip(&prev_card, prev_x, prev_y, -1.0, ink);
        let (play, play_card) = button(play_x, play_y, 30.0, "m-play");
        if vertical { play_card.circle(play_x, play_y, 13.0, theme::BTN); }
        paint::play_pause(&play_card, play_x+0.5, play_y, 15.0, playing, ink);
        let (next, next_card) = button(next_x, next_y, 30.0, "m-next");
        paint::skip(&next_card, next_x, next_y, 1.0, ink);
        let (volume_button, volume_card) = button(volume_x, volume_y, 26.0, "m-volume");
        let (volume, muted) = self.volume_ui.unwrap_or((0, false));
        paint::speaker(&volume_card, volume_x, volume_y, muted, ink);
        if self.volume_ui.is_some() && volume_button.on_hover_text(if muted {"取消静音"} else {"静音"}).clicked() {
            self.volume_ui = Some((volume, !muted));
            self.volume_quiet_until = self.t + 0.9;
            cmds.push(Cmd::ToggleMute);
        }
        if has_music {
            if prev.on_hover_text("上一首").clicked() { cmds.push(Cmd::Media(Transport::Prev)); }
            if play.on_hover_text(if media.playing {"暂停"} else {"播放"}).clicked() { cmds.push(Cmd::Media(Transport::Toggle)); }
            if next.on_hover_text("下一首").clicked() { cmds.push(Cmd::Media(Transport::Next)); }
        }
        if words.on_hover_text(if self.show_lyrics {"隐藏歌词"} else {"显示歌词"}).clicked() {
            if has_music {
                self.show_lyrics = !self.show_lyrics;
                self.settings.lyrics_enabled = self.show_lyrics;
                self.settings.save();
            } else { self.say("先放一首歌吧"); }
        }
    }

    fn music_island_lyrics(&mut self, c: &Card, x: f32, y: f32) {
        let (button, card) = capsule(c, x, y, 68.0, 20.0, "m-island-lyrics",
            if self.settings.island_lyrics { Color32::from_gray(39) } else { Color32::from_gray(22) }, Color32::from_gray(47));
        card.text(x+34.0, y+14.0, "岛上歌词", 10.0, W::Semibold,
            if self.settings.island_lyrics { theme::WHITE } else { theme::GRAY }, H::Center);
        if button.on_hover_text(if self.settings.island_lyrics {"关闭收起时的歌词"} else {"收起时显示当前歌词"}).clicked()
        {
            self.settings.island_lyrics = !self.settings.island_lyrics;
            self.settings.save();
        }
    }

    pub(crate) fn side_music_panel(&mut self, c: &Card, env: &Env, cmds: &mut Vec<Cmd>) {
        let panel_width = self.expanded_width();
        let panel_height = self.expanded_height();
        let scale = (panel_height / 270.0).min(1.0);
        let fitted = c.placed((0.0, 0.0), (0.0, 0.0), scale);
        let c = &fitted;
        let (width, height) = (panel_width / scale, panel_height / scale);
        let media = self.media.clone();
        let compact = height < 365.0;
        let margin = 13.0;
        let center = width / 2.0;
        let header = if compact { 103.0 } else { 138.0 };
        let footer = height - 70.0;
        let art_y = if compact { 32.0 } else { 43.0 };
        let art_size = if compact { 30.0 } else { 36.0 };
        let date_left = if self.battery_on_dock() && env.battery.is_some() {
            paint::battery(c, 18.0, 18.0, env.battery, env.charging, 0.58);
            29.0
        } else { margin };
        if self.clock_on_dock() {
            c.centered_text(date_left, 18.0, &c.fit(&env.date, 10.0, W::Regular, width-date_left-49.0),
                10.0, W::Regular, theme::GRAY, H::Left);
        }
        self.dock_drag_handle(c, (8.0, 5.0, width - 58.0, 26.0), cmds);
        self.page_gear(c, width-14.0, self.page_settings_tab());
        let fold_x = width-39.0;
        let fold = c.hit(fold_x-10.0, 6.0, 20.0, 24.0, "side-music-fold").on_hover_text("收起播放器");
        let fold_ink = if fold.hovered() { theme::WHITE } else { theme::GRAY };
        c.path(&[(fold_x-3.0, 15.5), (fold_x, 18.5), (fold_x+3.0, 15.5)], 1.2, fold_ink);
        c.seg((fold_x-3.0, 21.0), (fold_x+3.0, 21.0), 1.2, fold_ink);
        if fold.clicked() { self.fold_side_player(); }

        self.music_cover(c, &media.title, margin, art_y, art_size);
        let clock = self.settings.clock_text(&env.clock);
        let split = clock.rsplit_once(' ');
        let (period, time) = split.unwrap_or(("", clock.as_str()));
        if self.clock_on_dock() {
            c.centered_text(width-margin, art_y+6.0, period, 9.0, W::Regular, theme::GRAY, H::Right);
            c.text(width-margin, art_y+27.0, time, 18.0, W::Bold, theme::WHITE, H::Right);
        }
        let title = if media.title.is_empty() {"暂无音乐"} else {media.title.as_str()};
        let title_y = art_y+art_size+if compact {13.0} else {23.0};
        c.scrolling_title((margin, title_y, width - 2.0 * margin), title, 14.0, self.t, "side-song-title");
        if !media.title.is_empty() {
            c.centered_text(center, title_y+17.0, &c.fit(&media.artist, 11.0, W::Regular, width-2.0*margin),
                11.0, W::Regular, theme::GRAY, H::Center);
        }

        for y in [header, footer] {
            c.seg((margin, y), (width-margin, y), 0.7, Color32::from_gray(39));
        }
        let track_x = 25.0;
        let start = header + 27.0;
        let end = (footer - 28.0).max(start + 12.0);
        let position = self.music_seek(c, &media, (start, end-start, track_x), true, cmds);
        let (elapsed, remaining, color) = music_time(&media, position);
        c.centered_text(track_x, start-11.0, &elapsed, 9.5, W::Regular, color, H::Center);
        c.centered_text(track_x, end+11.0, &remaining, 9.5, W::Regular, color, H::Center);

        let controls_x = width - 24.0;
        let step = ((footer-header-15.0)/4.0).clamp(19.0, 34.0);
        let y = (header+footer)/2.0 - step*1.5;
        self.music_controls(c, &media, [(25.0,footer+26.0), (controls_x,y), (controls_x,y+step),
            (controls_x,y+step*2.0), (controls_x,y+step*3.0)], true, cmds);
        if !media.title.is_empty() { self.music_island_lyrics(c, width-79.0, footer+16.0); }
        if self.show_lyrics && !media.title.is_empty() {
            self.draw_side_lyric(c, center, header+12.0, footer-12.0, position as f32, false);
        }
        self.nav_at(c, width, height-17.0, height-16.0);
    }

    pub(crate) fn draw_side_lyric(&mut self, c: &Card, center: f32, top: f32, bottom: f32, media_position: f32, island: bool) {
        let position = media_position - self.settings.lyric_delay_ms as f32 / 1000.0;
        let (text, line_time, next_time) = if let Some(lyrics) = &self.ly.lyrics {
            if let Some(index) = lyrics.synced.then(|| lyrics.index_at(position)).flatten() {
                let line = &lyrics.lines[index];
                (line.text.trim(), line.t, lyrics.lines.get(index+1).map(|line| line.t))
            } else { ("", 0.0, None) }
        } else { ("", 0.0, None) };
        let reduced = !crate::bar_layout::animations_enabled() || c.ui.style().animation_time == 0.0;
        let line = &mut self.ly.side_line;
        if reduced {
            if line.text != text { line.text = text.to_owned(); }
            line.opacity = 1.0;
        } else if line.text != text {
            line.opacity = (line.opacity - self.dt / 0.12).max(0.0);
            if line.opacity == 0.0 { line.text = text.to_owned(); }
            c.ui.ctx().request_repaint();
        } else if line.opacity < 1.0 {
            line.opacity = (line.opacity + self.dt / 0.16).min(1.0);
            c.ui.ctx().request_repaint();
        }
        if line.text.is_empty() || line.opacity <= 0.0 {
            if island {
                let label = if self.ly.lyrics.as_ref().is_some_and(|lyrics| lyrics.index_at(position).is_some()) { "间奏" } else { "前奏" };
                if text.is_empty() {
                    c.vertical_text(center, (top + bottom) / 2.0, label, 11.0, 16.0, W::Regular, theme::GRAY);
                }
            } else if !self.ly.lyrics.as_ref().is_some_and(|lyrics| lyrics.synced) {
                let label = if self.ly.loading { "正在查找歌词" } else { "暂无同步歌词" };
                let pitch = ((bottom - top) / label.chars().count() as f32).clamp(7.0, 16.0);
                c.vertical_text(center, (top + bottom) / 2.0, label, pitch.min(11.0), pitch, W::Regular, theme::GRAY);
                if self.ly.error.is_some() && c.hit(center-12.0, top, 24.0, bottom-top, "side-lyrics-retry")
                    .on_hover_text("重新查询当前歌曲的歌词").clicked()
                {
                    self.ly.key.clear();
                }
            }
            return;
        }
        let height = (bottom-top).max(1.0);
        let count = line.text.chars().count() as f32;
        let pitch = (height / count).clamp(14.0, if island { 19.0 } else { 25.0 });
        let font = (pitch - 5.0).clamp(10.0, if island { 13.0 } else { 17.0 });
        let overflow = (count * pitch - height).max(0.0);
        // Long lines stay in one column and travel slowly through a clipped lane.
        let time = (position-line_time).max(0.0);
        let duration = next_time.map(|next| next-line_time).unwrap_or(overflow/22.0).max(1.0);
        if line.text == text {
            line.scroll = if reduced { 0.0 } else { (overflow * time / duration).min(time * 26.0).min(overflow) };
        }
        let y = top + (height - count*pitch).max(0.0)/2.0 + pitch/2.0 - line.scroll.min(overflow);
        let lyric = c.clipped(center-25.0, top, 50.0, height).faded(line.opacity);
        for (i, character) in line.text.chars().enumerate() {
            let row = y + i as f32*pitch;
            if row < top - pitch || row > bottom + pitch { continue; }
            let mut bytes = [0; 4];
            lyric.centered_text(center, row, character.encode_utf8(&mut bytes), font, W::Semibold, theme::WHITE, H::Center);
        }
    }

    fn draw_music_lyrics(&mut self,c:&Card) {
        let col=c.clipped(243.0,28.0,136.0,74.0);
        c.seg((233.0,32.0),(233.0,98.0),1.0,Color32::from_gray(35));
        if let Some(lyrics)=&self.ly.lyrics {
            let current=if lyrics.synced {lyrics.index_at(self.lyric_position())}else{Some(0)};
            let target=current.unwrap_or(0);
            let id=egui::Id::new("music-lyrics-scroll");
            if !self.ly.scroll_animation_started {
                c.ui.ctx().animate_value_with_time(id,target as f32,0.0);
                self.ly.scroll_animation_started=true;
            }
            let position=c.ui.ctx().animate_value_with_time(id,target as f32,0.3);
            let first=(position.floor() as usize).saturating_sub(2);
            for (index,line) in lyrics.lines.iter().enumerate().skip(first).take(6) {
                let distance=index as f32-position;
                if distance.abs()>2.2 {continue;}
                let active=current==Some(index);
                let ink=if active {theme::WHITE}else{theme::GRAY};
                let row=col.faded((1.0-distance.abs()*0.35).clamp(0.0,1.0));
                let mut job=egui::text::LayoutJob::simple(line.text.clone(),row.font(if active {12.0}else{10.0},if active {W::Semibold}else{W::Regular}),ink,132.0*c.s);
                job.wrap.max_rows=if active {3}else{1};
                job.wrap.break_anywhere=true;
                let galley=row.painter.layout_job(job);
                let y=64.0+distance*29.0-galley.size().y/(2.0*c.s);
                row.painter.galley(row.pos(246.0,y),galley,ink);
            }
            if !lyrics.synced {col.text(246.0,39.0,"文本歌词",8.0,W::Regular,theme::GRAY,H::Left);}
        } else {
            let status=if self.ly.loading {"正在查找歌词…"}else if let Some(error)=&self.ly.error {error.label()}else{"暂无歌词"};
            let galley=col.painter.layout(status.into(),col.font(10.0,W::Regular),theme::GRAY,130.0*c.s);
            col.painter.galley(col.pos(246.0,61.0-galley.size().y/(2.0*c.s)),galley,theme::GRAY);
            if self.ly.error.is_some() {
                col.text(246.0,87.0,"点击重试",9.0,W::Regular,theme::GRAY,H::Left);
                if c.hit(243.0,28.0,136.0,74.0,"lyrics-retry").on_hover_text("重新查询当前歌曲的歌词").clicked(){self.ly.key.clear();}
            }
        }
    }

    // ================= 专注（番茄钟） =================
    pub(crate) fn page_focus(&mut self, c: &Card, _env: &Env) {
        let side = self.side();
        let settings = if side { c.placed((90.0, 8.0), (104.0, 2.0), 1.0) } else { c.clone() };
        {
        let c = &settings;
        if c.hit(90.0,8.0,42.0,21.0,"focus-settings").on_hover_text("专注设置").clicked(){self.open_settings_category(2);}
        c.text(111.0,23.0,"设置",10.5,W::Regular,theme::GRAY,H::Center);
        }
        let (rx, ry, rr) = if side { (78.0, 57.0, 32.0) } else { (79.0, 74.0, 32.0) };
        let frac = self.ring_shown.pos.clamp(0.0, 1.0);
        let run = c.ui.ctx().animate_bool_with_time(egui::Id::new("pomo-run"), self.pomo.running, 0.3);
        let resting=self.pomo.phase!=Phase::Focus;
        let (top,bottom)=if resting{(theme::SOFT_GREEN,mix(theme::SOFT_GREEN,Color32::BLACK,0.2))}else{(theme::RED_TOP,theme::RED)};
        // 外圈柔光；计时中缓慢呼吸
        let breathe = 0.5 + 0.5 * ((self.t * 2.2).sin() as f32);
        let glow = 0.30 + 0.25 * run * breathe;
        c.glow(rx, ry, 48.0, paint::alpha(bottom, glow * 0.55));
        c.ring(rx, ry, rr, 7.0, if resting{mix(theme::CARD,top,0.18)}else{Color32::from_rgb(58,22,26)});
        // 渐变进度环随当前阶段配色。
        if frac > 0.001 {
            let steps = (120.0 * frac).ceil().max(2.0) as usize;
            let a0 = -FRAC_PI_2;
            let mut prev = (rx + rr * a0.cos(), ry + rr * a0.sin());
            for i in 1..=steps {
                let a = a0 + TAU * frac * i as f32 / steps as f32;
                let next = (rx + rr * a.cos(), ry + rr * a.sin());
                let t = ((prev.1 + next.1) * 0.5 - (ry - rr)) / (2.0 * rr);
                c.seg(prev, next, 7.0, mix(top, bottom, t));
                prev = next;
            }
            // 圆头
            c.circle(rx, ry - rr, 3.5, top);
            c.circle(prev.0, prev.1, 3.5, mix(top, bottom, (prev.1 - (ry - rr)) / (2.0 * rr)));
        }
        let hand = if self.pomo.running { (self.t as f32 * 1.2) % TAU } else { 0.0 };
        if resting {
            let t=if self.pomo.running && crate::bar_layout::animations_enabled(){self.t as f32}else{0.0};
            crate::focus_bar::tea(&c.scaled(rx,ry,2.2),rx,ry,frac,t);
        }else{paint::stopwatch(c, rx, ry + 0.5, 15.5, hand * run, theme::WHITE);}

        let secs = self.pomo.remaining.max(0.0).ceil() as u32;
        let (timer_x, timer_y, reset_x) = if side { (78.0, 123.0, 131.0) } else { (264.0, 61.0, 329.0) };
        c.text(timer_x, timer_y, &format!("{:02}:{:02}", secs / 60, secs % 60), size::HERO + 1.0, W::Bold, if resting{theme::SOFT_GREEN}else{theme::WHITE}, H::Center);
        c.text(timer_x, timer_y + 18.0, self.pomo.phase.label(), size::SMALL, W::Regular, theme::GRAY, H::Center);

        // 重置：点一下图标转一圈
        let reset_y = timer_y - 11.0;
        let r = c.hit(reset_x-12.0, reset_y-12.0, 24.0, 24.0, "f-reset");
        let (h, p) = c.hover_t(&r);
        let rc = c.scaled(reset_x, reset_y, 1.0 - 0.1 * p);
        rc.circle(reset_x, reset_y, 11.0, mix(Color32::from_rgb(28, 28, 30), Color32::from_rgb(50, 50, 54), h));
        paint::reset(&rc, reset_x, reset_y, -self.reset_spin.pos, Color32::from_gray(230));
        if r.clicked() {
            self.pomo.remaining = self.phase_secs(self.pomo.phase);
            self.pomo.running = false;
            self.reset_spin.target += TAU;
        }

        for (phase,label,top_x,side_y,id) in [(Phase::Focus,"专注",148.0,151.0,"f-focus"),(Phase::Rest,"休息",292.0,226.0,"f-rest")] {
            let (x, y, width, height) = if side { (46.0, side_y, 64.0, 28.0) } else { (top_x, 94.0, 88.0, 25.0) };
            let selected=self.pomo.phase==phase;
            let accent=if phase==Phase::Focus{theme::RED_TOP}else{theme::SOFT_GREEN};
            let background=if selected{mix(theme::BTN,accent,0.22)}else{theme::BTN};
            let (response,button)=capsule(c,x,y,width,height,id,background,if selected{mix(theme::BTN_HOVER,accent,0.25)}else{theme::BTN_HOVER});
            let color = if selected { mix(accent, theme::WHITE, 0.5) } else { theme::WHITE };
            if side {
                button.centered_text(x + width/2.0, y + height/2.0, label, size::BODY, W::Semibold, color, H::Center);
            } else {
                button.text(x+width/2.0,110.0,label,size::BODY,W::Semibold,color,H::Center);
            }
            if response.clicked(){self.set_phase(phase);}
        }
        let (play_x, play_y) = if side { (78.0, 202.5) } else { (264.0, 106.5) };
        let (b2, c2) = capsule(c, play_x - 18.0, play_y - 12.5, 36.0, 25.0, "f-start", theme::RED, Color32::from_rgb(255, 82, 98));
        paint::play_pause(&c2, play_x, play_y, 11.0, run, theme::WHITE);
        let b2=b2.on_hover_text(if self.pomo.running{"暂停计时"}else{"开始计时"});
        if b2.clicked() {
            if self.pomo.remaining <= 0.0 {
                self.pomo.remaining = self.phase_secs(self.pomo.phase);
            }
            self.pomo.running = !self.pomo.running;
        }
    }

    pub(crate) fn set_phase(&mut self, ph: Phase) {
        if self.pomo.phase==ph {return;}
        self.pomo.phase = ph;
        self.pomo.remaining = self.phase_secs(ph);
        self.pomo.running = false;
    }

    // ================= 饮水 =================
    fn next_hint(&self, env: &Env) -> String {
        let left = (self.settings.water_minutes as f64 * 60.0 - (self.t - self.last_reminder)).max(0.0) as u32 / 60;
        let total = env.hour() as u32 * 60 + env.minute() + left;
        format!("{:02}:{:02}", (total / 60) % 24, total % 60)
    }

    pub(crate) fn page_water(&mut self, c: &Card, env: &Env) {
        let side = self.side();
        let metrics = if side { c.placed((42.0, 35.0), (11.0, 12.0), 0.82) } else { c.clone() };
        let pct = self.water.percent();
        let f = self.water_ring.pos.clamp(0.0, 1.0);
        {
        let c = &metrics;
        let liters = self.water_ml / 1000.0;
        let ww = c.text(44.0, 61.0, &format!("{liters:.1}"), size::HERO, W::Bold, theme::WHITE, H::Left);
        c.text(44.0 + ww + 5.0, 60.0, &format!("/ {:.1} L", self.water.goal_ml as f32 / 1000.0), 11.0, W::Semibold, theme::GRAY, H::Left);

        // 提醒开关：旋钮弹簧滑动
        let sw = c.hit(160.0, 35.0, 38.0, 22.0, "w-remind");
        let (sh, _) = c.hover_t(&sw);
        let on = self.remind_t.pos;
        let track = mix(Color32::from_rgb(58, 58, 62), theme::BLUE, on.clamp(0.0, 1.0));
        c.fill(162.0, 36.0, 34.0, 20.0, 10.0, mix(track, theme::WHITE, sh * 0.08));
        let kx = lerp(172.0, 186.0, on);
        c.circle(kx, 46.0, 8.0, theme::WHITE);
        paint::bell(&c.scaled(kx, 46.0, 0.9), kx, 46.0, mix(Color32::from_gray(140), theme::BLUE, on.clamp(0.0, 1.0)),0.0);
        if sw.clicked() {
            self.water.remind = !self.water.remind;
            self.water.save();
            self.last_reminder = self.t;
            self.say(if self.water.remind { "饮水提醒已打开" } else { "饮水提醒已关闭" });
        }

        // 今天 · 3% · 下次 08:07
        let mut x = 44.0;
        x += c.text(x, 77.0, "今天 · ", size::SMALL, W::Regular, theme::GRAY, H::Left);
        x += c.text(x, 77.0, &format!("{pct:.0}%"), size::SMALL, W::Bold, theme::BLUE, H::Left);
        if self.water.percent()>=100.0 {
            c.text(x,77.0," · 已达标",size::SMALL,W::Regular,theme::GRAY,H::Left);
        } else if self.water.remind && self.water_hours(env.hour() as u32) {
            c.text(x, 77.0, &format!(" · 下次 {}", self.next_hint(env)), size::SMALL, W::Regular, theme::GRAY, H::Left);
        }
        c.fill(44.0, 84.0, 151.0, 3.5, 1.75, Color32::from_rgb(34, 34, 37));
        if f > 0.0 {
            c.fill(44.0, 84.0, (151.0 * f).max(3.5), 3.5, 1.75, theme::BLUE);
        }

        let (d, dc) = capsule(c, 43.0, 98.0, 62.0, 25.0, "w-drink", theme::BLUE_DEEP, Color32::from_rgb(22, 92, 165));
        paint::droplet(&dc, 56.0, 110.5, 11.0, Color32::from_rgb(120, 200, 255));
        dc.text(65.0, 114.0, "喝一口", size::BODY, W::Semibold, Color32::from_rgb(220, 236, 255), H::Left);
        let (s, sc) = capsule(c, 111.0, 98.0, 66.0, 25.0, "w-set", theme::BTN, theme::BTN_HOVER);
        paint::sliders(&sc, 126.0, 110.5, theme::WHITE);
        sc.text(149.0, 114.0, "设置", size::BODY, W::Regular, theme::WHITE, H::Center);
        if d.clicked() { self.drink_water(); }
        if s.clicked() { self.open_settings_category(1); }
        if self.last_drink.is_some() {
            c.text(191.0,114.0,"撤销",10.5,W::Regular,theme::GRAY,H::Center);
            if c.hit(177.0,98.0,28.0,25.0,"water-undo").on_hover_text("撤销最近一次饮水记录").clicked() {
                if let Some((ml,hour))=self.last_drink.take() {
                    self.water.drunk_ml=self.water.drunk_ml.saturating_sub(ml);
                    self.water.hours[hour]=self.water.hours[hour].saturating_sub(ml);
                    self.water.save(); self.say("已撤销这次记录");
                }
            }
        }

        if !side { c.fill(208.0, 36.0, 1.0, 86.0, 0.5, Color32::from_rgb(30, 30, 33)); }
        }
        let side_card = c;
        let chart = if side { c.placed((214.0, 30.0), (10.0, 106.0), 0.82) } else { c.clone() };
        let c = &chart;

        // 右侧：环形 ↔ 柱状图（点右侧区域或右上角两颗豆切换）
        let vt = self.water_view_t.pos;
        let area = if side && self.water_view == 1 {
            side_card.hit(10.0, 106.0, 136.0, 161.0, "w-view")
        } else {
            c.hit(214.0, 30.0, 166.0, 96.0, "w-view")
        };
        if area.clicked() {
            self.water_view = 1 - self.water_view;
        }
        for i in 0..2 {
            let on = if i == 0 { 1.0 - vt.clamp(0.0, 1.0) } else { vt.clamp(0.0, 1.0) };
            let w = lerp(3.0, 7.0, on);
            let x = 362.0 + i as f32 * 8.0 - w / 2.0 + if i == 0 { -1.0 * on } else { 1.0 * on };
            c.fill(x, 33.5, w, 3.0, 1.5, mix(Color32::from_gray(80), theme::WHITE, on));
        }

        let ring_a = 1.0 - smoothstep(0.0, 0.6, vt);
        if ring_a > 0.0 {
            let k = 1.0 + 0.22 * vt;
            let rc = c.faded(ring_a).scaled(292.0, 78.0, k);
            Self::water_ring(&rc, 292.0, 78.0, 33.0, 6.0, f);
            rc.text(292.0, 86.0, &format!("{pct:.0}"), 26.0, W::Bold, theme::WHITE, H::Center);
            paint::droplet(&rc, 287.0, 97.0, 7.0, theme::CYAN);
            rc.text(292.5, 100.0, "%", 10.5, W::Semibold, Color32::from_gray(217), H::Left);
        }
        let chart_a = smoothstep(0.25, 1.0, vt);
        if chart_a > 0.0 {
            if side {
                self.side_water_chart(&side_card.faded(chart_a), env, vt);
            } else {
                self.water_chart(&c.faded(chart_a), env, vt);
            }
        }
    }

    /// 底部留缺口的进度环（约 276°），起点有圆头亮点。
    fn water_ring(c: &Card, cx: f32, cy: f32, r: f32, thick: f32, frac: f32) {
        let start = 132.0f32.to_radians();
        let sweep = 276.0f32.to_radians();
        c.arc(cx, cy, r, start, start + sweep, thick, theme::WATER_TRACK);
        c.circle(cx + r * start.cos(), cy + r * start.sin(), thick * 0.5, theme::WATER_TRACK);
        c.circle(cx + r * (start + sweep).cos(), cy + r * (start + sweep).sin(), thick * 0.5, theme::WATER_TRACK);
        let end = start + sweep * frac;
        if frac > 0.002 {
            c.arc(cx, cy, r, start, end, thick, theme::BLUE);
        }
        c.circle(cx + r * start.cos(), cy + r * start.sin(), thick * 0.55, theme::BLUE);
        if frac > 0.002 {
            c.circle(cx + r * end.cos(), cy + r * end.sin(), thick * 0.5, theme::BLUE);
        }
    }

    fn water_chart_scale(&self) -> f32 {
        let max = self.water.hours.iter().copied().max().unwrap_or(0).max(500) as f32;
        (max / 250.0).ceil() * 250.0
    }

    fn side_water_chart(&self, c: &Card, env: &Env, vt: f32) {
        let hour = env.hour();
        let first = hour.saturating_sub(8).min(12);
        let max = self.water_chart_scale();
        let (left, width, top, step) = (34.0, 107.0, 126.0, 10.0);
        let bottom = top + 12.0 * step;
        for (index, label) in [0, max as u32 / 2, max as u32].into_iter().enumerate() {
            let x = left + width * index as f32 / 2.0;
            let label = if index == 2 { format!("{label} mL") } else { label.to_string() };
            c.text(x, bottom + 14.0, &label, 8.5, W::Regular, theme::GRAY,
                if index == 2 { H::Right } else { H::Center });
            c.seg((x, top), (x, bottom), 0.6, Color32::from_gray(55));
        }
        for index in 0..12 {
            let hr = first + index;
            let y = top + (index as f32 + 0.5) * step;
            let value = self.water.hours[hr] as f32;
            let current = hr == hour;
            let color = if current { theme::BLUE } else { Color32::from_rgb(40, 120, 190) };
            c.centered_text(27.0, y, &format!("{hr:02}"), 8.5, W::Regular,
                if current { theme::BLUE } else { theme::GRAY }, H::Right);
            c.fill(left, y - 2.8, width, 5.6, 2.8, Color32::from_rgb(26, 26, 29));
            let rise = anim::ease_out_cubic(((vt - 0.3 - index as f32 * 0.03) / 0.5).clamp(0.0, 1.0));
            if value > 0.0 {
                c.fill(left, y - 2.8, (width * value / max).max(5.6) * rise, 5.6, 2.8, color);
            } else if current {
                c.circle(left + 2.8, y, 2.8 * rise, color);
            }
        }
    }

    /// 今日分小时柱状图：12 根柱，从当前时间往前；柱子依次升起。
    fn water_chart(&self, c: &Card, env: &Env, vt: f32) {
        let hour = env.hour();
        let first = hour.saturating_sub(8).min(12);
        let (x0, bottom, top) = (224.0, 103.0, 52.0);
        let scale = self.water_chart_scale();
        for (i, label) in [(0.0, format!("{}", scale as u32)), (0.5, format!("{}", (scale / 2.0) as u32)), (1.0, "0 ml".to_string())] {
            let y = lerp(top, bottom, i);
            c.text(343.0, y + 3.0, &label, size::TINY, W::Regular, theme::DIM, H::Left);
            if i == 0.5 {
                for k in 0..28 {
                    c.fill(x0 + k as f32 * 4.0, y, 2.0, 0.8, 0.0, Color32::from_gray(48));
                }
            }
        }
        c.fill(x0 - 2.0, bottom + 1.5, 114.0, 0.8, 0.0, Color32::from_gray(48));
        for i in 0..12 {
            let hr = first + i;
            let x = x0 + i as f32 * 9.4;
            // 依次升起：每根晚 18 ms
            let rise = anim::ease_out_cubic(((vt - 0.3 - i as f32 * 0.03) / 0.5).clamp(0.0, 1.0));
            let full = bottom - top;
            c.fill(x, bottom - full * rise, 5.6, full * rise, 2.8, Color32::from_rgb(26, 26, 29));
            let v = self.water.hours.get(hr).copied().unwrap_or(0) as f32;
            if v > 0.0 {
                let hgt = (full * (v / scale)).max(5.6) * rise;
                let col = if hr == hour { theme::BLUE } else { Color32::from_rgb(40, 120, 190) };
                c.fill(x, bottom - hgt, 5.6, hgt, 2.8, col);
            } else if hr == hour {
                c.circle(x + 2.8, bottom - 2.8, 2.8 * rise, theme::BLUE);
            }
            if hr % 3 == 0 {
                c.text(x + 2.8, 117.0, &format!("{hr:02}"), size::TINY, W::Regular, theme::GRAY, H::Center);
            }
        }
    }

    // ================= 置物架 =================
    pub(crate) fn page_shelf(&mut self, c: &Card, _env: &Env, cmds: &mut Vec<Cmd>) {
        let side = self.side();
        let n = self.shelf.items.len();
        let drag = self.drag_a;
        let add_x = if side { 140.0 } else { self.expanded_width()-75.0 };
        c.text(add_x,23.0,"添加",size::SMALL,W::Regular,theme::GRAY,H::Right);
        if c.hit(add_x-33.0,8.0,38.0,22.0,"shelf-add").on_hover_text("从电脑选择文件").clicked(){cmds.push(Cmd::AddFiles);}
        let capacity=self.shelf_page_capacity().max(1);
        self.shelf_first=self.shelf_first.min(n.saturating_sub(1));
        if n>capacity && !side {c.text(self.expanded_width()/2.0,self.expanded_height()-21.0,&format!("{}–{} / {n} · 滚轮翻页",self.shelf_first+1,(self.shelf_first+capacity).min(n)),10.5,W::Regular,theme::DIM,H::Center);}

        if n > 0 {
            // 「添加」左边的「清空」：第一次点变红，3 秒内再点才真正清空。
            // 「确认」与「清空」同宽，不会挤到顶栏时间。
            let confirming = self.confirm_clear.is_some();
            let label = if confirming { "确认" } else { "清空" };
            let lw = c.text_w("清空", size::SMALL, W::Regular) + 16.0;
            let base = if confirming { Color32::from_rgb(120, 28, 36) } else { theme::BTN };
            let hover = if confirming { Color32::from_rgb(150, 34, 44) } else { theme::BTN_HOVER };
            // 放在右下角：顶栏中间是日期时间，宽度随 12 小时制等设置变化。
            let clear_y=if self.expanded_height()<=PH {109.0}else{self.expanded_height()-40.0};
            let (x, y) = if side { (101.0 - lw, 10.0) } else { (self.expanded_width()-42.0-lw, clear_y) };
            let (r, bc) = capsule(&c.faded(1.0 - drag), x, y, lw, 17.0, "shelf-clear", base, hover);
            let r = if confirming { r.on_hover_text("再点一次清空置物架（文件本身不动）") } else { r.on_hover_text("清空置物架") };
            bc.text(x + lw / 2.0, y + 12.0, label, size::SMALL, W::Regular, theme::WHITE, H::Center);
            if r.clicked() {
                if confirming {
                    self.shelf.clear();
                    self.icons.clear();
                    self.shelf_born.clear();
                    self.shelf_x.clear();
                    self.shelf_first = 0;
                    self.confirm_clear = None;
                    self.say("置物架已清空（文件本身没动）");
                } else {
                    self.confirm_clear = Some(self.t);
                }
            }
        }

        let body = c.faded(lerp(1.0, 0.14, drag));
        if n == 0 {
            let a = 1.0 - drag;
            if a > 0.0 {
                let ec = body.faded(a);
                let (x, w, center, height) = if side { (10.0, 136.0, 78.0, 91.0) } else { (36.0, self.expanded_width()-72.0, self.expanded_width()/2.0, (self.expanded_height()-48.0).max(91.0)) };
                ec.dashed(x, 32.0, w, height, 12.0, 1.2, Color32::from_gray(70), 0.0);
                paint::tray(&ec, center, 60.0, 28.0, 0.0, Color32::from_gray(200), Color32::from_gray(80));
                ec.text(center, 93.0, "把文件拖到这儿", 12.0, W::Semibold, theme::WHITE, H::Center);
                if side {
                    ec.text(center, 107.0, "只记位置，不复制文件", size::TINY, W::Regular, theme::GRAY, H::Center);
                    ec.text(center, 119.0, "点一下打开", size::TINY, W::Regular, theme::GRAY, H::Center);
                } else {
                    ec.text(center, 108.0, "只记位置，不复制文件 · 点一下打开", size::TINY, W::Regular, theme::GRAY, H::Center);
                }
            }
        } else {
            self.shelf_row(&body, cmds);
        }
        if drag > 0.002 {
            self.drag_overlay(&c.faded(drag));
        }
    }

    fn shelf_row(&mut self, c: &Card, cmds: &mut Vec<Cmd>) {
        let side = self.side();
        let n = self.shelf.items.len();
        if side { self.shelf_row_side(c,cmds); return; }
        let width=self.expanded_width();
        let bounds=self.shelf_list_bounds();
        let row=c.clipped(bounds.left(),bounds.top(),bounds.width(),bounds.height());
        let columns=Self::shelf_columns_for_width(width);
        let capacity=self.shelf_page_capacity().max(1);
        let first=self.shelf_first.min(n.saturating_sub(1));
        let end=(first+capacity).min(n);
        let dt = self.dt;
        let items = self.shelf.items.clone();
        self.icons.retain(&items);
        self.shelf_x.retain(|p, _| items.contains(p));
        let available_width=width-72.0;
        for (visible_index,p) in items.iter().enumerate().take(end).skip(first) {
            let page_index=visible_index-first;
            let row_start=(page_index/columns)*columns;
            let items_in_row=(end-row_start).min(columns);
            let slot=available_width/items_in_row as f32;
            let column=page_index%columns;
            let row_index=page_index/columns;
            let x=36.0+(column as f32+0.5)*slot;
            let y=30.0+row_index as f32*94.0;
            let born = self.shelf_born.get(p).copied();
            let spring = self.shelf_x.entry(p.clone()).or_insert_with(|| {
                let start=if born.is_some(){x+slot*0.9}else{x};
                Spring::new(start,230.0,0.9)
            });
            spring.target=x;
            spring.step(dt);
            let x=spring.pos;
            let half_w=31.0;
            let height=82.0;
            let appear=born.map(|b|smoothstep(0.0,0.35,(self.t-b) as f32)).unwrap_or(1.0);
            let edge=smoothstep(20.0,36.0,x)*smoothstep(width-20.0,width-36.0,x);
            let a=appear*edge;
            if a<=0.01 {continue;}
            let r=row.hit_drag(x-half_w,y,half_w*2.0,height,("shelf",p));
            let show_hint=self.shelf_menu_rect.is_none()&&!r.context_menu_opened()&&!r.secondary_clicked();
            if show_hint {r.clone().on_hover_text(p.display().to_string());}
            let (h,pr)=row.hover_t(&r);
            let cc=row.faded(a).scaled(x,y+36.0,1.0-0.04*pr);
            if h>0.01 {cc.fill(x-half_w,y,half_w*2.0,height,10.0,Color32::from_white_alpha((16.0*h) as u8));}
            let lift=-2.0*h+(1.0-appear)*8.0;
            self.file_icon_at(&cc,p,x,y+22.0+lift,36.0);
            let name=p.file_name().map(|s|s.to_string_lossy().to_string()).unwrap_or_default();
            let name_width=(slot-8.0).clamp(54.0,104.0);
            let lines=cc.wrap(&name,size::SMALL,W::Semibold,name_width,2);
            for (i,line) in lines.iter().enumerate(){cc.text(x,y+56.0+i as f32*13.0,line,size::SMALL,W::Semibold,theme::WHITE,H::Center);}
            let meta=self.icons.get(c.ui.ctx(),p).map(|preview|preview.metadata).unwrap_or_default();
            cc.text(x,y+if lines.len()>1{82.0}else{69.0},&meta,size::TINY,W::Regular,theme::GRAY,H::Center);
            if r.drag_started()&&p.exists(){cmds.push(Cmd::DragFile(p.clone()));}
            let remove_y=y+9.0;
            let remove=row.hit(x+15.0,remove_y-9.0,17.0,18.0,("shelf-remove",p));
            let remove_card=row.faded(a);
            remove_card.circle(x+23.5,remove_y,7.5,Color32::from_rgb(55,40,46));
            paint::close(&remove_card,x+23.5,remove_y,2.4,theme::WHITE);
            let mut delete=remove.clicked();
            if show_hint {remove.on_hover_text("从置物架移除，保留原文件");}
            r.context_menu(|ui|{
                if ui.button("打开").clicked(){if p.exists(){cmds.push(Cmd::Open(p.clone()));}ui.close_menu();}
                if ui.button("在文件夹中显示").clicked(){cmds.push(Cmd::Reveal(p.clone()));ui.close_menu();}
                if ui.button("复制完整路径").clicked(){cmds.push(Cmd::Copy(p.display().to_string()));ui.close_menu();}
                if ui.button("从置物架移除").clicked(){delete=true;ui.close_menu();}
                self.shelf_menu_rect=Some(egui::Frame::menu(ui.style()).widget_rect(ui.min_rect()));
            });
            if delete {self.shelf.remove(p);self.icons.remove(p);self.shelf_x.remove(p);self.shelf_born.remove(p);self.shelf_first=self.shelf_first.min(self.shelf.items.len().saturating_sub(1));}
            else if r.clicked(){if p.exists(){cmds.push(Cmd::Open(p.clone()));}else{self.say("文件已移动或删除，可从置物架移除");}}
        }
    }

    fn shelf_row_side(&mut self,c:&Card,cmds:&mut Vec<Cmd>){
        let n=self.shelf.items.len();
        let slot=68.0;
        let center=209.5;
        let visible=n.min(5) as f32;
        let scroll=self.shelf_scroll.pos;
        let row=c.clone();
        let dt = self.dt;
        let items = self.shelf.items.clone();
        self.icons.retain(&items);
        self.shelf_x.retain(|p, _| items.contains(p));
        for (i, p) in items.iter().enumerate() {
            let slot_card = row.shifted(0.0, 3.0 + i as f32 * 94.0);
            let row = &slot_card;
            let target = center + (i as f32 - scroll - (visible - 1.0) / 2.0) * slot;
            let born = self.shelf_born.get(p).copied();
            let s = self.shelf_x.entry(p.clone()).or_insert_with(|| {
                // 新放入的从右侧滑进来；启动时已有的直接就位
                let start = if born.is_some() { target + slot * 0.9 } else { target };
                Spring::new(start, 230.0, 0.9)
            });
            s.target = target;
            s.step(dt);
            let x = 78.0;
            let (half_w, height) = (63.0,91.0);
            if !row.rect(x-half_w,30.0,half_w*2.0,height).intersects(row.painter.clip_rect()){continue;}
            if x < 0.0 || x > 420.0 {
                continue;
            }
            let appear = born.map(|b| smoothstep(0.0, 0.35, (self.t - b) as f32)).unwrap_or(1.0);
            let edge = smoothstep(26.0, 60.0, x) * smoothstep(392.0, 358.0, x);
            let a = appear * edge;
            if a <= 0.01 {
                continue;
            }
            let r = row.hit_drag(x - half_w, 30.0, half_w * 2.0, height, ("shelf", p));
            let show_hint=self.shelf_menu_rect.is_none() && !r.context_menu_opened() && !r.secondary_clicked();
            if show_hint {r.clone().on_hover_text(p.display().to_string());}
            let (h, pr) = row.hover_t(&r);
            let cc = row.faded(a).scaled(x, 66.0, 1.0 - 0.04 * pr);
            if h > 0.01 {
                cc.fill(x - half_w, 30.0, half_w * 2.0, height, 10.0, Color32::from_white_alpha((16.0 * h) as u8));
            }
            let lift = -2.0 * h + (1.0 - appear) * 8.0;
            self.file_icon_at(&cc, p, x, 52.0 + lift, 36.0);
            let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let lines = cc.wrap(&name, size::SMALL, W::Semibold, 116.0, 2);
            for (i, line) in lines.iter().enumerate() {
                cc.text(x, 86.0 + i as f32 * 13.0, line, size::SMALL, W::Semibold, theme::WHITE, H::Center);
            }
            let meta=self.icons.get(c.ui.ctx(),p).map(|preview|preview.metadata).unwrap_or_default();
            cc.text(x, if lines.len() > 1 { 112.0 } else { 99.0 }, &meta, size::TINY, W::Regular, theme::GRAY, H::Center);
            if r.drag_started() && p.exists() {cmds.push(Cmd::DragFile(p.clone()));}
            let remove=row.hit(x+15.0,30.0,17.0,18.0,("shelf-remove",p));
            let remove_card=row.faded(a);
            remove_card.circle(x+23.5,39.0,7.5,Color32::from_rgb(55,40,46));
            paint::close(&remove_card,x+23.5,39.0,2.4,theme::WHITE);
            let mut delete=remove.clicked();
            if show_hint {remove.on_hover_text("从置物架移除，保留原文件");}
            r.context_menu(|ui|{
                if ui.button("打开").clicked(){if p.exists(){cmds.push(Cmd::Open(p.clone()));}ui.close_menu();}
                if ui.button("在文件夹中显示").clicked(){cmds.push(Cmd::Reveal(p.clone()));ui.close_menu();}
                if ui.button("复制完整路径").clicked(){cmds.push(Cmd::Copy(p.display().to_string()));ui.close_menu();}
                if ui.button("从置物架移除").clicked(){delete=true;ui.close_menu();}
                self.shelf_menu_rect=Some(egui::Frame::menu(ui.style()).widget_rect(ui.min_rect()));
            });
            if delete { self.shelf.remove(p); self.icons.remove(p); self.shelf_x.remove(p); self.shelf_born.remove(p); self.shelf_first=self.shelf_first.min(self.shelf.items.len().saturating_sub(5)); }
            else if r.clicked() { if p.exists(){cmds.push(Cmd::Open(p.clone()));} else {self.say("文件已移动或删除，可从置物架移除");} }
        }
    }

    // ================= 独立剪贴板页面 =================
    pub(crate) fn page_clipboard(&mut self, c: &Card, _env: &Env, _cmds: &mut Vec<Cmd>) {
        let side = self.side();
        let offset = if side { 199.0 } else { 0.0 };
        let center = 209.5 - offset / 2.0;
        let card = if side { c.shifted(-32.0, 0.0) } else { c.clone() };
        let c = &card;
        if !self.settings.clipboard_history {
            c.dashed(42.0, 32.0, 335.0-offset, 91.0, 12.0, 1.2, Color32::from_gray(70), 0.0);
            c.text(center, 66.0, "剪贴板记录已关闭", 12.0, W::Semibold, theme::WHITE, H::Center);
            c.text(center, 82.0, if side { "开启后显示最近复制的文字" } else { "开启后，最近复制的文字会出现在这里" }, size::TINY, W::Regular, theme::GRAY, H::Center);
            let (r, bc) = capsule(c, center-30.0, 92.0, 60.0, 20.0, "clip-enable", theme::BTN, theme::BTN_HOVER);
            bc.centered_text(center, 102.0, "开启", size::SMALL, W::Semibold, theme::WHITE, H::Center);
            if r.clicked() { self.settings.clipboard_history = true; self.clipboard.set_enabled(true); self.settings.save(); }
            return;
        }
        let items = self.clipboard.lock().items.clone();
        let n = items.len();
        let recent = items.iter().filter(|clip| !clip.pinned).count();
        self.clip_first = self.clip_first.min(n.saturating_sub(CLIP_ROWS));

        // 右上：条数与「清空」（只清未固定的）。
        if recent > 0 {
            let confirming = self.clip_confirm_clear.is_some_and(|at| self.t - at < 3.0);
            if !confirming { self.clip_confirm_clear = None; }
            let label = if confirming { "确认" } else { "清空" };
            let lw = c.text_w("清空", size::SMALL, W::Regular) + 16.0;
            let base = if confirming { Color32::from_rgb(120, 28, 36) } else { theme::BTN };
            let hover = if confirming { Color32::from_rgb(150, 34, 44) } else { theme::BTN_HOVER };
            let x = 351.0 - lw - offset;
            let (r, bc) = capsule(c, x, 11.5, lw, 17.0, "clip-clear", base, hover);
            bc.text(x + lw / 2.0, 23.5, label, size::SMALL, W::Regular, theme::WHITE, H::Center);
            let r = r.on_hover_text(if confirming { "再点一次清空未固定的记录" } else { "清空未固定的记录" });
            if r.clicked() {
                if confirming {
                    self.clipboard.lock().clear_recent();
                    self.clip_first = 0;
                    self.clip_confirm_clear = None;
                    self.say("剪贴板记录已清空（固定的保留）");
                } else {
                    self.clip_confirm_clear = Some(self.t);
                }
            }
        }

        if n == 0 {
            c.dashed(42.0, 32.0, 335.0-offset, 91.0, 12.0, 1.2, Color32::from_gray(70), 0.0);
            paint::clipboard_glyph(c, center, 58.0, 22.0, Color32::from_gray(200), Color32::from_gray(80));
            c.text(center, 93.0, if side { "暂无复制记录" } else { "复制的文字会出现在这里" }, 12.0, W::Semibold, theme::WHITE, H::Center);
            c.text(center, 108.0, if side { "记录保存在本机" } else { "记录保存在本机，重新打开仍可查看" }, size::TINY, W::Regular, theme::GRAY, H::Center);
            return;
        }

        let list_h = if side { 187.0 } else { 93.0 };
        c.fill(42.0, 31.0, 335.0-offset, list_h, 12.0, theme::CARD);
        // 超过一屏时右侧细滚动条，滚轮查看。
        if n > CLIP_ROWS {
            let (top, track) = (38.0, list_h - 14.0);
            let thumb = (track * CLIP_ROWS as f32 / n as f32).max(12.0);
            let y = top + (track - thumb) * self.clip_first as f32 / (n - CLIP_ROWS) as f32;
            c.fill(371.0-offset, top, 2.5, track, 1.25, Color32::from_white_alpha(14));
            c.fill(371.0-offset, y, 2.5, thumb, 1.25, Color32::from_white_alpha(70));
        }
        let mut action: Option<(usize, u8)> = None; // 0 复制 1 固定 2 删除
        for (row, (index, clip)) in items.iter().enumerate().skip(self.clip_first).take(CLIP_ROWS).enumerate() {
            let y = if side { 67.0 + row as f32 * 45.0 } else { 44.0 + row as f32 * 22.5 };
            let (row_top, row_h) = if side { (y - 33.0, 43.0) } else { (y - 10.0, 20.0) };
            let r = c.hit(48.0, row_top, 323.0-offset, row_h, ("clip", index, clip.text.len(), clip.at.as_str()));
            let (h, _) = c.hover_t(&r);
            if h > 0.01 {
                c.fill(48.0, row_top, 323.0-offset, row_h, 7.0, Color32::from_white_alpha((12.0 * h) as u8));
            }
            if clip.pinned {
                paint::pin(c, 58.0, y, 9.0, theme::BLUE);
            } else {
                c.circle(58.0, y, 2.2, Color32::from_gray(110));
            }
            let preview = crate::clipboard::preview(&clip.text);
            if side {
                for (i, line) in c.wrap(&preview, 10.5, W::Regular, 112.0, 2).iter().enumerate() {
                    c.text(53.0, y - 21.0 + i as f32 * 12.0, line, 10.5, W::Regular, theme::WHITE, H::Left);
                }
            } else {
                let text = c.fit(&preview, size::BODY, W::Regular, 210.0);
                c.text(68.0, y + 3.8, &text, size::BODY, W::Regular, Color32::from_gray(235), H::Left);
            }
            c.text(318.0-offset, y + 3.5, &clip.at, size::TINY, W::Regular, theme::DIM, H::Right);
            let tip: String = clip.text.chars().take(400).collect();
            let r = r.on_hover_text(format!("{tip}{}\n点击复制", if clip.text.chars().count() > 400 { "…" } else { "" }));
            let pin = c.hit(323.0-offset, y - 9.0, 18.0, 18.0, ("clip-pin", index)).on_hover_text(if clip.pinned { "取消固定" } else { "固定（清空时保留）" });
            paint::pin(c, 332.0-offset, y, 9.0, if clip.pinned || pin.hovered() { theme::BLUE } else { theme::GRAY });
            let delete = c.hit(345.0-offset, y - 9.0, 17.0, 18.0, ("clip-delete", index)).on_hover_text("删除这条记录");
            paint::close(c, 353.5-offset, y, 2.4, theme::GRAY);
            if delete.clicked() { action = Some((index, 2)); }
            else if pin.clicked() { action = Some((index, 1)); }
            else if r.clicked() { action = Some((index, 0)); }
        }
        match action {
            Some((index, 0)) => {
                #[cfg(windows)]
                self.say(if crate::clipboard::copy_history(&items[index].text) { "已复制" } else { "复制失败，请重试" });
                #[cfg(not(windows))]
                _cmds.push(Cmd::Copy(items[index].text.clone()));
            }
            Some((index, 1)) => {
                if !self.clipboard.lock().toggle_pin(index) { self.say(format!("最多固定 {} 条", crate::clipboard::MAX_PINNED)); }
            }
            Some((index, _)) => { self.clipboard.lock().remove(index); }
            None => {}
        }
    }

    pub(crate) fn file_icon_at(&mut self, c: &Card, p: &std::path::Path, cx: f32, cy: f32, sz: f32) {
        let tex=self.icons.get(c.ui.ctx(),p).and_then(|preview|preview.image);
        if let Some(tex) = tex {
            let s = tex.size_vec2();
            let k = sz / s.x.max(s.y);
            let r = Rect::from_center_size(c.pos(cx, cy), s * k * c.s);
            c.painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        } else {
            paint::file_glyph(c, paint::file_kind(p), cx, cy, sz);
        }
    }

    /// 拖入遮罩：流动虚线框 + 跟随光标的柔光 + 上下轻跳的托盘图标。
    fn drag_overlay(&self, c: &Card) {
        let (x, y, w, h) = if self.side() { (10.0, 31.0, 136.0, 93.0) } else { (42.0, 31.0, 335.0, 93.0) };
        let center = x + w / 2.0;
        c.fill(x, y, w, h, 12.0, Color32::from_black_alpha(110));
        if let Some(p) = self.cursor_local {
            let px = p.x.clamp(x + 20.0, x + w - 20.0);
            let py = p.y.clamp(y + 10.0, y + h - 10.0);
            c.clipped(x, y, w, h).glow(px, py, 90.0, Color32::from_rgba_unmultiplied(120, 170, 255, 34));
        }
        c.dashed(x, y, w, h, 12.0, 1.4, Color32::from_white_alpha(230), self.t as f32 * 14.0);
        let bob = ((self.t * 5.0).sin() as f32 * 0.5 + 0.5) * 3.0;
        paint::tray(c, center, 64.0, 30.0, bob, theme::WHITE, Color32::from_gray(90));
        c.text(center, 98.0, "松手放进置物架", 12.0, W::Semibold, theme::WHITE, H::Center);
    }

    // ================= 待办 =================
    pub(crate) fn page_todo(&mut self, c: &Card, env: &Env, _cmds: &mut Vec<Cmd>) {
        let side = self.side();
        let summary = if side { c.placed((41.0, 34.0), (10.0, 5.0), 0.91) } else { c.clone() };
        // 今天要做的在前；不在今天的每周重复待办排在后面，变暗显示、不能勾选。
        let (done, total) = self.todos.progress(&env.today);
        let order = self.todos.display_order(&env.today);
        let count = order.len();
        let pop = self.todo_pop.pos.abs().min(0.4);
        let c_summary = &summary;
        let hc = c_summary.scaled(60.0, 50.0, 1.0 + pop * 0.25);
        let w = hc.text(42.0, 61.0, &format!("{done}"), size::HERO, W::Bold, if done > 0 && done == total { theme::GREEN } else { theme::WHITE }, H::Left);
        hc.text(42.0 + w, 61.0, &format!("/{total}"), size::HERO, W::Bold, theme::WHITE, H::Left);
        c_summary.text(42.0, 78.0, "已完成", size::SMALL, W::Regular, theme::GRAY, H::Left);

        let (add, ac) = capsule(c_summary, 41.0, 96.0, 74.0, 26.0, "t-add", theme::BTN, theme::BTN_HOVER);
        paint::plus(&ac, 56.0, 109.0, 6.2, theme::WHITE, Color32::BLACK);
        ac.text(67.0, 112.5, "加一个", size::BODY, W::Regular, theme::WHITE, H::Left);
        let can_edit = self.todos.items.iter().any(|t| !t.done_for(&env.today));
        let ec = if can_edit { c_summary.clone() } else { c_summary.faded(0.45) };
        let (edit, edc) = capsule(&ec, 121.0, 96.0, 68.0, 26.0, "t-edit", theme::BTN, if can_edit { theme::BTN_HOVER } else { theme::BTN });
        paint::pencil(&edc, 135.0, 109.0, 11.0, theme::WHITE);
        edc.text(145.0, 112.5, "改一下", size::BODY, W::Regular, theme::WHITE, H::Left);

        let list = if side { crate::side_pages::todo_list_card(c) } else { c.clone() };
        let c = &list;
        let list_h = if side { 185.0 } else { 93.0 };
        c.fill(196.0, 31.0, 181.0, list_h, 12.0, theme::CARD);
        let editing = self.adding || self.editing.is_some();
        if count == 0 && !editing {
            paint::checklist(c, 286.5, 64.0, 32.0, Color32::from_gray(150), Color32::from_rgb(52, 54, 58));
            c.text(286.5, 99.0, "今天还没有安排", size::SMALL, W::Regular, theme::GRAY, H::Center);
        } else {
            let mut row = 0;
            let row_y = |r: usize| if side && (r > 0 || !editing) { 58.0 + r as f32 * 53.0 } else { 46.0 + r as f32 * 22.0 };
            if editing {
                // 输入行（文本框本身由 app::input_field 放置）
                let y = row_y(0);
                c.fill(203.0, y - 10.0, 167.0, 20.0, 7.0, Color32::from_rgb(34, 34, 38));
                c.stroke(203.0, y - 10.0, 167.0, 20.0, 7.0, 1.0, paint::alpha(theme::BLUE, 0.7));
                if self.editing.is_some() {
                    paint::pencil(c, 212.0, y, 9.0, theme::BLUE);
                } else {
                    paint::plus(c, 212.0, y, 5.0, theme::BLUE, Color32::BLACK);
                }
                row = 1;
            }
            let ctx = c.ui.ctx().clone();
            let cap = if editing { 2 } else { 3 };
            let mut shown = 0;
            for &i in order.iter().skip(self.todo_first.min(count.saturating_sub(1))) {
                if shown >= cap {
                    break;
                }
                if Some(i) == self.editing {
                    continue;
                }
                let item = self.todos.items[i].clone();
                let today_item = item.scheduled_on(&env.today);
                let y = row_y(row);
                let (row_top, row_h) = if side { (y - 22.0, 49.0) } else { (y - 10.0, 20.0) };
                let r = c.hit(202.0, row_top, 169.0, row_h, ("todo", i, item.text.as_str()));
                let (h, _) = c.hover_t(&r);
                if h > 0.01 {
                    c.fill(202.0, row_top, 169.0, row_h, 7.0, Color32::from_white_alpha((12.0 * h) as u8));
                }
                let checked = item.done_for(&env.today);
                let ct = ctx.animate_bool_with_time(egui::Id::new(("todo-check", item.text.as_str())), checked, 0.28);
                let rc = if today_item { c.clone() } else { c.faded(0.45) };
                rc.ring(212.0, y, 5.2, 1.3, paint::alpha(Color32::from_gray(120), 1.0 - ct));
                if ct > 0.0 {
                    c.circle(212.0, y, 6.0 * anim::ease_out_cubic(ct.min(1.0) / 0.6).min(1.0), theme::GREEN);
                    paint::check(c, 212.0, y, 6.0, smoothstep(0.3, 1.0, ct), 1.4, Color32::BLACK);
                }
                // 每周重复：文字右侧小标签（周一三五 / 工作日 / 每天）。
                let tag = crate::model::repeat_tag(item.repeat);
                let tag_w = if tag.is_empty() { 0.0 } else { c.text_w(&tag, size::TINY, W::Regular) + 6.0 };
                let repeat_hint = if item.is_repeating() { format!("\n{}{}", crate::model::repeat_label(item.repeat), if today_item { "" } else { " · 今天不用做" }) } else { String::new() };
                r.clone().on_hover_text(format!("{}{}{}",item.text,repeat_hint,item.reminder_text().map(|d|format!("\n提醒：{d}")).unwrap_or_default()));
                let tcol = mix(Color32::from_gray(235), Color32::from_gray(110), ct);
                let text = c.wrap(&item.text, size::BODY, W::Regular, if side { 145.0 } else { 104.0 - tag_w }, if side { 2 } else { 1 });
                let mut tw = 0.0;
                for (index, line) in text.iter().enumerate() {
                    let baseline = y + if side { -7.0 + index as f32 * 14.0 } else { 3.8 };
                    tw = rc.text(223.0, baseline, line, size::BODY, W::Regular, tcol, H::Left);
                    if ct > 0.0 {
                        c.fill(223.0, baseline - 3.6, tw * smoothstep(0.2, 1.0, ct), 1.0, 0.5, Color32::from_gray(110));
                    }
                }
                if !tag.is_empty() {
                    let (tag_x, tag_y) = if side { (223.0, y + 21.0) } else { (223.0 + tw + 5.0, y + 3.2) };
                    rc.text(tag_x, tag_y, &tag, size::TINY, W::Regular, if today_item { theme::BLUE } else { theme::GRAY }, H::Left);
                }
                let y = if side { y + 17.0 } else { y };
                let delete=c.hit(353.0,y-9.0,17.0,18.0,("todo-delete",i)).on_hover_text("删除待办").clicked();
                paint::close(c,361.5,y,2.4,theme::GRAY);
                let reminder=c.hit(333.0,y-9.0,18.0,18.0,("todo-reminder",i)).on_hover_text(item.reminder_text().map(|due|format!("提醒：{due} · 点击修改")).unwrap_or_else(||"设置提醒与每周重复（可选）".into()));
                let reminder_clicked=reminder.clicked();
                let ink=if item.due.is_some()||reminder.hovered(){theme::BLUE}else{theme::GRAY};
                c.ring(342.0,y,4.7,1.1,ink);
                c.path(&[(342.0,y-2.5),(342.0,y),(344.3,y+1.0)],1.1,ink);
                if reminder_clicked {self.open_task_reminder(i);}
                let mut edit_this=false;
                r.context_menu(|ui|{
                    if ui.button("编辑与提醒…").clicked(){edit_this=true;ui.close_menu();}
                    if ui.button("取消提醒").clicked(){self.todos.items[i].due=None;self.todos.save();ui.close_menu();}
                });
                if edit_this {self.open_task_dialog(i);}
                if delete {self.todos.items.remove(i);self.todos.save();self.todo_first=self.todo_first.min(self.todos.items.len().saturating_sub(3));break;}
                if r.clicked() && !edit_this && !reminder_clicked && !today_item {
                    self.say(format!("{}的待办，今天不用做", crate::model::repeat_label(item.repeat)));
                } else if r.clicked() && !edit_this && !reminder_clicked {
                    let now = self.todos.items[i].done_for(&env.today);
                    self.todos.items[i].toggle(&env.today);
                    self.todos.save();
                    if !now {
                        self.todo_pop.vel += 5.0;
                        self.pet.react(crate::extras::PetState::Happy);
                    }
                }
                row += 1;
                shown += 1;
            }
            let rest = count.saturating_sub(shown + self.editing.map(|_|1).unwrap_or(0));
            if rest > 0 {
                c.text(370.0, 27.0 + list_h, &format!("共 {count} 项 · 滚轮查看"), size::TINY, W::Regular, theme::DIM, H::Right);
            }
        }

        if add.clicked() {
            self.todo_first=0;
            self.adding = true;
            self.editing = None;
            self.buf.clear();
            self.todo_input_focused=false;
        }
        if edit.clicked() && can_edit {
            if let Some(i) = self.todos.items.iter().position(|t| !t.done_for(&env.today)) {
                self.editing = Some(i);
                self.adding = false;
                self.buf = self.todos.items[i].text.clone();
                self.todo_input_focused=false;
            }
        }
    }

    // ---------------------------------------------------------------
    // 邮件：有验证码大卡时分栏，否则显示紧凑单行列表。
    // ---------------------------------------------------------------
    pub(crate) fn visible_mail_code(&self)->Option<&mail::Item> {
        self.mail.code_item().filter(|item|self.settings.mail_code && !self.mail_code_hidden(item))
    }
    pub(crate) fn mail_list_bounds(&self)->Rect {
        let left=if self.visible_mail_code().is_some(){182.0}else{36.0};
        let right=(self.expanded_width()-36.0).max(left+48.0);
        let bottom=(self.expanded_height()-18.0).max(122.0);
        Rect::from_min_max(Pos2::new(left,36.0),Pos2::new(right,bottom))
    }
    pub(crate) fn page_mail(&mut self, c: &Card, _env: &Env, cmds: &mut Vec<Cmd>) {
        let side = self.side();
        if self.mail.accounts.is_empty() {
            let empty = if side { c.placed((42.0, 44.0), (13.0, 12.0), 0.87) } else { c.clone() };
            let c = &empty;
            c.text(42.0, 60.0, "还没有邮箱", 13.0, W::Bold, theme::WHITE, H::Left);
            let hints = c.wrap("在设置 → 邮箱里添加；授权码只存进系统凭据管理器", 10.5, W::Regular, if side {145.0}else{344.0}, if side {3}else{1});
            for (i, hint) in hints.iter().enumerate() { c.text(42.0, 77.0+i as f32*13.0, hint, 10.5, W::Regular, theme::GRAY, H::Left); }
            let button = if side { c.shifted(0.0, 29.0) } else { c.clone() };
            let c = &button;
            let (go, gc) = capsule(c, 42.0, 88.0, 84.0, 26.0, "mail-setup", theme::BTN, theme::BTN_HOVER);
            gc.text(58.0, 105.0, "去添加", size::BODY, W::Regular, theme::WHITE, H::Left);
            if go.clicked() {
                self.open_mail_settings();
            }
            return;
        }

        // v0.5.5：点开某封邮件 → 整块换成正文摘要，左上「返回」回到列表。
        if let Some((account, uid)) = self.mail.open_uid.clone() {
            match self
                .mail
                .recent
                .iter()
                .find(|m| m.uid == uid && m.account.eq_ignore_ascii_case(&account))
                .cloned()
            {
                Some(item) => {
                    if side { self.side_mail_detail(c, &item, cmds); } else { self.mail_detail(c, &item, cmds); }
                    return;
                }
                None => self.mail.open_uid = None,
            }
        }
        let code=self.visible_mail_code().cloned();
        if let Some(raw)=&code {
            let code_card = if side { c.placed((36.0, 36.0), (10.0, 8.0), 136.0 / 140.0) } else { c.clone() };
            let c = &code_card;
            c.fill(36.0,36.0,140.0,86.0,11.0,theme::CARD);
            let hidden=self.mail_code_hidden(raw);
            let item=self.display_mail(raw);
            let value=item.code.as_deref().unwrap_or("••••••");
            c.text(46.0,51.0,"验证码",10.5,W::Regular,theme::GRAY,H::Left);
            c.text(166.0,51.0,&c.fit(&item.from,10.5,W::Semibold,78.0),10.5,W::Semibold,theme::WHITE,H::Right);
            c.text(46.0,75.0,value,23.0,W::Bold,theme::GREEN,H::Left);
            c.text(46.0,91.0,&c.fit(&item.subject,10.5,W::Regular,120.0),10.5,W::Regular,Color32::from_gray(190),H::Left);
            c.text(46.0,108.0,if hidden{"已隐藏 · 点击查看"}else{"点击复制"},10.5,W::Regular,theme::DIM,H::Left);
            c.text(166.0,108.0,item.time.rsplit(' ').next().unwrap_or(&item.time),10.0,W::Regular,theme::GRAY,H::Right);
            c.hit(42.0,38.0,128.0,16.0,"mail-code-source").on_hover_text(format!("来自：{}\n{}\n收件邮箱：{}\n{}",item.from,item.subject,item.account,item.time));
            if c.hit(40.0,56.0,132.0,38.0,("mail-code",item.uid)).on_hover_text(format!("复制 {} 的验证码",item.from)).clicked(){if hidden{self.reveal_mail_code(raw);}else{cmds.push(Cmd::Copy(value.to_string()));self.code_copied(value);}}
        }
        let list_bounds = if side {
            let top = if code.is_some() { 104.0 } else { 8.0 };
            Rect::from_min_size(Pos2::new(10.0, top), egui::vec2(136.0, 267.0 - top))
        } else { self.mail_list_bounds() };
        let list_x=list_bounds.left();
        let list_w=list_bounds.width();
        let row_w=list_w-12.0;
        let compact=code.is_none() && !side;
        let row_h=if compact {21.5} else if side {46.0} else {34.0};
        if self.mail.recent.is_empty(){c.text(list_x+6.0,66.0,"还没有收到邮件",11.0,W::Regular,theme::GRAY,H::Left);return;}
        let mut content=egui::Ui::new(c.ui.ctx().clone(),egui::Id::new(("mail-list",c.live)),
            egui::UiBuilder::new().max_rect(c.rect(list_bounds.left(),list_bounds.top(),list_bounds.width(),list_bounds.height())).layer_id(c.painter.layer_id()).style(c.ui.style().clone()));
        content.set_clip_rect(c.rect(list_bounds.left(),list_bounds.top(),list_bounds.width(),list_bounds.height()).intersect(c.painter.clip_rect()));
        content.set_opacity(c.painter.opacity());if !c.live{content.disable();}
        content.spacing_mut().item_spacing.y=0.0;
        content.spacing_mut().scroll.floating=false;
        content.spacing_mut().scroll.bar_width=4.0*c.s;
        content.visuals_mut().widgets.inactive.bg_fill=Color32::from_gray(80);
        egui::ScrollArea::vertical().id_salt("mail-list-scroll").max_height(list_bounds.height()*c.s).auto_shrink([false,false]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible).show_rows(&mut content,row_h*c.s,self.mail.recent.len(),|ui,range|{
            let rows:Vec<_>=self.mail.recent[range].iter().map(|item|(self.mail_code_hidden(item),self.display_mail(item))).collect();
            for (hidden,item) in &rows {
                let (rect,_)=ui.allocate_exact_size(egui::vec2(row_w*c.s,row_h*c.s),egui::Sense::hover());
                let mut row=Card::new(ui,rect.min,ui.clip_rect());row.s=c.s;row.live=c.live;
                let hit=row.hit(0.0,0.0,row_w,row_h-1.0,("mail-row",item.uid,item.account.as_str()));
                let selected=code.as_ref().is_some_and(|m|m.uid==item.uid&&m.account.eq_ignore_ascii_case(&item.account));
                if hit.hovered()||selected{row.fill(0.0,0.0,row_w,row_h-2.0,6.0,Color32::from_gray(if hit.hovered(){27}else{17}));}
                if selected{row.fill(0.0,6.0,2.0,20.0,1.0,theme::GREEN);}
                if compact {
                    let sender_w=(row_w*0.18).clamp(62.0,112.0);
                    let subject_x=6.0+sender_w+6.0;
                    let subject_w=(row_w-subject_x-46.0-if *hidden {34.0}else{0.0}).max(12.0);
                    row.text(6.0,14.0,&row.fit(&item.from,11.0,W::Semibold,sender_w),11.0,W::Semibold,theme::WHITE,H::Left);
                    let width=row.text(subject_x,14.0,&row.fit(&item.subject,10.5,W::Regular,subject_w),10.5,W::Regular,Color32::from_gray(200),H::Left);
                    if *hidden {
                        row.fill(subject_x+width,5.0,30.0,12.0,3.0,theme::BTN);
                        row.text(subject_x+15.0+width,14.0,"已隐藏",8.5,W::Regular,theme::GRAY,H::Center);
                    }
                    row.text(row_w-6.0,14.0,item.time.rsplit(' ').next().unwrap_or(&item.time),10.0,W::Regular,Color32::from_gray(170),H::Right);
                    row.seg((6.0,row_h-0.5),(row_w-6.0,row_h-0.5),0.5,Color32::from_gray(40));
                } else {
                row.text(6.0,12.0,&row.fit(&item.from,11.5,W::Semibold,row_w-54.0),11.5,W::Semibold,theme::WHITE,H::Left);
                row.text(row_w-6.0,12.0,item.time.rsplit(' ').next().unwrap_or(&item.time),10.0,W::Regular,Color32::from_gray(170),H::Right);
                for (index, line) in row.wrap(&item.subject,10.5,W::Regular,row_w-12.0,if side {2} else {1}).iter().enumerate() {
                    row.text(6.0,27.0 + index as f32 * 12.0,line,10.5,W::Regular,Color32::from_gray(200),H::Left);
                }
                }
                hit.clone().on_hover_text(format!("{}\n{}\n收件邮箱：{}\n{}",item.from,item.subject,item.account,item.time));
                if hit.clicked(){self.mail.open_uid=Some((item.account.clone(),item.uid));}
            }
        });
    }

    fn side_mail_detail(&mut self, c: &Card, raw: &mail::Item, cmds: &mut Vec<Cmd>) {
        let hidden = self.mail_code_hidden(raw);
        let item = self.display_mail(raw);
        c.fill(10.0, 8.0, 136.0, 258.0, 11.0, theme::CARD);
        let read = c.hit(18.0, 13.0, 63.0, 20.0, ("side-mail-read", item.uid));
        c.centered_text(18.0, 23.0, "阅读全文", 10.5, W::Regular, theme::BLUE, H::Left);
        if read.clicked() {
            self.mail.reading = Some(raw.clone());
            self.mail.reader_focus = true;
        }
        let back = c.hit(103.0, 13.0, 34.0, 20.0, ("side-mail-back", item.uid));
        c.centered_text(137.0, 23.0, "返回", 10.5, W::Regular, theme::GRAY, H::Right);
        if back.clicked() {
            self.mail.open_uid = None;
            return;
        }

        let bounds = c.rect(18.0, 39.0, 118.0, 184.0);
        let mut content = egui::Ui::new(c.ui.ctx().clone(), egui::Id::new(("side-mail-detail", &item.account, item.uid)),
            egui::UiBuilder::new().max_rect(bounds).layer_id(c.painter.layer_id()).style(c.ui.style().clone()));
        content.set_clip_rect(bounds.intersect(c.painter.clip_rect()));
        content.set_opacity(c.painter.opacity());
        if !c.live { content.disable(); }
        egui::ScrollArea::vertical().max_height(bounds.height()).auto_shrink([false, false]).show(&mut content, |ui| {
            let label = |text, size, weight, color| egui::Label::new(egui::RichText::new(text).font(c.font(size, weight)).color(color)).wrap();
            ui.add(label(item.subject.as_str(), 13.0, W::Semibold, theme::WHITE));
            ui.add(label(item.from.as_str(), 10.5, W::Semibold, theme::GRAY));
            ui.add(label(item.time.as_str(), 10.0, W::Regular, theme::GRAY));
            ui.add(label(item.account.as_str(), 10.0, W::Regular, theme::GRAY));
            ui.separator();
            if !item.summary.is_empty() {
                ui.add(label(item.summary.as_ref(), 11.0, W::Regular, theme::WHITE));
            } else if item.links.is_empty() {
                ui.add(label("这封邮件没有正文", 11.0, W::Regular, theme::GRAY));
            }
            for link in &item.links {
                if ui.link(&link.label).on_hover_text(&link.url).clicked() {
                    cmds.push(Cmd::OpenUrl(link.url.clone()));
                }
            }
        });

        let action = if hidden { Some("查看验证码 15 秒".to_owned()) }
            else if let Some(event) = &item.meeting {
                Some(if self.mail_event_ignored(event) { "恢复日程" } else { "忽略日程" }.to_owned())
            } else { item.code.as_ref().map(|code| format!("复制 {code}")) };
        if let Some(action) = action {
            let (response, button) = capsule(c, 17.0, 232.0, 122.0, 24.0, "side-mail-action", theme::BTN, theme::BTN_HOVER);
            button.centered_text(78.0, 244.0, &action, 10.5, W::Regular, theme::BLUE, H::Center);
            if response.clicked() {
                if hidden { self.reveal_mail_code(raw); }
                else if let Some(event) = &item.meeting { self.toggle_mail_event(event); }
                else if let Some(code) = &item.code {
                    cmds.push(Cmd::Copy(code.clone()));
                    self.code_copied(code);
                }
            }
        }
    }

    /// 岛内邮件详情可滚动，也可以进入独立窗口阅读全文。
    pub(crate) fn mail_detail(&mut self, c: &Card, raw: &mail::Item, cmds: &mut Vec<Cmd>) {
        let hidden=self.mail_code_hidden(raw);
        let display=self.display_mail(raw);let item=&display;
        let width=self.expanded_width();
        let height=self.expanded_height();
        let right=width-45.0;
        let body_width=(right-46.0).max(120.0);
        let body_height=(height-146.0).max(90.0);
        c.fill(36.0,36.0,width-72.0,height-48.0,11.0,theme::CARD);
        let back = c.hit(right-44.0, 42.0, 44.0, 14.0, ("mail-back", item.uid));
        let bh = c.hover_t(&back).0;
        c.text(right, 50.0, "返回", 10.5, W::Regular, if bh > 0.5 { theme::WHITE } else { theme::GRAY }, H::Right);
        if back.clicked() {
            self.mail.open_uid = None;
            return;
        }
        let read_x=right-108.0;
        let from = c.fit(&item.from, 11.5, W::Semibold, (read_x-54.0).max(100.0));
        c.text(46.0, 50.0, &from, 11.5, W::Semibold, theme::WHITE, H::Left);
        let read=c.hit(read_x,42.0,60.0,14.0,("mail-read",&item.account,item.uid)).on_hover_text("在独立窗口阅读完整正文");
        c.text(read_x+58.0,50.0,"阅读全文",10.5,W::Regular,if read.hovered(){theme::BLUE}else{theme::GRAY},H::Right);
        if read.clicked(){self.mail.reading=Some(raw.clone());self.mail.reader_focus=true;}
        let subject = c.fit(&item.subject, 13.5, W::Semibold, body_width);
        c.text(46.0, 73.0, &subject, 13.5, W::Semibold, theme::WHITE, H::Left);
        c.hit(46.0,58.0,body_width,20.0,("mail-subject",&item.account,item.uid)).on_hover_text(&item.subject);
        c.text(right, 92.0, &item.time, 10.0, W::Regular, Color32::from_gray(170), H::Right);
        let account=format!("收件邮箱：{}",item.account);
        c.text(46.0,92.0,&c.fit(&account,10.0,W::Regular,(body_width-50.0).max(100.0)),10.0,W::Regular,Color32::from_gray(175),H::Left);
        c.hit(46.0,80.0,body_width,16.0,("mail-account",&item.account,item.uid)).on_hover_text(&account);

        // 正文和链接共用滚动区域，会议与验证码操作保留在底部。
        let mut content=egui::Ui::new(c.ui.ctx().clone(),egui::Id::new(("mail-content",&item.account,item.uid,c.live)),
            egui::UiBuilder::new().max_rect(c.rect(46.0,104.0,body_width,body_height)).layer_id(c.painter.layer_id()).style(c.ui.style().clone()));
        content.set_clip_rect(c.rect(46.0,104.0,body_width,body_height).intersect(c.painter.clip_rect()));
        content.set_opacity(c.painter.opacity());
        if !c.live {content.disable();}
        content.spacing_mut().item_spacing.y=6.0*c.s;
        content.style_mut().spacing.scroll.floating=false;
        content.spacing_mut().scroll.bar_width=4.0*c.s;
        egui::ScrollArea::vertical().id_salt(("mail-body",&item.account,item.uid)).max_height(body_height*c.s).auto_shrink([false,false]).show(&mut content,|ui|{
            if !item.summary.is_empty() {
                ui.add(egui::Label::new(egui::RichText::new(item.summary.as_ref()).font(c.font(12.0,W::Regular)).color(Color32::from_gray(220))).wrap());
            }else if item.links.is_empty(){
                ui.label(egui::RichText::new("（这封邮件没有正文）").font(c.font(12.0,W::Regular)).color(theme::DIM));
            }
            for link in &item.links {
                let text=format!("{} ↗",c.fit(&link.label,12.0,W::Regular,(body_width-32.0).max(80.0)));
                if ui.add(egui::Link::new(egui::RichText::new(text).font(c.font(12.0,W::Regular)).color(theme::BLUE))).on_hover_text(&link.url).clicked(){
                    cmds.push(Cmd::OpenUrl(link.url.clone()));
                }
            }
        });

        // 底部一行：会议时间（绿）优先，其次验证码（点一下复制）。
        let footer_y=height-37.0;
        c.seg((46.0,footer_y),(right,footer_y),0.6,Color32::from_gray(48));
        let y = height-22.0;
        if hidden {
            c.text(46.0,y,"验证码已隐藏（不代表失效）",11.0,W::Regular,theme::GRAY,H::Left);
            c.text(right,y,"查看 15 秒",10.5,W::Regular,theme::BLUE,H::Right);
            if c.hit(right-84.0,y-14.0,86.0,24.0,("mail-reveal",item.uid)).clicked(){self.reveal_mail_code(raw);}
        } else if let Some(m) = &item.meeting {
            let ignored=self.mail_event_ignored(m);
            c.text(46.0,y,&format!("{} · {}",if ignored{"已忽略"}else{"日程"},m.at),10.5,W::Regular,if ignored{theme::DIM}else{theme::GREEN},H::Left);
            let label=if ignored{"恢复日程"}else{"忽略日程"};
            c.text(right,y,label,10.5,W::Regular,theme::GRAY,H::Right);
            if c.hit(right-58.0,y-14.0,60.0,24.0,("mail-ignore-event",item.uid)).on_hover_text(&m.title).clicked(){self.toggle_mail_event(m);}
        } else if let Some(code) = &item.code {
            let label = format!("验证码 {code}");
            let w = c.text(46.0, y, &label, 13.0, W::Semibold, theme::GREEN, H::Left);
            c.text(46.0 + w + 6.0, y, "点击复制", 10.5, W::Regular, theme::DIM, H::Left);
            let hit = c.hit(42.0, y - 14.0, w + 60.0, 24.0, ("mail-detail-code", item.uid));
            if hit.clicked() {
                cmds.push(Cmd::Copy(code.clone()));
                self.code_copied(code);
            }
        }
    }
}
