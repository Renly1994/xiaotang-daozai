//! 世界时钟：离线 IANA 时区换算，界面时间由外壳传入。
use crate::{app::{App, Env}, paint::{Card, H}, theme::{self, W}};
use chrono::{DateTime, Datelike, FixedOffset, Local, NaiveDateTime, Offset, Timelike, Utc};
use chrono_tz::Tz;
use eframe::egui;
mod globe;

pub(crate) const MAX_CITIES: usize = 6;

pub(crate) fn page_marker(c:&Card,x:f32,y:f32,r:f32,selected:bool) {
    let ocean=if selected{theme::BLUE}else{theme::BLUE_DEEP};
    let land=if selected{theme::WHITE}else{theme::GRAY};
    c.circle(x,y,r,ocean);
    for polygon in [
        &[(-0.72,-0.35),(-0.36,-0.66),(-0.08,-0.36),(-0.22,-0.1),(-0.43,-0.04)][..],
        &[(-0.37,0.02),(-0.07,0.2),(-0.13,0.62),(-0.3,0.72),(-0.4,0.3)][..],
        &[(0.04,-0.55),(0.54,-0.6),(0.8,-0.04),(0.4,0.12),(0.14,-0.05)][..],
        &[(0.12,-0.04),(0.45,0.1),(0.28,0.62),(0.04,0.34)][..],
    ] {
        c.poly(&polygon.iter().map(|(px,py)|(x+px*r,y+py*r)).collect::<Vec<_>>(),land);
    }
    c.ring(x,y,r,0.7,if selected{theme::CYAN}else{theme::DIM});
}

#[derive(Default)]
pub(crate) struct State {
    pub search: String,
    pub first: usize,
    pub preview_hours: i32,
    pub globe_view: bool,
    pub window_open: bool,
    pub window_focus: bool,
    pub selected: usize,
    globe: globe::Globe,
}

pub(crate) fn default_cities() -> Vec<String> {
    ["Asia/Shanghai", "Europe/London", "America/New_York"].into_iter().map(str::to_owned).collect()
}

pub(crate) fn valid_cities(cities: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for city in cities {
        if city.parse::<Tz>().is_ok() && !out.contains(&city) {out.push(city);}
        if out.len() == MAX_CITIES {break;}
    }
    out
}

fn city_name(zone: &str) -> String {
    let name = match zone {
        "Asia/Shanghai" => "北京",
        "Asia/Hong_Kong" => "香港",
        "Asia/Taipei" => "台北",
        "Asia/Tokyo" => "东京",
        "Asia/Seoul" => "首尔",
        "Asia/Singapore" => "新加坡",
        "Asia/Bangkok" => "曼谷",
        "Asia/Kolkata" => "新德里",
        "Asia/Kathmandu" => "加德满都",
        "Asia/Dubai" => "迪拜",
        "Europe/London" => "伦敦",
        "Europe/Paris" => "巴黎",
        "Europe/Berlin" => "柏林",
        "Europe/Moscow" => "莫斯科",
        "America/New_York" => "纽约",
        "America/Chicago" => "芝加哥",
        "America/Denver" => "丹佛",
        "America/Los_Angeles" => "洛杉矶",
        "America/Vancouver" => "温哥华",
        "America/Toronto" => "多伦多",
        "America/Sao_Paulo" => "圣保罗",
        "Australia/Sydney" => "悉尼",
        "Australia/Adelaide" => "阿德莱德",
        "Pacific/Auckland" => "奥克兰",
        "Africa/Cairo" => "开罗",
        "Africa/Johannesburg" => "约翰内斯堡",
        "UTC" => "协调世界时",
        _ => return zone.rsplit('/').next().unwrap_or(zone).replace('_', " "),
    };
    name.into()
}

fn offset_text(seconds: i32) -> String {
    let minutes = seconds.abs() / 60;
    format!("{}{hours}:{:02}", if seconds < 0 {"−"} else {"+"}, minutes % 60, hours = minutes / 60)
}

struct Reading {
    time: String,
    date: String,
    difference: String,
    daytime: bool,
    fraction: f32,
    zone: String,
    offset: String,
}

