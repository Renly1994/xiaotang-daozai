use crate::{bar_layout::*,paint::{self,Card},theme::{self,W}};
use eframe::egui::{self,Color32,RichText,Rect,Pos2,Vec2};

#[derive(Clone, Copy)]
struct ResidentDrag { slot: usize, dock: crate::model::Dock }

pub(crate) fn preview_layout(order:&[usize],quotas:usize,focus_running:bool)->Layout {
    let focus_width=if focus_running {crate::focus_bar::WIDTH}else{20.0};
    let mut layout=Layout::with_focus_width(true,quotas.max(1),true,true,false,true,focus_width);
    layout.add_pages([true;9],true);
    layout.reorder(order);layout
}

impl crate::app::App {
    pub(crate) fn resident_order_settings(&mut self,ui:&mut egui::Ui){
        let default=if self.settings.show_battery {DEFAULT_ORDER.to_vec()}else{vec![QUOTA,PET,CLOCK,BATTERY,MUSIC,FOCUS,WATER,SHELF,TODO,MAIL,STEAM]};
        let mut order=if valid_order(&self.settings.resident_order){self.settings.resident_order.clone()}else{default.clone()};
        ui.horizontal(|ui|{
            ui.label(RichText::new("拖动任一岛上的图标，可同步调整三处排列").color(ui.visuals().text_color()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                if ui.small_button("恢复默认").clicked(){self.settings.resident_order.clear();order=default;}
            });
        });
        ui.add_space(12.0);
        let indices=self.quota_bar_indices();let count=indices.len();
        let mut layout=preview_layout(&order,count,self.pomo.running);
        for (position,extra) in self.quota_balance_layout(ui.ctx()){layout.balance_tail(position,extra);}
        if self.settings.clock_period!=0 { layout.widen_clock(40.0); }
        self.resident_all_docks_preview(ui,&mut order,&layout,count);
    }

