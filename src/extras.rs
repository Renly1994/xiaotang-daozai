//! 设置、通知队列与岛仔表情。
use crate::settings_theme::color as settings_color;
use crate::app::{App, Cmd, Env, NotificationSound, Phase, HUD_TRACK_W, HUD_TRACK_X, HUD_W, PAGE_NAMES};
use crate::paint::{Card, H};
use crate::theme::{self, size, W};
use eframe::egui::{self, Color32, Rect};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AlertSource {
    Internal,
    MailCode(String,u32),
    Messenger(crate::app::Messenger),
    External(String),
    #[cfg(windows)]
    Plugin { source: String, id: String },
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum HealthReminder {Water,Sit}
impl HealthReminder {
    pub(crate) const ALL:[Self;2]=[Self::Water,Self::Sit];
    pub(crate) fn title(self)->&'static str {match self {Self::Water=>"喝水时间到了",Self::Sit=>crate::app::SIT_TITLE}}
    pub(crate) fn label(self)->&'static str {match self {Self::Water=>"待喝水",Self::Sit=>"待活动"}}
    pub(crate) fn id(self)->&'static str {match self {Self::Water=>"health-water",Self::Sit=>"health-sit"}}
    pub(crate) fn icon(self,c:&Card,x:f32,y:f32,scale:f32) {
        let c=&c.scaled(x,y,scale);
        match self {
            Self::Water=>{
                c.poly(&[(x-7.0,y-1.0),(x+5.0,y-1.0),(x+3.5,y+8.0),(x-5.5,y+8.0)],theme::BLUE);
                c.path(&[(x-9.0,y-10.0),(x-6.5,y+9.0),(x+4.5,y+9.0),(x+7.0,y-10.0)],1.6,theme::CYAN);
                c.seg((x-9.0,y-10.0),(x+7.0,y-10.0),1.6,theme::CYAN);
                c.path(&[(x+6.5,y-6.0),(x+10.5,y-6.0),(x+11.5,y-3.0),(x+10.0,y+2.0),(x+5.5,y+3.0)],1.6,theme::CYAN);
                c.seg((x-5.0,y-6.5),(x-4.2,y-2.5),1.0,Color32::from_white_alpha(170));
            },
            Self::Sit=>crate::paint::chair(c,x,y,0.6,theme::SOFT_GREEN),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Alert {
    pub title: String,
    pub source: AlertSource,
    pub body: String,
    pub page: usize,
    pub at: f64,
    pub duration: f64,
    pub bluetooth: Option<(crate::bt::DeviceKind, bool)>,
    pub meeting: Option<(crate::mail::Meeting, String)>,
    pub unread: Option<crate::app::Unread>,
}
impl Alert {
    pub(crate) fn health(&self)->Option<HealthReminder> {
        if self.source!=AlertSource::Internal{return None;}
        HealthReminder::ALL.into_iter().find(|kind|self.title==kind.title())
    }
    pub(crate) fn messenger(&self)->Option<crate::app::Messenger>{
        match &self.source {
            AlertSource::Messenger(client)=>Some(*client),
            AlertSource::External(id)=>crate::app::Messenger::from_notification_id(id).filter(|client|client.adapter().capture==crate::messenger::Capture::SystemToast),
            _=>None,
        }
    }
    pub(crate) fn is_hud(&self)->bool {self.source==AlertSource::Internal && matches!(self.title.as_str(),"系统音量"|"大写锁定"|"键盘输入语言"|"网络连接"|"电源已连接"|"已切换电池供电"|"蓝牙设备")}
}

fn reminder_module(title:&str)->Option<usize> {
    use crate::app::*;
    match title {
        "岛仔已暂停音乐功能"=>Some(MUSIC_PAGE),
        "专注计时"=>Some(FOCUS_PAGE),
        "喝水时间到了"|SIT_TITLE|"今日饮水目标达成"=>Some(WATER_PAGE),
        "待办到时间了"=>Some(TODO_PAGE),
        "新邮件"=>Some(MAIL_PAGE),
        "会议提醒"=>Some(CALENDAR_PAGE),
        _=>None,
    }
}

fn mail_invitation(title:&str,body:&str)->bool {
    title=="会议提醒" && body.ends_with(" · 邮件邀请")
}

fn bluetooth_notice_content(body:&str)->(&str,&str,Option<u8>){
    if let Some(name)=body.strip_suffix(" · 已断开") {return (name,"已断开",None);}
    if body=="已断开连接" {return ("蓝牙设备","已断开",None);}
    if let Some((name,level))=body.rsplit_once(" · 电量 ") {
        if let Some(level)=level.strip_suffix('%').and_then(|s|s.parse::<u8>().ok()).filter(|v|*v<=100){
            return (name,"已连接",Some(level));
        }
    }
    // 电量事件：「名称 · 开始充电 · 64%」。
    if let Some((rest,level))=body.rsplit_once(" · ") {
        if let (Some(level),Some((name,status)))=(level.strip_suffix('%').and_then(|s|s.parse::<u8>().ok()).filter(|v|*v<=100),rest.rsplit_once(" · ")) {
            return (name,status,Some(level));
        }
    }
    (body,"已连接",None)
}

/// 每种设备一个图标；入耳、骨传导、耳夹等各有外形，AirPods 系列按带杆样式单独画。
pub(crate) fn device_icon_source(kind: crate::bt::DeviceKind) -> egui::ImageSource<'static> {
    use crate::bt::DeviceKind::*;
    match kind {
        Headphones=>egui::include_image!("../assets/device-icons/headphones-round-bold.svg"),
        Earbuds=>egui::include_image!("../assets/device-icons/earbuds.svg"),
        EarbudsStem=>egui::include_image!("../assets/device-icons/earbuds-stem.svg"),
        EarbudsStemTips=>egui::include_image!("../assets/device-icons/earbuds-stem-tips.svg"),
        OpenEar=>egui::include_image!("../assets/device-icons/open-ear.svg"),
        Bone=>egui::include_image!("../assets/device-icons/bone.svg"),
        HearingAid=>egui::include_image!("../assets/device-icons/hearing-aid.svg"),
        Speaker=>egui::include_image!("../assets/device-icons/speaker-bold.svg"),
        Mic=>egui::include_image!("../assets/device-icons/mic.svg"),
        Keyboard=>egui::include_image!("../assets/device-icons/keyboard-bold.svg"),
        Mouse=>egui::include_image!("../assets/device-icons/mouse-bold.svg"),
        Trackpad=>egui::include_image!("../assets/device-icons/trackpad.svg"),
        Pen=>egui::include_image!("../assets/device-icons/pen.svg"),
        Presenter=>egui::include_image!("../assets/device-icons/presenter.svg"),
        Remote=>egui::include_image!("../assets/device-icons/remote.svg"),
        Gamepad=>egui::include_image!("../assets/device-icons/gamepad-bold.svg"),
        JoyCon=>egui::include_image!("../assets/device-icons/joycon.svg"),
        Phone=>egui::include_image!("../assets/device-icons/smartphone-bold.svg"),
        Tablet=>egui::include_image!("../assets/device-icons/tablet.svg"),
        Watch=>egui::include_image!("../assets/device-icons/watch.svg"),
        Band=>egui::include_image!("../assets/device-icons/band.svg"),
        Health=>egui::include_image!("../assets/device-icons/health.svg"),
        Receiver24G=>egui::include_image!("../assets/device-icons/usb-receiver.svg"),
        Other=>egui::include_image!("../assets/device-icons/bluetooth-linear.svg"),
    }
}
pub(crate) const ALL_DEVICE_KINDS:[crate::bt::DeviceKind;24]={
    use crate::bt::DeviceKind::*;
    [Headphones,Earbuds,EarbudsStem,EarbudsStemTips,OpenEar,Bone,HearingAid,Speaker,Mic,Keyboard,Mouse,Trackpad,Pen,Presenter,Remote,Gamepad,JoyCon,Phone,Tablet,Watch,Band,Health,Receiver24G,Other]
};
pub(crate) fn device_icon(c: &Card, kind: crate::bt::DeviceKind, x: f32, y: f32, size: f32) {
    device_icon_tinted(c,kind,x,y,size,Color32::WHITE);
}
pub(crate) fn device_icon_tinted(c: &Card, kind: crate::bt::DeviceKind, x: f32, y: f32, size: f32, tint: Color32) {
    let icon=device_icon_source(kind);
    let rect=c.rect(x,y,size,size);
    let egui::ImageSource::Bytes{uri,bytes}=icon else{return};
    let pixels=(rect.width()*c.ui.ctx().pixels_per_point()).ceil() as u32;
    let icon=egui::Image::from_bytes(format!("{uri}/{pixels}.svg"),bytes);
    if let Ok(egui::load::TexturePoll::Ready { texture })=icon.fit_to_exact_size(rect.size()).load_for_size(c.ui.ctx(),rect.size()) {
        crate::skin_paint::mark_monochrome(texture.id);
        c.painter.image(texture.id,rect,Rect::from_min_max(egui::Pos2::ZERO,egui::pos2(1.0,1.0)),tint);
    }
}

pub(crate) use crate::pet::PetState;

pub(crate) fn notice_badge(c:&Card,x:f32,y:f32,a:&Alert){
    if a.source==AlertSource::Internal && a.title=="蓝牙设备"{
        c.circle(x,y,11.0,if a.body.contains("已断开"){Color32::from_gray(95)}else{theme::BLUE});
        device_icon(c,a.bluetooth.map_or(crate::bt::DeviceKind::Other,|v|v.0),x-7.5,y-7.5,15.0);
    }else{notice_icon(&c.scaled(x,y,0.85),x,y,a);}
}
pub(crate) fn messenger_icon(c:&Card,x:f32,y:f32,source:crate::app::Messenger){(source.adapter().paint_icon)(c,x,y);}
fn notice_icon(c:&Card,x:f32,y:f32,a:&Alert) {
    if a.source!=AlertSource::Internal{crate::paint::bell(c,x,y,theme::WHITE,0.0);return;}
    match a.title.as_str() {
        "系统音量"=>{
            c.poly(&[(x-9.0,y-3.0),(x-5.0,y-3.0),(x,y-7.0),(x,y+7.0),(x-5.0,y+3.0),(x-9.0,y+3.0)],theme::WHITE);
            c.arc(x,y,5.0,-0.8,0.8,1.5,theme::WHITE);c.arc(x,y,8.0,-0.7,0.7,1.3,theme::WHITE);
        },
        "大写锁定"=>{
            let ink=if a.body=="已开启"{theme::GREEN}else{theme::GRAY};
            c.poly(&[(x-8.0,y-1.0),(x,y-9.0),(x+8.0,y-1.0)],ink);
            c.fill(x-3.0,y-1.5,6.0,6.5,0.5,ink);
            c.fill(x-4.0,y+7.0,8.0,2.0,0.8,ink);
        },
        "键盘输入语言"=>{
            c.stroke(x-10.0,y-8.5,20.0,17.0,4.0,1.2,theme::GRAY);
            let label=match a.body.as_str(){"中文"=>"中","英语"=>"EN",_=>"Aa"};
            c.centered_text(x,y,label,if label=="EN"{9.0}else{11.0},W::Semibold,theme::WHITE,H::Center);
        },
        "网络连接"=>{
            let connected=a.body.contains("已连接");
            let ink=if connected{theme::GREEN}else{theme::GRAY};
            if a.body.starts_with("Wi-Fi") {
                for r in [4.0,8.0,12.0]{c.arc(x,y+6.0,r,-2.35,-0.79,1.7,ink);}
                c.circle(x,y+6.0,1.6,ink);
            }else if a.body.starts_with("以太网") {
                c.stroke(x-8.0,y-8.0,16.0,14.0,2.5,1.6,ink);
                for dx in [-4.0,-1.3,1.3,4.0]{c.seg((x+dx,y-5.0),(x+dx,y-1.0),1.2,ink);}
                c.path(&[(x-4.0,y+6.0),(x-4.0,y+9.0),(x+4.0,y+9.0),(x+4.0,y+6.0)],1.6,ink);
            }else{
                c.ring(x,y,8.0,1.4,ink);c.seg((x-8.0,y),(x+8.0,y),1.0,ink);
                c.stroke(x-3.5,y-8.0,7.0,16.0,3.5,1.0,ink);
            }
            if !connected{c.seg((x-8.0,y-8.0),(x+9.0,y+9.0),2.0,theme::WHITE);}
        },
        "电源已连接"|"已切换电池供电"|"电量偏低"=>{
            let low=a.title=="电量偏低";let ink=if low{theme::RED_TOP}else if a.title=="电源已连接"{theme::GREEN}else{theme::WHITE};
            c.stroke(x-11.0,y-6.0,20.0,12.0,3.0,1.5,ink);c.fill(x+10.0,y-2.5,2.0,5.0,0.8,ink);
            if low{c.fill(x-8.0,y-3.5,3.5,7.0,1.0,ink);}
            else if a.title=="电源已连接"{
                c.poly(&[(x,y-4.5),(x-4.0,y+0.8),(x+0.5,y+0.8)],ink);
                c.poly(&[(x+0.5,y-0.8),(x+4.0,y-0.8),(x-1.0,y+4.5)],ink);
            }else{c.fill(x-8.0,y-3.5,12.0,7.0,1.0,ink);}
        },
        "喝水时间到了"=>HealthReminder::Water.icon(c,x,y,1.0),
        crate::app::SIT_TITLE=>HealthReminder::Sit.icon(c,x,y,1.0),
        "待办到时间了"=>crate::paint::checklist(c,x,y,23.0,Color32::from_rgb(190,180,255),Color32::from_rgb(43,35,66)),
        "会议提醒"=>{
            let label=a.meeting.as_ref().map(|(_,source)|source.as_str()).unwrap_or("");
            let source=if label=="本机日程"{crate::calendar_ui::Source::Local}else if label.starts_with("邮件"){crate::calendar_ui::Source::Mail("")}
                else{crate::calendar_ui::Source::Calendar(crate::calendar::Provider::ALL.into_iter().find(|provider|provider.label()==label))};
            crate::calendar_ui::source_icon(c,source,x-14.0,y-14.0,28.0);
        },
        "专注计时"=>crate::paint::stopwatch(&c.scaled(x,y,0.65),x,y,8.0,0.0,theme::RED_TOP),
        _=>crate::paint::bell(&c.scaled(x,y,1.7),x,y,theme::GRAY,0.0),
    }
}

impl App {
    pub(crate) fn health_badges(&self)->Vec<HealthReminder> {
        if self.fullscreen_hidden || !self.module_on(crate::app::WATER_PAGE){return Vec::new();}
        HealthReminder::ALL.into_iter().filter(|kind|self.health_pending[*kind as usize]
            && (match kind {HealthReminder::Water=>self.water.remind,HealthReminder::Sit=>self.settings.sit_remind})
            && (self.open || !self.notice.as_ref().is_some_and(|a|a.health()==Some(*kind)))).collect()
    }
    pub(crate) fn confirm_health_reminder(&mut self,kind:HealthReminder) {
        self.health_pending[kind as usize]=false;
        crate::model::save_health_pending(self.health_pending);
        match kind {HealthReminder::Water=>self.last_reminder=self.t,HealthReminder::Sit=>self.sit_start=self.t}
        self.alerts.retain(|a|a.health()!=Some(kind));
        if self.notice.as_ref().is_some_and(|a|a.health()==Some(kind)){self.notice=None;}
        if self.hud_notice.as_ref().is_some_and(|a|a.health()==Some(kind)){self.hud_notice=None;}
    }
    pub(crate) fn phase_secs(&self, phase: Phase) -> f32 {
        (match phase {
            Phase::Focus => self.settings.focus_minutes,
            Phase::Rest => self.settings.rest_minutes,
        }) as f32
            * 60.0
    }
    /// 已连续久坐的分钟数。
    pub(crate) fn sat_minutes(&self) -> u32 {
        ((self.t - self.sit_start).max(0.0) / 60.0) as u32
    }
    /// 下一次久坐提醒的时刻：从上次确认或起身起算满 sit_minutes。
    pub(crate) fn sit_due(&self) -> f64 {
        self.sit_start + self.settings.sit_minutes as f64 * 60.0
    }
    /// 「起身了」：久坐计时归零。
    pub(crate) fn stood_up(&mut self) {
        self.confirm_health_reminder(HealthReminder::Sit);
        self.pet.react(PetState::Happy);
        self.say("久坐计时已重新开始");
    }
    /// 现在是否在午休免打扰时段内（支持跨午夜；起止相同视为未设置）。
    pub(crate) fn lunch_now(&self, env: &Env) -> bool {
        let (a, b) = (self.settings.lunch_start, self.settings.lunch_end);
        let m = env.hour() as u32 * 60 + env.minute();
        self.settings.lunch_rest && a != b && if a < b { m >= a && m < b } else { m >= a || m < b }
    }
    /// 喝水和久坐提醒此刻能否弹出：在提醒时段内，且不在午休。
    pub(crate) fn remind_window(&self, env: &Env) -> bool {
        self.water_hours(env.hour() as u32) && !self.lunch_now(env)
    }
    pub(crate) fn water_hours(&self, hour: u32) -> bool {
        let (a, b) = (self.settings.water_start, self.settings.water_end);
        if a == b {
            true
        } else if a < b {
            hour >= a && hour < b
        } else {
            hour >= a || hour < b
        }
    }
    pub(crate) fn pet_state(&self) -> PetState {
        if let Some((state, _)) = self.pet.reaction {
            state
        } else if self.pet.waking_until > self.t {
            PetState::Waking
        } else if self.pomo.running && self.pomo.phase == Phase::Focus {
            PetState::Focused
        } else if self.pet.worried {
            PetState::Worried
        } else if self.media.playing && self.media.is_music() {
            if self.notice.is_some() || self.hud_notice.is_some() {PetState::Awake}else{PetState::Music}
        } else if self.pet.hovered {
            PetState::Curious
        } else if self.idle_for() >= 180.0 {
            PetState::Sleeping
        } else if self.idle_for() >= self.opts.pet_idle {
            PetState::Dozing
        } else {
            PetState::Awake
        }
    }
    pub fn alert(
        &mut self,
        title: impl Into<String>,
        body: impl Into<String>,
        page: usize,
        system: bool,
    ) {
        let title = title.into();
        let transient = matches!(
            title.as_str(),
            "系统音量" | "大写锁定" | "键盘输入语言" | "网络连接" | "电源已连接" | "已切换电池供电" | "蓝牙设备"
        );
        let a = Alert {
            source: AlertSource::Internal,
            title,
            body: body.into(),
            page,
            at: self.t,
            duration: if transient { 2.5 } else { 8.0 },
            bluetooth: None,
            meeting: None,
            unread: None,
        };
        self.enqueue_alert(a, system);
    }
    pub(crate) fn meeting_alert(&mut self, meeting: crate::mail::Meeting, source: String, page: usize) {
        self.enqueue_alert(Alert {
            source: AlertSource::Internal,
            title: "会议提醒".into(), body: format!("{} · {} · {}", meeting.title, meeting.at, source),
            page, at: self.t, duration: 12.0, bluetooth: None, meeting: Some((meeting, source)), unread: None,
        }, true);
    }
    pub(crate) fn new_mail_alert(&mut self, body: String) {
        self.alert("新邮件",body,crate::app::MAIL_PAGE,false);
        self.mail.bell_at=Some(self.t);
    }
    pub(crate) fn mail_code_alert(&mut self, account: String, uid: u32, body: String) {
        self.enqueue_alert(Alert {
            source: AlertSource::MailCode(account,uid), title: "新邮件".into(), body,
            page: crate::app::MAIL_PAGE, at: self.t, duration: 8.0, bluetooth: None, meeting: None, unread: None,
        }, false);
    }
    pub(crate) fn bluetooth_alert(&mut self, device: &crate::bt::Device, connected: bool) {
        if !self.settings.bluetooth_notifications { return; }
        if connected && device.kind.is_audio() {
            self.alerts.retain(|a|a.title!="系统音量");
            if self.notice.as_ref().is_some_and(|a|a.title=="系统音量") {self.notice=None;}
        }
        let a = Alert {
            source: AlertSource::Internal,
            title: "蓝牙设备".into(),
            body: if connected { device.summary() } else { format!("{} · 已断开", device.name) },
            page: 5, at: self.t, duration: 2.5,
            bluetooth: Some((device.kind, connected)),
            meeting: None,
            unread: None,
        };
        self.enqueue_alert(a, false);
    }
    pub(crate) fn enqueue_alert(&mut self, a: Alert, system: bool) {
        if a.source==AlertSource::Internal &&
            (reminder_module(&a.title).is_some_and(|page|!self.module_on(page))
                || mail_invitation(&a.title,&a.body)&&!self.module_on(crate::app::MAIL_PAGE)) {return;}
        if let Some(kind)=a.health() {
            if self.health_pending[kind as usize]{return;}
            self.health_pending[kind as usize]=true;
            crate::model::save_health_pending(self.health_pending);
        }
        // 通知仅保存在本次运行的内存中。
        if matches!(a.source, AlertSource::Internal | AlertSource::MailCode(..)) {
            self.history.push_front(Alert { source: AlertSource::Internal, ..a.clone() });
        }
        self.history.truncate(50);
        if self.fullscreen_hidden && a.health().is_some(){return;}
        if self.settings.notifications {
            const MAX_PENDING_ALERTS: usize = 8;
            #[cfg(windows)]
            if let AlertSource::Plugin { source, .. } = &a.source {
                let pending = self.alerts.iter_mut().find(|alert| {
                    matches!(&alert.source, AlertSource::Plugin { source: current, .. } if current == source)
                });
                if let Some(pending) = pending {
                    *pending = a;
                } else if self.alerts.len() < MAX_PENDING_ALERTS {
                    self.alerts.push_back(a);
                }
                return;
            }
            if system && a.source==AlertSource::Internal && self.settings.system_notify {
                let sound = if !self.settings.sound { NotificationSound::Silent }
                    else if a.health().is_some() { NotificationSound::Health }
                    else { NotificationSound::System };
                self.pending.push(Cmd::Notify(a.title.clone(), a.body.clone(), sound));
            }
            let same_event=|old:&Alert| match (&a.meeting,&old.meeting) {
                (Some((next,_)),Some((previous,_)))=>crate::calendar::dedupe_key(next)==crate::calendar::dedupe_key(previous),
                _=>old.source==a.source && old.title==a.title && old.unread==a.unread && (a.is_hud() || old.body==a.body),
            };
            if a.source!=AlertSource::Internal || a.title != "蓝牙设备" { self.alerts.retain(|old| !same_event(old)); }
            if (a.source!=AlertSource::Internal || a.title != "蓝牙设备") && self.notice.as_ref().is_some_and(same_event) {
                self.notice = Some(a);
                return;
            }
            let index = self.alerts.len();
            #[cfg(windows)]
            let index = {
                if self.notice.as_ref().is_some_and(|alert| matches!(alert.source, AlertSource::Plugin { .. })) {
                    self.notice = None;
                }
                self.alerts.iter().position(|alert| matches!(alert.source, AlertSource::Plugin { .. })).unwrap_or(index)
            };
            self.alerts.insert(index, a);
            while self.alerts.len() > MAX_PENDING_ALERTS {
                #[cfg(windows)]
                if let Some(index) = self.alerts.iter().position(|alert| matches!(alert.source, AlertSource::Plugin { .. })) {
                    self.alerts.remove(index);
                    continue;
                }
                self.alerts.pop_front();
            }
        }
    }
    /// 清除模块尚在显示、排队或等待交给系统的提醒。跳转页码可能与其他提醒共用。
    pub(crate) fn clear_module_reminders(&mut self,page:usize) {
        let belongs=|title:&str,body:&str|reminder_module(title)==Some(page)
            || page==crate::app::MAIL_PAGE&&mail_invitation(title,body);
        let matches=|a:&Alert|matches!(a.source,AlertSource::Internal|AlertSource::MailCode(..))&&belongs(&a.title,&a.body);
        self.alerts.retain(|a|!matches(a));
        if self.notice.as_ref().is_some_and(matches){self.notice=None;}
        if self.hud_notice.as_ref().is_some_and(matches){self.hud_notice=None;}
        self.pending.retain(|cmd|!matches!(cmd,Cmd::Notify(title,body,_) if belongs(title,body)));
    }
    /// 该来源的所有账号都已查看。
    pub fn viewed_messenger(&mut self,source:crate::app::Messenger){
        self.pending_messengers.retain(|u|u.source!=source);
        self.clear_messenger_notices(source);
    }
    /// 只清已查看的账号；无法确认账号的 QQ 提醒由用户明确关闭。
    pub fn viewed_client(&mut self,source:crate::app::Messenger,pid:u32){
        self.pending_messengers.retain(|u|!u.viewed_by(source,pid));
        if pid!=0{self.clear_account_notices(source,pid);}
        if !source.adapter().keep_unidentified{self.clear_account_notices(source,0);}
        if !self.pending_messengers.iter().any(|u|u.source==source){self.clear_messenger_notices(source);}
    }
    pub fn confirm_messenger_open(&mut self,unread:crate::app::Unread,foreground:Option<(crate::app::Messenger,u32)>)->bool{
        if unread.pid==0 || foreground!=Some((unread.source,unread.pid)){return false;}
        self.viewed_client(unread.source,unread.pid);
        true
    }
    pub fn discard_unread(&mut self,unread:crate::app::Unread){
        if !self.pending_messengers.contains(&unread){return;}
        self.pending_messengers.retain(|current|*current!=unread);
        self.clear_account_notices(unread.source,unread.pid);
        if !self.pending_messengers.iter().any(|current|current.source==unread.source){self.clear_messenger_notices(unread.source);}
    }
    fn clear_account_notices(&mut self,source:crate::app::Messenger,pid:u32){
        let matches=|notice:&Alert|notice.unread.is_some_and(|u|u.source==source && u.pid==pid);
        self.alerts.retain(|notice|!matches(notice));
        if self.notice.as_ref().is_some_and(matches){self.notice=None;self.notification_bell_until=0.0;}
    }
    fn clear_messenger_notices(&mut self,source:crate::app::Messenger){
        self.alerts.retain(|a|a.messenger()!=Some(source));
        if self.notice.as_ref().is_some_and(|a|a.messenger()==Some(source)){self.notice=None;self.notification_bell_until=0.0;}
    }
    fn mark_messenger(&mut self,source:crate::app::Messenger,pid:u32,window:isize){
        if let Some(index)=self.pending_messengers.iter().position(|u|u.source==source && u.pid==pid){
            if window!=0 && self.pending_messengers[index].window!=window {
                self.pending_messengers[index].window=window;
                if source.adapter().capture==crate::messenger::Capture::Taskbar{
                    let unread=self.pending_messengers[index];
                    for alert in self.alerts.iter_mut().chain(self.notice.iter_mut()){
                        if alert.unread.is_some_and(|old|old.source==source && old.pid==pid){alert.unread=Some(unread);}
                    }
                }else{self.clear_account_notices(source,pid);}
            }
            return;
        }
        self.pending_messengers.push(crate::app::Unread{source,pid,window});
    }
    /// 横幅「查看」：跳到该来源最近一条未读的窗口。
    pub(crate) fn latest_unread(&self,source:crate::app::Messenger)->crate::app::Unread{
        self.pending_messengers.iter().rev().find(|u|u.source==source).copied().unwrap_or(crate::app::Unread::of(source))
    }
    pub fn receive_wechat_reminder(&mut self){self.receive_wechat_reminder_from(0,0);}
    /// 微信窗口闪动：`pid` 是该账号的进程，`window` 是闪动的窗口。
    pub fn receive_wechat_reminder_from(&mut self,pid:u32,window:isize){
        self.receive_messenger_reminder(crate::app::Unread{source:crate::app::Messenger::Wechat,pid,window});
    }
    pub fn receive_messenger_reminder(&mut self,unread:crate::app::Unread){
        let client=unread.source;
        if client.adapter().capture!=crate::messenger::Capture::Taskbar || !client.enabled(&self.settings){return;}
        let pending=self.pending_messengers.iter().any(|old|old.source==client && old.pid==unread.pid);
        self.mark_messenger(client,unread.pid,unread.window);
        if pending{return;}
        self.notification_bell_at=self.t;self.notification_bell_until=self.t+4.5;
        self.enqueue_alert(Alert{source:AlertSource::Messenger(client),title:client.name().into(),body:format!("有新消息 · 请到{}查看",client.name()),page:usize::MAX,at:self.t,duration:4.5,bluetooth:None,meeting:None,unread:Some(unread)},false);
    }
    pub fn set_messenger_status(&mut self,client:crate::app::Messenger,status:String){self.messenger_statuses.insert(client,status);}
    pub fn messenger_status(&self,client:crate::app::Messenger)->&str{self.messenger_statuses.get(&client).map(String::as_str).unwrap_or("")}
    pub fn receive_notification(&mut self, app:String, id:String, body:String){
        self.receive_notification_from(app,id,body,None);
    }
    pub fn receive_notification_from(&mut self,app:String,id:String,body:String,unread:Option<crate::app::Unread>){
        if !self.settings.read_notifications || id.is_empty() || !crate::messenger::NotificationFilter::for_settings(&self.settings).allows(&id){return;}
        if crate::messenger::suppresses_toast(&app,&id,crate::messenger::toast_suppression(&self.settings)){return;}
        let client=crate::app::Messenger::from_notification_id(&id).filter(|client|client.adapter().capture==crate::messenger::Capture::SystemToast);
        let id=client.map(|client|client.adapter().notification_id.to_string()).unwrap_or(id);
        // 岛仔自己发出的系统提醒会被通知中心回传；已在历史里的同一内容不再入队，避免重复显示。
        if crate::messenger::Messenger::from_notification_id(&id).is_none() && self.history.iter().any(|a|format!("{} · {}",a.title,a.body)==body){return;}
        if !self.notification_apps.observe(&id,&app) || !self.settings.notifications{return;}
        let unread=client.map(|client|unread.filter(|u|u.source==client && u.pid!=0).unwrap_or(crate::app::Unread::of(client)));
        if let Some(unread)=unread{self.mark_messenger(unread.source,unread.pid,unread.window);}
        self.notification_bell_at=self.t;
        self.notification_bell_until=self.t+4.5;
        let title=app.chars().filter(|c|!c.is_control()).take(80).collect();
        let body=if self.settings.notification_preview {
            let mut text=body.chars().filter(|c|!c.is_control()).take(300).collect::<String>();
            if let Some(code)=crate::otp::extract("",&text,&[]) {text=crate::mail_privacy::mask(&text,&code.text);}
            text
        }else{"收到新通知 · 内容已隐藏".into()};
        self.enqueue_alert(Alert{source:AlertSource::External(id),title,body,page:usize::MAX,at:self.t,duration:4.5,bluetooth:None,meeting:None,unread},false);
    }
    pub fn reveal_messenger_notice(&mut self,unread:crate::app::Unread){
        if !unread.source.adapter().dismiss_unread || !self.pending_messengers.contains(&unread){return;}
        let body=if unread.pid==0{"消息账号暂无法确认"}else{"暂时无法打开对应窗口"};
        let source=if unread.source.adapter().capture==crate::messenger::Capture::Taskbar{AlertSource::Messenger(unread.source)}else{AlertSource::External(unread.source.adapter().notification_id.into())};
        self.notice=Some(Alert{source,title:unread.source.name().into(),body:body.into(),page:usize::MAX,at:self.t,duration:4.5,bluetooth:None,meeting:None,unread:Some(unread)});
        self.notification_bell_at=self.t;self.notification_bell_until=self.t+4.5;
    }
    pub(crate) fn tick_alerts(&mut self) {
        self.pending_messengers.retain(|u|u.source.enabled(&self.settings) && (u.source.adapter().capture!=crate::messenger::Capture::SystemToast
            || self.notification_apps.items.iter().any(|(key,_,on)|key==u.source.adapter().notification_id && *on)));
        let allowed=|a:&Alert|match &a.source {
            AlertSource::Internal=>!self.fullscreen_hidden || a.health().is_none(),
            AlertSource::MailCode(..)=>!self.fullscreen_hidden,
            AlertSource::Messenger(client)=>client.enabled(&self.settings),
            AlertSource::External(id)=>self.settings.read_notifications && crate::messenger::NotificationFilter::for_settings(&self.settings).allows(id) && self.settings.notifications && !crate::messenger::suppresses_toast(&a.title,id,crate::messenger::toast_suppression(&self.settings)) && self.notification_apps.items.iter().any(|(key,_,on)|key==id && *on),
            #[cfg(windows)]
            AlertSource::Plugin { .. } => self.settings.notifications,
        };
        self.alerts.retain(&allowed);
        if self.notice.as_ref().is_some_and(|a|!allowed(a)){self.notice=None;}
        if self.hud_notice.as_ref().is_some_and(|a|!allowed(a)){self.hud_notice=None;}
        if self.fullscreen_hidden {
            self.pending.retain(|cmd|!matches!(cmd,Cmd::Notify(title,..)if HealthReminder::ALL.into_iter().any(|kind|title==kind.title())));
        }
        if !self.settings.notification_preview {
            for a in self.alerts.iter_mut().chain(self.notice.iter_mut()).chain(self.hud_notice.iter_mut()) {
                if matches!(&a.source,AlertSource::External(_)){a.body="收到新通知 · 内容已隐藏".into();}
            }
        }
        if self
            .notice
            .as_ref()
            .is_some_and(|a| self.t - a.at > a.duration)
        {
            self.notice = None;
        }
        if self.notice.is_none() {
            if let Some(mut a) = self.alerts.pop_front() {
                a.at = self.t;
                self.notice = Some(a);
            }
        }
    }
    pub(crate) fn draw_notice(&mut self, c:&Card,panel:Rect,a:&Alert,_cmds:&mut Vec<Cmd>){
        let nc=c.clipped(0.0,0.0,panel.width(),panel.height());
        #[cfg(windows)]
        if let AlertSource::Plugin { source, id } = &a.source {
            self.draw_plugin_notice_banner(&nc, a, source, id, _cmds);
            return;
        }
        if let AlertSource::MailCode(account,uid)=&a.source {
            if let Some(item)=self.mail.alert_code_item(account,*uid,self.t,self.settings.mail_code_hide_seconds) {
                let code=item.code.as_deref().unwrap_or_default();
                let width=nc.text_w(code,27.0,W::Bold);
                let size=27.0*(126.0/width).min(1.0);
                crate::paint::bell(&nc,22.0,30.0,theme::GRAY,0.0);
                nc.centered_text(135.0,16.0,&nc.fit(&item.from,11.0,W::Regular,126.0),11.0,W::Regular,theme::GRAY,H::Center);
                nc.centered_text(135.0,39.0,code,size,W::Bold,theme::GREEN,H::Center);
                let copy_id=format!("mail-code-copy:{account}:{uid}");
                if notice_button(&nc,204.0,20.0,52.0,"复制",&copy_id) {
                    let code=code.to_owned();
                    self.notice=None;
                    self.code_copied(&code);
                    _cmds.push(Cmd::Copy(code));
                }
            }
            return;
        }
        if let Some(source)=a.messenger(){
            messenger_icon(&nc,22.0,22.0,source);
            nc.circle(31.0,14.0,2.6,Color32::from_rgb(255,72,72));
            nc.centered_text(44.0,14.0,&a.title,11.0,W::Semibold,theme::WHITE,H::Left);
            nc.centered_text(44.0,30.0,&nc.fit(&a.body,10.5,W::Regular,154.0),10.5,W::Regular,theme::GRAY,H::Left);
            if notice_button(&nc,206.0,12.0,52.0,"查看","messenger-view"){_cmds.push(Cmd::ViewMessenger(a.unread.unwrap_or_else(||self.latest_unread(source))));}
            if notice_button(&nc,265.0,12.0,52.0,"关闭","messenger-dismiss"){
                if source.adapter().dismiss_unread{if let Some(unread)=a.unread{self.discard_unread(unread);}}
                self.notice=None;self.notification_bell_until=0.0;
            }
            return;
        }
        if a.source!=AlertSource::Internal {
            crate::paint::bell(&nc,310.0,22.0,theme::GRAY,crate::paint::bell_swing(self.t-a.at));
            nc.circle(314.0,17.5,2.0,Color32::from_rgb(255,72,72));
            nc.centered_text(16.0,14.0,&nc.fit(&a.title,11.0,W::Semibold,198.0),11.0,W::Semibold,theme::WHITE,H::Left);
            nc.centered_text(16.0,30.0,&nc.fit(&a.body,10.5,W::Regular,198.0),10.5,W::Regular,theme::GRAY,H::Left);
            if notice_button(&nc,226.0,12.0,60.0,"关闭","external-dismiss"){
                self.notice=None;
                self.notification_bell_until=0.0;
            }
            return;
        }
        if let Some((meeting,_))=&a.meeting {
            let secondary=Color32::from_gray(174);
            notice_icon(&nc.scaled(34.0,46.0,36.0/28.0),34.0,46.0,a);
            let now=chrono::Local::now();
            let start=chrono::DateTime::from_timestamp(meeting.start,0).unwrap().with_timezone(&chrono::Local);
            let left=meeting.start-now.timestamp();
            let countdown=if left>0 {format!("{} 分钟后",(left+59)/60)}else{"正在开始".into()};
            let countdown_width=nc.text_w(&countdown,10.5,W::Semibold)+16.0;
            let countdown_x=344.0-countdown_width;
            nc.centered_text(64.0,33.0,&start.format("%H:%M").to_string(),30.0,W::Semibold,theme::WHITE,H::Left);
            nc.fill(countdown_x,23.0,countdown_width,20.0,10.0,Color32::from_gray(28));
            nc.centered_text(countdown_x+countdown_width/2.0,33.0,&countdown,10.5,W::Semibold,theme::WHITE,H::Center);
            let overflow=(nc.text_w(&meeting.title,size::TITLE,W::Semibold)-280.0).max(0.0);
            let offset=(((self.t-a.at-1.2).max(0.0) as f32)*24.0).min(overflow);
            nc.clipped(64.0,52.0,280.0,24.0).centered_text(64.0-offset,64.0,&meeting.title,size::TITLE,W::Semibold,theme::WHITE,H::Left);
            nc.seg((16.0,84.0),(344.0,84.0),0.6,Color32::from_gray(36));
            let date=format!("{}{}",if start.date_naive()==now.date_naive(){"今天 · "}else{""},start.format("%-m月%-d日"));
            nc.centered_text(16.0,110.0,&date,11.0,W::Regular,secondary,H::Left);
            let (dismiss,dc)=crate::paint::capsule(&nc,188.0,98.0,72.0,24.0,"meeting-dismiss",Color32::from_gray(32),Color32::from_gray(48));
            dc.centered_text(224.0,110.0,"关闭",10.5,W::Semibold,theme::WHITE,H::Center);
            if dismiss.on_hover_text("关闭本次提醒，日程仍保留").clicked(){
                self.notice=None;
                return;
            }
            let (button,bc)=crate::paint::capsule(&nc,272.0,98.0,72.0,24.0,"meeting-action",Color32::from_gray(42),Color32::from_gray(56));
            bc.centered_text(308.0,110.0,"查看",10.5,W::Semibold,theme::WHITE,H::Center);
            if button.clicked() {
                self.notice=None;
                self.open_calendar(Some(meeting.clone()));
            }
            return;
        }
        if a.is_hud(){
            if a.title=="蓝牙设备" {
                let (name,status,battery)=bluetooth_notice_content(&a.body);
                let badge=if status=="已连接"{Color32::from_rgb(10,132,255)}else if let Some(level)=battery.filter(|_|status!="已断开"){crate::bluetooth_ui::level_color(level)}else{Color32::from_gray(95)};
                nc.circle(26.0,24.0,14.0,badge);
                let kind=a.bluetooth.map(|(kind,_)|kind).unwrap_or(crate::bt::DeviceKind::Other);
                device_icon(&nc,kind,17.0,15.0,18.0);
                let right=HUD_W-14.0;
                let name_end=if let Some(level)=battery {
                    let pct=format!("{level}%");
                    let pct_width=nc.text_w(&pct,10.5,W::Semibold);
                    let x=right-pct_width-22.0;
                    nc.stroke(x,20.0,15.0,8.0,2.0,1.0,Color32::from_gray(175));
                    nc.fill(x+15.5,22.5,2.0,3.0,0.8,Color32::from_gray(175));
                    let fill=11.0*level as f32/100.0;
                    if fill>0.0 {nc.fill(x+2.0,22.0,fill,4.0,1.0,crate::bluetooth_ui::level_color(level));}
                    nc.centered_text(right,24.0,&pct,10.5,W::Semibold,theme::WHITE,H::Right);
                    x-8.0
                } else {right};
                nc.centered_text(49.0,16.0,&nc.fit(name,11.0,W::Semibold,(name_end-49.0).max(20.0)),11.0,W::Semibold,theme::WHITE,H::Left);
                nc.centered_text(49.0,32.0,status,10.5,W::Regular,theme::GRAY,H::Left);
                return;
            }
            match a.title.as_str(){
                "系统音量"=>{
                    nc.poly(&[(17.0,16.0),(21.0,16.0),(26.0,12.0),(26.0,26.0),(21.0,22.0),(17.0,22.0)],theme::WHITE);
                    if self.volume_ui.is_some_and(|(_,muted)|muted){nc.seg((29.0,16.0),(34.0,22.0),1.4,theme::WHITE);nc.seg((34.0,16.0),(29.0,22.0),1.4,theme::WHITE);}
                    else{nc.arc(26.0,19.0,5.0,-0.8,0.8,1.5,theme::WHITE);nc.arc(26.0,19.0,8.0,-0.7,0.7,1.3,theme::WHITE);}
                },
                _=>notice_icon(&nc,24.0,19.0,a),
            }
            if a.title=="系统音量" {
                self.draw_volume_hud(&nc);
            } else {
                let (title,body)=if a.title=="键盘输入语言"{("输入语言",a.body.as_str())}
                    else if a.title=="已切换电池供电"{("电池供电",a.body.as_str())}
                    else if a.title=="网络连接"{a.body.split_once(" · ").unwrap_or(("网络",a.body.as_str()))}
                    else{(a.title.as_str(),a.body.as_str())};
                nc.centered_text(48.0,19.0,&nc.fit(title,11.0,W::Semibold,90.0),11.0,W::Semibold,theme::WHITE,H::Left);
                nc.centered_text(HUD_W-16.0,19.0,&nc.fit(body,10.5,W::Regular,92.0),10.5,W::Regular,if a.title=="大写锁定"&&a.body=="已开启"||a.title=="网络连接"&&a.body.contains("已连接"){theme::GREEN}else{theme::GRAY},H::Right);
            }
        } else {
            let is_sit=a.health()==Some(HealthReminder::Sit);let is_water=a.health()==Some(HealthReminder::Water);let is_focus=a.page==1;let is_power=a.title=="电量偏低";
            notice_icon(&nc,22.0,22.0,a);
            let max=if is_water||is_sit{125.0}else{195.0};
            nc.centered_text(43.0,14.0,&nc.fit(&a.title,11.0,W::Semibold,max),11.0,W::Semibold,theme::WHITE,H::Left);
            nc.centered_text(43.0,30.0,&nc.fit(&a.body,10.5,W::Regular,max),10.5,W::Regular,theme::GRAY,H::Left);
            if is_sit {
                if notice_button(&nc,181.0,12.0,62.0,"知道了","sit-confirm-read"){self.confirm_health_reminder(HealthReminder::Sit);}
                if notice_button(&nc,250.0,12.0,68.0,"起身了","sit-confirm"){self.notice=None;self.stood_up();}
            }else if is_water {
                if notice_button(&nc,181.0,12.0,62.0,"知道了","water-confirm-read"){self.confirm_health_reminder(HealthReminder::Water);}
                if notice_button(&nc,250.0,12.0,68.0,"喝一口","water-confirm"){self.notice=None;self.drink_water();}
            }else{
                let label=if is_focus{"开始"}else if is_power{"知道了"}else{"查看"};
                if notice_button(&nc,257.0,12.0,60.0,label,"notice-action"){
                    self.notice=None;if is_power{return;}if is_focus{self.pomo.running=true;}else if a.page<6{
                        self.show_page(a.page);
                        if a.page==4 {
                            let order=self.todos.display_order(&self.today);
                            if let Some(index)=order.iter().position(|&i|self.todos.items[i].text==a.body && !self.todos.items[i].done_for(&self.today)){
                                self.todo_first=index.min(order.len().saturating_sub(3));
                            }
                        }
                    }else{self.settings_tab=6;self.open_settings();}
                }
            }
            if let Some(kind)=a.health() {
                if nc.hit(0.0,0.0,175.0,44.0,"health-notice-confirm").clicked(){self.confirm_health_reminder(kind);}
            }
        }
    }
    /// v0.5.4：可交互音量条。样式参考 oyenza-studio（MIT）的 HUD 音量条：
    /// 圆角胶囊、填充缓动、拖动时加粗并出现手柄；拖动/滚轮改音量，点喇叭静音。
    pub(crate) fn draw_volume_hud(&mut self, nc: &Card) {
        let (v, muted) = self.volume_ui.unwrap_or((0, false));
        let v = v.min(100);
        // 喇叭：点击静音 / 恢复
        let icon = nc.hit(12.0, 9.0, 28.0, 22.0, "volume-mute");
        if icon.clicked() && nc.live {
            self.volume_ui = Some((v, !muted));
            self.volume_quiet_until = self.t + 0.9;
            self.pending.push(Cmd::ToggleMute);
        }
        // 滑条：拖动 / 点击定位；滚轮微调
        let track = nc.hit_drag(HUD_TRACK_X, 8.0, HUD_TRACK_W, 24.0, "volume-track");
        let mut want: Option<u32> = None;
        if track.dragged() || track.clicked() {
            if let Some(p) = track.interact_pointer_pos() {
                let x = nc.to_local(p).x;
                want = Some((((x - HUD_TRACK_X) / HUD_TRACK_W) * 100.0).clamp(0.0, 100.0).round() as u32);
            }
        } else if nc.live && track.hovered() {
            let dy = nc.ui.ctx().input(|i| i.raw_scroll_delta.y);
            if dy.abs() > 0.5 {
                want = Some(((v as f32 + (dy / 120.0) * 4.0).clamp(0.0, 100.0)).round() as u32);
            }
        }
        if let Some(nv) = want {
            if self.volume_ui.map(|(o, _)| o) != Some(nv) || muted {
                self.volume_ui = Some((nv, false));
                self.volume_quiet_until = self.t + 0.9;
                self.pending.push(Cmd::SetVolume(nv));
            }
        }
        let (v, muted) = self.volume_ui.unwrap_or((v, muted));
        let pressed = track.dragged() || track.is_pointer_button_down_on() || icon.is_pointer_button_down_on();
        let target = if muted { 0.0 } else { v as f32 / 100.0 };
        self.volume_a = crate::anim::approach(self.volume_a, target, if pressed { 34.0 } else { 13.0 }, self.dt);
        let h = if pressed { 7.0 } else { 5.0 };
        let y = 19.0 - h / 2.0;
        nc.fill(HUD_TRACK_X, y, HUD_TRACK_W, h, h / 2.0, Color32::from_white_alpha(if pressed { 46 } else { 34 }));
        if self.volume_a > 0.0 {
            let w = (HUD_TRACK_W * self.volume_a).max(h);
            let col = if muted { Color32::from_gray(150) } else { theme::WHITE };
            nc.fill(HUD_TRACK_X, y, w, h, h / 2.0, col);
            nc.fill(HUD_TRACK_X, y + 0.8, (w - 1.2).max(0.0), h * 0.4, h * 0.2, Color32::from_white_alpha(64));
            if pressed {
                nc.circle(HUD_TRACK_X + w - h / 2.0, y + h / 2.0, h * 0.95, col);
            }
        }
        let label = if muted { "静音".to_string() } else { format!("{v}%") };
        nc.centered_text(HUD_W - 26.0, 19.0, &label, 10.5, if pressed { W::Semibold } else { W::Regular }, if muted { theme::GRAY } else { theme::WHITE }, H::Center);
    }

    pub(crate) fn drink_water(&mut self) {        let hour = self.current_hour;
        let amount = self.settings.serving_ml;
        let was = self.water.percent();
        let before = self.water.drunk_ml;
        self.water.drink(amount, hour);
        let amount = self.water.drunk_ml.saturating_sub(before);
        self.last_drink = Some((amount, hour));
        self.confirm_health_reminder(HealthReminder::Water);
        self.pet.react(PetState::Satisfied);
        self.say(format!("已记录 {amount} ml"));
        if was < 100.0 && self.water.percent() >= 100.0 {
            self.alert("今日饮水目标达成", "做得不错，继续保持", 2, true);
        }
    }


}

/// 将本地提醒时间规范化，拒绝不存在的日期与超出范围的时分。
pub(crate) fn parse_due(value: &str, today: &str) -> Option<String> {
    let value = value.trim();
    let full = if value.len() == 5 {
        format!("{today} {value}")
    } else {
        value.to_string()
    };
    if !full.is_ascii() || full.len() != 16 {
        return None;
    }
    let b = full.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || b[10] != b' ' || b[13] != b':' {
        return None;
    }
    let year = full[0..4].parse::<u32>().ok()?;
    let month = full[5..7].parse::<usize>().ok()?;
    let day = full[8..10].parse::<u32>().ok()?;
    let hour = full[11..13].parse::<u32>().ok()?;
    let minute = full[14..16].parse::<u32>().ok()?;
    if !(2000..=9999).contains(&year) || !(1..=12).contains(&month) || hour > 23 || minute > 59 {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day == 0 || day > days[month - 1] {
        return None;
    }
    Some(full)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn island_size_drag_commits_only_on_release(){
        let ctx=egui::Context::default();let mut value=100;
        let pos=egui::pos2(30.0,30.0);let end=egui::pos2(110.0,30.0);
        let button=|pos,pressed|egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed,modifiers:Default::default()};
        let frames=[vec![],vec![egui::Event::PointerMoved(pos)],vec![button(pos,true)],vec![egui::Event::PointerMoved(end)],vec![button(end,false)]];
        for (i,events) in frames.into_iter().enumerate(){
            let _=ctx.run(egui::RawInput{events,screen_rect:Some(Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(320.0,80.0))),..Default::default()},|ctx|{
                egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(20)).show(ctx,|ui|{ui.spacing_mut().slider_width=180.0;island_size_slider(ui,&mut value);});
            });
            if i<4{assert_eq!(value,100);}else{assert_ne!(value,100);}
        }
    }
    #[test]
    fn number_stepper_clicks_follow_steps_and_limits() {
        for (initial,step,x,expected) in [(5u32,1,127.0,6),(5,1,13.0,4),(0,1,13.0,0),(10,1,127.0,10),(9,3,127.0,10),(1,3,13.0,0)] {
            let ctx=egui::Context::default();
            let mut value=initial;
            let pos=egui::pos2(x+20.0,34.0);
            let events=[vec![],vec![egui::Event::PointerMoved(pos)],vec![egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:true,modifiers:Default::default()}],vec![egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:false,modifiers:Default::default()}]];
            for events in events {
                let raw=egui::RawInput{screen_rect:Some(Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(200.0,80.0))),events,..Default::default()};
                let _=ctx.run(raw,|ctx|{
                    egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(20)).show(ctx,|ui|{
                        ui.horizontal(|ui|{number_stepper(ui,&mut value,0,10,"",step,76.0);});
                    });
                });
            }
            assert_eq!(value,expected,"初值 {initial}，步长 {step}，点击位置 {x}");
        }
    }

    #[test]
    fn number_stepper_fits_with_settings_button_padding() {
        let ctx=egui::Context::default();
        let raw=egui::RawInput{screen_rect:Some(Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(340.0,240.0))),..Default::default()};
        let _=ctx.run(raw,|ctx|{
            egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx,|ui|{
                ui.spacing_mut().button_padding=egui::vec2(10.0,6.0);
                for mut value in [0u32,25,180] {
                    let row=ui.horizontal(|ui|{number_stepper(ui,&mut value,0,180," 分钟",1,76.0);});
                    assert!(row.response.rect.width()<=140.1,"数值控件超出预留宽度：{}",row.response.rect.width());
                }
                for mut value in [25u32,5,15] {setting_number(ui,"计时时长",&mut value,1,180," 分钟",1.0);}
                assert!(ui.min_rect().right()<=340.1,"连续数值行不应撑宽卡片");
            });
        });
    }

    #[test]
    fn settings_groups_keep_every_page_reachable() {
        let groups = ["通用", "音乐", "专注与健康", "专注与健康", "待办与置物", "待办与置物", "消息提醒", "邮箱", "日程", "关于", "页面管理", "AI 额度", "天气", "外观", "位置", "系统性能", "无线设备"];
        for (tab, expected) in groups.iter().enumerate() {
            let matches: Vec<_> = SETTINGS_GROUPS.iter().filter(|(_,start,end,_)| (*start..=*end).contains(&tab)).collect();
            assert_eq!(matches.len(), 1, "页面 {tab} 必须且只能属于一个分组");
            assert_eq!(matches[0].0, *expected);
        }
    }

    #[test]
    fn bluetooth_notice_uses_real_connection_and_battery_data(){
        assert_eq!(bluetooth_notice_content("OpenMove by Shokz · 电量 80%"),("OpenMove by Shokz","已连接",Some(80)));
        assert_eq!(bluetooth_notice_content("耳机"),("耳机","已连接",None));
        assert_eq!(bluetooth_notice_content("已断开连接"),("蓝牙设备","已断开",None));
        assert_eq!(bluetooth_notice_content("青龙键盘 · 已断开"),("青龙键盘","已断开",None));
        assert_eq!(bluetooth_notice_content("耳机 · 电量 101%"),("耳机 · 电量 101%","已连接",None));
    }

    #[test]
    fn device_icons_render_as_visible_svg() {
        for kind in ALL_DEVICE_KINDS {
            let egui::ImageSource::Bytes{bytes,..}=device_icon_source(kind) else {panic!("{kind:?}")};
            let image=egui_extras::image::load_svg_bytes(&bytes).unwrap();
            assert!(image.pixels.iter().any(|p|p.a()>0),"{kind:?}");
        }
    }

    /// 图标要在圆环里视觉居中：按 96px 渲染后，不透明部分的外框中心偏离不超过 0.25 个图标单位。
    #[test]
    fn device_icons_are_visually_centered() {
        let mut off=vec![];
        for kind in ALL_DEVICE_KINDS {
            let egui::ImageSource::Bytes{uri,bytes}=device_icon_source(kind) else {panic!()};
            let image=egui_extras::image::load_svg_bytes_with_size(&bytes,Some(egui::load::SizeHint::Size(96,96))).unwrap();
            let [w,h]=image.size;let (mut x0,mut y0,mut x1,mut y1)=(w,h,0,0);
            for y in 0..h {for x in 0..w {if image.pixels[y*w+x].a()>40 {x0=x0.min(x);y0=y0.min(y);x1=x1.max(x);y1=y1.max(y);}}}
            let dx=(w as f32/2.0-(x0+x1+1) as f32/2.0)*24.0/w as f32;let dy=(h as f32/2.0-(y0+y1+1) as f32/2.0)*24.0/h as f32;
            println!("ICON-OFFSET {uri} {dx:.2} {dy:.2}");
            if dx.abs()>0.25||dy.abs()>0.25 {off.push(format!("{kind:?} 偏移 ({dx:.2},{dy:.2})"));}
        }
        assert!(off.is_empty(),"图标没有居中：{off:?}");
    }

    #[test]
    fn due_time_checks_calendar_and_format() {
        assert_eq!(
            parse_due("09:30", "2026-10-29").as_deref(),
            Some("2026-10-29 09:30")
        );
        assert!(parse_due("2026-02-29 12:00", "").is_none());
        assert!(parse_due("2028-02-29 12:00", "").is_some());
        for value in [
            "24:00",
            "12:60",
            "明天中午",
            "2026-13-01 10:00",
            "2026-00-01 10:00",
        ] {
            assert!(parse_due(value, "2026-10-29").is_none());
        }
    }
}

