//! 常驻组件按实际占位排布，并在开关变化时连续补位。
use crate::anim::{smoothstep, Spring};

pub(crate) const CLOCK: usize = 0;
pub(crate) const BATTERY: usize = 1;
pub(crate) const QUOTA: usize = 2;
pub(crate) fn quota_slot(position:usize)->usize { if position<3 {QUOTA+position} else {SLOT_COUNT+position-3} }
pub(crate) const MUSIC: usize = 6;
pub(crate) const FOCUS: usize = 7;
pub(crate) const BELL: usize = 8;
pub(crate) const PET: usize = 9;
pub(crate) const STEAM: usize = 10;
pub(crate) const WATER: usize = 11;
pub(crate) const SHELF: usize = 12;
pub(crate) const TODO: usize = 13;
pub(crate) const MAIL: usize = 14;
pub(crate) const CALENDAR: usize = 15;
pub(crate) const SLOT_COUNT: usize = 16;
const WIDTHS: [f32; SLOT_COUNT] = [44.0,20.0,27.0,27.0,27.0,22.0,14.0,crate::focus_bar::WIDTH,20.0,32.0,18.0,18.0,18.0,18.0,18.0,18.0];
const GAP: f32 = 12.0;

pub(crate) const LEGACY_ORDER: [usize;6]=[BATTERY,PET,CLOCK,QUOTA,MUSIC,FOCUS];
pub(crate) const DEFAULT_ORDER: [usize;11]=[BATTERY,PET,CLOCK,QUOTA,MUSIC,FOCUS,WATER,SHELF,TODO,MAIL,STEAM];
pub(crate) fn valid_order(order:&[usize])->bool {
    order.len()==DEFAULT_ORDER.len() && DEFAULT_ORDER.iter().all(|slot|order.iter().filter(|value|*value==slot).count()==1)
}
pub(crate) fn normalize_order(order:&[usize])->Option<Vec<usize>> {
    if valid_order(order){return Some(order.to_vec());}
    if order.len()==DEFAULT_ORDER.len()+1 {
        let without_calendar:Vec<_>=order.iter().copied().filter(|slot|*slot!=CALENDAR).collect();
        if valid_order(&without_calendar){return Some(without_calendar);}
    }
    (order.len()==LEGACY_ORDER.len() && LEGACY_ORDER.iter().all(|slot|order.iter().filter(|value|*value==slot).count()==1))
        .then(||order.iter().copied().chain(DEFAULT_ORDER[LEGACY_ORDER.len()..].iter().copied()).collect())
}

pub(crate) fn animations_enabled()->bool {
    #[cfg(windows)] {
        static ENABLED:std::sync::OnceLock<bool>=std::sync::OnceLock::new();
        *ENABLED.get_or_init(||{
            use windows::Win32::{Foundation::BOOL,UI::WindowsAndMessaging::*};
            let mut enabled=BOOL(1);
            unsafe {let _=SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION,0,Some((&mut enabled as *mut BOOL).cast()),SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));}
            enabled.as_bool()
        })
    }
    #[cfg(not(windows))] {true}
}

#[derive(Clone, Debug)]
pub(crate) struct Layout {
    pub width: f32,
    pub x: Vec<Option<f32>>,
    pub widths: Vec<f32>,
    quotas: usize,
    pub focus_width: f32,
}

