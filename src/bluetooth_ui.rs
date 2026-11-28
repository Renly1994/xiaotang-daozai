//! 无线设备电量环：展开页、详情、常驻条和侧边岛。
//! 动效只在展开时出现（充电流光、低电量呼吸、一次性事件）；常驻条保持静止，不抢注意力。
use crate::{anim,app::{App,Env,BLUETOOTH_PAGE},bt::{BatterySource,Device,DeviceKind},paint::{self,Card,H},theme::{self,W},bar_layout};
use eframe::egui::Color32;
use std::f32::consts::{PI,TAU};
use std::time::Instant;

/// 一次性事件的起始时间（秒，取 App::t）。
#[derive(Clone,Debug,Default)]
pub(crate) struct WirelessFx {
    /// 电量变化：从哪个百分比缓动过来。
    pub morph:Option<(u8,f64)>,
    last_level:Option<(BatterySource,u8)>,
    last_charge:Option<ChargeReport>,
    band_level:u8,
    morph_from_band:u8,
    pub plug:Option<f64>,
    pub unplug:Option<f64>,
    pub full:Option<f64>,
    /// 跌进橙色或红色时，图标左右轻晃。
    pub nudge:Option<f64>,
}
#[derive(Clone,Copy,Debug)]
struct ChargeReport { charging:bool, level:u8, at:Instant }
const FLOW:f64=1.8;
const ENTER:f32=0.9;
const STAGGER:f32=0.07;
const LINKED:&str="已连接";
const RECEIVER_NOTE:&str="2.4G 接收器 · 电量未知";
const NO_BATTERY_NOTE:&str="设备没有提供电量";
/// 连上后这么久还没有电量读数，就当作设备不提供电量。新连接会立即读一次，读数通常几秒内到。
const READ_GRACE:f64=15.0;

fn charge_state(device:&Device,fx:Option<&WirelessFx>)->Option<bool> {
    let now=Instant::now();
    if device.stale_at(now) {return None;}
    device.reading.as_ref().and_then(|r|r.charging).or_else(|| {
        let (charging, observed)=device.charging_only?;
        (now.saturating_duration_since(observed)<=crate::bt::FRESH_FOR).then_some(charging)
    }).or_else(|| {
        let report=fx?.last_charge?;
        (now.saturating_duration_since(report.at)<=crate::bt::FRESH_FOR).then_some(report.charging)
    })
}

fn charging(device:&Device,fx:Option<&WirelessFx>)->bool {
    charge_state(device,fx)==Some(true)
}
fn stale(device:&Device)->bool {device.stale_at(Instant::now())}
/// 绿 ≥50 · 黄 20–49 · 橙 10–19 · 红 <10。
pub(crate) fn level_color(level:u8)->Color32 {
    band_color(band(level))
}
fn band(level:u8)->u8 {if level>=50 {3} else if level>=20 {2} else if level>=10 {1} else {0}}
fn band_color(value:u8)->Color32 {[theme::RED,theme::ORANGE,theme::YELLOW,theme::GREEN][value as usize]}
fn display_band(device:&Device,fx:Option<&WirelessFx>)->Option<u8> {
    device.battery.map(|level|fx.filter(|f|f.last_level.is_some()).map_or_else(||band(level),|f|band(f.band_level)))
}
pub(crate) fn color(device:&Device,fx:Option<&WirelessFx>)->Color32 {
    display_band(device,fx).filter(|_|!stale(device)).map_or(theme::DIM,band_color)
}
fn low(device:&Device,fx:Option<&WirelessFx>)->bool {
    !stale(device)&&!charging(device,fx)&&display_band(device,fx).is_some_and(|b|b<2)
}

/// 闪电由两个三角拼成（填充多边形只支持凸形）。设计稿坐标系 12×16。
fn bolt(c:&Card,cx:f32,cy:f32,h:f32,col:Color32) {
    if h<=0.2 {return;}
    let s=h/16.0;let p=|x:f32,y:f32|(cx+(x-6.0)*s,cy+(y-8.0)*s);
    c.poly(&[p(7.6,0.6),p(1.2,9.0),p(7.0,9.0)],col);
    c.poly(&[p(4.2,15.4),p(10.8,6.8),p(5.0,6.8)],col);
}
/// 充电标识的缩放：接电时弹出、拔电时缩回；展开时跟流光同拍跳一下。
fn bolt_scale(device:&Device,fx:Option<&WirelessFx>,t:f64,live:bool)->f32 {
    let is_charging=charging(device,fx);
    let fx=fx.filter(|_|live).cloned().unwrap_or_default();
    if !is_charging {
        return fx.unplug.map_or(0.0,|at|{let k=((t-at)/0.35) as f32;if k<1.0 {1.0-anim::clamp01(k)}else{0.0}});
    }
    if let Some(at)=fx.plug {
        let k=((t-at)/0.55) as f32;
        if (0.0..1.0).contains(&k) {return if k<0.6 {k/0.6*1.35}else{1.35-0.35*(k-0.6)/0.4};}
    }
    if !live||device.battery==Some(100) {return 1.0;}
    let u=((t%FLOW)/FLOW) as f32;
    if u<0.82 {1.0} else if u<0.91 {1.0+0.22*(u-0.82)/0.09} else {1.22-0.22*(u-0.91)/0.09}
}

