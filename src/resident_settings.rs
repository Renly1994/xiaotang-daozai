use crate::{bar_layout::*,paint::{self,Card},theme::{self,W}};
use eframe::egui::{self,Color32,Pos2,Rect,RichText,Vec2};
use std::time::Duration;

#[derive(Clone, Copy)]
struct ResidentDrag { slot: usize, dock: crate::model::Dock }

#[derive(Clone,Copy)]
struct TransferMotion {
    slot:usize,
    source:crate::model::Dock,
    phase:TransferPhase,
    target:Option<crate::model::Dock>,
    target_index:usize,
    origin:Pos2,
    source_edge:Pos2,
    current:Pos2,
    previous:Pos2,
    velocity:Vec2,
    sampled_at:f64,
    phase_started:f64,
    from:Pos2,
    to:Pos2,
    target_entry:Pos2,
}

#[derive(Clone,Copy,PartialEq,Eq)]
enum TransferPhase { Ejecting, Free, Engulfing, Returning, Settling }

#[derive(Clone,Copy)]
struct CrossDrop { slot:usize, source:crate::model::Dock, dock:crate::model::Dock, index:usize, from:Pos2 }

#[derive(Clone,Copy)]
struct TransferSkin { fill:Color32, rim:Color32, edge:Color32, sheen:Color32 }

fn preview_layout(order:&[usize],quotas:usize,focus_running:bool,assigned:[bool;crate::bar_layout::SLOT_COUNT])->Layout {
    let focus_width=if focus_running {crate::focus_bar::WIDTH}else{20.0};
    let mut layout=Layout::with_focus_width(
        assigned[BATTERY],
        if assigned[QUOTA]{quotas.max(1)}else{0},
        assigned[MUSIC],
        assigned[FOCUS],
        false,
        assigned[PET],
        focus_width,
    );
    if !assigned[CLOCK]{layout.remove_clock();}
    layout.add_pages([
        false,false,assigned[WATER],assigned[SHELF],assigned[TODO],assigned[MAIL],false,
        assigned[CLOCK],assigned[STEAM],assigned[CLIPBOARD],
    ],assigned[STEAM]);
    layout.reorder(order);
    layout
}

impl crate::app::App {
    pub(crate) fn resident_order_settings(&mut self,ui:&mut egui::Ui){
        let default=if self.settings.show_battery {DEFAULT_ORDER.to_vec()}else{vec![QUOTA,PET,CLOCK,BATTERY,MUSIC,FOCUS,WATER,SHELF,CLIPBOARD,TODO,MAIL,STEAM]};
        let mut order=if valid_order(&self.settings.resident_order){self.settings.resident_order.clone()}else{default.clone()};
        ui.horizontal(|ui|{
            ui.label(RichText::new("拖动任一岛上的图标，可同步调整三处排列").color(ui.visuals().text_color()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                if ui.small_button("恢复默认").clicked(){self.settings.resident_order.clear();order=default;}
            });
        });
        ui.add_space(12.0);
        let indices=self.quota_bar_indices();let count=indices.len();
        let assigned=std::array::from_fn(|slot|self.resident_preview_assigned(slot,crate::model::Dock::Top));
        let mut layout=preview_layout(&order,count,self.pomo.running,assigned);
        for (position,extra) in self.quota_balance_layout(ui.ctx()){layout.balance_tail(position,extra);}
        if assigned[CLOCK] && self.settings.clock_period!=0 { layout.widen_clock(40.0); }
        self.resident_all_docks_preview(ui,&mut order,&layout,count);
    }

