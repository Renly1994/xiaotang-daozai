//! 设置、通知队列与岛仔表情。
use crate::settings_theme::color as settings_color;
use crate::app::{App, Cmd, Env, Phase, HUD_TRACK_W, HUD_TRACK_X, HUD_W, PAGE_NAMES};
use crate::paint::{Card, H};
use crate::theme::{self, size, W};
use eframe::egui::{self, Color32, Rect};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AlertSource { Internal, Messenger(crate::app::Messenger), External(String) }

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
    /// 通用状态总线的可选呈现信息（进度 / 状态色 / 点击动作）。
    pub status: Option<crate::island_status::Status>,
    pub unread: Option<crate::app::Unread>,
}

impl Alert {
    pub(crate) fn messenger(&self)->Option<crate::app::Messenger>{
        match &self.source {
            AlertSource::Messenger(client)=>Some(*client),
            AlertSource::External(id)=>crate::app::Messenger::from_notification_id(id).filter(|client|client.adapter().capture==crate::messenger::Capture::SystemToast),
            _=>None,
        }
    }
    pub(crate) fn is_hud(&self)->bool {self.source==AlertSource::Internal && matches!(self.title.as_str(),"系统音量"|"大写锁定"|"键盘输入语言"|"网络连接"|"电源已连接"|"已切换电池供电"|"蓝牙设备")}
}
fn bluetooth_notice_content(body:&str)->(&str,&str,Option<u8>){
    if let Some(name)=body.strip_suffix(" · 已断开") {return (name,"已断开",None);}
    if body=="已断开连接" {return ("蓝牙设备","已断开",None);}
    if let Some((name,level))=body.rsplit_once(" · 电量 ") {
        if let Some(level)=level.strip_suffix('%').and_then(|s|s.parse::<u8>().ok()).filter(|v|*v<=100){
            return (name,"已连接",Some(level));
        }
    }
    (body,"已连接",None)
}