/// linked：已连接但没有电量可看（2.4G 接收器，或确认不报电量的设备），画蓝色连接环。
struct RingSpec {r:f32,w:f32,icon:f32,live:bool,enter:f32,hover:bool,linked:bool}

/// 弧线从 12 点钟顺时针。展开时叠加：低电量呼吸、充电流光与弧端闪点、接电/充满波纹、充满扫光、跌档晃动。
fn ring(c:&Card,cx:f32,cy:f32,spec:&RingSpec,device:&Device,fx:Option<&WirelessFx>,t:f64) {
    let rr=spec.r-spec.w/2.0;let chg=charging(device,fx);
    let mut track=if spec.hover {paint::mix(theme::TRACK,theme::WHITE,0.18)} else {theme::TRACK};
    let critical=display_band(device,fx)==Some(0);
    let is_low=low(device,fx);
    let mut col=color(device,fx);
    let fx=fx.filter(|_|spec.live);
    if spec.live&&critical&&!stale(device)&&!chg {track=paint::mix(track,theme::RED,0.22);}
    match device.battery {
        // 已连接无电量：轨道中线压一条细蓝环，表示“连着”，不表示电量。
        None if spec.linked=>{c.ring_centered(cx,cy,rr,spec.w,track);c.ring_centered(cx,cy,rr,spec.w*0.5,paint::alpha(theme::LINK,0.9));}
        // 虚线只留给“正在读取”。
        None=>for i in 0..24 {let a=i as f32/24.0*TAU;c.arc(cx,cy,rr,a,a+TAU/24.0*0.42,spec.w*0.8,track);},
        Some(_)=>c.ring_centered(cx,cy,rr,spec.w,track),
    }
    let target=device.battery.map_or(0.0,|b|b as f32/100.0);
    let mut frac=target;
    if let Some(fx)=fx {
        if let Some((from,at))=fx.morph {
            let k=anim::clamp01(((t-at)/0.8) as f32);
            if k<1.0 {
                let e=anim::ease_out_cubic(k);frac=from as f32/100.0+(target-from as f32/100.0)*e;
                if !stale(device) {col=paint::mix(band_color(fx.morph_from_band),col,e);}
            }
        }
    }
    frac*=anim::ease_out_cubic(spec.enter);
    if spec.live&&is_low {
        // 呼吸用混向轨道色代替透明度，弧线和圆头叠在一起时不会出现深色接缝。
        let period=if critical {1.3} else {2.6};
        let k=0.5-0.5*((t/period) as f32*TAU).cos();col=paint::mix(col,track,0.6*k);
    }
    let a0=-PI/2.0;let a1=a0+TAU*frac;
    if frac>0.002 {
        c.arc(cx,cy,rr,a0,a1,spec.w,col);
        for a in [a0,a1] {c.circle(cx+rr*a.cos(),cy+rr*a.sin(),spec.w/2.0,col);}
    }
    if let Some(fx)=fx.filter(|_|spec.live) {
        for at in [fx.plug,fx.full].into_iter().flatten() {
            for i in 0..2 {
                let k=((t-at) as f32-i as f32*0.9)/0.9;
                if (0.0..1.0).contains(&k) {c.ring_centered(cx,cy,spec.r*(0.95+0.5*k),(spec.w*0.5).max(1.0),paint::alpha(col,0.85*(1.0-k)));}
            }
        }
        if let Some(at)=fx.full {
            let k=((t-at)/1.1) as f32;
            if (0.0..1.0).contains(&k) {let e=anim::ease_in_out(k);let s=a0+e*TAU;c.arc(cx,cy,rr,s,s+0.14*TAU,spec.w,paint::alpha(paint::mix(col,theme::WHITE,0.7),1.0-k));}
        }
    }
    if spec.live&&chg&&frac<0.999&&spec.enter>=1.0&&frac>0.02 {
        let span=TAU*frac;let seg=(0.1*TAU).min(span*0.5);
        let u=((t%FLOW)/FLOW) as f32;let e=anim::ease_in_out(u);
        let fade=anim::smoothstep(0.0,0.15,u)*(1.0-anim::smoothstep(0.85,1.0,u))*0.95;
        let s=a0+e*(span-seg);
        c.arc(cx,cy,rr,s,s+seg,spec.w,paint::alpha(paint::mix(col,theme::WHITE,0.65),fade));
        if u>0.8 {
            let k=(u-0.8)/0.2;let (scale,op)=if k<0.5 {(0.3+1.2*k/0.5,k/0.5)} else {(1.5-0.5*(k-0.5)/0.5,1.0-(k-0.5)/0.5)};
            c.circle(cx+rr*a1.cos(),cy+rr*a1.sin(),spec.w*0.55*scale,paint::alpha(paint::mix(col,theme::WHITE,0.6),op));
        }
    }
    let mut dx=0.0;
    if let Some(at)=fx.and_then(|fx|fx.nudge).filter(|_|spec.live) {
        let e=(t-at) as f32;if (0.0..1.2).contains(&e) {dx=2.5*(e/0.6*TAU).sin()*(1.0-e/1.2);}
    }
    let tint=if (stale(device)&&!spec.linked)||!device.connected {theme::DIM} else {theme::WHITE};
    // 2.4G 接收器在环底压一枚“2.4G”角标，图标略上移给角标让位；常驻小环里不画。
    let chip=device.kind==DeviceKind::Receiver24G&&spec.r>=15.0;
    let dy=if chip {-spec.r*0.08} else {0.0};
    crate::extras::device_icon_tinted(c,device.kind,cx-spec.icon/2.0+dx,cy-spec.icon/2.0+dy,spec.icon,tint);
    if chip {
        let k=if spec.r>35.0 {1.4} else {1.0};
        let (w,h)=(28.0*k,14.0*k);let y=cy+spec.r-h/2.0+spec.r*0.08;
        c.fill(cx-w/2.0-2.0,y-h/2.0-2.0,w+4.0,h+4.0,h/2.0+2.0,Color32::BLACK);
        c.fill(cx-w/2.0,y-h/2.0,w,h,h/2.0,theme::LINK);
        c.centered_text(cx,y,"2.4G",9.0*k,W::Bold,theme::WHITE,H::Center);
    }
}