pub(crate) struct TaskDraft {index:usize,original:String,text:String,due:String,error:String,focused:bool,reminder_only:bool,pub(crate) repeat:u8,pub(crate) picker:crate::task_picker::ReminderPicker}
impl App {
    pub(crate) fn open_task_dialog(&mut self,index:usize){
        self.todo_choosing=false;
        if let Some(task)=self.todos.items.get(index){
            let draft=TaskDraft{index,original:task.text.clone(),text:task.text.clone(),due:task.due.clone().unwrap_or_default(),error:String::new(),focused:false,reminder_only:false,repeat:task.repeat,picker:crate::task_picker::ReminderPicker::new(task.due.as_deref())};
            self.show_page(4);self.task_dialog=Some(draft);
        }
    }
    pub(crate) fn open_task_reminder(&mut self,index:usize){
        self.open_task_dialog(index);
        if let Some(d)=self.task_dialog.as_mut(){d.reminder_only=true;d.picker.show();}
    }
    pub(crate) fn draw_task_editor(&mut self,c:&Card){
        if self.task_dialog.as_ref().is_some_and(|draft|draft.picker.open){self.draw_task_reminder(c);return;}
        let reminder_only=self.task_dialog.as_ref().is_some_and(|d|d.reminder_only);
        c.text(15.0,25.0,if reminder_only{"设置提醒"}else{"编辑待办"},size::TITLE,W::Bold,theme::WHITE,H::Left);
        c.text(43.0,48.0,if reminder_only{"待办"}else{"内容"},10.5,W::Regular,theme::GRAY,H::Left);
        c.fill(91.0,33.0,275.0,20.0,6.0,theme::CARD);
        if let Some(d)=self.task_dialog.as_ref().filter(|d|d.reminder_only){
            c.text(97.0,47.0,&c.fit(&d.text,11.5,W::Regular,260.0),11.5,W::Regular,theme::WHITE,H::Left);
        }
        c.text(43.0,72.0,"提醒",10.5,W::Regular,theme::GRAY,H::Left);
        let time=self.task_dialog.as_ref().map(|d|if d.due.is_empty(){"点击选择时间".into()}
            else if d.repeat!=0{format!("{} {}",crate::model::repeat_label(d.repeat),d.due.get(11..16).unwrap_or(&d.due))}else{d.due.clone()}).unwrap_or_default();
        if setting_button(c,91.0,58.0,275.0,&time,false,"task-select-reminder"){if let Some(d)=self.task_dialog.as_mut(){d.picker.show();}}
        // 每周重复：七个星期按钮，全不选即一次性待办。
        c.text(43.0,95.0,"重复",10.5,W::Regular,theme::GRAY,H::Left);
        self.task_repeat_chips(c,91.0,81.0,25.0,19.0,28.0);
        let repeat=self.task_dialog.as_ref().map(|d|d.repeat).unwrap_or(0);
        let quick=if repeat==crate::model::REPEAT_ALL{"不重复"}else{"每天"};
        if setting_button(c,289.0,81.0,77.0,quick,false,"task-repeat-all"){
            if let Some(d)=self.task_dialog.as_mut(){d.repeat=if d.repeat==crate::model::REPEAT_ALL{0}else{crate::model::REPEAT_ALL};}
        }
        if let Some(d)=&self.task_dialog {
            let summary=if d.repeat!=0{crate::model::repeat_label(d.repeat)}else{"只做一次".into()};
            let hint=if d.error.is_empty(){summary.as_str()}else{&d.error};
            c.text(43.0,117.0,&c.fit(hint,10.5,W::Regular,if reminder_only{140.0}else{154.0}),10.5,if d.error.is_empty(){W::Regular}else{W::Semibold},if d.error.is_empty(){theme::GRAY}else{theme::RED},H::Left);
        }
        if reminder_only {
            let has_reminder=self.task_dialog.as_ref().and_then(|d|self.todos.items.get(d.index)).is_some_and(|task|task.due.is_some());
            let skip=if has_reminder{"取消提醒"}else if repeat!=0{"只设重复"}else{"暂不提醒"};
            if setting_button(c,188.0,104.0,88.0,skip,false,"task-reminder-skip"){
                if let Some(d)=self.task_dialog.as_mut(){d.due.clear();}
                self.save_task_editor();
            }
        }else if setting_button(c,202.0,104.0,74.0,"取消",false,"task-cancel"){self.task_dialog=None;}
        if setting_button(c,286.0,104.0,80.0,if reminder_only{"选择时间"}else{"保存"},true,"task-save"){
            if reminder_only{if let Some(d)=self.task_dialog.as_mut(){d.picker.show();}}else{self.save_task_editor();}
        }
    }
    pub(crate) fn task_editor_inputs(&mut self,ui:&mut egui::Ui,origin:egui::Pos2,cmds:&mut Vec<Cmd>){
        let Some(d)=self.task_dialog.as_mut() else{return};
        if d.reminder_only || d.picker.open{return;}
        let name=ui.put(Rect::from_min_size(origin+egui::vec2(97.0,35.0),egui::vec2(260.0,17.0)),egui::TextEdit::singleline(&mut d.text).vertical_align(egui::Align::Center).id(egui::Id::new("task-name-edit")).frame(false).font(theme::font(11.5,W::Regular)));
        let first_focus=!d.focused;
        if first_focus{name.request_focus();d.focused=true;cmds.push(Cmd::Focus);}
        // 新建内容与下一步共用这一帧，避免同一次回车又提交了空提醒。
        if !first_focus && ui.input(|i|i.key_pressed(egui::Key::Enter)){self.save_task_editor();}
    }
    /// 七个星期按钮（一…日），点一下切换；全不选即一次性待办。
    fn task_repeat_chips(&mut self,c:&Card,x:f32,y:f32,w:f32,h:f32,step:f32){
        let repeat=self.task_dialog.as_ref().map(|d|d.repeat).unwrap_or(0);
        for (day,name) in crate::model::WEEKDAY_NAMES.iter().enumerate() {
            let on=repeat&(1<<day)!=0;
            let cx=x+day as f32*step;
            let (r,b)=crate::paint::capsule(c,cx,y,w,h,&format!("task-repeat-{day}"),if on{Color32::from_rgb(39,94,150)}else{theme::BTN},theme::BTN_HOVER);
            b.centered_text(cx+w/2.0,y+h/2.0,name,if h<18.0{9.5}else{10.5},W::Semibold,if on{theme::WHITE}else{theme::GRAY},H::Center);
            if r.on_hover_text(format!("每周{name}")).clicked(){
                if let Some(d)=self.task_dialog.as_mut(){d.repeat^=1<<day;d.error.clear();}
            }
        }
    }
    fn draw_task_reminder(&mut self,c:&Card){
        use crate::task_picker::PickerAction;
        let Some(d)=self.task_dialog.as_mut().filter(|d|d.picker.open) else{return};
        let has_reminder=self.todos.items.get(d.index).is_some_and(|task|task.due.is_some());
        c.text(15.0,25.0,"待办",size::TITLE,W::Bold,theme::WHITE,H::Left);
        c.text(44.0,25.0,"清单",size::TITLE,W::Bold,theme::GREEN,H::Left);
        let (done,total)=self.todos.progress(&self.today);
        c.text(42.0,61.0,&format!("{done}/{total}"),29.0,W::Bold,theme::WHITE,H::Left);
        // 左侧：每周重复。选了星期几后，提醒只按时间、在这些日子到点提醒。
        d.picker.repeat_note=Some(if d.repeat!=0{format!("{} · 到点提醒",crate::model::repeat_label(d.repeat))}else{String::new()});
        let action=d.picker.draw(c,&d.text,has_reminder);
        let focus=std::mem::take(&mut d.picker.focus);
        self.task_repeat_chips(c,41.0,84.0,17.0,16.0,19.5);
        let Some(d)=self.task_dialog.as_mut() else{return};
        if focus{self.pending.push(Cmd::Focus);}
        match action{
            Some(PickerAction::Save)=>{d.due=d.picker.due();d.picker.open=false;if d.reminder_only{self.save_task_editor();}},
            Some(PickerAction::Skip)=>{d.due.clear();d.picker.open=false;if d.reminder_only{self.save_task_editor();}},
            Some(PickerAction::Cancel)=>{d.picker.open=false;if d.reminder_only{self.task_dialog=None;}},
            None=>{},
        }
    }
    fn save_task_editor(&mut self){
        let Some(mut d)=self.task_dialog.take() else{return};
        let due=if d.due.trim().is_empty(){Some(None)}else{parse_due(&d.due,&self.today).map(Some)};
        if d.text.trim().is_empty(){d.error="请填写任务内容".into();}
        else if due.is_none(){d.error="时间格式应为 18:30 或完整日期时间".into();}
        else if self.todos.items.get(d.index).is_none_or(|t|t.text!=d.original){d.error="任务已变化，请取消后重新打开".into();}
        else{
            let item=&mut self.todos.items[d.index];
            if item.repeat!=d.repeat{item.done_on=None;}
            item.text=d.text.trim().into();item.due=due.unwrap();item.reminded=false;item.reminded_on=None;item.repeat=d.repeat;
            self.todos.save();self.say("待办已保存");return;
        }
        self.task_dialog=Some(d);
    }
}