impl Layout {
    pub fn new(battery:bool,quotas:usize,music:bool,focus:bool,bell:bool,pet:bool)->Self {
        Self::with_focus_width(battery,quotas,music,focus,bell,pet,crate::focus_bar::WIDTH)
    }
    pub fn with_focus_width(battery:bool,quotas:usize,music:bool,focus:bool,bell:bool,pet:bool,focus_width:f32)->Self {
        fn place(cursor:&mut f32,width:f32)->f32 {let x=*cursor+width/2.0;*cursor+=width+GAP;x}
        let mut widths=WIDTHS.to_vec();
        widths[FOCUS]=focus_width;
        widths.resize(SLOT_COUNT+quotas.saturating_sub(3),27.0);
        let mut x=vec![None;widths.len()];let mut cursor=16.0;
        let count=quotas;
        let order=if battery {[BATTERY,CLOCK,QUOTA]}else{[QUOTA,CLOCK,BATTERY]};
        for slot in order {
            match slot {
                BATTERY if battery=>x[slot]=Some(place(&mut cursor,WIDTHS[slot])),
                CLOCK=>{
                    if pet {x[PET]=Some(place(&mut cursor,WIDTHS[PET]));}
                    x[slot]=Some(place(&mut cursor,WIDTHS[slot]));
                },
                QUOTA if count>0=>{
                    let width=count as f32*27.0+(count-1) as f32*7.0;
                    let left=place(&mut cursor,width)-width/2.0;
                    for i in 0..count {x[quota_slot(i)]=Some(left+13.5+i as f32*34.0);}
                },
                _=>{},
            }
        }
        for (slot,on) in [(MUSIC,music),(FOCUS,focus),(BELL,bell)] {
            if on {x[slot]=Some(place(&mut cursor,if slot==FOCUS {focus_width}else{WIDTHS[slot]}));}
        }
        let width=cursor-GAP+16.0;
        for value in x.iter_mut().flatten() {*value-=width/2.0;}
        Self {width:width.max(96.0),x,widths,quotas,focus_width}
    }
    pub fn add_pages(&mut self,pinned:[bool;9],steam:bool) {
        let old_width=self.width;
        let mut cursor=old_width/2.0-16.0+GAP;
        for (slot,on) in [(WATER,pinned[2]),(SHELF,pinned[3]),(TODO,pinned[4]),(MAIL,pinned[5]),(STEAM,steam||pinned[8])] {
            if on {self.x[slot]=Some(cursor+WIDTHS[slot]/2.0);cursor+=WIDTHS[slot]+GAP;}
        }
        let growth=cursor-(old_width/2.0-16.0+GAP);
        if growth>0.0 {for x in self.x.iter_mut().flatten(){*x-=growth/2.0;} self.width+=growth;}
    }
    pub fn fit(&mut self,max_width:f32) {
        if self.width<=max_width {return;}
        let mut slots:Vec<_>=self.x.iter().enumerate().filter_map(|(slot,x)|x.map(|x|(x,slot))).collect();
        slots.sort_by(|a,b|a.0.total_cmp(&b.0));
        let gap=7.0;
        let gaps=gap*(slots.len().saturating_sub(1) as f32);
        let widths: f32=slots.iter().map(|(_,slot)|self.widths[*slot]).sum();
        let scale=((max_width-32.0-gaps).max(1.0)/widths.max(1.0)).min(1.0);
        for (_,slot) in &slots {self.widths[*slot]*=scale;}
        let content: f32=slots.iter().map(|(_,slot)|self.widths[*slot]).sum::<f32>()+gaps;
        let mut cursor=-content*0.5;
        for (_,slot) in slots {
            let width=self.widths[slot];
            self.x[slot]=Some(cursor+width*0.5);
            cursor+=width+gap;
        }
        self.width=max_width;
    }
    pub fn quota_slots(&self)->impl Iterator<Item=usize> { (0..self.quotas).map(quota_slot) }
    pub fn balance_tail(&mut self,position:usize,extra:f32){
        let slot=quota_slot(position);
        let Some(center)=self.x[slot] else{return;};
        for (i,x) in self.x.iter_mut().enumerate(){if i!=slot{if let Some(x)=x{*x+=if *x<center{-extra/2.0}else{extra/2.0};}}}
        self.widths[slot]+=extra;self.width+=extra;
    }
    pub fn widen_clock(&mut self, extra:f32) {
        let clock=self.x[CLOCK].unwrap();
        for (slot,x) in self.x.iter_mut().enumerate() {
            if slot!=CLOCK { if let Some(x)=x { *x+=if *x<clock {-extra/2.0}else{extra/2.0}; } }
        }
        self.width+=extra;
    }
    pub fn remove_clock(&mut self) {
        if let Some(clock) = self.x[CLOCK].take() {
            let space = self.widths[CLOCK] + GAP;
            for x in self.x.iter_mut().flatten() { *x += if *x < clock { space / 2.0 } else { -space / 2.0 }; }
            self.width = (self.width - space).max(64.0);
        }
    }
    pub fn reorder(&mut self,order:&[usize]) {
        if !valid_order(order){return;}
        let original=self.x.clone();let mut cursor=-self.width/2.0+16.0;
        for slot in order.iter().copied().chain(std::iter::once(BELL)) {
            let slots:Vec<usize>=if slot==QUOTA{self.quota_slots().collect()}else{vec![slot]};
            let left=slots.iter().filter_map(|&i|original[i].map(|x|x-self.widths[i]/2.0)).fold(f32::INFINITY,f32::min);
            let right=slots.iter().filter_map(|&i|original[i].map(|x|x+self.widths[i]/2.0)).fold(f32::NEG_INFINITY,f32::max);
            if !left.is_finite(){continue;}
            for i in slots {if let Some(x)=original[i]{self.x[i]=Some(cursor+x-left);}}
            cursor+=right-left+GAP;
        }
    }
    /// 圆环换边经过时钟时，暂时淡出时钟，避免移动中相互遮挡。
    pub fn clearance(&self,slot:usize)->f32 {
        let Some(x)=self.x[slot] else{return 0.0;};
        let distance=self.quota_slots().filter_map(|i|self.x[i].map(|other|(other-x).abs()-(self.widths[i]-27.0)/2.0)).fold(f32::INFINITY,f32::min);
        let quota=if slot==CLOCK {smoothstep(34.0,44.0,distance)}else{smoothstep(22.0,34.0,distance)};
        quota*self.pet_clearance(slot)
    }
    pub fn pet_clearance(&self,slot:usize)->f32 {
        match (self.x[slot],self.x[PET]) {
            (Some(x),Some(pet))=>smoothstep(self.widths[slot]/2.0+14.0,self.widths[slot]/2.0+24.0,(x-pet).abs()),
            _=>1.0,
        }
    }
}