/// 百分比（充电时前面带闪电）。centered 时整组以 x 居中，否则从 x 左对齐。返回总宽。
fn reading(c:&Card,x:f32,cy:f32,size:f32,device:&Device,fx:Option<&WirelessFx>,t:f64,live:bool,enter:f32,centered:bool,linked:bool)->f32 {
    if linked&&device.battery.is_none() {
        let label=if charging(device,fx) {"充电中"} else {LINKED};
        let size=size*0.87;let tw=c.text_w(label,size,W::Semibold);
        c.centered_text(if centered {x-tw/2.0} else {x},cy,label,size,W::Semibold,theme::LINK_SOFT,H::Left);
        return tw;
    }
    let col=color(device,fx);
    let text=match device.battery {
        None=>"···".to_string(),
        Some(level)=>{
            let mut shown=level as f32;
            if let Some((from,at))=fx.filter(|_|live).and_then(|fx|fx.morph) {
                let k=anim::clamp01(((t-at)/0.8) as f32);shown=from as f32+(level as f32-from as f32)*anim::ease_out_cubic(k);
            }
            let progress=if device.kind==DeviceKind::Receiver24G {1.0} else {anim::ease_out_cubic(enter)};
            let marker=if device.reading.as_ref().is_some_and(|reading| reading.approximate) {"≈"} else {""};
            let cached=if device.reading.as_ref().is_some_and(|r|r.source==BatterySource::Plugin)&&stale(device){"旧 "}else{""};
            format!("{cached}{marker}{}%",(shown*progress).round() as u8)
        }
    };
    let weight=if device.battery.is_some()&&!stale(device) {W::Bold} else {W::Semibold};
    let tw=c.text_w(&text,size,weight);
    let scale=bolt_scale(device,fx,t,live);
    let bw=if scale>0.0 {size*0.62+2.0} else {0.0};
    let left=if centered {x-(tw+bw)/2.0} else {x};
    if scale>0.0 {bolt(c,left+size*0.31,cy,size*0.84*scale,col);}
    c.centered_text(left+bw,cy,&text,size,weight,col,H::Left);
    tw+bw
}