    fn resident_all_docks_preview(&mut self,ui:&mut egui::Ui,order:&mut Vec<usize>,layout:&Layout,count:usize){
        let side_height=26.0*order.len() as f32+14.0;
        let (stage,_)=ui.allocate_exact_size(Vec2::new(ui.available_width(),side_height+106.0),egui::Sense::hover());
        ui.painter().rect_filled(stage,10.0,Color32::from_rgb(19,25,36));
        let top_width=(stage.width()-112.0).max(1.0);
        let top_scale=(top_width/layout.width).min(1.0);
        let origin=Pos2::new(stage.center().x-layout.width*top_scale/2.0,stage.top()+12.0);
        let pill=self.settings.island_shape==crate::model::IslandShape::Pill;
        let origin=if pill{origin+Vec2::new(0.0,4.0*top_scale)}else{origin};
        let body=Rect::from_min_size(origin,Vec2::new(layout.width*top_scale,30.0*top_scale));
        let fill=self.preview_fill();
        if pill {ui.painter().rect_filled(body,15.0*top_scale,fill);}
        else {crate::shape::fill(ui.painter(),body,8.0*top_scale,10.0*top_scale,12.0*top_scale,fill,1.0/ui.ctx().pixels_per_point());}
        if self.settings.skin.is_glass() {
            let rim=egui::Stroke::new(1.0_f32,Color32::from_white_alpha(60));
            if pill {ui.painter().rect_stroke(body,15.0*top_scale,rim,egui::StrokeKind::Inside);}
            else {
                let edge=Rect::from_min_max(body.min+Vec2::new(0.5,0.0),body.max-Vec2::new(0.5,0.5));
                ui.painter().add(egui::Shape::line(crate::shape::outline(edge,8.0*top_scale,10.0*top_scale,12.0*top_scale-0.5),rim));
            }
        }
        let side_origin_y=stage.top()+72.0;
        let side_left=Rect::from_min_size(Pos2::new(stage.left()+10.0,side_origin_y),Vec2::new(36.0,side_height));
        let side_right=Rect::from_min_size(Pos2::new(stage.right()-46.0,side_origin_y),Vec2::new(36.0,side_height));
        for (dock,body) in [(crate::model::Dock::Left,side_left),(crate::model::Dock::Right,side_right)] {
            if pill {ui.painter().rect_filled(body,18.0,fill);}
            else {crate::shape::fill_docked(ui.painter(),body,dock,5.0,8.0,12.0,fill,1.0/ui.ctx().pixels_per_point());}
            if self.settings.skin.is_glass(){ui.painter().rect_stroke(body,12.0,egui::Stroke::new(1.0,Color32::from_white_alpha(60)),egui::StrokeKind::Inside);}
        }
        ui.painter().text(Pos2::new(stage.center().x,stage.top()+53.0),egui::Align2::CENTER_CENTER,"顶部",theme::font(11.0,W::Semibold),Color32::from_gray(175));
        ui.painter().text(Pos2::new(side_left.center().x,stage.top()+57.0),egui::Align2::CENTER_CENTER,"左侧",theme::font(11.0,W::Semibold),Color32::from_gray(175));
        ui.painter().text(Pos2::new(side_right.center().x,stage.top()+57.0),egui::Align2::CENTER_CENTER,"右侧",theme::font(11.0,W::Semibold),Color32::from_gray(175));
        let content_from=crate::skin_paint::shape_count(ui.ctx(),ui.layer_id());
        let mut top_card=Card::new(ui,origin,stage);top_card.s=top_scale;
        let side_left_card=Card::new(ui,Pos2::new(side_left.left(),side_left.top()),stage);
        let side_right_card=Card::new(ui,Pos2::new(side_right.left(),side_right.top()),stage);
        let cx=layout.width/2.0;
        let pointer=ui.ctx().pointer_interact_pos();
        let dragged=egui::DragAndDrop::payload::<ResidentDrag>(ui.ctx());
        let mut movement=None;
        for (index,&slot) in order.iter().enumerate(){
            let x=cx+layout.x[slot].unwrap();
            let quota_left=cx+layout.quota_slots().filter_map(|i|layout.x[i].map(|x|x-layout.widths[i]/2.0)).fold(f32::INFINITY,f32::min);
            let quota_right=cx+layout.quota_slots().filter_map(|i|layout.x[i].map(|x|x+layout.widths[i]/2.0)).fold(f32::NEG_INFINITY,f32::max);
            let width=match slot{CLOCK=>if self.settings.clock_period!=0 {84.0}else{44.0},BATTERY=>20.0,PET=>32.0,MUSIC=>14.0,FOCUS=>layout.focus_width,QUOTA=>quota_right-quota_left,_=>18.0};
            let left=if slot==QUOTA{quota_left}else{x-width/2.0};
            let rect=top_card.rect(left-5.0,1.0,width+10.0,28.0);
            let response=ui.interact(rect,ui.id().with(("resident-icon",slot)),egui::Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::Grab);
            response.dnd_set_drag_payload(ResidentDrag{slot,dock:crate::model::Dock::Top});
            if let (Some(from),Some(point))=(dragged.as_deref(),pointer){
                if from.slot!=slot && rect.contains(point) && ui.input(|i|i.pointer.primary_down()){movement=Some((from.slot,index));}
            }
            let is_dragged=dragged.as_deref().is_some_and(|drag|drag.slot==slot&&drag.dock==crate::model::Dock::Top);
            let dx=if is_dragged{pointer.map(|p|((p.x-origin.x)/top_scale-(left+width/2.0)).clamp(-left,layout.width-left-width)).unwrap_or(0.0)}else{0.0};
            if is_dragged {ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);}
            else if response.hovered(){ui.painter().rect_filled(rect,6.0,Color32::from_white_alpha(18));}
            let active=self.resident_preview_active(slot, crate::model::Dock::Top, count);
            let c=top_card.shifted(dx,0.0).faded(if active||is_dragged{1.0}else{0.38}).ghost();
            self.draw_resident_preview_icon(&c, slot, x, cx, &layout, count, false);
            if is_dragged {ui.painter().line_segment([Pos2::new(rect.center().x,body.bottom()+6.0),Pos2::new(rect.center().x,body.bottom()+12.0)],egui::Stroke::new(2.0,theme::BLUE));}
            response.on_hover_text(resident_label(slot));
        }
        for (dock,body,card) in [(crate::model::Dock::Left,side_left,side_left_card),(crate::model::Dock::Right,side_right,side_right_card)] {
            for (index,&slot) in order.iter().enumerate(){
                let step=26.0;
                let y=7.0+step*(index as f32+0.5);
                let rect=card.rect(0.0,y-step/2.0,36.0,step);
                let id=ui.id().with(("resident-icon",dock.key(),slot));
                let response=ui.interact(rect,id,egui::Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::Grab);
                response.dnd_set_drag_payload(ResidentDrag{slot,dock});
                if let (Some(from),Some(point))=(dragged.as_deref(),pointer){
                    if from.slot!=slot && rect.contains(point) && ui.input(|i|i.pointer.primary_down()){movement=Some((from.slot,index));}
                }
                let is_dragged=dragged.as_deref().is_some_and(|drag|drag.slot==slot&&drag.dock==dock);
                let dy=if is_dragged{pointer.map_or(0.0,|point|(point.y-body.top()).clamp(10.0,body.height()-10.0)-y)}else{0.0};
                if is_dragged{ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);}
                else if response.hovered(){ui.painter().rect_filled(rect,7.0,Color32::from_white_alpha(18));}
                let active=self.resident_preview_active(slot,dock,count);
                let c=card.shifted(0.0,y+dy-15.0).faded(if active||is_dragged{1.0}else{0.38}).ghost();
                self.draw_resident_preview_icon(&c,slot,18.0,18.0,layout,count,true);
                response.on_hover_text(resident_label(slot));
            }
        }
        if let Some(palette)=self.skin_palette() {
            let end=crate::skin_paint::shape_count(ui.ctx(),ui.layer_id());
            crate::skin_paint::recolor_layer(ui.ctx(),ui.layer_id(),&[(content_from,end)],&palette);
        }
        if let Some((slot,index))=movement{
            if let Some(from)=order.iter().position(|value|*value==slot){order.remove(from);order.insert(index.min(order.len()),slot);self.settings.resident_order.clone_from(order);}
        }
        ui.painter().text(Pos2::new(stage.center().x,stage.bottom()-14.0),egui::Align2::CENTER_CENTER,"在顶部左右拖动，在两侧上下拖动；三处共用排列顺序",theme::font(11.0,W::Regular),Color32::from_gray(160));
        ui.add_space(8.0);
        ui.label(RichText::new("灰色图标为暂未显示的组件；位置会保留。").size(11.0).color(ui.visuals().weak_text_color()));
    }

    fn resident_preview_active(&self, slot: usize, dock: crate::model::Dock, count: usize) -> bool {
        let page = match slot {
            MUSIC=>Some(0),FOCUS=>Some(1),WATER=>Some(2),SHELF=>Some(3),TODO=>Some(4),MAIL=>Some(5),QUOTA=>Some(6),
            CLOCK=>Some(crate::app::CALENDAR_PAGE),STEAM=>Some(8),_=>None,
        };
        let assigned = if self.settings.split_docks { match slot {
            BATTERY => self.settings.status_docks[0] == dock,
            PET => self.settings.dock == dock,
            _ => page.is_none_or(|page| self.settings.page_on(page,dock)),
        }} else { self.settings.dock == dock };
        assigned && match slot {
            BATTERY=>self.settings.show_battery && self.resident_power.0.is_some(),PET=>self.settings.pet,
            MUSIC=>self.media.is_music() || self.settings.resident_pages[0],FOCUS=>self.pomo.running || self.settings.resident_pages[1],
            QUOTA=>count>0 || self.settings.resident_pages[6],STEAM=>self.steam.badge().is_some() || self.settings.resident_pages[8],
            CLOCK=>true,
            _=>page.is_some_and(|page| self.settings.resident_pages[page]),
        }
    }

    fn draw_resident_preview_icon(&mut self, c: &Card, slot: usize, x: f32, cx: f32, layout: &Layout, count: usize, side: bool) {
        match slot{
                BATTERY=>paint::battery(&c,x,15.0,self.resident_power.0,self.resident_power.1,1.0),
                CLOCK if side=>{
                    let clock=self.settings.clock_text(&chrono::Local::now().format("%H:%M:%S").to_string());
                    let (hours,minutes)=crate::side_dock::side_clock(&clock);
                    c.centered_text(x,9.0,&hours,10.0,W::Bold,theme::WHITE,paint::H::Center);
                    c.centered_text(x,21.0,&minutes,10.0,W::Bold,theme::WHITE,paint::H::Center);
                },
                CLOCK=>{let text=self.settings.clock_text(&chrono::Local::now().format("%H:%M:%S").to_string());paint::clock(&c,x-paint::clock_width(&c,&text,theme::size::PILL_CLOCK)/2.0,21.0,&text,theme::size::PILL_CLOCK);},
                PET=>self.pet.draw(&c,x,self.t),
                MUSIC=>paint::playback_bars(&c,x,15.0,self.t,self.media.playing,theme::WHITE),
                FOCUS=>if self.pomo.running && !side{crate::focus_bar::draw(&c,x,15.0,self.t,&self.pomo,self.phase_secs(self.pomo.phase),false)}else{crate::focus_bar::shortcut(&c,x,15.0)},
                QUOTA if count>0 && !side=>self.draw_quota_bar(&c,cx,&layout),
                QUOTA if count>0=>{
                    if let Some((index,height))=self.quota_side_items().first().copied(){
                        self.draw_quota_side(&c.scaled(x,15.0,0.78),x,15.0,height,index);
                    }
                },
                QUOTA=>{c.circle(x,15.0,10.0,Color32::from_rgb(44,78,58));paint::target(&c,x,15.0,theme::SOFT_GREEN);},
                STEAM=>{
                    let game=self.steam.badge().map(|(_,g)|g.clone());
                    let icon=game.as_ref().and_then(|game|self.steam_images.get(c.ui.ctx(),game.icon.as_deref(),None));
                    if let Some(texture)=icon{c.painter.image(texture,c.rect(x-9.0,6.0,18.0,18.0),Rect::from_min_max(Pos2::ZERO,Pos2::new(1.0,1.0)),Color32::WHITE);}
                    else{crate::extras::device_icon(&c,crate::bt::DeviceKind::Gamepad,x-9.0,6.0,18.0);}
                },
                WATER=>paint::resident_water(&c,x,15.0),
                SHELF=>paint::resident_shelf(&c,x,15.0),
                TODO=>paint::resident_todo(&c,x,15.0),
                MAIL=>paint::resident_mail(&c,x,15.0),
                _=>{},
            }
    }

}

fn resident_label(slot: usize) -> &'static str {
    match slot {
        CLOCK => "时间 · 悬停打开日程",
        BATTERY => "电量",
        QUOTA => "AI 额度",
        MUSIC => "音乐",
        FOCUS => "专注",
        PET => "吉祥物",
        STEAM => "Steam",
        WATER => "饮水",
        SHELF => "置物架",
        TODO => "待办",
        MAIL => "邮箱",
        _ => "图标",
    }
}