    fn resident_all_docks_preview(&mut self,ui:&mut egui::Ui,order:&mut Vec<usize>,layout:&Layout,count:usize){
        use crate::model::Dock;
        let transfer_id=ui.id().with("resident-cross-dock-transfer");
        let mut motion=ui.ctx().data(|data|data.get_temp::<TransferMotion>(transfer_id));
        let pointer=ui.ctx().pointer_interact_pos();
        let dragged=egui::DragAndDrop::payload::<ResidentDrag>(ui.ctx()).as_deref().copied();
        let active_source=dragged.map(|drag|drag.dock)
            .or_else(||motion.filter(|state|matches!(state.phase,TransferPhase::Ejecting|TransferPhase::Free|TransferPhase::Engulfing)).map(|state|state.source));
        let preview_area=ui.available_rect_before_wrap();
        let preview_side_counts=[Dock::Left,Dock::Right].map(|dock|
            order.iter().filter(|slot|self.resident_preview_assigned(**slot,dock)).count());
        let preview_side_heights=preview_side_counts.map(|count|26.0*count.max(1) as f32+14.0);
        let preview_stage_height=preview_side_heights[0].max(preview_side_heights[1])+106.0;
        let preview_stage=Rect::from_min_size(preview_area.min,Vec2::new(ui.available_width(),preview_stage_height));
        let preview_top_scale=((preview_stage.width()-112.0).max(1.0)/layout.width).min(1.0);
        let preview_top_origin=Pos2::new(preview_stage.center().x-layout.width*preview_top_scale/2.0,preview_stage.top()+12.0);
        let pill=self.settings.island_shape==crate::model::IslandShape::Pill;
        let preview_top_origin=if pill{preview_top_origin+Vec2::new(0.0,4.0*preview_top_scale)}else{preview_top_origin};
        let preview_top=Rect::from_min_size(preview_top_origin,Vec2::new(layout.width*preview_top_scale,30.0*preview_top_scale));
        let preview_side_y=preview_stage.top()+72.0;
        let preview_left=Rect::from_min_size(Pos2::new(preview_stage.left()+10.0,preview_side_y),Vec2::new(36.0,preview_side_heights[0]));
        let preview_right=Rect::from_min_size(Pos2::new(preview_stage.right()-46.0,preview_side_y),Vec2::new(36.0,preview_side_heights[1]));
        let preview_bodies=[(Dock::Top,preview_top),(Dock::Left,preview_left),(Dock::Right,preview_right)];
        let preview_outlines=[
            island_outline(preview_top,Dock::Top,pill,preview_top_scale),
            island_outline(preview_left,Dock::Left,pill,1.0),
            island_outline(preview_right,Dock::Right,pill,1.0),
        ];
        let preview_target=pointer.and_then(|point|preview_bodies.iter()
            .filter(|(dock,_)|active_source.map_or(true,|source|source!=*dock))
            .filter_map(|(dock,_)|{
                let outline=&preview_outlines[dock.index()];
                let distance=nearest_boundary(outline,point).distance(point);
                (polygon_contains(outline,point)||distance<=48.0).then_some((*dock,distance))
            })
            .min_by(|left,right|left.1.total_cmp(&right.1))
            .map(|(dock,_)|dock));
        let preview_inside=dragged.and_then(|drag|{
            let target=preview_target?;
            let point=pointer?;
            if !motion.is_some_and(|state|state.phase==TransferPhase::Free){return None;}
            let expanded_body=match target{
                Dock::Top=>{
                    let mut assigned=std::array::from_fn(|slot|self.resident_preview_assigned(slot,Dock::Top));
                    if drag.slot<assigned.len(){assigned[drag.slot]=true;}
                    let mut expanded=preview_layout(order,count,self.pomo.running,assigned);
                    if assigned[CLOCK]&&self.settings.clock_period!=0{expanded.widen_clock(40.0);}
                    for (position,extra) in self.quota_balance_layout(ui.ctx()){expanded.balance_tail(position,extra);}
                    let scale=((preview_stage.width()-112.0).max(1.0)/expanded.width).min(1.0);
                    let y=preview_stage.top()+12.0+if pill{4.0*scale}else{0.0};
                    Rect::from_min_size(Pos2::new(preview_stage.center().x-expanded.width*scale/2.0,y),Vec2::new(expanded.width*scale,30.0*scale))
                }
                Dock::Left|Dock::Right=>{
                    let index=if target==Dock::Left{0}else{1};
                    let count=preview_side_counts[index]+usize::from(!self.resident_preview_assigned(drag.slot,target));
                    let height=26.0*count.max(1) as f32+14.0;
                    let x=if target==Dock::Left{preview_stage.left()+10.0}else{preview_stage.right()-46.0};
                    Rect::from_min_size(Pos2::new(x,preview_side_y),Vec2::new(36.0,height))
                }
            };
            expanded_body.contains(point).then_some((drag.slot,drag.dock,target))
        });
        let inbound=dragged.and_then(|drag|preview_target.filter(|target|*target!=drag.dock)
                .map(|target|(drag.slot,drag.dock,target)))
            .or_else(||motion.filter(|state|state.phase==TransferPhase::Engulfing)
                .and_then(|state|state.target.filter(|target|*target!=state.source)
                    .map(|target|(state.slot,state.source,target))));
        let mut layout=layout.clone();
        if let Some((slot,source,target))=inbound.filter(|(slot,source,target)|
            *target==Dock::Top||preview_inside.is_some_and(|(moving,from,to)|moving==*slot&&from==*source&&from==Dock::Top&&to!=Dock::Top)){
            let mut assigned=std::array::from_fn(|slot|self.resident_preview_assigned(slot,Dock::Top));
            if slot<assigned.len(){
                if target==Dock::Top{assigned[slot]=true;}
                if preview_inside.is_some_and(|(moving,from,to)|moving==slot&&from==Dock::Top&&to!=Dock::Top){assigned[slot]=false;}
            }
            let mut expanded=preview_layout(order,count,self.pomo.running,assigned);
            if assigned[CLOCK]&&self.settings.clock_period!=0{expanded.widen_clock(40.0);}
            for (position,extra) in self.quota_balance_layout(ui.ctx()){expanded.balance_tail(position,extra);}
            layout=expanded;
        }
        let animate_size=crate::bar_layout::animations_enabled()
            &&(inbound.is_some()||motion.is_some_and(|state|matches!(state.phase,TransferPhase::Free|TransferPhase::Engulfing|TransferPhase::Settling)));
        // The dock surface leads item motion slightly so icons never outrun its edge.
        let size_duration=if motion.is_some_and(|state|state.phase==TransferPhase::Settling){0.12}else{0.14};
        let animated_top_width=if animate_size{
            ui.ctx().animate_value_with_time(ui.id().with("resident-top-island-width"),layout.width,size_duration)
        }else{layout.width};
        let mut side_counts=[Dock::Left,Dock::Right].map(|dock|
            order.iter().filter(|slot|self.resident_preview_assigned(**slot,dock)).count());
        if let Some((slot,source,target))=inbound{
            if preview_inside.is_some_and(|(moving,from,to)|moving==slot&&from==source&&from.is_side()&&to!=from){
                let source_index=if source==Dock::Left{0}else{1};
                side_counts[source_index]=side_counts[source_index].saturating_sub(1);
            }
            if target.is_side()&&!self.resident_preview_assigned(slot,target){
                let target_index=if target==Dock::Left{0}else{1};
                side_counts[target_index]+=1;
            }
        }
        let side_height_targets=side_counts.map(|count|26.0*count.max(1) as f32+14.0);
        let side_heights=[Dock::Left,Dock::Right].map(|dock|{
            let index=if dock==Dock::Left{0}else{1};
            if animate_size{
                ui.ctx().animate_value_with_time(ui.id().with(("resident-side-island-height",dock.key())),side_height_targets[index],size_duration)
            }else{side_height_targets[index]}
        });
        let side_height=side_heights[0].max(side_heights[1]);
        let (stage,_)=ui.allocate_exact_size(Vec2::new(ui.available_width(),side_height+106.0),egui::Sense::hover());
        ui.painter().rect_filled(stage,10.0,Color32::from_rgb(19,25,36));
        let top_width=(stage.width()-112.0).max(1.0);
        let top_scale=(top_width/layout.width).min(1.0);
        let origin=Pos2::new(stage.center().x-layout.width*top_scale/2.0,stage.top()+12.0);
        let pill=self.settings.island_shape==crate::model::IslandShape::Pill;
        let origin=if pill{origin+Vec2::new(0.0,4.0*top_scale)}else{origin};
        let top_body=Rect::from_center_size(Pos2::new(stage.center().x,origin.y+15.0*top_scale),Vec2::new(animated_top_width*top_scale,30.0*top_scale));
        let side_origin_y=stage.top()+72.0;
        let side_left=Rect::from_min_size(Pos2::new(stage.left()+10.0,side_origin_y),Vec2::new(36.0,side_heights[0]));
        let side_right=Rect::from_min_size(Pos2::new(stage.right()-46.0,side_origin_y),Vec2::new(36.0,side_heights[1]));
        let bodies=[(Dock::Top,top_body),(Dock::Left,side_left),(Dock::Right,side_right)];
        let outlines=[
            island_outline(top_body,Dock::Top,pill,top_scale),
            island_outline(side_left,Dock::Left,pill,1.0),
            island_outline(side_right,Dock::Right,pill,1.0),
        ];
        let fill=self.preview_fill();
        let transfer_skin=self.resident_transfer_skin(fill);
        if pill {ui.painter().rect_filled(top_body,15.0*top_scale,fill);}
        else {crate::shape::fill(ui.painter(),top_body,8.0*top_scale,10.0*top_scale,12.0*top_scale,fill,1.0/ui.ctx().pixels_per_point());}
        for (dock,rect) in [(Dock::Left,side_left),(Dock::Right,side_right)] {
            if pill {ui.painter().rect_filled(rect,18.0,fill);}
            else {crate::shape::fill_docked(ui.painter(),rect,dock,5.0,8.0,12.0,fill,1.0/ui.ctx().pixels_per_point());}
            if self.settings.skin.is_glass(){ui.painter().rect_stroke(rect,12.0,egui::Stroke::new(1.0_f32,Color32::from_white_alpha(60)),egui::StrokeKind::Inside);}
        }
        if self.settings.skin.is_glass() {
            let rim=egui::Stroke::new(1.0_f32,Color32::from_white_alpha(60));
            if pill {ui.painter().rect_stroke(top_body,15.0*top_scale,rim,egui::StrokeKind::Inside);}
            else {
                let edge=Rect::from_min_max(top_body.min+Vec2::new(0.5,0.0),top_body.max-Vec2::new(0.5,0.5));
                ui.painter().add(egui::Shape::line(crate::shape::outline(edge,8.0*top_scale,10.0*top_scale,12.0*top_scale-0.5),rim));
            }
        }

        let now=ui.input(|input|input.time);
        let motion_progress=motion.map(|animation|smooth01(((now-animation.phase_started)/phase_duration(animation.phase)).clamp(0.0,1.0) as f32));
        let hover_dock=pointer.and_then(|point|{
            bodies.iter()
                .filter(|(dock,_)|active_source.map_or(true,|source|source!=*dock))
                .filter_map(|(dock,_)|{
                    let outline=&outlines[dock.index()];
                    let distance=nearest_boundary(outline,point).distance(point);
                    (polygon_contains(outline,point)||distance<=48.0).then_some((*dock,distance))
                })
                .min_by(|left,right|left.1.total_cmp(&right.1))
                .map(|(dock,_)|dock)
        });
        // Use the same expanded, non-animated hit area that removed the source
        // placeholder above. Testing the animated island body here makes entry
        // toggle while the island is growing: the target list and transfer
        // bubble alternately claim the dragged icon, which looks like flicker.
        let entered_target=preview_inside;
        let target_dock=entered_target.map(|(_,_,target)|target)
            .or_else(||hover_dock.filter(|dock|active_source.is_some_and(|source|source!=*dock)));
        let inside_assignment=entered_target.map(|(slot,_,target)|(slot,target));
        let live_drag=entered_target.map(|(slot,_,dock)|ResidentDrag{slot,dock}).or(dragged);
        // Blend the membrane skin only; the transfer layer remains the sole
        // visible copy of the dragged component until the handoff completes.
        let internal_drag_blend=if crate::bar_layout::animations_enabled(){
            ui.ctx().animate_value_with_time(
                ui.id().with("resident-cross-dock-internal-drag"),
                if entered_target.is_some(){1.0}else{0.0},
                0.14,
            )
        }else if entered_target.is_some(){1.0}else{0.0};
        for (dock,_) in bodies {
            let highlighted=target_dock==Some(dock);
            let color=if highlighted {transfer_skin.rim}else{Color32::from_gray(175)};
            ui.painter().text(Pos2::new(match dock{Dock::Top=>stage.center().x,Dock::Left=>side_left.center().x,Dock::Right=>side_right.center().x},stage.top()+if dock==Dock::Top{53.0}else{57.0}),egui::Align2::CENTER_CENTER,dock.label(),theme::font(11.0,W::Semibold),color);
        }

        // 先注册整块投放区，再注册图标响应：投到空白处也能切换停靠，图标仍优先接收排序。
        let zones=[
            (Dock::Top,ui.interact(top_body,ui.id().with(("resident-drop-zone",Dock::Top.key())),egui::Sense::hover())),
            (Dock::Left,ui.interact(side_left,ui.id().with(("resident-drop-zone",Dock::Left.key())),egui::Sense::hover())),
            (Dock::Right,ui.interact(side_right,ui.id().with(("resident-drop-zone",Dock::Right.key())),egui::Sense::hover())),
        ];
        let content_from=crate::skin_paint::shape_count(ui.ctx(),ui.layer_id());
        let mut top_card=Card::new(ui,origin,stage);top_card.s=top_scale;
        let side_left_card=Card::new(ui,Pos2::new(side_left.left(),side_left.top()),stage);
        let side_right_card=Card::new(ui,Pos2::new(side_right.left(),side_right.top()),stage);
        let cx=layout.width/2.0;
        let transfer_state=motion.filter(|state|matches!(state.phase,TransferPhase::Ejecting|TransferPhase::Free|TransferPhase::Engulfing));
        let reorder_target=target_dock.or_else(||transfer_state.and_then(|state|state.target));
        // Keep the drag's original source while it is captured by a target.
        // The source/target distinction keeps the target's live reorder lane active.
        let reorder_source=active_source;
        let cross_reorder=reorder_source.zip(reorder_target)
            .filter(|(source,target)|source!=target)
            .and_then(|(_,target)|{
                let target_slots:Vec<_>=order.iter().copied().filter(|slot|self.resident_preview_assigned(*slot,target)).collect();
                let positions:Vec<_>=match target {
                    Dock::Top=>target_slots.iter().filter_map(|slot|layout.x.get(*slot).copied().flatten()
                        .map(|offset|(*slot,Pos2::new(stage.center().x+offset*top_scale,top_body.center().y)))).collect(),
                    Dock::Left|Dock::Right=>target_slots.iter().enumerate()
                        .map(|(index,slot)|(*slot,Pos2::new(bodies[target.index()].1.center().x,side_origin_y+20.0+26.0*index as f32))).collect(),
                };
                let live_target=target_dock==Some(target)||(
                    dragged.is_none()&&transfer_state.is_some_and(|state|matches!(state.phase,TransferPhase::Ejecting|TransferPhase::Free)&&state.target==Some(target))
                );
                let rank=if live_target {
                    pointer.map(|point|{
                        let coordinate=if target==Dock::Top{point.x}else{point.y};
                        positions.iter().position(|(_,center)|coordinate<if target==Dock::Top{center.x}else{center.y}).unwrap_or(positions.len())
                    })
                }else{
                    transfer_state.filter(|state|state.target==Some(target)).map(|state|{
                        positions.iter().position(|(slot,_)|order.iter().position(|value|value==slot).unwrap_or(order.len())>=state.target_index).unwrap_or(positions.len())
                    })
                }?;
                let incoming=dragged.map(|drag|drag.slot).or_else(||transfer_state.map(|state|state.slot))?;
                let incoming_order_index=order.iter().position(|slot|*slot==incoming).unwrap_or(order.len());
                let base_rank=target_slots.iter().filter(|slot|
                    order.iter().position(|value|value==*slot).unwrap_or(order.len())<incoming_order_index
                ).count();
                let incoming_width=resident_preview_slot_width(incoming,&layout,self.settings.clock_period);
                let gap=if target==Dock::Top{incoming_width+12.0}else{26.0};
                Some((target,rank,base_rank,gap))
            });
        let cross_snap_target=cross_reorder.and_then(|(target,rank,base_rank,_)|{
            let incoming=dragged.map(|drag|drag.slot).or_else(||transfer_state.map(|state|state.slot))?;
            let center=if target==Dock::Top{
                let mut x=layout.x.get(incoming).copied().flatten()?;
                let incoming_width=resident_preview_slot_width(incoming,&layout,self.settings.clock_period);
                let slots:Vec<_>=order.iter().copied().filter(|slot|self.resident_preview_assigned(*slot,target)).collect();
                if rank>base_rank{
                    for slot in slots.iter().skip(base_rank).take(rank-base_rank){
                        x+=(incoming_width+resident_preview_slot_width(*slot,&layout,self.settings.clock_period))*0.5+12.0;
                    }
                }else if rank<base_rank{
                    for slot in slots.iter().skip(rank).take(base_rank-rank){
                        x-=(incoming_width+resident_preview_slot_width(*slot,&layout,self.settings.clock_period))*0.5+12.0;
                    }
                }
                Pos2::new(stage.center().x+x*top_scale,top_body.center().y)
            }else{
                let body=bodies[target.index()].1;
                Pos2::new(body.center().x,side_origin_y+7.0+26.0*(rank as f32+0.5))
            };
            Some((target,center))
        });
        let native_snap_target=entered_target.and_then(|(slot,_,dock)|cross_snap_target
            .filter(|(target,_)|*target==dock).map(|(_,center)|center)
            .or_else(||match dock{
            Dock::Top=>layout.x.get(slot).copied().flatten()
                .map(|offset|Pos2::new(stage.center().x+offset*top_scale,top_body.center().y)),
            Dock::Left|Dock::Right=>{
                let slots:Vec<_>=order.iter().copied().filter(|candidate|
                    self.resident_preview_assigned_for_transfer(*candidate,dock,inside_assignment)
                ).collect();
                let index=slots.iter().position(|candidate|*candidate==slot)?;
                Some(Pos2::new(bodies[dock.index()].1.center().x,side_origin_y+7.0+26.0*(index as f32+0.5)))
            }
        }));
        let visual_pointer=pointer.map(|point|{
            if let Some(center)=native_snap_target{
                let attraction=smooth01(((34.0-point.distance(center))/22.0).clamp(0.0,1.0))*internal_drag_blend;
                point.lerp(center,attraction)
            }else{point}
        });
        let animate_positions=crate::bar_layout::animations_enabled();
        let settling=motion.filter(|state|state.phase==TransferPhase::Settling);
        // Continue using the destination island's normal drag projection after
        // capture. This keeps the single transfer copy attached to the pointer.
        let handoff_center=entered_target.and_then(|(_,_,target)|{
            let point=visual_pointer?;
            let body=bodies[target.index()].1;
            Some(match target{
                Dock::Top=>Pos2::new(point.x,body.center().y),
                Dock::Left|Dock::Right=>Pos2::new(body.center().x,point.y.clamp(body.top()+10.0,body.bottom()-10.0)),
            })
        });
        let drag_visual_pointer=handoff_center.or(visual_pointer);
        let same_dock_drag=dragged
            .filter(|drag|active_source==Some(drag.dock))
            .filter(|drag|pointer.is_some_and(|point|bodies[drag.dock.index()].1.contains(point)))
            .map(|drag|drag.dock);
        let animate_top_reorder=same_dock_drag==Some(Dock::Top)
            ||reorder_source.is_some_and(|source|source!=Dock::Top)
            ||transfer_state.is_some_and(|state|state.phase==TransferPhase::Engulfing&&state.target==Some(Dock::Top))
            ||entered_target.is_some_and(|(_,_,dock)|dock==Dock::Top)
            ||settling.is_some_and(|state|state.source==Dock::Top||state.target==Some(Dock::Top));
        let animate_left_reorder=same_dock_drag==Some(Dock::Left)
            ||reorder_source.is_some_and(|source|source!=Dock::Left)
            ||transfer_state.is_some_and(|state|state.phase==TransferPhase::Engulfing&&state.target==Some(Dock::Left))
            ||entered_target.is_some_and(|(_,_,dock)|dock==Dock::Left)
            ||settling.is_some_and(|state|state.source==Dock::Left||state.target==Some(Dock::Left));
        let animate_right_reorder=same_dock_drag==Some(Dock::Right)
            ||reorder_source.is_some_and(|source|source!=Dock::Right)
            ||transfer_state.is_some_and(|state|state.phase==TransferPhase::Engulfing&&state.target==Some(Dock::Right))
            ||entered_target.is_some_and(|(_,_,dock)|dock==Dock::Right)
            ||settling.is_some_and(|state|state.source==Dock::Right||state.target==Some(Dock::Right));
        let top_slots:Vec<_>=order.iter().copied().filter(|slot|self.resident_preview_assigned_for_transfer(*slot,Dock::Top,inside_assignment)).collect();
        let mut movement=None;
        let mut cross_drop=None;
        let mut dock_positions:[Vec<(usize,Pos2)>;3]=std::array::from_fn(|_|Vec::new());

        for &slot in order.iter(){
            if !self.resident_preview_assigned_for_transfer(slot,Dock::Top,inside_assignment){continue;}
            let Some(offset)=layout.x.get(slot).copied().flatten() else{continue;};
            let index=order.iter().position(|value|*value==slot).unwrap_or(0);
            let base_x=cx+offset;
            let shift=cross_reorder.filter(|(dock,_,_,_)|*dock==Dock::Top)
                .map(|(_,insert,base,gap)|{
                    let index=top_slots.iter().position(|value|*value==slot).unwrap_or(usize::MAX);
                    if insert>base&&index>=base&&index<insert{-gap}
                    else if insert<base&&index>=insert&&index<base{gap}
                    else{0.0}
                })
                .unwrap_or(0.0);
            let target_x=base_x+shift;
            let is_dragged=live_drag.is_some_and(|drag|drag.slot==slot&&drag.dock==Dock::Top);
            let x=if animate_positions&&animate_top_reorder&&!is_dragged {ui.ctx().animate_value_with_time(ui.id().with(("resident-preview-x",slot)),target_x,0.18)}else{target_x};
            let quota_left=cx+layout.quota_slots().filter_map(|i|layout.x[i].map(|x|x-layout.widths[i]/2.0)).fold(f32::INFINITY,f32::min);
            let quota_right=cx+layout.quota_slots().filter_map(|i|layout.x[i].map(|x|x+layout.widths[i]/2.0)).fold(f32::NEG_INFINITY,f32::max);
            let width=resident_preview_slot_width(slot,&layout,self.settings.clock_period);
            // Keep drag hit targets at the destination positions. Using the
            // interpolated visual position lets adjacent hit targets overlap
            // during cross-dock entry, so the hovered neighbor changes every
            // frame and repeatedly reorders the same item.
            let hit_left=if slot==QUOTA&&quota_left.is_finite(){quota_left+shift}else{target_x-width/2.0};
            let rect=top_card.rect(hit_left-5.0,1.0,width+10.0,28.0);
            let response=ui.interact(rect,ui.id().with(("resident-icon",slot)),egui::Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::Grab);
            response.dnd_set_drag_payload(ResidentDrag{slot,dock:Dock::Top});
            if let (Some(from),Some(point))=(live_drag,pointer){
                if active_source==Some(Dock::Top)&&from.dock==Dock::Top&&from.slot!=slot&&rect.contains(point)&&ui.input(|input|input.pointer.primary_down()){movement=Some((from.slot,index));}
            }
            if let Some(released)=response.dnd_release_payload::<ResidentDrag>() {
                if released.dock!=Dock::Top {cross_drop=Some(CrossDrop{slot:released.slot,source:released.dock,dock:Dock::Top,index,from:pointer.unwrap_or(rect.center())});}
            }
            let source_body=bodies[Dock::Top.index()].1;
            let transfer_drag=is_dragged&&pointer.is_some_and(|point|!source_body.contains(point));
            let dx=if is_dragged&&!transfer_drag{drag_visual_pointer.map(|p|((p.x-origin.x)/top_scale-(hit_left+width/2.0)).clamp(-hit_left,layout.width-hit_left-width)).unwrap_or(0.0)}else{0.0};
            if is_dragged {ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);}
            else if response.hovered(){ui.painter().rect_filled(rect,6.0,Color32::from_white_alpha(18));}
            let active=self.resident_preview_active(slot,count);
            let staged_alpha=transfer_icon_alpha(motion,motion_progress,slot,Dock::Top);
            let source_alpha=if transfer_drag{if animations_enabled(){0.0}else{0.35}}else{1.0};
            let internal_alpha=if entered_target.is_some_and(|(moving,_,dock)|moving==slot&&dock==Dock::Top){0.0}else{1.0};
            let draw_dx=dx+if slot==QUOTA{shift}else{0.0};
            let c=top_card.shifted(draw_dx,0.0).faded((if active{1.0}else{0.38})*staged_alpha*source_alpha*internal_alpha).ghost();
            self.draw_resident_preview_icon(&c,slot,x,cx,&layout,count,false);
            dock_positions[Dock::Top.index()].push((slot,rect.center()));
            response.on_hover_text(resident_label(slot));
        }