fn notice_button(c:&Card,x:f32,y:f32,w:f32,text:&str,id:&str)->bool {
    let Some(c)=c.edge_control(x,y,w,20.0) else {return false};
    let (r,b)=crate::paint::capsule(&c,x,y,w,20.0,id,Color32::from_gray(35),theme::BTN_HOVER);
    b.centered_text(x+w/2.0,y+10.0,text,10.5,W::Semibold,theme::WHITE,H::Center);
    r.clicked()
}
fn setting_button(c:&Card,x:f32,y:f32,w:f32,text:&str,on:bool,id:&str)->bool {
    let (r,b)=crate::paint::capsule(c,x,y,w,19.0,id,if on{Color32::from_rgb(39,94,150)}else{theme::BTN},theme::BTN_HOVER);
    b.text(x+w/2.0,y+13.3,text,10.5,W::Semibold,theme::WHITE,H::Center);
    r.clicked()
}
fn hhmm(m:u32)->String{format!("{:02}:{:02}",m/60%24,m%60)}
/// 岛内小开关（与健康页右上开关同款）。
fn island_switch(c:&Card,x:f32,y:f32,on:bool,id:&str)->bool{
    let r=c.hit(x-2.0,y-2.0,38.0,24.0,id);
    let (hv,_)=c.hover_t(&r);
    let track=if on{theme::SIT_GREEN}else{Color32::from_rgb(58,58,62)};
    c.fill(x,y,34.0,20.0,10.0,crate::paint::mix(track,theme::WHITE,hv*0.08));
    c.circle(if on{x+24.0}else{x+10.0},y+10.0,8.0,theme::WHITE);
    r.clicked()
}
impl App {
    fn health_time_summary(&self)->String{
        let s=&self.settings;
        let hours=format!("{:02}:00–{:02}:00",s.water_start,s.water_end);
        if s.lunch_rest{format!("{hours} · 午休 {}–{}",hhmm(s.lunch_start),hhmm(s.lunch_end))}else{hours}
    }
    /// 「提醒时段」二级页：提醒时段起止、午休开关、午休起止（15 分钟一档），喝水和久坐共用。
    fn health_time_page(&mut self,c:&Card){
        let row_y=|i:usize|38.0+i as f32*22.0;
        for i in 1..4{c.fill(44.0,row_y(i)-2.0,326.0,1.0,0.5,Color32::from_rgb(28,28,31));}
        let y=row_y(0);
        c.text(44.0,y+13.0,"提醒时段",10.5,W::Regular,theme::GRAY,H::Left);
        c.text(250.0,y+13.0,"至",10.5,W::Regular,theme::GRAY,H::Center);
        for i in 0..2{
            let hour=if i==0{&mut self.settings.water_start}else{&mut self.settings.water_end};
            let value=egui::DragValue::new(hour).range(0..=if i==0{23}else{24})
                .custom_formatter(|v,_|format!("{:02}:00",v as u32))
                .custom_parser(|t|{let t=t.trim();t.strip_suffix(":00").or_else(||t.strip_suffix("：00")).unwrap_or(t).parse::<u32>().ok().map(|v|v as f64)});
            let (d,field)=stepper(c,if i==0{238.0}else{370.0},y,value,&format!("hour-{i}"));
            if field.gained_focus(){self.pending.push(Cmd::Focus);}self.settings_editing|=field.has_focus();
            if d!=0{let hour=if i==0{&mut self.settings.water_start}else{&mut self.settings.water_end};*hour=(*hour as i32+d).rem_euclid(24) as u32;}
        }
        let y=row_y(1);
        c.text(44.0,y+13.0,"午休不提醒",10.5,W::Regular,theme::GRAY,H::Left);
        if island_switch(c,336.0,y-0.5,self.settings.lunch_rest,"lunch-rest"){self.settings.lunch_rest=!self.settings.lunch_rest;}
        let y=row_y(2);
        let on=self.settings.lunch_rest;
        let lc=c.faded(if on{1.0}else{0.35});let lc=if on{lc}else{lc.ghost()};
        lc.text(44.0,y+13.0,"午休时间",10.5,W::Regular,theme::GRAY,H::Left);
        lc.text(250.0,y+13.0,"至",10.5,W::Regular,theme::GRAY,H::Center);
        for i in 0..2{
            let minute=if i==0{&mut self.settings.lunch_start}else{&mut self.settings.lunch_end};
            let value=egui::DragValue::new(minute).range(0..=1439).speed(1.0).custom_formatter(|v,_|hhmm(v as u32))
                .custom_parser(|t|{let (h,m)=t.trim().split_once([':','：'])?;Some((h.parse::<u32>().ok()?.min(23)*60+m.parse::<u32>().ok()?.min(59)) as f64)});
            let (d,field)=stepper(&lc,if i==0{238.0}else{370.0},y,value,&format!("lunch-{i}"));
            if field.gained_focus(){self.pending.push(Cmd::Focus);}self.settings_editing|=field.has_focus();
            if d!=0{let minute=if i==0{&mut self.settings.lunch_start}else{&mut self.settings.lunch_end};*minute=(*minute as i32+d*15).rem_euclid(1440) as u32;}
        }
        c.text(44.0,row_y(3)+13.0,"喝水和久坐提醒都只在提醒时段内、午休以外弹出",10.5,W::Regular,theme::GRAY,H::Left);
    }
}
fn island_number_field(c:&Card,rect:Rect,value:egui::DragValue<'_>,id:impl std::hash::Hash)->egui::Response {
    let mut ui=egui::Ui::new(c.ui.ctx().clone(),egui::Id::new(id),egui::UiBuilder::new().layer_id(c.ui.layer_id()).max_rect(rect).style(c.ui.style().clone()));
    ui.set_clip_rect(c.painter.clip_rect());ui.set_opacity(c.painter.opacity());if !c.live{ui.disable();}
    ui.spacing_mut().interact_size=rect.size();ui.spacing_mut().button_padding=egui::vec2(2.0,1.0);
    ui.style_mut().drag_value_text_style=egui::TextStyle::Body;
    ui.style_mut().text_styles.insert(egui::TextStyle::Body,c.font(11.0,W::Bold));
    ui.style_mut().explanation_tooltips=false;
    ui.visuals_mut().widgets.inactive.weak_bg_fill=Color32::TRANSPARENT;
    crate::quota_ui::bar_tooltip(ui.put(rect,value.update_while_editing(false)),"点击输入数值".into())
}
/// 加减器：[−] 可输入数值 [+]，右缘对齐到 right。
fn stepper(c:&Card,right:f32,y:f32,value:egui::DragValue<'_>,id:&str)->(i32,egui::Response) {
    let (w,vw)=(25.0,58.0);
    let x=right-w*2.0-vw;
    let field=island_number_field(c,c.rect(x+w,y,vw,19.0),value,("health-number",id));
    let delta=if setting_button(c,x,y,w,"−",false,&format!("{id}-minus")){-1}
        else if setting_button(c,right-w,y,w,"+",false,&format!("{id}-plus")){1}else{0};
    (delta,field)
}
impl App {
    pub(crate) fn page_settings(&mut self,c:&Card) {
        let help=match self.settings_category{0=>"外观与系统",1=>"调整喝水、久坐与提醒时段",2=>"点击分钟数可直接输入，也可用加减微调",_=>"通知"};
        if self.settings_category!=1{c.text(44.0,44.0,help,10.5,W::Regular,theme::GRAY,H::Left);}
        let before=format!("{:?}",self.settings);
        self.settings_editing=false;
        match self.settings_category {
            0=>{
                let choices=[("吉祥物",self.settings.pet),("极简刘海",self.settings.minimal),("系统提示",self.settings.system_hud),("全屏隐藏",self.settings.hide_fullscreen)];
                for (i,(name,on)) in choices.iter().enumerate(){let x=42.0+(i%2) as f32*169.0;let y=57.0+(i/2) as f32*27.0;let text=format!("{}  {}",name,if *on{"开"}else{"关"});if setting_button(c,x,y,158.0,&text,*on,&format!("general-{i}")){match i{0=>self.set_pet(!self.settings.pet),1=>self.set_minimal(!self.settings.minimal),2=>self.settings.system_hud=!self.settings.system_hud,_=>self.settings.hide_fullscreen=!self.settings.hide_fullscreen}}}
                c.text(42.0,116.0,"岛仔收起时常驻；极简模式保留细线",10.5,W::Regular,theme::GRAY,H::Left);
            }
            1 if self.health_time=>self.health_time_page(c),
            1=>{
                // 与健康页同一套切换（标题旁的水滴 / 椅子）；四行一列，标签靠左、加减器靠右对齐。
                // 喝水三行、久坐一行加两句说明；最后一行「提醒时段」两边共用，位置不变。
                let sv=self.sit_view_t.pos.clamp(0.0,1.0);
                let row_y=|i:usize|38.0+i as f32*22.0;
                for i in 1..4{c.fill(44.0,row_y(i)-2.0,326.0,1.0,0.5,Color32::from_rgb(28,28,31));}
                if sv<0.99{
                    let wc=c.faded(1.0-sv);let wc=if self.sit_mode{wc.ghost()}else{wc};
                    for (i,label) in ["每日目标","每次记录","提醒间隔"].iter().enumerate(){
                        wc.text(44.0,row_y(i)+13.0,label,10.5,W::Regular,theme::GRAY,H::Left);
                        let goal=self.water.goal_ml;let mut liters=goal as f32/1000.0;
                        let value=match i {
                            0=>egui::DragValue::new(&mut liters).range(0.5..=6.0).speed(0.25).suffix(" L").min_decimals(1).max_decimals(2),
                            1=>egui::DragValue::new(&mut self.settings.serving_ml).range(50..=1000).speed(50.0).suffix(" ml"),
                            _=>egui::DragValue::new(&mut self.settings.water_minutes).range(5..=180).speed(5.0).suffix(" 分钟"),
                        };
                        let (d,field)=stepper(&wc,370.0,row_y(i),value,&format!("water-{i}"));
                        if field.gained_focus(){self.pending.push(Cmd::Focus);}self.settings_editing|=field.has_focus();
                        match i {
                            0=>{self.water.goal_ml=((liters*1000.0).round() as i32+d*250).clamp(500,6000) as u32;if self.water.goal_ml!=goal{self.water.save();}}
                            1=>self.settings.serving_ml=(self.settings.serving_ml as i32+d*50).clamp(50,1000) as u32,
                            _=>self.settings.water_minutes=(self.settings.water_minutes as i32+d*5).clamp(5,180) as u32,
                        }
                    }
                }
                if sv>0.01{
                    let sc=c.faded(sv);let sc=if self.sit_mode{sc}else{sc.ghost()};
                    sc.text(44.0,row_y(0)+13.0,"坐满多久提醒",10.5,W::Regular,theme::GRAY,H::Left);
                    let value=egui::DragValue::new(&mut self.settings.sit_minutes).range(15..=180).speed(5.0).suffix(" 分钟");
                    let (d,field)=stepper(&sc,370.0,row_y(0),value,"sit");
                    if field.gained_focus(){self.pending.push(Cmd::Focus);}self.settings_editing|=field.has_focus();
                    if d!=0{self.settings.sit_minutes=(self.settings.sit_minutes as i32+d*5).clamp(15,180) as u32;}
                    sc.text(44.0,row_y(1)+13.0,"专注计时进行中不打扰，专注结束后再提醒",10.5,W::Regular,theme::GRAY,H::Left);
                    sc.text(44.0,row_y(2)+13.0,"提醒后常驻，点击确认或起身后重新计时",10.5,W::Regular,theme::GRAY,H::Left);
                }
                // 最后一行两边共用：提醒时段 + 午休的摘要，点进二级页修改。
                let y=row_y(3);
                let r=c.hit(40.0,y-2.0,334.0,22.0,"health-time").on_hover_text("设置提醒时段和午休免打扰");
                let (hv,_)=c.hover_t(&r);
                if hv>0.0{c.fill(40.0,y-2.0,334.0,22.0,8.0,Color32::from_white_alpha((hv*14.0) as u8));}
                c.text(44.0,y+13.0,"提醒时段",10.5,W::Regular,theme::GRAY,H::Left);
                c.text(356.0,y+13.0,&self.health_time_summary(),10.5,W::Semibold,theme::WHITE,H::Right);
                c.text(366.0,y+13.5,"›",14.0,W::Bold,theme::GRAY,H::Center);
                if r.clicked(){self.health_time=true;}
            }
            2=>{
                for (i,name) in ["专注","休息"].iter().enumerate(){
                    let y=53.0+i as f32*23.0;
                    c.text(48.0,y+13.0,name,10.5,W::Regular,theme::GRAY,H::Left);
                    let value=if i==0{&mut self.settings.focus_minutes}else{&mut self.settings.rest_minutes};
                    let max=if i==0{180}else{60};let rect=c.rect(168.0,y,76.0,19.0);
                    let field=island_number_field(c,rect,egui::DragValue::new(value).range(1..=max).suffix(" 分钟"),("focus-duration",i));
                    if field.gained_focus(){self.pending.push(Cmd::Focus);}
                    self.settings_editing|=field.has_focus();
                    if setting_button(c,131.0,y,27.0,"−",false,&format!("focus-minus-{i}")){*value=value.saturating_sub(1).max(1);}
                    if setting_button(c,255.0,y,27.0,"+",false,&format!("focus-plus-{i}")){*value=(*value+1).min(max);}
                }
                c.text(315.0,93.0,"下次生效",10.5,W::Regular,theme::GRAY,H::Center);
            }
            _=>{
                if setting_button(c,43.0,56.0,152.0,if self.settings.notifications{"提醒 · 开"}else{"提醒 · 关"},self.settings.notifications,"set-notice"){self.settings.notifications=!self.settings.notifications;}
                if setting_button(c,212.0,56.0,153.0,if self.settings.system_notify && self.settings.sound{"声音 · 开"}else{"声音 · 关"},self.settings.system_notify && self.settings.sound,"set-sound"){
                    self.settings.sound=!(self.settings.system_notify && self.settings.sound);
                    if self.settings.sound {self.settings.system_notify=true;}
                }
                if setting_button(c,43.0,84.0,152.0,if self.settings.read_notifications{"断开系统通知"}else{"连接系统通知"},self.settings.read_notifications,"set-listener"){self.pending.push(if self.settings.read_notifications{Cmd::DisconnectNotifications}else{Cmd::RequestNotifications});}
                let status=c.fit(&self.listener_status,10.5,W::Regular,170.0);c.text(210.0,97.0,&status,10.5,W::Regular,theme::GRAY,H::Left);
            }
        }
        if before!=format!("{:?}",self.settings){self.settings.save();}
    }
}