/// 读数时长：1 分钟内按秒，1 小时内按分钟，1 天内按小时（带零头分钟），再往上按天。
pub(crate) fn duration_text(secs:u64)->String {
    let (minutes,hours,days)=(secs/60,secs/3600,secs/86400);
    if secs<60 {format!("{secs} 秒")}
    else if hours==0 {format!("{minutes} 分钟")}
    else if days==0 {if minutes%60==0 {format!("{hours} 小时")} else {format!("{hours} 小时 {} 分钟",minutes%60)}}
    else {format!("{days} 天")}
}
fn reading_age(device:&Device)->Option<String> {
    let reading=device.reading.as_ref()?;
    let secs=match reading.source {
        BatterySource::Windows=>reading.observed.saturating_duration_since(reading.value_since).as_secs(),
        BatterySource::Gatt|BatterySource::UsbReceiver|BatterySource::Plugin=>reading.observed.elapsed().as_secs(),
    };
    let age=duration_text(secs);
    Some(match reading.source {
        BatterySource::Windows if secs==0=>"刚读到".into(),
        BatterySource::Windows=>format!("读数已保持 {age}"),
        BatterySource::Gatt|BatterySource::UsbReceiver=>format!("{age}前读取"),
        BatterySource::Plugin=>format!("{}{age}前读取",if stale(device){"缓存 · "}else{""}),
    })
}

impl App {
    /// 展开页入场进度：每台设备错开 70ms，0.9s 画完。
    fn wireless_enter(&self,index:usize)->f32 {
        self.wireless_enter_at.map_or(1.0,|at|anim::clamp01(((self.t-at) as f32-index as f32*STAGGER)/ENTER))
    }
    /// 展开页是否需要持续重绘：入场、充电流光、低电量呼吸或一次性事件进行中。
    pub(crate) fn wireless_animating(&self)->bool {
        if !(self.open&&self.page==BLUETOOTH_PAGE) {return false;}
        self.wireless_enter_at.is_some_and(|at|self.t-at<ENTER as f64+0.5)
            || self.wireless_devices.iter().any(|d|{let fx=self.wireless_fx.get(&d.id);d.connected&&(charging(d,fx)||low(d,fx))})
            || self.wireless_fx.values().any(|fx|[fx.morph.map(|m|m.1),fx.plug,fx.unplug,fx.full,fx.nudge].into_iter().flatten().any(|at|self.t-at<2.0))
    }
    /// 已连接但没有电量可看：接收器一插上就是；其他设备连上超过 READ_GRACE 仍没读数才算。
    fn wireless_linked(&self,device:&Device)->bool {
        device.connected&&device.battery.is_none()&&(device.kind==DeviceKind::Receiver24G
            ||self.wireless_seen_at.get(&device.id).is_none_or(|at|self.t-at>=READ_GRACE))
    }
    /// 对比新旧快照，记录一次性动效并按需在岛上提示。
    pub(crate) fn track_wireless_changes(&mut self,next:&[Device]) {
        let now=self.t;
        self.wireless_seen_at.retain(|id,_|next.iter().any(|d|d.connected&&d.id==*id));
        for device in next.iter().filter(|d|d.connected) {self.wireless_seen_at.entry(device.id.clone()).or_insert(now);}
        self.wireless_fx.retain(|id,_|next.iter().any(|d|d.connected&&d.id==*id));
        self.wireless_alerted.retain(|_,at|now-*at<600.0);
        for device in next.iter().filter(|d|d.connected&&!stale(d)) {
            let Some(r)=device.reading.as_ref() else {continue};
            let fx=self.wireless_fx.entry(device.id.clone()).or_default();
            let was=fx.last_charge;
            let mut events=vec![];
            match (was.map(|r|r.charging),r.charging) {
                (Some(false),Some(true))=>{fx.plug=Some(now);fx.unplug=None;events.push("开始充电");}
                (Some(true),Some(false))=>{fx.unplug=Some(now);fx.plug=None;}
                _=>{}
            }
            if let Some(charging)=r.charging {
                if r.percent==100&&was.is_some_and(|r|r.level<100&&(r.charging||charging)) {
                    fx.full=Some(now);events.push("已充满，可以拔掉了");
                }
                fx.last_charge=Some(ChargeReport{charging,level:r.percent,at:r.observed});
            }
            let old=fx.last_level;
            let old_band=band(fx.band_level);
            if old!=Some((r.source,r.percent)) {
                // 换来源时容忍两个百分点的偏差，百分比仍展示原值。
                let hold=old.is_some_and(|(source,_)|source!=r.source&&fx.band_level.abs_diff(r.percent)<=2);
                if !hold {fx.band_level=r.percent;}
                let new_band=band(fx.band_level);
                if let Some((_,from))=old.filter(|(_,level)|*level!=r.percent) {
                    fx.morph=Some((from,now));fx.morph_from_band=old_band;
                }
                fx.last_level=Some((r.source,r.percent));
                if !charging(device,Some(fx))&&new_band<2&&(old.is_none()||new_band<old_band) {
                    fx.nudge=Some(now);
                    events.push(if new_band==0 {"电量即将耗尽，请尽快充电"} else {"电量偏低"});
                }
            }
            for event in events {self.battery_alert(device,event);}
        }
    }
    /// 电量事件提示：同一台设备的同一件事 10 分钟内只提示一次。
    fn battery_alert(&mut self,device:&Device,event:&'static str) {
        if !self.settings.bluetooth_battery_notifications||!self.module_on(BLUETOOTH_PAGE) {return;}
        let key=(device.id.clone(),event);
        if self.wireless_alerted.get(&key).is_some_and(|at|self.t-at<600.0) {return;}
        self.wireless_alerted.insert(key,self.t);
        let Some(level)=device.battery else {return};
        self.enqueue_alert(crate::extras::Alert{
            source:crate::extras::AlertSource::Internal,title:"蓝牙设备".into(),
            body:format!("{} · {event} · {}{level}%",device.name,if device.reading.as_ref().is_some_and(|r|r.approximate){"≈"}else{""}),page:BLUETOOTH_PAGE,at:self.t,duration:2.6,
            bluetooth:Some((device.kind,true)),meeting:None,unread:None,
        },false);
    }