        for (dock,body,card) in [(Dock::Left,side_left,side_left_card),(Dock::Right,side_right,side_right_card)] {
            // The captured component is rendered by the transfer layer while the
            // target island reorders around its insertion point. Do not also count
            // its hidden native slot here: `virtual_index` already reserves that
            // single position for the moving component.
            let slots:Vec<_>=order.iter().copied()
                .filter(|slot|self.resident_preview_assigned_for_transfer(*slot,dock,inside_assignment))
                .filter(|slot|!inside_assignment.is_some_and(|(moving,target)|moving==*slot&&target==dock))
                .collect();
            for (dock_index,slot) in slots.into_iter().enumerate(){
                let index=order.iter().position(|value|*value==slot).unwrap_or(0);
                let step=26.0;
                let insertion=cross_reorder.filter(|(target,_,_,_)|*target==dock).map(|(_,rank,_,_)|rank);
                let virtual_index=dock_index+usize::from(insertion.is_some_and(|rank|dock_index>=rank));
                let target_y=7.0+step*(virtual_index as f32+0.5);
                let is_dragged=live_drag.is_some_and(|drag|drag.slot==slot&&drag.dock==dock);
                let animate_side=if dock==Dock::Left{animate_left_reorder}else{animate_right_reorder};
                let y=if animate_positions&&animate_side&&!is_dragged {ui.ctx().animate_value_with_time(ui.id().with(("resident-preview-y",dock.key(),slot)),target_y,0.18)}else{target_y};
                let rect=card.rect(0.0,target_y-step/2.0,36.0,step);
                let visual_rect=card.rect(0.0,y-step/2.0,36.0,step);
                let id=ui.id().with(("resident-icon",dock.key(),slot));
                let response=ui.interact(rect,id,egui::Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::Grab);
                response.dnd_set_drag_payload(ResidentDrag{slot,dock});
                if let (Some(from),Some(point))=(live_drag,pointer){
                    if active_source==Some(dock)&&from.dock==dock&&from.slot!=slot&&rect.contains(point)&&ui.input(|input|input.pointer.primary_down()){movement=Some((from.slot,index));}
                }
                if let Some(released)=response.dnd_release_payload::<ResidentDrag>() {
                    if released.dock!=dock {cross_drop=Some(CrossDrop{slot:released.slot,source:released.dock,dock,index,from:pointer.unwrap_or(rect.center())});}
                }
                let transfer_drag=is_dragged&&pointer.is_some_and(|point|!body.contains(point));
                let dy=if is_dragged&&!transfer_drag{drag_visual_pointer.map_or(0.0,|point|(point.y-body.top()).clamp(10.0,body.height()-10.0)-y)}else{0.0};
                if is_dragged{ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);}
                else if response.hovered(){ui.painter().rect_filled(visual_rect,7.0,Color32::from_white_alpha(18));}
                let active=self.resident_preview_active(slot,count);
                let staged_alpha=transfer_icon_alpha(motion,motion_progress,slot,dock);
                let source_alpha=if transfer_drag{if animations_enabled(){0.0}else{0.35}}else{1.0};
                let internal_alpha=if entered_target.is_some_and(|(moving,_,target)|moving==slot&&target==dock){0.0}else{1.0};
                let c=card.shifted(0.0,y+dy-15.0).faded((if active{1.0}else{0.38})*staged_alpha*source_alpha*internal_alpha).ghost();
                self.draw_resident_preview_icon(&c,slot,18.0,18.0,&layout,count,true);
                dock_positions[dock.index()].push((slot,visual_rect.center()));
                response.on_hover_text(resident_label(slot));
            }
        }

        for (dock,zone) in zones {
            if let Some(released)=zone.dnd_release_payload::<ResidentDrag>() {
                if released.dock!=dock&&cross_drop.is_none(){
                    let point=pointer.unwrap_or_else(||bodies[dock.index()].1.center());
                    let positions:Vec<_>=dock_positions[dock.index()].iter().copied()
                        .filter(|(slot,_)|*slot!=released.slot).collect();
                    let index=insertion_index(order,&positions,dock,point);
                    cross_drop=Some(CrossDrop{slot:released.slot,source:released.dock,dock,index,from:point});
                }
            }
        }

        let snap_target=cross_snap_target.or_else(||entered_target.and_then(|(_,_,target)|native_snap_target.map(|center|(target,center))));

        if let Some(palette)=self.skin_palette() {
            let end=crate::skin_paint::shape_count(ui.ctx(),ui.layer_id());
            crate::skin_paint::recolor_layer(ui.ctx(),ui.layer_id(),&[(content_from,end)],&palette);
        }

        let animations=crate::bar_layout::animations_enabled();
        if let (Some(drag),Some(pointer))=(dragged,pointer) {
            let source_body=bodies[drag.dock.index()].1;
            let source_outline=&outlines[drag.dock.index()];
            let source_origin=dock_positions[drag.dock.index()].iter().find(|(slot,_)|*slot==drag.slot).map(|(_,center)|*center);
            let active=motion.filter(|state|state.slot==drag.slot&&state.source==drag.dock&&matches!(state.phase,TransferPhase::Ejecting|TransferPhase::Free));
            if animations&&(active.is_some()||!source_body.contains(pointer)) {
                if let Some(origin)=source_origin {
                    let mut state=active.unwrap_or(TransferMotion{
                        slot:drag.slot,source:drag.dock,phase:TransferPhase::Ejecting,target:None,target_index:0,
                        origin,source_edge:boundary_toward(source_outline,origin,pointer),current:pointer,previous:pointer,
                        velocity:Vec2::ZERO,sampled_at:now,phase_started:now,from:origin,to:origin,target_entry:origin,
                    });
                    let dt=(now-state.sampled_at).clamp(1.0/240.0,0.12) as f32;
                    let instant=(pointer-state.previous)/dt;
                    state.velocity=state.velocity*0.55+instant*0.45;
                    state.current=if state.phase==TransferPhase::Free{
                        if let Some(handoff)=handoff_center{handoff}else{
                        let followed=state.current.lerp(pointer,0.62);
                        if let Some((target,slot_center))=snap_target.filter(|(target,_)|Some(*target)==target_dock){
                            let attraction=smooth01(((34.0-pointer.distance(slot_center))/22.0).clamp(0.0,1.0));
                            followed.lerp(slot_center,attraction)
                        }else{followed}
                        }
                    }else{pointer};
                    state.previous=pointer;state.sampled_at=now;state.target=target_dock;
                    if source_body.contains(pointer) {
                        if state.phase!=TransferPhase::Returning {
                            state.phase=TransferPhase::Returning;state.phase_started=now;state.from=state.current;state.to=origin;state.target=None;
                        }
                    } else if state.phase==TransferPhase::Ejecting && now-state.phase_started>=phase_duration(TransferPhase::Ejecting) {
                        let direction=(state.current-state.source_edge).normalized();
                        state.current=state.source_edge+direction*(16.0*0.62);
                        state.phase=TransferPhase::Free;state.phase_started=now;
                    }
                    motion=Some(state);
                }
            }else if active.is_none()&&source_body.contains(pointer){
                motion=motion.filter(|state|!matches!(state.phase,TransferPhase::Ejecting|TransferPhase::Free));
            }
        }

        if let Some(drop)=cross_drop {
            if animations {
                let target_body=bodies[drop.dock.index()].1;
                let target_outline=&outlines[drop.dock.index()];
                let from=motion.filter(|state|state.slot==drop.slot&&state.source==drop.source)
                    .map(|state|state.current).unwrap_or(drop.from);
                let entry=nearest_boundary(target_outline,from);
                let origin=motion.filter(|state|state.slot==drop.slot&&state.source==drop.source)
                    .map(|state|state.origin)
                    .or_else(||dock_positions[drop.source.index()].iter().find(|(slot,_)|*slot==drop.slot).map(|(_,center)|*center))
                    .unwrap_or(drop.from);
                let source_outline=&outlines[drop.source.index()];
                let to=snap_target.filter(|(target,_)|*target==drop.dock).map(|(_,center)|center)
                    .unwrap_or_else(||insertion_position(order,&dock_positions[drop.dock.index()],drop.index,drop.dock,target_body));
                motion=Some(TransferMotion{
                    slot:drop.slot,source:drop.source,phase:TransferPhase::Engulfing,target:Some(drop.dock),target_index:drop.index,
                    origin,source_edge:nearest_boundary(source_outline,origin),current:from,previous:from,
                    velocity:Vec2::ZERO,sampled_at:now,phase_started:now,
                    from,to,target_entry:entry,
                });
            }else{
                self.commit_resident_transfer(order,drop.slot,drop.source,drop.dock,drop.index);
                motion=None;
            }
        }else if dragged.is_none(){
            if let Some(mut state)=motion.filter(|state|matches!(state.phase,TransferPhase::Ejecting|TransferPhase::Free)){
                if animations {
                    state.phase=TransferPhase::Returning;state.phase_started=now;state.from=state.current;state.to=state.origin;state.target=None;
                    motion=Some(state);
                }else{motion=None;}
            }
        }

        if let Some(mut state)=motion {
            let duration=phase_duration(state.phase);
            let progress=smooth01(((now-state.phase_started)/duration).clamp(0.0,1.0) as f32);
            let painter=ui.painter().with_clip_rect(stage);
            let speed=state.velocity.length();
            let stretch=(speed/1800.0).clamp(0.0,0.16);
            let moving_angle=if speed>24.0{(-state.velocity).angle()}else{(state.source_edge-state.current).angle()};
            let mut clear=false;
            match state.phase {
                TransferPhase::Ejecting|TransferPhase::Free=>{
                    let direction=(state.current-state.source_edge).normalized();
                    let eject=progress;
                    let radius=if state.phase==TransferPhase::Ejecting{10.0+6.0*eject}else{16.0};
                    let exposed_progress=-0.82+1.02*eject;
                    let local_center=state.source_edge+direction*(radius*exposed_progress);
                    let inside_blend=if state.phase==TransferPhase::Free
                        &&entered_target.is_some_and(|(slot,source,_)|slot==state.slot&&source==state.source)
                    {internal_drag_blend}else{0.0};
                    let mut center=if state.phase==TransferPhase::Ejecting{local_center}else{state.current};
                    if state.phase==TransferPhase::Free&&inside_blend>0.0 {
                        if let Some(handoff)=handoff_center{center=handoff;}
                    }
                    let mut render_angle=moving_angle;
                    let mut render_stretch=stretch;
                    if state.phase==TransferPhase::Free&&handoff_center.is_none() {
                        if let Some(target)=state.target.filter(|target|*target!=state.source) {
                            let body=bodies[target.index()].1;
                            let outline=&outlines[target.index()];
                            let entry=nearest_boundary(outline,center);
                            let distance=entry.distance(center);
                            let pull=smooth01(((48.0-distance)/32.0).clamp(0.0,1.0));
                            if pull>0.0 {
                                let toward=if polygon_contains(outline,center){(body.center()-center).normalized()}else{(entry-center).normalized()};
                                center+=toward*(5.0*pull);
                                render_angle=toward.angle();
                                render_stretch=render_stretch.max(0.20*pull);
                            }
                        }
                    }
                    // Keep the membrane visible by actual component-to-island contact, not
                    // pointer hover. This includes the source island during exocytosis and
                    // the destination island during engulfment.
                    let receiving=outlines.iter()
                        .filter_map(|target_outline|{
                            let entry=nearest_boundary(target_outline,center);
                            let distance=entry.distance(center);
                            (polygon_contains(target_outline,center)||distance<=radius+0.75)
                                .then_some((target_outline.as_slice(),entry,distance))
                        })
                        .min_by(|a,b|a.2.total_cmp(&b.2))
                        .map(|(outline,_,_)|{
                            erase_covered_island_edge(&painter,outline,center,radius,transfer_skin.fill,1.0);
                            outline
                        });
                    let alpha=if state.phase==TransferPhase::Ejecting{0.68+0.32*eject}else{1.0};
                    let detached_alpha=alpha*(1.0-inside_blend);
                    let mut ejecting_edge=None;
                    let detached_contour=if state.phase==TransferPhase::Ejecting {
                        let source_outline=&outlines[state.source.index()];
                        draw_exocytosis_bud(&painter,source_outline,state.source_edge,local_center,direction,radius,transfer_skin,1.0);
                        ejecting_edge=Some(source_outline.as_slice());
                        vesicle_contour(center,radius,render_angle,0.0)
                    } else {
                        draw_free_trail(&painter,center,render_angle,radius,render_stretch,transfer_skin,(speed/900.0).clamp(0.0,1.0)*(1.0-inside_blend));
                        draw_vesicle(&painter,center,radius,render_angle,render_stretch,transfer_skin,detached_alpha);
                        vesicle_contour(center,radius,render_angle,render_stretch)
                    };
                    self.draw_transfer_icon(ui,center,state.slot,stage,&layout,count,1.0,if state.source==Dock::Top{top_scale}else{1.0});
                    let island=if state.phase==TransferPhase::Ejecting {ejecting_edge.unwrap_or(&[])}else{receiving.unwrap_or(&[])};
                    let edge_alpha=if state.phase==TransferPhase::Ejecting {alpha}else{1.0-inside_blend};
                    draw_exposed_contour_edge(&painter,&detached_contour,island,transfer_skin.edge,edge_alpha);
                    if state.phase==TransferPhase::Ejecting && now-state.phase_started>=phase_duration(TransferPhase::Ejecting) {
                        state.current=local_center;
                        state.phase=TransferPhase::Free;state.phase_started=now;
                    }
                    if dragged.is_some(){ui.ctx().request_repaint_after(Duration::from_millis(16));}
                }
                TransferPhase::Returning=>{
                    let center=quadratic_bezier(state.from,state.from.lerp(state.to,0.5),state.to,progress);
                    state.current=center;
                    let tether=state.source_edge.distance(center);
                    let fusion=smooth01((46.0-tether)/28.0);
                    let mut source_body=None;
                    if tether<=46.0 {
                        let outline=&outlines[state.source.index()];
                        draw_exocytosis_bud(&painter,outline,state.source_edge,center,(center-state.source_edge).normalized(),16.0,transfer_skin,fusion);
                        source_body=Some(outline.as_slice());
                    }
                    let radius=16.0*(1.0-progress);
                    draw_vesicle(&painter,center,radius,moving_angle,stretch,transfer_skin,(1.0-progress)*(1.0-fusion));
                    self.draw_transfer_icon(ui,center,state.slot,stage,&layout,count,1.0,if state.source==Dock::Top{top_scale}else{1.0});
                    let contour=vesicle_contour(center,radius,moving_angle,stretch);
                    draw_exposed_contour_edge(&painter,&contour,source_body.unwrap_or(&[]),transfer_skin.edge,1.0-progress);
                    clear=progress>=1.0;
                }
                TransferPhase::Engulfing=>{
                    let target=state.target.unwrap_or(Dock::Top);
                    let body=bodies[target.index()].1;
                    let outline=&outlines[target.index()];
                    let inward=(body.center()-state.target_entry).normalized();
                    let middle=state.target_entry+inward*18.0;
                    let center=if progress<0.48 {
                        quadratic_bezier(state.from,state.target_entry+inward*5.0,middle,smooth01(progress/0.48))
                    }else{
                        middle.lerp(state.to,smooth01((progress-0.48)/0.52))
                    };
                    state.current=center;
                    let swallowed=smooth01((progress-0.38)/0.62);
                    let merge=smooth01((progress-0.58)/0.42);
                    let bubble_radius=16.0-11.5*merge;
                    erase_covered_island_edge(&painter,outline,center,bubble_radius,transfer_skin.fill,1.0-merge);
                    let angle=(state.target_entry-center).angle();
                    let stretch=0.08+0.28*swallowed;
                    draw_vesicle(&painter,center,bubble_radius,angle,stretch,transfer_skin,1.0-merge);
                    self.draw_transfer_icon(ui,center,state.slot,stage,&layout,count,1.0,if state.source==Dock::Top{top_scale}else{1.0});
                    let contour=vesicle_contour(center,bubble_radius,angle,stretch);
                    draw_exposed_contour_edge(&painter,&contour,outline,transfer_skin.edge,1.0-merge);
                    if progress>=1.0 {
                        self.commit_resident_transfer(order,state.slot,state.source,target,state.target_index);
                        state.phase=TransferPhase::Settling;state.phase_started=now;state.from=state.to;state.current=state.to;
                    }
                }
                TransferPhase::Settling=>{
                    let target=state.target.unwrap_or(Dock::Top);
                    let end=dock_positions[target.index()].iter().find(|(slot,_)|*slot==state.slot).map(|(_,center)|*center).unwrap_or(state.to);
                    let center=state.from.lerp(end,progress);
                    let radius=9.0*(1.0-progress);
                    draw_vesicle(&painter,center,radius,moving_angle,0.0,transfer_skin,1.0-progress);
                    self.draw_transfer_icon(ui,center,state.slot,stage,&layout,count,1.0,if state.source==Dock::Top{top_scale}else{1.0});
                    let contour=vesicle_contour(center,radius,moving_angle,0.0);
                    draw_exposed_contour_edge(&painter,&contour,&outlines[target.index()],transfer_skin.edge,1.0-progress);
                    clear=progress>=1.0;
                }
            }
            if clear {ui.ctx().data_mut(|data|data.remove::<TransferMotion>(transfer_id));}
            else {ui.ctx().data_mut(|data|data.insert_temp(transfer_id,state));ui.ctx().request_repaint_after(Duration::from_millis(16));}
        }else{
            ui.ctx().data_mut(|data|data.remove::<TransferMotion>(transfer_id));
        }

        if let Some((slot,index))=movement{
            if let Some(from)=order.iter().position(|value|*value==slot){order.remove(from);order.insert(index.min(order.len()),slot);self.settings.resident_order.clone_from(order);self.settings.save();}
        }
        ui.painter().text(Pos2::new(stage.center().x,stage.bottom()-14.0),egui::Align2::CENTER_CENTER,"拖动组件可调序，拖到另一岛可直接更改停靠位置",theme::font(11.0,W::Regular),Color32::from_gray(160));
        ui.add_space(8.0);
        ui.label(RichText::new("每个图标只显示在归属岛上；可拖入另一处岛体切换停靠。不同岛上的排列顺序保持同步。").size(11.0).color(ui.visuals().weak_text_color()));
    }

    fn draw_transfer_icon(&mut self,ui:&egui::Ui,center:Pos2,slot:usize,stage:Rect,layout:&Layout,count:usize,alpha:f32,scale:f32){
        let mut base=Card::new(ui,center-Vec2::new(18.0,15.0)*scale,stage);
        base.s=scale;
        let icon=base.faded(alpha).ghost();
        self.draw_resident_preview_icon(&icon,slot,18.0,15.0,layout,count,true);
    }

    fn commit_resident_transfer(&mut self,order:&mut Vec<usize>,slot:usize,source:crate::model::Dock,target:crate::model::Dock,target_index:usize){
        if source==target{return;}
        if let Some(from)=order.iter().position(|value|*value==slot){
            order.remove(from);
            let insertion=if from<target_index{target_index.saturating_sub(1)}else{target_index};
            order.insert(insertion.min(order.len()),slot);
        }
        self.set_resident_dock(slot,target);
        self.settings.resident_order.clone_from(order);
        self.settings.save();
    }

    fn resident_transfer_skin(&self,fill:Color32)->TransferSkin{
        transfer_skin_from_fill(fill)
    }

    fn set_resident_dock(&mut self,slot:usize,dock:crate::model::Dock){
        match slot {
            BATTERY=>self.settings.status_docks[0]=dock,
            PET=>self.settings.dock=dock,
            _=>if let Some(page)=resident_page(slot){self.settings.set_page_dock(page,dock);},
        }
    }

    fn resident_preview_assigned(&self,slot:usize,dock:crate::model::Dock)->bool{
        if !self.settings.split_docks{return self.settings.dock==dock;}
        match slot {
            BATTERY=>self.settings.status_docks[0]==dock,
            PET=>self.settings.dock==dock,
            _=>resident_page(slot).is_some_and(|page|self.settings.page_dock(page)==dock),
        }
    }

    fn resident_preview_assigned_for_transfer(&self,slot:usize,dock:crate::model::Dock,inside:Option<(usize,crate::model::Dock)>)->bool{
        if let Some(target)=inside.filter(|(moving,_)|*moving==slot).map(|(_,target)|target){return dock==target;}
        self.resident_preview_assigned(slot,dock)
    }

    fn resident_preview_active(&self, slot: usize, count: usize) -> bool {
        match slot {
            BATTERY=>self.settings.show_battery && self.resident_power.0.is_some(),PET=>self.settings.pet,
            MUSIC=>self.media.is_music() || self.settings.page_resident(0),FOCUS=>self.pomo.running || self.settings.page_resident(1),
            QUOTA=>count>0 || self.settings.page_resident(6),STEAM=>self.steam.badge().is_some() || !self.steam.installed_games.is_empty() || self.settings.page_resident(8),
            CLOCK=>true,
            _=>resident_page(slot).is_some_and(|page|self.settings.page_resident(page)),
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
                    let game=self.steam.game().cloned().or_else(||self.steam.installed_games.first().cloned()).or_else(||self.steam.badge().map(|(_,game)|game.clone()));
                    let icon=game.as_ref().and_then(|game|self.steam_images.get(c.ui.ctx(),game.icon.as_deref(),None));
                    if let Some(texture)=icon{c.painter.image(texture,c.rect(x-9.0,6.0,18.0,18.0),Rect::from_min_max(Pos2::ZERO,Pos2::new(1.0,1.0)),Color32::WHITE);}
                    else{crate::extras::device_icon(&c,crate::bt::DeviceKind::Gamepad,x-9.0,6.0,18.0);}
                },
                WATER=>paint::resident_water(&c,x,15.0),
                SHELF=>paint::resident_shelf(&c,x,15.0),
                CLIPBOARD=>paint::resident_clipboard(&c,x,15.0),
                TODO=>paint::resident_todo(&c,x,15.0),
                MAIL=>paint::resident_mail(&c,x,15.0),
                _=>{},
            }
    }

}