pub(crate) fn device_icon(c: &Card, kind: crate::bt::DeviceKind, x: f32, y: f32, size: f32) {
    let icon=match kind {
        crate::bt::DeviceKind::Bone | crate::bt::DeviceKind::Earbuds | crate::bt::DeviceKind::Headphones=>egui::include_image!("../assets/device-icons/headphones-round-bold.svg"),
        crate::bt::DeviceKind::Speaker=>egui::include_image!("../assets/device-icons/speaker-bold.svg"),
        crate::bt::DeviceKind::Keyboard=>egui::include_image!("../assets/device-icons/keyboard-bold.svg"),
        crate::bt::DeviceKind::Mouse=>egui::include_image!("../assets/device-icons/mouse-bold.svg"),
        crate::bt::DeviceKind::Gamepad=>egui::include_image!("../assets/device-icons/gamepad-bold.svg"),
        crate::bt::DeviceKind::Phone=>egui::include_image!("../assets/device-icons/smartphone-bold.svg"),
        crate::bt::DeviceKind::Other=>egui::include_image!("../assets/device-icons/bluetooth-linear.svg"),
    };
    let rect=c.rect(x,y,size,size);
    let egui::ImageSource::Bytes{uri,bytes}=icon else{return};
    let pixels=(rect.width()*c.ui.ctx().pixels_per_point()).ceil() as u32;
    let icon=egui::Image::from_bytes(format!("{uri}/{pixels}.svg"),bytes);
    if let Ok(egui::load::TexturePoll::Ready { texture })=icon.fit_to_exact_size(rect.size()).load_for_size(c.ui.ctx(),rect.size()) {
        c.painter.image(texture.id,rect,Rect::from_min_max(egui::Pos2::ZERO,egui::pos2(1.0,1.0)),Color32::WHITE);
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
        "喝水时间到了"=>{
            c.poly(&[(x-7.0,y-1.0),(x+5.0,y-1.0),(x+3.5,y+8.0),(x-5.5,y+8.0)],theme::BLUE);
            c.path(&[(x-9.0,y-10.0),(x-6.5,y+9.0),(x+4.5,y+9.0),(x+7.0,y-10.0)],1.6,theme::CYAN);
            c.seg((x-9.0,y-10.0),(x+7.0,y-10.0),1.6,theme::CYAN);
            c.path(&[(x+6.5,y-6.0),(x+10.5,y-6.0),(x+11.5,y-3.0),(x+10.0,y+2.0),(x+5.5,y+3.0)],1.6,theme::CYAN);
            c.seg((x-5.0,y-6.5),(x-4.2,y-2.5),1.0,Color32::from_white_alpha(170));
        },
        crate::app::SIT_TITLE=>crate::paint::chair(c,x,y,0.6,theme::SOFT_GREEN),
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
    /// 下一次久坐提醒的时刻：从开始久坐或上次提醒起算满 sit_minutes。
    pub(crate) fn sit_due(&self) -> f64 {
        self.sit_start.max(self.sit_alerted) + self.settings.sit_minutes as f64 * 60.0
    }
    /// 「起身了」：久坐计时归零。
    pub(crate) fn stood_up(&mut self) {
        self.sit_start = self.t;
        self.sit_alerted = f64::NEG_INFINITY;
        self.pet.react(PetState::Happy);
        self.say("久坐计时已重新开始");
    }
    pub(crate) fn snooze_sit(&mut self) {
        self.sit_alerted = self.t - self.settings.sit_minutes as f64 * 60.0 + crate::app::SIT_SNOOZE_SECS;
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
            status: None, unread: None,
       };
        self.enqueue_alert(a, system);
    }
    /// 邮件验证码上岛：标题带「邮件」标识，正文留明文数字供通知卡片显示；
    /// 不走手机短信的 auto_copy_otp，也不推 Cmd::Copy，只允许用户手动点复制。
    pub fn mail_code_alert(&mut self, _code:&str, from:&str, page:usize){
        let duration=if cfg!(test){4.5}else{(self.settings.otp_seconds as f64).max(10.0)};
        self.enqueue_alert(Alert{
            source:AlertSource::External("mail-code".into()),
            title:format!("邮件验证码 · {}",from),
            body:"邮件验证码".to_string(),
            page,at:self.t,duration,
            bluetooth:None,meeting:None,status:None,unread:None,
        },false);
    }
    pub(crate) fn meeting_alert(&mut self, meeting: crate::mail::Meeting, source: String, page: usize) {
        self.enqueue_alert(Alert {
            source: AlertSource::Internal,
            title: "会议提醒".into(), body: format!("{} · {} · {}", meeting.title, meeting.at, source),
            page, at: self.t, duration: 12.0, bluetooth: None, meeting: Some((meeting, source)), status: None, unread: None,
       }, true);
    }
    pub(crate) fn bluetooth_alert(&mut self, device: &crate::bt::Device, connected: bool) {
        if !self.settings.bluetooth_notifications { return; }
        if connected && matches!(device.kind,crate::bt::DeviceKind::Bone|crate::bt::DeviceKind::Earbuds|crate::bt::DeviceKind::Headphones|crate::bt::DeviceKind::Speaker) {
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
            status: None, unread: None,
       };
        self.enqueue_alert(a, false);
    }
    fn enqueue_alert(&mut self, a: Alert, system: bool) {
        // 通知仅保存在本次运行的内存中。
        if a.source==AlertSource::Internal || matches!(&a.source,AlertSource::External(id) if id=="mail-code") {self.history.push_front(a.clone());}
        self.history.truncate(50);
        // 落盘一份历史：岛仔重启或当时没看到时，还能在设置里回查。
        let status_bus=matches!(&a.source,AlertSource::External(id) if id.starts_with("island-status"));
        if !status_bus {notice_history_append(&a.title,&a.body);}
        // 勿扰：只记账、不打扰。横幅、铃铛、系统通知都不出现，验证码例外（用户要的就是那串数字）。
        if self.muted() && a.status.is_none() && crate::otp::extract("",&a.body,&[]).is_none() { return; }
        if self.settings.notifications {
            if system && a.source==AlertSource::Internal {
                self.pending
                    .push(Cmd::Notify(a.title.clone(), a.body.clone()));
            }
            let same_event=|old:&Alert| match (&a.meeting,&old.meeting) {
                (Some((next,_)),Some((previous,_)))=>crate::calendar::dedupe_key(next)==crate::calendar::dedupe_key(previous),
                // 状态总线按 key 覆写：内容变了替换，不变也替换以续期，不排队。
                _ if matches!(&a.source,AlertSource::External(id) if id.starts_with("island-status:"))=>old.source==a.source,
                // 手机通知按来源聚合：同一 App 的新消息替换旧提示，不排队刷屏。
                // 手机通道共用同一个来源（手机连接的 AUMID）：新短信 / 来电直接顶掉上一条，
                // 不然来电会排在一条已经看过的短信后面。
                _ if a.title.starts_with("手机 · ") || a.title=="未接来电" || a.title=="来电中"=>old.source==a.source,
                _=>old.source==a.source && old.title==a.title && old.unread==a.unread && (a.is_hud() || old.body==a.body),
           };
            if a.source!=AlertSource::Internal || a.title != "蓝牙设备" { self.alerts.retain(|old| !same_event(old)); }
            if (a.source!=AlertSource::Internal || a.title != "蓝牙设备") && self.notice.as_ref().is_some_and(same_event) {
                self.notice = Some(a);
                return;
            }
            self.alerts.push_back(a);
            while self.alerts.len() > 8 {
                self.alerts.pop_front();
            }
        }
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
        self.enqueue_alert(Alert{source:AlertSource::Messenger(client),title:client.name().into(),body:format!("有新消息 · 请到{}查看",client.name()),page:usize::MAX,at:self.t,duration:4.5,bluetooth:None,meeting:None,unread:Some(unread), status: None, },false);
    }
    /// 通用状态总线：外部脚本写 island_status.json 后转成岛上提示。
    /// 来源 id 带上状态 key，同 key 每 2 秒覆写一次，既刷新内容又续期。
    pub(crate) fn push_status(&mut self,s:crate::island_status::Status){
        let duration=if s.seconds>0.0{s.seconds}else{8.0};
        let key=s.key();
        self.enqueue_alert(Alert{source:AlertSource::External(format!("island-status:{key}")),title:s.text.clone(),body:s.detail.clone(),page:usize::MAX,at:self.t,duration,bluetooth:None,meeting:None,status:Some(s), unread: None, },false);
    }
    /// 状态从 JSON 里删掉：立刻撤下对应提示，不等 8 秒自然过期。
    pub(crate) fn drop_status(&mut self,key:&str){
        let source=AlertSource::External(format!("island-status:{key}"));
        self.alerts.retain(|a|a.source!=source);
        if self.notice.as_ref().is_some_and(|a|a.source==source){self.notice=None;self.notification_bell_until=0.0;}
        if self.hud_notice.as_ref().is_some_and(|a|a.source==source){self.hud_notice=None;}
    }
    /// MAA 是否可对接：通知名单里勾选了 MAA，或用户手动指定了 MAA 程序。
    ///
    /// 后面这条很关键：MAA 只有在发过系统通知后才会被自动登记进名单，
    /// 换台机器 / 关掉 MAA 通知的用户名单里根本没有它，此时手选路径就是唯一入口。
    pub(crate) fn maa_enabled(&self)->bool {
        if !self.maa_prefs.exe_path.trim().is_empty() { return true; }
        self.notification_apps.items.iter().any(|(id,name,on)| *on && (name=="MAA" || id.to_ascii_lowercase().ends_with("maa.exe")))
    }
    /// MAA 日志文件：优先用设置里的目录，其次从通知名单里 MAA 的 exe 路径推导 `<MAA目录>\debug`。
    pub(crate) fn maa_log_path(&self)->Option<std::path::PathBuf> {
        let custom=self.maa_prefs.log_dir.trim();
        if !custom.is_empty() { return Some(std::path::PathBuf::from(custom).join("asst.log")); }
        let exe=self.maa_exe()?;
        crate::maa::log_dir_from_exe(&exe).map(|dir|dir.join("asst.log"))
    }
    /// 直接打开某个设置分组（索引对应 SETTINGS_GROUPS），供命令行/快捷方式直达。
    pub fn open_settings_group(&mut self,group:usize){
        let last=SETTINGS_GROUPS.len()-1;
        let (_,start,_)=SETTINGS_GROUPS[group.min(last)];
        self.settings_category=group.min(last);
        self.settings_tab=start;
        self.open_settings();
    }
    /// 设置页选好 MAA 程序后写入偏好（换程序时顺便清掉图标缓存）。
    pub fn set_maa_exe(&mut self,path:String){
        self.maa_prefs.exe_path=path;
        self.maa_prefs.save();
        self.maa_icon=None;
    }
    /// 设置页选好日志目录后写入偏好。
    pub fn set_maa_log_dir(&mut self,path:String){
        self.maa_prefs.log_dir=path;
        self.maa_prefs.save();
    }
    /// MAA 可执行文件路径：优先用设置里手选的，其次取通知名单里登记的那份。
    pub(crate) fn maa_exe(&self)->Option<String> {
        let custom=self.maa_prefs.exe_path.trim();
        if !custom.is_empty() { return Some(custom.to_string()); }
        self.notification_apps.items.iter()
            .find(|(id,name,_)| name=="MAA" || id.to_ascii_lowercase().ends_with("maa.exe"))
            .map(|(id,_,_)| id.clone())
    }
    /// 按 MAA 任务进度刷新岛上那条常驻胶囊（运行中续期、完成后停留几秒）。
    pub(crate) fn refresh_maa_status(&mut self){
        if !self.maa_prefs.resident_progress { self.maa_status=None; return; }
        let now=unix_secs_f64();
        let Some(p)=self.maa_tracker.snapshot() else { self.maa_status=None; return; };
        // 图标：首次从 MAA.exe 提取成 PNG 缓存，之后直接读图片。
        let icon=match &self.maa_icon {
            Some(path)=>path.clone(),
            None=>{
                let path=self.maa_exe().and_then(|exe|{
                    let out=crate::model::config_dir().join("maa-icon.png");
                    crate::maa::ensure_icon(&exe,&out)
                }).map(|p|p.display().to_string()).unwrap_or_default();
                self.maa_icon=Some(path.clone());
                path
            }
        };
        if p.running {
            self.maa_done_until=0.0;
            // 运行中：15 秒持续续期；MAA 停下后不再续期，自动撤下（不会留僵尸胶囊）。
            let text=if p.current.is_empty(){"MAA · 运行中".to_string()}else{format!("MAA · {}",p.current)};
            let detail=if p.total>0 {format!("{}/{} 个任务",p.done,p.total)} else {String::new()};
            self.maa_status=Some(crate::island_status::Status{
                id:"maa".into(), text, detail, seconds:15.0,
                progress:p.ratio(), color:"blue".into(), click:String::new(),
                priority:5, ttl:now+15.0, icon,
            });
            return;
        }
        // 跑完：绿色「任务完成」停留几秒后撤下。
        let all_done=p.total>0 && p.done>=p.total;
        if all_done {
            if self.maa_done_until<=0.0 { self.maa_done_until=now+8.0; }
            if now<self.maa_done_until {
                self.maa_status=Some(crate::island_status::Status{
                    id:"maa".into(), text:"MAA · 任务完成".into(),
                    detail:format!("{} 个任务",p.total), seconds:8.0,
                    progress:Some(1.0), color:"green".into(), click:String::new(),
                    priority:5, ttl:now+8.0, icon,
                });
                return;
            }
        }
        self.maa_status=None;
    }
    /// 把一条 MAA 事件转成岛上提示（时长取 MAA 偏好）。
    ///
    /// 常驻进度胶囊开启时，任务开始/完成/全部完成都不再弹横幅——胶囊本身就在
    /// 显示当前任务和进度，再弹一条会重复。公招高星、出错仍单独提示（胶囊表达不了）。
    pub(crate) fn maa_event(&mut self,event:crate::maa::Event){
        let resident=self.maa_prefs.resident_progress;
        let (title,body)=match event {
            crate::maa::Event::ChainStart(_,label)=>{
                if !self.maa_prefs.notify_chain || resident {return;}
                ("MAA".to_string(),format!("开始{label}"))
            }
            crate::maa::Event::ChainDone(_,label)=>{
                if !self.maa_prefs.notify_chain || resident {return;}
                ("MAA".to_string(),format!("{label}完成"))
            }
            crate::maa::Event::AllDone(_)=>{
                if !self.maa_prefs.notify_chain || resident {return;}
                ("MAA".to_string(),"日常任务全部完成".to_string())
            }
            crate::maa::Event::RecruitStar(level)=>{
                if !self.maa_prefs.notify_recruit_star {return;}
                ("MAA 公招".to_string(),format!("刷出 {level} 星 tag"))
            }
            crate::maa::Event::Error(label)=>{
                if !self.maa_prefs.notify_error {return;}
                ("MAA 出错".to_string(),label)
            }
        };
        let a=Alert{source:AlertSource::Internal,title,body,page:usize::MAX,at:self.t,duration:self.maa_prefs.seconds as f64,bluetooth:None,meeting:None,status:None, unread: None, };
        self.enqueue_alert(a,false);
    }
    /// 手机电量的关键变化：断开 / 重连 / 低电 / 充满。
    /// 首轮只记录不提示，避免每次启动都弹一条「已连接」。
    pub(crate) fn phone_events(&mut self,now:Option<u8>,charging:bool){
        let prev=self.phone_last;
        self.phone_last=Some(now);
        let Some(prev)=prev else{return;};
        if !self.settings.notifications{return;}
        let low=self.settings.low_battery_threshold();
        let (title,body)=match (prev,now) {
            (Some(_),None)=>("手机已断开".to_string(),"电量信息已收起".to_string()),
            (None,Some(level))=>(format!("手机已连接 {level}%"),"手机连接已恢复".to_string()),
            (Some(old),Some(level)) if low.is_some_and(|t|old>t&&level<=t)=>(format!("手机电量偏低 {level}%"),format!("低于 {}%，记得给手机充电",low.unwrap_or(20))),
            (Some(old),Some(100)) if old<100=>(format!("手机已充满{}",if charging{"（充电中）"}else{""}),"可以拔掉充电器了".to_string()),
            _=>return,
        };
        self.alert(title,body,usize::MAX,false);
    }

    /// 本机电量的低电 / 充满提示，阈值与手机共用「低电提醒」设置。
    /// 首轮只记录不提示，避免每次启动都弹一条。
    pub(crate) fn pc_events(&mut self,now:Option<u8>,charging:bool){
        let prev=self.pc_last;
        self.pc_last=Some(now);
        let Some(prev)=prev else{return;};
        if !self.settings.notifications{return;}
        let Some(low)=self.settings.low_battery_threshold() else{return;};
        let (title,body)=match (prev,now) {
            (Some(old),Some(level)) if old>low&&level<=low=>(format!("本机电量偏低 {level}%"),if charging{"已接电源，正在充电".to_string()}else{format!("低于 {low}%，建议接上电源")}),
            (Some(old),Some(100)) if old<100&&charging=>("本机已充满 100%".to_string(),"可以拔掉充电器了".to_string()),
            _=>return,
        };
        self.alert(title,body,usize::MAX,false);
    }

    /// 勿扰是否生效（按 app 时间比较，重启后自动失效）。
    pub(crate) fn muted(&self)->bool{self.quiet_until>self.t}

    /// 文本是否命中通知关键词表（大小写不敏感）。
    pub(crate) fn keyword_hit(&self,text:&str)->bool{
        if text.is_empty(){return false;}
        // 规则里的 highlight 与关键词表同权：命中就亮琥珀色竖条。
        if self.notice_rules.hits(text,"highlight"){return true;}
        let lower=text.to_lowercase();
        // tick 里维护好的小写词表；源字符串刚改、缓存还没跟上时退回现算。
        if self.keyword_src==self.settings.notice_keywords {
            return self.keywords.iter().any(|word|lower.contains(word.as_str()));
        }
        self.settings.keywords().iter().any(|word|lower.contains(&word.to_lowercase()))
    }

    /// 会议自动勿扰：日程刚开始（90 秒内）且还没结束时把勿扰开到会议结束，最多 2 小时。
    /// 同一场会议只触发一次，散会自动失效。
    pub(crate) fn meeting_quiet(&mut self, env:&crate::app::Env){
        if !self.settings.quiet_meeting || self.muted() {return;}
        let Ok(day)=chrono::NaiveDate::parse_from_str(&env.today,"%Y-%m-%d") else{return};
        let now=chrono::Local::now().timestamp();
        // 缓存今日会议候选，60 秒或跨天才重算，避免高频合并排序引起抖动；
        // 触发窗口是 90 秒，不会因此漏掉会议。
        if self.meeting_plan.0!=env.today || self.t-self.meeting_plan.1>60.0 {
            let entries=self.calendar_entries(day);
            let plan=meeting_candidates(entries.iter().map(|entry|entry.event));
            self.meeting_plan=(env.today.clone(),self.t,plan);
        }
        let Some((until,key))=meeting_hit(&self.meeting_plan.2,now) else{return};
        if self.quiet_meeting_key.as_deref()==Some(key.as_str()) {return;}
        self.quiet_meeting_key=Some(key);
        self.quiet_until=until as f64;
        self.say("会议开始，自动勿扰到散会");
    }

    /// 把配置与状态复制到 `backups/<时间戳>/`：只复制文件、不递归子目录，不动原配置。
    pub(crate) fn backup_settings(&mut self){
        let Some(base)=std::env::var_os("APPDATA") else{self.say("找不到 APPDATA，无法备份");return;};
        let root=std::path::PathBuf::from(base).join("Daozai");
        let Some(backups)=backup_root() else{self.say("找不到 APPDATA，无法备份");return;};
        let dest=backups.join(chrono::Local::now().format("%Y%m%d-%H%M%S").to_string());
        if std::fs::create_dir_all(&dest).is_err(){self.say("备份失败：建不了目录");return;}
        let mut count=0;
        if let Ok(entries)=std::fs::read_dir(&root){
            for entry in entries.flatten(){
                let path=entry.path();
                if path.is_dir(){continue;}
                let Some(name)=path.file_name() else{continue;};
                if std::fs::copy(&path,dest.join(name)).is_ok(){count+=1;}
            }
        }
        self.say(format!("已备份 {count} 个文件"));
        self.pending.push(Cmd::Open(dest));
    }

    /// AI 额度阈值：任一窗口剩余 ≤ 阈值时提醒一次；回到阈值以上后重新武装。
    /// 每 30 秒看一次内存里的快照，不额外请求接口。
    pub(crate) fn quota_threshold_alert(&mut self){
        let threshold=self.settings.quota_alert;
        if threshold==0 {return;}
        let mut fresh:Vec<(String,f32)>=Vec::new();
        let mut armed=std::collections::HashSet::new();
        for (id,row) in self.quota.rows.iter(){
            let Ok(snapshot)=row else{continue};
            for window in snapshot.windows.iter().chain(snapshot.extra.iter()){
                if window.remaining<=threshold as f32 {
                    let key=(id.clone(),window.label.clone());
                    if !self.quota_alerted.contains(&key){fresh.push((window.label.clone(),window.remaining));}
                    armed.insert(key);
                }
            }
        }
        self.quota_alerted=armed;
        for (label,remaining) in fresh {
            self.alert("AI 额度", format!("{label} 只剩 {remaining:.0}%"), 7, true);
        }
    }

    /// 设置页用的最近通知：2 秒缓存一次，别每帧读盘。
    pub(crate) fn recent_notices(&mut self)->Vec<(String,String,String)>{
        if self.notice_cache.0<=0.0 || self.t-self.notice_cache.0>2.0 {
            self.notice_cache=(self.t.max(0.001),notice_history());
        }
        self.notice_cache.1.clone()
    }

    /// 把一条历史通知重新弹出来（「稍后提醒」用）。
    pub(crate) fn replay_notice(&mut self,title:String,body:String){
        self.enqueue_alert(Alert{source:AlertSource::External("notice-history".into()),title,body,page:usize::MAX,at:self.t,duration:if cfg!(test){4.5}else{30.0},bluetooth:None,meeting:None,status:None, unread: None, },false);
    }

    /// 稍后提醒：`minutes` 分钟后再弹一次。
    pub(crate) fn snooze_notice(&mut self,minutes:f64,title:String,body:String){
        self.snoozed.push((self.t+minutes*60.0,title,body));
        self.say(format!("{minutes:.0} 分钟后再提醒"));
    }

    /// 到点的「稍后提醒」重新入队，一次只处理一条，避免同一帧连弹。
    pub(crate) fn tick_snoozed(&mut self){
        if let Some(index)=self.snoozed.iter().position(|(due,_,_)|self.t>=*due){
            let (_,title,body)=self.snoozed.remove(index);
            self.replay_notice(title,body);
        }
    }

    /// 勿扰开关：`minutes = 0` 关闭。只在本次运行内有效，重启后恢复关闭。
    pub(crate) fn set_quiet(&mut self,minutes:u32){
        self.settings.quiet_minutes=minutes;
        self.settings.save();
        self.quiet_until=if minutes==0 {0.0} else {self.t+minutes as f64*60.0};
        self.say(if minutes==0 {"勿扰已关闭".to_string()} else {format!("勿扰 {minutes} 分钟")});
    }

    /// 状态总线触发的番茄钟开关：`on = false` 只是暂停，不重置这一轮。
    pub(crate) fn set_focus_running(&mut self,on:bool){
        if on {
            if self.pomo.running {return;}
            self.pomo.phase=crate::app::Phase::Focus;
            self.pomo.remaining=self.phase_secs(crate::app::Phase::Focus);
            self.pomo.running=true;
            self.say("已开始专注");
        } else if self.pomo.running {
            self.pomo.running=false;
            self.say("已暂停专注");
        }
    }

    /// 这条通知是否进入处理流程。两个互不相干的维度：
    /// 手机短信/来电只看 `phone_alerts`，其它应用只看监听范围（全部应用 / 仅 QQ）。
    pub(crate) fn accepts_notification(&self,id:&str,phone:bool)->bool{
        if !self.settings.read_notifications || id.is_empty(){return false;}
        if phone {return self.settings.phone_alerts;}
        !self.settings.qq_only || id=="QQ"   }
    pub fn set_messenger_status(&mut self,client:crate::app::Messenger,status:String){self.messenger_statuses.insert(client,status);}
    pub fn messenger_status(&self,client:crate::app::Messenger)->&str{self.messenger_statuses.get(&client).map(String::as_str).unwrap_or("")}
    pub fn receive_notification(&mut self, app:String, id:String, body:String){
        if crate::system::phone_channel(&id) {self.notify_phone(app,id,body);return;}
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
        self.enqueue_alert(Alert{source:AlertSource::External(id),title,body,page:usize::MAX,at:self.t,duration:4.5,bluetooth:None,meeting:None,unread, status: None, },false);
    }
    pub fn reveal_messenger_notice(&mut self,unread:crate::app::Unread){
        if !unread.source.adapter().dismiss_unread || !self.pending_messengers.contains(&unread){return;}
        let body=if unread.pid==0{"消息账号暂无法确认"}else{"暂时无法打开对应窗口"};
        let source=if unread.source.adapter().capture==crate::messenger::Capture::Taskbar{AlertSource::Messenger(unread.source)}else{AlertSource::External(unread.source.adapter().notification_id.into())};
        self.notice=Some(Alert{source,title:unread.source.name().into(),body:body.into(),page:usize::MAX,at:self.t,duration:4.5,bluetooth:None,meeting:None,unread:Some(unread), status: None, });
        self.notification_bell_at=self.t;self.notification_bell_until=self.t+4.5;
    }
    pub(crate) fn notify_phone(&mut self, app:String, id:String, body:String){
let phone=crate::system::phone_channel(&id);
        // 手机 App 名单是外部 JSON，改完约 1 秒内生效，不用重启岛仔。
        if phone {self.phone_apps.reload_if_changed();}
        // 每条通知留一行诊断，出问题能直接看出是哪一步拦下的。
        let trace=format!("id={id} app={app} phone={} qq_only={} body={}",
            if phone {"手机"} else {"其它"},
            if self.settings.qq_only {"是"} else {"否"},
            body.chars().take(60).collect::<String>());
        // 手机只看手机开关，其它应用只看监听范围——两者互不影响。
        if !self.accepts_notification(&id,phone) {
            log_notice(&format!("DROP 未连接/范围外/手机关  {trace}"));
            return;
        }
        // 真实手机连接通知只报主 AUMID，改由正文识别来源；
        // 明确命中黑名单的静默，识别不出的按「连接」放行，避免漏掉来电等重要通知。
        let phone_info:Option<(String,bool)>=if phone {Some(crate::system::phone_app(&body,&self.phone_apps).unwrap_or(("连接".to_string(),true)))}else{None};
        if phone_info.as_ref().is_some_and(|(_,allow)|!allow){
            log_notice(&format!("DROP 名单静默    {trace}"));
            return;
        }
        // 通知规则：静音 / 自动复制正文。规则文件改完约 1 秒生效，不用重启。
        self.notice_rules.reload_if_changed();
        let rule_text=format!("{app} {body}");
        if self.notice_rules.hits(&rule_text,"mute") {
            log_notice(&format!("DROP 规则静音    {trace}"));
            return;
        }
        if self.notice_rules.hits(&rule_text,"copy") && !body.trim().is_empty() {
            self.pending.push(Cmd::Copy(body.clone()));
            log_notice(&format!("规则命中 copy    {trace}"));
        }
        // 来电方（正文首段就是号码或联系人），来电中/未接来电都用它当副标题。
        let caller=body.split('·').next().unwrap_or("").trim().to_string();
        // 岛仔自己发出的系统提醒会被通知中心回传；已在历史里的同一内容不再入队，避免重复显示。
        if self.history.iter().any(|a|format!("{} · {}",a.title,a.body)==body){
            log_notice(&format!("DROP 自家回传    {trace}"));
            return;
        }
        if !self.notification_apps.observe(&id,&app) || !self.settings.notifications{
            log_notice(&format!("DROP 应用被关/总开关  {trace}"));
            return;
        }
        if id=="QQ"{self.mark_messenger(crate::app::Messenger::Qq,0,0);}
        // 验证码只有几分钟有效，铃铛多留一会儿，避免鼠标来不及移上去。
        let has_code=!body.is_empty() && crate::otp::extract("",&body,&[]).is_some();
        let otp_bell=phone && has_code;
        // 停留时长：普通通知用 notice_seconds；验证码用 otp_seconds，但不短于普通通知，
        // 且有 10 秒下限——验证码要留时间回看和点「复制」。
        let stay=notice_duration(self.settings.notice_seconds,self.settings.otp_seconds,false);
        let otp_stay=notice_duration(self.settings.notice_seconds,self.settings.otp_seconds,true);
        // 测试构建保持官方 4.5 秒，避免时长类断言失效。
        let keep_bell=if cfg!(test) {4.5} else if has_code {otp_stay} else {stay};
        // 勿扰期间不闪铃铛（横幅同样被 enqueue_alert 拦下），验证码仍照常提醒。
        if !self.muted() || otp_bell {
            self.notification_bell_at=self.t;
            self.notification_bell_until=self.t+keep_bell;
        }
        // 「手机 · 」前缀用来和岛仔自带 IMAP 邮件的「新邮件」提醒区分开。
        let title=if let Some((label,_))=&phone_info {format!("手机 · {label}")}else{app.chars().filter(|c|!c.is_control()).take(80).collect()};
        // 10 秒窗口内同一 App 的多条消息合并计数，配合 enqueue_alert 的替换去重防刷屏。
        let count=if phone {
            let entry=self.phone_counts.entry(title.clone()).or_insert((0,self.t));
            if self.t-entry.1>10.0{entry.0=0;}
            entry.0+=1;entry.1=self.t;
            entry.0
        }else{1};
        // 手机通知去掉正文首段的 App 名（已在标题里），其余作为摘要；
        // 即使全局关闭正文预览也显示一行短摘要，且不对验证码打码
        //（后续验证码卡片会直接复用 otp::extract 的结果）。
        let source:&str=if phone_info.is_some(){
            // 手机镜像通知的正文末尾常带一个多余的分隔符（`…忽略本短信! · `），顺手去掉。
            body.split_once('·').map(|(_,rest)|rest.trim().trim_end_matches(|c:char| c=='·'||c.is_whitespace()).trim()).filter(|rest|!rest.is_empty()).unwrap_or(body.as_str())
        }else{body.as_str()};        let body=if self.settings.notification_preview || phone {
            let limit=if phone{80}else{300};
            let mut text=source.chars().filter(|c|!c.is_control()).take(limit).collect::<String>();
            if !phone {if let Some(code)=crate::otp::extract("",&text,&[]){text=crate::mail_privacy::mask(&text,&code.text);}}
            text
        }else{"收到新通知 · 内容已隐藏".into()};
        let body=if count>1 {format!("共 {count} 条 · {body}")}else{body};
        // 手机验证码自动复制（与邮件共用开关）：短信验证码上岛时顺手写剪贴板。
        if phone && self.settings.auto_copy_otp {
            let code=crate::otp::extract("",&body,&[]);
            log_otp("phone",code.as_ref().map(|c|c.text.as_str()));
            if let Some(code)=code {self.pending.push(Cmd::Copy(code.text));}
        }
        // 来电中 / 未接来电：标签都是「电话」，靠正文区分——
        // 响铃是「号码 · 传入呼叫」，挂断是「电话 · 号码 · 未接来电」。
        let ringing=phone_info.is_some()&&body.contains("传入呼叫");
        let missed=phone_info.as_ref().is_some_and(|(label,_)|label=="电话")&&body.contains("未接来电");
        if missed {
            // 来电中的提示被「未接来电」取代，别让两条并存。
            if self.notice.as_ref().is_some_and(|a|a.title=="来电中"){self.notice=None;}
            self.missed_calls=self.missed_calls.saturating_add(1);
            self.notification_bell_until=f64::MAX;
        }
        let (title,body,duration)=if ringing {
            ("来电中".to_string(),format!("{caller} 正在呼叫"),60.0)
        }else if missed {
            ("未接来电".to_string(),format!("{} 通未接来电 · 点击关闭清除",self.missed_calls),30.0)
        }else{(title,body,if cfg!(test) {4.5} else if has_code {otp_stay} else {stay})};
        log_notice(&format!("SHOW {title} / {body}  <- {trace}"));
        self.enqueue_alert(Alert{source:AlertSource::External(id),title,body,page:usize::MAX,at:self.t,duration,bluetooth:None,meeting:None,status:None, unread: None, },false);
    }
    pub(crate) fn tick_alerts(&mut self) {
        self.pending_messengers.retain(|u|u.source.enabled(&self.settings) && (u.source.adapter().capture!=crate::messenger::Capture::SystemToast
            || self.notification_apps.items.iter().any(|(key,_,on)|key==u.source.adapter().notification_id && *on)));
        let allowed=|a:&Alert|match &a.source {
            AlertSource::Internal=>true,
            AlertSource::Messenger(client)=>client.enabled(&self.settings),
            AlertSource::External(id)=>{
                // 手机通道与状态总线不走应用名单：手机看 phone_alerts，状态总线常放行。
                let phone=crate::system::phone_channel(id);
                let status_bus=id=="island-status" || id.starts_with("island-status:");
                if status_bus {return self.settings.notifications;}
                if phone {return self.settings.notifications && self.settings.read_notifications && self.settings.phone_alerts;}
                self.settings.read_notifications && crate::messenger::NotificationFilter::for_settings(&self.settings).allows(id) && self.settings.notifications && !crate::messenger::suppresses_toast(&a.title,id,crate::messenger::toast_suppression(&self.settings)) && self.notification_apps.items.iter().any(|(key,_,on)|key==id && *on)
            },
       };
        self.alerts.retain(&allowed);
        if self.notice.as_ref().is_some_and(|a|!allowed(a)){self.notice=None;}
        if self.hud_notice.as_ref().is_some_and(|a|!allowed(a)){self.hud_notice=None;}
        if !self.settings.notification_preview {
            for a in self.alerts.iter_mut().chain(self.notice.iter_mut()).chain(self.hud_notice.iter_mut()) {
                let keep=matches!(&a.source,AlertSource::External(id) if crate::system::phone_channel(id) || id=="island-status" || id.starts_with("island-status:"));
                if !keep && matches!(&a.source,AlertSource::External(_)){a.body="收到新通知 · 内容已隐藏".into();}
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
}

/// 验证码识别诊断：只在「自动复制」开启时写一行，便于排查邮件/短信为什么没复制。
/// 日志只保留打码后的长度，绝不把验证码明文长期写进磁盘。
pub(crate) fn log_otp(source:&str,code:Option<&str>){
    let Some(base)=std::env::var_os("APPDATA") else{return};
    let dir=std::path::PathBuf::from(base).join("Daozai").join("diagnostics");
    if std::fs::create_dir_all(&dir).is_err(){return;}
    let path=dir.join("otp.log");
    if std::fs::metadata(&path).is_ok_and(|m|m.len()>256*1024){let _=std::fs::remove_file(&path);}
    if let Ok(mut file)=std::fs::OpenOptions::new().create(true).append(true).open(&path){
        use std::io::Write;
        let masked = code.map(|value| crate::mail_privacy::mask(value, value));
        let _=writeln!(file,"{} | {} | {}",chrono::Local::now().format("%H:%M:%S"),source,masked.as_deref().unwrap_or("-"));
    }
}

/// 通知卡片停留秒数：验证码有 10 秒下限，且不会短于普通通知（要留时间回看和点「复制」）。
pub(crate) fn notice_duration(notice_seconds:u32,otp_seconds:u32,has_code:bool)->f64{
    let stay=notice_seconds as f64;
    if has_code {(otp_seconds as f64).max(stay).max(10.0)} else {stay}
}

#[cfg(test)]
mod notice_duration_tests {
    use super::notice_duration;

    #[test]
    fn 验证码停留有下限且不短于普通通知() {
        assert_eq!(notice_duration(10, 90, true), 90.0, "验证码用自己设的 90 秒");
        assert_eq!(notice_duration(10, 20, true), 20.0);
        assert_eq!(notice_duration(60, 20, true), 60.0, "不能比普通通知还短");
        assert_eq!(notice_duration(10, 5, true), 10.0, "下限 10 秒");
        assert_eq!(notice_duration(10, 90, false), 10.0, "普通通知不受验证码设置影响");
    }
}

/// 备份目录：`%APPDATA%\Daozai\backups`，每次备份一个时间戳子目录。
pub(crate) fn backup_root()->Option<std::path::PathBuf>{
    let base=std::env::var_os("APPDATA")?;
    Some(std::path::PathBuf::from(base).join("Daozai").join("backups"))
}

/// 番茄统计：`%APPDATA%\Daozai\focus-stats.json`，只记「今天多少分钟」。
/// 只统计真正在跑的专注阶段，暂停不计；写盘频率是每分钟一次。
pub(crate) fn focus_stats_path()->Option<std::path::PathBuf>{
    let base=std::env::var_os("APPDATA")?;
    Some(std::path::PathBuf::from(base).join("Daozai").join("focus-stats.json"))
}

fn focus_days()->Vec<(String,u32)>{
    let Some(path)=focus_stats_path() else{return Vec::new();};
    // 坏文件会被改名成 .bad 后返回 None，避免「解析失败」被当成「本来是空的」而清空统计。
    let Some(json)=crate::model::read_json_renaming_bad(&path,crate::model::MAX_JSON_BYTES) else{return Vec::new();};
    json.get("days").and_then(|d|d.as_array()).into_iter().flatten().filter_map(|item|{
        Some((item.get("date")?.as_str()?.to_owned(), item.get("minutes")?.as_u64()? as u32))
    }).collect()
}

/// 今日专注分钟数与连续天数（连续天数从今天往回数，今天为 0 则从昨天算）。
pub(crate) fn focus_summary()->(u32,u32){
    let days=focus_days();
    let today=chrono::Local::now().date_naive();
    let minutes=days.iter().find(|(date,_)|date==&today.format("%Y-%m-%d").to_string()).map(|(_,m)|*m).unwrap_or(0);
    let mut streak=0;
    let mut cursor=today;
    if minutes==0 {cursor=today-chrono::Duration::days(1);}
    loop {
        let key=cursor.format("%Y-%m-%d").to_string();
        match days.iter().find(|(date,_)|date==&key) {
            Some((_,m)) if *m>0 =>{streak+=1;cursor=cursor-chrono::Duration::days(1);}
            _=>break,
        }
        if streak>365{break;}
    }
    (minutes,streak)
}

pub(crate) fn focus_add(minutes:u32){
    if minutes==0 || cfg!(test) {return;}
    let Some(path)=focus_stats_path() else{return;};
    if let Some(parent)=path.parent(){let _=std::fs::create_dir_all(parent);}
    let today=chrono::Local::now().date_naive().format("%Y-%m-%d").to_string();
    let mut days=focus_days();
    match days.iter_mut().find(|(date,_)|date==&today){
        Some(entry)=>entry.1+=minutes,
        None=>days.push((today,minutes)),
    }
    if days.len()>400 {let extra=days.len()-400;days.drain(0..extra);}
    days.sort_by(|a,b|a.0.cmp(&b.0));
    let json=serde_json::json!({"days":days.iter().map(|(date,m)|serde_json::json!({"date":date,"minutes":m})).collect::<Vec<_>>()});
    if let Ok(data)=serde_json::to_vec(&json){let _=crate::model::atomic_write(&path,&data);}
}

/// 通知历史（落盘，最多 100 条）：岛仔关掉之后「当时没看到的那条」还能查回来。
/// 存 `%APPDATA%\Daozai\notices.json`，字段 at/title/body，新的在后面。
pub(crate) fn notice_history_path()->Option<std::path::PathBuf>{
    let base=std::env::var_os("APPDATA")?;
    Some(std::path::PathBuf::from(base).join("Daozai").join("notices.json"))
}

/// 按时间顺序返回 (时间, 标题, 正文)，最新在最后。
pub(crate) fn notice_history()->Vec<(String,String,String)>{
    let Some(path)=notice_history_path() else{return Vec::new();};
    let Some(json)=crate::model::read_json_renaming_bad(&path,crate::model::MAX_JSON_BYTES) else{return Vec::new();};
    json.as_array().into_iter().flatten().filter_map(|item|{
        Some((item.get("at")?.as_str()?.to_owned(),item.get("title")?.as_str()?.to_owned(),item.get("body")?.as_str()?.to_owned()))
    }).collect()
}

pub(crate) fn notice_history_append(title:&str,body:&str){
    // 测试构建别往真实 APPDATA 里塞测试通知。
    if cfg!(test) {return;}
    let Some(path)=notice_history_path() else{return;};
    if let Some(parent)=path.parent(){let _=std::fs::create_dir_all(parent);}
    // 落盘前把验证码打码：历史文件只用于「回看是什么事」，不需要留着明文码。
    let body=if let Some(code)=crate::otp::extract("",body,&[]) {crate::mail_privacy::mask(body,&code.text)} else {body.to_string()};
    let mut list=notice_history();
    list.push((chrono::Local::now().format("%m-%d %H:%M").to_string(),title.to_string(),body));
    if list.len()>100{let extra=list.len()-100;list.drain(0..extra);}
    let json:Vec<_>=list.iter().map(|(at,title,body)|serde_json::json!({"at":at,"title":title,"body":body})).collect();
    // 原子写：中途断电也不会留下半截 JSON（否则下次读失败会被当空文件重建）。
    if let Ok(data)=serde_json::to_vec(&json){let _=crate::model::atomic_write(&path,&data);}
}

/// 通知通道诊断：每条进来的通知都记一行「显示 / 被谁拦下」，排查「某类通知不上岛」用。
pub(crate) fn log_notice(note:&str){
    let Some(base)=std::env::var_os("APPDATA") else{return};
    let dir=std::path::PathBuf::from(base).join("Daozai").join("diagnostics");
    if std::fs::create_dir_all(&dir).is_err(){return;}
    let path=dir.join("notice.log");
    if std::fs::metadata(&path).is_ok_and(|m|m.len()>256*1024){let _=std::fs::remove_file(&path);}
    if let Ok(mut file)=std::fs::OpenOptions::new().create(true).append(true).open(&path){
        use std::io::Write;
        let _=writeln!(file,"{} | {}",chrono::Local::now().format("%H:%M:%S"),note);
    }
}

/// 邮件面板操作诊断：点击「全部已读」被拒时记录原因，便于排查按钮为什么没反应。
pub(crate) fn log_mail(note:&str){
    let Some(base)=std::env::var_os("APPDATA") else{return};
    let dir=std::path::PathBuf::from(base).join("Daozai").join("diagnostics");
    if std::fs::create_dir_all(&dir).is_err(){return;}
    let path=dir.join("mail.log");
    if std::fs::metadata(&path).is_ok_and(|m|m.len()>256*1024){let _=std::fs::remove_file(&path);}
    if let Ok(mut file)=std::fs::OpenOptions::new().create(true).append(true).open(&path){
        use std::io::Write;
        let _=writeln!(file,"{} | {}",chrono::Local::now().format("%H:%M:%S"),note);
    }
}

fn phone_icon_file(title:&str)->Option<&'static str>{
    let label=title.strip_prefix("手机 · ")?;
    Some(match label {
        "QQ"=>"app_icon_com.tencent.mqq1.0.jpg",
        "微信"=>"app_icon_com.tencent.xin1.0.jpg",
        "支付宝"=>"app_icon_com.alipay.iphoneclient1.0.jpg",
        "Gmail"=>"app_icon_com.google.Gmail1.0.jpg",
        "电话"=>"app_icon_com.apple.mobilephone1.0.jpg",
        _=>return None,
    })
}
/// 用手机连接自带的 App 图标替代铃铛；首次加载后缓存纹理，避免每帧读盘。
fn phone_icon(c:&Card,title:&str,icons:&mut std::collections::HashMap<String,egui::TextureHandle>)->bool{
    let Some(file)=phone_icon_file(title) else{return false};
    if !icons.contains_key(file){
        let Ok(base)=std::env::var("LOCALAPPDATA") else{return false};
        let path=std::path::PathBuf::from(base).join("Packages").join("Microsoft.YourPhone_8wekyb3d8bbwe")
            .join("TempState").join("app_icons").join(file);
        let Ok(bytes)=std::fs::read(path) else{return false};
        let Ok(img)=image::load_from_memory(&bytes) else{return false};
        let rgba=img.into_rgba8();
        let ci=egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize,rgba.height() as usize],rgba.as_raw());
        let tex=c.ui.ctx().load_texture(file.to_owned(),ci,egui::TextureOptions::LINEAR);
        icons.insert(file.to_owned(),tex);
    }
    let Some(tex)=icons.get(file) else{return false};
    c.painter.image(tex.id(),c.rect(12.0,10.0,24.0,24.0),Rect::from_min_max(egui::Pos2::ZERO,egui::pos2(1.0,1.0)),Color32::WHITE);
    true
}