fn reading(zone: Tz, now: DateTime<Utc>, home: DateTime<FixedOffset>, twelve: bool) -> Reading {
    let local = now.with_timezone(&zone);
    let delta = local.offset().fix().local_minus_utc() - home.offset().local_minus_utc();
    let difference = if delta == 0 {"与本地无时差".into()} else {
        let minutes = delta.abs() / 60;
        let duration = if minutes % 60 == 0 {format!("{}时", minutes / 60)} else {format!("{}时{}分", minutes / 60, minutes % 60)};
        format!("比本地{}{duration}", if delta > 0 {"快"} else {"慢"})
    };
    let days = (local.date_naive() - home.date_naive()).num_days();
    let day = match days {-1 => "昨天".into(), 0 => "今天".into(), 1 => "明天".into(), _ => format!("{days:+}天")};
    let weekday = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"][local.weekday().num_days_from_monday() as usize];
    let time = if twelve {
        let (pm, hour) = local.hour12();
        format!("{} {hour}:{:02}", if pm {"下午"} else {"上午"}, local.minute())
    } else {local.format("%H:%M").to_string()};
    Reading {
        time, date: format!("{} {weekday} · {day}", local.format("%m/%d")), difference,
        daytime: (6..18).contains(&local.hour()),
        fraction: (local.hour() as f32 * 60.0 + local.minute() as f32) / 1440.0,
        zone: local.format("%Z").to_string(),
        offset: if delta==0 {String::new()} else {format!("{}{}h{}",if delta<0{"−"}else{"+"},delta.abs()/3600,if delta.abs()%3600==0{String::new()}else{format!("{:02}m",delta.abs()%3600/60)})},
    }
}

fn city_row(c:&Card,zone:&str,value:&Reading,x:f32,y:f32,width:f32,height:f32,large:bool) {
    use crate::paint::mix;
    let top=if value.daytime{mix(theme::CARD,theme::BLUE_DEEP,0.22)}else{theme::CARD};
    c.vgrad(x,y,width,height,if large{12.0}else{8.0},top,theme::CARD);
    let inset=if large{16.0}else{10.0};let right=x+width-inset;
    let primary=y+if large{28.0}else{21.0};let secondary=y+if large{52.0}else{43.0};
    let name_size=if large{16.0}else{15.0};let small=11.0;
    c.centered_text(x+inset,primary,&c.fit(&city_name(zone),name_size,W::Semibold,width*0.46),name_size,W::Semibold,theme::WHITE,H::Left);
    let (period,digits)=value.time.split_once(' ').unwrap_or(("",value.time.as_str()));
    let time_right=right-if period.is_empty(){0.0}else{if large{29.0}else{23.0}};
    c.centered_text(time_right,primary,digits,if large{30.0}else{28.0},W::Semibold,theme::WHITE,H::Right);
    if !period.is_empty(){c.centered_text(right,primary+3.0,period,small,W::Regular,theme::GRAY,H::Right);}
    c.centered_text(x+inset,secondary,&c.fit(&value.date,small,W::Regular,width*0.57),small,W::Regular,theme::GRAY,H::Left);
    let offset=if value.offset.is_empty(){value.zone.clone()}else{format!("{} {}",value.zone,value.offset)};
    c.centered_text(right,secondary,&c.fit(&offset,small,W::Regular,width*0.4),small,W::Regular,theme::GRAY,H::Right);
    let bar_y=y+height-15.0;let bar_width=width-inset*2.0;
    c.fill(x+inset,bar_y,bar_width,2.0,1.0,theme::TRACK);
    c.fill(x+inset+bar_width/4.0,bar_y,bar_width/2.0,2.0,1.0,theme::GRAY.gamma_multiply(0.55));
    let marker=x+inset+bar_width*value.fraction;
    let accent=if value.daytime{theme::YELLOW}else{theme::CYAN};
    c.circle(marker,bar_y+1.0,if large{5.0}else{3.0},accent.gamma_multiply(0.16));
    c.circle(marker,bar_y+1.0,if large{3.0}else{2.0},accent);
}