fn resident_preview_slot_width(slot:usize,layout:&Layout,clock_period:u8)->f32{
    match slot{
        CLOCK if clock_period!=0=>84.0,
        CLOCK=>44.0,
        BATTERY=>20.0,
        PET=>32.0,
        MUSIC=>14.0,
        FOCUS=>layout.focus_width,
        QUOTA=>{
            let left=layout.quota_slots().filter_map(|index|layout.x[index].map(|x|x-layout.widths[index]/2.0)).fold(f32::INFINITY,f32::min);
            let right=layout.quota_slots().filter_map(|index|layout.x[index].map(|x|x+layout.widths[index]/2.0)).fold(f32::NEG_INFINITY,f32::max);
            if left.is_finite()&&right.is_finite(){right-left}else{18.0}
        },
        _=>layout.widths.get(slot).copied().unwrap_or(18.0),
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
        CLIPBOARD => "剪贴板",
        TODO => "待办",
        MAIL => "邮箱",
        _ => "图标",
    }
}

fn resident_page(slot:usize)->Option<usize>{
    match slot {
        MUSIC=>Some(0),FOCUS=>Some(1),WATER=>Some(2),SHELF=>Some(3),TODO=>Some(4),MAIL=>Some(5),CLIPBOARD=>Some(9),
        QUOTA=>Some(6),CLOCK=>Some(crate::app::CALENDAR_PAGE),STEAM=>Some(8),_=>None,
    }
}