fn switch(ui:&mut egui::Ui,value:&mut bool)->bool {
    let (rect,response)=ui.allocate_exact_size(egui::vec2(34.0,20.0),egui::Sense::click());
    if response.clicked(){*value=!*value;}
    let fill=if *value{Color32::from_rgb(48,184,92)}else{settings_color(&ui.ctx().clone(),Color32::from_rgb(82,82,87),Color32::from_rgb(203,207,214))};
    ui.painter().rect_filled(rect,10.0,fill);
    let x=if *value{rect.right()-10.0}else{rect.left()+10.0};
    ui.painter().circle_filled(egui::pos2(x,rect.center().y+0.5),7.5,Color32::from_black_alpha(35));
    ui.painter().circle_filled(egui::pos2(x,rect.center().y),7.0,Color32::WHITE);
    response.clicked()
}
/// 设置页版式：标签列宽、开关尺寸、控件与卡片右边缘之间的固定留白。
pub(crate) const SETTINGS_LABEL_W: f32 = 80.0;
pub(crate) const SETTINGS_SWITCH_W: f32 = 34.0;
const SETTINGS_ROW_GAP: f32 = 16.0;

fn setting_row_text(ui:&mut egui::Ui,title:&str,description:&str,width:f32){
    let ctx=&ui.ctx().clone();
    ui.vertical(|ui|{
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y=2.0;
        ui.add(egui::Label::new(egui::RichText::new(title).size(13.0).color(settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41)))).wrap());
        if !description.is_empty(){ui.add(egui::Label::new(egui::RichText::new(description).size(11.5).color(settings_color(ctx,Color32::from_gray(185),Color32::from_gray(82)))).wrap());}
    });
}

/// 开关行：左侧标题 + 说明（宽度固定、自动换行），右侧开关，长说明也不会把开关挤出卡片。
pub(crate) fn setting_row(ui:&mut egui::Ui,title:&str,description:&str,value:&mut bool){
    ui.add_space(2.0);
    ui.horizontal(|ui|{
        let text_w=(ui.available_width()-SETTINGS_SWITCH_W-SETTINGS_ROW_GAP).max(120.0);
        setting_row_text(ui,title,description,text_w);
        // add_space 之后还会再插入一次 item_spacing.x，这里一并扣掉，开关才不会每行往外挤一点。
        ui.add_space((ui.available_width()-SETTINGS_SWITCH_W-ui.spacing().item_spacing.x).max(0.0));
        switch(ui,value);
    });
    ui.add_space(2.0);
}

/// 模块关闭时禁用常驻开关，圆点仍反映保存的偏好。
fn resident_setting_row(ui:&mut egui::Ui,title:&str,description:&str,module_on:bool,value:&mut bool){
    ui.add_enabled_ui(module_on,|ui|{
        setting_row(ui,title,if module_on {description}else{"功能页已关闭"},value);
    });
}

pub(crate) const PAGE_VISIBILITY_ACTION_WIDTH: f32 = 92.0;

pub(crate) fn page_visibility_action(ui:&mut egui::Ui,on:bool,hidden:bool,can_disable:bool)->bool{
    let label=if hidden {"显示入口"} else if on {"关闭并隐藏"} else {"隐藏入口"};
    let response=ui.add_enabled_ui(can_disable,|ui|{
        ui.add_sized([PAGE_VISIBILITY_ACTION_WIDTH,26.0],egui::Button::new(egui::RichText::new(label).size(12.0)).frame(false))
    }).inner.on_disabled_hover_text("至少保留一个页面");
    if response.clicked() {ui.ctx().request_repaint();}
    response.clicked()
}

/// 设置页统一的分段选择器（灰色底框 + 选中块）。返回值为是否发生了切换。
pub(crate) fn settings_segmented<T:PartialEq+Copy>(ui:&mut egui::Ui,value:&mut T,options:&[(T,&str)],item_w:f32)->bool{
    let ctx=&ui.ctx().clone();
    let mut changed=false;
    egui::Frame::new().fill(settings_color(ctx,Color32::from_rgb(28,28,30),Color32::from_rgb(230,233,238)))
        .corner_radius(8.0).inner_margin(3).show(ui,|ui|{
            ui.horizontal_wrapped(|ui|{
                ui.spacing_mut().item_spacing.x=2.0;
                for (option,label) in options {
                    let selected=*value==*option;
                    let text=egui::RichText::new(*label).size(12.0).color(if selected{settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41))}else{settings_color(ctx,Color32::from_gray(200),Color32::from_gray(70))});
                    let button=egui::Button::new(text)
                        .fill(if selected{settings_color(ctx,Color32::from_rgb(72,72,76),Color32::WHITE)}else{Color32::TRANSPARENT})
                        .stroke(egui::Stroke::NONE).corner_radius(6.0);
                    let width=item_w.max(ctx.fonts(|f|f.layout_no_wrap(label.to_string(),egui::FontId::proportional(12.0),Color32::WHITE).size().x)+20.0);
                    // 选中项浮起一点：浅色模式白底加淡阴影，深色模式更亮的底色加阴影。
                    let clicked=if selected {
                        egui::Frame::new().corner_radius(6.0).shadow(egui::Shadow{offset:[0,1],blur:4,spread:0,color:Color32::from_black_alpha(if ui.visuals().dark_mode{90}else{38})})
                            .show(ui,|ui|ui.add_sized([width,26.0],button)).inner.clicked()
                    } else {ui.add_sized([width,26.0],button).clicked()};
                    if clicked && !selected {*value=*option;changed=true;}
                }
            });
        });
    changed
}

/// 选项行：标签列之后紧跟控件，同一卡片里所有控件左缘对齐成一条竖线。
pub(crate) fn settings_control_row_right(ui:&mut egui::Ui,title:&str,controls:impl FnOnce(&mut egui::Ui))->egui::Response{
    settings_control_row(ui,title,SETTINGS_LABEL_W,|ui|ui.horizontal(|ui|controls(ui)).response)
}

/// 设置项下方的说明文字。
fn island_size_slider(ui:&mut egui::Ui,value:&mut u32)->egui::Response {
    let id=ui.make_persistent_id("island-size-draft");
    let mut draft=ui.data(|d|d.get_temp::<u32>(id)).unwrap_or(*value);
    let response=ui.add(egui::Slider::new(&mut draft,crate::model::ISLAND_SIZE_MIN..=crate::model::ISLAND_SIZE_MAX)
        .step_by(crate::model::ISLAND_SIZE_STEP as f64).suffix("%"));
    if ui.input(|i|i.pointer.primary_down()) {
        if response.changed(){ui.data_mut(|d|d.insert_temp(id,draft));}
    }else{
        *value=crate::model::island_size_value(draft);
        ui.data_mut(|d|d.remove::<u32>(id));
    }
    response
}

pub(crate) fn settings_hint(ui:&mut egui::Ui,text:&str){
    let ctx=&ui.ctx().clone();
    ui.add(egui::Label::new(egui::RichText::new(text).size(11.5).color(settings_color(ctx,Color32::from_gray(170),Color32::from_gray(92)))).wrap());
}
pub(crate) fn settings_card(ui:&mut egui::Ui,add:impl FnOnce(&mut egui::Ui)) {
    let ctx=&ui.ctx().clone();
    egui::Frame::new().fill(settings_color(ctx,Color32::from_rgb(35,35,38),Color32::from_rgb(255,255,255)))
        .corner_radius(10.0).inner_margin(egui::Margin::symmetric(16,12))
        .show(ui,|ui|{ui.set_min_width(ui.available_width());ui.spacing_mut().item_spacing.y=8.0;add(ui);});
}

/// 卡片底部的「高级」折叠区：只放内容多的页面里不常改的项，默认收起。整条都可以点，右侧箭头表示展开状态。
pub(crate) fn settings_advanced(ui:&mut egui::Ui,id:&str,add:impl FnOnce(&mut egui::Ui)){
    let ctx=&ui.ctx().clone();
    let state=egui::Id::new(("settings-advanced",id));
    let mut open=ctx.data(|d|d.get_temp::<bool>(state)).unwrap_or(false);
    ui.add_space(2.0);
    let (rect,response)=ui.allocate_exact_size(egui::vec2(ui.available_width(),34.0),egui::Sense::click());
    if response.hovered(){ui.painter().rect_filled(rect.expand2(egui::vec2(6.0,0.0)),7.0,settings_color(ctx,Color32::from_rgb(44,44,48),Color32::from_rgb(242,244,247)));}
    let fg=settings_color(ctx,Color32::from_gray(205),Color32::from_gray(60));
    ui.painter().text(egui::pos2(rect.left(),rect.center().y),egui::Align2::LEFT_CENTER,"高级",egui::FontId::proportional(13.0),fg);
    // 右侧箭头：收起朝右，展开朝下。
    let c=egui::pos2(rect.right()-10.0,rect.center().y);
    let pts=if open{vec![c+egui::vec2(-4.5,-2.0),c+egui::vec2(0.0,2.5),c+egui::vec2(4.5,-2.0)]}else{vec![c+egui::vec2(-2.0,-4.5),c+egui::vec2(2.5,0.0),c+egui::vec2(-2.0,4.5)]};
    ui.painter().add(egui::Shape::line(pts,egui::Stroke::new(1.6,fg)));
    if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked(){open=!open;ctx.data_mut(|d|d.insert_temp(state,open));}
    if open {
        ui.spacing_mut().item_spacing.y=8.0;
        add(ui);
    }
}
/// 状态小标签：「已开启」绿色，「已关闭」灰色；固定高度的胶囊，不随行高拉伸。
fn settings_state_chip(ui:&mut egui::Ui,on:bool){
    let ctx=&ui.ctx().clone();
    let (text,fg,bg)=if on{("已开启",settings_color(ctx,Color32::from_rgb(127,214,155),Color32::from_rgb(29,122,60)),settings_color(ctx,Color32::from_rgb(28,51,36),Color32::from_rgb(227,244,232)))}
        else{("已关闭",settings_color(ctx,Color32::from_gray(160),Color32::from_gray(105)),settings_color(ctx,Color32::from_rgb(44,44,48),Color32::from_rgb(232,235,240)))};
    let galley=ui.painter().layout_no_wrap(text.into(),egui::FontId::proportional(11.5),fg);
    let (rect,_)=ui.allocate_exact_size(egui::vec2(galley.size().x+16.0,20.0),egui::Sense::hover());
    ui.painter().rect_filled(rect,10.0,bg);
    ui.painter().galley(rect.center()-galley.size()/2.0,galley,fg);
}
/// 设置页统一的控件样式（按钮、输入框、选中色）；设置页正文和弹窗共用。
pub(crate) fn settings_apply_style(ui:&mut egui::Ui){
    let ctx=&ui.ctx().clone();
    // 分割线：浅色模式用很淡的灰，深色模式略亮于卡片，不再是系统默认的粗灰线。
    ui.visuals_mut().widgets.noninteractive.bg_stroke=egui::Stroke::new(1.0,settings_color(ctx,Color32::from_rgb(52,52,57),Color32::from_rgb(233,235,240)));
    ui.spacing_mut().item_spacing.y=ui.spacing().item_spacing.y.max(5.0);
    ui.spacing_mut().item_spacing=egui::vec2(8.0,5.0);
    ui.spacing_mut().button_padding=egui::vec2(10.0,6.0);
    ui.style_mut().text_styles.insert(egui::TextStyle::Small,egui::FontId::proportional(12.0));
    ui.visuals_mut().override_text_color=Some(settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41)));
    ui.visuals_mut().extreme_bg_color=settings_color(ctx,Color32::from_rgb(25,25,28),Color32::from_rgb(244,246,249));
    ui.visuals_mut().widgets.inactive.bg_fill=settings_color(ctx,Color32::from_rgb(50,50,54),Color32::from_rgb(228,231,236));
    ui.visuals_mut().widgets.hovered.bg_fill=settings_color(ctx,Color32::from_rgb(65,65,71),Color32::from_rgb(216,222,231));
    // 统一按钮：高度 28、圆角 7、内边距 12×5，无描边；默认按钮也套用这套样式。
    ui.spacing_mut().button_padding=egui::vec2(12.0,5.0);
    ui.spacing_mut().interact_size.y=28.0;
    {
        let fill=settings_color(ctx,Color32::from_rgb(50,50,54),Color32::from_rgb(228,231,236));
        let hover=settings_color(ctx,Color32::from_rgb(65,65,71),Color32::from_rgb(216,222,231));
        let widgets=&mut ui.visuals_mut().widgets;
        for (w,f) in [(&mut widgets.inactive,fill),(&mut widgets.hovered,hover),(&mut widgets.active,hover)] {
            w.corner_radius=egui::CornerRadius::same(7);
            w.weak_bg_fill=f;
            w.bg_stroke=egui::Stroke::NONE;
        }
    }
    ui.visuals_mut().selection.bg_fill=settings_color(ctx,Color32::from_rgb(45,105,165),Color32::from_rgb(209,226,248));
}
/// 设置页弹窗的外框：卡片底色、大圆角、细描边和阴影。
pub(crate) fn settings_modal_frame(ctx:&egui::Context)->egui::Frame{
    egui::Frame::new().fill(settings_color(ctx,Color32::from_rgb(36,36,40),Color32::WHITE)).corner_radius(14.0).inner_margin(egui::Margin::same(20))
        .stroke(egui::Stroke::new(1.0,settings_color(ctx,Color32::from_rgb(58,58,64),Color32::from_rgb(222,225,231))))
        .shadow(egui::Shadow{offset:[0,8],blur:28,spread:0,color:Color32::from_black_alpha(90)})
}
/// 弹窗标题栏：标题在左、关闭在右，下面一条分隔线。返回是否点了关闭。
pub(crate) fn settings_modal_title(ui:&mut egui::Ui,title:&str)->bool{
    let ctx=&ui.ctx().clone();
    let mut close=false;
    ui.horizontal(|ui|{
        ui.label(egui::RichText::new(title).size(16.0).strong().color(settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41))));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
            close=ui.add(egui::Button::new(egui::RichText::new("×").size(18.0)).frame(false)).on_hover_text("关闭").clicked();
        });
    });
    ui.add_space(4.0);settings_separator(ui);ui.add_space(6.0);
    close
}
/// 设置页分割线（样式由 settings_apply_style 统一设定）。
fn settings_separator(ui:&mut egui::Ui){ui.separator();}
/// 可点选的标签：开启时绿色底，关闭时描边灰字。
fn settings_toggle_chip(ui:&mut egui::Ui,text:&str,on:bool)->egui::Response{
    let ctx=&ui.ctx().clone();
    let label=text.to_string();
    let fg=if on{settings_color(ctx,Color32::from_rgb(127,214,155),Color32::from_rgb(29,122,60))}else{settings_color(ctx,Color32::from_gray(185),Color32::from_gray(80))};
    let fill=if on{settings_color(ctx,Color32::from_rgb(28,51,36),Color32::from_rgb(227,244,232))}else{Color32::TRANSPARENT};
    let stroke=if on{egui::Stroke::NONE}else{egui::Stroke::new(1.0,settings_color(ctx,Color32::from_rgb(60,60,66),Color32::from_rgb(214,218,225)))};
    ui.add(egui::Button::new(egui::RichText::new(label).size(12.0).color(fg)).fill(fill).stroke(stroke).corner_radius(13.0).min_size(egui::vec2(0.0,26.0)))
}
/// 功能页面对应的图标，与侧栏同一套。
/// 纹理缓存只按地址区分、不分尺寸：侧栏 23px 先加载后，36px 的卡片图标会被放大发糊，所以这里用独立地址。
fn settings_page_icon(page:usize)->egui::ImageSource<'static>{
    use crate::app::*;
    let (name,bytes):(&str,&'static [u8])=match page {
        MUSIC_PAGE=>("music",include_bytes!("../assets/settings-icons/system/music.svg")),
        FOCUS_PAGE=>("focus",include_bytes!("../assets/settings-icons/system/focus.svg")),
        WATER_PAGE=>("health",include_bytes!("../assets/settings-icons/system/health.svg")),
        SHELF_PAGE=>("shelf",include_bytes!("../assets/settings-icons/system/shelf.svg")),
        TODO_PAGE=>("tasks",include_bytes!("../assets/settings-icons/system/tasks.svg")),
        MAIL_PAGE=>("mail",include_bytes!("../assets/settings-icons/system/mail.svg")),
        QUOTA_PAGE=>("quota",include_bytes!("../assets/settings-icons/system/quota.svg")),
        CALENDAR_PAGE=>("calendar",include_bytes!("../assets/settings-icons/system/calendar.svg")),
        PERFORMANCE_PAGE=>("performance",include_bytes!("../assets/settings-icons/system/performance.svg")),
        BLUETOOTH_PAGE=>("wireless",include_bytes!("../assets/device-icons/bluetooth-linear.svg")),
        _=>("weather",include_bytes!("../assets/settings-icons/system/weather.svg")),
    };
    egui::ImageSource::Bytes{uri:format!("bytes://settings-header-{name}.svg").into(),bytes:bytes.into()}
}

fn paint_wireless_settings_icon(ui:&egui::Ui,rect:egui::Rect,icon:egui::ImageSource<'static>){
    ui.painter().rect_filled(rect,rect.width()*0.25,Color32::from_rgb(57,118,211));
    egui::Image::new(icon).paint_at(ui,rect.shrink(rect.width()*0.15));
}

/// 设置分页对应的功能模块（页面序号）；通用、外观、位置、消息提醒、关于、页面管理不属于任何模块。
pub(crate) fn settings_tab_module(tab:usize)->Option<usize>{
    use crate::app::*;
    match tab {1=>Some(MUSIC_PAGE),2=>Some(FOCUS_PAGE),3=>Some(WATER_PAGE),4=>Some(TODO_PAGE),5=>Some(SHELF_PAGE),7=>Some(MAIL_PAGE),8=>Some(CALENDAR_PAGE),11=>Some(QUOTA_PAGE),12=>Some(WEATHER_PAGE),15=>Some(PERFORMANCE_PAGE),SETTINGS_TAB_WIRELESS=>Some(BLUETOOTH_PAGE),_=>None}
}

pub(crate) fn settings_label(ui:&mut egui::Ui,text:&str){
    let ctx=&ui.ctx().clone();
    ui.add_space(9.0);
    ui.label(egui::RichText::new(text).size(11.5).color(settings_color(ctx,Color32::from_gray(185),Color32::from_gray(70))));
    ui.add_space(2.0);
}
const SUPPORT_RECENT_URI: &str = "support-recent-uri";
const SUPPORT_WALL_URI: &str = "support-wall-uri";
const BUNDLED_SUPPORTERS: usize = 108;

fn support_author(ui:&mut egui::Ui, supporters: &mut crate::supporters::State){
    supporters.refresh(ui.ctx());
    let snapshot = supporters.snapshot();
    let wall_id = egui::Id::new("support-author-wall");
    if ui.ctx().data(|data| data.get_temp::<bool>(wall_id).unwrap_or(false)) {
        release_support_image(ui.ctx(), SUPPORT_RECENT_URI);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("感谢墙").size(17.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.link("← 返回").clicked() {
                    ui.ctx().data_mut(|data| data.insert_temp(wall_id, false));
                }
            });
        });
        ui.label(egui::RichText::new("按累计支持排序").size(11.0).weak());
        ui.add_space(4.0);
        let image = match snapshot {
            Some(data) => egui::Image::from_bytes(format!("bytes://supporters/wall-{}.webp", data.version), data.wall.clone()),
            None => egui::Image::new(egui::include_image!("../assets/support/wall.webp")),
        }.fit_to_original_size(1.0).max_width(ui.available_width()).alt_text("岛仔感谢墙");
        add_support_image(ui, image, SUPPORT_WALL_URI);
        return;
    }
    release_support_image(ui.ctx(), SUPPORT_WALL_URI);
    let id=egui::Id::new("support-author-open");
    let open=ui.ctx().data(|data|data.get_temp::<bool>(id).unwrap_or(false));
    if !open {release_support_codes(ui.ctx());}
    let dark=ui.visuals().dark_mode;
    ui.scope(|ui|{
    if !open {
    egui::Frame::new().fill(if dark{Color32::from_rgb(42,37,53)}else{Color32::from_rgb(239,233,250)})
        .corner_radius(10.0).inner_margin(egui::Margin::symmetric(12,8)).show(ui,|ui|{
            let image_width=120.0;
            let text_width=(ui.available_width()-image_width-ui.spacing().item_spacing.x).max(0.0);
            ui.horizontal(|ui|{
                ui.allocate_ui_with_layout(egui::vec2(text_width,64.0),egui::Layout::top_down(egui::Align::Min).with_main_align(egui::Align::Center),|ui|{
                    ui.set_max_width(text_width);
                    ui.label(egui::RichText::new("请小唐喝杯咖啡").size(19.0).strong());
                    ui.add(egui::Label::new("岛仔陪你认真生活，你的支持，陪小唐把它慢慢做好。").wrap());
                });
                ui.add(egui::Image::new(egui::include_image!("../assets/support/daozai-coffee.svg")).fit_to_exact_size(egui::vec2(image_width,62.5)));
            });
        });
    } else {
        ui.set_min_height(200.0);
        let ctx=ui.ctx().clone();
        // 只解码一次并裁出二维码区域，按实际显示像素缩小：原图解码后两张合计约 17MB，缩小后不到 1MB。
        let pixels=((180.0*ctx.pixels_per_point()).ceil() as u32).clamp(180,600);
        ui.columns(2,|columns|{
            for (i,(name,bytes,crop,color)) in [
                ("微信支付",include_bytes!("../assets/support/support-wechat-original.jpg").as_slice(),[330.0,370.0,600.0],Color32::from_rgb(5,160,80)),
                ("支付宝",include_bytes!("../assets/support/support-alipay-original.jpg").as_slice(),[200.0,660.0,860.0],Color32::from_rgb(22,120,255)),
            ].into_iter().enumerate(){
                let texture_id=egui::Id::new(("support-author-qr",i));
                let texture=ctx.data_mut(|data|data.get_temp::<egui::TextureHandle>(texture_id)).filter(|texture|texture.size()==[pixels as usize;2]).unwrap_or_else(||{
                    let image=support_code_image(bytes,crop,pixels);
                    let texture=ctx.load_texture(name,image,egui::TextureOptions::LINEAR);
                    ctx.data_mut(|data|data.insert_temp(texture_id,texture.clone()));texture
                });
                let uv=egui::Rect::from_min_max(egui::pos2(0.0,0.0),egui::pos2(1.0,1.0));
                columns[i].vertical_centered(|ui|{
                    ui.label(egui::RichText::new(name).color(color).strong());
                    let side=ui.available_width().min(156.0);
                    ui.add(egui::Image::new((texture.id(),egui::vec2(side,side))).uv(uv));
                    ui.label(egui::RichText::new(if i==0{"打开微信扫一扫"}else{"打开支付宝扫一扫"}).size(11.0));
                });
            }
        });
    }
    });
    let button=egui::Button::new(egui::RichText::new(if open{"返回"}else{"请喝咖啡 · 支持作者"}).color(Color32::from_rgb(33,27,51)))
        .fill(Color32::from_rgb(182,163,245)).corner_radius(8.0);
    if ui.add_sized([ui.available_width(),32.0],button).clicked(){
        ui.ctx().data_mut(|data|data.insert_temp(id,!open));
    }
    ui.add_space(if open{4.0}else{7.0});
    ui.label(egui::RichText::new("完全自愿 · 感谢每一份支持 · 不影响功能使用").size(11.0).color(ui.visuals().weak_text_color()));
    if !open {
        ui.add_space(4.0);
        ui.separator();
        let count = snapshot.map_or(BUNDLED_SUPPORTERS, |data| data.count);
        ui.label(format!("已有 {count} 位朋友请岛仔喝过咖啡"));
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("最近支持").size(11.0).weak());
            let image = match snapshot {
                Some(data) => egui::Image::from_bytes(format!("bytes://supporters/recent-{}.webp", data.version), data.recent.clone()),
                None => egui::Image::new(egui::include_image!("../assets/support/recent.webp")),
            }.max_size(egui::vec2(190.0, 29.0)).alt_text("最近支持的朋友");
            add_support_image(ui, image, SUPPORT_RECENT_URI);
            if ui.link("查看感谢墙 →").clicked() {
                ui.ctx().data_mut(|data| data.insert_temp(wall_id, true));
            }
        });
    } else {
        release_support_image(ui.ctx(), SUPPORT_RECENT_URI);
    }
}