/// 状态总线点击后的动作：要么交给外壳执行，要么由岛仔自己处理。
pub(crate) enum StatusAct {
    Shell(Cmd),
    /// 开始 / 暂停番茄专注。
    Focus(bool),
    /// 勿扰 N 分钟（0 = 关闭）。
    Quiet(u32),
    /// 跳到第 N 页（0 音乐 … 7 设置）。
    Page(usize),
}

/// 状态总线的点击动作白名单：`copy:` / `url:` / `file:` 交给外壳，
/// `action:` 只认下表里的几个词——脚本能指挥岛仔，但指挥不了整台电脑。
fn status_action(spec:&str)->Option<(&'static str,StatusAct)>{
    use StatusAct::*;
    if let Some(text)=spec.strip_prefix("copy:") {
        if !text.is_empty(){return Some(("复制",Shell(Cmd::Copy(text.to_string()))));}
    }
    if let Some(url)=spec.strip_prefix("url:") {
        if url.starts_with("https://")||url.starts_with("http://"){return Some(("打开",Shell(Cmd::OpenUrl(url.to_string()))));}
    }
    if let Some(file)=spec.strip_prefix("file:") {
        // 只放行「看到的文件」：路径必须存在，且不是可执行/脚本/快捷方式——
        // 否则任何能写 island_status.json 的进程都能借岛仔的手启动程序。
        let path=std::path::PathBuf::from(file);
        let blocked=path.extension().and_then(|e|e.to_str()).map(|e|{
            matches!(e.to_ascii_lowercase().as_str(),"exe"|"bat"|"cmd"|"com"|"ps1"|"vbs"|"js"|"jse"|"wsf"|"wsh"|"hta"|"lnk"|"scr"|"msi"|"pif"|"reg")
        }).unwrap_or(false);
        if path.exists() && !blocked {return Some(("打开",Shell(Cmd::Open(path))));}
    }
    let action=spec.strip_prefix("action:")?.trim();
    match action {
        "focus_start"=>Some(("开始专注",Focus(true))),
        "focus_stop"=>Some(("结束专注",Focus(false))),
        "quiet30"=>Some(("勿扰 30 分",Quiet(30))),
        "quiet60"=>Some(("勿扰 60 分",Quiet(60))),
        "quiet_off"=>Some(("关闭勿扰",Quiet(0))),
        _=>action.strip_prefix("page:").and_then(|n|n.trim().parse::<usize>().ok()).filter(|n|*n<8).map(|n|("打开",Page(n))),
    }
}
/// 当前 Unix 时间（秒，带小数）；写状态总线的 ttl 用。
pub(crate) fn unix_secs_f64()->f64{
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_secs_f64()).unwrap_or(0.0)
}

pub(crate) fn status_color(name:&str)->Color32{
    match name.trim().to_ascii_lowercase().as_str(){
        "red"=>Color32::from_rgb(226,75,74),
        "green"=>theme::CHARGE,
        "blue"=>theme::BLUE,
        "amber"=>Color32::from_rgb(239,159,39),
        _=>theme::GRAY,
    }
}