fn smooth01(value:f32)->f32{
    let t=value.clamp(0.0,1.0);
    t*t*(3.0-2.0*t)
}

fn phase_duration(phase:TransferPhase)->f64{
    match phase {
        TransferPhase::Ejecting=>0.22,
        TransferPhase::Free=>f64::MAX,
        TransferPhase::Engulfing=>0.39,
        TransferPhase::Returning=>0.28,
        TransferPhase::Settling=>0.13,
    }
}

fn transfer_icon_alpha(motion:Option<TransferMotion>,_progress:Option<f32>,slot:usize,dock:crate::model::Dock)->f32{
    let Some(state)=motion.filter(|state|state.slot==slot) else{return 1.0;};
    // Keep both island slots hidden while the transfer renderer owns the icon.
    // It disappears at the exact settled position, so the native copy can resume
    // on the next frame without a separated crossfade.
    if dock==state.source{return 0.0;}
    if Some(dock)==state.target&&matches!(state.phase,TransferPhase::Engulfing|TransferPhase::Settling){return 0.0;}
    1.0
}

fn insertion_position(order:&[usize],positions:&[(usize,Pos2)],index:usize,_dock:crate::model::Dock,body:Rect)->Pos2{
    for (slot,center) in positions {
        if order.iter().position(|value|value==slot).is_some_and(|item_index|item_index>=index){return *center;}
    }
    positions.last().map_or(body.center(),|(_,center)|*center)
}