    /// 常驻候选：自动模式取有电量读数的设备，自定义只取勾选的；按电量从低到高，最多两台。
    pub(crate) fn wireless_resident(&self)->Vec<Device> {
        if !self.settings.show_bluetooth||!self.module_on(BLUETOOTH_PAGE){return vec![];}
        let custom=self.settings.bluetooth_resident_custom;
        let mut devices:Vec<_>=self.wireless_devices.iter().filter(|d|d.connected&&d.kind!=DeviceKind::Receiver24G)
            .filter(|d|if custom {self.settings.bluetooth_pinned.contains(&d.id)} else {
                d.battery.is_some()&&d.reading.as_ref().is_none_or(|r|r.source!=BatterySource::Plugin||!stale(d))
            }).cloned().collect();
        devices.sort_by_key(|d|(stale(d),d.battery.unwrap_or(101)));
        devices.truncate(2);devices
    }
    fn resident_value(&self,device:&Device)->bool {
        let fx=self.wireless_fx.get(&device.id);
        device.battery.is_some()&&(self.settings.bluetooth_resident_style==0||charging(device,fx)||low(device,fx))
    }
    fn resident_slot(&self,device:&Device)->f32 {
        if !self.resident_value(device) {return 18.0;}
        let digits=device.battery.map_or(1,|b|b.to_string().len()) as f32;
        let prefix=if device.reading.as_ref().is_some_and(|r|r.source==BatterySource::Plugin)&&stale(device){14.0}else{0.0}
            +if device.reading.as_ref().is_some_and(|r|r.approximate){8.0}else{0.0};
        18.0+3.0+if charging(device,self.wireless_fx.get(&device.id)){8.0}else{0.0}+digits*5.8+8.0+prefix
    }
    /// 常驻条上无线设备一组的宽度（含设备之间 8 的间距）。
    pub(crate) fn wireless_bar_width(&self)->f32 {
        let devices=self.wireless_resident();
        devices.iter().map(|d|self.resident_slot(d)).sum::<f32>()+8.0*devices.len().saturating_sub(1) as f32
    }
    /// 常驻条：静止的小环 + 数字，不走流光、不呼吸。
    pub(crate) fn draw_wireless_bar(&mut self,c:&Card,cx:f32,layout:&bar_layout::Layout) {
        let Some(x)=layout.x[bar_layout::BLUETOOTH] else{return;};
        let devices=self.wireless_resident();let total=self.wireless_bar_width().min(layout.widths[bar_layout::BLUETOOTH]+0.5);
        let mut left=cx+x-total/2.0;
        for device in &devices {
            let w=self.resident_slot(device);if left+w>cx+x+total/2.0+0.5 {break;}
            let spec=RingSpec{r:9.0,w:2.0,icon:10.0,live:false,enter:1.0,hover:false,linked:self.wireless_linked(device)};
            let fx=self.wireless_fx.get(&device.id);
            ring(c,left+9.0,15.0,&spec,device,fx,self.t);
            if self.resident_value(device) {reading(c,left+21.0+(w-21.0)/2.0,15.5,9.5,device,fx,self.t,false,1.0,true,false);}
            let tip=format!("{} · {}",device.name,device.battery.map_or("未上报电量".into(),|b|format!("{}{}{b}%",if stale(device){"缓存 "}else{""},if device.reading.as_ref().is_some_and(|r|r.approximate){"≈"}else{""})));
            let hit=c.hit(left-2.0,2.0,w+4.0,26.0,("wireless-resident",&device.id));
            if crate::quota_ui::bar_tooltip(hit,tip).clicked(){self.wireless_focus=Some(device.id.clone());self.show_page(BLUETOOTH_PAGE);}
            left+=w+8.0;
        }
    }
    pub(crate) fn draw_wireless_side(&mut self,c:&Card,cx:f32,y:f32) {
        let Some(device)=self.wireless_resident().into_iter().next() else{return;};
        let spec=RingSpec{r:11.0,w:2.2,icon:12.0,live:false,enter:1.0,hover:false,linked:self.wireless_linked(&device)};
        let value=self.resident_value(&device);
        let ring_y=if value {y-6.0} else {y};
        let fx=self.wireless_fx.get(&device.id);
        ring(c,cx,ring_y,&spec,&device,fx,self.t);
        if value {reading(c,cx,y+14.0,8.5,&device,fx,self.t,false,1.0,true,false);}
        let hit=c.hit(cx-17.0,y-18.0,34.0,37.0,"wireless-side");
        if crate::quota_ui::bar_tooltip(hit,device.name.clone()).clicked(){self.wireless_focus=Some(device.id.clone());self.show_page(BLUETOOTH_PAGE);}
    }