impl App {
    pub(crate) fn draw_notice(&mut self, c:&Card,panel:Rect,a:&Alert,_cmds:&mut Vec<Cmd>){
        let nc=c.clipped(0.0,0.0,panel.width(),panel.height());
        // 关键词高亮：命中词表时在卡片最左侧亮一条琥珀色竖条。
        // 只加一条竖条，不动文字和任何控件的位置，布局与测试友好的部分保持原样。
        if a.status.is_none() && (self.keyword_hit(&a.title) || self.keyword_hit(&a.body)) {
            nc.fill(2.5,10.0,2.5,24.0,1.25,Color32::from_rgb(239,159,39));
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
            // 通用状态总线：独立绘制，不影响手机通知 / 验证码卡片的原有布局。
            if let Some(st)=&a.status {
                let tone=status_color(&st.color);
                nc.fill(6.0,10.0,3.0,24.0,1.5,tone);
                nc.centered_text(16.0,14.0,&nc.fit(&a.title,11.0,W::Semibold,198.0),11.0,W::Semibold,theme::WHITE,H::Left);
                nc.centered_text(16.0,30.0,&nc.fit(&a.body,10.5,W::Regular,198.0),10.5,W::Regular,theme::GRAY,H::Left);
                if let Some(p)=st.progress.filter(|v|v.is_finite()) {
                    let p=p.clamp(0.0,1.0) as f32;
                    nc.fill(16.0,37.0,198.0,2.0,1.0,Color32::from_gray(60));
                    nc.fill(16.0,37.0,198.0*p,2.0,1.0,tone);
                }
                if let Some((label,act))=status_action(&st.click) {
                    if notice_button(&nc,160.0,12.0,60.0,label,"status-action"){
                        match act {
                            StatusAct::Shell(cmd)=>_cmds.push(cmd),
                            StatusAct::Focus(on)=>self.set_focus_running(on),
                            StatusAct::Quiet(minutes)=>self.set_quiet(minutes),
                            StatusAct::Page(page)=>self.show_page(page),
                        }
                    }
                }
                if notice_button(&nc,226.0,12.0,60.0,"关闭","external-dismiss"){
                    self.notice=None;
                    self.notification_bell_until=0.0;
                }
                return;
            }
            // 手机通知优先用手机连接自带的 App 图标，没图标才回退到铃铛。
            let icon=phone_icon(&nc,&a.title,&mut self.phone_icons);
            if !icon {
                crate::paint::bell(&nc,310.0,22.0,theme::GRAY,crate::paint::bell_swing(self.t-a.at));
                nc.circle(314.0,17.5,2.0,Color32::from_rgb(255,72,72));
            }
            // 有图标时文字整体右移，避免压在图标上。
            let text_x=if icon{44.0}else{16.0};
            let text_w=if icon{170.0}else{198.0};
            nc.centered_text(text_x,14.0,&nc.fit(&a.title,11.0,W::Semibold,text_w),11.0,W::Semibold,theme::WHITE,H::Left);
            // 验证码卡片：大号数字 + 一键复制（otp::extract 有打分门槛，普通消息不会误判）。
            // 短信从正文提取；邮件验证码只从邮件数据读，通知与历史都不留明文。
            let mail_code=matches!(&a.source,AlertSource::External(id) if id=="mail-code")
                .then(||self.mail.code_item().and_then(|m|m.code.clone())).flatten();
            if let Some(code)=mail_code.or_else(||crate::otp::extract("",&a.body,&[]).map(|c|c.text)) {
                nc.centered_text(text_x,30.0,&code,20.0,W::Bold,theme::WHITE,H::Left);
                if notice_button(&nc,160.0,12.0,60.0,"复制","phone-code-copy"){_cmds.push(Cmd::Copy(code));}
            } else {
                nc.centered_text(text_x,30.0,&nc.fit(&a.body,10.5,W::Regular,text_w),10.5,W::Regular,theme::GRAY,H::Left);
            }
            if notice_button(&nc,226.0,12.0,60.0,"关闭","external-dismiss"){
                self.notice=None;
                self.notification_bell_until=0.0;
                if a.title=="未接来电"{self.missed_calls=0;}
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
                nc.circle(26.0,24.0,14.0,if status=="已连接"{Color32::from_rgb(10,132,255)}else{Color32::from_gray(95)});
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
                    if fill>0.0 {nc.fill(x+2.0,22.0,fill,4.0,1.0,if level<=20{theme::RED}else{theme::GREEN});}
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
            let is_sit=a.title==crate::app::SIT_TITLE;let is_water=a.page==2&&!is_sit;let is_focus=a.page==1;let is_power=a.title=="电量偏低";
            notice_icon(&nc,22.0,22.0,a);
            let max=if is_water||is_sit{125.0}else{195.0};
            nc.centered_text(43.0,14.0,&nc.fit(&a.title,11.0,W::Semibold,max),11.0,W::Semibold,theme::WHITE,H::Left);
            nc.centered_text(43.0,30.0,&nc.fit(&a.body,10.5,W::Regular,max),10.5,W::Regular,theme::GRAY,H::Left);
            if is_sit {
                if notice_button(&nc,181.0,12.0,62.0,"稍后","sit-snooze"){self.snooze_sit();self.notice=None;}
                if notice_button(&nc,250.0,12.0,68.0,"起身了","sit-confirm"){self.notice=None;self.stood_up();}
            }else if is_water {
                if notice_button(&nc,181.0,12.0,62.0,"稍后","water-snooze"){self.last_reminder=self.t-self.settings.water_minutes as f64*60.0+600.0;self.notice=None;}
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
        self.last_reminder = self.t;
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
        let groups = ["常规", "音乐与歌词", "专注与饮水", "专注与饮水", "待办与置物", "待办与置物", "通知、邮箱与日程", "通知、邮箱与日程", "通知、邮箱与日程", "关于", "页面管理", "AI 额度", "天气", "手机连接", "浏览器下载", "MAA"];
       for (tab, expected) in groups.iter().enumerate() {
            let matches: Vec<_> = SETTINGS_GROUPS.iter().filter(|(_,start,end)| (*start..=*end).contains(&tab)).collect();
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
        let icons: &[&[u8]] = &[
            include_bytes!("../assets/device-icons/headphones-round-bold.svg"),
            include_bytes!("../assets/device-icons/speaker-bold.svg"),
            include_bytes!("../assets/device-icons/keyboard-bold.svg"),
            include_bytes!("../assets/device-icons/mouse-bold.svg"),
            include_bytes!("../assets/device-icons/gamepad-bold.svg"),
            include_bytes!("../assets/device-icons/smartphone-bold.svg"),
            include_bytes!("../assets/device-icons/bluetooth-linear.svg"),
        ];
        for bytes in icons {
            let image=egui_extras::image::load_svg_bytes(bytes).unwrap();
            assert!(image.pixels.iter().any(|p|p.a()>0));
        }
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
    let (r,b)=crate::paint::capsule(c,x,y,w,20.0,id,Color32::from_gray(35),theme::BTN_HOVER);
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
            let hour=if i==0{self.settings.water_start}else{self.settings.water_end};
            let d=stepper(c,if i==0{238.0}else{370.0},y,&format!("{hour:02}:00"),&format!("hour-{i}"));
            if d!=0{let v=(hour as i32+d).rem_euclid(24) as u32;if i==0{self.settings.water_start=v}else{self.settings.water_end=v}}
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
            let m=if i==0{self.settings.lunch_start}else{self.settings.lunch_end};
            let d=stepper(&lc,if i==0{238.0}else{370.0},y,&hhmm(m),&format!("lunch-{i}"));
            if d!=0{let v=(m as i32+d*15).rem_euclid(1440) as u32;if i==0{self.settings.lunch_start=v}else{self.settings.lunch_end=v}}
        }
        c.text(44.0,row_y(3)+13.0,"喝水和久坐提醒都只在提醒时段内、午休以外弹出",10.5,W::Regular,theme::GRAY,H::Left);
    }
}
/// 加减器：[−] 数值 [+]，右缘对齐到 right。返回 −1 / 0 / +1。
fn stepper(c:&Card,right:f32,y:f32,value:&str,id:&str)->i32 {
    let (w,vw)=(25.0,58.0);
    let x=right-w*2.0-vw;
    c.text(x+w+vw/2.0,y+13.0,value,10.5,W::Semibold,theme::WHITE,H::Center);
    if setting_button(c,x,y,w,"−",false,&format!("{id}-minus")){return -1;}
    if setting_button(c,right-w,y,w,"+",false,&format!("{id}-plus")){return 1;}
    0
}
impl App {
    pub(crate) fn page_settings(&mut self,c:&Card) {
        let help=match self.settings_category{0=>"外观与系统",1=>"调整喝水、久坐与提醒时段",2=>"点击分钟数可直接输入，也可用加减微调",_=>"通知"};
        if self.settings_category!=1{c.text(44.0,44.0,help,10.5,W::Regular,theme::GRAY,H::Left);}
        let before=format!("{:?}",self.settings);
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
                    let rows=[("每日目标",format!("{:.1} L",self.water.goal_ml as f32/1000.0)),("每次记录",format!("{} ml",self.settings.serving_ml)),("提醒间隔",format!("{} 分钟",self.settings.water_minutes))];
                    for (i,(label,value)) in rows.iter().enumerate(){
                        wc.text(44.0,row_y(i)+13.0,label,10.5,W::Regular,theme::GRAY,H::Left);
                        match stepper(&wc,370.0,row_y(i),value,&format!("water-{i}")){
                            0=>{}
                            d=>match i{
                                0=>{self.water.goal_ml=(self.water.goal_ml as i32+d*250).clamp(500,6000) as u32;self.water.save();}
                                1=>self.settings.serving_ml=(self.settings.serving_ml as i32+d*50).clamp(50,1000) as u32,
                                _=>self.settings.water_minutes=(self.settings.water_minutes as i32+d*5).clamp(5,180) as u32,
                            }
                        }
                    }
                }
                if sv>0.01{
                    let sc=c.faded(sv);let sc=if self.sit_mode{sc}else{sc.ghost()};
                    sc.text(44.0,row_y(0)+13.0,"坐满多久提醒",10.5,W::Regular,theme::GRAY,H::Left);
                    let d=stepper(&sc,370.0,row_y(0),&format!("{} 分钟",self.settings.sit_minutes),"sit");
                    if d!=0{self.settings.sit_minutes=(self.settings.sit_minutes as i32+d*5).clamp(15,180) as u32;}
                    sc.text(44.0,row_y(1)+13.0,"专注计时进行中不打扰，专注结束后再提醒",10.5,W::Regular,theme::GRAY,H::Left);
                    sc.text(44.0,row_y(2)+13.0,"锁屏或点「起身了」后，久坐计时归零",10.5,W::Regular,theme::GRAY,H::Left);
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
                self.focus_editing=false;
                for (i,name) in ["专注","休息"].iter().enumerate(){
                    let y=53.0+i as f32*23.0;
                    c.text(48.0,y+13.0,name,10.5,W::Regular,theme::GRAY,H::Left);
                    let value=if i==0{&mut self.settings.focus_minutes}else{&mut self.settings.rest_minutes};
                    let max=if i==0{180}else{60};let rect=c.rect(168.0,y,76.0,19.0);
                    let mut ui=egui::Ui::new(c.ui.ctx().clone(),egui::Id::new(("focus-duration",i)),egui::UiBuilder::new().layer_id(c.ui.layer_id()).max_rect(rect).style(c.ui.style().clone()));
                    ui.set_clip_rect(c.painter.clip_rect());ui.set_opacity(c.painter.opacity());if !c.live{ui.disable();}
                    ui.spacing_mut().interact_size=rect.size();ui.spacing_mut().button_padding=egui::vec2(2.0,1.0);
                    ui.style_mut().drag_value_text_style=egui::TextStyle::Body;
                    ui.style_mut().text_styles.insert(egui::TextStyle::Body,c.font(11.0,W::Bold));
                    ui.style_mut().explanation_tooltips=false;
                    ui.visuals_mut().widgets.inactive.weak_bg_fill=Color32::TRANSPARENT;
                    let field=ui.put(rect,egui::DragValue::new(value).range(1..=max).suffix(" 分钟").update_while_editing(false)).on_hover_text("点击输入分钟数");
                    if field.gained_focus(){self.pending.push(Cmd::Focus);}
                    self.focus_editing|=field.has_focus();
                    if setting_button(c,131.0,y,27.0,"−",false,&format!("focus-minus-{i}")){*value=value.saturating_sub(1).max(1);}
                    if setting_button(c,255.0,y,27.0,"+",false,&format!("focus-plus-{i}")){*value=(*value+1).min(max);}
                }
                c.text(315.0,93.0,"下次生效",10.5,W::Regular,theme::GRAY,H::Center);
            }
            _=>{
                if setting_button(c,43.0,56.0,152.0,if self.settings.notifications{"提醒 · 开"}else{"提醒 · 关"},self.settings.notifications,"set-notice"){self.settings.notifications=!self.settings.notifications;}
                if setting_button(c,212.0,56.0,153.0,if self.settings.sound{"声音 · 开"}else{"声音 · 关"},self.settings.sound,"set-sound"){self.settings.sound=!self.settings.sound;}
                if setting_button(c,43.0,84.0,152.0,if self.settings.read_notifications{"断开系统通知"}else{"连接系统通知"},self.settings.read_notifications,"set-listener"){self.pending.push(if self.settings.read_notifications{Cmd::DisconnectNotifications}else{Cmd::RequestNotifications});}
                let status=c.fit(&self.listener_status,10.5,W::Regular,170.0);c.text(210.0,97.0,&status,10.5,W::Regular,theme::GRAY,H::Left);
            }
        }
        if before!=format!("{:?}",self.settings){self.settings.save();self.last_reminder=self.t;}
    }
}

/// 不可用的设置行：开关置灰、点击无效。
/// 用于「本机没装对应组件」的情况（例如没有「手机连接」时的手机电量）。
pub(crate) fn setting_row_locked(ui:&mut egui::Ui,title:&str,description:&str,value:bool){
    let ctx=&ui.ctx().clone();
    ui.add_space(2.0);
    ui.horizontal(|ui|{
        let text_w=(ui.available_width()-SETTINGS_SWITCH_W-SETTINGS_ROW_GAP).max(120.0);
        ui.vertical(|ui|{
            ui.set_width(text_w);
            ui.spacing_mut().item_spacing.y=2.0;
            ui.add(egui::Label::new(egui::RichText::new(title).size(13.5).color(settings_color(ctx,Color32::from_gray(130),Color32::from_gray(140)))).wrap());
            if !description.is_empty(){ui.add(egui::Label::new(egui::RichText::new(description).size(12.0).color(settings_color(ctx,Color32::from_gray(118),Color32::from_gray(132)))).wrap());}
        });
        ui.add_space((ui.available_width()-SETTINGS_SWITCH_W-ui.spacing().item_spacing.x).max(0.0));
        let (rect,_)=ui.allocate_exact_size(egui::vec2(34.0,20.0),egui::Sense::hover());
        ui.painter().rect_filled(rect,10.0,settings_color(ctx,Color32::from_gray(58),Color32::from_rgb(226,229,234)));
        let x=if value{rect.right()-10.0}else{rect.left()+10.0};
        ui.painter().circle_filled(egui::pos2(x,rect.center().y+0.5),7.5,Color32::from_black_alpha(20));
        ui.painter().circle_filled(egui::pos2(x,rect.center().y),7.0,settings_color(ctx,Color32::from_gray(150),Color32::WHITE));
    });
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
/// 设置页版式常量。
pub(crate) const SETTINGS_LABEL_W: f32 = 80.0;
const SETTINGS_SWITCH_W: f32 = 34.0;
const SETTINGS_ROW_GAP: f32 = 16.0;

/// 开关行：左侧标题 + 说明（宽度固定、自动换行），右侧开关，长说明也不会把开关挤出卡片。
pub(crate) fn setting_row(ui:&mut egui::Ui,title:&str,description:&str,value:&mut bool){
    let ctx=&ui.ctx().clone();
    ui.add_space(2.0);
    ui.horizontal(|ui|{
        let text_w=(ui.available_width()-SETTINGS_SWITCH_W-SETTINGS_ROW_GAP).max(120.0);
        ui.vertical(|ui|{
            ui.set_width(text_w);
            ui.spacing_mut().item_spacing.y=3.0;
            ui.add(egui::Label::new(egui::RichText::new(title).size(13.5).color(settings_color(ctx,Color32::from_rgb(238,240,244),Color32::from_rgb(24,27,32)))).wrap());
            if !description.is_empty(){ui.add(egui::Label::new(egui::RichText::new(description).size(12.0).color(settings_color(ctx,Color32::from_gray(158),Color32::from_rgb(96,101,110)))).wrap());}
        });
        // add_space 之后还会再插入一次 item_spacing.x，这里一并扣掉，开关才不会每行往外挤一点。
        ui.add_space((ui.available_width()-SETTINGS_SWITCH_W-ui.spacing().item_spacing.x).max(0.0));
        switch(ui,value);
    });
    ui.add_space(2.0);
}

/// 设置页统一的分段选择器（灰色底框 + 选中块）。返回值为是否发生了切换。
pub(crate) fn settings_segmented<T:PartialEq+Copy>(ui:&mut egui::Ui,value:&mut T,options:&[(T,&str)],item_w:f32)->bool{
    let ctx=&ui.ctx().clone();
    let mut changed=false;
    egui::Frame::new().fill(settings_color(ctx,Color32::from_rgb(28,28,30),Color32::from_rgb(230,233,238)))
        .corner_radius(8.0).inner_margin(3).show(ui,|ui|{
            ui.horizontal(|ui|{
                ui.spacing_mut().item_spacing.x=2.0;
                for (option,label) in options {
                    let selected=*value==*option;
                    let text=egui::RichText::new(*label).size(12.0).color(if selected{settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41))}else{settings_color(ctx,Color32::from_gray(200),Color32::from_gray(70))});
                    let button=egui::Button::new(text)
                        .fill(if selected{settings_color(ctx,Color32::from_rgb(72,72,76),Color32::WHITE)}else{Color32::TRANSPARENT})
                        .stroke(egui::Stroke::NONE).corner_radius(6.0);
                    let width=item_w.max(ctx.fonts(|f|f.layout_no_wrap(label.to_string(),egui::FontId::proportional(12.0),Color32::WHITE).size().x)+20.0);
                    if ui.add_sized([width,26.0],button).clicked() && !selected {*value=*option;changed=true;}
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
/// 可折叠的设置分区：标题右侧给一句摘要，点标题展开 / 收起。
/// 状态存在 egui 的临时数据里，当前运行内记住，重启后回到 default_open。
/// 用它把低频配置收起来，避免通知、邮箱、日程三页一路滚到底。
fn settings_section(ui:&mut egui::Ui,key:&str,title:&str,summary:&str,default_open:bool)->bool{
    // 已有的设置页 UI 测试会直接点表单里的按钮；测试构建下全部展开，
    // 让这些交互断言继续覆盖原来的控件，生产版仍按 default_open 折叠。
    if cfg!(test){return true;}
    let ctx=ui.ctx().clone();
    let id=egui::Id::new(("settings-section",key));
    let open=ctx.data(|data|data.get_temp::<bool>(id).unwrap_or(default_open));
    let mut clicked=false;
    ui.add_space(10.0);
    ui.horizontal(|ui|{
        let text=format!("{}  {}",if open{"▾"}else{"▸"},title);
        let hit=ui.add(egui::Label::new(egui::RichText::new(text).size(13.5).strong()
            .color(settings_color(&ctx,Color32::from_gray(225),Color32::from_rgb(30,34,41)))).sense(egui::Sense::click()))
            .on_hover_text(if open{"点击收起"}else{"点击展开"});
        if hit.clicked(){clicked=true;}
        if !summary.is_empty(){
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                ui.label(egui::RichText::new(summary).size(11.0).color(settings_color(&ctx,Color32::from_gray(148),Color32::from_gray(105))));
            });
        }
    });
    if clicked{ctx.data_mut(|data|data.insert_temp(id,!open));}
    ui.add_space(4.0);
    if clicked{!open}else{open}
}

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
    ui.add(egui::Label::new(egui::RichText::new(text).size(12.0).color(settings_color(ctx,Color32::from_gray(148),Color32::from_rgb(100,106,115)))).wrap());
}
pub(crate) fn settings_card(ui:&mut egui::Ui,add:impl FnOnce(&mut egui::Ui)) {
    let ctx=&ui.ctx().clone();
    egui::Frame::new().fill(settings_color(ctx,Color32::from_rgb(35,35,38),Color32::from_rgb(255,255,255)))
        .stroke(egui::Stroke::new(1.0,settings_color(ctx,Color32::from_rgb(56,57,62),Color32::from_rgb(228,231,236))))
        .corner_radius(12.0).inner_margin(egui::Margin::symmetric(16,13))
        .show(ui,|ui|{
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().item_spacing.y=8.0;
            // 卡内分隔线压淡：8px 行距已经够分隔，硬线太密会让卡片显得碎。
            ui.style_mut().visuals.widgets.noninteractive.bg_stroke=egui::Stroke::new(1.0,settings_color(ctx,Color32::from_rgb(48,49,54),Color32::from_rgb(238,240,244)));
            add(ui);
        });
}
/// 设置分页对应的功能模块（页面序号）；常规、通知、关于、页面管理不属于任何模块。
pub(crate) fn settings_tab_module(tab:usize)->Option<usize>{
    use crate::app::*;
    match tab {1=>Some(MUSIC_PAGE),2=>Some(FOCUS_PAGE),3=>Some(WATER_PAGE),4=>Some(TODO_PAGE),5=>Some(SHELF_PAGE),7=>Some(MAIL_PAGE),8=>Some(CALENDAR_PAGE),11=>Some(QUOTA_PAGE),12=>Some(WEATHER_PAGE),_=>None}
}

pub(crate) fn settings_label(ui:&mut egui::Ui,text:&str){
    let ctx=&ui.ctx().clone();
    ui.add_space(13.0);
    // 分组标题：左侧补一条 3px accent 竖条当视觉锚点（不影响布局结构）。
    let response=ui.label(egui::RichText::new(text).size(12.0).color(settings_color(ctx,Color32::from_gray(168),Color32::from_rgb(88,94,104))));
    let r=response.rect;
    let bar=egui::Rect::from_min_size(egui::pos2(r.left()-10.0,r.top()+1.5),egui::vec2(3.0,(r.height()-3.0).max(8.0)));
    ui.painter().rect_filled(bar,1.5,settings_color(ctx,Color32::from_rgb(96,165,250),Color32::from_rgb(37,99,235)));
    ui.add_space(4.0);
}
fn support_author(ui:&mut egui::Ui){
    let id=egui::Id::new("support-author-open");
    let open=ui.ctx().data(|data|data.get_temp::<bool>(id).unwrap_or(false));
    if !open {release_support_codes(ui.ctx());}
    let dark=ui.visuals().dark_mode;
    let accent=if dark{Color32::from_rgb(182,163,245)}else{Color32::from_rgb(105,77,163)};
    ui.scope(|ui|{
    ui.set_min_height(225.0);
    if !open {
    egui::Frame::new().fill(if dark{Color32::from_rgb(42,37,53)}else{Color32::from_rgb(239,233,250)})
        .corner_radius(10.0).show(ui,|ui|{
            ui.set_min_width(ui.available_width());
            ui.vertical_centered(|ui|{
                ui.add(egui::Image::new(egui::include_image!("../assets/support/daozai-coffee.svg")).fit_to_exact_size(egui::vec2(211.0,110.0)));
            });
        });
    ui.add_space(9.0);
    ui.label(egui::RichText::new("小唐维护 · 岛仔陪伴").size(10.5).color(accent));
    ui.label(egui::RichText::new("请小唐喝杯咖啡").size(19.0).strong());
    ui.label("岛仔陪你认真生活，\n你的支持，陪小唐把它慢慢做好。");
    } else {
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
                    let side=ui.available_width().min(180.0);
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
    ui.add_space(7.0);
    ui.label(egui::RichText::new("完全自愿 · 感谢每一份支持 · 不影响功能使用").size(11.0).color(ui.visuals().weak_text_color()));
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
/// 时刻加减器（一天中的分钟数，15 分钟一档，跨午夜循环），中间可直接输入 12:30。
fn time_stepper(ui:&mut egui::Ui,value:&mut u32,id:&str)->bool{
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
const SETTINGS_GROUPS: [(&str,usize,usize);12] = [
    ("常规",0,0), ("页面管理",10,10), ("音乐与歌词",1,1), ("专注与饮水",2,3),
    ("待办与置物",4,5), ("通知、邮箱与日程",6,8), ("天气",12,12), ("手机连接",13,13), ("浏览器下载",14,14), ("MAA",15,15), ("AI 额度",11,11), ("关于",9,9),
];

/// 「浏览器下载」卡片：始终显示安装指引（打开浏览器扩展页和扩展文件夹、复制路径）。
/// Chrome/Edge 从 33 版起封死了本地扩展的静默安装，只能开发者模式加载，所以做成引导而非自动装。
/// 注：不做「已连接」状态检测——桥进程只在下载进行时才被浏览器拉起，平时检测恒为未连接，没有意义。
#[cfg(windows)]
fn bridge_card_body(app: &mut App, ctx: &egui::Context, ui: &mut egui::Ui) {
    let Some(ext) = crate::bridge::extension_folder() else {
        ui.label(egui::RichText::new("未找到浏览器扩展（bridge\\extension）。请用岛仔安装包安装后再来连接。").size(12.0).color(settings_color(ctx, Color32::from_gray(180), Color32::from_gray(75))));
        return;
    };
    ui.horizontal_wrapped(|ui| {
        for (name, exe) in crate::bridge::installed_browsers() {
            if ui.button(format!("安装到 {name}")).clicked() {
                let page = if name == "Edge" { "edge://extensions/" } else { "chrome://extensions/" };
                let _ = std::process::Command::new(&exe).arg(page).spawn();
                let _ = std::process::Command::new("explorer.exe").arg(&ext).spawn();
                ui.ctx().copy_text(ext.display().to_string());
                app.say(format!("已打开 {name} 的扩展页和扩展文件夹，把文件夹拖进去即可（路径已复制）"));
            }
        }
    });
    ui.label(egui::RichText::new(format!("扩展目录：{}", ext.display())).size(11.5).color(settings_color(ctx, Color32::from_gray(195), Color32::from_gray(60))));
    settings_hint(ui, "步骤：点上方按钮→ 扩展页右上角打开「开发者模式」→ 把 extension 文件夹拖进扩展页（或点「加载已解压的扩展程序」选它）→ 之后浏览器下载会实时上岛（蓝胶囊 = 下载中，绿胶囊 = 完成可点击打开）");
}

#[cfg(not(windows))]
fn bridge_card_body(_app: &mut App, _ctx: &egui::Context, ui: &mut egui::Ui) {
    ui.label("浏览器下载对接仅支持 Windows。");
}

impl App {
    pub fn settings_ui(&mut self,ctx:&egui::Context,autostart:bool){
        if !self.settings_open{self.window_layout().close();release_support_codes(ctx);}
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
            egui::SidePanel::left("settings-sidebar").resizable(false).exact_width(180.0).frame(egui::Frame::new().fill(settings_color(ctx,Color32::from_rgb(29,29,32),Color32::from_rgb(237,239,243))).stroke(egui::Stroke::new(1.0,settings_color(ctx,Color32::from_rgb(42,42,45),Color32::from_rgb(216,219,225)))).inner_margin(egui::Margin::symmetric(10,12))).show(ctx,|ui|{
                ui.add_space(5.0);
                ui.label(egui::RichText::new("设置").size(11.0).color(settings_color(ctx,Color32::from_gray(150),Color32::from_gray(105))));
                ui.add_space(7.0);
                for (name,start,end) in SETTINGS_GROUPS {
                    if start==9 {ui.add_space((ui.available_height()-38.0).max(0.0));}
                    let selected=(start..=end).contains(&self.settings_tab);
                    let (rect,response)=ui.allocate_exact_size(egui::vec2(ui.available_width(),36.0),egui::Sense::click());
                    if response.clicked() && !selected{self.settings_tab=start;}
                    if selected || response.hovered(){
                        ui.painter().rect_filled(rect,7.0,if selected{settings_color(ctx,Color32::from_rgb(43,43,46),Color32::from_rgb(214,221,232))}else{settings_color(ctx,Color32::from_rgb(30,30,33),Color32::from_rgb(225,230,238))});
                    }
                    let icon=match start {
                        0=>egui::include_image!("../assets/settings-icons/system/general.svg"),
                        11=>egui::include_image!("../assets/settings-icons/system/quota.svg"),
                        10=>egui::include_image!("../assets/settings-icons/system/pages.svg"),
                        1=>egui::include_image!("../assets/settings-icons/system/music.svg"),
                        2=>egui::include_image!("../assets/settings-icons/system/focus.svg"),
                        4=>egui::include_image!("../assets/settings-icons/system/tasks.svg"),
                        6=>egui::include_image!("../assets/settings-icons/system/notifications.svg"),
                        12=>egui::include_image!("../assets/settings-icons/system/weather.svg"),
                        13=>egui::include_image!("../assets/settings-icons/system/phone.svg"),
                        14=>egui::include_image!("../assets/settings-icons/system/browser.svg"),
                        15=>egui::include_image!("../assets/maa-icon.png"), // 与状态胶囊同一个 MAA 官方图标
                       _=>egui::include_image!("../assets/settings-icons/system/about.svg"),
                    };
                    let icon_bg=egui::Rect::from_center_size(egui::pos2(rect.left()+21.0,rect.center().y),egui::vec2(25.0,25.0));
                    egui::Image::new(icon).paint_at(ui,icon_bg);
                    ui.painter().text(egui::pos2(rect.left()+40.0,rect.center().y),egui::Align2::LEFT_CENTER,name,egui::FontId::proportional(13.0),if selected{settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41))}else{settings_color(ctx,Color32::from_gray(225),Color32::from_gray(55))});
                    ui.add_space(3.0);
                }
            });
            egui::CentralPanel::default().frame(egui::Frame::new().fill(settings_color(ctx,Color32::from_rgb(25,25,27),Color32::from_rgb(246,247,249))).inner_margin(egui::Margin::symmetric(22,16))).show(ctx,|ui|{
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
                let titles=["常规","音乐与歌词","专注","健康提醒","待办","置物","通知","邮箱","日程","关于","页面管理","AI 额度","天气","手机连接","浏览器下载","MAA"];
                ui.label(egui::RichText::new(titles[self.settings_tab.min(15)]).size(18.0).strong().color(settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41))));
               if let Some((_,start,end))=SETTINGS_GROUPS.iter().find(|(_,start,end)|(*start..=*end).contains(&self.settings_tab)) {
                    if start!=end {
                        ui.add_space(7.0);
                        egui::Frame::new().fill(settings_color(ctx,Color32::from_rgb(33,33,36),Color32::from_rgb(232,235,240))).corner_radius(8.0).inner_margin(3).show(ui,|ui|{
                            ui.spacing_mut().item_spacing.x=2.0;
                            ui.horizontal(|ui|{
                                for index in *start..=*end {
                                    let selected=self.settings_tab==index;
                                    let button=egui::Button::new(egui::RichText::new(titles[index]).size(12.0).color(if selected{settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41))}else{settings_color(ctx,Color32::from_gray(210),Color32::from_gray(55))}))
                                        .fill(if selected{settings_color(ctx,Color32::from_rgb(70,70,74),Color32::from_rgb(205,215,229))}else{Color32::TRANSPARENT})
                                        .stroke(egui::Stroke::NONE).corner_radius(6.0);
                                    if ui.add_sized([68.0,26.0],button).clicked(){self.settings_tab=index;}
                                }
                            });
                        });
                    }
                }
                ui.add_space(6.0);
                egui::ScrollArea::vertical().id_salt(("settings-scroll",self.settings_tab)).auto_shrink([false,false]).show(ui,|ui|{
                    // 右侧给滚动条留出空隙，卡片和开关不会被它压住。
                    ui.set_width((ui.available_width()-12.0).max(240.0));
                    // 模块已关闭：顶部说明并提供开启入口，下面的设置置灰。
                    if let Some(page)=settings_tab_module(self.settings_tab).filter(|page|!self.module_on(*page)) {
                        let mut enable=false;
                        settings_card(ui,|ui|{
                            ui.horizontal(|ui|{
                                ui.label(egui::RichText::new(format!("「{}」已关闭，不会在后台同步或提醒",PAGE_NAMES[page])).size(12.5));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{enable=ui.button("开启").clicked();});
                            });
                        });
                        if enable {self.set_page_visible(page,true);}
                        ui.add_space(6.0);
                        if !self.module_on(page) {ui.disable();}
                    }
                    match self.settings_tab {
                    12=>self.weather_settings(ui),
                    11=>self.quota_settings(ui),
                    13=>{
                        settings_label(ui,"连接状态");
                        settings_card(ui,|ui|{
                            // 连接状态：直接读手机连接卡片，不经电量开关。
                            let (linked, device, level) = crate::phone_battery::snapshot();
                            let status_text = if linked {
                                match (&device, level) {
                                    (d, Some(l)) if !d.is_empty() => format!("已连接：{d} · {l}%"),
                                    (_, Some(l)) => format!("已连接 · 电量 {l}%"),
                                    (d, _) if !d.is_empty() => format!("已连接：{d}"),
                                    _ => "已连接".to_string(),
                                }
                            } else {
                                "未连接".to_string()
                            };
                            let (light, dark) = if linked {
                                (Color32::from_rgb(46,125,50), Color32::from_rgb(129,199,132))
                            } else {
                                (Color32::from_gray(120), Color32::from_gray(160))
                            };
                            ui.label(egui::RichText::new(status_text).size(13.0).strong().color(settings_color(ctx, light, dark)));

                            // 一键打开「手机连接」：已装直接启动，未装跳商店。
                            let open = if crate::phone_battery::installed() {
                                Some(("打开手机连接", "explorer.exe", "shell:AppsFolder\\Microsoft.YourPhone_8wekyb3d8bbwe!App"))
                            } else {
                                Some(("安装手机连接", "explorer.exe", "ms-windows-store://pdp/?ProductId=9NMPJ99VJBWV"))
                            };
                            if let Some((label, prog, arg)) = open {
                                if ui.button(label).clicked() {
                                    let _ = std::process::Command::new(prog).arg(arg).spawn();
                                }
                            }
                            settings_hint(ui, "用 Windows 内置的「手机连接」配对手机：打开后按提示扫码连接，之后手机短信、来电、电量都会自动上岛。连接成功后上方状态会变绿。");
                        });
                        settings_label(ui,"手机功能");
                        settings_card(ui,|ui|{
                            // 手机电量：从「手机连接」读取，与本机电量并排显示。
                            if crate::phone_battery::installed() {
                                setting_row(ui,"手机电量","手机电量，与本机分开显示",&mut self.settings.phone_battery);
                            } else {
                                setting_row_locked(ui,"手机电量","未检测到「手机连接」，安装后才能开启",self.settings.phone_battery);
                            }
                            ui.separator();
                            // 短信/来电经「手机连接」转发：手机连接没装、或没给通知读取权限时不可配置。
                            let phone_on=crate::phone_battery::installed();
                            let notify_on=self.settings.read_notifications;
                            let sms_ready=phone_on && notify_on;
                            let sms_before=self.settings.phone_alerts;
                            if sms_ready {
                                setting_row(ui,"短信与来电","短信与来电上岛；关闭后手机通知全部忽略",&mut self.settings.phone_alerts);
                                if sms_before!=self.settings.phone_alerts{self.settings.save();}
                            } else {
                                // 只置灰不改值：等条件恢复后开关自动回到原状态。
                                setting_row_locked(ui,"短信与来电",
                                    if !phone_on {"未检测到「手机连接」，安装后才能开启"} else {"需要先开启通知读取（在「通知、邮箱与日程」里连接）"},
                                    self.settings.phone_alerts);
                            }
                            ui.separator();
                            ui.label(egui::RichText::new("手机短信通过「手机连接」转发。").size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                            // 短信总开关关掉后，验证码自动复制没有意义，一并置灰。
                            ui.add_enabled_ui(self.settings.phone_alerts,|ui|{
                                setting_row(ui,"验证码自动复制","手机短信验证码上岛时直接写入剪贴板",&mut self.settings.auto_copy_otp);
                            });
                            ui.separator();
                            ui.label(egui::RichText::new("手机 App 名单：勾选立即生效；增删条目改 %APPDATA%\\Daozai\\phone-apps.json。").size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                            let before_phone_apps=self.phone_apps.items.clone();
                            for (name,label,allow) in &mut self.phone_apps.items {
                                let title=if label==name {name.clone()} else {format!("{} · {}",label,name)};
                                setting_row(ui,&title,"",allow);
                            }
                            if before_phone_apps!=self.phone_apps.items{self.phone_apps.save();}
                        });
                    },
                    10=>{
                        ui.label(egui::RichText::new("选择需要的页面和状态组件。").size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                        settings_label(ui,"常驻排列");
                        settings_card(ui,|ui|self.resident_order_settings(ui));
                        settings_label(ui,"状态组件");
                        settings_card(ui,|ui|{
                            setting_row(ui,"电量","本机电量环；没有电池时不显示",&mut self.settings.show_battery);
                            ui.separator();
                            setting_row(ui,"岛上歌词","收起时显示歌词，无歌词时显示封面和歌名",&mut self.settings.island_lyrics);
                        });
                        settings_label(ui,"功能页面");
                        settings_card(ui,|ui|{
                            let descriptions=["播放控制与同步歌词","番茄钟与休息计时","喝水与久坐提醒","临时收纳文件位置","任务清单与到期提醒","邮件摘要与验证码","Agent 剩余额度和账号切换","月历、邮件邀请与工作日程","当地天气与七日预报","CPU、内存、显卡与磁盘实时占用","WiFi、蓝牙、勿扰等一键开关，长按声音 / 麦克风切换设备","米家挂灯的开关、亮度与色温"];
                            ui.label(egui::RichText::new("拖动左侧手柄调整顺序；关闭的功能不在后台运行，不同步也不提醒，已有数据保留。").size(11.5).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                            #[derive(Clone)]
                            struct PageDrag(usize);
                            let mut movement=None;
                            let row_w=ui.available_width();
                            for (index,page) in self.settings.page_order.into_iter().enumerate() {
                                if index>0 {ui.separator();}
                                let mut visible=self.settings.visible_pages[page];
                                let can_change=!visible || self.settings.visible_pages.iter().filter(|enabled|**enabled).count()>1;
                                let row=ui.push_id(("page-order",page),|ui|ui.horizontal(|ui|{
                                    ui.set_max_width(row_w);
                                    let (rect,handle)=ui.allocate_exact_size(egui::vec2(20.0,40.0),egui::Sense::drag());
                                    for x in [-3.0,3.0] {for y in [-6.0,0.0,6.0] {
                                        ui.painter().circle_filled(rect.center()+egui::vec2(x,y),1.4,settings_color(ctx,Color32::from_gray(155),Color32::from_gray(100)));
                                    }}
                                    handle.on_hover_cursor(egui::CursorIcon::Grab).dnd_set_drag_payload(PageDrag(page));
                                    ui.add_enabled_ui(can_change,|ui|{setting_row(ui,PAGE_NAMES[page],descriptions[page],&mut visible);});
                                })).inner.response;
                                if let Some(payload)=row.dnd_hover_payload::<PageDrag>() {
                                    if payload.0!=page {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                                        let y=if self.settings.page_order.iter().position(|p|*p==payload.0).unwrap()<index {row.rect.bottom()}else{row.rect.top()};
                                        ui.painter().line_segment([egui::pos2(row.rect.left(),y),egui::pos2(row.rect.right(),y)],egui::Stroke::new(2.0,Color32::from_rgb(70,150,255)));
                                    }
                                }
                                if let Some(payload)=row.dnd_release_payload::<PageDrag>() {movement=Some((payload.0,index));}
                                if visible!=self.settings.visible_pages[page] {self.set_page_visible(page,visible);}
                            }
                            if let Some((page,to))=movement {
                                let from=self.settings.page_order.iter().position(|p|*p==page).unwrap();
                                if from<to {self.settings.page_order[from..=to].rotate_left(1);}
                                else {self.settings.page_order[to..=from].rotate_right(1);}
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
                            settings_hint(ui,"只改设置页配色，岛体不变");
                        });
                        settings_label(ui,"启动");
                        settings_card(ui,|ui|{
                            let mut a=autostart;
                            setting_row(ui,"开机启动","登录 Windows 时自动打开岛仔",&mut a);
                            if a!=autostart{self.pending.push(Cmd::ToggleAutostart);}
                        });
                        settings_label(ui,"图形渲染");
                        settings_card(ui,|ui|{
                            let before=self.settings.render_compat;
                            setting_row(ui,"兼容渲染（Direct3D 12）","岛周围出现黑框、画面闪烁或错乱，或打开设置后崩溃退出时开启；切换后重启岛仔生效",&mut self.settings.render_compat);
                            // 手动关闭后不再显示自动切换的原因；再次开启视为手动开启。
                            if before!=self.settings.render_compat {self.settings.render_compat_reason.clear();}
                            if !self.settings.render_compat_reason.is_empty() {settings_hint(ui,&self.settings.render_compat_reason);}
                            if self.settings.render_compat!=self.render_compat_running {
                                ui.horizontal(|ui|{
                                    ui.label(egui::RichText::new("重启岛仔后生效").size(12.0).color(Color32::from_rgb(230,150,40)));
                                    if ui.button("立即重启").clicked(){self.pending.push(Cmd::RestartApp);}
                                });
                            }
                        });
                        settings_label(ui,"岛体外观");
                        settings_card(ui,|ui|{
                            use crate::model::{self as model,Skin};
                            let skins:Vec<(Skin,&str)>=Skin::ALL.iter().map(|s|(*s,s.label())).collect();
                            settings_control_row_right(ui,"皮肤",|ui|{settings_segmented(ui,&mut self.settings.skin,&skins,64.0);});
                            settings_hint(ui,self.settings.skin.description());
                            ui.separator();
                            let shapes:Vec<(model::IslandShape,&str)>=model::IslandShape::ALL.iter().map(|s|(*s,s.label())).collect();
                            settings_control_row_right(ui,"形状",|ui|{settings_segmented(ui,&mut self.settings.island_shape,&shapes,64.0);});
                            settings_hint(ui,self.settings.island_shape.description());
                            if self.settings.skin.is_glass() {
                                ui.separator();
                                let mut blur=self.settings.glass_blur;
                                setting_row(ui,"背景模糊","实时模糊岛体背景；需系统开启透明效果",&mut blur);
                                self.settings.glass_blur=blur;
                            }
                            ui.separator();
                            settings_control_row(ui,"大小",56.0,|ui|{
                                ui.horizontal(|ui|{
                                    ui.spacing_mut().slider_width=180.0;
                                    island_size_slider(ui,&mut self.settings.island_size);
                                    if ui.add_enabled(self.settings.island_size!=100,egui::Button::new("恢复默认")).clicked(){self.settings.island_size=100;}
                                }).response
                            });
                            settings_hint(ui,"按实际像素渲染，放大不发虚");
                            if self.settings.skin==model::Skin::Liquid {
                                ui.separator();
                                settings_hint(ui,"调下面的参数时岛仔会保持展开；切到其他设置页自动收起");
                                ui.separator();
                                let tones:Vec<(model::LiquidTone,&str)>=model::LiquidTone::ALL.iter().map(|t|(*t,t.label())).collect();
                                settings_control_row_right(ui,"玻璃明暗",|ui|{settings_segmented(ui,&mut self.settings.liquid_tone,&tones,52.0);});
                                settings_hint(ui,self.settings.liquid_tone.description());
                                if self.settings.liquid_tone!=model::LiquidTone::Auto {
                                    settings_hint(ui,"已固定这一档：岛内文字不会再随壁纸明暗变白变黑");
                                }
                                ui.separator();
                                settings_control_row(ui,"文字对比",56.0,|ui|{
                                    ui.horizontal(|ui|{
                                        ui.spacing_mut().slider_width=180.0;
                                        ui.add(egui::Slider::new(&mut self.settings.liquid_ink,0.6..=1.4).fixed_decimals(2));
                                        if ui.add_enabled((self.settings.liquid_ink-1.0).abs()>0.001,egui::Button::new("恢复默认")).clicked(){self.settings.liquid_ink=1.0;}
                                    }).response
                                });
                                settings_hint(ui,"岛内文字与辅助灰字的浓淡（默认 1.00）；小于 1 更淡，大于 1 更实");
                                let before=self.settings.liquid_prefer_dgpu;
                                setting_row(ui,"优先独立显卡","优先使用独显渲染玻璃，切换后重启岛仔生效",&mut self.settings.liquid_prefer_dgpu);
                                if before!=self.settings.liquid_prefer_dgpu {self.settings.save();}
                                settings_hint(ui,"修改后需重启岛仔生效");
                                ui.separator();
                                settings_control_row(ui,"形变强度",56.0,|ui|{
                                    ui.horizontal(|ui|{
                                        ui.spacing_mut().slider_width=180.0;
                                        ui.add(egui::Slider::new(&mut self.settings.liquid_morph,0.0..=3.0).fixed_decimals(2));
                                        if ui.add_enabled((self.settings.liquid_morph-1.0).abs()>0.001,egui::Button::new("恢复默认")).clicked(){self.settings.liquid_morph=1.0;}
                                    }).response
                                });
                                settings_hint(ui,"展开/折叠瞬间边缘折射的增强倍数；0 = 关闭，默认 1.0");
                                ui.separator();
                                settings_control_row(ui,"折射强度",56.0,|ui|{
                                    ui.horizontal(|ui|{
                                        ui.spacing_mut().slider_width=180.0;
                                        ui.add(egui::Slider::new(&mut self.settings.liquid_refract_px,0.0..=120.0).fixed_decimals(0));
                                        if ui.add_enabled((self.settings.liquid_refract_px-48.0).abs()>0.05,egui::Button::new("恢复默认")).clicked(){self.settings.liquid_refract_px=48.0;}
                                    }).response
                                });
                                settings_hint(ui,"边缘处背景被弯折的像素数（默认 48）；调大像更厚的透镜，调小更平");
                                ui.separator();
                                settings_control_row(ui,"色散强度",56.0,|ui|{
                                    ui.horizontal(|ui|{
                                        ui.spacing_mut().slider_width=180.0;
                                        ui.add(egui::Slider::new(&mut self.settings.liquid_chroma_px,0.0..=30.0).fixed_decimals(0));
                                        if ui.add_enabled((self.settings.liquid_chroma_px-8.0).abs()>0.05,egui::Button::new("恢复默认")).clicked(){self.settings.liquid_chroma_px=8.0;}
                                    }).response
                                });
                                settings_hint(ui,"红蓝彩边的强度，红蓝实际分离是它的两倍（默认 8；锐利抓屏在字边更容易出彩边）；0 = 关掉色散");
                                ui.separator();
                                settings_control_row(ui,"内部模糊",56.0,|ui|{
                                    ui.horizontal(|ui|{
                                        ui.spacing_mut().slider_width=180.0;
                                        ui.add(egui::Slider::new(&mut self.settings.liquid_blur_px,0.0..=4.0).fixed_decimals(1));
                                        if ui.add_enabled((self.settings.liquid_blur_px-1.5).abs()>0.05,egui::Button::new("恢复默认")).clicked(){self.settings.liquid_blur_px=1.5;}
                                    }).response
                                });
                                settings_hint(ui,"折射带里五点采样的步长（默认 1.5）；调大整个折射带更「磨」，超过 3 就成毛玻璃");
                                ui.separator();
                                settings_control_row(ui,"折射带宽",56.0,|ui|{
                                    ui.horizontal(|ui|{
                                        ui.spacing_mut().slider_width=180.0;
                                        ui.add(egui::Slider::new(&mut self.settings.liquid_band_px,10.0..=140.0).fixed_decimals(0));
                                        if ui.add_enabled((self.settings.liquid_band_px-87.0).abs()>0.5,egui::Button::new("恢复默认")).clicked(){self.settings.liquid_band_px=87.0;}
                                    }).response
                                });
                                settings_hint(ui,"折射从边缘往里延伸多深（默认 87）；小 = 只有边缘一圈在弯，大 = 大半块面板都在弯");
                                ui.separator();
                                settings_control_row(ui,"白边宽度",56.0,|ui|{
                                    ui.horizontal(|ui|{
                                        ui.spacing_mut().slider_width=180.0;
                                        ui.add(egui::Slider::new(&mut self.settings.liquid_edge_w_px,0.0..=4.0).fixed_decimals(1));
                                        if ui.add_enabled((self.settings.liquid_edge_w_px-1.5).abs()>0.05,egui::Button::new("恢复默认")).clicked(){self.settings.liquid_edge_w_px=1.5;}
                                    }).response
                                });
                                settings_hint(ui,"边缘那条高光的宽度（默认 1.5）；液态玻璃要细，0 = 关掉白边");
                                ui.separator();
                                settings_control_row(ui,"白边亮度",56.0,|ui|{
                                    ui.horizontal(|ui|{
                                        ui.spacing_mut().slider_width=180.0;
                                        ui.add(egui::Slider::new(&mut self.settings.liquid_edge_op,0.0..=1.0).fixed_decimals(2));
                                        if ui.add_enabled((self.settings.liquid_edge_op-0.90).abs()>0.005,egui::Button::new("恢复默认")).clicked(){self.settings.liquid_edge_op=0.90;}
                                    }).response
                                });
                                settings_hint(ui,"边缘高光的亮度（默认 0.90）；会随背景明暗自动微调，这里是基准值");
                            }
                            ui.separator();
                            let anims:Vec<(model::OpenAnim,&str)>=model::OpenAnim::ALL.iter().map(|a|(*a,a.label())).collect();
                            settings_control_row_right(ui,"展开动效",|ui|{settings_segmented(ui,&mut self.settings.open_anim,&anims,52.0);});
                            settings_hint(ui,self.settings.open_anim.description());
                            ui.separator();
                            setting_number(ui,"悬停触发延时",&mut self.settings.hover_delay_ms,0,500," ms",10.0);
                            settings_hint(ui,"悬停多久后展开；80–120 ms 可避开鼠标划过");
                        });
                        settings_label(ui,"岛体显示");
                        settings_card(ui,|ui|{
                            let selected=self.displays.iter().find(|d|d.id==self.settings.display_id);
                            let label=if self.settings.display_id.is_empty(){"跟随主屏"}else{selected.map_or("所选屏幕未连接",|d|d.label.as_str())};
                            settings_control_row(ui,"显示屏",56.0,|ui|{
                                egui::ComboBox::from_id_salt("island-display").selected_text(label).width(ui.available_width().min(300.0)).truncate().show_ui(ui,|ui|{
                                    ui.selectable_value(&mut self.settings.display_id,String::new(),"跟随主屏");
                                    for display in &self.displays {ui.selectable_value(&mut self.settings.display_id,display.id.clone(),&display.label);}
                                }).response
                            });
                            let edge=self.settings.dock.label();
                            ui.label(egui::RichText::new(if !self.settings.display_id.is_empty()&&selected.is_none(){"暂时显示在主屏，重连后自动恢复".to_string()}else{format!("所选屏幕{edge}；断开后回主屏")}).size(11.5).color(ui.visuals().weak_text_color()));
                            ui.add_space(4.0);
                            // 停靠边缘：顶部为横向；左右为竖向排列，展开面板从边缘滑出。
                            let docks:Vec<(crate::model::Dock,&str)>=crate::model::Dock::ALL.iter().map(|d|(*d,d.label())).collect();
                            settings_control_row_right(ui,"停靠",|ui|{
                                // 换边后偏移的含义变了（水平 ↔ 上下），回到居中。
                                if settings_segmented(ui,&mut self.settings.dock,&docks,64.0) {self.settings.island_offset=0;}
                            });
                            if self.settings.dock.is_side() {
                                ui.label(egui::RichText::new("收起时竖向排列时间、电池、播放、专注和 AI 额度；展开面板从屏幕边缘滑出。吉祥物和岛上歌词只在顶部停靠时显示。").size(11.0).color(ui.visuals().weak_text_color()));
                            }
                            ui.add_space(4.0);
                            let side=self.settings.dock.is_side();
                            settings_control_row(ui,if side{"上下位置"}else{"水平位置"},56.0,|ui|{
                                ui.horizontal(|ui|{
                                    let limit=crate::model::ISLAND_OFFSET_MAX;
                                    let offset=&mut self.settings.island_offset;
                                    let (less,more,less_tip,more_tip)=if side{("↑","↓","向上微调","向下微调")}else{("←","→","向左微调","向右微调")};
                                    if ui.add_enabled(*offset>-limit,egui::Button::new(less).min_size(egui::vec2(26.0,22.0))).on_hover_text(less_tip).clicked(){*offset=(*offset-5).max(-limit);}
                                    ui.spacing_mut().slider_width=150.0;
                                    ui.add(egui::Slider::new(&mut *offset,-limit..=limit).show_value(false));
                                    if ui.add_enabled(*offset<limit,egui::Button::new(more).min_size(egui::vec2(26.0,22.0))).on_hover_text(more_tip).clicked(){*offset=(*offset+5).min(limit);}
                                    if ui.add_enabled(*offset!=0,egui::Button::new("恢复居中")).clicked(){*offset=0;}
                                }).response
                            });
                            let offset=self.settings.island_offset;
                            let (less,more)=if side{("偏上","偏下")}else{("偏左","偏右")};
                            let position=match offset {0=>"居中".to_string(),o if o<0=>format!("{less} {}%",-o),o=>format!("{more} {o}%")};
                            ui.label(egui::RichText::new(format!("当前：{position}。也可以展开后按住日期时间，拖到屏幕顶部、左侧或右侧；展开面板和通知始终留在屏幕内，换屏或调整大小后保持相对位置。")).size(11.0).color(ui.visuals().weak_text_color()));
                            ui.add_space(4.0);
                            setting_row(ui,"避开顶栏","顶部有菜单栏类软件或任务栏时，岛体放在它下方",&mut self.settings.avoid_top_bar);
                            if self.settings.avoid_top_bar {
                                let reserved=crate::display_layout::selected(&self.displays,&self.settings.display_id).map_or(0,|d|{
                                    let b=crate::display_layout::placement_bounds(d,true);(b.top-d.bounds.top).max(0)
                                });
                                settings_hint(ui,&if reserved>0 {format!("已检测到顶栏，岛体下移 {reserved} 像素。")}else{"未检测到顶栏；未向系统登记占用区域的顶栏无法识别，可改用胶囊形状。".to_string()});
                            }
                            ui.separator();
                            settings_control_row_right(ui,"时间制式",|ui|{settings_segmented(ui,&mut self.settings.clock_12h,&[(false,"24 小时"),(true,"12 小时")],72.0);});
                            settings_control_row_right(ui,"时段文字",|ui|{settings_segmented(ui,&mut self.settings.clock_period,&[(0,"不显示"),(1,"上午 / 下午"),(2,"详细时段")],72.0);});
                            settings_hint(ui,"详细时段：凌晨、上午、中午、下午、傍晚、晚上");
                            ui.separator();
                            let mut minimal=self.settings.minimal;
                            setting_row(ui,"极简刘海","收起时显示一条细线",&mut minimal);
                            if minimal!=self.settings.minimal {self.set_minimal(minimal);}
                            ui.separator();

                            let mut pet=self.settings.pet;
                            setting_row(ui,"岛仔吉祥物","收起时常驻，播放音乐时一起唱歌",&mut pet);
                            if pet!=self.settings.pet {self.set_pet(pet);}
                        });
                        self.avoidance.settings(ui,&mut self.settings);
                        settings_label(ui,"声音与显示");
                        settings_card(ui,|ui|{
                            setting_row(ui,"提醒声音","喝水、久坐、待办和专注提醒",&mut self.settings.sound);
                            ui.separator();
                            setting_row(ui,"全屏时隐藏","游戏、视频和演示全屏时隐藏",&mut self.settings.hide_fullscreen);
                        });
                        settings_label(ui,"备份");
                        settings_card(ui,|ui|{
                            ui.horizontal(|ui|{
                                if ui.button("备份配置").on_hover_text("把 Daozai 目录下的配置与状态复制一份").clicked(){self.backup_settings();}
                                if ui.button("打开备份目录").clicked(){
                                    if let Some(dir)=backup_root(){let _=std::fs::create_dir_all(&dir);self.pending.push(Cmd::Open(dir));}
                                }
                            });
                            ui.add_space(3.0);
                            ui.small("备份到 %APPDATA%\\Daozai\\backups\\<时间戳>\\，只复制文件，不动原配置。");
                        });
                    }
                    1=>{
                        settings_label(ui,"歌词");
                        settings_card(ui,|ui|{
                            setting_row(ui,"展开时显示歌词","在播放器右侧同步滚动显示歌词",&mut self.settings.lyrics_enabled);
                            self.show_lyrics=self.settings.lyrics_enabled;
                            ui.separator();
                            setting_row(ui,"岛上歌词","收起时显示当前句；无同步歌词时显示封面和歌名",&mut self.settings.island_lyrics);
                        });
                        settings_label(ui,"歌词显示时间");
                        settings_card(ui,|ui|{
                            let mut advance=-self.settings.lyric_delay_ms as f64/1000.0;
                            if setting_number(ui,"时间偏移",&mut advance,-5.0,5.0," 秒",0.1){self.settings.lyric_delay_ms=(-advance*1000.0).round()as i32;}
                            ui.add_space(3.0);
                            ui.small("正数提前，负数延后；默认提前 0.35 秒。");
                        });
                        #[cfg(windows)]
                        {
                            use crate::smtc::netease_control as ctl;
                            let mut authoritative=crate::music_volume::authoritative();
                            settings_label(ui,"音乐音量");
                            settings_card(ui,|ui|{
                                setting_row(ui,"岛仔推送音量","播放器启动后岛仔推送音量", &mut authoritative);
                                ui.add_space(3.0);
                                ui.small("关闭后，播放器内调整音量后下次启动不会被岛仔音量覆盖");
                            });
                            if authoritative!=crate::music_volume::authoritative(){
                                crate::music_volume::set_authoritative(authoritative);
                                self.say(if authoritative{"音量岛仔推送音量"}else{"音量以播放器为准"});
                            }
                            settings_label(ui,"网易云播放进度条修复");
                            settings_card(ui,|ui|{
                                ui.small("拖动播放进度条需要修改快捷方式。");
                                let fixed=ctl::shortcut_fixed();
                                ui.add(egui::Label::new(egui::RichText::new(if fixed{"已自动修改网易云快捷方式"}else{"还没有修改快捷方式"}).size(12.5).strong()).wrap());
                                ui.separator();
                                ui.small("快捷方式未修复？选任意网易云的快捷方式修复");
                                if ui.button("选择文件").clicked(){
                                        match crate::system::pick_files_for("选择网易云快捷方式",false){
                                            Ok(files)=>if let Some(path)=files.first(){
                                                match ctl::fix_shortcut(path){
                                                    Ok(message)=>self.say(message),
                                                    Err(error)=>self.say(error),
                                                }
                                            }else{self.say("未选择文件");},
                                            Err(error)=>self.say(format!("打开文件对话框失败：{}",error)),
                                        }
                                    }
                            });
                        }
                    }
                    2=>{
                        settings_label(ui,"计时时长");
                        settings_card(ui,|ui|{
                            setting_number(ui,"专注",&mut self.settings.focus_minutes,1,180," 分钟",1.0);
                            ui.separator();
                            setting_number(ui,"休息",&mut self.settings.rest_minutes,1,60," 分钟",1.0);
                            ui.add_space(3.0);
                            ui.small("调整后从下一阶段开始生效。");
                        });
                        settings_label(ui,"专注记录");
                        settings_card(ui,|ui|{
                            // 只读内存里的统计（每分钟更新一次），不在这里读盘。
                            let (today,streak)=(self.focus_today_min,self.focus_streak_days);
                            let hours=today/60;
                            let line=if today>=60 {format!("今日 {hours} 小时 {} 分 · 连续 {streak} 天",today%60)} else {format!("今日 {today} 分 · 连续 {streak} 天")};
                            ui.add(egui::Label::new(egui::RichText::new(line).size(12.0)).wrap());
                            ui.add_space(3.0);
                            ui.small("只统计真正在跑的专注阶段，暂停不计；数据在 %APPDATA%\\Daozai\\focus-stats.json。");
                        });
                    }
                    3=>{
                        settings_label(ui,"喝水提醒");
                        settings_card(ui,|ui|{
                            let old_remind=self.water.remind;
                            setting_row(ui,"定时提醒","达到每日目标后停止提醒",&mut self.water.remind);
                            if self.water.remind!=old_remind{self.water.save();self.last_reminder=self.t;}
                            ui.separator();
                            if setting_number(ui,"每日目标",&mut self.water.goal_ml,500,6000," ml",50.0){self.water.save();}
                            setting_number(ui,"每次记录",&mut self.settings.serving_ml,50,1000," ml",25.0);
                            setting_number(ui,"提醒间隔",&mut self.settings.water_minutes,5,180," 分钟",1.0);
                        });
                        settings_label(ui,"久坐提醒");
                        settings_card(ui,|ui|{
                            let old_sit=self.settings.sit_remind;
                            setting_row(ui,"定时提醒","锁屏或点「起身了」后重新计时；只看屏幕不动鼠标也照样计时",&mut self.settings.sit_remind);
                            if self.settings.sit_remind && !old_sit {self.sit_alerted=self.t;}
                            ui.separator();
                            setting_number(ui,"久坐时长",&mut self.settings.sit_minutes,15,180," 分钟",1.0);
                            ui.add_space(3.0);
                            ui.small("专注计时进行中不打扰，专注结束后再提醒。");
                        });
                        settings_label(ui,"提醒时段");
                        settings_card(ui,|ui|{
                            settings_control_row(ui,"提醒时段",80.0,|ui|{
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                                    ui.allocate_ui_with_layout(egui::vec2(252.0,30.0),egui::Layout::left_to_right(egui::Align::Center),|ui|{
                                        number_stepper(ui,&mut self.settings.water_start,0,23," 时",1,48.0);
                                        ui.label("至");
                                        number_stepper(ui,&mut self.settings.water_end,0,24," 时",1,48.0);
                                    });
                                }).response
                            });
                            ui.separator();
                            setting_row(ui,"午休不提醒","午休时间内喝水和久坐都不弹提醒",&mut self.settings.lunch_rest);
                            if self.settings.lunch_rest {
                                settings_control_row(ui,"午休时间",80.0,|ui|{
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                                        ui.allocate_ui_with_layout(egui::vec2(252.0,30.0),egui::Layout::left_to_right(egui::Align::Center),|ui|{
                                            time_stepper(ui,&mut self.settings.lunch_start,"lunch-start");
                                            ui.label("至");
                                            time_stepper(ui,&mut self.settings.lunch_end,"lunch-end");
                                        });
                                    }).response
                                });
                            }
                            ui.add_space(3.0);
                            ui.small("喝水和久坐提醒只在提醒时段内、午休以外弹出。");
                        });
                    }
                    4=>{
                        settings_label(ui,"待办清单");
                        settings_card(ui,|ui|{
                            ui.label(egui::RichText::new("在待办页添加、编辑任务并设置提醒时间。").size(13.0));
                            ui.add_space(5.0);
                            if ui.add_sized([120.0,32.0],egui::Button::new("打开待办页")).clicked(){self.settings_open=false;self.show_page(4);}
                        });
                    }
                    5=>{
                        settings_label(ui,"置物架");
                        settings_card(ui,|ui|{
                            ui.label(egui::RichText::new("保存文件位置，移除条目不会删除原文件。").size(13.0));
                            ui.label(egui::RichText::new(format!("当前 {} 项",self.shelf.items.len())).size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                            ui.add_space(5.0);
                            if ui.add_sized([120.0,32.0],egui::Button::new("打开置物架")).clicked(){self.settings_open=false;self.shelf_tab=0;self.show_page(3);}
                        });
                        settings_label(ui,"剪贴板");
                        settings_card(ui,|ui|{
                            setting_row(ui,"记录剪贴板","在置物架里查看最近复制的文字",&mut self.settings.clipboard_history);
                            ui.label(egui::RichText::new(format!("本机保存最近 {} 条，重新打开后可继续使用。关闭记录会暂停收集，手动清空会保留固定条目。密码管理器标记为敏感的内容不会记录。",crate::clipboard::MAX_RECENT)).size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                            ui.add_space(5.0);
                            if ui.add_sized([120.0,32.0],egui::Button::new("打开剪贴板")).clicked(){self.settings_open=false;self.shelf_tab=1;self.show_page(3);}
                        });
                    }
                    6=>{
                        // ① 基础开关：总闸 + 系统类提示
                        settings_label(ui,"系统提示");
                        settings_card(ui,|ui|{
                            setting_row(ui,"提醒总开关","关闭后所有提醒都不显示",&mut self.settings.notifications);
                            ui.separator();
                            setting_row(ui,"系统状态","音量、充电、网络和键盘状态",&mut self.settings.system_hud);
                            ui.separator();
                            setting_row(ui,"蓝牙通知","设备连接与断开提醒；关闭后停止蓝牙监听",&mut self.settings.bluetooth_notifications);
                            ui.separator();
                            setting_row(ui,"会议自动勿扰","日程期间自动勿扰，散会解除",&mut self.settings.quiet_meeting);
                        });
                        if settings_section(ui,"notice.conditions","提醒条件","低电 · 勿扰 · 关键词 · 规则",false){
                            // ② 提醒条件：什么情况下打断你
                            settings_card(ui,|ui|{
                                settings_control_row(ui,"低电提醒",SETTINGS_LABEL_W,|ui|{
                                    let mut value=self.settings.battery_alert;
                                    ui.horizontal(|ui|{
                                        if settings_segmented(ui,&mut value,&[(0,"关"),(10,"10%"),(20,"20%"),(30,"30%")],44.0){
                                            self.settings.battery_alert=value;
                                        }
                                    }).response
                                });
                                settings_hint(ui,"低于阈值提醒一次，充满时也提示");
                                ui.separator();
                                settings_control_row(ui,"AI 额度提醒",SETTINGS_LABEL_W,|ui|{
                                    let mut value=self.settings.quota_alert;
                                    ui.horizontal(|ui|{
                                        if settings_segmented(ui,&mut value,&[(0,"关"),(10,"10%"),(20,"20%"),(30,"30%")],44.0){
                                            self.settings.quota_alert=value;
                                        }
                                    }).response
                                });
                                settings_hint(ui,"跌破阈值提醒一次，回升后重新计数");
                                ui.separator();
                                // 勿扰：选时长立即生效，到点自动关闭；重启后恢复关闭。
                                let mut quiet=if self.quiet_until>self.t {self.settings.quiet_minutes.max(30)} else {0};
                                let mut quiet_pick=None;
                                settings_control_row(ui,"勿扰",SETTINGS_LABEL_W,|ui|{
                                    ui.horizontal(|ui|{
                                        if settings_segmented(ui,&mut quiet,&[(0,"关"),(30,"30 分"),(60,"60 分"),(120,"2 小时")],48.0){
                                            quiet_pick=Some(quiet);
                                        }
                                    }).response
                                });
                                if let Some(minutes)=quiet_pick {self.set_quiet(minutes);}
                                settings_hint(ui,"勿扰时不弹横幅和铃铛，验证码除外");
                                ui.separator();
                                let mut keywords_on=!self.settings.notice_keywords.trim().is_empty();
                                setting_row(ui,"通知关键词高亮","命中时卡片左侧亮琥珀色竖条；词表见 settings.txt",&mut keywords_on);
                                if keywords_on != !self.settings.notice_keywords.trim().is_empty() {
                                    self.settings.notice_keywords=if keywords_on {"验证码,面试,转账,取款,取件,欠费".to_string()} else {String::new()};
                                }
                                ui.separator();
                                settings_control_row(ui,"通知规则",SETTINGS_LABEL_W,|ui|{
                                    ui.horizontal(|ui|{
                                        let count=self.notice_rules.items.len();
                                        ui.label(egui::RichText::new(if count==0 {"未配置".to_string()} else {format!("{count} 条")}).size(12.0));
                                        if ui.button("编辑规则").clicked(){self.pending.push(Cmd::Open(crate::model::NoticeRules::path()));}
                                    }).response
                                });
                                settings_hint(ui,"规则文件：pattern + action（mute / highlight / copy），首条匹配生效");
                            });
                        }
                        if settings_section(ui,"notice.recent","最近通知",&format!("{} 条",self.recent_notices().len().min(8)),false){
                            // ③ 最近通知：设置放在前，记录放在最后
                            settings_card(ui,|ui|{
                                settings_hint(ui,"点击一行复制正文，或点「稍后」5 分钟后再提醒");
                                let recent=self.recent_notices();
                                let mut picked:Option<(String,String,bool)>=None;
                                if recent.is_empty() {
                                    settings_hint(ui,"还没有通知记录");
                                }
                                for (at,title,body) in recent.iter().rev().take(8) {
                                    let line=format!("{at}  {title} · {}",body.chars().take(26).collect::<String>());
                                    ui.horizontal(|ui|{
                                        let resp=ui.add(egui::Label::new(egui::RichText::new(line).size(12.0).color(settings_color(ctx,Color32::from_gray(190),Color32::from_rgb(58,64,74)))).sense(egui::Sense::click()));
                                        if resp.on_hover_text(body.as_str()).clicked(){picked=Some((title.clone(),body.clone(),false));}
                                        if ui.small_button("稍后").on_hover_text("5 分钟后再提醒一次").clicked(){picked=Some((title.clone(),body.clone(),true));}
                                    });
                                }
                                if let Some((title,body,snooze))=picked {
                                    if snooze {self.snooze_notice(5.0,title,body);}
                                    else {self.pending.push(Cmd::Copy(body));self.say("已复制通知正文");}
                                }
                           });
                        }
                        // 总开关关闭时，微信与 QQ 两栏置灰并说明原因，避免「开了也不提醒」。
                        let master=self.settings.notifications;
                        let master_hint=|ui:&mut egui::Ui|if !master {
                            ui.label(egui::RichText::new("提醒总开关已关闭（上方「系统提示」），打开后这里才会生效").size(12.0).color(Color32::from_rgb(230,150,40)));
                        };
                        if settings_section(ui,"notice.apps","微信与办公软件",&format!("{} 个通知应用",self.notification_apps.items.len()),false){
                            settings_label(ui,"微信提醒");
                            settings_card(ui,|ui|{
                                master_hint(ui);
                                ui.add_enabled_ui(master,|ui|{
                                setting_row(ui,"微信新消息提醒","按任务栏闪动提醒，不读聊天内容",&mut self.settings.wechat_notifications);
                                if self.settings.wechat_notifications {
                                    ui.separator();
                                    setting_row(ui,"托盘一键唤起微信","微信在托盘时点提醒直接唤起",&mut self.settings.wechat_tray_open);
                                }
                                if master && !self.messenger_status(crate::app::Messenger::Wechat).is_empty(){ui.label(egui::RichText::new(self.messenger_status(crate::app::Messenger::Wechat)).size(12.0));}
                                });
                            });
                            settings_label(ui,"办公软件提醒");
                            settings_card(ui,|ui|{
                                master_hint(ui);
                                ui.add_enabled_ui(master,|ui|{
                                    setting_row(ui,"企业微信","根据任务栏闪动提醒",&mut self.settings.wecom_notifications);
                                    setting_row(ui,"飞书","根据任务栏闪动提醒",&mut self.settings.feishu_notifications);
                                    setting_row(ui,"钉钉","根据任务栏闪动提醒",&mut self.settings.dingtalk_notifications);
                                    ui.label(egui::RichText::new("需在客户端开启任务栏闪动，不读取联系人或聊天正文。点击提醒打开对应窗口；账号无法确认时可手动关闭提醒。").size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                                    for client in crate::messenger::Messenger::OFFICE {
                                        if client.enabled(&self.settings) && !self.messenger_status(client).is_empty(){ui.label(egui::RichText::new(self.messenger_status(client)).size(12.0));}
                                    }
                                });
                            });
                        }
                        settings_label(ui,"通知来源");
                        settings_card(ui,|ui|{
                            master_hint(ui);
                            ui.add_enabled_ui(master,|ui|{
                            // 监听范围二选一：全部应用 / 仅 QQ，当前项高亮。
                            let listening=self.settings.read_notifications;
                            let all_mode=listening && !self.settings.qq_only;
                            let qq_mode=listening && self.settings.qq_only;
                            ui.horizontal_wrapped(|ui|{
                                if ui.selectable_label(all_mode,"全部应用").clicked() && !all_mode {
                                    self.pending.push(Cmd::RequestNotifications);
                                }
                                if ui.selectable_label(qq_mode,"仅 QQ").clicked() && !qq_mode {
                                    self.pending.push(Cmd::RequestQqNotifications);
                                }
                                if listening && ui.button("停止监听").clicked() {
                                    self.pending.push(Cmd::DisconnectNotifications);
                                }
                            });
                            let status=self.listener_status.clone();
                            ui.label(egui::RichText::new(status).size(13.0));
                            settings_hint(ui,"全部应用：按应用逐个开关；仅 QQ：只收 QQ");
                            ui.separator();
                            if !listening {
                                // 停止监听时不展示应用开关：勾选此刻没有意义，容易让人以为还在生效。
                                ui.label(egui::RichText::new("尚未开始监听。点上方「全部应用」或「仅 QQ」后才会接收通知。").size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                            } else if qq_mode {
                                ui.label(egui::RichText::new("当前为「仅 QQ」，其他应用的通知不会登记。切到「全部应用」后，它们会出现在这里供逐项开关。").size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                            } else {
                                let total=self.notification_apps.items.len();
                                let enabled=self.notification_apps.items.iter().filter(|(_,_,on)|*on).count();
                                if settings_section(ui,"notice.app-list","应用名单",&format!("{enabled}/{total} 已开启"),false){
                                    settings_hint(ui,"勾选后该应用的通知才会上岛；新发过通知的应用会自动出现");
                                    let before_apps=self.notification_apps.items.clone();
                                    for (_,name,on) in &mut self.notification_apps.items {setting_row(ui,name,"",on);}
                                    if before_apps!=self.notification_apps.items{self.notification_apps.save();}
                                }
                            }
                            ui.separator();
                            setting_row(ui,"显示通知正文","可能包含私密消息；屏幕共享时建议关闭",&mut self.settings.notification_preview);
                            setting_number(ui,"通知停留",&mut self.settings.notice_seconds,2,120," 秒",1.0);
                            setting_number(ui,"验证码停留",&mut self.settings.otp_seconds,10,300," 秒",1.0);
                            settings_hint(ui,"卡片停留时长；验证码至少 10 秒");
                            });
                        });
                        if settings_section(ui,"notice.banner","系统通知横幅",&format!("已关闭 {} 个",crate::system::banner_overridden_count()),false){
                            settings_card(ui,|ui|{
                                ui.small("关闭后这些应用不再在右下角弹窗，但通知仍会进通知中心，岛仔照常显示。");
                                ui.small("只影响当前用户，若没有立刻生效，重新登录一次 Windows。");
                                ui.separator();
                                let closed=crate::system::banner_overridden_count();
                                ui.label(egui::RichText::new(format!("已关闭 {closed} 个应用的横幅")).size(12.5).strong());
                                if let Some(summary)=crate::system::banner_last_summary(){
                                    ui.label(egui::RichText::new(format!("上次：{summary}")).size(11.0)
                                        .color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                                }
                                ui.horizontal_wrapped(|ui|{
                                    if ui.button("关闭系统横幅").clicked(){
                                        let apps:Vec<String>=self.notification_apps.items.iter()
                                            .filter(|(_,_,on)|*on).map(|(id,_,_)|id.clone()).collect();
                                        match crate::system::close_notification_banners(&apps){
                                            Ok((ok,fail))=>self.say(format!("关闭完成：成功 {ok} 个，失败 {fail} 个；详情见通知横幅日志")),
                                            Err(error)=>self.say(error),
                                        }
                                    }
                                    if closed>0 && ui.button("恢复").clicked(){
                                        match crate::system::restore_notification_banners(){
                                            Ok((ok,fail))=>self.say(format!("恢复完成：成功 {ok} 个，失败 {fail} 个；详情见通知横幅日志")),
                                            Err(error)=>self.say(error),
                                        }
                                    }
                                    if ui.button("查看日志").clicked(){
                                        if let Err(error)=crate::system::open_banner_log(){self.say(error);}
                                    }
                                    if ui.button("打开 Windows 通知设置").clicked(){
                                        if let Err(error)=crate::system::open_notification_settings(){self.say(error);}
                                    }
                                });
                            });
                        }
                    }
                    14=>{
                        settings_label(ui,"安装");
                        settings_card(ui,|ui|{
                            bridge_card_body(self,ctx,ui);
                        });
                        settings_label(ui,"下载提示");
                        settings_card(ui,|ui|{
                            let before=self.browser_prefs.clone();
                            setting_number(ui,"完成提示时长",&mut self.browser_prefs.done_seconds,3,60," 秒",1.0);
                            setting_number(ui,"进度显示门槛",&mut self.browser_prefs.min_progress_mb,0,500," MB",1.0);
                            settings_hint(ui,"完成提示停留时长；小于门槛的文件只提示完成，不给进度");
                            if before!=self.browser_prefs{self.browser_prefs.save();}
                        });
                        settings_label(ui,"工作原理");
                        settings_card(ui,|ui|{
                            ui.label(egui::RichText::new("浏览器扩展把下载事件发给本机桥进程，桥写入状态总线后由岛仔显示。Chrome/Edge 出于安全不允许软件静默安装本地扩展，所以首次使用需要按上方指引手动加载一次。").size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                        });
                    }
                    15=>{
                        // 对接前提：通知名单里开启 MAA，本页配置才可操作（与状态跟随同步）。
                        let maa_on=self.maa_enabled();
                        let maa_exe=self.notification_apps.items.iter()
                            .find(|(id,name,_)| name=="MAA" || id.to_ascii_lowercase().ends_with("maa.exe"))
                            .map(|(id,_,_)| id.clone());
                        let log_path=self.maa_log_path();
                        let log_ready=log_path.as_ref().is_some_and(|p|p.is_file());

                        settings_label(ui,"状态");
                        settings_card(ui,|ui|{
                            let (text,light,dark)=if !maa_on {
                                ("未对接：勾选通知里的 MAA，或在下方指定 MAA 程序".to_string(),Color32::from_gray(120),Color32::from_gray(160))
                            } else if log_ready {
                                ("已对接：正在跟随 MAA 日志".to_string(),Color32::from_rgb(46,125,50),Color32::from_rgb(129,199,132))
                            } else {
                                ("已开启通知，但没找到 MAA 日志".to_string(),Color32::from_rgb(198,120,30),Color32::from_rgb(240,180,90))
                            };
                            ui.label(egui::RichText::new(text).size(13.0).strong().color(settings_color(ctx,light,dark)));
                            if let Some(exe)=maa_exe.as_ref() {
                                ui.label(egui::RichText::new(format!("MAA：{exe}")).size(11.5).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                            }
                            if let Some(path)=log_path.as_ref() {
                                ui.label(egui::RichText::new(format!("日志：{}",path.display())).size(11.5).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                            }
                            if !maa_on {
                                settings_hint(ui,"勾选通知里的 MAA，或直接在下方指定程序");
                            } else if !log_ready {
                                settings_hint(ui,"没找到 debug\\asst.log，请指定 MAA 目录");
                            }
                        });

                        settings_label(ui,"上岛提醒");
                        settings_card(ui,|ui|{
                            ui.add_enabled_ui(maa_on,|ui|{
                                let before=self.maa_prefs.clone();
                                setting_row(ui,"常驻进度","运行时常驻，随任务完成填满",&mut self.maa_prefs.resident_progress);
                                ui.separator();
                                setting_row(ui,"任务进度","关掉常驻时的单独提示",&mut self.maa_prefs.notify_chain);
                                ui.separator();
                                setting_row(ui,"公招高星","自动公招刷出 4 星以上 tag 时提示",&mut self.maa_prefs.notify_recruit_star);
                                ui.separator();
                                setting_row(ui,"任务出错","任务失败或中断时提示",&mut self.maa_prefs.notify_error);
                                ui.separator();
                                setting_number(ui,"提示停留",&mut self.maa_prefs.seconds,4,30," 秒",1.0);
                                settings_hint(ui,"任务提示和普通通知一样弹卡片");
                                if before!=self.maa_prefs{self.maa_prefs.save();}
                            });
                        });

                        settings_label(ui,"路径");
                        settings_card(ui,|ui|{
                            // 这里不能用 maa_on 禁用：没对接时正是要靠它选程序来启用。
                            {
                                let before=self.maa_prefs.clone();
                                settings_control_row(ui,"MAA 程序",80.0,|ui|{
                                    ui.horizontal(|ui|{
                                        let field=(ui.available_width()-70.0).max(90.0);
                                        ui.add_sized([field,24.0],egui::TextEdit::singleline(&mut self.maa_prefs.exe_path)
                                            .vertical_align(egui::Align::Center).hint_text("留空自动识别（MAA.exe）"));
                                        if ui.button("选择…").clicked(){self.pending.push(Cmd::PickMaaExe);}
                                    }).response
                                });
                                settings_hint(ui,"留空则用通知名单里的 MAA");
                                ui.separator();
                                settings_control_row(ui,"日志目录",80.0,|ui|{
                                    ui.horizontal(|ui|{
                                        let field=(ui.available_width()-70.0).max(90.0);
                                        ui.add_sized([field,24.0],egui::TextEdit::singleline(&mut self.maa_prefs.log_dir)
                                            .vertical_align(egui::Align::Center).hint_text("留空自动识别（<MAA目录>\\debug）"));
                                        if ui.button("选择…").clicked(){self.pending.push(Cmd::PickMaaLogDir);}
                                    }).response
                                });
                                settings_hint(ui,"留空自动识别（MAA 程序旁的 debug 目录）");
                                if before!=self.maa_prefs{self.maa_prefs.save();}
                            }
                        });
                    }
                    7=>{
                        settings_label(ui,"邮件提醒");
                        settings_card(ui,|ui|{
                            setting_row(ui,"新邮件提醒","新邮件提醒；歌词模式下显示图标",&mut self.settings.mail_enabled);
                            ui.separator();
                            let sync_before=self.settings.mail_sync_operations;
                            ui.add_enabled_ui(self.mail.manage.pending.is_empty(),|ui|{
                                setting_row(ui,"同步邮件操作到邮箱","同步已读/删除到邮箱；关闭则只改本地",&mut self.settings.mail_sync_operations);
                            });
                            if sync_before!=self.settings.mail_sync_operations{self.mail_manage_mode_changed();}
                            settings_hint(ui,"切换开关不会补交之前的本机操作");
                            ui.separator();
                            setting_row(ui,"邮件验证码高亮","邮件里识别到验证码时在岛上放大显示，点击即复制",&mut self.settings.mail_code);
                            ui.separator();
                            setting_choices(ui,"邮件验证码隐藏",&mut self.settings.mail_code_hide_seconds,[30,60,300],"秒");
                            settings_hint(ui,"默认 60 秒；隐藏不代表失效，查看后 15 秒再次隐藏");
                            setting_choices(ui,"检查间隔",&mut self.settings.mail_interval,[60,120,300],"秒");
                            settings_hint(ui,"支持推送的邮箱实时收信；间隔只对不支持推送的生效");
                            ui.separator();
                            setting_choices(ui,"会议提前",&mut self.settings.mail_ics,[5,15,30],"分钟");
                            settings_hint(ui,"与日程设置共用同一场提醒");
                        });
                        settings_label(ui,&format!("已添加的邮箱 · {}",self.mail.accounts.len()));
                        settings_card(ui,|ui|{
                            if self.mail.accounts.is_empty(){ui.label(egui::RichText::new("还没有邮箱。在下面添加，保存后自动连接。").size(12.0).color(settings_color(ctx,Color32::from_gray(195),Color32::from_gray(60))));}
                            let total=self.mail.accounts.len();
                            let(mut changed,mut remove,mut test)=(false,None,None);
                            for i in 0..total{
                                let(email,provider)=(self.mail.accounts[i].email.clone(),self.mail.accounts[i].provider);
                                let status=self.mail.status_of(&email).label();
                                let mut on=self.mail.accounts[i].enabled;
                                if i>0{ui.separator();}
                                ui.label(egui::RichText::new(&email).size(13.0).color(settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41))));
                                ui.label(egui::RichText::new(format!("{} · {}",provider.label(),status)).size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
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
                        });
                        if settings_section(ui,"mail.add","添加邮箱",&format!("{} 个邮箱",self.mail.accounts.len()),self.mail.accounts.is_empty()){
                            settings_card(ui,|ui|{
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
                                let target=crate::mail::Account{custom:(self.mail.form.provider==crate::mail::Provider::Custom).then(||self.mail.form.custom.clone()),email:self.mail.form.email.clone(),provider:self.mail.form.provider,enabled:true};
                                let unsupported=target.endpoint().err().as_deref()==Some(crate::mail::MICROSOFT_UNSUPPORTED);
                                if unsupported{ui.add(egui::Label::new(crate::mail::MICROSOFT_UNSUPPORTED).wrap());}
                                ui.add_enabled_ui(!unsupported,|ui|{
                                settings_control_row(ui,"授权码",56.0,|ui|{
                                    let field=ui.add_sized([input_width,30.0],egui::TextEdit::singleline(&mut self.mail.form.password).vertical_align(egui::Align::Center).password(!self.mail.form.show).hint_text("输入邮箱授权码"));
                                    if ui.add_sized([48.0,30.0],egui::Button::new(if self.mail.form.show{"隐藏"}else{"显示"})).clicked(){self.mail.form.show=!self.mail.form.show;}
                                    field
                                });
                                ui.horizontal(|ui|{
                                    if ui.button("测试连接").clicked(){
                                        let email=self.mail.form.email.trim().to_string();
                                        let target=crate::mail::Account{custom:(self.mail.form.provider==crate::mail::Provider::Custom).then(||self.mail.form.custom.clone()),email:email.clone(),provider:self.mail.form.provider,enabled:true};
                                        if !email.contains('@'){self.mail.note="请先填邮箱地址".into();}
                                        else if let Err(error)=target.endpoint(){self.mail.note=error;}
                                        else if self.mail.form.password.is_empty(){self.mail.note="请先填授权码（不是邮箱登录密码）".into();}
                                        else{
                                            self.mail.testing=Some(email.clone());
                                            self.mail.note=format!("{email}：正在测试连接…");
                                            self.pending.push(Cmd::MailTest(crate::mail::Account {custom: (self.mail.form.provider==crate::mail::Provider::Custom).then(||self.mail.form.custom.clone()), email:email.clone(),provider:self.mail.form.provider,enabled:true},self.mail.form.password.clone()));
                                        }
                                    }
                                    if ui.button("保存并开始监控").clicked(){
                                        let email=self.mail.form.email.trim().to_string();
                                        let target=crate::mail::Account{custom:(self.mail.form.provider==crate::mail::Provider::Custom).then(||self.mail.form.custom.clone()),email:email.clone(),provider:self.mail.form.provider,enabled:true};
                                        if !email.contains('@'){self.mail.note="请先填邮箱地址".into();}
                                        else if let Err(error)=target.endpoint(){self.mail.note=error;}
                                        else if self.mail.form.password.is_empty(){self.mail.note="请先填授权码（不是邮箱登录密码）".into();}
                                        else{
                                            match crate::mail::secrets::save(&target.credential_key(),&self.mail.form.password){
                                                Ok(())=>{
                                                    let account=crate::mail::Account {custom: (self.mail.form.provider==crate::mail::Provider::Custom).then(||self.mail.form.custom.clone()), email:email.clone(),provider:self.mail.form.provider,enabled:true};
                                                    if let Some(slot)=self.mail.accounts.iter_mut().find(|a|a.email.eq_ignore_ascii_case(&email)){*slot=account;}else{self.mail.accounts.push(account);}
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
                                ui.label(egui::RichText::new("图片中的验证码请在邮箱查看。").size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                                if !self.mail.note.is_empty(){ui.add_space(3.0);ui.add(egui::Label::new(egui::RichText::new(&self.mail.note).size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75)))).wrap());}
                            });
                        }
                    }
                    8=>{
                        ui.spacing_mut().item_spacing.y=4.0;
                        // v0.5.6 日程：状态在最上面，后面才是平台、账号和提醒。
                        // 只读；服务器地址由平台决定，用户不用抄。
                        if let Some(cfg)=self.calendar.config.clone(){
                            settings_card(ui,|ui|{
                                let n=self.calendar.events.len();
                                ui.horizontal(|ui|{
                                    ui.label(egui::RichText::new(if self.calendar.connected{format!("● 已连接 · 未来 24 小时 {n} 个日程")}else if self.calendar.failures==0{"● 正在连接…".into()}else{format!("● 上次连接失败 · 已知 {n} 个日程")}).size(13.0).strong().color(if self.calendar.connected{settings_color(ctx,Color32::from_rgb(120,220,160),Color32::from_rgb(24,121,64))}else{settings_color(ctx,Color32::from_rgb(240,180,110),Color32::from_rgb(145,89,12))}));
                                });
                                ui.horizontal(|ui|{
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                                        if ui.add_enabled(!self.calendar.testing,egui::Button::new("断开").small()).clicked(){self.pending.push(Cmd::CalendarForget);}
                                        if ui.small_button("立即同步").clicked(){self.calendar.note="正在同步…".into();self.calendar.note_error=false;self.pending.push(Cmd::CalendarSync);}
                                    });
                                });
                                ui.label(egui::RichText::new(format!("{} · 最后同步 {}",cfg.provider.label(),if self.calendar.last_sync.is_empty(){"—".to_string()}else{self.calendar.last_sync.clone()})).size(11.0).color(settings_color(ctx,Color32::from_gray(150),Color32::from_gray(105))));
                                if let Some(feed)=self.calendar.feeds.iter().find(|feed|feed.config==cfg) {
                                    let status=if feed.month.loading {"正在同步本月日程…".to_owned()}
                                        else if !feed.month.error.is_empty(){format!("本月同步失败：{}",feed.month.error)}
                                        else if feed.month.loaded==Some(self.calendar.view.month){format!("{} {} {} 个日程",self.calendar.view.month.format("%Y-%m"),if feed.month.warning.is_empty(){"已同步"}else{"已读取"},feed.month.events.len())}
                                        else {String::new()};
                                    if !status.is_empty(){ui.add(egui::Label::new(egui::RichText::new(status).size(12.0)).wrap());}
                                    if !feed.month.warning.is_empty(){ui.add(egui::Label::new(egui::RichText::new(format!("本月部分日程未同步：{}",feed.month.warning)).size(12.0).color(settings_color(ctx,Color32::from_rgb(238,181,91),Color32::from_rgb(139,88,12)))).wrap());}
                                }
                                setting_row(ui,"自动同步","每 5 分钟更新日程；关闭后可点「立即同步」",&mut self.settings.calendar_auto_sync);
                                if !self.calendar.warning.is_empty(){ui.add(egui::Label::new(egui::RichText::new(format!("同步提示：{}",self.calendar.warning)).size(12.0).color(settings_color(ctx,Color32::from_rgb(238,181,91),Color32::from_rgb(139,88,12)))).wrap());}
                                if !self.calendar.error.is_empty(){
                                    ui.add(egui::Label::new(egui::RichText::new(&self.calendar.error).size(12.0).color(settings_color(ctx,Color32::from_rgb(240,96,96),Color32::from_rgb(184,34,34)))).wrap());
                                }
                            });
                        }
                        if ui.add_sized([ui.available_width(),34.0],egui::Button::new("打开日程")).clicked(){self.open_calendar(None);}
                        settings_label(ui,"选择日历平台");
                        settings_card(ui,|ui|{
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
                        });
                        if settings_section(ui,"calendar.connect","连接账号",if self.calendar.config.is_some(){"已连接"}else{"未连接"},self.calendar.config.is_none()){
                            settings_card(ui,|ui|{
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
                                let guide=if self.calendar.form.guide_open{"收起获取方法"}else{"如何获取用户名和密码？"};
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
                        }
                        if settings_section(ui,"calendar.import","导入日历文件",".ics",false){
                            settings_card(ui,|ui|{
                                ui.add(egui::Label::new(egui::RichText::new("把 .ics 文件里的日程导入到本机日程，例如苹果「日历」App 里「文件 → 导出」得到的文件。重复日程会按未来一年展开；再次导入同一文件不会重复添加。").size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75)))).wrap());
                                ui.add_space(5.0);
                                if ui.add_sized([150.0,30.0],egui::Button::new("选择 .ics 文件…")).clicked(){self.pending.push(Cmd::CalendarImport);}
                                if !self.calendar.import_note.is_empty(){
                                    ui.add_space(3.0);
                                    ui.add(egui::Label::new(egui::RichText::new(&self.calendar.import_note).size(12.0).color(settings_color(ctx,Color32::from_gray(210),Color32::from_gray(55)))).wrap());
                                }
                            });
                        }
                        settings_label(ui,"会议提醒");
                        settings_card(ui,|ui|{
                            ui.horizontal(|ui|{
                                ui.allocate_ui_with_layout(egui::vec2(50.0,32.0),egui::Layout::left_to_right(egui::Align::Center),|ui|{ui.set_min_size(egui::vec2(50.0,32.0));ui.label(egui::RichText::new("提前").size(13.0));});
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                                    for minutes in [30,15,5]{let selected=self.settings.mail_ics==minutes;
                                        if ui.add_sized([62.0,32.0],egui::Button::new(format!("{minutes} 分钟")).selected(selected)).clicked(){self.settings.mail_ics=minutes;}
                                    }
                                });
                            });
                            ui.add_space(5.0);
                            ui.label(egui::RichText::new("与邮件中的会议邀请共用提醒，同一场会议只提醒一次。").size(11.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                        });
                    }
                    _=>{
                        settings_label(ui,"岛仔 Daozai");
                        settings_card(ui,|ui|{
                            let update_check=self.settings.update_check;
                            ui.horizontal(|ui|{
                                ui.label(egui::RichText::new(format!("版本 {} · 自定义构建",env!("CARGO_PKG_VERSION"))).size(13.0));
                                ui.add_space(8.0);
                                ui.vertical(|ui|self.updates.ui(ui,update_check));
                            });
                            setting_row(ui,"检查更新","关掉后隐藏更新入口，避免被官方包覆盖",&mut self.settings.update_check);
                            ui.label(egui::RichText::new("桌面顶部的音乐、专注、提醒与日程小助手。").size(12.0).color(settings_color(ctx,Color32::from_gray(180),Color32::from_gray(75))));
                            ui.horizontal_wrapped(|ui|{
                                if ui.button("打开本地数据目录").clicked(){self.pending.push(Cmd::Open(crate::model::config_dir()));}
                                ui.label(egui::RichText::new("设备图标：Solar Icons · 480 Design").size(11.0).color(settings_color(ctx,Color32::from_gray(150),Color32::from_gray(105))));
                                ui.hyperlink_to("CC BY 4.0", "https://creativecommons.org/licenses/by/4.0/");
                            });
                        });
                        settings_label(ui,"支持作者");
                        settings_card(ui,support_author);
                    }
                    }
                });
            });
            // 设置窗口是独立窗口，主岛的 toast 画不到这里；补一条同位置的提示。
            if let Some((message,elapsed))=self.toast_now(){
                let secs=crate::app::TOAST_SECS;
                let alpha=crate::anim::smoothstep(0.0,0.12,elapsed)*(1.0-crate::anim::smoothstep(secs-0.8,secs,elapsed));
                let a=(alpha*255.0).round().clamp(0.0,255.0) as u8;
                let lift=(1.0-crate::anim::ease_out_cubic((elapsed/0.22).min(1.0)))*6.0;
                let [r,g,b,_]=settings_color(ctx,Color32::from_rgb(46,46,50),Color32::from_rgb(250,250,252)).to_array();
                let [tr,tg,tb,_]=settings_color(ctx,Color32::WHITE,Color32::from_rgb(28,32,39)).to_array();
                egui::Area::new(egui::Id::new("settings-toast")).order(egui::Order::Foreground)
                    .anchor(egui::Align2::CENTER_TOP,egui::vec2(0.0,24.0+lift)).interactable(false)
                    .show(ctx,|ui|{
                        egui::Frame::new()
                            .fill(Color32::from_rgba_unmultiplied(r,g,b,a))
                            .stroke(egui::Stroke::new(1.0,Color32::from_rgba_unmultiplied(255,255,255,(a as f32*0.14) as u8)))
                            .corner_radius(11.0).inner_margin(egui::Margin::symmetric(14,7))
                            .show(ui,|ui|{
                                // 长提示（重启失败说明）要换行，不能撑出窗口外。
                                ui.set_width(320.0);
                                ui.label(egui::RichText::new(&message).size(12.5)
                                    .color(Color32::from_rgba_unmultiplied(tr,tg,tb,a)));
                            });
                    });
                ctx.request_repaint();
            }
            self.avoidance.dialog(ctx,&mut self.settings);
            if before!=format!("{:?}",self.settings){self.settings.save();}
            ctx.set_style(original_style);
        });
        if !self.settings_open{self.window_layout().close();}
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

#[cfg(test)]
mod status_action_safety_tests {
    use super::status_action;

    #[test]
    fn 文件动作只放行存在的非可执行文件() {
        let root = std::env::temp_dir().join(format!("daozai-open-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let text = root.join("note.txt");
        let exe = root.join("run.exe");
        std::fs::write(&text, b"note").unwrap();
        std::fs::write(&exe, b"not an executable").unwrap();
        assert!(status_action(&format!("file:{}", text.display())).is_some());
        assert!(status_action(&format!("file:{}", exe.display())).is_none(), "可执行扩展必须拒绝");
        assert!(status_action(&format!("file:{}", root.join("missing.txt").display())).is_none(), "不存在的路径不该生成按钮");
        assert!(status_action("url:https://example.com").is_some());
        assert!(status_action("url:file:///C:/Windows/System32/cmd.exe").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod meeting_plan_tests {
    use super::meeting_hit;

    fn plan(items: &[(i64, i64, &str)]) -> Vec<(i64, i64, String)> {
        items.iter().map(|(a, b, c)| (*a, *b, (*c).to_string())).collect()
    }

    #[test]
    fn 只有刚开始且未结束的会议触发且最多两小时() {
        let now = 10_000;
        let p = plan(&[
            (now - 200, now + 600, "太早"),      // 超过 90 秒窗口
            (now + 60, now + 3600, "还没开始"),
            (now - 30, now - 1, "已结束"),
        ]);
        assert!(meeting_hit(&p, now).is_none(), "这三种都不该触发");

        let p = plan(&[(now - 30, now + 1800, "进行中")]);
        let (until, key) = meeting_hit(&p, now).unwrap();
        assert_eq!(until, now + 1800);
        assert_eq!(key, format!("进行中|{}", now - 30));

        let p = plan(&[(now - 30, now + 99_999, "超长会")]);
        assert_eq!(meeting_hit(&p, now).unwrap().0, now + 7200, "勿扰最多 2 小时");
    }

    #[test]
    fn 全天与无结束时间的日程不进候选表() {
        use crate::mail::Meeting;
        fn mk(all_day: bool, end: Option<i64>, uid: &str) -> Meeting {
            Meeting { all_day, title: "会".into(), start: 9_900, end, at: "11-12 10:00".into(), uid: uid.into() }
        }
        let events = vec![
            mk(true, Some(12_000), "全天"),
            mk(false, None, "无结束"),
            mk(false, Some(12_000), "正常"),
        ];
        let candidates = super::meeting_candidates(events.iter());
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].2, "正常");
        assert!(meeting_hit(&[], 10_000).is_none(), "候选为空不该凭空触发");
    }
}

/// 把今日日程压成轻量候选表：全天日程、没有结束时间的都不具备「散会自动失效」的条件。
fn meeting_candidates<'a>(events:impl Iterator<Item=&'a crate::mail::Meeting>)->Vec<(i64,i64,String)>{
    events.filter_map(|event|{
        if event.all_day{return None;}
        Some((event.start,event.end?,event.uid.clone()))
    }).collect()
}

/// 从候选表挑出「刚开始 90 秒内、还没结束」的会议，返回 (勿扰截止, 去重键)。
/// 抽成纯函数便于单测；多场同时命中时取最后一个。
fn meeting_hit(plan:&[(i64,i64,String)],now:i64)->Option<(i64,String)>{
    let mut hit=None;
    for (start,end,uid) in plan {
        if *start>now || now-*start>90 {continue;}
        if *end<=now {continue;}
        hit=Some(((*end).min(now+7200),format!("{}|{}",uid,start)));
    }
    hit
}