fn quadratic_bezier(start:Pos2,control:Pos2,end:Pos2,t:f32)->Pos2{
    start.lerp(control,t).lerp(control.lerp(end,t),t)
}

fn insertion_index(order:&[usize],positions:&[(usize,Pos2)],dock:crate::model::Dock,pointer:Pos2)->usize{
    let coordinate=if dock==crate::model::Dock::Top{pointer.x}else{pointer.y};
    for (slot,center) in positions {
        let item_coordinate=if dock==crate::model::Dock::Top{center.x}else{center.y};
        if coordinate<item_coordinate {
            return order.iter().position(|value|value==slot).unwrap_or(order.len());
        }
    }
    positions.last().and_then(|(slot,_)|order.iter().position(|value|value==slot)).map_or(order.len(),|index|index+1)
}

fn island_outline(rect:Rect,dock:crate::model::Dock,pill:bool,scale:f32)->Vec<Pos2>{
    if pill {
        let radius=(if dock==crate::model::Dock::Top{15.0*scale}else{18.0})
            .min(rect.width().min(rect.height())*0.5);
        let corners=[
            (Pos2::new(rect.right()-radius,rect.top()+radius),-std::f32::consts::FRAC_PI_2,0.0),
            (Pos2::new(rect.right()-radius,rect.bottom()-radius),0.0,std::f32::consts::FRAC_PI_2),
            (Pos2::new(rect.left()+radius,rect.bottom()-radius),std::f32::consts::FRAC_PI_2,std::f32::consts::PI),
            (Pos2::new(rect.left()+radius,rect.top()+radius),std::f32::consts::PI,3.0*std::f32::consts::FRAC_PI_2),
        ];
        let mut points=Vec::with_capacity(52);
        for (center,start,end) in corners {
            for step in 0..=12 {
                let angle=start+(end-start)*step as f32/12.0;
                points.push(center+Vec2::new(angle.cos(),angle.sin())*radius);
            }
        }
        points
    }else if dock==crate::model::Dock::Top {
        crate::shape::outline(rect,8.0*scale,10.0*scale,12.0*scale)
    }else{
        crate::shape::outline_docked(rect,dock,5.0,8.0,12.0)
    }
}