fn instant(env: &Env) -> Option<DateTime<Utc>> {
    if let Some(timestamp) = env.unix_time {return DateTime::from_timestamp(timestamp, 0);}
    // 兼容已有录制及无窗口调用方，不在界面层读取系统时钟。
    let text = format!("{} {}", env.today, env.clock);
    let local = NaiveDateTime::parse_from_str(&text, "%Y-%m-%d %H:%M:%S")
        .or_else(|_|NaiveDateTime::parse_from_str(&text, "%Y-%m-%d %H:%M")).ok()?;
    Some(local.and_local_timezone(Local).earliest()?.with_timezone(&Utc))
}

impl App {
    pub fn open_world_clock_island(&mut self) {
        self.show_page(crate::app::WORLD_CLOCK_PAGE);
    }

    pub fn open_world_clock(&mut self) {
        self.open_world_clock_island();
        if let Some(zone)=self.settings.world_clock_cities.get(self.world_clock.selected){self.world_clock.globe.center_on(zone);}
        self.world_clock.window_open=true;
        self.world_clock.window_focus=true;
    }

    pub(crate) fn page_world_clock(&mut self, c: &Card, env: &Env) {
        c.text(15.0,25.0,"世界",theme::size::TITLE,W::Bold,theme::WHITE,H::Left);
        c.text(45.5,25.0,"时钟",theme::size::TITLE,W::Bold,theme::BLUE,H::Left);
        let toggle=c.hit(308.0,6.0,31.0,25.0,"world-view").on_hover_text("切换城市列表与地球");
        let (hover,_)=c.hover_t(&toggle);
        c.centered_text(323.5,18.0,if self.world_clock.globe_view{"列表"}else{"地球"},10.5,W::Regular,crate::paint::mix(theme::GRAY,theme::WHITE,hover),H::Center);
        if toggle.clicked(){self.world_clock.globe_view=!self.world_clock.globe_view;}
        let expand=c.hit(344.0,6.0,31.0,25.0,"world-expand").on_hover_text("展开世界时钟窗口");
        let (hover,_)=c.hover_t(&expand);
        c.centered_text(359.5,18.0,"展开",10.5,W::Regular,crate::paint::mix(theme::GRAY,theme::WHITE,hover),H::Center);
        if expand.clicked(){self.world_clock.window_open=true;self.world_clock.window_focus=true;}
        let Some(now) = instant(env) else {
            c.text(209.5,90.0,"等待系统时间",13.0,W::Regular,theme::GRAY,H::Center);
            return;
        };
        let now = now + chrono::Duration::hours(self.world_clock.preview_hours as i64);
        let home = now.with_timezone(&Local).fixed_offset();
        let count = self.settings.world_clock_cities.len();
        self.world_clock.selected=self.world_clock.selected.min(count.saturating_sub(1));
        self.world_clock.first = self.world_clock.first.min(count.saturating_sub(1) / 3 * 3);
        let footer_y=crate::app::WORLD_CLOCK_H-44.0;
        if self.world_clock.globe_view {
            let area=egui::Rect::from_min_size(egui::pos2(45.0,72.0),egui::vec2(174.0,174.0));
            if let Some(index)=self.world_clock.globe.draw(c,area,now,&self.settings.world_clock_cities,self.world_clock.selected,false){self.world_clock.selected=index;}
            if let Some(zone)=self.settings.world_clock_cities.get(self.world_clock.selected) {
                if let Ok(tz)=zone.parse::<Tz>() {
                    let value=reading(tz,now,home,self.settings.clock_12h);
                    c.text(237.0,117.0,&c.fit(&city_name(zone),14.0,W::Semibold,150.0),14.0,W::Semibold,theme::WHITE,H::Left);
                    let(period,digits)=value.time.split_once(' ').unwrap_or(("",value.time.as_str()));
                    c.text(237.0,159.0,digits,30.0,W::Semibold,theme::WHITE,H::Left);
                    if !period.is_empty(){c.text(335.0,157.0,period,10.0,W::Regular,theme::GRAY,H::Left);}
                    c.text(237.0,185.0,&value.date,10.5,W::Regular,theme::GRAY,H::Left);
                    c.text(237.0,211.0,&format!("{} {}",value.zone,value.offset),10.5,W::Regular,theme::CYAN,H::Left);
                    if c.hit(232.0,229.0,139.0,26.0,"world-center").on_hover_text("将当前城市转到地球中央").clicked(){self.world_clock.globe.center_on(zone);}
                    c.centered_text(237.0,242.0,"定位城市",10.0,W::Regular,theme::DIM,H::Left);
                }
            }
        } else {for (row, zone) in self.settings.world_clock_cities.iter().skip(self.world_clock.first).take(3).enumerate() {
            let Ok(tz) = zone.parse::<Tz>() else {continue;};
            let value = reading(tz, now, home, self.settings.clock_12h);
            let y = 42.0 + row as f32 * 80.0;
            city_row(c,zone,&value,44.0,y,331.0,70.0,false);
            if c.hit(44.0,y,331.0,70.0,("world-city",zone)).on_hover_text(format!("{zone} · UTC{} · {}\n日间参考为 06:00–18:00，并非当地日出日落。",offset_text(now.with_timezone(&tz).offset().fix().local_minus_utc()),value.difference)).clicked(){self.world_clock.selected=self.world_clock.first+row;self.world_clock.globe.center_on(zone);}
        }}
        c.fill(44.0,footer_y+1.0,86.0,23.0,11.5,theme::BTN);
        c.seg((87.0,footer_y+6.0),(87.0,footer_y+19.0),0.7,theme::BTN_HOVER);
        for (x,label,delta) in [(44.0,"−1h",-1),(90.0,"+1h",1)] {
            let response=c.hit(x,footer_y,40.0,24.0,("world-shift",delta));
            let (hover,_) = c.hover_t(&response);
            if hover>0.01{c.faded(hover).fill(x+2.0,footer_y+3.0,36.0,19.0,9.5,theme::BTN_HOVER);}
            c.centered_text(x+20.0,footer_y+12.0,label,10.0,W::Regular,theme::WHITE,H::Center);
            if response.clicked(){self.world_clock.preview_hours=(self.world_clock.preview_hours+delta).clamp(-24,24);}
        }
        let preview=self.world_clock.preview_hours;
        let label=if preview==0 {"此刻".into()} else {format!("{preview:+} 小时 · 回到此刻")};
        let reset=c.hit(137.0,footer_y,151.0,24.0,"world-now").on_hover_text("回到当前时间");
        let (hover,_) = c.hover_t(&reset);
        c.centered_text(143.0,footer_y+12.0,&label,10.5,W::Regular,crate::paint::mix(theme::BLUE,theme::WHITE,hover),H::Left);
        if reset.clicked(){self.world_clock.preview_hours=0;}
        if count>3 {
            let label=format!("{}/{}",self.world_clock.first/3+1,count.div_ceil(3));
            c.centered_text(328.0,footer_y+12.0,&label,10.0,W::Regular,theme::GRAY,H::Center);
            for (x,label,first) in [(292.0,"‹",0),(349.0,"›",3)] {
                let response=c.hit(x,footer_y,26.0,24.0,("world-list",first));
                let (hover,_) = c.hover_t(&response);
                c.centered_text(x+13.0,footer_y+12.0,label,20.0,W::Regular,crate::paint::mix(theme::GRAY,theme::WHITE,hover),H::Center);
                if response.clicked(){self.world_clock.first=first;}
            }
        }
    }