pub(crate) struct Motion {
    pub target: Layout,
    positions: Vec<Spring>,
    initialized: bool,
}
impl Default for Motion {
    fn default()->Self {Self {target:Layout::new(false,0,false,false,false,false),positions:vec![Spring::new(0.0,360.0,0.76);SLOT_COUNT],initialized:false}}
}
impl Motion {
    pub fn step(&mut self,target:Layout,dt:f32,animate:bool) {
        self.positions.resize(target.x.len(),Spring::new(0.0,360.0,0.76));
        for (i,x) in target.x.iter().enumerate() {
            if let Some(x)=x {
                let spring=&mut self.positions[i];spring.target=*x;
                if !self.initialized || self.target.x.get(i).copied().flatten().is_none() || !animate {spring.snap(*x);}else{spring.step(dt);}
            }
        }
        self.target=target;self.initialized=true;
    }
    pub fn current(&self)->Layout {
        let mut layout=self.target.clone();
        for (i,x) in layout.x.iter_mut().enumerate() {if x.is_some(){*x=Some(self.positions[i].pos);}}
        layout
    }
    pub fn position(&self,slot:usize)->f32 {self.positions[slot].pos}
    pub fn settled(&self)->bool {self.target.x.iter().enumerate().all(|(i,x)|x.is_none()||self.positions[i].settled(100.0))}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn deepseek_balance_follows_more_and_never_overlaps(){
        for order in [&DEFAULT_ORDER[..],&[FOCUS,MUSIC,QUOTA,CLOCK,PET,BATTERY][..]] {for quotas in 1..=6 {for battery in [false,true]{
            let mut layout=Layout::new(battery,quotas,true,true,false,true);layout.reorder(&order);layout.balance_tail(quotas-1,76.0);layout.widen_clock(40.0);
            let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|{let width=layout.widths[i]+if i==CLOCK{40.0}else{0.0};(x-width/2.0,x+width/2.0)})).collect();boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
            assert!(boxes.windows(2).all(|p|p[1].0-p[0].1>=6.9));assert!(boxes.last().unwrap().1<=layout.width/2.0-15.9);
            assert!(layout.width<crate::app::WIN_W);
        }}}
    }
    #[test]
    fn idle_focus_uses_shortcut_width_and_old_calendar_order_migrates(){
        let running=Layout::new(false,0,false,true,false,false);
        let mut idle=Layout::with_focus_width(false,0,false,true,false,false,20.0);
        assert!((running.width-idle.width-48.0).abs()<0.01);
        idle.reorder(&DEFAULT_ORDER);
        let clock=idle.x[CLOCK].unwrap();
        let focus=idle.x[FOCUS].unwrap();
        assert!(focus-clock>=44.0/2.0+20.0/2.0+GAP-0.01);
        let old=[BATTERY,PET,CLOCK,QUOTA,MUSIC,FOCUS,WATER,SHELF,TODO,MAIL,CALENDAR,STEAM];
        assert_eq!(normalize_order(&old),Some(DEFAULT_ORDER.to_vec()));
    }
    #[test]
    fn clock_12h_keeps_components_separate_in_custom_orders(){
        for order in [&DEFAULT_ORDER[..],&[FOCUS,MUSIC,QUOTA,CLOCK,PET,BATTERY,WATER,SHELF,TODO,MAIL,STEAM][..]] {
            for quotas in 0..=5 {
                let mut layout=Layout::new(true,quotas,true,true,false,true);
                layout.reorder(&order);layout.widen_clock(40.0);
                let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|{
                    let width=layout.widths[i]+if i==CLOCK {40.0}else{0.0};
                    (x-width/2.0,x+width/2.0)
                })).collect();
                boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
                assert!(boxes.windows(2).all(|p|p[1].0-p[0].1>=6.9));
                assert!(boxes.first().unwrap().0>=-layout.width/2.0+15.9);
                assert!(boxes.last().unwrap().1<=layout.width/2.0-15.9);
            }
        }
    }
    #[test]
    fn custom_order_preserves_groups_and_hidden_positions(){
        assert!(!valid_order(&[0,0,2,6,7,9]));
        assert!(!valid_order(&[0,1,2]));
        let order=[FOCUS,MUSIC,QUOTA,CLOCK,PET,BATTERY,WATER,SHELF,TODO,MAIL,STEAM];
        for battery in [false,true]{for count in 0..=5 {
            let mut layout=Layout::new(battery,count,true,true,false,true);
            let width=layout.width;layout.reorder(&order);
            assert_eq!(layout.width,width);
            let positions:Vec<_>=order.iter().filter_map(|i|layout.x[*i]).collect();
            assert!(positions.windows(2).all(|p|p[0]<p[1]));
            if count>1 {assert_eq!(layout.x[QUOTA+1].unwrap()-layout.x[QUOTA].unwrap(),34.0);}
            let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|(x-layout.widths[i]/2.0,x+layout.widths[i]/2.0))).collect();
            boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
            assert!(boxes.windows(2).all(|p|p[1].0-p[0].1>=6.9));
        }}
    }
    #[test]
    fn every_component_combination_fits_without_overlaps(){
        for quotas in 0..=8 {for battery in [false,true] {for music in [false,true] {for focus in [false,true] {for bell in [false,true] {for pet in [false,true] {
            let layout=Layout::new(battery,quotas,music,focus,bell,pet);
            let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|(x-layout.widths[i]/2.0,x+layout.widths[i]/2.0))).collect();
            boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
            assert_eq!(layout.quota_slots().count(),quotas);
            assert!(boxes.first().unwrap().0>=-layout.width/2.0+15.9);
            assert!(boxes.last().unwrap().1<=layout.width/2.0-15.9);
            assert!(boxes.windows(2).all(|pair|pair[1].0-pair[0].1>=6.9),"组件不能挤在一起：{layout:?}");
            assert_eq!(layout.clearance(CLOCK),1.0);
        }}}}}}
    }
    #[test]
    fn replacing_battery_moves_continuously_and_supports_an_instant_state(){
        let before=Layout::new(true,1,true,false,false,true);let after=Layout::new(false,1,true,false,false,true);
        let mut motion=Motion::default();motion.step(before.clone(),0.0,true);
        motion.step(after.clone(),1.0/60.0,true);let mid=motion.current().x[QUOTA].unwrap();
        assert!(mid<before.x[QUOTA].unwrap()&&mid>after.x[QUOTA].unwrap());
        for _ in 0..120 {motion.step(after.clone(),1.0/60.0,true);}
        assert!((motion.current().x[QUOTA].unwrap()-after.x[QUOTA].unwrap()).abs()<0.01);
        motion.step(before.clone(),1.0/60.0,false);assert_eq!(motion.current().x,before.x);
    }
}