fn polygon_contains(outline:&[Pos2],point:Pos2)->bool{
    let mut inside=false;
    let mut previous=*outline.last().unwrap_or(&point);
    for &current in outline {
        if (current.y>point.y)!=(previous.y>point.y)
            && point.x<(previous.x-current.x)*(point.y-current.y)/(previous.y-current.y)+current.x
        {inside=!inside;}
        previous=current;
    }
    inside
}

fn nearest_boundary(outline:&[Pos2],point:Pos2)->Pos2{
    let mut nearest=point;
    let mut distance=f32::INFINITY;
    for index in 0..outline.len(){
        let start=outline[index];
        let end=outline[(index+1)%outline.len()];
        let edge=end-start;
        let t=((point-start).dot(edge)/edge.length_sq().max(0.001)).clamp(0.0,1.0);
        let candidate=start+edge*t;
        let candidate_distance=candidate.distance_sq(point);
        if candidate_distance<distance {distance=candidate_distance;nearest=candidate;}
    }
    nearest
}

fn boundary_toward(outline:&[Pos2],start:Pos2,target:Pos2)->Pos2{
    let ray=target-start;
    if ray.length_sq()<0.001{return nearest_boundary(outline,start);}
    let mut nearest_t=f32::INFINITY;
    let mut intersection=None;
    for index in 0..outline.len(){
        let edge_start=outline[index];
        let edge=outline[(index+1)%outline.len()]-edge_start;
        let denominator=ray.x*edge.y-ray.y*edge.x;
        if denominator.abs()<0.001{continue;}
        let offset=edge_start-start;
        let t=(offset.x*edge.y-offset.y*edge.x)/denominator;
        let u=(offset.x*ray.y-offset.y*ray.x)/denominator;
        if t>=0.0&&(-0.001..=1.001).contains(&u)&&t<nearest_t {
            nearest_t=t;intersection=Some(start+ray*t);
        }
    }
    intersection.unwrap_or_else(||nearest_boundary(outline,start))
}

fn draw_exocytosis_bud(painter:&egui::Painter,outline:&[Pos2],anchor:Pos2,center:Pos2,direction:Vec2,radius:f32,skin:TransferSkin,alpha:f32){
    let alpha=alpha.clamp(0.0,1.0);
    if alpha<=0.01{return;}
    let direction=if direction.length_sq()<0.001{Vec2::X}else{direction.normalized()};
    let normal=Vec2::new(-direction.y,direction.x);
    let radius=radius.clamp(8.0,18.0);
    let neck=(radius*(0.34+alpha*0.08)).clamp(3.0,6.0);
    let fill=with_opacity(skin.fill,alpha);
    let tip=center-direction*(radius*0.55);
    erase_covered_island_edge(painter,outline,center,radius,skin.fill,alpha);
    draw_membrane(painter,anchor,tip,skin,alpha,1.25);
    painter.circle_filled(anchor,neck,fill);
    painter.circle_filled(tip,radius*0.68,fill);
    painter.circle_filled(center,radius,fill);
    painter.circle_filled(center-direction*(radius*0.25)-normal*(radius*0.3),radius*0.13,with_opacity(skin.sheen,alpha*0.18));
}

fn erase_covered_island_edge(painter:&egui::Painter,outline:&[Pos2],center:Pos2,radius:f32,fill:Color32,alpha:f32){
    let cover=with_opacity(fill,smooth01(alpha*2.0));
    for index in 0..outline.len(){
        let start=outline[index];
        let edge=outline[(index+1)%outline.len()]-start;
        let length_sq=edge.length_sq();
        if length_sq<=0.001{continue;}
        let projection=(center-start).dot(edge)/length_sq;
        let closest=start+edge*projection;
        let perpendicular_sq=center.distance_sq(closest);
        if perpendicular_sq>=radius*radius{continue;}
        let half=(radius*radius-perpendicular_sq).sqrt()/length_sq.sqrt();
        let from=(projection-half).max(0.0);
        let to=(projection+half).min(1.0);
        if to>from {painter.line_segment([start+edge*from,start+edge*to],egui::Stroke::new(2.5_f32,cover));}
    }
}

fn draw_exposed_contour_edge(painter:&egui::Painter,contour:&[Pos2],island:&[Pos2],color:Color32,alpha:f32){
    if contour.len()<2||color==Color32::TRANSPARENT||alpha<=0.01{return;}
    let stroke=egui::Stroke::new(1.25_f32,with_opacity(color,alpha));
    for path in exposed_contour_paths(contour,island){painter.add(egui::Shape::line(path,stroke));}
}

fn exposed_contour_paths(contour:&[Pos2],island:&[Pos2])->Vec<Vec<Pos2>>{
    if contour.len()<2{return Vec::new();}
    if island.is_empty(){let mut closed=contour.to_vec();closed.push(contour[0]);return vec![closed];}
    let intervals=exposed_contour_intervals(contour,island);
    if intervals.iter().all(|(_,_,outside)|*outside){
        let mut closed=contour.to_vec();closed.push(contour[0]);
        return vec![closed];
    }
    let Some(first_inside)=intervals.iter().position(|(_,_,outside)|!*outside) else{return Vec::new();};
    let mut paths=Vec::new();
    let mut run=Vec::new();
    for step in 1..=intervals.len(){
        let index=(first_inside+step)%intervals.len();
        let (from,to,outside)=intervals[index];
        if outside {
            if run.last().is_none_or(|last: &Pos2|last.distance_sq(from)>0.01){
                if run.len()>1{paths.push(std::mem::take(&mut run));}
                run=vec![from];
            }
            run.push(to);
        }else if run.len()>1{
            paths.push(std::mem::take(&mut run));
        }else{
            run.clear();
        }
    }
    if run.len()>1{paths.push(run);}
    paths
}