    pub(crate) fn page_wireless(&mut self,c:&Card,env:&Env) {
        if self.wireless_enter_at.is_none() {self.wireless_enter_at=Some(self.t);}
        let devices=self.wireless_devices.clone();
        // 只有一台时直接用详情版式，不让一个小环孤零零地待在中间。
        if devices.len()==1 {self.wireless_detail(c,&devices[0],false);return;}
        if let Some(id)=self.wireless_focus.clone() {
            if let Some(device)=devices.iter().find(|d|d.id==id) {
                self.wireless_detail(c,device,true);
                return;
            }
            self.wireless_focus=None;
        }
        if devices.is_empty(){
            crate::extras::device_icon(c,DeviceKind::Other,191.5,51.0,36.0);
            c.text(209.5,112.0,if env.bt_ready{"还没有连接的设备"}else{"正在读取无线设备"},13.0,W::Semibold,theme::WHITE,H::Center);
            c.text(209.5,135.0,"连接蓝牙设备或插入支持的 2.4G 接收器",10.5,W::Regular,theme::DIM,H::Center);return;
        }
        let per_page=if devices.len()<=6 {devices.len()} else {4};
        let pages=devices.len().div_ceil(per_page);self.wireless_offset=self.wireless_offset.min(pages-1);
        let shown:Vec<_>=devices.iter().skip(self.wireless_offset*per_page).take(per_page).collect();
        let compact=per_page>4;
        let (col_w,gap,r,w,icon,pct,name,ring_y,pct_y,name_y)=if compact {(64.0,4.0,23.0,3.5,19.0,13.0,9.5,78.0,118.0,136.0)} else {(82.0,12.0,29.0,4.0,24.0,15.0,10.0,80.0,127.0,146.0)};
        let count=shown.len() as f32;
        // 展开页两侧留给翻页箭头，两端的设备也能完整点击。
        let col_w=f32::min(col_w,(crate::app::PW-72.0-(count-1.0)*gap)/count);
        let start=(crate::app::PW-(count*col_w+(count-1.0)*gap))/2.0;
        for (index,device) in shown.iter().enumerate() {
            let x=start+index as f32*(col_w+gap);let cx=x+col_w/2.0;
            let linked=self.wireless_linked(device);
            let details=if device.kind==DeviceKind::Receiver24G&&device.reading.is_none() {format!("{}\n{RECEIVER_NOTE}",device.name)}
                else if linked {format!("{}\n{LINKED} · {}",device.name,if device.id.starts_with("plugin:"){"等待插件更新电量"}else{NO_BATTERY_NOTE})} else {
                device.reading.as_ref().map(|r|format!("{}\n{} · {}",device.name,r.label(),reading_age(device).unwrap_or_default())).unwrap_or_else(||format!("{}\n正在读取电量",device.name))
            };
            let tile_top=ring_y-r-6.0;
            let tile_bottom=name_y+8.0;
            let hit=c.hit(x,tile_top,col_w,tile_bottom-tile_top,("wireless-device",&device.id)).on_hover_text(details);
            let hover=hit.hovered();
            let enter=self.wireless_enter(index);let fx=self.wireless_fx.get(&device.id).cloned();
            let cc=if device.connected {c.clone()} else {c.faded(0.4)};
            ring(&cc,cx,ring_y,&RingSpec{r,w,icon,live:true,enter,hover,linked},device,fx.as_ref(),self.t);
            let rise=(1.0-anim::ease_out_cubic(anim::clamp01((enter-0.25)/0.55)))*6.0;
            let tc=cc.faded(anim::clamp01((enter-0.25)/0.55));
            reading(&tc,cx,pct_y+rise,pct,device,fx.as_ref(),self.t,true,enter,true,linked);
            let label=tc.fit(&device.name,name,W::Regular,col_w-2.0);
            tc.centered_text(cx,name_y+rise,&label,name,W::Regular,if hover{theme::WHITE}else{theme::DIM},H::Center);
            if hit.clicked() {self.wireless_focus=Some(device.id.clone());}
        }
        if pages>1 {
            c.text(322.0,178.0,&format!("{} / {pages}",self.wireless_offset+1),9.5,W::Regular,theme::GRAY,H::Center);
            for (x,next,text) in [(296.0,false,"‹"),(348.0,true,"›")] {
                c.text(x,179.0,text,15.0,W::Semibold,theme::WHITE,H::Center);
                if c.hit(x-12.0,165.0,24.0,20.0,("wireless-page",next)).clicked(){self.wireless_offset=(self.wireless_offset+if next{1}else{pages-1})%pages;}
            }
        }
    }
    /// 详情：环、名称、读数与总览同一顺序，只是放大。有返回入口时标题前带“‹”。
    fn wireless_detail(&mut self,c:&Card,device:&Device,back:bool) {
        if back {
            let wa=c.text(15.0,25.0,"‹ 无线",crate::theme::size::TITLE,W::Bold,theme::WHITE,H::Left);
            c.text(15.0+wa+1.5,25.0,"设备",crate::theme::size::TITLE,W::Bold,theme::CYAN,H::Left);
            let title_back=c.hit(6.0,4.0,wa+46.0,30.0,"wireless-back");
            let detail_back=c.hit(36.0,46.0,346.0,100.0,"wireless-detail-back")
                .on_hover_cursor(eframe::egui::CursorIcon::PointingHand);
            if title_back.clicked()||detail_back.clicked(){self.wireless_focus=None;}
        }
        let fx=self.wireless_fx.get(&device.id).cloned();let enter=self.wireless_enter(0);
        let linked=self.wireless_linked(device);
        ring(c,112.0,100.0,&RingSpec{r:50.0,w:6.0,icon:38.0,live:true,enter,hover:false,linked},device,fx.as_ref(),self.t);
        let x=190.0;
        c.centered_text(x,66.0,&c.fit(&device.name,15.0,W::Bold,190.0),15.0,W::Bold,theme::WHITE,H::Left);
        if device.kind==DeviceKind::Receiver24G&&device.reading.is_none() {
            reading(c,x,101.0,26.0,device,None,self.t,true,1.0,false,true);
            c.text(x,140.0,RECEIVER_NOTE,10.5,W::Regular,theme::DIM,H::Left);
            return;
        }
        reading(c,x,101.0,if linked {26.0} else {34.0},device,fx.as_ref(),self.t,true,enter,false,linked);
        let mut mx=x;
        match charge_state(device,fx.as_ref()) {
            Some(true)=>{mx+=c.text(mx,140.0,"充电中",10.5,W::Semibold,color(device,fx.as_ref()),H::Left)+10.0;}
            None if device.reading.as_ref().is_some_and(|r|r.source!=BatterySource::Plugin)&&!stale(device)=>{
                c.text(x,126.0,"充电状态未知",10.5,W::Regular,theme::DIM,H::Left);
            }
            _=>{}
        }
        let source=device.reading.as_ref().map_or_else(||if device.id.starts_with("plugin:"){"插件未提供有效电量".to_owned()} else if linked {NO_BATTERY_NOTE.to_owned()} else {"正在读取电量".to_owned()},|r|{
            if r.source==BatterySource::Plugin&&charge_state(device,fx.as_ref()).is_none()&&!stale(device) {
                format!("{} · 充电未知",r.label())
            } else {r.label().to_owned()}
        });
        let source=c.fit(&source,10.5,W::Regular,(380.0-mx).max(24.0));
        mx+=c.text(mx,140.0,&source,10.5,W::Regular,theme::DIM,H::Left)+10.0;
        if let Some(ago)=reading_age(device) {
            if device.reading.as_ref().is_some_and(|r|r.source==BatterySource::Plugin) {
                c.text(x,126.0,&ago,10.5,W::Regular,theme::DIM,H::Left);
            } else {c.text(mx,140.0,&ago,10.5,W::Regular,theme::DIM,H::Left);}
        }
        if device.kind==DeviceKind::Receiver24G {return;}
        let automatic=(!self.settings.bluetooth_resident_custom).then(||self.wireless_resident());
        let selected=automatic.as_ref().map_or_else(
            ||self.settings.bluetooth_pinned.contains(&device.id),
            |devices|devices.iter().any(|d|d.id==device.id),
        );
        let on=self.settings.show_bluetooth&&selected;
        let (tx,ty)=(x,158.0);
        c.fill(tx,ty-7.0,24.0,14.0,7.0,if on{theme::CYAN}else{theme::TRACK});
        c.circle(if on{tx+17.0}else{tx+7.0},ty,5.0,if on{Color32::BLACK}else{theme::DIM});
        c.centered_text(tx+31.0,ty,"常驻显示",10.5,W::Regular,if on{theme::WHITE}else{theme::DIM},H::Left);
        let hint=match (on,automatic.is_some()) {
            (true,true)=>"取消这台设备的常驻，保留其他常驻设备",
            (true,false)=>"取消这台设备的常驻",
            (false,_)=>"在收起的岛上常驻显示这台设备",
        };
        if c.hit(tx-4.0,ty-11.0,92.0,22.0,("wireless-pin",&device.id)).on_hover_text(hint).clicked() {
            if let Some(devices)=automatic {
                self.settings.bluetooth_resident_custom=true;
                self.settings.bluetooth_pinned.clear();
                if on {
                    self.settings.bluetooth_pinned.extend(devices.into_iter().map(|d|d.id));
                }
            }
            if on {
                self.settings.bluetooth_pinned.retain(|id|id!=&device.id);
            } else {
                self.settings.show_bluetooth=true;
                if !self.settings.bluetooth_pinned.contains(&device.id) {
                    self.settings.bluetooth_pinned.push(device.id.clone());
                }
            }
            self.settings.save();
        }
    }
}