fn add_support_image(ui: &mut egui::Ui, image: egui::Image<'_>, key: &str) {
    if let Some(uri) = image.uri() {
        let previous = ui.ctx().data_mut(|data| {
            let slot = data.get_temp_mut_or_default::<String>(egui::Id::new(key));
            (slot.as_str() != uri).then(|| std::mem::replace(slot, uri.to_owned()))
        });
        if let Some(previous) = previous.filter(|old| !old.is_empty()) {
            ui.ctx().forget_image(&previous);
        }
    }
    ui.add(image);
}

fn release_support_image(ctx: &egui::Context, key: &str) {
    if let Some(uri) = ctx.data_mut(|data| data.remove_temp::<String>(egui::Id::new(key))) {
        ctx.forget_image(&uri);
    }
}

fn release_support_images(ctx: &egui::Context) {
    let uris = ctx.data_mut(|data| [SUPPORT_RECENT_URI, SUPPORT_WALL_URI].map(|key| data.remove_temp::<String>(egui::Id::new(key))));
    for uri in uris.into_iter().flatten() { ctx.forget_image(&uri); }
}
pub(crate) fn settings_control_row(ui:&mut egui::Ui,title:&str,label_width:f32,controls:impl FnOnce(&mut egui::Ui)->egui::Response)->egui::Response{
    ui.horizontal(|ui|{
        // 同一张卡片里的控件左缘对齐：标签列至少 SETTINGS_LABEL_W。
        let (label_rect,_)=ui.allocate_exact_size(egui::vec2(label_width.max(SETTINGS_LABEL_W),30.0),egui::Sense::hover());
        let content=controls(ui);
        ui.painter().text(egui::pos2(label_rect.left(),content.rect.center().y),egui::Align2::LEFT_CENTER,title,egui::TextStyle::Body.resolve(ui.style()),ui.visuals().text_color());
        content
    }).inner
}
fn mail_connection_choice(ui: &mut egui::Ui, id_salt: impl std::hash::Hash, use_windows_proxy: &mut bool) -> bool {
    let before = *use_windows_proxy;
    let direct = "直连（默认）";
    let proxy = "使用 Windows 手动代理";
    settings_control_row(ui, "连接方式", 56.0, |ui| {
        egui::ComboBox::from_id_salt(id_salt).width(170.0)
            .selected_text(if before { proxy } else { direct })
            .show_ui(ui, |ui| {
                ui.selectable_value(use_windows_proxy, false, direct);
                ui.selectable_value(use_windows_proxy, true, proxy);
            }).response
    });
    *use_windows_proxy != before
}
fn setting_number<N:egui::emath::Numeric>(ui:&mut egui::Ui,title:&str,value:&mut N,min:N,max:N,suffix:&str,speed:f64)->bool{
    let mut changed=false;
    settings_control_row(ui,title,80.0,|ui|{
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
            ui.allocate_ui_with_layout(egui::vec2(140.0,30.0),egui::Layout::left_to_right(egui::Align::Center),|ui|{
                changed=number_stepper(ui,value,min,max,suffix,N::from_f64(speed),76.0);
            });
        }).response
    });
    changed
}
fn number_stepper<N:egui::emath::Numeric>(ui:&mut egui::Ui,value:&mut N,min:N,max:N,suffix:&str,step:N,field_width:f32)->bool{
    let ctx=&ui.ctx().clone();
    let mut changed=false;
    ui.scope(|ui|{
        ui.spacing_mut().item_spacing.x=6.0;
        ui.spacing_mut().button_padding=egui::vec2(4.0,2.0);
        let button=|text|egui::Button::new(egui::RichText::new(text).size(16.0).color(settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41))))
            .fill(settings_color(ctx,Color32::from_rgb(48,48,51),Color32::from_rgb(229,232,238))).stroke(egui::Stroke::NONE).corner_radius(13.0);
        let decrease=ui.add_enabled(*value>min,button("−").min_size(egui::vec2(26.0,26.0))).on_hover_text("减少");
        if decrease.clicked(){
            *value=N::from_f64((value.to_f64()-step.to_f64()).max(min.to_f64()));changed=true;
        }
        let field=ui.add_sized([field_width,28.0],egui::DragValue::new(value).range(min..=max).speed(step.to_f64()).suffix(suffix).fixed_decimals(if N::INTEGRAL{0}else{2}));
        changed|=field.changed();
        if ui.add_enabled(*value<max,button("+").min_size(egui::vec2(26.0,26.0))).on_hover_text("增加").clicked(){
            *value=N::from_f64((value.to_f64()+step.to_f64()).min(max.to_f64()));changed=true;
        }
    });
    changed
}
/// 时段行的宽度：两个加减器（−、输入框、+ 各间隔 6）加中间的「至」和两侧间距。
/// 区域刚好装下内容，靠右放置时右边缘就和单个加减器对齐。
const TIME_RANGE_HOURS_W:f32=(26.0+6.0+48.0+6.0+26.0)*2.0+8.0+13.0+8.0;
const TIME_RANGE_CLOCK_W:f32=(26.0+6.0+56.0+6.0+26.0)*2.0+8.0+13.0+8.0;
/// 时刻加减器（一天中的分钟数，15 分钟一档，跨午夜循环），中间可直接输入 12:30。
pub(crate) fn time_stepper(ui:&mut egui::Ui,value:&mut u32,id:&str)->bool{
    let ctx=&ui.ctx().clone();
    let mut changed=false;
    ui.push_id(id,|ui|{
        ui.spacing_mut().item_spacing.x=6.0;
        ui.spacing_mut().button_padding=egui::vec2(4.0,2.0);
        let button=|text|egui::Button::new(egui::RichText::new(text).size(16.0).color(settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41))))
            .fill(settings_color(ctx,Color32::from_rgb(48,48,51),Color32::from_rgb(229,232,238))).stroke(egui::Stroke::NONE).corner_radius(13.0);
        if ui.add(button("−").min_size(egui::vec2(26.0,26.0))).on_hover_text("提前 15 分钟").clicked(){*value=(*value+1440-15)%1440;changed=true;}
        let field=ui.add_sized([56.0,28.0],egui::DragValue::new(value).range(0..=1439).speed(1.0)
            .custom_formatter(|v,_|{let m=v as u32;format!("{:02}:{:02}",m/60,m%60)})
            .custom_parser(|t|{let (h,m)=t.trim().split_once([':','：'])?;Some((h.parse::<u32>().ok()?.min(23)*60+m.parse::<u32>().ok()?.min(59)) as f64)}));
        changed|=field.changed();
        if ui.add(button("+").min_size(egui::vec2(26.0,26.0))).on_hover_text("推后 15 分钟").clicked(){*value=(*value+15)%1440;changed=true;}
    });
    changed
}
fn setting_choices(ui:&mut egui::Ui,title:&str,value:&mut u32,values:[u32;3],unit:&str){
    let labels=values.map(|v|format!("{v} {unit}"));
    let options:Vec<(u32,&str)>=values.iter().zip(labels.iter()).map(|(v,l)|(*v,l.as_str())).collect();
    settings_control_row_right(ui,title,|ui|{settings_segmented(ui,value,&options,62.0);});
}
/// 设置页分页序号。岛上入口和测试按序号跳转。
pub(crate) const SETTINGS_TAB_APPEARANCE: usize = 13;
pub(crate) const SETTINGS_TAB_POSITION: usize = 14;
pub(crate) const SETTINGS_TAB_PAGES: usize = 10;
pub(crate) const SETTINGS_TAB_WIRELESS: usize = 16;
#[cfg_attr(not(feature="sdk-js"),allow(dead_code))]
pub(crate) const SETTINGS_TAB_PLUGINS: usize = 17;
#[cfg(feature="sdk-js")]
pub(crate) const SETTINGS_TAB_PLUGIN_DETAIL: usize = 18;
/// 侧栏：一项就是一页，不再有页内子标签。专注与健康（2、3）、待办与置物（4、5）各合成一页；
/// 插件和关于固定在左下角，上方一条细线和功能页分开。
/// 第四列是这一项之前的分组标题。
const SETTINGS_GROUPS: [(&str,usize,usize,&str);15 + cfg!(feature="sdk-js") as usize] = [
    ("通用",0,0,"岛仔"), ("外观",SETTINGS_TAB_APPEARANCE,SETTINGS_TAB_APPEARANCE,""), ("位置",SETTINGS_TAB_POSITION,SETTINGS_TAB_POSITION,""), ("页面管理",SETTINGS_TAB_PAGES,SETTINGS_TAB_PAGES,""),
    ("音乐",1,1,"功能"), ("专注与健康",2,3,""), ("待办与置物",4,5,""), ("消息提醒",6,6,""), ("邮箱",7,7,""), ("日程",8,8,""), ("AI 额度",11,11,""), ("天气",12,12,""), ("系统性能",15,15,""), ("无线设备",SETTINGS_TAB_WIRELESS,SETTINGS_TAB_WIRELESS,""),
    #[cfg(feature="sdk-js")] ("插件",SETTINGS_TAB_PLUGINS,SETTINGS_TAB_PLUGINS,""),
    ("关于",9,9,""),
];
const SETTINGS_BOTTOM_ROWS: usize = if cfg!(feature="sdk-js") { 2 } else { 1 };
/// 分页标题；合并页（3→2、5→4）显示合并后的名字。
fn settings_tab_title(tab:usize)->&'static str{
    SETTINGS_GROUPS.iter().find(|(_,start,end,_)|(*start..=*end).contains(&tab)).map_or("通用",|g|g.0)
}
#[cfg(test)]
pub(crate) fn settings_tab_group_for_tests(tab:usize)->&'static str{settings_tab_title(tab)}

fn settings_sidebar_row(
    ui: &mut egui::Ui, name: &str, icon: egui::ImageSource<'static>, row_h: f32,
    selected: bool, wireless: bool,
    #[cfg(feature="sdk-js")] navigation_icon: Option<&crate::sdk_spike::local::NavigationIcon>,
) -> egui::Response {
    let ctx = ui.ctx().clone();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), name));
    if selected || response.hovered() {
        let fill = if selected {
            settings_color(&ctx, Color32::from_rgb(43,43,46), Color32::from_rgb(214,221,232))
        } else {
            settings_color(&ctx, Color32::from_rgb(30,30,33), Color32::from_rgb(225,230,238))
        };
        ui.painter().rect_filled(rect, 7.0, fill);
    }
    let icon_bg = egui::Rect::from_center_size(egui::pos2(rect.left()+21.0,rect.center().y),egui::vec2(23.0,23.0));
    #[cfg(feature="sdk-js")]
    let use_default_icon = if let Some(navigation_icon) = navigation_icon {
        let painter = ui.painter();
        let [r, g, b] = navigation_icon.background();
        painter.rect_filled(icon_bg, 6.0, Color32::from_rgb(r, g, b));
        crate::plugin_canvas_ui::draw_navigation_icon(
            painter, icon_bg, navigation_icon.commands(), Color32::WHITE,
        );
        false
    } else {
        true
    };
    #[cfg(not(feature="sdk-js"))]
    let use_default_icon = true;
    if use_default_icon {
        if wireless { paint_wireless_settings_icon(ui, icon_bg, icon); }
        else { egui::Image::new(icon).paint_at(ui, icon_bg); }
    }
    let color = if selected {
        settings_color(&ctx, Color32::WHITE, Color32::from_rgb(30,34,41))
    } else {
        settings_color(&ctx, Color32::from_gray(225), Color32::from_gray(55))
    };
    ui.painter().with_clip_rect(rect.shrink2(egui::vec2(5.0,0.0))).text(
        egui::pos2(rect.left()+40.0,rect.center().y), egui::Align2::LEFT_CENTER, name,
        egui::FontId::proportional(13.0), color,
    );
    response
}

impl App {
    fn settings_group_visible(&self,start:usize,end:usize)->bool{
        (start..=end).any(|tab|settings_tab_module(tab).is_none_or(|page|!self.settings.hidden_settings_pages[page]))
    }

    fn settings_sidebar_builtin(
        &mut self, ui: &mut egui::Ui, (name, start, end): (&str, usize, usize),
        row_h: f32, row_gap: f32,
    ) {
        let icon = match start {
            SETTINGS_TAB_WIRELESS=>egui::include_image!("../assets/device-icons/bluetooth-linear.svg"),
            0=>egui::include_image!("../assets/settings-icons/system/general.svg"),
            11=>egui::include_image!("../assets/settings-icons/system/quota.svg"),
            10=>egui::include_image!("../assets/settings-icons/system/pages.svg"),
            1=>egui::include_image!("../assets/settings-icons/system/music.svg"),
            2=>egui::include_image!("../assets/settings-icons/system/focus.svg"),
            4=>egui::include_image!("../assets/settings-icons/system/tasks.svg"),
            6=>egui::include_image!("../assets/settings-icons/system/notifications.svg"),
            15=>egui::include_image!("../assets/settings-icons/system/performance.svg"),
            12=>egui::include_image!("../assets/settings-icons/system/weather.svg"),
            7=>egui::include_image!("../assets/settings-icons/system/mail.svg"),
            8=>egui::include_image!("../assets/settings-icons/system/calendar.svg"),
            SETTINGS_TAB_APPEARANCE=>egui::include_image!("../assets/settings-icons/system/appearance.svg"),
            SETTINGS_TAB_POSITION=>egui::include_image!("../assets/settings-icons/system/position.svg"),
            #[cfg(feature="sdk-js")]
            SETTINGS_TAB_PLUGINS=>egui::include_image!("../assets/settings-icons/system/shelf.svg"),
            _=>egui::include_image!("../assets/settings-icons/system/about.svg"),
        };
        #[cfg(feature="sdk-js")]
        let icon = settings_tab_module(start).and_then(|page|self.plugins.appearance_icon(page,true)).unwrap_or(icon);
        let selected = (start..=end).contains(&self.settings_tab);
        if settings_sidebar_row(
            ui, name, icon, row_h, selected, start == SETTINGS_TAB_WIRELESS,
            #[cfg(feature="sdk-js")]
            None,
        ).clicked() && !selected {
            self.settings_tab = start;
            #[cfg(feature="sdk-js")]
            { self.settings_plugin = None; }
        }
        ui.add_space(row_gap);
    }

    #[cfg(feature="sdk-js")]
    fn apply_settings_navigation(&mut self, route: crate::sdk_spike::local::SettingsNavigation) {
        use crate::sdk_spike::local::SettingsNavigation;
        match route {
            SettingsNavigation::Tab(tab) => {
                self.settings_tab = tab;
                self.settings_plugin = None;
            }
            SettingsNavigation::Plugin(id) => self.open_plugin_settings(&id),
            SettingsNavigation::OpenStatus(id) => self.show_status_page(&id),
        }
    }