fn exposed_contour_intervals(contour:&[Pos2],island:&[Pos2])->Vec<(Pos2,Pos2,bool)>{
    let mut intervals=Vec::new();
    for index in 0..contour.len(){
        let start=contour[index];
        let end=contour[(index+1)%contour.len()];
        let direction=end-start;
        let mut cuts=vec![0.0,1.0];
        for edge_index in 0..island.len(){
            let edge_start=island[edge_index];
            let edge=island[(edge_index+1)%island.len()]-edge_start;
            let denominator=direction.x*edge.y-direction.y*edge.x;
            if denominator.abs()<0.0001{continue;}
            let offset=edge_start-start;
            let t=(offset.x*edge.y-offset.y*edge.x)/denominator;
            let u=(offset.x*direction.y-offset.y*direction.x)/denominator;
            if (0.0001..0.9999).contains(&t)&&(-0.0001..=1.0001).contains(&u){cuts.push(t);}
        }
        cuts.sort_by(f32::total_cmp);
        cuts.dedup_by(|left,right|(*left-*right).abs()<0.0001);
        for pair in cuts.windows(2){
            let from=start+direction*pair[0];
            let to=start+direction*pair[1];
            let outside=!polygon_contains(island,from.lerp(to,0.5));
            intervals.push((from,to,outside));
        }
    }
    intervals
}

fn draw_membrane(painter:&egui::Painter,anchor:Pos2,center:Pos2,skin:TransferSkin,alpha:f32,width_scale:f32){
    let delta=center-anchor;
    let length=delta.length();
    if length<2.0||length>72.0||alpha<=0.01{return;}
    let direction=delta/length;
    let normal=Vec2::new(-direction.y,direction.x);
    let control=anchor.lerp(center,0.5)+normal*(length*0.07).clamp(-4.0,4.0);
    let points=(0..=12).map(|step|quadratic_bezier(anchor,control,center,step as f32/12.0)).collect::<Vec<_>>();
    let width=5.2*width_scale.clamp(0.42,1.2);
    painter.add(egui::Shape::line(points.clone(),egui::Stroke::new(width,with_opacity(skin.fill,alpha*0.92))));
    painter.circle_filled(anchor,width*0.5,with_opacity(skin.fill,alpha*0.92));
    painter.circle_filled(center,width*0.5,with_opacity(skin.fill,alpha*0.92));
}

fn draw_free_trail(painter:&egui::Painter,center:Pos2,trail_angle:f32,radius:f32,stretch:f32,skin:TransferSkin,speed:f32){
    if speed<=0.05{return;}
    let direction=Vec2::new(trail_angle.cos(),trail_angle.sin());
    let trail_length=radius*(0.55+stretch*5.0);
    let base=center+direction*(radius*0.48);
    let tip=base+direction*trail_length;
    draw_membrane(painter,base,tip,skin,speed*0.22,0.72);
    painter.circle_filled(base+direction*(trail_length*0.38),radius*0.31,with_opacity(skin.sheen,speed*0.16));
    painter.circle_filled(tip,radius*0.18,with_opacity(skin.fill,speed*0.12));
}

fn draw_vesicle(painter:&egui::Painter,center:Pos2,radius:f32,angle:f32,stretch:f32,skin:TransferSkin,alpha:f32){
    if radius<=0.2||alpha<=0.01{return;}
    let points=vesicle_contour(center,radius,angle,stretch);
    painter.add(egui::Shape::convex_polygon(points,with_opacity(skin.fill,alpha*0.86),egui::Stroke::NONE));
    let direction=Vec2::new(angle.cos(),angle.sin());
    let perpendicular=Vec2::new(-direction.y,direction.x);
    painter.circle_filled(center-direction*(radius*0.28)-perpendicular*(radius*0.28),radius*0.15,with_opacity(skin.sheen,alpha*0.22));
}

fn vesicle_contour(center:Pos2,radius:f32,angle:f32,stretch:f32)->Vec<Pos2>{
    let direction=Vec2::new(angle.cos(),angle.sin());
    let perpendicular=Vec2::new(-direction.y,direction.x);
    let stretch=stretch.clamp(0.0,0.2);
    let longitudinal=radius*(1.0+stretch);
    let transverse=radius*(0.92-stretch*0.35);
    let mut points=Vec::with_capacity(32);
    for step in 0..32 {
        let theta=std::f32::consts::TAU*step as f32/32.0;
        let bulge=1.0+0.055*theta.cos();
        points.push(center-direction*(radius*0.06)+direction*(theta.cos()*longitudinal*bulge)
            +perpendicular*(theta.sin()*transverse));
    }
    points
}

fn with_opacity(color:Color32,opacity:f32)->Color32{
    let [r,g,b,a]=color.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r,g,b,(a as f32*opacity.clamp(0.0,1.0)).round() as u8)
}

/// Build transfer material from the island's own surface color. These shapes
/// share the island preview's palette-recolor pass, so added skins inherit the
/// same mapping without adding a `Skin` match here.
fn transfer_skin_from_fill(fill:Color32)->TransferSkin{
    let [r,g,b,_]=fill.to_srgba_unmultiplied();
    let luminance=0.2126*r as f32+0.7152*g as f32+0.0722*b as f32;
    let contrast=if luminance>142.0{Color32::BLACK}else{Color32::WHITE};
    let rim=tint_material(fill,contrast,0.36,168);
    let edge=tint_material(fill,contrast,0.16,190);
    let sheen=tint_material(fill,Color32::WHITE,0.34,66);
    TransferSkin{fill,rim,edge,sheen}
}

fn tint_material(base:Color32,toward:Color32,amount:f32,alpha:u8)->Color32{
    let [r,g,b,_]=base.to_srgba_unmultiplied();
    let [tr,tg,tb,_]=toward.to_srgba_unmultiplied();
    let mix=|from:u8,to:u8|((from as f32+(to as f32-from as f32)*amount).round()).clamp(0.0,255.0) as u8;
    Color32::from_rgba_unmultiplied(mix(r,tr),mix(g,tg),mix(b,tb),alpha)
}

#[cfg(test)]
mod transfer_edge_tests{
    use super::*;

    #[test]
    fn exposed_outline_is_clipped_at_exact_side_dock_contacts(){
        let island=[Pos2::new(0.0,0.0),Pos2::new(36.0,0.0),Pos2::new(36.0,120.0),Pos2::new(0.0,120.0)];
        let contour=(0..64).map(|step|{
            let angle=std::f32::consts::TAU*step as f32/64.0;
            Pos2::new(18.0+14.0*angle.cos(),116.0+14.0*angle.sin())
        }).collect::<Vec<_>>();
        let paths=exposed_contour_paths(&contour,&island);
        assert!(!paths.is_empty());
        for path in paths{
            assert!(path.len()>=2);
            for pair in path.windows(2){
                assert!(!polygon_contains(&island,pair[0].lerp(pair[1],0.5)),"包边跨入岛体内部：{pair:?}");
            }
            for endpoint in [path[0],*path.last().unwrap()]{
                let distance=(0..island.len()).map(|index|{
                    let start=island[index];let edge=island[(index+1)%island.len()]-start;
                    let t=((endpoint-start).dot(edge)/edge.length_sq()).clamp(0.0,1.0);
                    endpoint.distance(start+edge*t)
                }).fold(f32::INFINITY,f32::min);
                assert!(distance<0.01,"包边端点未贴合灵动岛边缘：{endpoint:?}, gap={distance}");
            }
        }
    }

    #[test]
    fn detached_component_outline_closes_after_leaving_the_island(){
        let island=[Pos2::new(0.0,0.0),Pos2::new(36.0,0.0),Pos2::new(36.0,120.0),Pos2::new(0.0,120.0)];
        let contour=(0..32).map(|step|{
            let angle=std::f32::consts::TAU*step as f32/32.0;
            Pos2::new(18.0+12.0*angle.cos(),140.0+12.0*angle.sin())
        }).collect::<Vec<_>>();
        let paths=exposed_contour_paths(&contour,&island);
        assert_eq!(paths.len(),1);
        assert_eq!(paths[0].first(),paths[0].last());
    }

    #[test]
    fn transfer_material_tracks_each_island_surface_sample(){
        let fills=[
            Color32::BLACK,
            Color32::from_rgb(54,62,74),
            Color32::from_rgb(255,243,228),
            Color32::from_rgba_unmultiplied(35,38,44,215),
            Color32::from_rgba_unmultiplied(214,222,235,225),
        ];
        let materials=fills.map(transfer_skin_from_fill);
        for (fill,material) in fills.into_iter().zip(materials){
            assert_eq!(material.fill,fill,"迁移表面必须沿用当前岛体底色");
            assert_ne!(material.edge,Color32::from_white_alpha(60),"包边不能再固定使用某一套皮肤的白色边线");
        }
        assert_ne!(materials[0].edge,materials[1].edge,"悬浮云色变化时包边也应随表面取色");
        assert_ne!(materials[0].sheen,materials[2].sheen,"不同明暗表面应生成不同的高光");
    }
}
