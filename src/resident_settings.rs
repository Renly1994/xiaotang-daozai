use crate::{bar_layout::*,paint::{self,Card},theme::{self,W}};
use eframe::egui::{self,Color32,RichText,Rect,Pos2,Vec2};

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
            ui.label(RichText::new("拖动岛上的图标").color(ui.visuals().text_color()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                if ui.small_button("恢复默认").clicked(){self.settings.resident_order.clear();order=default;}
            });
        });
        ui.add_space(12.0);
        let count=self.quota_bar_indices().len();
        let mut layout=preview_layout(&order,count,self.pomo.running);
        if self.settings.clock_period!=0 { layout.widen_clock(40.0); }
        let (stage,_)=ui.allocate_exact_size(Vec2::new(ui.available_width(),100.0),egui::Sense::hover());
        ui.painter().rect_filled(stage,10.0,Color32::from_rgb(19,25,36));
        let scale=((stage.width()-28.0)/layout.width).min(1.0);
        let origin=Pos2::new(stage.center().x-layout.width*scale/2.0,stage.top());
        let body=Rect::from_min_size(origin,Vec2::new(layout.width*scale,30.0*scale));
        crate::shape::fill(ui.painter(),body,8.0*scale,10.0*scale,12.0*scale,Color32::BLACK,1.0/ui.ctx().pixels_per_point());
        let mut card=Card::new(ui,origin,stage);card.s=scale;
        let cx=layout.width/2.0;
        let pointer=ui.ctx().pointer_interact_pos();
        let dragged=egui::DragAndDrop::payload::<usize>(ui.ctx());
        let mut movement=None;
        let label=|slot|match slot{CLOCK=>"时间 · 悬停打开日程",BATTERY=>"电量",QUOTA=>"AI 额度",MUSIC=>"音乐",FOCUS=>"专注",PET=>"吉祥物",STEAM=>"Steam",WATER=>"饮水",SHELF=>"置物架",TODO=>"待办",MAIL=>"邮箱",_=>"图标"};
        for (index,&slot) in order.iter().enumerate(){
            let x=cx+layout.x[slot].unwrap();
            let width=match slot{CLOCK=>if self.settings.clock_period!=0 {84.0}else{44.0},BATTERY=>20.0,PET=>32.0,MUSIC=>14.0,FOCUS=>layout.focus_width,QUOTA=>{
                let last=layout.x[MORE].or_else(||layout.x[QUOTA+count.min(3).max(1)-1]).unwrap();
                last-layout.x[QUOTA].unwrap()+27.0
            },_=>18.0};
            let left=if slot==QUOTA{x-13.5}else{x-width/2.0};
            let rect=card.rect(left-5.0,1.0,width+10.0,28.0);
            let response=ui.interact(rect,ui.id().with(("resident-icon",slot)),egui::Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::Grab);
            response.dnd_set_drag_payload(slot);
            if let (Some(from),Some(point))=(dragged.as_deref(),pointer){
                if *from!=slot && rect.contains(point) && ui.input(|i|i.pointer.primary_down()){movement=Some((*from,index));}
            }
            let is_dragged=dragged.as_deref()==Some(&slot);
            let dx=if is_dragged{pointer.map(|p|((p.x-origin.x)/scale-(left+width/2.0)).clamp(-left,layout.width-left-width)).unwrap_or(0.0)}else{0.0};
            if is_dragged {ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);}
            else if response.hovered(){ui.painter().rect_filled(rect,6.0,Color32::from_white_alpha(18));}
            let active=match slot{BATTERY=>self.settings.show_battery&&self.resident_power.0.is_some(),PET=>self.settings.pet,QUOTA=>count>0||self.settings.resident_pages[6],MUSIC=>self.media.is_music()||self.settings.resident_pages[0],FOCUS=>self.pomo.running||self.settings.resident_pages[1],STEAM=>self.steam.badge().is_some()||self.settings.resident_pages[8],WATER=>self.settings.resident_pages[2],SHELF=>self.settings.resident_pages[3],TODO=>self.settings.resident_pages[4],MAIL=>self.settings.resident_pages[5],_=>true};
            let c=card.shifted(dx,0.0).faded(if active||is_dragged{1.0}else{0.38}).ghost();
            match slot{
                BATTERY=>paint::battery(&c,x,15.0,self.resident_power.0,self.resident_power.1,1.0),
                CLOCK=>{let text=self.settings.clock_text(&chrono::Local::now().format("%H:%M:%S").to_string());paint::clock(&c,x-paint::clock_width(&c,&text,theme::size::PILL_CLOCK)/2.0,21.0,&text,theme::size::PILL_CLOCK);},
                PET=>self.pet.draw(&c,x,self.t),
                MUSIC=>paint::playback_bars(&c,x,15.0,self.t,self.media.playing,theme::WHITE),
                FOCUS=>if self.pomo.running{crate::focus_bar::draw(&c,x,15.0,self.t,&self.pomo,self.phase_secs(self.pomo.phase),false)}else{crate::focus_bar::shortcut(&c,x,15.0)},
                QUOTA if count>0=>self.draw_quota_bar(&c,cx,&layout),
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
            if is_dragged {ui.painter().line_segment([Pos2::new(rect.center().x,body.bottom()+6.0),Pos2::new(rect.center().x,body.bottom()+12.0)],egui::Stroke::new(2.0,theme::BLUE));}
            response.on_hover_text(label(slot));
        }
        if let Some((slot,index))=movement{
            if let Some(from)=order.iter().position(|value|*value==slot){order.remove(from);order.insert(index,slot);self.settings.resident_order=order;}
        }
        ui.painter().text(Pos2::new(stage.center().x,stage.bottom()-22.0),egui::Align2::CENTER_CENTER,"按住图标左右拖动",theme::font(11.0,W::Regular),Color32::from_gray(160));
        ui.add_space(8.0);
        ui.label(RichText::new("灰色图标为暂未显示的组件；位置会保留。").size(11.0).color(ui.visuals().weak_text_color()));
    }
}