    pub(crate) fn world_clock_settings(&mut self, ui: &mut egui::Ui) {
        ui.label("把常联系的城市，放在同一片天空下。");
        ui.add_space(8.0);
        if ui.add_sized([ui.available_width(),34.0],egui::Button::new("打开世界时钟")).clicked(){self.world_clock.window_open=true;self.world_clock.window_focus=true;}
        crate::extras::settings_label(ui,"我的城市");
        let mut movement = None;
        let mut remove = None;
        let count = self.settings.world_clock_cities.len();
        crate::extras::settings_card(ui,|ui|{for (index, zone) in self.settings.world_clock_cities.iter().enumerate() {
            if index>0{ui.separator();}
            ui.push_id(("world-city-settings",zone),|ui|{
                ui.scope(|ui|{
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui|{
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                            if ui.add_enabled(count>1,egui::Button::new("移除")).clicked(){remove=Some(index);}
                            if ui.add_enabled(index+1<count,egui::Button::new("↓")).on_hover_text("下移城市").clicked(){movement=Some((index,index+1));}
                            if ui.add_enabled(index>0,egui::Button::new("↑")).on_hover_text("上移城市").clicked(){movement=Some((index,index-1));}
                            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(),38.0),egui::Layout::top_down(egui::Align::Min),|ui|{
                                ui.add(egui::Label::new(egui::RichText::new(city_name(zone)).strong()).truncate());
                                ui.add(egui::Label::new(egui::RichText::new(zone).small().color(ui.visuals().weak_text_color())).truncate()).on_hover_text(zone);
                            });
                        });
                    });
                });
            });
        }});
        if let Some((from,to))=movement {self.settings.world_clock_cities.swap(from,to);}
        if let Some(index)=remove {self.settings.world_clock_cities.remove(index);}
        crate::extras::settings_label(ui,&format!("添加城市 · {count}/{MAX_CITIES}"));
        crate::extras::settings_card(ui,|ui|{
        ui.add_enabled_ui(count<MAX_CITIES,|ui|{
            ui.add(egui::TextEdit::singleline(&mut self.world_clock.search).id_salt("world-clock-search").hint_text("搜索城市，例如：东京、London").desired_width(ui.available_width()));
            ui.add_space(6.0);
            let query=self.world_clock.search.trim().to_lowercase();
            let suggestions=[chrono_tz::Asia::Tokyo,chrono_tz::Asia::Singapore,chrono_tz::America::Los_Angeles,chrono_tz::Australia::Sydney,chrono_tz::Europe::Paris,chrono_tz::Asia::Dubai,chrono_tz::Asia::Kathmandu,chrono_tz::UTC];
            let candidates=if query.is_empty(){suggestions.as_slice()}else{chrono_tz::TZ_VARIANTS.as_slice()};
            let matches:Vec<Tz>=candidates.iter().copied().filter(|zone|{
                let aliases=match zone.name(){"Asia/Shanghai"=>"beijing peking shanghai","Asia/Kolkata"=>"new delhi india",_=>""};
                !self.settings.world_clock_cities.iter().any(|saved|saved==zone.name()) &&
                    (zone.name().to_lowercase().replace('_'," ").contains(&query.replace('_'," ")) || city_name(zone.name()).to_lowercase().contains(&query) || aliases.contains(&query))
            }).take(6).collect();
            if matches.is_empty(){ui.label("没有匹配的时区，请尝试城市英文名或 IANA 时区。");}
            ui.horizontal_wrapped(|ui|{for zone in matches {
                if ui.button(format!("+ {}",city_name(zone.name()))).on_hover_text(zone.name()).clicked(){
                    self.settings.world_clock_cities.push(zone.name().into());
                    self.world_clock.search.clear();
                    break;
                }
            }});
        });
        });
        ui.add_space(10.0);
        ui.label(egui::RichText::new("最多 6 个城市 · 自动处理夏令时 · 时间制式跟随常规设置").small().color(ui.visuals().weak_text_color()));
    }

    pub fn world_clock_window_ui(&mut self,ctx:&egui::Context,env:&Env) {
        if !self.world_clock.window_open{return;}
        let focus=std::mem::take(&mut self.world_clock.window_focus);
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("daozai-world-clock"),
            egui::ViewportBuilder::default().with_title("世界时钟 · 岛仔").with_icon(theme::app_icon()).with_inner_size([460.0,800.0]).with_min_inner_size([420.0,600.0]).with_visible(false).with_active(false),|ctx,_|{
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark));
            if focus{ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));ctx.send_viewport_cmd(egui::ViewportCommand::Focus);}
            if ctx.input(|i|i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)){self.world_clock.window_open=false;}
            egui::CentralPanel::default().frame(egui::Frame::new().fill(egui::Color32::from_rgb(13,15,18)).inner_margin(18)).show(ctx,|ui|{
                self.world_clock_window_content(ui,env);
            });
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        });
    }

    pub(crate) fn world_clock_window_content(&mut self,ui:&mut egui::Ui,env:&Env) {
        ui.visuals_mut().override_text_color=Some(theme::WHITE);
        ui.visuals_mut().widgets.inactive.bg_fill=theme::BTN;
        ui.visuals_mut().widgets.hovered.bg_fill=theme::BTN_HOVER;
        ui.visuals_mut().widgets.inactive.corner_radius=egui::CornerRadius::same(14);
        ui.visuals_mut().widgets.hovered.corner_radius=egui::CornerRadius::same(14);
        ui.visuals_mut().widgets.active.corner_radius=egui::CornerRadius::same(14);
        ui.spacing_mut().button_padding=egui::vec2(12.0,7.0);
        ui.style_mut().text_styles.insert(egui::TextStyle::Button,egui::FontId::proportional(11.5));
        let Some(now)=instant(env) else{ui.label("等待系统时间");return;};
        let now=now+chrono::Duration::hours(self.world_clock.preview_hours as i64);
        let home=now.with_timezone(&Local).fixed_offset();
        let width=ui.available_width();let scale=width/419.0;
        let (rect,_)=ui.allocate_exact_size(egui::vec2(width,96.0*scale),egui::Sense::hover());
        let mut c=Card::new(ui,rect.min,rect);c.s=scale;
        let subtitle=if self.world_clock.preview_hours==0{format!("此刻，本地时间 {}",self.settings.clock_text(&env.clock))}else{format!("时间对照 · 本地 {}",home.format("%m月%d日 %H:%M"))};
        c.centered_text(209.5,13.0,&subtitle,11.0,W::Regular,theme::GRAY,H::Center);
        c.centered_text(209.5,43.0,"世界时钟",24.0,W::Semibold,theme::WHITE,H::Center);
        c.fill(138.5,64.0,142.0,27.0,13.5,theme::CARD);
        for (x,label,globe) in [(141.5,"城市",false),(210.5,"地球",true)] {
            let selected=self.world_clock.globe_view==globe;
            let hit=c.hit(x,67.0,66.0,21.0,("world-window-view",globe));
            if selected {c.fill(x,67.0,66.0,21.0,10.5,theme::BTN_HOVER);}
            c.centered_text(x+33.0,77.5,label,11.0,W::Semibold,if selected{theme::WHITE}else{theme::GRAY},H::Center);
            if hit.clicked(){self.world_clock.globe_view=globe;}
        }
        let body_height=(ui.available_height()-52.0).max(220.0);
        let count=self.settings.world_clock_cities.len();
        self.world_clock.selected=self.world_clock.selected.min(count.saturating_sub(1));
        if self.world_clock.globe_view {
            let (rect,_)=ui.allocate_exact_size(egui::vec2(width,body_height),egui::Sense::hover());
            let mut c=Card::new(ui,rect.min,rect);c.s=scale;
            let height=body_height/scale;let diameter=370.0_f32.min(height-72.0);
            let top=((height-diameter-72.0)/2.0).max(8.0);
            let area=egui::Rect::from_min_size(egui::pos2((419.0-diameter)/2.0,top),egui::vec2(diameter,diameter));
            if let Some(index)=self.world_clock.globe.draw(&c,area,now,&self.settings.world_clock_cities,self.world_clock.selected,true){self.world_clock.selected=index;}
            if let Some(zone)=self.settings.world_clock_cities.get(self.world_clock.selected) {
                if let Ok(tz)=zone.parse::<Tz>() {
                    let value=reading(tz,now,home,self.settings.clock_12h);
                    c.centered_text(209.5,top+diameter+24.0,&format!("{}  {}",city_name(zone),value.time),15.0,W::Semibold,theme::WHITE,H::Center);
                    c.centered_text(209.5,top+diameter+45.0,"拖动旋转 · 点击城市 · 昼夜为示意",10.5,W::Regular,theme::GRAY,H::Center);
                }
            }
        } else {
            egui::ScrollArea::vertical().id_salt("world-clock-cities").max_height(body_height).auto_shrink([false,false]).show(ui,|ui|{
                ui.spacing_mut().item_spacing.y=8.0*scale;
                for (index,zone) in self.settings.world_clock_cities.iter().enumerate() {
                    let Ok(tz)=zone.parse::<Tz>()else{continue;};
                    let value=reading(tz,now,home,self.settings.clock_12h);
                    let (rect,_)=ui.allocate_exact_size(egui::vec2(width,88.0*scale),egui::Sense::hover());
                    let mut c=Card::new(ui,rect.min,rect);c.s=scale;
                    city_row(&c,zone,&value,0.0,0.0,419.0,88.0,true);
                    if c.hit(0.0,0.0,419.0,88.0,("world-window-city",index)).on_hover_text(format!("{} · 点击在地球上定位",value.difference)).clicked(){
                        self.world_clock.selected=index;self.world_clock.globe.center_on(zone);self.world_clock.globe_view=true;
                    }
                }
            });
        }
        ui.add_space(8.0);
        ui.horizontal(|ui|{
            for (label,delta) in [("−1h",-1),("+1h",1)] {
                if ui.add_enabled((-24..=24).contains(&(self.world_clock.preview_hours+delta)),egui::Button::new(label)).clicked(){self.world_clock.preview_hours+=delta;}
            }
            if ui.button("此刻").clicked(){self.world_clock.preview_hours=0;}
            if self.world_clock.globe_view {
                let choices=self.settings.world_clock_cities.clone();let selected=self.world_clock.selected;
                egui::ComboBox::from_id_salt("world-jump-city").selected_text(choices.get(selected).map(|zone|city_name(zone)).unwrap_or_default()).width(100.0).show_ui(ui,|ui|{
                    for (index,zone) in choices.iter().enumerate(){if ui.selectable_label(index==selected,city_name(zone)).clicked(){self.world_clock.selected=index;self.world_clock.globe.center_on(zone);}}
                });
            } else if ui.button("+ 添加城市").clicked(){self.settings_tab=12;self.open_settings();}
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn utc(text: &str) -> DateTime<Utc> {DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)}

    #[test]
    fn dst_transitions_keep_the_same_instant_across_cities() {
        let before=utc("2026-03-08T06:59:00Z");
        let after=utc("2026-03-08T07:00:00Z");
        let home=before.with_timezone(&chrono_tz::Asia::Shanghai).fixed_offset();
        assert_eq!(reading(chrono_tz::America::New_York,before,home,false).time,"01:59");
        let home=after.with_timezone(&chrono_tz::Asia::Shanghai).fixed_offset();
        let value=reading(chrono_tz::America::New_York,after,home,false);
        assert_eq!(value.time,"03:00");assert_eq!(value.difference,"比本地慢12时");
        let before=utc("2026-11-01T05:59:00Z");let after=utc("2026-11-01T06:00:00Z");
        assert_eq!(reading(chrono_tz::America::New_York,before,home,false).time,"01:59");
        assert_eq!(reading(chrono_tz::America::New_York,after,home,false).time,"01:00");
    }

    #[test]
    fn date_boundaries_and_fractional_offsets_are_visible() {
        let now=utc("2026-01-01T01:00:00Z");
        let home=now.with_timezone(&chrono_tz::Asia::Shanghai).fixed_offset();
        let ny=reading(chrono_tz::America::New_York,now,home,false);
        assert_eq!(ny.time,"20:00");assert!(ny.date.contains("12/31 周三 · 昨天"));assert!(!ny.daytime);
        let nepal=reading(chrono_tz::Asia::Kathmandu,now,home,true);
        assert_eq!(nepal.time,"上午 6:45");assert_eq!(nepal.difference,"比本地慢2时15分");assert!(nepal.daytime);
        let local=reading(chrono_tz::Asia::Shanghai,now,home,true);
        assert_eq!(local.time,"上午 9:00");assert!(local.date.ends_with("今天"));
        let home=now.with_timezone(&chrono_tz::America::Los_Angeles).fixed_offset();
        assert!(reading(chrono_tz::Pacific::Auckland,now,home,false).date.ends_with("明天"));
    }

    #[test]
    fn city_configuration_rejects_invalid_and_duplicate_entries() {
        assert_eq!(valid_cities(vec!["Bad/Zone".into(),"Asia/Tokyo".into(),"Asia/Tokyo".into()]),vec!["Asia/Tokyo"]);
        assert_eq!(valid_cities(chrono_tz::TZ_VARIANTS.iter().take(10).map(|zone|zone.name().into()).collect()).len(),MAX_CITIES);
    }
}