    /// 功能页顶部卡片：名称、说明和开启状态。开关统一在「页面管理」；关闭时这里给出「开启」入口。
    /// 返回功能是否开启，调用方据此把下面的设置置灰。
    fn settings_feature_header(&mut self,ui:&mut egui::Ui,page:usize,description:&str,extra:impl FnOnce(&mut Self,&mut egui::Ui))->bool{
        let ctx=&ui.ctx().clone();
        let on=self.module_on(page);
        let (mut enable,mut open)=(false,false);
        settings_card(ui,|ui|{
            ui.horizontal(|ui|{
                let icon=settings_page_icon(page);
                #[cfg(feature="sdk-js")]
                let icon=self.plugins.appearance_icon(page,false).unwrap_or(icon);
                if page==crate::app::BLUETOOTH_PAGE {
                    let (rect,_)=ui.allocate_exact_size(egui::vec2(36.0,36.0),egui::Sense::hover());
                    paint_wireless_settings_icon(ui,rect,icon);
                }else{
                    ui.add(egui::Image::new(icon).fit_to_exact_size(egui::vec2(36.0,36.0)));
                }
                ui.add_space(4.0);
                ui.vertical(|ui|{
                    ui.spacing_mut().item_spacing.y=2.0;
                    ui.label(egui::RichText::new(PAGE_NAMES[page]).size(15.0).strong().color(settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41))));
                    ui.label(egui::RichText::new(description).size(11.5).color(settings_color(ctx,Color32::from_gray(175),Color32::from_gray(90))));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                    // 每个功能卡片同一个位置：开着时「打开」跳到岛上对应页面，关着时「开启」。
                    if on {open=ui.button("打开").on_hover_text(format!("在岛上打开「{}」",PAGE_NAMES[page])).clicked();extra(self,ui);} else {enable=ui.button("开启").clicked();}
                    settings_state_chip(ui,on);
                });
            });
            if !on {
                ui.label(egui::RichText::new(format!("「{}」已关闭，不会在后台同步或提醒",PAGE_NAMES[page])).size(12.0).color(settings_color(ctx,Color32::from_rgb(238,181,91),Color32::from_rgb(139,88,12))));
            }
        });
        if enable {self.set_page_visible(page,true);}
        if open {
            self.settings_open=false;
            if page==crate::app::CALENDAR_PAGE {self.open_calendar(None);}
            else {if page==crate::app::SHELF_PAGE {self.shelf_tab=0;} self.show_page(page);}
        }
        self.module_on(page)
    }
    pub fn settings_ui(&mut self,ctx:&egui::Context,autostart:bool){
        // 设置弹窗独立于页面的灰显区域。模块关掉后撤下弹窗，避免继续操作。
        if !self.module_on(crate::app::MAIL_PAGE) {self.mail.form.open=false;}
        if !self.module_on(crate::app::CALENDAR_PAGE) {self.calendar.form.open=false;}
        if !self.module_on(crate::app::QUOTA_PAGE)&&self.quota.connect_open {self.quota_close_connection();}
        if !self.settings_open{
            if self.window_layout().close(){release_support_images(ctx);}
            release_support_codes(ctx);
            self.weather_source.close();
        }
        self.updates.poll(ctx);
        self.updates.dialog_ui(ctx);
        if !self.settings_open{return;}
        let id=egui::ViewportId::from_hash_of("daozai-settings");
        let focus_requested=std::mem::take(&mut self.settings_focus_requested);
        let opening=self.window_layout().open();
        ctx.show_viewport_immediate(id,egui::ViewportBuilder::default().with_title("岛仔设置").with_icon(theme::app_icon()).with_inner_size([690.0,560.0]).with_min_inner_size([600.0,450.0]).with_visible(false).with_active(false).with_window_level(egui::WindowLevel::Normal),|ctx,_|{
            let restored=opening && self.window_layout().restore(ctx);
            if !restored{self.window_layout().observe(ctx);}
            let dark=self.settings.settings_theme.dark(crate::settings_theme::system_dark(ctx));
            let original_style=ctx.style();
            let mut style=(*original_style).clone();
            style.visuals=if dark{egui::Visuals::dark()}else{egui::Visuals::light()};
            // 侧栏与正文之间的竖线、弹窗里的分隔线都用这条淡线；浅色模式不再是一条深灰粗线。
            // 侧栏边框已去掉，这里是唯一的一条：浅色模式一条淡线，深色模式不画线，靠两块底色区分。
            style.visuals.widgets.noninteractive.bg_stroke=if dark{egui::Stroke::NONE}else{egui::Stroke::new(1.0,Color32::from_rgb(226,229,234))};
            ctx.set_style(style);
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(if dark{egui::SystemTheme::Dark}else{egui::SystemTheme::Light}));
            if self.settings.settings_theme==crate::settings_theme::SettingsTheme::System {
                ctx.request_repaint_after(std::time::Duration::from_secs(1));
            }
            if focus_requested {
                // Glow 在首帧交换缓冲后执行窗口命令，避免先显示白色空窗口。
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            if ctx.input(|i|i.viewport().close_requested()){self.settings_open=false;self.quota_close_connection();}
            let before=format!("{:?}",self.settings);
            #[cfg(feature="sdk-js")]
            let plugin_pages: Vec<_> = self.plugins.standalone_settings_pages()
                .map(|(id, name)| (id.to_owned(), name.to_owned())).collect();
            #[cfg(not(feature="sdk-js"))]
            let plugin_pages: &[(String, String)] = &[];
            #[cfg(feature="sdk-js")]
            if self.settings_tab == SETTINGS_TAB_PLUGIN_DETAIL
                && !plugin_pages.iter().any(|(id, _)| self.settings_plugin.as_ref() == Some(id))
            {
                self.settings_tab = SETTINGS_TAB_PLUGINS;
                self.settings_plugin = None;
            }
            egui::SidePanel::left("settings-sidebar").resizable(false).exact_width(180.0).frame(egui::Frame::new().fill(settings_color(ctx,Color32::from_rgb(29,29,32),Color32::from_rgb(237,239,243))).stroke(egui::Stroke::NONE).inner_margin(egui::Margin::symmetric(10,12))).show(ctx,|ui|{
                ui.add_space(2.0);
                let builtin_rows=SETTINGS_GROUPS.iter().filter(|&&(_,start,end,_)|self.settings_group_visible(start,end)).count();
                #[cfg(feature="sdk-js")]
                let plugin_rows=plugin_pages.iter().filter(|(id,_)|!self.plugins.status_page_settings_hidden(id)).count();
                #[cfg(not(feature="sdk-js"))]
                let plugin_rows=0;
                let per=(ui.available_height()-80.0)/(builtin_rows+plugin_rows) as f32;
                let row_h=(per-6.0).clamp(20.0,32.0);
                let row_gap=(per-3.0-row_h).clamp(0.0,3.0);
                let (main, bottom) = SETTINGS_GROUPS.split_at(SETTINGS_GROUPS.len()-SETTINGS_BOTTOM_ROWS);
                let (general, features) = main.split_at(4);
                let builtin_order = self.settings.page_order;
                let plugin_order = self.settings.plugin_page_order.clone();
                let order = crate::model::ordered_pages(&builtin_order, &plugin_order, plugin_pages.iter().map(|(id, _)| id.as_str()));
                let footer_height = bottom.len() as f32*(row_h+row_gap+ui.spacing().item_spacing.y)+17.0;
                let main_height = (ui.available_height()-footer_height).max(0.0);
                egui::ScrollArea::vertical().id_salt("settings-sidebar-scroll").max_height(main_height)
                    .auto_shrink([false,false]).show(ui, |ui| {
                        let heading = |ui: &mut egui::Ui, label: &str, space| {
                            ui.add_space(space);
                            let color = settings_color(ui.ctx(), Color32::from_gray(150), Color32::from_gray(105));
                            ui.label(egui::RichText::new(label).size(11.0).color(color));
                            ui.add_space(3.0);
                        };
                        heading(ui, general[0].3, 2.0);
                        for &(name, start, end, _) in general {
                            if self.settings_group_visible(start,end) {
                                self.settings_sidebar_builtin(ui, (name, start, end), row_h, row_gap);
                            }
                        }
                        heading(ui, features[0].3, 8.0);
                        let mut shown = std::collections::HashSet::new();
                        for page in order {
                            match page {
                                crate::model::OrderedPage::Builtin(page) => {
                                    // 消息提醒没有岛上页面，在邮箱入口前显示。
                                    let tab = crate::app::PAGE_SETTINGS_TABS[page];
                                    for &(name, start, end, _) in features.iter().filter(|&&(_, start, end, _)| {
                                        (start..=end).contains(&tab) || (start == 6 && page == crate::app::MAIL_PAGE)
                                    }) {
                                        if shown.insert(start) {
                                            if self.settings_group_visible(start,end) {
                                                self.settings_sidebar_builtin(ui, (name, start, end), row_h, row_gap);
                                            }
                                        }
                                    }
                                }
                                crate::model::OrderedPage::Plugin(_plugin_id) => {
                                    #[cfg(feature="sdk-js")]
                                    {
                                        let Some((id, name)) = plugin_pages.iter().find(|(id, _)| id == _plugin_id) else { continue; };
                                        if self.plugins.status_page_settings_hidden(id) {continue;}
                                        let selected = self.settings_tab == SETTINGS_TAB_PLUGIN_DETAIL
                                            && self.settings_plugin.as_ref() == Some(id);
                                        let icon = egui::include_image!("../assets/settings-icons/system/pages.svg");
                                        let response = ui.push_id(("plugin-settings-sidebar", id), |ui| {
                                            settings_sidebar_row(
                                                ui, name, icon, row_h, selected, false,
                                                self.plugins.navigation_icon(id),
                                            ).on_hover_text(name)
                                        }).inner;
                                        if selected && focus_requested && !ui.clip_rect().contains_rect(response.rect) {
                                            response.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        if response.clicked() && !selected { self.open_plugin_settings(id); }
                                        ui.add_space(row_gap);
                                    }
                                }
                            }
                        }
                    });
                ui.add_space(4.0);
                ui.painter().hline(ui.max_rect().x_range().shrink(4.0), ui.cursor().top(), egui::Stroke::new(1.0,
                    settings_color(ctx, Color32::from_rgb(46,46,51), Color32::from_rgb(218,222,229))));
                ui.add_space(9.0);
                for &(name, start, end, _) in bottom {
                    self.settings_sidebar_builtin(ui, (name, start, end), row_h, row_gap);
                }
            });
            egui::CentralPanel::default().frame(egui::Frame::new().fill(settings_color(ctx,Color32::from_rgb(25,25,27),Color32::from_rgb(246,247,249))).inner_margin(egui::Margin::symmetric(22,16))).show(ctx,|ui|{
                settings_apply_style(ui);
                #[cfg(feature="sdk-js")]
                let plugin = plugin_pages.iter().find(|(id, _)| {
                    self.settings_tab == SETTINGS_TAB_PLUGIN_DETAIL && self.settings_plugin.as_ref() == Some(id)
                });
                let title = settings_tab_title(self.settings_tab);
                #[cfg(feature="sdk-js")]
                let title = plugin.map_or(title, |(_, name)| name.as_str());
                ui.horizontal(|ui|{
                    let label = egui::RichText::new(title).size(18.0).strong()
                        .color(settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41)));
                    #[cfg(feature="sdk-js")]
                    if let Some((id, _)) = plugin {
                        let can_open = self.plugins.status_pages().any(|(page, _)| page == id);
                        let title_width = (ui.available_width() - if can_open { 106.0 } else { 0.0 }).max(90.0);
                        ui.allocate_ui_with_layout(egui::vec2(title_width, 24.0),
                            egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.scope(|ui| {
                                    ui.set_max_width((title_width - 58.0).max(20.0));
                                    ui.add(egui::Label::new(label).truncate()).on_hover_text(title);
                                });
                                crate::sdk_spike::local::plugin_tag(ui);
                            });
                        if can_open {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("打开功能页").clicked() {
                                    self.settings_open = false;
                                    self.show_status_page(id);
                                }
                            });
                        }
                        return;
                    }
                    ui.label(label);
                    // 插件页：「集市 / 已安装」切换放在标题同一行的右侧。
                    #[cfg(feature="sdk-js")]
                    if self.settings_tab==SETTINGS_TAB_PLUGINS {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|self.plugins.header_tabs(ui));
                    }
                });
                ui.add_space(6.0);
                // 合并页：健康（3）并入专注（2），置物（4→5）并入待办，旧序号仍能打开对应页面。
                let tab=match self.settings_tab {3=>2,5=>4,t=>t};
                if tab != 9 { release_support_images(ctx); }
                let scroll = egui::ScrollArea::vertical().auto_shrink([false,false]);
                #[cfg(feature="sdk-js")]
                let scroll = if let Some((id, _)) = plugin { scroll.id_salt(("settings-plugin-scroll", id)) }
                    else { scroll.id_salt(("settings-scroll",tab)) };
                #[cfg(not(feature="sdk-js"))]
                let scroll = scroll.id_salt(("settings-scroll",tab));
                scroll.show(ui,|ui|{
                    // 右侧给滚动条留出空隙，卡片和开关不会被它压住。
                    ui.set_width((ui.available_width()-12.0).max(240.0));
                    #[cfg(feature="sdk-js")]
                    if let Some((id, _)) = plugin {
                        self.plugins.standalone_settings(ui, id);
                        return;
                    }
                    #[cfg(any(feature="sdk-js",feature="sdk-wasm"))]
                    if tab==4 {crate::sdk_spike::live_settings(ui);}
                    match tab {
                    15=>{
                        let on=self.settings_feature_header(ui,crate::app::PERFORMANCE_PAGE,"查看硬件负载和实时吞吐量",|_,_|{});
                        ui.add_enabled_ui(on,|ui|self.performance_settings(ui));
                    }
                    SETTINGS_TAB_WIRELESS=>{
                        let on=self.settings_feature_header(ui,crate::app::BLUETOOTH_PAGE,"查看蓝牙电量和 2.4G 接收器状态",|_,_|{});
                        #[cfg(all(windows,feature="sdk-js"))]
                        self.plugins.wireless_settings(ui, &self.wireless_enhancement, on);
                        ui.add_enabled_ui(on,|ui|{
                            settings_label(ui,"岛上常驻");
                            settings_card(ui,|ui|{
                                resident_setting_row(ui,"无线设备电量","收起时显示，最多两台",on,&mut self.settings.show_bluetooth);
                                if self.settings.show_bluetooth {
                                    settings_separator(ui);
                                    self.wireless_resident_settings(ui);
                                }
                            });
                            settings_label(ui,"电量提醒");
                            settings_card(ui,|ui|{
                                setting_row(ui,"岛上提示","接上电源、充满或电量偏低时提醒",&mut self.settings.bluetooth_battery_notifications);
                            });
                        });
                    }
                    #[cfg(feature="sdk-js")]
                    SETTINGS_TAB_PLUGINS=>{
                        let on=self.module_on(crate::app::TODO_PAGE);
                        let calendar_on=self.module_on(crate::app::CALENDAR_PAGE);
                        let quota_on=self.quota_active();
                        if let Some(route)=self.plugins.manage(ui,on,calendar_on,quota_on,
                            #[cfg(all(windows,feature="sdk-material"))] self.material_active_id.as_deref().filter(|_| self.material_capture_forced),
                        ){self.apply_settings_navigation(route);}
                    }
                    12=>{
                        let on=self.settings_feature_header(ui,crate::app::WEATHER_PAGE,"当地天气与七日预报",|_,_|{});
                        ui.add_space(4.0);
                        ui.add_enabled_ui(on,|ui|self.weather_settings(ui));
                    }
                    11=>{
                        let on=self.settings_feature_header(ui,crate::app::QUOTA_PAGE,"每 3 分钟自动更新剩余额度",|_,_|{});
                        ui.add_space(4.0);
                        ui.add_enabled_ui(on,|ui|{
                            self.quota_settings(ui);
                            #[cfg(feature="sdk-js")]
                            self.plugins.settings(ui,"quota",on);
                        });
                    }
                    SETTINGS_TAB_PAGES=>{
                        ui.label(egui::RichText::new("选择需要的页面和状态组件。").size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                        // 岛上显示：收起时的常驻内容在这里统一排序和开关；岛上歌词属于音乐的显示方式，在音乐页设置。
                        settings_label(ui,"岛上显示");
                        settings_card(ui,|ui|{
                            self.resident_order_settings(ui);
                            ui.separator();
                            setting_row(ui,"时间","关闭后仍可在展开顶栏查看",&mut self.settings.show_clock);
                            ui.separator();
                            setting_row(ui,"电量","无电池设备自动隐藏",&mut self.settings.show_battery);
                            ui.separator();
                            resident_setting_row(ui,"天气","天气图标和温度，点击打开天气页",
                                self.module_on(crate::app::WEATHER_PAGE),&mut self.settings.show_weather);
                            ui.separator();
                            resident_setting_row(ui,"无线设备电量","收起时显示，最多两台",
                                self.module_on(crate::app::BLUETOOTH_PAGE),&mut self.settings.show_bluetooth);
                            ui.separator();
                            ui.add_enabled_ui(self.module_on(crate::app::PERFORMANCE_PAGE),|ui|setting_row(ui,"系统性能","点击常驻指标打开性能页",&mut self.settings.show_performance));
                            ui.separator();
                            let before=self.quota.config.compact;
                            resident_setting_row(ui,"AI 额度","常驻账号，WorkBuddy 积分在前，DeepSeek 余额在最后",
                                self.module_on(crate::app::QUOTA_PAGE),&mut self.quota.config.compact);
                            if before!=self.quota.config.compact {self.quota.config.save();}
                        });
                        settings_label(ui,"功能页面");
                        settings_card(ui,|ui|{
                            #[cfg(feature = "sdk-js")]
                            if self.plugins.page_shortcut_settings(ui,&mut self.settings.show_page_shortcuts,&mut self.settings.hide_page_arrows) {
                                ui.separator();
                            }
                            let descriptions=["播放控制与同步歌词","番茄钟与休息计时","喝水与久坐提醒","临时收纳文件位置","任务清单与到期提醒","邮件摘要与验证码","Agent 剩余额度和账号切换","月历、邮件邀请与工作日程","当地天气与七日预报","处理器、显卡、内存、网络与磁盘","蓝牙设备连接状态与多来源电量"];
                            settings_hint(ui,"拖动左侧手柄调整岛上顺序。关闭后暂停后台运行，保留已有数据。选择「关闭并隐藏」可从设置左栏收起入口，合并入口需隐藏组内所有功能。");
                            use crate::model::OrderedPage;
                            #[derive(Clone)]
                            struct PageDrag(egui::Id);
                            #[cfg(feature = "sdk-js")]
                            let plugins: Vec<_> = self.plugins.status_page_preferences()
                                .filter(|(_, _, enabled, _)| *enabled)
                                .map(|(id, _, _, _)| id.to_owned()).collect();
                            #[cfg(not(feature = "sdk-js"))]
                            let plugins: Vec<String> = Vec::new();
                            let builtin = self.settings.page_order;
                            let saved_plugins = self.settings.plugin_page_order.clone();
                            let mut order: Vec<_> = crate::model::ordered_pages(&builtin, &saved_plugins, plugins.iter().map(String::as_str)).collect();
                            let rows: Vec<_> = order.iter().copied().enumerate().filter(|(_, page)| match page {
                                OrderedPage::Builtin(_) => true,
                                OrderedPage::Plugin(id) => plugins.iter().any(|installed| installed == id),
                            }).collect();
                            let drag_index = |payload: &PageDrag| rows.iter().find_map(|(index, page)| {
                                (egui::Id::new(page) == payload.0).then_some(*index)
                            });
                            let mut movement=None;
                            let mut jump=None;
                            let row_w=ui.available_width();
                            for (row_index, &(index, page)) in rows.iter().enumerate() {
                                if row_index > 0 {ui.separator();}
                                let row=ui.push_id(("page-order",page),|ui|ui.horizontal(|ui|{
                                    ui.set_max_width(row_w);
                                    let (rect,handle)=ui.allocate_exact_size(egui::vec2(20.0,40.0),egui::Sense::drag());
                                    for x in [-3.0,3.0] {for y in [-6.0,0.0,6.0] {
                                        ui.painter().circle_filled(rect.center()+egui::vec2(x,y),1.4,settings_color(ctx,Color32::from_gray(155),Color32::from_gray(100)));
                                    }}
                                    handle.on_hover_cursor(egui::CursorIcon::Grab).dnd_set_drag_payload(PageDrag(egui::Id::new(page)));
                                    match page {
                                        OrderedPage::Builtin(page) => {
                                            let mut visible=self.settings.visible_pages[page];
                                            let hidden=self.settings.hidden_settings_pages[page];
                                            let can_disable=!visible || self.settings.visible_pages.iter().filter(|enabled|**enabled).count()>1;
                                            let jump_w=58.0;
                                            let text_w=ui.available_width()-jump_w-SETTINGS_SWITCH_W-PAGE_VISIBILITY_ACTION_WIDTH-3.0*ui.spacing().item_spacing.x;
                                            setting_row_text(ui,PAGE_NAMES[page],descriptions[page],text_w);
                                            ui.add_enabled_ui(can_disable,|ui|switch(ui,&mut visible));
                                            let change_hidden=page_visibility_action(ui,visible,hidden,can_disable);
                                            if ui.add_sized([jump_w,26.0],egui::Button::new(egui::RichText::new("设置 ›").size(12.0)).frame(false)).on_hover_text(format!("打开「{}」设置",PAGE_NAMES[page])).clicked(){jump=Some(crate::app::PAGE_SETTINGS_TABS[page]);}
                                            if change_hidden {
                                                self.set_page_settings_hidden(page,!hidden);
                                            } else if visible!=self.settings.visible_pages[page] {
                                                self.set_page_visible(page,visible);
                                                ctx.request_repaint();
                                            }
                                        }
                                        OrderedPage::Plugin(_id) => {
                                            #[cfg(feature="sdk-js")]
                                            if let Some(route) = self.plugins.status_page_settings(ui, _id) {
                                                self.apply_settings_navigation(route);
                                            }
                                        }
                                    }
                                })).inner.response;
                                if let Some(from) = row.dnd_hover_payload::<PageDrag>()
                                    .and_then(|payload| drag_index(&payload)).filter(|from| *from != index)
                                {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                                    let y = if from < index { row.rect.bottom() } else { row.rect.top() };
                                    ui.painter().line_segment([egui::pos2(row.rect.left(),y),egui::pos2(row.rect.right(),y)],egui::Stroke::new(2.0,Color32::from_rgb(70,150,255)));
                                }
                                if let Some(from) = row.dnd_release_payload::<PageDrag>().and_then(|payload| drag_index(&payload)) {
                                    movement = Some((from, index));
                                }
                            }
                            if egui::DragAndDrop::has_payload_of_type::<PageDrag>(ctx) {
                                let clip = ui.clip_rect();
                                if let Some(pointer) = ctx.pointer_hover_pos().filter(|pos| clip.contains(*pos)) {
                                    let edge = 26.0;
                                    let up = (clip.top() + edge - pointer.y).max(0.0);
                                    let down = (pointer.y - clip.bottom() + edge).max(0.0);
                                    let delta = (up - down) * 14.0 * ctx.input(|i| i.stable_dt.min(1.0 / 30.0));
                                    if delta != 0.0 {
                                        ui.scroll_with_delta_animation(egui::vec2(0.0, delta), egui::style::ScrollAnimation::none());
                                        ctx.request_repaint();
                                    }
                                }
                            }
                            if let Some(tab)=jump {self.settings_tab=tab;}
                            if let Some((from, to)) = movement.filter(|(from, to)| from != to) {
                                if from < to { order[from..=to].rotate_left(1); }
                                else { order[to..=from].rotate_right(1); }
                                self.settings.plugin_page_order.clear();
                                let mut slot = 0;
                                for page in order {
                                    match page {
                                        OrderedPage::Builtin(page) => {
                                            self.settings.page_order[slot] = page;
                                            slot += 1;
                                        }
                                        OrderedPage::Plugin(id) => self.settings.plugin_page_order.push((id.to_owned(), slot)),
                                    }
                                }
                                ctx.request_repaint();
                                ctx.request_repaint_of(egui::ViewportId::ROOT);
                            }
                            if self.settings.visible_pages.iter().filter(|enabled|**enabled).count()==1 {
                                ui.add_space(5.0);
                                ui.label(egui::RichText::new("至少保留一个页面。").size(11.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                            }
                        });
                    }
                    0=>{
                        settings_label(ui,"设置页外观");
                        settings_card(ui,|ui|{
                            use crate::settings_theme::SettingsTheme;
                            settings_control_row_right(ui,"主题",|ui|{
                                if settings_segmented(ui,&mut self.settings.settings_theme,&[(SettingsTheme::System,"跟随系统"),(SettingsTheme::Light,"浅色"),(SettingsTheme::Dark,"深色")],52.0) {ctx.request_repaint();}
                            });
                            settings_hint(ui,"仅影响设置页，岛的外观保持不变。");
                        });
                        settings_label(ui,"时间日期格式");
                        settings_card(ui,|ui|{
                            settings_control_row_right(ui,"时间制式",|ui|{settings_segmented(ui,&mut self.settings.clock_12h,&[(false,"24 小时"),(true,"12 小时")],72.0);});
                            settings_control_row_right(ui,"时段文字",|ui|{settings_segmented(ui,&mut self.settings.clock_period,&[(0,"不显示"),(1,"上午 / 下午"),(2,"详细时段")],72.0);});
                            if self.settings.clock_period==2 {settings_hint(ui,"详细时段：凌晨、上午、中午、下午、傍晚、晚上");}
                            ui.separator();
                            setting_row(ui,"显示秒数","时间显示为 14:32:08",&mut self.settings.clock_seconds);
                            ui.separator();
                            let mut date_modes=|ui:&mut egui::Ui|{
                                use crate::model::ResidentDate;
                                let modes=[
                                    (ResidentDate::Off,"关闭"),
                                    (ResidentDate::LunarOnly,"仅农历"),
                                    (ResidentDate::Gregorian,"仅公历"),
                                    (ResidentDate::Lunar,"公历＋农历"),
                                ];
                                if settings_segmented(ui,&mut self.settings.resident_date,&modes,52.0) {
                                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                                }
                            };
                            if ui.available_width()<360.0 {
                                ui.label("日期常驻");
                                date_modes(ui);
                            } else {
                                settings_control_row_right(ui,"日期常驻",date_modes);
                            }
                        });
                        settings_label(ui,"启动");
                        settings_card(ui,|ui|{
                            let mut a=autostart;
                            setting_row(ui,"开机启动","",&mut a);
                            if a!=autostart{self.pending.push(Cmd::ToggleAutostart);}
                            ui.separator();
                            setting_row(ui,"全屏时隐藏","全屏隐藏岛仔，健康提醒静默保留",&mut self.settings.hide_fullscreen);
                            ui.separator();
                            #[cfg(windows)]
                            let supported = crate::native::Window::capture_exclusion_supported();
                            #[cfg(not(windows))]
                            let supported = false;
                            #[cfg(all(windows,feature="sdk-material"))]
                            let material_forced = self.material_capture_forced;
                            #[cfg(not(all(windows,feature="sdk-material")))]
                            let material_forced = false;
                            let mut enabled = self.settings.exclude_from_capture || material_forced;
                            ui.add_enabled_ui(supported && !material_forced, |ui| {
                                setting_row(ui, "在截图和录屏中隐藏岛仔", "桌面上照常显示，支持的截图和录屏工具会略过岛仔。", &mut enabled);
                            });
                            if material_forced {
                                settings_hint(ui,"材质生效时需隐藏岛仔自身，才能读取它背后的画面。停用材质后恢复原设置。");
                            } else if enabled != self.settings.exclude_from_capture {
                                self.capture_exclusion_request = Some(enabled);
                                self.capture_exclusion_error = None;
                            }
                            let error = self.capture_exclusion_error.as_deref()
                                .or((!supported).then_some("当前系统不支持在截图和录屏中隐藏岛仔。"));
                            if let Some(error) = error {
                                ui.add(egui::Label::new(egui::RichText::new(error).size(11.5)
                                    .color(settings_color(ctx, Color32::from_rgb(238,181,91), Color32::from_rgb(139,88,12)))).wrap());
                            }
                        });
                        self.avoidance.settings(ui,&mut self.settings);
                        ui.add_space(8.0);
                        settings_card(ui,|ui|{
                            settings_advanced(ui,"settings-general-advanced",|ui|{
                                let before=self.settings.render_compat;
                                setting_row(ui,"兼容渲染（Direct3D 12）","出现黑框、闪烁或打开设置后崩溃时开启；切换后重启岛仔生效",&mut self.settings.render_compat);
                                // 手动关闭后不再显示自动切换的原因；再次开启视为手动开启。
                                if before!=self.settings.render_compat {self.settings.render_compat_reason.clear();}
                                if !self.settings.render_compat_reason.is_empty() {settings_hint(ui,&self.settings.render_compat_reason);}
                            });
                            // 待重启的提示不折叠，收起「高级」也能看到。
                            if self.settings.render_compat!=self.render_compat_running {
                                ui.horizontal(|ui|{
                                    ui.label(egui::RichText::new("兼容渲染重启岛仔后生效").size(12.0).color(Color32::from_rgb(230,150,40)));
                                    if ui.button("立即重启").clicked(){self.pending.push(Cmd::RestartApp);}
                                });
                            }
                        });
                    }
                    SETTINGS_TAB_APPEARANCE=>{
                        settings_label(ui,"岛体外观");
                        settings_card(ui,|ui|{
                            use crate::model::{self as model,Skin};
                            #[cfg(feature="sdk-js")]
                            let plugin_picker=self.plugins.appearance_picker(ui,crate::sdk_spike::local::appearance_assets::Kind::Skin,Some(&mut self.settings.skin));
                            #[cfg(not(feature="sdk-js"))]
                            let plugin_picker=false;
                            if !plugin_picker {
                                let skins:Vec<(Skin,&str)>=Skin::ALL.iter().map(|s|(*s,s.label())).collect();
                                settings_control_row_right(ui,"皮肤",|ui|{settings_segmented(ui,&mut self.settings.skin,&skins,64.0);});
                            }
                            settings_hint(ui,self.effective_skin().description());
                            if self.effective_skin().is_glass() {
                                let mut blur=self.settings.glass_blur;
                                setting_row(ui,"背景模糊","需开启系统“透明效果”；关闭后改为烟熏半透明",&mut blur);
                                self.settings.glass_blur=blur;
                            }
                            #[cfg(all(windows,feature="sdk-js",feature="sdk-material"))]
                            {
                                let active=self.material_active_id.as_deref().filter(|_| self.material_capture_forced);
                                if self.plugins.material_settings(ui,active,self.settings.render_compat) {
                                    self.settings.render_compat=true;
                                    self.settings.render_compat_reason.clear();
                                    self.pending.push(Cmd::RestartApp);
                                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                                }
                            }
                            #[cfg(feature="sdk-js")]
                            self.plugins.appearance_picker(ui,crate::sdk_spike::local::appearance_assets::Kind::Icon,None);
                            ui.separator();
                            let shapes:Vec<(model::IslandShape,&str)>=model::IslandShape::ALL.iter().map(|s|(*s,s.label())).collect();
                            settings_control_row_right(ui,"形状",|ui|{settings_segmented(ui,&mut self.settings.island_shape,&shapes,64.0);});
                            settings_hint(ui,self.settings.island_shape.description());
                            ui.separator();
                            settings_control_row(ui,"大小",56.0,|ui|{
                                ui.horizontal_wrapped(|ui|{
                                    // 给百分比和恢复按钮留出空间，宽窗口仍使用原来的轨道长度。
                                    ui.spacing_mut().slider_width=(ui.available_width()-150.0).clamp(64.0,180.0);
                                    island_size_slider(ui,&mut self.settings.island_size);
                                    if ui.add_enabled(self.settings.island_size!=100,egui::Button::new("恢复默认")).clicked(){self.settings.island_size=100;}
                                }).response
                            });
                            settings_hint(ui,"只影响岛体，放大后文字和图标依旧清晰。");
                        });
                        settings_label(ui,"岛体显示");
                        settings_card(ui,|ui|{
                            let mut minimal=self.settings.minimal;
                            setting_row(ui,"极简刘海","收起时显示一条细线",&mut minimal);
                            if minimal!=self.settings.minimal {self.set_minimal(minimal);}
                            ui.separator();
                            let mut pet=self.settings.pet;
                            setting_row(ui,"岛仔吉祥物","收起时常驻，播放音乐时一起唱歌",&mut pet);
                            if pet!=self.settings.pet {self.set_pet(pet);}
                            #[cfg(feature="sdk-js")]
                            self.plugins.appearance_picker(ui,crate::sdk_spike::local::appearance_assets::Kind::Pet,None);
                            settings_advanced(ui,"settings-appearance-advanced",|ui|{
                                setting_row(ui,"OLED 像素偏移","防止 OLED 屏幕烧屏：收起静止时让岛体缓慢移动几个像素，不改动你保存的位置",&mut self.settings.oled_shift);
                            });
                        });
                    }
                    SETTINGS_TAB_POSITION=>{
                        settings_card(ui,|ui|{
                            let selected=self.displays.iter().find(|d|d.id==self.settings.display_id);
                            let label=if self.settings.display_id.is_empty(){"跟随主屏"}else{selected.map_or("所选屏幕未连接",|d|d.label.as_str())};
                            settings_control_row(ui,"显示屏",56.0,|ui|{
                                egui::ComboBox::from_id_salt("island-display").selected_text(label).width(ui.available_width().min(300.0)).truncate().show_ui(ui,|ui|{
                                    ui.selectable_value(&mut self.settings.display_id,String::new(),"跟随主屏");
                                    for display in &self.displays {ui.selectable_value(&mut self.settings.display_id,display.id.clone(),&display.label);}
                                }).response
                            });
                            // 只在所选屏幕断开时说明，平时不占位置。
                            if !self.settings.display_id.is_empty()&&selected.is_none(){settings_hint(ui,"暂时显示在主屏，重新连接所选屏幕后自动恢复。");}
                            ui.separator();
                            // 停靠边缘：上下为横向；左右为竖向排列，展开面板从边缘滑出。
                            let docks:Vec<(crate::model::Dock,&str)>=crate::model::Dock::ALL.iter().map(|d|(*d,d.label())).collect();
                            settings_control_row_right(ui,"停靠",|ui|{
                                // 换边后偏移的含义变了（水平 ↔ 上下），回到居中。
                                if settings_segmented(ui,&mut self.settings.dock,&docks,64.0) {self.settings.island_offset=0;}
                            });
                            if self.settings.dock.is_side() {settings_hint(ui,"左右停靠时竖向排列；吉祥物和岛上歌词在顶部或底部停靠时显示。");}
                            let side=self.settings.dock.is_side();
                            settings_control_row(ui,if side{"上下位置"}else{"水平位置"},56.0,|ui|{
                                ui.horizontal_wrapped(|ui|{
                                    let limit=crate::model::ISLAND_OFFSET_MAX;
                                    let offset=&mut self.settings.island_offset;
                                    let (less,more,less_tip,more_tip)=if side{("↑","↓","向上微调","向下微调")}else{("←","→","向左微调","向右微调")};
                                    if ui.add_enabled(*offset>-limit,egui::Button::new(less).min_size(egui::vec2(26.0,22.0))).on_hover_text(less_tip).clicked(){*offset=(*offset-5).max(-limit);}
                                    // 左侧微调按钮已占位，剩余宽度还要容纳右侧微调和恢复居中。
                                    ui.spacing_mut().slider_width=(ui.available_size_before_wrap().x-134.0).clamp(64.0,150.0);
                                    ui.add(egui::Slider::new(&mut *offset,-limit..=limit).show_value(false));
                                    if ui.add_enabled(*offset<limit,egui::Button::new(more).min_size(egui::vec2(26.0,22.0))).on_hover_text(more_tip).clicked(){*offset=(*offset+5).min(limit);}
                                    if ui.add_enabled(*offset!=0,egui::Button::new("恢复居中")).clicked(){*offset=0;}
                                }).response
                            });
                            let offset=self.settings.island_offset;
                            let (less,more)=if side{("偏上","偏下")}else{("偏左","偏右")};
                            let position=match offset {0=>"居中".to_string(),o if o<0=>format!("{less} {}%",-o),o=>format!("{more} {o}%")};
                            settings_hint(ui,&format!("当前：{position}。也可以展开后按住日期时间直接拖动。"));
                            ui.separator();
                            setting_row(ui,"顶栏适配","顶部有菜单栏或任务栏时使用",&mut self.settings.avoid_top_bar);
                            if self.settings.avoid_top_bar {
                                settings_control_row_right(ui,"显示方式",|ui|{
                                    settings_segmented(ui,&mut self.settings.top_bar_on_top,&[(false,"避开顶栏"),(true,"岛仔置顶")],90.0);
                                });
                                if self.settings.top_bar_on_top {
                                    settings_hint(ui,"岛体显示在顶栏上方。");
                                } else {
                                    let reserved=crate::display_layout::selected(&self.displays,&self.settings.display_id).map_or(0,|d|{
                                        let b=crate::display_layout::placement_bounds(d,true);(b.top-d.bounds.top).max(0)
                                    });
                                    settings_hint(ui,&if reserved>0 {format!("已检测到顶栏，岛体下移 {reserved} 像素。")}else{"未检测到顶栏；没有向系统登记的顶栏可尝试“岛仔置顶”。".to_string()});
                                }
                            }
                        });
                    }
                    1=>{
                        let on=self.settings_feature_header(ui,crate::app::MUSIC_PAGE,"播放控制与同步歌词",|_,_|{});
                        ui.add_enabled_ui(on,|ui|{
                            settings_label(ui,"Feishin 接入");
                            settings_card(ui,|ui|{
                                setting_row(ui,"连接 Feishin","读取本机 Feishin 播放状态并支持控制",&mut self.settings.feishin_enabled);
                                settings_control_row_right(ui,"WebSocket 端口",|ui|{ui.add_enabled(self.settings.feishin_enabled,egui::DragValue::new(&mut self.settings.feishin_port).range(1..=65535));});
                                settings_control_row_right(ui,"用户名",|ui|{ui.add_enabled(self.settings.feishin_enabled,egui::TextEdit::singleline(&mut self.settings.feishin_username).desired_width(180.0));});
                                let draft_id=ui.make_persistent_id("feishin-password-draft");
                                let (mut password,username)=match ui.data(|d|d.get_temp::<(String,String)>(draft_id)){
                                    Some((cached_user,cached_password)) if cached_user==self.settings.feishin_username=>(cached_password,self.settings.feishin_username.clone()),
                                    _=>(crate::feishin::get_password(&self.settings.feishin_username).unwrap_or_default(),self.settings.feishin_username.clone()),
                                };
                                settings_control_row_right(ui,"密码",|ui|{ui.add_enabled(self.settings.feishin_enabled,egui::TextEdit::singleline(&mut password).password(true).desired_width(180.0));});
                                ui.horizontal(|ui|{
                                    if ui.add_enabled(self.settings.feishin_enabled,egui::Button::new("保存凭据")).clicked(){self.pending.push(Cmd::FeishinCredential(password.clone()));}
                                    if ui.add_enabled(self.settings.feishin_enabled,egui::Button::new("清除密码")).clicked(){self.pending.push(Cmd::FeishinForgetPassword);password.clear();}
                                });
                                ui.data_mut(|d|d.insert_temp(draft_id,(username,password)));
                                settings_hint(ui,"默认端口 4333；密码保存在 Windows 凭据管理器中。Feishin 需启用远程控制（WebSocket）。");
                            });
                            settings_label(ui,"歌词");
                            settings_card(ui,|ui|{
                                setting_row(ui,"岛上歌词","播放时收起的岛显示当前句，其他常驻图标暂时隐藏；无同步歌词时显示封面和歌名",&mut self.settings.island_lyrics);
                                ui.separator();
                                setting_row(ui,"展开时显示歌词","在播放器右侧同步滚动",&mut self.settings.lyrics_enabled);
                                self.show_lyrics=self.settings.lyrics_enabled;
                                ui.separator();
                                let mut advance=-self.settings.lyric_delay_ms as f64/1000.0;
                                if setting_number(ui,"时间偏移",&mut advance,-5.0,5.0," 秒",0.1){self.settings.lyric_delay_ms=(-advance*1000.0).round()as i32;}
                                settings_hint(ui,"歌词和歌声对不上时调整：正数提前，负数延后；默认提前 0.35 秒。");
                            });
                        });
                    }
                    2=>{
                        let on=self.settings_feature_header(ui,crate::app::FOCUS_PAGE,"番茄钟与休息计时",|_,_|{});
                        ui.add_enabled_ui(on,|ui|{
                            settings_card(ui,|ui|{
                                setting_number(ui,"专注",&mut self.settings.focus_minutes,1,180," 分钟",1.0);
                                ui.separator();
                                setting_number(ui,"休息",&mut self.settings.rest_minutes,1,60," 分钟",1.0);
                                settings_hint(ui,"调整后从下一阶段开始生效。");
                            });
                        });
                        ui.add_space(12.0);
                        let on=self.settings_feature_header(ui,crate::app::WATER_PAGE,"喝水与久坐提醒",|_,_|{});
                        ui.add_enabled_ui(on,|ui|{
                            settings_card(ui,|ui|{
                                // 喝水：开关一行，数量与间隔紧随其后。
                                let old_remind=self.water.remind;
                                setting_row(ui,"喝水提醒","确认或喝水后重新计时；达标后停止提醒",&mut self.water.remind);
                                if self.water.remind!=old_remind{self.water.save();self.confirm_health_reminder(HealthReminder::Water);}
                                ui.add_enabled_ui(self.water.remind,|ui|{
                                    setting_number(ui,"提醒间隔",&mut self.settings.water_minutes,5,180," 分钟",1.0);
                                    setting_number(ui,"每次记录",&mut self.settings.serving_ml,50,1000," ml",25.0);
                                    if setting_number(ui,"每日目标",&mut self.water.goal_ml,500,6000," ml",50.0){self.water.save();}
                                });
                                ui.separator();
                                let old_sit=self.settings.sit_remind;
                                setting_row(ui,"久坐提醒","确认提醒或点「起身了」后重新计时",&mut self.settings.sit_remind);
                                if self.settings.sit_remind!=old_sit {self.confirm_health_reminder(HealthReminder::Sit);}
                                ui.add_enabled_ui(self.settings.sit_remind,|ui|{
                                    setting_number(ui,"久坐时长",&mut self.settings.sit_minutes,15,180," 分钟",1.0);
                                });
                                ui.separator();
                                // 提醒时段与午休对喝水和久坐都生效。
                                // 喝水和久坐提醒都关掉时，时段和午休不起作用，一起置灰。
                                let any_reminder=self.water.remind||self.settings.sit_remind;
                                ui.add_enabled_ui(any_reminder,|ui|{
                                    settings_control_row(ui,"提醒时段",80.0,|ui|{
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                                            // 区域宽度正好等于两个加减器加「至」，靠右放置后与上面单个加减器的右边缘对齐。
                                            ui.allocate_ui_with_layout(egui::vec2(TIME_RANGE_HOURS_W,30.0),egui::Layout::left_to_right(egui::Align::Center),|ui|{
                                                number_stepper(ui,&mut self.settings.water_start,0,23," 时",1,48.0);
                                                ui.label("至");
                                                number_stepper(ui,&mut self.settings.water_end,0,24," 时",1,48.0);
                                            });
                                        }).response
                                    });
                                    setting_row(ui,"午休不提醒","午休时间内喝水和久坐都不弹提醒",&mut self.settings.lunch_rest);
                                    if self.settings.lunch_rest {
                                        settings_control_row(ui,"午休时间",80.0,|ui|{
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                                                ui.allocate_ui_with_layout(egui::vec2(TIME_RANGE_CLOCK_W,30.0),egui::Layout::left_to_right(egui::Align::Center),|ui|{
                                                    time_stepper(ui,&mut self.settings.lunch_start,"lunch-start");
                                                    ui.label("至");
                                                    time_stepper(ui,&mut self.settings.lunch_end,"lunch-end");
                                                });
                                            }).response
                                        });
                                    }
                                    settings_hint(ui,"提醒时段和午休对喝水、久坐都生效；专注计时进行中不打扰，结束后再提醒。");
                                });
                            });
                        });
                    }
                    4=>{
                        let on=self.settings_feature_header(ui,crate::app::TODO_PAGE,"任务清单与到期提醒",|_,_|{});
                        let _=on;
                        #[cfg(feature="sdk-js")] self.plugins.settings(ui,"tasks",on);
                        ui.add_space(12.0);
                        let on=self.settings_feature_header(ui,crate::app::SHELF_PAGE,"临时收纳文件与剪贴板",|_,_|{});
                        ui.add_enabled_ui(on,|ui|{
                            settings_card(ui,|ui|{
                                ui.label(egui::RichText::new(format!("当前 {} 项 · 移除条目不会删除原文件",self.shelf.items.len())).size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                                ui.separator();
                                setting_row(ui,"记录剪贴板","在置物架的「剪贴板」里查看最近复制的文字，点一下再次复制",&mut self.settings.clipboard_history);
                                settings_hint(ui,&format!("本机保存最近 {} 条；密码管理器标记为敏感的内容不会记录。",crate::clipboard::MAX_RECENT));
                                if ui.button("打开剪贴板").clicked(){self.settings_open=false;self.shelf_tab=1;self.show_page(3);}
                            });
                        });
                    }
                    6=>{
                        settings_label(ui,"提醒");
                        settings_card(ui,|ui|{
                            setting_row(ui,"提醒总开关","关闭后饮水、专注、待办及聊天软件提醒都不显示",&mut self.settings.notifications);
                            // 提醒方式由系统通知和提示音两个设置共同决定。
                            ui.separator();
                            ui.add_enabled_ui(self.settings.notifications,|ui|{
                                let mut mode:u8=if !self.settings.system_notify{0}else if self.settings.sound{2}else{1};
                                settings_control_row_right(ui,"提醒方式",|ui|{settings_segmented(ui,&mut mode,&[(0,"仅岛上"),(1,"加系统通知"),(2,"系统通知 + 声音")],96.0);});
                                settings_hint(ui,["只在岛上提醒，不发送 Windows 系统通知","同时在 Windows 通知中心留下记录，不响铃","同时发送 Windows 系统通知并播放提示音"][mode as usize]);
                                self.settings.system_notify=mode>0;
                                self.settings.sound=mode==2;
                            });
                        });
                        settings_label(ui,"系统提示");
                        settings_card(ui,|ui|{
                            setting_row(ui,"系统状态","音量、充电、网络和键盘状态",&mut self.settings.system_hud);
                            ui.separator();
                            setting_row(ui,"蓝牙设备连接","连接与断开时提醒",&mut self.settings.bluetooth_notifications);
                        });
                        // 聊天软件：一个软件一行、一个开关；附属选项缩进跟在所属软件下面。
                        let master=self.settings.notifications;
                        settings_label(ui,"聊天软件");
                        if !master {
                            ui.label(egui::RichText::new("提醒总开关已关闭，打开后下面的提醒才会生效").size(12.0).color(Color32::from_rgb(230,150,40)));
                        }
                        let sub=|ui:&mut egui::Ui,title:&str,description:&str,value:&mut bool|{
                            ui.horizontal(|ui|{ui.add_space(22.0);ui.vertical(|ui|setting_row(ui,title,description,value));});
                        };
                        let mut open_connect=false;
                        settings_card(ui,|ui|{
                            ui.add_enabled_ui(master,|ui|{
                                setting_row(ui,"微信","根据任务栏闪动提醒，不读取联系人或聊天正文",&mut self.settings.wechat_notifications);
                                if self.settings.wechat_notifications {
                                    use crate::model::{WechatWakeMethod,WechatWakeShortcut};
                                    ui.horizontal(|ui|{ui.add_space(22.0);ui.vertical(|ui|{
                                        setting_row(ui,"托盘一键唤起","微信缩在托盘时，点提醒直接唤起微信",&mut self.settings.wechat_tray_open);
                                        if !self.settings.wechat_tray_open {return;}
                                        settings_control_row_right(ui,"托盘唤起方式",|ui|{
                                            settings_segmented(ui,&mut self.settings.wechat_wake_method,&[
                                                (WechatWakeMethod::Tray,"系统托盘"),
                                                (WechatWakeMethod::Shortcut,"快捷键优先"),
                                            ],72.0);
                                        });
                                        settings_hint(ui,match self.settings.wechat_wake_method {
                                            WechatWakeMethod::Tray=>"适合微信缩进托盘或同时使用多个账号。Windows 可能会展开隐藏图标。",
                                            WechatWakeMethod::Shortcut=>"适合微信单开且缩进托盘。点击提醒时模拟一次按键，未能唤起时改用系统托盘。",
                                        });
                                        if self.settings.wechat_wake_method==WechatWakeMethod::Shortcut {
                                            settings_control_row(ui,"组合键",SETTINGS_LABEL_W,|ui|{
                                                ui.add(egui::TextEdit::singleline(&mut self.settings.wechat_wake_shortcut)
                                                    .desired_width(160.0).char_limit(48).hint_text("例如 Ctrl+Alt+W"))
                                            });
                                            settings_hint(ui,"填入微信「设置 → 快捷键」里的「显示/隐藏窗口」组合键。微信改键后，请在这里同步更新。");
                                            if !self.settings.wechat_wake_shortcut.trim().is_empty() && WechatWakeShortcut::parse(&self.settings.wechat_wake_shortcut).is_none() {
                                                ui.add(egui::Label::new(egui::RichText::new("当前使用系统托盘。组合键需用 Ctrl、Alt 或 Shift 搭配字母、数字或 F1–F12。").size(11.5).color(Color32::from_rgb(230,150,40))).wrap());
                                            }
                                        }
                                    });});
                                }
                                if master && !self.messenger_status(crate::app::Messenger::Wechat).is_empty(){settings_hint(ui,&self.messenger_status(crate::app::Messenger::Wechat));}
                                ui.separator();
                                setting_row(ui,"企业微信","",&mut self.settings.wecom_notifications);
                                setting_row(ui,"飞书","",&mut self.settings.feishu_notifications);
                                setting_row(ui,"钉钉","",&mut self.settings.dingtalk_notifications);
                                settings_hint(ui,"企业微信、飞书、钉钉需在客户端开启任务栏闪动；点击提醒打开对应窗口。");
                                for client in crate::messenger::Messenger::OFFICE {
                                    if client.enabled(&self.settings) && !self.messenger_status(client).is_empty(){settings_hint(ui,&self.messenger_status(client));}
                                }
                                ui.separator();
                                ui.horizontal(|ui|{
                                    ui.vertical(|ui|{
                                        ui.spacing_mut().item_spacing.y=2.0;
                                        ui.label(egui::RichText::new("QQ 与其他应用").size(13.0).color(settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41))));
                                        ui.label(egui::RichText::new(&self.listener_status).size(11.5).color(settings_color(ctx,Color32::from_gray(175),Color32::from_gray(90))));
                                    });
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                                        if self.settings.read_notifications {
                                            if ui.button(if self.settings.qq_only{"断开 QQ 通知"}else{"断开系统通知"}).clicked(){self.pending.push(Cmd::DisconnectNotifications);}
                                        } else if ui.button("连接…").clicked(){open_connect=true;}
                                    });
                                });
                                sub(ui,"显示通知正文","适用于 QQ、Windows 通知和本机接入；屏幕共享时建议关闭",&mut self.settings.notification_preview);
                                // 收到过通知的应用可能有几十个：默认收起，展开后用可点选的标签排成几行，不再一应用一行。
                                let before_apps=self.notification_apps.items.clone();
                                let total=self.notification_apps.items.len();
                                if total>0 {
                                    let on_count=self.notification_apps.items.iter().filter(|(_,_,enabled)|*enabled).count();
                                    ui.horizontal(|ui|{ui.add_space(22.0);ui.vertical(|ui|{
                                        egui::CollapsingHeader::new(egui::RichText::new(format!("按应用接收（已开 {on_count} / 共 {total}）")).size(12.5)).id_salt("settings-notification-apps").default_open(false).show(ui,|ui|{
                                            ui.horizontal_wrapped(|ui|{
                                                ui.spacing_mut().item_spacing=egui::vec2(6.0,6.0);
                                                for (_,name,enabled) in &mut self.notification_apps.items {
                                                    if settings_toggle_chip(ui,name,*enabled).clicked(){*enabled=!*enabled;}
                                                }
                                            });
                                            settings_hint(ui,"点选切换；绿色为接收，灰框为不接收。应用会在收到通知后出现在这里。");
                                        });
                                    });});
                                }
                                if before_apps!=self.notification_apps.items{self.notification_apps.save();}
                            });
                        });
                        let connect_id=egui::Id::new("settings-notify-connect");
                        if open_connect {ctx.data_mut(|d|d.insert_temp(connect_id,true));}
                        if ctx.data(|d|d.get_temp::<bool>(connect_id)).unwrap_or(false) {
                            let mut close=false;
                            let modal=egui::Modal::new(connect_id.with("modal")).frame(settings_modal_frame(ctx)).backdrop_color(Color32::from_black_alpha(110)).show(ctx,|ui|{
                                ui.set_width(360.0);
                                settings_apply_style(ui);if settings_modal_title(ui,"连接 QQ 与其他应用"){close=true;}
                                if ui.add_sized([ui.available_width(),30.0],egui::Button::new("只连接 QQ")).clicked(){self.pending.push(Cmd::RequestQqNotifications);close=true;}
                                settings_hint(ui,"QQ 需开启系统通知；新通知在岛上显示约 4.5 秒，默认隐藏正文。");
                                ui.add_space(6.0);
                                if ui.add_sized([ui.available_width(),30.0],egui::Button::new("连接所有应用通知")).clicked(){self.pending.push(Cmd::RequestNotifications);close=true;}
                                settings_hint(ui,"只接收 Windows 通知中心可见的内容；应用会在收到通知后出现在列表里。");
                                ui.add_space(8.0);
                                if ui.button("取消").clicked(){close=true;}
                            });
                            if close || modal.should_close() {ctx.data_mut(|d|d.remove::<bool>(connect_id));}
                        }
                        #[cfg(windows)]
                        self.plugin_notice_settings(ui);
                    }
                    7=>{
                        let mut add_mail=false;
                        let on=self.settings_feature_header(ui,crate::app::MAIL_PAGE,"新邮件提醒与验证码；岛上歌词模式显示右侧图标",|_,_|{});
                        ui.add_enabled_ui(on,|ui|{
                        settings_label(ui,"设置");
                        settings_card(ui,|ui|{
                            setting_row(ui,"仅提醒验证码邮件","所有邮箱共用。其他邮件静默收进列表，可随时查看。",&mut self.settings.mail_code_only_notifications);
                            ui.separator();
                            setting_row(ui,"验证码高亮","识别到验证码时在岛上放大显示，点击即复制",&mut self.settings.mail_code);
                            ui.separator();
                            ui.add_enabled_ui(self.settings.mail_code,|ui|{
                                setting_row(ui,"避让时弹出验证码","自动避让时短暂显示新收到的验证码，点击即可复制",&mut self.settings.mail_code_popup);
                            });
                            ui.separator();
                            let sync_before=self.settings.mail_sync_operations;
                            ui.add_enabled_ui(self.mail.manage.pending.is_empty(),|ui|{
                                setting_row(ui,"同步邮件操作到邮箱","已读、删除和恢复同步到邮箱；关闭时只整理岛仔里的记录，切换不会补交之前的操作",&mut self.settings.mail_sync_operations);
                            });
                            if sync_before!=self.settings.mail_sync_operations{self.mail_manage_mode_changed();}
                            ui.separator();
                            ui.horizontal(|ui|{
                                ui.vertical(|ui|{
                                    ui.spacing_mut().item_spacing.y=2.0;
                                    ui.label(egui::RichText::new("会议提前").size(13.0).color(settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41))));
                                    ui.label(egui::RichText::new(format!("{} 分钟 · 与日程共用",self.settings.mail_ics)).size(11.5).color(settings_color(ctx,Color32::from_gray(175),Color32::from_gray(90))));
                                });
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{if ui.button("在日程中设置 ›").clicked(){self.settings_tab=8;}});
                            });
                            settings_advanced(ui,"settings-mail-advanced",|ui|{
                                setting_choices(ui,"验证码隐藏",&mut self.settings.mail_code_hide_seconds,[30,60,300],"秒");
                                settings_hint(ui,"明确有效时长更短时提前隐藏；隐藏不代表失效，主动查看后 15 秒再次隐藏。");
                                setting_choices(ui,"检查间隔",&mut self.settings.mail_interval,[60,120,300],"秒");
                                settings_hint(ui,"支持推送的邮箱实时收信；间隔只用于不支持推送的邮箱。");
                            });
                        });
                        ui.horizontal(|ui|{
                            settings_label(ui,"邮箱账号");
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::BOTTOM),|ui|{if ui.button("＋ 添加邮箱").clicked(){add_mail=true;}});
                        });
                        settings_card(ui,|ui|{
                            if self.mail.accounts.is_empty(){ui.label(egui::RichText::new("还没有邮箱。点「＋ 添加邮箱」，保存后自动连接。").size(12.0).color(settings_color(ctx,Color32::from_gray(195),Color32::from_gray(60))));}
                            let total=self.mail.accounts.len();
                            let(mut changed,mut remove,mut test)=(false,None,None);
                            for i in 0..total{
                                let(email,provider)=(self.mail.accounts[i].email.clone(),self.mail.accounts[i].provider);
                                let status=self.mail.status_of(&email).label();
                                let mut on=self.mail.accounts[i].enabled;
                                if i>0{ui.separator();}
                                ui.label(egui::RichText::new(&email).size(13.0).color(settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41))));
                                ui.label(egui::RichText::new(format!("{} · {}",provider.label(),status)).size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                                if provider == crate::mail::Provider::Gmail
                                    && mail_connection_choice(ui, ("mail-account-connection", email.as_str()), &mut self.mail.accounts[i].use_windows_proxy)
                                {
                                    changed = true;
                                    self.mail.note = format!("{email}：连接方式已更新");
                                }
                                ui.horizontal(|ui|{
                                    if ui.add_sized([66.0,30.0],egui::Button::new("测试")).clicked(){test=Some(i);}
                                    if ui.add_sized([66.0,30.0],egui::Button::new("删除")).clicked(){remove=Some(i);}
                                    ui.add_space(8.0);
                                    ui.label(egui::RichText::new("启用").size(12.0).color(settings_color(ctx,Color32::from_gray(190),Color32::from_gray(65))));
                                    if switch(ui,&mut on){self.mail.accounts[i].enabled=on;changed=true;}
                                });
                            }
                            if changed{self.mail_accounts_changed();}
                            if let Some(i)=remove{
                                let email=self.mail.accounts[i].email.clone();
                                let credential=self.mail.accounts[i].credential_key();
                                self.mail.accounts.remove(i);
                                crate::mail::secrets::delete(&credential);
                                self.mail.status.remove(&email.to_ascii_lowercase());
                                self.mail.note=format!("{email}：已移除，凭据已从系统凭据管理器删除");
                                self.mail_accounts_changed();
                            }
                            if let Some(i)=test{
                                let account=self.mail.accounts[i].clone();
                                if let Err(error)=account.endpoint(){
                                    self.mail.note=error;
                                }else{
                                match crate::mail::secrets::load(&account.credential_key()){
                                    Ok(pass)=>{self.mail.testing=Some(account.email.clone());self.mail.note=format!("{}：正在测试连接…",account.email);self.pending.push(Cmd::MailTest(account,pass));}
                                    Err(e)=>self.mail.note=format!("{}：{e}",account.email),
                                }
                                }
                            }
                                                    if !self.mail.note.is_empty(){ui.add_space(3.0);ui.add(egui::Label::new(egui::RichText::new(&self.mail.note).size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75)))).wrap());}
                        });
                        });
                        #[cfg(feature="sdk-js")]
                        self.plugins.settings(ui,"mail",on);
                        // 添加邮箱：一次性的长表单放进弹窗，页面上只保留账号列表。
                        let form_id=egui::Id::new("settings-mail-add");
                        if add_mail {self.mail.form.open=true;}
                        if self.mail.form.open {
                            let mut close=false;
                            let modal=egui::Modal::new(form_id).frame(settings_modal_frame(ctx)).backdrop_color(Color32::from_black_alpha(110)).show(ctx,|ui|{
                            ui.set_width(440.0);
                            settings_apply_style(ui);if settings_modal_title(ui,"添加邮箱"){close=true;}
                            ui.spacing_mut().item_spacing.y=8.0;
                            settings_control_row(ui,"邮箱服务",56.0,|ui|{
                                egui::ComboBox::from_id_salt("mail-provider").width(160.0).selected_text(self.mail.form.provider.label()).show_ui(ui,|ui|{
                                    for p in crate::mail::Provider::ALL{if ui.selectable_label(self.mail.form.provider==p,p.label()).clicked(){self.mail.form.provider=p;}}
                                }).response
                            });
                            ui.add_space(5.0);
                            let microsoft=self.mail.form.provider==crate::mail::Provider::Microsoft;
                            ui.label(egui::RichText::new(if microsoft{"连接微软邮箱"}else{"获取授权码或专用密码"}).size(12.5).strong());
                            ui.add(egui::Label::new(egui::RichText::new(self.mail.form.provider.hint()).size(12.0).color(settings_color(ctx,Color32::from_gray(200),Color32::from_gray(55)))).wrap());
                            let (label,url)=self.mail.form.provider.setup_link();
                            if !url.is_empty() && ui.link(label).clicked(){self.pending.push(Cmd::OpenUrl(url.into()));}
                            ui.add_space(8.0);
                            if self.mail.form.provider==crate::mail::Provider::Custom {
                                settings_control_row(ui,"IMAP 主机",80.0,|ui|{ui.text_edit_singleline(&mut self.mail.form.custom.host)});
                                settings_control_row(ui,"TLS 端口",80.0,|ui|{ui.add(egui::DragValue::new(&mut self.mail.form.custom.port).range(1..=65535))});
                                settings_control_row(ui,"登录用户名",80.0,|ui|{ui.add(egui::TextEdit::singleline(&mut self.mail.form.custom.username).vertical_align(egui::Align::Center).hint_text("留空使用邮箱地址"))});
                            }
                            if !microsoft{
                            let input_width=(ui.available_width()-56.0-48.0-2.0*ui.spacing().item_spacing.x).max(100.0);
                            settings_control_row(ui,"邮箱",56.0,|ui|{
                                ui.add_sized([input_width,30.0],egui::TextEdit::singleline(&mut self.mail.form.email).vertical_align(egui::Align::Center).hint_text("输入邮箱地址"))
                            });
                            let gmail = self.mail.form.provider == crate::mail::Provider::Gmail;
                            if gmail {
                                mail_connection_choice(ui, "mail-form-connection", &mut self.mail.form.use_windows_proxy);
                            }
                            let target = crate::mail::Account {
                                custom: (self.mail.form.provider == crate::mail::Provider::Custom).then(|| self.mail.form.custom.clone()),
                                email: self.mail.form.email.trim().to_owned(),
                                provider: self.mail.form.provider,
                                enabled: true,
                                use_windows_proxy: gmail && self.mail.form.use_windows_proxy,
                            };
                            let unsupported=target.endpoint().err().as_deref()==Some(crate::mail::MICROSOFT_UNSUPPORTED);
                            if unsupported{ui.add(egui::Label::new(crate::mail::MICROSOFT_UNSUPPORTED).wrap());}
                            ui.add_enabled_ui(!unsupported,|ui|{
                            settings_control_row(ui,"授权码",56.0,|ui|{
                                let field=ui.add_sized([input_width,30.0],egui::TextEdit::singleline(&mut self.mail.form.password).vertical_align(egui::Align::Center).password(!self.mail.form.show).hint_text("输入邮箱授权码"));
                                if ui.add_sized([48.0,30.0],egui::Button::new(if self.mail.form.show{"隐藏"}else{"显示"})).clicked(){self.mail.form.show=!self.mail.form.show;}
                                field
                            });
                            ui.horizontal(|ui|{
                                let test = ui.button("测试连接").clicked();
                                let save = ui.button("保存并开始监控").clicked();
                                if test || save {
                                    let email = target.email.clone();
                                    if !email.contains('@'){self.mail.note="请先填邮箱地址".into();}
                                    else if let Err(error)=target.endpoint(){self.mail.note=error;}
                                    else if self.mail.form.password.is_empty(){self.mail.note="请先填授权码（不是邮箱登录密码）".into();}
                                    else if test {
                                        self.mail.testing=Some(email.clone());
                                        self.mail.note=format!("{email}：正在测试连接…");
                                        self.pending.push(Cmd::MailTest(target, self.mail.form.password.clone()));
                                    } else {
                                        match crate::mail::secrets::save(&target.credential_key(),&self.mail.form.password){
                                            Ok(())=>{
                                                if let Some(slot)=self.mail.accounts.iter_mut().find(|a|a.email.eq_ignore_ascii_case(&email)){*slot=target;}else{self.mail.accounts.push(target);}
                                                self.mail.form.password.clear();
                                                self.mail.note=format!("{email}：已保存，正在连接…");
                                                self.mail_accounts_changed();
                                            }
                                            Err(e)=>self.mail.note=e,
                                        }
                                    }
                                }
                            });
                            ui.label(egui::RichText::new("授权码存储在 Windows 凭据管理器中。").size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                            });
                            }
                                                    if !self.mail.note.is_empty(){ui.add_space(3.0);ui.add(egui::Label::new(egui::RichText::new(&self.mail.note).size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75)))).wrap());}
                            });
                            // 保存后弹窗保持打开，结果和「测试连接」都在这里；用户自己关闭。
                            if close || modal.should_close() {
                                self.mail.form.open = false;
                                self.mail.form.use_windows_proxy = false;
                            }
                        }
                    }
                    8=>{
                        ui.spacing_mut().item_spacing.y=4.0;
                        let mut open_add=false;
                        let on=self.settings_feature_header(ui,crate::app::CALENDAR_PAGE,"月历、邮件邀请与工作日程",|_,_|{});
                        ui.add_enabled_ui(on,|ui|{
                        settings_label(ui,"设置");
                        settings_card(ui,|ui|{
                            setting_row(ui,"自动同步","每 5 分钟更新日程；关闭后可点「立即同步」",&mut self.settings.calendar_auto_sync);
                            ui.separator();
                            setting_choices(ui,"会议提前",&mut self.settings.mail_ics,[5,15,30],"分钟");
                            settings_hint(ui,"与邮件中的会议邀请共用提醒，同一场会议只提醒一次。");
                        });
                        #[cfg(feature="sdk-js")] self.plugins.settings(ui,"calendar",on);
                        settings_label(ui,"日历账号");
                        let mut import=false;
                        settings_card(ui,|ui|{
                        if let Some(cfg)=self.calendar.config.clone(){
                                let n=self.calendar.events.len();
                                // 一行：平台图标、名称与状态，右侧立即同步和断开。
                                ui.horizontal(|ui|{
                                    let (rect,_)=ui.allocate_exact_size(egui::vec2(28.0,28.0),egui::Sense::hover());
                                    ui.painter().rect_filled(rect,7.0,Color32::from_rgb(51,112,255));
                                    ui.painter().text(rect.center(),egui::Align2::CENTER_CENTER,cfg.provider.label().chars().next().unwrap_or('日'),egui::FontId::proportional(13.0),Color32::WHITE);
                                    ui.vertical(|ui|{
                                        ui.spacing_mut().item_spacing.y=2.0;
                                        ui.label(egui::RichText::new(format!("{} · 最后同步 {}",cfg.provider.label(),if self.calendar.last_sync.is_empty(){"—".to_string()}else{self.calendar.last_sync.clone()})).size(13.0).color(settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41))));
                                        let status = if self.calendar.connected {
                                            if n == 0 && !self.calendar.warning.is_empty() { "● 已连接 · 日程同步不完整".into() }
                                            else { format!("● 已连接 · 未来 24 小时 {n} 个日程") }
                                        } else if self.calendar.failures == 0 { "● 正在连接…".into() }
                                        else { format!("● 上次连接失败 · 已知 {n} 个日程") };
                                        ui.label(egui::RichText::new(status).size(11.5).color(if self.calendar.connected{settings_color(ctx,Color32::from_rgb(120,220,160),Color32::from_rgb(24,121,64))}else{settings_color(ctx,Color32::from_rgb(240,180,110),Color32::from_rgb(145,89,12))}));
                                    });
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                                        if ui.add_enabled(!self.calendar.testing,egui::Button::new("断开")).clicked(){self.pending.push(Cmd::CalendarForget);}
                                        if ui.button("立即同步").clicked(){self.calendar.note="正在同步…".into();self.calendar.note_error=false;self.pending.push(Cmd::CalendarSync);}
                                    });
                                });
                                if let Some(feed)=self.calendar.feeds.iter().find(|feed|feed.config==cfg) {
                                    let status=if feed.month.loading {"正在同步本月日程…".to_owned()}
                                        else if !feed.month.error.is_empty(){format!("本月同步失败：{}",feed.month.error)}
                                        else if feed.month.loaded==Some(self.calendar.view.month){format!("{} {} {} 个日程",self.calendar.view.month.format("%Y-%m"),if feed.month.warning.is_empty(){"已同步"}else{"已读取"},feed.month.events.len())}
                                        else {String::new()};
                                    if !status.is_empty(){ui.add(egui::Label::new(egui::RichText::new(status).size(12.0)).wrap());}
                                    if !feed.month.warning.is_empty(){ui.add(egui::Label::new(egui::RichText::new(format!("本月部分日程未同步：{}",feed.month.warning)).size(12.0).color(settings_color(ctx,Color32::from_rgb(238,181,91),Color32::from_rgb(139,88,12)))).wrap());}
                                }
                                if !self.calendar.warning.is_empty(){ui.add(egui::Label::new(egui::RichText::new(format!("同步提示：{}",self.calendar.warning)).size(12.0).color(settings_color(ctx,Color32::from_rgb(238,181,91),Color32::from_rgb(139,88,12)))).wrap());}
                                if !self.calendar.error.is_empty(){
                                    ui.add(egui::Label::new(egui::RichText::new(&self.calendar.error).size(12.0).color(settings_color(ctx,Color32::from_rgb(240,96,96),Color32::from_rgb(184,34,34)))).wrap());
                                }
                                ui.separator();
                        }else{
                            ui.label(egui::RichText::new("还没有连接日历。添加飞书、钉钉等日历，或导入 .ics 文件。").size(12.0).color(settings_color(ctx,Color32::from_gray(195),Color32::from_gray(60))));
                        }
                        ui.horizontal(|ui|{
                            if ui.button(if self.calendar.config.is_some(){"更换日历"}else{"＋ 添加日历"}).clicked(){open_add=true;}
                            if ui.button("导入 .ics 文件").clicked(){import=true;}
                        });
                        if !self.calendar.import_note.is_empty(){settings_hint(ui,&self.calendar.import_note);}
                        });
                        if import {self.pending.push(Cmd::CalendarImport);}
                        });
                        // 添加日历：选平台、填账号、获取方法、导入 .ics 都在弹窗里。
                        let add_id=egui::Id::new("settings-calendar-add");
                        if open_add {self.calendar.form.open=true;self.calendar.form.guide_open=true;}
                        if self.calendar.form.open {
                            let mut close=false;
                            let connected_before=self.calendar.config.clone();
                            let modal=egui::Modal::new(add_id.with("modal")).frame(settings_modal_frame(ctx)).backdrop_color(Color32::from_black_alpha(110)).show(ctx,|ui|{
                            ui.set_width(460.0);
                            ui.spacing_mut().item_spacing.y=6.0;
                            settings_apply_style(ui);if settings_modal_title(ui,"添加日历"){close=true;}
                            settings_label(ui,"选择日历平台");
                            egui::Frame::new().fill(settings_color(ctx,Color32::from_rgb(29,29,31),Color32::from_rgb(236,239,244))).corner_radius(8.0).inner_margin(3).show(ui,|ui|{
                                ui.set_min_width(ui.available_width());
                                ui.spacing_mut().item_spacing.x=2.0;
                                // 六个平台分两行：飞书 / 钉钉 / 企业微信，iCloud / CalDAV / Exchange。
                                let width=((ui.available_width()-4.0)/3.0).max(80.0);
                                for row in crate::calendar::Provider::ALL.chunks(3){
                                    ui.horizontal(|ui|{
                                        for &p in row{
                                            let selected=self.calendar.form.provider==p;
                                            let button=egui::Button::new(egui::RichText::new(p.label()).size(13.0).color(if selected{settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41))}else{settings_color(ctx,Color32::from_gray(190),Color32::from_gray(65))}))
                                                .fill(if selected{settings_color(ctx,Color32::from_rgb(81,81,85),Color32::from_rgb(208,217,231))}else{Color32::TRANSPARENT})
                                                .stroke(egui::Stroke::NONE).corner_radius(6.0);
                                            let button=ui.add_enabled_ui(!self.calendar.testing,|ui|ui.add_sized([width,30.0],button)).inner;
                                            let button=match p{
                                                crate::calendar::Provider::ICloud=>button.on_hover_text("苹果日历：Apple ID + App 专用密码"),
                                                crate::calendar::Provider::Custom=>button.on_hover_text("任意 CalDAV 日历：Nextcloud、群晖、Fastmail 等"),
                                                crate::calendar::Provider::Exchange=>button.on_hover_text("公司自建或第三方 Exchange 日历（ActiveSync），只读同步"),
                                                _=>button,
                                            };
                                            if button.clicked() && !selected{
                                                self.select_calendar_provider(p);
                                            }
                                        }
                                    });
                                }
                            });
                            ui.add_space(7.0);
                            ui.horizontal_top(|ui|{
                                ui.allocate_ui_with_layout(egui::vec2(50.0,18.0),egui::Layout::left_to_right(egui::Align::Center),|ui|{ui.set_min_size(egui::vec2(50.0,18.0));ui.label(egui::RichText::new("服务器").size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));});
                                if self.calendar.form.provider.custom_server(){
                                    ui.add_sized([ui.available_width(),22.0],egui::TextEdit::singleline(&mut self.calendar.form.server).vertical_align(egui::Align::Center)
                                        .hint_text(egui::RichText::new(self.calendar.form.provider.server_hint()).color(settings_color(ctx,Color32::from_gray(155),Color32::from_gray(100)))));
                                }else{
                                    ui.add(egui::Label::new(egui::RichText::new(self.calendar.form.provider.server()).size(12.0).color(settings_color(ctx,Color32::from_gray(205),Color32::from_gray(55)))).wrap());
                                }
                            });
                            settings_label(ui,"连接账号");
                            ui.scope(|ui|{
                                ui.visuals_mut().extreme_bg_color=settings_color(ctx,Color32::from_rgb(25,25,28),Color32::from_rgb(244,246,249));
                            let input_width=(ui.available_width()-50.0-48.0-2.0*ui.spacing().item_spacing.x).max(80.0);
                                ui.horizontal(|ui|{
                                    ui.allocate_ui_with_layout(egui::vec2(50.0,30.0),egui::Layout::left_to_right(egui::Align::Center),|ui|{ui.set_min_size(egui::vec2(50.0,30.0));ui.label(egui::RichText::new("用户名").size(13.0).color(settings_color(ctx,Color32::from_gray(210),Color32::from_gray(55))));});
                                    let user=ui.add_sized([input_width,30.0],egui::TextEdit::singleline(&mut self.calendar.form.username).vertical_align(egui::Align::Center)
                                        .hint_text(egui::RichText::new("输入用户名或粘贴账号信息").color(settings_color(ctx,Color32::from_gray(155),Color32::from_gray(100)))));
                                    if user.changed(){
                                        let pasted=self.calendar.form.username.clone();
                                        if let Some((u,p))=crate::calendar::parse_paste(&pasted){
                                            self.calendar.form.username=u;
                                            self.calendar.form.password=p;
                                            self.calendar.note="已从粘贴内容中填入用户名和密码".into();
                                            self.calendar.note_error=false;
                                        }
                                    }
                                });
                                ui.add_space(5.0);
                                ui.horizontal(|ui|{
                                    ui.allocate_ui_with_layout(egui::vec2(50.0,30.0),egui::Layout::left_to_right(egui::Align::Center),|ui|{ui.set_min_size(egui::vec2(50.0,30.0));ui.label(egui::RichText::new("密码").size(13.0).color(settings_color(ctx,Color32::from_gray(210),Color32::from_gray(55))));});
                                    ui.add_sized([input_width,30.0],egui::TextEdit::singleline(&mut self.calendar.form.password).vertical_align(egui::Align::Center)
                                        .password(!self.calendar.form.show).hint_text(egui::RichText::new("输入平台生成的密码").color(settings_color(ctx,Color32::from_gray(155),Color32::from_gray(100)))));
                                    if ui.add_sized([48.0,30.0],egui::Button::new(if self.calendar.form.show{"隐藏"}else{"显示"})).clicked(){self.calendar.form.show=!self.calendar.form.show;}
                                });
                            });
                            ui.add_space(7.0);
                            let guide=if self.calendar.form.guide_open{"如何获取用户名和密码？（点击收起）"}else{"如何获取用户名和密码？"};
                            if ui.link(egui::RichText::new(guide).color(settings_color(ctx,Color32::from_rgb(120,185,245),Color32::from_rgb(26,100,177)))).clicked(){self.calendar.form.guide_open=!self.calendar.form.guide_open;}
                            if self.calendar.form.guide_open{
                                ui.add_space(4.0);
                                for(i,step)in self.calendar.form.provider.guide().iter().enumerate(){
                                    ui.label(egui::RichText::new(format!("{}. {}",i+1,step)).size(12.0).color(settings_color(ctx,Color32::from_gray(200),Color32::from_gray(55))));
                                }
                            }
                            ui.add_space(9.0);
                            let busy=self.calendar.testing;
                            let label=if busy{"正在测试连接…"}else{"测试并保存"};
                            let btn=egui::Button::new(egui::RichText::new(label).size(13.0).strong().color(Color32::WHITE))
                                .fill(if busy{settings_color(ctx,Color32::from_rgb(70,70,76),Color32::from_rgb(213,219,227))}else{Color32::from_rgb(45,105,165)})
                                .corner_radius(7.0);
                            if ui.add_enabled_ui(!busy,|ui|ui.add_sized([130.0,32.0],btn)).inner.clicked(){
                                let user=self.calendar.form.username.trim().to_string();
                                let custom=self.calendar.form.provider.custom_server();
                                let server=crate::calendar::normalize_server(&self.calendar.form.server);
                                if custom && server.is_none(){self.calendar.note="请填写以 https:// 开头的 CalDAV 服务器地址".into();self.calendar.note_error=true;}
                                else if user.is_empty(){self.calendar.note="请先填用户名".into();self.calendar.note_error=true;}
                                else if self.calendar.form.password.is_empty(){self.calendar.note="请先填密码".into();self.calendar.note_error=true;}
                                else{
                                    let cfg=crate::calendar::Config{provider:self.calendar.form.provider,username:user,href:None,server:if custom{server}else{None}};
                                    self.calendar.testing=true;
                                    self.calendar.test_password=self.calendar.form.password.clone();
                                    self.calendar.note="正在连接日历…".into();
                                    self.calendar.note_error=false;
                                    self.pending.push(Cmd::CalendarTest(cfg,self.calendar.form.password.clone()));
                                }
                            }
                            ui.add_space(5.0);
                            ui.label(egui::RichText::new("连接成功后保存，密码存储在 Windows 凭据管理器中。").size(11.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                            if !self.calendar.note.is_empty(){
                                ui.add_space(5.0);
                                let color=if self.calendar.note_error{settings_color(ctx,Color32::from_rgb(255,125,125),Color32::from_rgb(184,34,34))}else{settings_color(ctx,Color32::from_gray(210),Color32::from_gray(55))};
                                ui.add(egui::Label::new(egui::RichText::new(&self.calendar.note).size(12.0).color(color)).wrap());
                            }
                            });
                            // 连接成功（日历配置变化）后自动关闭弹窗。
                            if self.calendar.config.is_some() && self.calendar.config!=connected_before && !self.calendar.testing {close=true;}
                            if close || modal.should_close() {self.calendar.form.open=false;}
                        }
                    }
                    _=>{
                        settings_label(ui,"岛仔 Daozai");
                        settings_card(ui,|ui|{
                            ui.horizontal(|ui|{
                                ui.label(egui::RichText::new(format!("版本 {}",env!("CARGO_PKG_VERSION"))).size(13.0));
                                ui.add_space(8.0);
                                ui.vertical(|ui|self.updates.ui(ui));
                            });
                            ui.label(egui::RichText::new("桌面顶部的音乐、专注、提醒与日程小助手。").size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                            ui.horizontal_wrapped(|ui|{
                                if ui.button("打开本地数据目录").clicked(){self.pending.push(Cmd::Open(crate::model::config_dir()));}
                                if ui.button("收集诊断").clicked(){
                                    // 另起 `daozai.exe --collect-diagnostics`：在新窗口里收集，诊断包保存到桌面并自动选中。
                                    if crate::diagnostics_tool::launch().is_err(){self.say("无法启动诊断收集，请从开始菜单打开「收集岛仔诊断」");}
                                }
                                ui.label(egui::RichText::new("设备图标：Solar Icons · 480 Design").size(11.0).color(settings_color(ctx,Color32::from_gray(150),Color32::from_gray(105))));
                                ui.hyperlink_to("CC BY 4.0", "https://creativecommons.org/licenses/by/4.0/");
                            });
                        });
                        settings_label(ui,"支持作者");
                        settings_card(ui,|ui|support_author(ui,&mut self.supporters));
                    }
                    }
                });
            });
            self.avoidance.dialog(ctx,&mut self.settings);
            if before!=format!("{:?}",self.settings){self.settings.save();}
            ctx.set_style(original_style);
        });
        if !self.settings_open && self.window_layout().close(){release_support_images(ctx);}
        if self.settings_open&&self.module_on(crate::app::WEATHER_PAGE) {self.weather_tutorial_ui(ctx);}
        else {self.weather_source.close();}
    }
}