impl App {
    /// 无线设备设置中的常驻样式与设备选择。
    pub(crate) fn wireless_resident_settings(&mut self,ui:&mut eframe::egui::Ui) {
        use crate::extras::{settings_control_row_right,settings_segmented,settings_hint};
        let mut style=self.settings.bluetooth_resident_style;
        settings_control_row_right(ui,"常驻样式",|ui|{settings_segmented(ui,&mut style,&[(0,"环 + 数字"),(1,"只显示环")],76.0);});
        self.settings.bluetooth_resident_style=style;
        settings_hint(ui,if style==0 {"显示电量环和百分比"} else {"电量偏低或充电时显示百分比"});
        ui.separator();
        let mut custom=self.settings.bluetooth_resident_custom;
        settings_control_row_right(ui,"常驻设备",|ui|{settings_segmented(ui,&mut custom,&[(false,"自动"),(true,"自定义")],62.0);});
        self.settings.bluetooth_resident_custom=custom;
        settings_hint(ui,if custom {"优先显示所选设备中电量较低的设备"} else {"自动挑电量最低的设备常驻"});
        if custom {
            let mut devices=self.wireless_devices.iter().filter(|d|d.kind!=DeviceKind::Receiver24G).peekable();
            if devices.peek().is_none() {settings_hint(ui,"连接蓝牙设备后可以在这里勾选");}
            for device in devices {
                let mut on=self.settings.bluetooth_pinned.contains(&device.id);
                let label=match device.battery {Some(level)=>format!("{} · {level}%",device.name),None=>format!("{} · 未上报电量",device.name)};
                if ui.checkbox(&mut on,label).changed() {
                    if on {self.settings.bluetooth_pinned.push(device.id.clone());} else {self.settings.bluetooth_pinned.retain(|id|id!=&device.id);}
                }
            }
        }
    }
}