/// 收款码只在「支持作者」卡片展开时占用贴图；收起卡片或关闭设置窗口即释放。
fn release_support_codes(ctx:&egui::Context){
    ctx.data_mut(|data|for i in 0..2usize {data.remove::<egui::TextureHandle>(egui::Id::new(("support-author-qr",i)));});
}

/// 从原图裁出二维码区域 `[x, y, 边长]` 并缩放到 `pixels` 见方。
fn support_code_image(bytes:&[u8],crop:[f32;3],pixels:u32)->egui::ColorImage {
    let image=image::load_from_memory(bytes).expect("内置收款码图片有效");
    let side=(crop[2] as u32).min(image.width().saturating_sub(crop[0] as u32)).min(image.height().saturating_sub(crop[1] as u32));
    let code=image.crop_imm(crop[0] as u32,crop[1] as u32,side,side).resize_exact(pixels,pixels,image::imageops::FilterType::Lanczos3).to_rgba8();
    egui::ColorImage::from_rgba_unmultiplied([pixels as usize,pixels as usize],code.as_raw())
}

#[test]
fn support_codes_are_cropped_small_and_released_when_hidden(){
    for (bytes,crop) in [(include_bytes!("../assets/support/support-wechat-original.jpg").as_slice(),[330.0,370.0,600.0]),(include_bytes!("../assets/support/support-alipay-original.jpg").as_slice(),[200.0,660.0,860.0])] {
        let image=support_code_image(bytes,crop,360);
        assert_eq!(image.size,[360,360]);
        assert!(image.pixels.len()*4<=600*1024,"每张收款码贴图应小于 0.6MB");
        // 裁剪结果应是二维码本身：同时含有大量深色与浅色像素。
        let dark=image.pixels.iter().filter(|p|p.r()<80).count() as f32/image.pixels.len() as f32;
        assert!((0.2..0.8).contains(&dark),"裁剪区域不像二维码：深色占比 {dark}");
    }
    let ctx=egui::Context::default();
    let _=ctx.run(Default::default(),|ctx|{
        let texture=ctx.load_texture("qr",egui::ColorImage::new([4,4],Color32::WHITE),Default::default());
        ctx.data_mut(|data|data.insert_temp(egui::Id::new(("support-author-qr",0usize)),texture));
    });
    assert_eq!(ctx.tex_manager().read().allocated().filter(|(_,meta)|meta.name=="qr").count(),1);
    release_support_codes(&ctx);
    let _=ctx.run(Default::default(),|_|{});
    assert_eq!(ctx.tex_manager().read().allocated().filter(|(_,meta)|meta.name=="qr").count(),0,"收起后应释放收款码贴图");
}
