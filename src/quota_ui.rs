//! 同心额度环、账号选择及账号设置。
use crate::{app::{App,Cmd},extras::{setting_row,settings_card,settings_control_row},paint::{Card,H},quota::{self,Accent,Account,Amount,Config,Provider,Snapshot},theme::{self,W}};
use eframe::egui::{self,Color32};
use std::collections::{HashMap,HashSet};
use chrono::Datelike;

const CLAUDE_LOCAL_NOTICE:&str="由于 Claude 对网络使用环境要求较高，岛仔不会使用登录凭证向 Claude 服务发送任何请求，只读取本机用量记录与缓存，自动显示其中的重置时间；缺少有效记录时，显示“重置时间未知”。";

#[derive(Default)]
pub(crate) struct State {
    cache_path:Option<std::path::PathBuf>,
    pub config:Config,pub rows:HashMap<String,Result<Snapshot,String>>,pub busy:HashSet<String>,
    pub provider:Provider,pub alias:String,pub source:String,pub note:String,
    pub relay_manual:bool,
    pub connect_open:bool,pub connect_target:Option<String>,
    pub login:Option<quota::grok::Login>,
    pub workbuddy_connecting:Option<quota::workbuddy::Connect>,
    pub deepseek_connecting:Option<quota::deepseek::Connect>,pub deepseek_key:String,
    pub api_connecting:Option<quota::api::Connect>,pub api_key:String,
    pub detail_account:Option<String>,
    pub kiro_connecting:Option<quota::kiro::Connect>,
    pub stale:HashMap<String,String>,
    pub gemini_connecting:Option<quota::gemini::Connect>,pub gemini_email:String,pub gemini_cookie:String,
    pub antigravity_connecting:Option<quota::antigravity::Connect>,pub antigravity_email:Option<String>,
    /// CLI 登录态：邮箱 + 令牌到期时间，只读系统凭据。`login_at` 控制多久重读一次（本地读，不发请求）。
    pub antigravity_login:Option<quota::antigravity::Login>,pub login_at:Option<std::time::Instant>,
    pub renew_rx:Option<std::sync::mpsc::Receiver<Result<i64,String>>>,pub renew_note:Option<String>,
    /// 内存里那枚令牌的到期时间，以及上一次自动续期的尝试时间（一分钟内不重复打 OAuth）。
    pub renewed_until:Option<i64>,pub auto_renew_at:Option<std::time::Instant>,pub renew_auto:bool,
    /// 周额度要跑一次本机 CLI（6~10 秒），单独记它什么时候排过、这一轮跑完没有。
    pub weekly_pending:Option<quota::antigravity::WeeklyFetch>,pub weekly_at:Option<std::time::Instant>,
    /// 下一轮自动刷新什么时候排下、排多长间隔，供页面显示「Ns 后刷新」。
    pub scheduled_at:Option<std::time::Instant>,pub scheduled_every:std::time::Duration,
    pub doubao_connecting:Option<quota::doubao::Connect>,pub doubao_cookie:String,
}
impl State {
    fn balance_text(&self,id:&str)->Option<String>{let s=self.rows.get(id)?.as_ref().ok()?;s.wallet.as_ref().map(|w|w.primary().compact()).or_else(||s.credits.as_ref().map(|c|c.compact())).or_else(||s.relay.as_ref().map(|q|q.compact()))}
    fn balance_loading(&self,id:&str)->bool{self.balance_text(id).is_none()&&(self.busy.contains(id)||!self.rows.contains_key(id))}
    /// 环类账号跟随在图标右侧的数字：有具体点数时取主周期剩余点数（如 Kiro 的 904）。
    fn ring_tail_text(&self,id:&str)->Option<String>{self.rows.get(id)?.as_ref().ok()?.rings().first()?.amount.as_ref().map(Amount::short)}
    /// 常驻胶囊里紧跟图标的文字：余额类给余额，点数类给剩余点数。
    fn tail_text(&self,id:&str)->Option<String>{self.balance_text(id).or_else(||self.ring_tail_text(id))}
    pub fn load()->Self {
        if cfg!(test){return Self::default();}
        let mut state=Self::restore(Config::load(),crate::model::config_dir().join("ai-quota-cache.json"));
        if let Err(error)=quota::api::cleanup_pending(&state.config){state.note=error;}state
    }

    fn restore(config:Config,path:std::path::PathBuf)->Self {
        let mut state=Self{config,cache_path:Some(path.clone()),..Default::default()};
        let cache:serde_json::Value=std::fs::read(path).ok()
            .and_then(|raw|serde_json::from_slice(&raw).ok()).unwrap_or_default();
        for a in &state.config.accounts {
            let Some(snapshot)=a.cached_snapshot(&cache)else{continue;};
            state.rows.insert(a.id.clone(),Ok(snapshot));
            state.stale.insert(a.id.clone(),"上次记录，等待刷新".into());
        }
        state
    }

    fn save_snapshots(&self){
        let Some(path)=&self.cache_path else{return;};
        let mut entries=serde_json::Map::new();
        for a in &self.config.accounts {
            if let Some(Ok(snapshot))=self.rows.get(&a.id) {
                let mut entry=serde_json::json!({"provider":a.provider.name(),"slot":a.slot,"source":a.source,"snapshot":snapshot});
                if a.provider==Provider::Claude{entry["local_identity_checked"]=a.cache_identity_marker();}
                entries.insert(a.id.clone(),entry);
            }
        }
        if let Ok(raw)=serde_json::to_vec(&entries){let _=crate::model::atomic_write(path,&raw);}
    }

    fn connect_kiro(&mut self,target:Option<&Account>,connected:quota::kiro::Connected)->Result<(),String>{
        let mut next=self.config.clone();let id=quota::kiro::apply_connection(&mut next,target,&connected)?;
        next.save_checked()?;self.config=next;
        self.stale.remove(&id);self.busy.remove(&id);self.rows.insert(id,Ok(connected.snapshot));self.save_snapshots();self.connect_target=None;Ok(())
    }
    pub fn receive(&mut self,mut event:quota::Event){
        self.busy.remove(&event.id);
        if !self.config.accounts.iter().any(|a|a.id==event.id && a.enabled){return;}
        if let Some(account)=self.config.accounts.iter().find(|a|a.id==event.id&&a.provider==Provider::Kiro).cloned(){
            if let Ok(snapshot)=&event.result{
                let verified=(||{
                    quota::kiro::check_owner(&account,snapshot)?;
                    if account.slot.is_none(){
                        let owner=snapshot.kiro.as_ref().unwrap().owner.clone();
                        if self.config.accounts.iter().any(|a|a.id!=account.id&&a.provider==Provider::Kiro&&a.slot.as_deref()==Some(&owner)){return Err("此 Kiro 账号已经连接，请移除重复的登录文件".into());}
                        let mut next=self.config.clone();next.accounts.iter_mut().find(|a|a.id==account.id).unwrap().slot=Some(owner);
                        next.save_checked()?;self.config=next;
                    }
                    Ok::<_,String>(())
                })();
                if let Err(error)=verified{event.result=Err(error);}
            }
        }
        if let Err(error)=&event.result{
            if matches!(self.rows.get(&event.id),Some(Ok(_))){
                self.stale.insert(event.id,error.clone());return;
            }
        }
        let success=event.result.is_ok();
        let old_local_record=self.config.accounts.iter().any(|a|a.id==event.id&&a.provider==Provider::Claude)&&event.result.as_ref().is_ok_and(|s|chrono::Utc::now().timestamp()-s.at>1800);
        self.stale.remove(&event.id);
        if old_local_record{self.stale.insert(event.id.clone(),"本地记录较旧，等待 Claude 更新".into());}
        // 周额度是另跑 CLI 拿的，接口这一轮刷新没有它：把旧值接回来，别让那两扇窗忽隐忽现。
        if let Ok(next)=&mut event.result{if let Some(Ok(old))=self.rows.get(&event.id){preserve_weekly(old,next);}}
        self.rows.insert(event.id,event.result);
        if success{self.save_snapshots();}
    }
    /// 把 CLI 读回来的周额度并进快照：只替换那两条周窗，接口给的窗口保持原样。
    fn apply_weekly(&mut self,weekly:quota::antigravity::Weekly){
        let Some(id)=self.config.accounts.iter().find(|a|a.provider==Provider::Gemini&&a.source.to_string_lossy()==quota::antigravity::SOURCE).map(|a|a.id.clone())else{return;};
        let Some(Ok(snapshot))=self.rows.get(&id)else{return;};
        let mut next=snapshot.clone();
        next.windows.retain(|w|w.label!=quota::antigravity::GEMINI_WEEKLY_LABEL&&w.label!=quota::antigravity::ARGON_WEEKLY_LABEL);
        next.windows.extend(weekly.gemini);
        next.windows.extend(weekly.argon);
        quota::antigravity::sort_windows(&mut next.windows);
        self.rows.insert(id,Ok(next));self.save_snapshots();
    }
    /// 轮询后台的 CLI 读数：拿到就并进快照；卡住超过 90 秒就放弃这一轮，下一轮再说。
    pub fn poll_weekly(&mut self){
        let Some(fetch)=self.weekly_pending.as_ref()else{return;};
        let polled=fetch.poll();let expired=fetch.timed_out();
        if let Some(weekly)=polled{self.weekly_pending=None;self.apply_weekly(weekly);}
        else if expired{self.weekly_pending=None;}
    }
    /// 周额度五分钟补一次就够：CLI 一次要 6~10 秒，没必要跟着接口一起转。
    pub fn should_start_weekly(&self)->bool{
        if cfg!(test){return false;}
        self.weekly_pending.is_none()
            && self.weekly_at.is_none_or(|at|at.elapsed()>=std::time::Duration::from_secs(300))
            && self.config.enabled
            && self.config.accounts.iter().any(|a|a.enabled&&a.provider==Provider::Gemini&&a.source.to_string_lossy()==quota::antigravity::SOURCE)
    }
    pub fn maybe_start_weekly(&mut self){
        if !self.should_start_weekly(){return;}
        self.weekly_at=Some(std::time::Instant::now());
        self.weekly_pending=Some(quota::antigravity::WeeklyFetch::start());
    }
    /// 重读本机 CLI 登录态：只读系统凭据，不发请求，也不改动任何文件。
    pub fn refresh_login(&mut self){
        self.antigravity_login=quota::antigravity::login_state();
        keep_freshest(&mut self.antigravity_login,self.renewed_until);
        self.login_at=Some(std::time::Instant::now());
    }
    /// 登录态半分钟刷新一次就够；点「续期」的结果也从这里取回来。
    pub fn poll_login(&mut self){
        if self.login_at.is_none_or(|at|at.elapsed()>=std::time::Duration::from_secs(30)){self.refresh_login();}
        self.auto_renew();
        let Some(rx)=self.renew_rx.as_ref()else{return;};
        let Ok(result)=rx.try_recv()else{return;};
        self.renew_rx=None;
        match result {
            Ok(expires_at)=>{
                let when=chrono::DateTime::from_timestamp(expires_at,0).map(|at|at.with_timezone(&chrono::Local).format("%H:%M").to_string()).unwrap_or_default();
                self.renewed_until=Some(expires_at);
                self.renew_note=Some(if self.renew_auto{format!("令牌快到期，已自动续期到 {when}")}else{format!("已续期，令牌到 {when} 失效")});
                self.renew_auto=false;
                self.refresh_login();
            }
            Err(error)=>{self.renew_auto=false;self.renew_note=Some(error);}
        }
    }
    /// 令牌快过期就自己换一枚：只换内存里的，不动系统凭据，也不碰 Antigravity 的文件。
    pub fn auto_renew(&mut self){
        if cfg!(test){return;}
        let now=chrono::Local::now().timestamp();
        if self.auto_renew_if_due(now){self.renew();}
    }
    /// 判据与节流都在这里：真的该续才记一次尝试时间并返回 true（发不发请求由调用方决定）。
    fn auto_renew_if_due(&mut self,now:i64)->bool{
        if !renew_due(self.antigravity_login.as_ref(),self.auto_renew_at,now){return false;}
        self.auto_renew_at=Some(std::time::Instant::now());
        self.renew_auto=true;
        true
    }
    /// 用户点「续期」：后台线程换一枚新令牌（只进内存缓存，不写回系统凭据）。
    pub fn renew(&mut self){
        if self.renew_rx.is_some(){return;}
        let (sender,receiver)=std::sync::mpsc::channel();
        std::thread::spawn(move||{
            let agent=quota::http_agent();
            let _=sender.send(quota::antigravity::renew(&agent));
        });
        self.renew_rx=Some(receiver);self.renew_note=None;
    }
    /// 下一轮刷新还剩几秒；没排过就返回 None（这时不显示倒计时，避免画一个假数字）。
    fn countdown(&self)->Option<String>{
        let at=self.scheduled_at?;
        let every=self.scheduled_every.as_secs_f32();
        if !(every>0.0){return None;}
        countdown_label((every-at.elapsed().as_secs_f32()).max(0.0))
    }
}
/// 自动续期的判据：只知道「快到期了」才动手，一分钟内不重复试；
/// 到期时间读不出来（没有 expiry 字段）就不猜，宁可等用户点「续期」。
fn renew_due(login:Option<&quota::antigravity::Login>,last:Option<std::time::Instant>,now:i64)->bool{
    let Some(expires)=login.and_then(|login|login.expires_at)else{return false;};
    if last.is_some_and(|at|at.elapsed()<std::time::Duration::from_secs(60)){return false;}
    expires-now<=600
}
/// 内存里续过期就用内存里的到期时间，别让系统凭据里的旧时间把界面又拉回「已过期」。
fn keep_freshest(login:&mut Option<quota::antigravity::Login>,renewed:Option<i64>){
    let Some(until)=renewed else{return;};
    if let Some(login)=login{
        if until>login.expires_at.unwrap_or(0){login.expires_at=Some(until);}
    }
}
/// 倒计时文案：向上取整（还剩 0.2 秒也算 1 秒），归零写 0s，绝不出现负数。
fn countdown_label(left:f32)->Option<String>{
    if !left.is_finite(){return None;}
    Some(format!("{}s 后刷新",left.ceil().max(0.0) as u64))
}
/// 接口刷新会把周窗冲掉（它只回 5 小时与 Claude/GPT 每周两条），这里把 CLI 读到的两条接回来。
fn preserve_weekly(old:&Snapshot,next:&mut Snapshot){
    for window in old.windows.iter().filter(|w|w.label==quota::antigravity::GEMINI_WEEKLY_LABEL||w.label==quota::antigravity::ARGON_WEEKLY_LABEL){
        if !next.windows.iter().any(|w|w.label==window.label){next.windows.push(window.clone());}
    }
    quota::antigravity::sort_windows(&mut next.windows);
}
fn accent_color(accent:Accent,provider:Provider)->Color32{
    if accent==Accent::Brand&&provider==Provider::DeepSeek{return Color32::from_rgb(77,107,254);}
    let accent=if accent==Accent::Brand{match provider{Provider::Codex|Provider::OpenCodeGo|Provider::Kiro=>Accent::Violet,Provider::Claude=>Accent::Orange,Provider::Kimi=>Accent::Blue,Provider::GrokBot=>Accent::Green,Provider::Gemini=>Accent::Cyan,Provider::ZCode|Provider::Glm|Provider::Doubao=>Accent::Blue,Provider::WorkBuddy=>Accent::Green,Provider::DeepSeek=>Accent::Blue,Provider::MiniMax=>Accent::Pink,_=>Accent::Cyan}}else{accent};
    let (r,g,b)=match accent{Accent::Brand|Accent::Violet=>(142,67,255),Accent::Orange=>(255,116,26),Accent::Blue=>(40,126,255),Accent::Cyan=>(0,213,242),Accent::Pink=>(255,43,132),Accent::Gold=>(255,190,0),Accent::Green=>(52,221,145)};
    Color32::from_rgb(r,g,b)
}
fn accent_colors(config:&Config,a:&Account)->[Color32;2]{
    let main=accent_color(config.colors.get(&a.id).copied().unwrap_or_default(),a.provider);
    [main,crate::paint::mix(main,Color32::WHITE,0.16)]
}
fn ring_colors(config:&Config,a:&Account)->[Color32;3]{
    let main=accent_color(config.colors.get(&a.id).copied().unwrap_or_default(),a.provider);
    let middle=crate::paint::mix(main,Color32::WHITE,0.88);
    let inner=if main.b()>=main.r(){Color32::from_rgb(255,190,0)}else{Color32::from_rgb(0,213,242)};
    [main,middle,inner]
}
fn color_picker(ui:&mut egui::Ui,provider:Provider,value:&mut Accent){
    ui.allocate_ui_with_layout(egui::vec2(ui.available_width(),30.0),egui::Layout::left_to_right(egui::Align::Center),|ui|{
        ui.label(egui::RichText::new("颜色").size(12.0).color(theme::GRAY));
        if ui.selectable_label(*value==Accent::Brand,egui::RichText::new("默认").size(12.0)).on_hover_text("跟随厂商").clicked(){*value=Accent::Brand;}
        for accent in Accent::ALL.into_iter().skip(1){
            let (rect,response)=ui.allocate_exact_size(egui::vec2(22.0,25.0),egui::Sense::click());
            let on=*value==accent;
            if response.on_hover_text(accent.label()).clicked(){*value=accent;}
            if on{ui.painter().circle_stroke(rect.center(),10.0,egui::Stroke::new(1.5,Color32::WHITE));}
            ui.painter().circle_filled(rect.center(),7.0,accent_color(accent,provider));
        }
        ui.label(egui::RichText::new(value.label()).size(12.0).color(theme::GRAY));
    });
}
pub(crate) fn bar_tooltip(response:egui::Response,text:String)->egui::Response{
    let ctx=&response.ctx;let id=response.id.with("bar-tooltip");
    if response.hovered()&&!ctx.input(|i|i.pointer.any_down()){
        let now=ctx.input(|i|i.time);
        let since=ctx.data_mut(|data|*data.get_temp_mut_or_insert_with(id,||now));
        if now-since>=0.5{
            egui::Area::new(id).order(egui::Order::Tooltip).fixed_pos(response.rect.left_bottom()+egui::vec2(0.0,4.0)).interactable(false).show(ctx,|ui|{
                egui::Frame::popup(ui.style()).show(ui,|ui|{ui.label(&text);});
            });
        }else{ctx.request_repaint_after_secs((0.5-(now-since))as f32);}
    }else{ctx.data_mut(|data|data.remove::<f64>(id));}
    response
}
/// 这些厂商的图标自带品牌配色（官方方形标），直接使用原图；其余统一压成白色标识。
fn keeps_brand_color(provider:Provider)->bool{matches!(provider,Provider::WorkBuddy|Provider::ZCode|Provider::MiniMax|Provider::Kiro|Provider::Doubao)}
fn logo_texture(ctx:&egui::Context,provider:Provider,pixels:u32)->Option<egui::TextureHandle>{
    let key=egui::Id::new(("quota-logo",provider.name(),pixels));
    if let Some(texture)=ctx.data_mut(|data|data.get_temp::<egui::TextureHandle>(key)){return Some(texture);}
    // WorkBuddy 图标来自官方 WorkBuddyAI 客户端的 workbuddy-app-icon SVG，保留原始配色。
    let mut image=if provider==Provider::Doubao{
        let bitmap=image::load_from_memory(include_bytes!("../assets/ai-icons/doubao.png")).ok()?.resize_exact(pixels,pixels,image::imageops::FilterType::Lanczos3).to_rgba8();
        egui::ColorImage::from_rgba_unmultiplied([pixels as usize,pixels as usize],bitmap.as_raw())
    }else{
        let bytes:&[u8]=match provider{Provider::ZCode=>include_bytes!("../assets/ai-icons/zcode.svg"),Provider::Codex=>include_bytes!("../assets/ai-icons/codex.svg"),Provider::Claude=>include_bytes!("../assets/ai-icons/claude.svg"),Provider::Kimi=>include_bytes!("../assets/ai-icons/kimi.svg"),Provider::GrokBot=>include_bytes!("../assets/ai-icons/grokbot.svg"),Provider::Gemini=>include_bytes!("../assets/ai-icons/gemini.svg"),Provider::WorkBuddy=>include_bytes!("../assets/ai-icons/workbuddy.svg"),Provider::DeepSeek=>include_bytes!("../assets/ai-icons/deepseek.svg"),Provider::Kiro=>include_bytes!("../assets/ai-icons/kiro.svg"),Provider::OpenCodeGo=>include_bytes!("../assets/ai-icons/opencode.svg"),Provider::MiniMax=>include_bytes!("../assets/ai-icons/minimax.svg"),Provider::Glm=>include_bytes!("../assets/ai-icons/glm.svg"),_=>include_bytes!("../assets/ai-icons/api-key.svg")};
        egui_extras::image::load_svg_bytes_with_size(bytes,Some(egui::load::SizeHint::Size(pixels,pixels))).ok()?
    };
    // 白色标识使用矢量覆盖率，避免透明边缘再次变暗。
    for pixel in &mut image.pixels{if keeps_brand_color(provider){continue;}*pixel=if provider==Provider::DeepSeek{Color32::from_rgba_unmultiplied(77,107,254,pixel.a())}else{Color32::from_white_alpha(pixel.a())};}
    let texture=ctx.load_texture(format!("quota-logo-{}-{pixels}",provider.name()),image,egui::TextureOptions::LINEAR);
    if (!provider.balance()||provider.relay())&&!keeps_brand_color(provider){crate::skin_paint::mark_monochrome(texture.id());}else{crate::skin_paint::mark_colored(texture.id());}
    ctx.data_mut(|data|data.insert_temp(key,texture.clone()));Some(texture)
}
fn logo(c:&Card,provider:Provider,x:f32,y:f32,size:f32){
    let ctx=c.ui.ctx();let ppp=ctx.pixels_per_point();let center=c.pos(x,y);
    let pixels=(size*c.s*ppp).round().max(1.0) as u32;let half=pixels as f32/2.0;
    let min=egui::pos2((center.x*ppp-half).round()/ppp,(center.y*ppp-half).round()/ppp);
    let rect=egui::Rect::from_min_size(min,egui::Vec2::splat(pixels as f32/ppp));
    if let Some(texture)=logo_texture(ctx,provider,pixels){c.painter.image(texture.id(),rect,egui::Rect::from_min_max(egui::pos2(0.0,0.0),egui::pos2(1.0,1.0)),Color32::WHITE);}
}
fn deepseek_badge_svg(skin:crate::model::Skin)->String{
    use crate::model::Skin;
    let (fill,rim)=match skin{Skin::Classic=>("#e9edff","#cfd7ff"),Skin::Cream=>("#e9edff","#dce2fc"),Skin::GlassLight=>("#f5f7ff","#dfe5fa"),Skin::GlassDark|Skin::Fluid=>("#dce4fa","#b8c9ec")};
    let whale=include_str!("../assets/ai-icons/deepseek.svg").replace("currentColor","#4d6bfe").replace("width=\"1em\"","x=\"12.5\" width=\"75\"").replace("height=\"1em\"","y=\"12.5\" height=\"75\"");
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 100\"><circle cx=\"50\" cy=\"50\" r=\"48\" fill=\"{fill}\" stroke=\"{rim}\" stroke-width=\"3\"/>{whale}</svg>")
}
fn deepseek_badge(c:&Card,skin:crate::model::Skin,x:f32,y:f32,size:f32){
    let ctx=c.ui.ctx();let ppp=ctx.pixels_per_point();let pixels=(size*c.s*ppp).round().max(1.0) as u32;
    let key=egui::Id::new(("deepseek-badge",skin.key(),pixels));
    let texture=ctx.data_mut(|data|data.get_temp::<egui::TextureHandle>(key)).or_else(||{
        let image=egui_extras::image::load_svg_bytes_with_size(deepseek_badge_svg(skin).as_bytes(),Some(egui::load::SizeHint::Size(pixels,pixels))).ok()?;
        let texture=ctx.load_texture(format!("deepseek-badge-{}-{pixels}",skin.key()),image,egui::TextureOptions::LINEAR);
        crate::skin_paint::mark_colored(texture.id());
        ctx.data_mut(|data|data.insert_temp(key,texture.clone()));Some(texture)
    });
    if let Some(texture)=texture{
        let center=c.pos(x,y);let half=pixels as f32/2.0;let min=egui::pos2((center.x*ppp-half).round()/ppp,(center.y*ppp-half).round()/ppp);
        c.painter.image(texture.id(),egui::Rect::from_min_size(min,egui::Vec2::splat(pixels as f32/ppp)),egui::Rect::from_min_max(egui::Pos2::ZERO,egui::pos2(1.0,1.0)),Color32::WHITE);
    }
}
fn ring(c:&Card,a:&Account,snapshot:Option<&Snapshot>,x:f32,y:f32,size:f32,colors:[Color32;3]){
    // 三圈统一画 5 小时 / Gemini 每周 / Argon 每周。小环（27 像素的常驻位）只有 13 像素半径，
    // 三条弧如果都用「暗弧 + 亮弧 + 圆头」，缩到 1.5 像素宽就糊成碎块：小环改成先铺一条暗轨、
    // 再把剩余量画成亮弧，不画端头，三圈才读得出是一圈套一圈。
    let max=3;
    let windows:Vec<&quota::Window>=snapshot.map(|s|{let mut all=s.ordered();all.truncate(max);all}).unwrap_or_default();
    let count=windows.len();let dual=count>1;let triple=count>2;
    let small=size<35.0;
    let stroke=if small{if triple{1.3}else if dual{2.2}else{(size*0.102).clamp(3.0,6.5)}}else if triple{4.5}else if dual{6.5}else{(size*0.102).clamp(3.0,6.5)};
    let radius=size/2.0-stroke/2.0-1.0;
    let gap=if small{if triple{1.3}else{1.5}}else if triple{2.5}else{3.0};
    let start=-std::f32::consts::FRAC_PI_2;
    for i in 0..count{
        let r=radius-i as f32*(stroke+gap);
        if let Some(w)=windows.get(i){
            let col=colors[i.min(2)];let dark=crate::paint::mix(col,Color32::BLACK,0.8);
            if small{
                // 小环：同色压暗的细轨 + 剩余亮弧，都从 12 点开始（读数方向一致），不画端头圆点。
                c.ring(x,y,r,stroke*0.55,crate::paint::mix(col,Color32::BLACK,0.55));
                if w.remaining>=100.0{c.ring(x,y,r,stroke,col);}else if w.remaining>0.0{
                    let end=start+std::f32::consts::TAU*w.remaining.clamp(0.0,100.0)/100.0;
                    c.arc(x,y,r,start,end,stroke,col);
                }
            }else if w.remaining>=100.0{c.ring(x,y,r,stroke,col);}else if w.remaining<=0.0{c.ring(x,y,r,stroke,dark);}else{
                let end=start+std::f32::consts::TAU*w.remaining/100.0;
                c.arc(x,y,r,end,start+std::f32::consts::TAU,stroke,dark);
                c.arc(x,y,r,start,end,stroke,col);
                for angle in [start,end]{c.circle(x+r*angle.cos(),y+r*angle.sin(),stroke/2.0,col);}
            }
        }
    }
    let logo_size=if small{if triple{6.0}else if dual{10.0}else{12.0}}else if triple{20.0}else if dual{24.0}else{34.0};
    // 小环最内圈半径只有 6 上下，logo 收到 7.5 才留得出一圈暗隙，双子星标志不会被金圈切到。
    if a.provider==Provider::Kiro{logo(c,a.provider,x,y,logo_size);}
    else if a.provider==Provider::ZCode && windows.is_empty(){logo(c,a.provider,x,y,if size<35.0{size-3.0}else{size*0.72});}
    else if dual {logo(c,a.provider,x,y,logo_size);}else if let Some(w)=windows.first(){
        let value=format!("{:.0}",w.remaining);let large=if value.len()>2{22.0}else{28.0};
        let number_width=c.text_w(&value,large,W::Bold);let width=number_width+c.text_w("%",11.0,W::Semibold)+2.0;
        let left=x-width/2.0;c.centered_text(left,y,&value,large,W::Bold,theme::WHITE,H::Left);
        c.centered_text(left+number_width+2.0,y+3.0,"%",11.0,W::Semibold,theme::GRAY,H::Left);
    }else{c.centered_text(x,y,"—",24.0,W::Semibold,theme::DIM,H::Center);}
}
/// 三圈釉面环（Antigravity 的 Gemini 专属形态）：外「5 小时」、中「Gemini 每周」、内「Argon 每周」，
/// 每圈按自己的剩余量在绿→黄→红之间取色，中央是彩虹四角星。27 像素的常驻位走 micro。
fn gemini_ring(c:&Card,snapshot:Option<&Snapshot>,x:f32,y:f32,size:f32){
    let micro=size<35.0;
    // 只画两圈：5 小时（外）与 Gemini 每周（内）。Argon 每周那圈在 27 像素上只剩几段短弧，
    // 读不出量，干脆不画——它仍然出现在卡片下面的额度行里，数值不丢。
    let mut windows:Vec<&quota::Window>=snapshot.map(|s|s.card_windows().into_iter().flatten().collect()).unwrap_or_default();
    windows.truncate(2);
    let count=windows.len().max(1);
    let(width,gap,radii)=crate::quota_visual::ring_layout(size,count,micro);
    let radius=radii.first().copied().unwrap_or(0.0);
    let inner_r=radii.last().copied().unwrap_or(radius);
    // 只留两圈之后中间空出来了，星可以放大到压住内圈：不切缺口，环保持完整，星盖在上面。
    let overlap=if micro{2.0}else{3.0};
    let star=(inner_r-width*0.5+overlap).max(3.0);
    let _=gap;
    if windows.is_empty(){
        crate::quota_visual::glossy_ring(c,x,y,radius,width,0.0,theme::GRAY,micro,&[]);
        crate::quota_visual::gemini_star(c,x,y,star.max(3.0),micro);
        return;
    }
    for (i,w) in windows.iter().enumerate(){
        let r=radii.get(i).copied().unwrap_or(radius);
        let base=crate::quota_visual::quota_color(w.remaining);
        crate::quota_visual::glossy_ring(c,x,y,r,width,w.remaining,base,micro,&[]);
    }
    crate::quota_visual::gemini_star(c,x,y,star.max(3.0),micro);
}
fn reset_text(at:Option<i64>,now:&chrono::DateTime<chrono::Local>)->String {
    let Some(at)=at.and_then(|at|chrono::DateTime::from_timestamp(at,0)) else{return "重置时间未知".into();};
    let at=at.with_timezone(&chrono::Local);
    if at.timestamp()<=now.timestamp(){return "等待刷新".into();}
    let format=if at.date_naive()==now.date_naive(){"今天 %H:%M 重置"}else if at.year()==now.year(){"%-m月%-d日 %H:%M 重置"}else{"%Y/%-m/%-d %H:%M 重置"};
    at.format(format).to_string()
}
/// 卡片底部那行「CLI · 邮箱 · 令牌几点到期」。只读系统凭据，不发请求；点它去设置面板。
fn cli_login_line(c:&Card,login:Option<&quota::antigravity::Login>,id:&str,cli_y:f32)->bool{
    let line=match login{
        Some(login)=>{
            let when=login.expires_at.and_then(|at|chrono::DateTime::from_timestamp(at,0)).map(|at|at.with_timezone(&chrono::Local).format("%H:%M").to_string()).unwrap_or_default();
            if login.expired(){format!("CLI · {} · 令牌已过期",login.email)}
            else if when.is_empty(){format!("CLI · {}",login.email)}
            else{format!("CLI · {} · 令牌 {when} 到期",login.email)}
        }
        None=>"CLI · 未检测到登录态（Gemini 每周与 Argon 每周读不到）".to_string(),
    };
    let warn=login.is_none_or(quota::antigravity::Login::expired);
    c.centered_text(157.0,cli_y,&c.fit(&line,9.0,W::Regular,200.0),9.0,W::Regular,if warn{Color32::from_rgb(226,164,44)}else{theme::DIM},H::Left);
    c.hit(157.0,cli_y-7.0,200.0,13.0,("quota-cli-login",id)).on_hover_text("本机 Antigravity CLI 登录态：两个周额度都靠它读。点这里打开设置面板，可检测、续期或重新登录。").clicked()
}
fn snapshot_details(snapshot:&Snapshot)->String{    if let Some(relay)=&snapshot.relay{return relay.full();}
    if snapshot.kiro.is_some(){return kiro_details(snapshot);}
    let mut lines=Vec::new();let now=chrono::Local::now();
    for w in snapshot.windows.iter().chain(snapshot.extra.iter()){
        lines.push(format!("{}：剩余 {:.1}% · {}",w.label,w.remaining,reset_text(w.reset,&now)));
    }
    lines.join("\n")
}
fn connection_group(provider:Provider)->Provider{
    if provider==Provider::Glm{Provider::ZCode}else if provider.relay(){Provider::NewApi}else{provider}
}
fn connection_name(provider:Provider)->&'static str{
    match connection_group(provider){Provider::ZCode=>"GLM / ZCode",Provider::NewApi=>"中转站",other=>other.name()}
}
fn kiro_details(snapshot:&Snapshot)->String{
    let Some(details)=&snapshot.kiro else{return String::new();};
    let mut lines=details.pools.iter().map(|p|{
        let time=if let Some(at)=p.expires{
            chrono::DateTime::from_timestamp(at,0).map(|t|format!("{} 到期",t.with_timezone(&chrono::Local).format("%Y/%-m/%-d %H:%M"))).unwrap_or_default()
        }else if p.reset.is_some(){reset_text(p.reset,&chrono::Local::now())}else{String::new()};
        format!("{}：{} 点{}",p.label,p.amount.full(),if time.is_empty(){String::new()}else{format!(" · {time}")})
    }).collect::<Vec<_>>();
    for window in snapshot.windows.iter().skip(1){lines.push(format!("{}：{} 点 · {}",window.label,window.value_text(),reset_text(window.reset,&chrono::Local::now())));}
    lines.join("\n")
}
fn quota_reset_text(snapshot:&Snapshot,window:&quota::Window,now:&chrono::DateTime<chrono::Local>)->String{
    if window.label=="可用点数"{
        if let Some(details)=&snapshot.kiro{return details.pools.first().map(|p|format!("月度 · {}",reset_text(p.reset,now))).unwrap_or_default();}
    }
    reset_text(window.reset,now)
}
fn claude_disclaimer(ui:&mut egui::Ui){
    ui.separator();
    ui.label(egui::RichText::new("免责声明").size(12.0).strong());
    ui.add(egui::Label::new(egui::RichText::new("岛仔为非官方辅助工具，数据仅供参考，以 Claude 官方显示为准；本地读取不构成账号安全保证。").size(12.0).color(crate::settings_theme::color(ui.ctx(),Color32::from_gray(180),Color32::from_gray(75)))).wrap());
}
impl App {
    pub(crate) fn quota_close_connection(&mut self){
        if let Some(connect)=&mut self.quota.kiro_connecting{connect.cancel();}
        self.quota.connect_open=false;self.quota.connect_target=None;
        self.quota.api_connecting=None;self.quota.api_key.clear();
        self.quota.gemini_connecting=None;
        self.quota.antigravity_connecting=None;
        self.quota.deepseek_key.clear();self.quota.gemini_cookie.clear();
        self.quota.doubao_connecting=None;self.quota.doubao_cookie.clear();
    }
    pub(crate) fn quota_tick(&mut self){
        // 周额度是跑本机 CLI 拿的（6~10 秒），单开一轮，别跟着接口刷新一起转。
        self.quota.poll_weekly();self.quota.maybe_start_weekly();
        // 登录态与「续期」结果：只读凭据 + 后台线程换令牌，不碰 CLI 进程。
        self.quota.poll_login();
        if (!self.settings_open||!self.quota.connect_open)&&self.quota.gemini_connecting.is_some(){self.quota.gemini_connecting=None;self.quota.gemini_cookie.clear();}
        if (!self.settings_open||!self.quota.connect_open)&&self.quota.doubao_connecting.is_some(){self.quota_close_connection();}
        if !self.settings_open && self.quota.api_connecting.is_some(){self.quota_close_connection();}
        if let Some(result)=self.quota.api_connecting.as_ref().and_then(|c|c.poll()){
            self.quota.api_connecting=None;
            // Only an open, current dialog may persist a validated key.
            if self.settings_open && self.quota.connect_open {
                match result.and_then(|v|v.finish()){
                    Ok(connected)=>{
                        let previous=self.quota.config.clone();
                        let outcome=self.quota.config.connect_api(&connected,self.quota.connect_target.as_deref()).and_then(|id|self.quota.config.save_checked().map(|_|id));
                        match outcome{
                            Ok(id)=>{
                                self.quota.rows.insert(id.clone(),Ok(connected.snapshot));self.quota.stale.remove(&id);self.quota.busy.remove(&id);self.quota.save_snapshots();self.quota.connect_target=None;self.quota.note="已连接；数据以服务端返回为准".into();self.pending.push(Cmd::QuotaConnected);
                            },
                            Err(error)=>{self.quota.config=previous;self.quota.note=error;}
                        }
                    },Err(error)=>self.quota.note=error
                }
                if let Err(error)=quota::api::cleanup_pending(&self.quota.config){self.quota.note=format!("{}；{error}",self.quota.note);}
            }
        }

        if !self.settings_open||!self.quota.connect_open{if let Some(connect)=&mut self.quota.kiro_connecting{connect.cancel();}}
        if let Some(result)=self.quota.kiro_connecting.as_ref().and_then(|c|c.poll()){
            let connect=self.quota.kiro_connecting.take().unwrap();
            if !connect.cancelled(){
                self.quota.note=match result.and_then(|connected|self.quota.connect_kiro(connect.target.as_ref(),connected)){Ok(())=>{self.pending.push(Cmd::QuotaConnected);"Kiro 已连接，用户身份已核验".into()},Err(error)=>error};
            }
        }
        if let Some(result)=self.quota.workbuddy_connecting.as_ref().and_then(|c|c.poll()){
            self.quota.workbuddy_connecting=None;
            match result{Ok(connected)=>{
                let id=self.quota.config.connect_workbuddy(connected.slot,connected.label);self.quota.stale.remove(&id);self.quota.rows.insert(id,Ok(connected.snapshot));self.quota.save_snapshots();self.quota.connect_target=None;self.quota.config.save();self.quota.note="WorkBuddy 已连接".into();self.pending.push(Cmd::QuotaConnected);
            },Err(error)=>self.quota.note=error}
        }

        if let Some(result)=self.quota.deepseek_connecting.as_ref().and_then(|connect|connect.poll()){
            self.quota.deepseek_connecting=None;
            match result{Ok(connected)=>{let old=self.quota.connect_target.as_ref().and_then(|id|self.quota.config.accounts.iter().find(|a|&a.id==id)).cloned();
                let id=if let Some(target)=self.quota.connect_target.as_deref(){match self.quota.config.reconnect_deepseek(target,connected.slot.clone(),connected.label){Ok(id)=>id,Err(error)=>{self.quota.note=error;return;}}}else{self.quota.config.connect_deepseek(connected.slot.clone(),connected.label)};
                if let Some(old)=old.filter(|a|a.slot.as_deref()!=Some(&connected.slot)){quota::deepseek::forget(&old);}
                self.quota.connect_target=None;self.quota.stale.remove(&id);self.quota.rows.insert(id,Ok(connected.snapshot));self.quota.save_snapshots();self.quota.config.save();self.quota.note="DeepSeek 已连接".into();self.pending.push(Cmd::QuotaConnected);},Err(error)=>self.quota.note=error}
        }
        let result=self.quota.gemini_connecting.as_ref().and_then(|connect|connect.poll());
        if let Some(result)=result{
            self.quota.gemini_connecting=None;
            if self.settings_open&&self.quota.connect_open{
                match result.and_then(|v|v.finish()){Ok(connected)=>{
                    let previous=self.quota.config.clone();let id=self.quota.config.connect_gemini(connected.slot,connected.label);
                    if let Err(error)=self.quota.config.save_checked(){self.quota.config=previous;self.quota.note=error;}
                    else{self.quota.rows.insert(id.clone(),Ok(connected.snapshot));self.quota.stale.remove(&id);self.quota.save_snapshots();self.quota.connect_target=None;self.quota.gemini_cookie.clear();self.quota.note="Gemini 已连接".into();self.pending.push(Cmd::QuotaConnected);}
                },Err(error)=>self.quota.note=error}
            }
        }
        if self.quota.antigravity_connecting.as_ref().is_some_and(|connect|connect.timed_out()){self.quota.antigravity_connecting=None;self.quota.note="读取本机 Antigravity 登录态超时，请稍后重试".into();}
        let result=self.quota.antigravity_connecting.as_ref().and_then(|connect|connect.poll());
        if let Some(result)=result{
            self.quota.antigravity_connecting=None;
            if self.settings_open&&self.quota.connect_open{
                match result{Ok(connected)=>{
                    let previous=self.quota.config.clone();let id=self.quota.config.connect_antigravity(connected.label);
                    if let Err(error)=self.quota.config.save_checked(){self.quota.config=previous;self.quota.note=error;}
                    else{self.quota.rows.insert(id.clone(),Ok(connected.snapshot));self.quota.stale.remove(&id);self.quota.save_snapshots();self.quota.connect_target=None;self.quota.note="Gemini 已通过本机 Antigravity 登录态连接".into();self.quota.maybe_start_weekly();self.pending.push(Cmd::QuotaConnected);}
                },Err(error)=>self.quota.note=error}
            }
        }
        if let Some(result)=self.quota.doubao_connecting.as_ref().and_then(|connect|connect.poll()){
            self.quota.doubao_connecting=None;
            if self.settings_open&&self.quota.connect_open{
                let target=self.quota.connect_target.clone();
                match result.and_then(|validated|validated.finish(&mut self.quota.config,target.as_deref())){
                    Ok(connected)=>{
                        self.quota.stale.remove(&connected.id);self.quota.busy.remove(&connected.id);self.quota.rows.insert(connected.id,Ok(connected.snapshot));
                        self.quota.save_snapshots();self.quota.connect_target=None;self.quota.doubao_cookie.clear();
                        self.quota.note=connected.cleanup_error.map(|error|format!("豆包已连接；{error}")).unwrap_or_else(||"豆包已连接".into());self.pending.push(Cmd::QuotaConnected);
                    },Err(error)=>self.quota.note=error
                }
            }
        }
        let result=self.quota.login.as_ref().and_then(|login|login.poll());
        if let Some(result)=result{
            self.quota.login=None;
            match result{Ok(connected)=>{self.quota.config.connect_grok(connected.slot,connected.label);self.quota.config.save();self.quota.note="Grok Bot 已连接".into();self.pending.push(Cmd::QuotaConnected);},Err(error)=>self.quota.note=error}
        }
    }
    fn quota_login(&mut self,alias:String){
        if self.quota.login.is_some(){return;}
        match quota::grok::Login::start(alias){Ok((login,url))=>{self.quota.login=Some(login);self.quota.note="请在浏览器中完成登录，账号会自动添加".into();self.pending.push(Cmd::OpenUrl(url));},Err(error)=>self.quota.note=error}
    }
    fn quota_start_kiro(&mut self,path:std::path::PathBuf){
        if self.quota.kiro_connecting.is_some(){return;}
        let target=self.quota.connect_target.as_ref().and_then(|id|self.quota.config.accounts.iter().find(|a|&a.id==id)).cloned();
        if self.quota.connect_target.is_some()&&target.is_none(){self.quota.note="原账号已移除，请重新添加".into();return;}
        self.quota.note.clear();self.quota.kiro_connecting=Some(quota::kiro::Connect::start(path,self.quota.alias.clone(),target));
    }
    pub fn quota_config(&self)->&Config{&self.quota.config}
    pub fn quota_initialize(&mut self){if !crate::model::config_dir().join("ai-quota.json").exists(){self.quota.config.discover();self.quota.config.save();}}
    pub fn quota_pending(&mut self,accounts:&[Account]){self.quota.busy.extend(accounts.iter().map(|a|a.id.clone()));}
    /// 额度页是否正开着（页 6 且这一页没在设置里被隐藏）：决定自动刷新用 30 秒还是 3 分钟。
    pub fn quota_page_live(&self)->bool{self.open&&self.page()==6&&self.settings.visible_pages[6]}
    /// 卡片里要排几行窗口：四行时要把额度页撑高，否则重置时间和下一行会叠在一起。
    pub fn quota_card_rows(&self)->usize{
        self.quota.config.selected_index().and_then(|index|self.quota.config.accounts.get(index)).and_then(|a|self.quota.rows.get(&a.id)).and_then(|r|r.as_ref().ok()).map(|s|s.ordered().len()).unwrap_or(0)
    }
    /// 记下这一轮刷新按多长间隔排的，页面靠它显示「Ns 后刷新」。
    pub fn quota_scheduled(&mut self,every:std::time::Duration){self.quota.scheduled_at=Some(std::time::Instant::now());self.quota.scheduled_every=every;}
    pub fn quota_event(&mut self,event:quota::Event){self.quota.receive(event);}
    pub fn quota_set_source(&mut self,path:std::path::PathBuf){self.quota.source=path.to_string_lossy().into_owned();}
    pub(crate) fn quota_bar_indices(&self)->Vec<usize>{
        if !self.quota.config.enabled || !self.quota.config.compact || !self.settings.visible_pages[6] || self.settings.minimal || self.t<self.notification_bell_until{return vec![];}
        self.quota.config.pinned_indices()
    }
    /// 常驻胶囊里每个额度槽的实际宽度：图标右侧跟数字的账号需要额外占位。
    pub(crate) fn quota_balance_layout(&self,ctx:&egui::Context)->Vec<(usize,f32)>{
        self.quota_bar_indices().iter().enumerate().filter_map(|(position,index)|{
            let account=&self.quota.config.accounts[*index];
            let extra=if let Some(text)=self.quota.tail_text(&account.id){ctx.fonts(|fonts|fonts.layout_no_wrap(text,theme::font(theme::size::PILL_CLOCK,W::Bold),theme::WHITE).size().x).ceil()+6.0}
                else if account.provider.balance()&&self.quota.balance_loading(&account.id){22.0}
                else{return None;};
            Some((position,extra))
        }).collect()
    }
    pub(crate) fn draw_quota_bar(&mut self,c:&Card,cx:f32,layout:&crate::bar_layout::Layout){
        let indices=self.quota_bar_indices();let mut chosen=None;
        for (position,i)in indices.iter().enumerate(){
            let alpha=layout.pet_clearance(crate::bar_layout::quota_slot(position));
            let faded=c.faded(alpha);let faded=if alpha<0.8{faded.ghost()}else{faded};let c=&faded;
            let a=&self.quota.config.accounts[*i];let x=cx+layout.x[crate::bar_layout::quota_slot(position)].unwrap();
            let snapshot=self.quota.rows.get(&a.id).and_then(|r|r.as_ref().ok());
            if a.provider.balance(){
                let wide=true;let extra=if wide{layout.widths[crate::bar_layout::quota_slot(position)]-27.0}else{0.0};let icon_x=x-extra/2.0;
                if a.provider!=Provider::DeepSeek{logo(c,a.provider,icon_x,15.0,22.0);}else{deepseek_badge(c,self.settings.skin,icon_x,15.0,24.0);}
                let loading=self.quota.balance_loading(&a.id);let stale=self.quota.stale.contains_key(&a.id);
                if stale||(!loading&&snapshot.is_none()){
                    c.circle(icon_x+9.0,24.0,3.8,Color32::BLACK);
                    c.circle(icon_x+9.0,24.0,2.5,Color32::from_rgb(240,165,84));
                }
                if wide{
                    if let Some(text)=self.quota.balance_text(&a.id){
                        let wallet=snapshot.and_then(|s|s.wallet.as_ref());
                        let color=if stale{theme::GRAY}else if wallet.is_some_and(|w|!w.available||w.primary().total<=0.0){Color32::from_rgb(255,166,77)}else{theme::WHITE};
                        c.text(icon_x+18.0,21.0,&text,theme::size::PILL_CLOCK,W::Bold,color,H::Left);
                    }else if loading{
                        let active=(self.t*3.0) as usize%3;
                        for dot in 0..3{c.circle(icon_x+20.0+dot as f32*5.0,15.0,1.65,theme::GRAY.gamma_multiply(if dot==active{1.0}else{0.35}));}
                        c.ui.ctx().request_repaint_after(std::time::Duration::from_millis(160));
                    }
                }
                if c.hit(x-(27.0+extra)/2.0,1.0,27.0+extra,28.0,("quota-pin",&a.id)).clicked(){chosen=Some(a.id.clone());}continue;
            }
            // 有具体点数的额度（如 Kiro）在环右侧跟一段剩余点数，与余额类的排布一致。
            let extra=(layout.widths[crate::bar_layout::quota_slot(position)]-27.0).max(0.0);
            let x=if extra>0.0{x-extra/2.0}else{x};
            if a.provider==Provider::Gemini&&a.source.to_string_lossy()==quota::antigravity::SOURCE{gemini_ring(c,snapshot,x,15.0,29.0);}else{ring(c,a,snapshot,x,15.0,27.0,ring_colors(&self.quota.config,a));}
            if extra>0.0{let stale=self.quota.stale.contains_key(&a.id);if let Some(text)=self.quota.tail_text(&a.id){c.text(x+15.5,21.0,&text,theme::size::PILL_CLOCK,W::Bold,if stale{theme::GRAY}else{theme::WHITE},H::Left);}}
            let mut same=self.quota.config.accounts.iter().filter(|other|other.provider==a.provider && other.enabled);
            if same.clone().count()>1{let n=same.position(|other|other.id==a.id).unwrap_or(0)+1;c.circle(x+10.0,24.0,4.5,Color32::from_gray(225));c.text(x+10.0,26.8,&n.to_string(),6.5,W::Bold,Color32::BLACK,H::Center);}
            let summary=snapshot.map(|s|s.ordered().iter().map(|w|w.summary()).collect::<Vec<_>>().join(" · ")).unwrap_or_else(||if self.quota.busy.contains(&a.id){"正在更新".into()}else{"额度暂不可用".into()});
            if bar_tooltip(c.hit(x-14.0,1.0,28.0+extra,28.0,("quota-pin",&a.id)),format!("{} · {}\n{}",a.provider.name(),a.alias,summary)).clicked(){chosen=Some(a.id.clone());}
        }
        if let Some(id)=chosen{self.quota.config.selected=id;self.quota.config.save();self.show_page(6);}
    }
    pub(crate) fn page_quota(&mut self,c:&Card){
        // 页面每帧都会走这里：登录态（本地读凭据）与倒计时都在这一帧对齐。
        self.quota.poll_login();
        // Antigravity 的 Gemini 账号用专属的釉面卡片：底色最先铺，胶囊、刷新、内容都叠在它上面。
        let gemini_form=self.quota.config.selected_index().map(|i|{let a=&self.quota.config.accounts[i];a.provider==Provider::Gemini&&a.source.to_string_lossy()==quota::antigravity::SOURCE}).unwrap_or(false);
        // 不再自绘卡片底色：底下那层由页面统一铺，自己再盖一块会露出圆角和色调的差。
        if c.hit(303.0,8.0,32.0,21.0,"quota-refresh").on_hover_text("刷新额度").clicked(){self.pending.push(Cmd::QuotaRefresh);}
        c.text(319.0,23.0,if self.quota.busy.is_empty(){"刷新"}else{"更新中"},10.5,W::Regular,theme::GRAY,H::Center);
        let Some(selected)=self.quota.config.selected_index() else{
            c.text(43.0,61.0,"还没有添加 AI 账号",13.0,W::Bold,theme::WHITE,H::Left);
            c.text(43.0,80.0,"连接 AI 账号，查看剩余额度",10.5,W::Regular,theme::GRAY,H::Left);
            let(r,b)=crate::paint::capsule(c,43.0,92.0,94.0,24.0,"add-quota-account",theme::BTN,theme::BTN_HOVER);b.text(90.0,108.0,"添加账号",10.0,W::Semibold,theme::WHITE,H::Center);if r.clicked(){self.settings_tab=11;self.open_settings();}return;
        };
        let indices=self.quota.config.ordered_indices(false);
        let pos=indices.iter().position(|i|*i==selected).unwrap_or(0);let start=pos/3*3;let mut picked=None;
        if indices.len()>3{for (x,offset,label)in [(42.0,-1isize,"‹"),(357.0,1,"›")]{if c.hit(x-7.0,30.0,18.0,19.0,("quota-account-next",offset)).clicked(){picked=Some(indices[(pos as isize+offset).rem_euclid(indices.len() as isize) as usize]);}c.text(x+2.0,44.0,label,13.0,W::Regular,theme::GRAY,H::Center);}}
        for (column,index)in indices.iter().skip(start).take(3).enumerate(){let a=&self.quota.config.accounts[*index];let x=59.0+column as f32*99.0;let on=*index==selected;let colors=accent_colors(&self.quota.config,a);let color=colors[0];let(r,b)=crate::paint::capsule(c,x,30.0,94.0,21.0,&format!("quota-account-{}",a.id),if on{Color32::from_rgb(color.r()/5,color.g()/5,color.b()/5)}else{Color32::BLACK},theme::BTN_HOVER);let text=b.fit(&a.alias,10.5,W::Regular,84.0);b.text(x+47.0,45.0,&text,10.5,W::Regular,if on{colors[1]}else{theme::GRAY},H::Center);if r.on_hover_text(format!("{} · {}",a.provider.name(),a.alias)).clicked(){picked=Some(*index);}}
        if let Some(i)=picked{self.quota.config.selected=self.quota.config.accounts[i].id.clone();self.quota.config.save();}
        let a=&self.quota.config.accounts[selected];let result=self.quota.rows.get(&a.id);let snapshot=result.and_then(|r|r.as_ref().ok());
        let colors=ring_colors(&self.quota.config,a);
        // 四条窗口时页面会被撑高到 208：行距、状态行、CLI 行一起下移，任何两行之间都留出空隙。
        let tall=self.quota_card_rows()>2;
        let rows_top=if tall{78.0}else{86.0};
        let rows_pitch=if tall{24.0}else{35.0};
        let status_y=if tall{176.0}else{154.0};
        let cli_y=if tall{188.0}else{167.0};
        if let Some(s)=snapshot.filter(|s|s.at>0){
            if let Some(at)=chrono::DateTime::from_timestamp(s.at,0){
                let at=at.with_timezone(&chrono::Local);let time=at.format(if at.date_naive()==chrono::Local::now().date_naive(){"%H:%M"}else{"%m/%d %H:%M"}).to_string();
                let busy=self.quota.busy.contains(&a.id);let stale=self.quota.stale.get(&a.id);
                let status=if busy{format!("{} · {time}",if a.provider==Provider::Claude{"读取中…"}else{"正在更新…"})}
                    else if stale.is_some(){format!("上次记录 · {time}")}
                    else if a.provider==Provider::Claude{format!("本地记录 · {time}")}
                    else if let Some(wallet)=&s.wallet{let b=wallet.primary();if wallet.balances.len()>1{let other=wallet.balances.iter().find(|other|other.currency!=b.currency).unwrap();format!("另有 {} {} · {time}",other.currency,other.full(other.total))}else{format!("{} · {time} 更新",b.currency)}}
                    else{format!("{time} 更新")};
                // 有排期就补上「Ns 后刷新」：用户一眼知道下一次自己动的时间，不用猜。
                let countdown=self.quota.countdown();
                let status=match &countdown{Some(left)=>format!("{status} · {left}"),None=>status};
                let x=if a.provider.balance(){125.0}else{157.0};let width=357.0-x;let y=status_y;
                c.centered_text(x,y,&c.fit(&status,10.0,W::Regular,width),10.0,W::Regular,if stale.is_some()&&!busy{Color32::from_rgb(255,166,77)}else{theme::DIM},H::Left);
                c.hit(x,y-6.0,width,8.0,("quota-update-time",&a.id)).on_hover_text(if let Some(reason)=stale{format!("{status}\n{reason}")}else{status});
                if countdown.is_some(){c.ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));}
            }
        }
        // Gemini（本机 Antigravity 登录态）的专属卡片：固定三行额度 + 三圈釉面环，Claude/GPT 只在悬停明细里。
        if gemini_form{
            let id=a.id.clone();
            let plan=snapshot.map(|s|s.plan.as_str()).filter(|p|!p.is_empty()).unwrap_or_default();
            let title=if plan.is_empty(){"Gemini".to_string()}else{format!("Gemini · {plan}")};
            c.text(157.0,62.0,&c.fit(&title,12.0,W::Semibold,145.0),12.0,W::Semibold,theme::WHITE,H::Left);
            gemini_ring(c,snapshot,103.0,118.0,78.0);
            let rows=snapshot.map(Snapshot::card_windows);
            // 有 Claude/GPT 的周额度就把第四行也放上：只占一行，环上仍旧只画三圈。
            let mut labels=vec![quota::antigravity::FIVE_HOUR_LABEL,quota::antigravity::GEMINI_WEEKLY_LABEL,quota::antigravity::ARGON_WEEKLY_LABEL];
            if snapshot.is_some_and(|s|s.windows.iter().any(|w|w.label==quota::antigravity::CLAUDE_WEEKLY_LABEL)){
                labels.push(quota::antigravity::CLAUDE_WEEKLY_LABEL);
            }
            let pitch=if labels.len()>3{23.0}else{30.0};
            let now=chrono::Local::now().timestamp();
            for (i,label) in labels.iter().enumerate(){
                let y=(if labels.len()>3{82.0}else{88.0})+i as f32*pitch;
                let window=snapshot.and_then(|s|s.windows.iter().find(|w|w.label==*label));
                let color=window.map(|w|crate::quota_visual::quota_color(w.remaining)).unwrap_or(theme::GRAY);
                c.circle(160.0,y-3.5,2.8,color);
                let (head,head_color)=match window{Some(w)=>(format!("{label} {:.0}%",w.remaining),theme::WHITE),None=>(format!("{label} 读取中…"),theme::GRAY)};
                c.text(169.0,y,&c.fit(&head,13.5,W::Semibold,180.0),13.5,W::Semibold,head_color,H::Left);
                let sub=match window{
                    Some(w)=>match crate::quota_visual::countdown_text(w.reset,now){Some(left)=>format!("重置倒计时：{left}"),None=>"重置时间未知".to_string()},
                    None=>"等待刷新".to_string(),
                };
                c.text(169.0,y+10.0,&c.fit(&sub,if labels.len()>3{9.0}else{9.5},W::Regular,180.0),if labels.len()>3{9.0}else{9.5},W::Regular,theme::GRAY,H::Left);
                c.hit(152.0,y-8.0,205.0,if labels.len()>3{21.0}else{26.0},("quota-window",&id,i)).on_hover_text(format!("{label}\n{sub}"));
            }
            if let Some(snapshot)=snapshot{
                let details=c.hit(309.0,51.0,88.0,20.0,("api-quota-details",&id)).on_hover_text(snapshot_details(snapshot)).clicked();
                if details{self.quota.detail_account=Some(id.clone());self.settings_tab=11;self.open_settings();}
            }
            let clicked={let login=self.quota.antigravity_login.as_ref();cli_login_line(c,login,&id,cli_y)};
            if clicked{self.quota.detail_account=Some(id);self.settings_tab=11;self.open_settings();}
            return;
        }
        if a.provider==Provider::WorkBuddy{
            logo(c,a.provider,85.0,105.0,44.0);
            if let Some(credits)=snapshot.and_then(|s|s.credits.as_ref()){
                c.text(125.0,81.0,"可用积分",10.5,W::Regular,theme::GRAY,H::Left);
                c.text(125.0,107.0,&credits.full(),23.0,W::Semibold,theme::WHITE,H::Left);
                let detail=if let Some(reason)=self.quota.stale.get(&a.id){reason.clone()}else{credits.packages.iter().map(|(label,n)|format!("{label} {n}")).collect::<Vec<_>>().join(" · ")};c.text(125.0,131.0,&c.fit(&detail,10.0,W::Regular,225.0),10.0,W::Regular,theme::GRAY,H::Left);
            }else{let text=result.and_then(|r|r.as_ref().err()).map(String::as_str).unwrap_or("正在查询积分…");for(i,line)in c.wrap(text,11.0,W::Regular,226.0,3).iter().enumerate(){c.text(125.0,92.0+i as f32*17.0,line,11.0,W::Regular,theme::GRAY,H::Left);}}
            return;
        }
        if a.provider==Provider::DeepSeek{
            deepseek_badge(c,self.settings.skin,85.0,105.0,50.0);
            if let Some(s)=snapshot{
                if let Some(wallet)=&s.wallet{
                    let b=wallet.primary();
                    c.text(125.0,81.0,if wallet.available{"可用余额"}else{"余额不可用"},10.5,W::Regular,theme::GRAY,H::Left);
                    let amount=b.full(b.total);let size=(23.0*145.0/c.text_w(&amount,23.0,W::Semibold).max(145.0)).min(23.0);
                    c.text(125.0,109.0,&amount,size,W::Semibold,if !wallet.available{Color32::from_rgb(255,166,77)}else{theme::WHITE},H::Left);
                    for (y,label,value) in [(90.0,"充值",b.topped_up),(118.0,"赠送",b.granted)]{
                        c.text(281.0,y,label,10.0,W::Regular,theme::GRAY,H::Left);
                        let text=b.full(value);let size=10.0*54.0/c.text_w(&text,10.0,W::Semibold).max(54.0);c.text(357.0,y,&text,size,W::Semibold,theme::WHITE,H::Right);
                    }
                }
            }else{let text=if self.quota.busy.contains(&a.id){"正在更新余额…"}else{result.and_then(|r|r.as_ref().err()).map(String::as_str).unwrap_or("等待更新余额")};for (i,line) in c.wrap(text,11.0,W::Regular,226.0,3).iter().enumerate(){c.text(125.0,92.0+i as f32*17.0,line,11.0,W::Regular,theme::GRAY,H::Left);}}
            return;
        }
        if a.provider.relay(){
            logo(c,a.provider,85.0,105.0,44.0);
            if let Some(q)=snapshot.and_then(|s|s.relay.as_ref()){
                if let Some(amount)=q.primary(){
                    let exhausted=amount.remaining.is_some_and(|n|n<=0.0)&&!amount.unlimited;
                    let title=if matches!(amount.scope,quota::relay::Scope::Account){if amount.label=="账户余额"{"账户余额".into()}else{"账户剩余额度".into()}}else{format!("{}剩余",amount.label)};
                    c.text(125.0,78.0,&c.fit(&title,11.0,W::Semibold,176.0),11.0,W::Semibold,theme::GRAY,H::Left);
                    let value=amount.remaining_text();let size=23.0*(225.0/c.text_w(&value,23.0,W::Semibold).max(225.0));
                    c.text(125.0,109.0,&value,size,W::Semibold,if exhausted{Color32::from_rgb(255,166,77)}else{theme::WHITE},H::Left);
                    let detail=if exhausted{format!("{}已用尽",amount.label)}else if matches!(amount.scope,quota::relay::Scope::Subscription){
                        let reset=snapshot.and_then(|s|s.windows.iter().find(|w|w.label==amount.label)).and_then(|w|w.reset);
                        format!("{} · {}",q.source,reset_text(reset,&chrono::Local::now()))
                    }else{format!("{} · 以站点返回单位为准",q.source)};
                    c.text(125.0,132.0,&c.fit(&detail,10.5,W::Regular,225.0),10.5,W::Regular,if exhausted{Color32::from_rgb(255,166,77)}else{theme::GRAY},H::Left);
                }else{
                    c.text(125.0,78.0,"账户余额与套餐额度",11.0,W::Semibold,theme::GRAY,H::Left);
                    c.text(125.0,109.0,q.unavailable_text(),18.0,W::Semibold,theme::WHITE,H::Left);
                    c.text(125.0,132.0,"请到站点查看账户余额或套餐",10.5,W::Regular,theme::GRAY,H::Left);
                }
                if c.hit(310.0,60.0,47.0,22.0,("relay-details",&a.id)).on_hover_text("查看全部周期与额度").clicked(){
                    self.quota.detail_account=Some(a.id.clone());self.settings_tab=11;self.open_settings();
                }
                c.text(355.0,77.0,"明细 ›",10.0,W::Regular,theme::GRAY,H::Right);
            }else{
                let text=result.and_then(|r|r.as_ref().err()).map(String::as_str).unwrap_or("等待更新额度");
                for(i,line)in c.wrap(text,11.0,W::Regular,226.0,3).iter().enumerate(){c.text(125.0,99.0+i as f32*17.0,line,11.0,W::Regular,theme::GRAY,H::Left);}
            }
            return;
        }
        if a.provider.api_key()||a.provider==Provider::Doubao{
            let label=match snapshot.map(|s|s.plan.as_str()).filter(|s|!s.is_empty()){Some(plan)=>format!("{} · {plan}",a.provider.name()),None=>a.provider.name().into()};
            c.text(157.0,64.0,&c.fit(&label,12.0,W::Semibold,145.0),12.0,W::Semibold,theme::WHITE,H::Left);
            if let Some(snapshot)=snapshot{
                let mut main=snapshot.windows.iter().collect::<Vec<_>>();main.sort_by(|a,b|a.remaining.total_cmp(&b.remaining));
                let meter=Snapshot{windows:main.iter().take(2).map(|w|(**w).clone()).collect(),..Default::default()};
                ring(c,a,Some(&meter),103.0,105.0,70.0,ring_colors(&self.quota.config,a));
                let displayed=meter.rings();
                let mut rows=snapshot.windows.iter().chain(snapshot.extra.iter()).take(3).collect::<Vec<_>>();
                if let Some(limit)=main.first(){
                    if rows.len()==3&&!rows.contains(limit){rows[2]=limit;}
                    let exhausted=limit.remaining<=0.0;
                    let label=if exhausted{format!("{}已用尽",limit.label)}else{format!("{}剩余",limit.label)};
                    c.text(103.0,153.0,&c.fit(&label,9.0,W::Regular,96.0),9.0,W::Regular,if exhausted{Color32::from_rgb(255,166,77)}else{theme::GRAY},H::Center);
                }
                let now=chrono::Local::now();
                for (i,w) in rows.iter().enumerate(){
                    let y=83.0+i as f32*23.0;let color=if w.remaining<=0.0{Color32::from_rgb(255,166,77)}else{displayed.iter().position(|v|**v==**w).map(|slot|colors[slot]).unwrap_or(theme::GRAY)};
                    c.circle(159.0,y-4.0,2.5,color);
                    c.text(168.0,y,&c.fit(&w.label,11.0,W::Semibold,114.0),11.0,W::Semibold,theme::WHITE,H::Left);
                    c.text(355.0,y,&format!("{:.0}%",w.remaining),17.0,W::Semibold,if w.remaining<=0.0{color}else{theme::WHITE},H::Right);
                    c.text(168.0,y+12.0,&c.fit(&reset_text(w.reset,&now),9.0,W::Regular,187.0),9.0,W::Regular,theme::GRAY,H::Left);
                }
                let details=c.hit(309.0,51.0,48.0,20.0,("api-quota-details",&a.id)).on_hover_text(snapshot_details(snapshot)).clicked();
                c.text(355.0,64.0,"明细 ›",10.0,W::Regular,theme::GRAY,H::Right);
                if details{self.quota.detail_account=Some(a.id.clone());self.settings_tab=11;self.open_settings();}
            }else{
                logo(c,a.provider,103.0,105.0,40.0);
                let text=result.and_then(|r|r.as_ref().err()).map(String::as_str).unwrap_or("等待更新额度");
                for(i,line)in c.wrap(text,11.0,W::Regular,197.0,3).iter().enumerate(){c.text(157.0,96.0+i as f32*17.0,line,11.0,W::Regular,theme::GRAY,H::Left);}
            }
            return;
        }
        ring(c,a,snapshot,103.0,105.0,70.0,colors);
        let raw_plan=snapshot.map(|s|s.plan.as_str()).unwrap_or_default();let plan=match raw_plan{"pro"|"prolite"|"pro_lite"=>"Pro","plus"=>"Plus","free"=>"Free","team"|"business"=>"Business","enterprise"=>"Enterprise",other=>other};let label=if a.provider==Provider::Claude{"Claude · 本地记录".into()}else if plan.is_empty(){a.provider.name().to_owned()}else{format!("{} · {}",a.provider.name(),plan)};
        c.text(157.0,64.0,&c.fit(&label,12.0,W::Semibold,145.0),12.0,W::Semibold,theme::WHITE,H::Left);
        if let Some(s)=snapshot{
            let windows=s.ordered();let now=chrono::Local::now();
            // 四条窗口时行距收紧到 17.5，重置时间挪进悬停提示：不然会顶到下面的状态行。
            let compact=windows.len()>2;
            if windows.is_empty() && a.provider==Provider::ZCode{
                c.text(157.0,104.0,"官网未分配体验额度",11.0,W::Regular,theme::GRAY,H::Left);
            }else if windows.len()==1{
                let w=windows[0];c.text(157.0,103.0,&format!("{}剩余",w.label),12.0,W::Semibold,theme::WHITE,H::Left);
                if let Some(amount)=&w.amount{let text=format!("{} 点",amount.full());let available=355.0-157.0-c.text_w(&format!("{}剩余",w.label),12.0,W::Semibold)-8.0;let size=12.5*(available/c.text_w(&text,12.5,W::Semibold).max(available));c.text(355.0,103.0,&text,size,W::Semibold,theme::WHITE,H::Right);}
                let reset=quota_reset_text(s,w,&now);c.text(157.0,124.0,&c.fit(&reset,11.0,W::Regular,197.0),11.0,W::Regular,theme::GRAY,H::Left);
            }else{
                c.text(355.0,64.0,"剩余",10.5,W::Regular,theme::GRAY,H::Right);
                for (i,w)in windows.iter().enumerate(){
                    let reset=quota_reset_text(s,w,&now);
                    if compact{
                        // 四行窗口：标签在上、重置时间贴在下面一行（8pt，左对齐）。页面已撑到 208，
                        // 行距 23 从 70 排到 139，重置行落在 149，还剩一截给状态行。
                        let y=rows_top+i as f32*rows_pitch;
                        c.circle(159.0,y-3.0,2.2,colors[i.min(2)]);
                        c.text(168.0,y,&c.fit(&w.label,10.5,W::Semibold,150.0),10.5,W::Semibold,theme::WHITE,H::Left);
                        c.centered_text(355.0,y,&w.value_text(),13.0,W::Semibold,theme::WHITE,H::Right);
                        c.text(168.0,y+10.0,&c.fit(&reset,8.0,W::Regular,150.0),8.0,W::Regular,theme::GRAY,H::Left);
                        c.hit(168.0,y-8.5,187.0,20.0,("quota-window",&a.id,i)).on_hover_text(format!("{} · {}\n{}",w.label,w.value_text(),reset));
                    }else{
                        let y=rows_top+i as f32*rows_pitch;
                        c.circle(159.0,y-4.0,2.5,colors[i.min(2)]);
                        c.text(168.0,y,&c.fit(&w.label,11.5,W::Semibold,150.0),11.5,W::Semibold,theme::WHITE,H::Left);
                        c.centered_text(355.0,y+4.0,&w.value_text(),22.0,W::Semibold,theme::WHITE,H::Right);
                        c.text(168.0,y+17.0,&c.fit(&reset,11.0,W::Regular,185.0),11.0,W::Regular,theme::GRAY,H::Left);
                    }
                }
            }
        }else{let message=if self.quota.busy.contains(&a.id){"正在更新额度…"}else{result.and_then(|r|r.as_ref().err()).map(String::as_str).unwrap_or("等待更新额度")};for(i,line)in c.wrap(message,11.0,W::Regular,197.0,2).iter().enumerate(){c.text(157.0,103.0+i as f32*17.0,line,11.0,W::Regular,theme::GRAY,H::Left);}}
        // 卡片底部：本机 CLI 登录态。只对 Antigravity 账号有意义，读的是系统凭据，不发任何请求。
        if gemini_form{
            let id=a.id.clone();
            let clicked={let login=self.quota.antigravity_login.as_ref();cli_login_line(c,login,&id,cli_y)};
            if clicked{self.quota.detail_account=Some(id);self.settings_tab=11;self.open_settings();}
        }
    }
    fn quota_open_connection(&mut self,id:Option<String>){
        if self.quota.kiro_connecting.is_some()||self.quota.api_connecting.is_some()||self.quota.workbuddy_connecting.is_some()||self.quota.deepseek_connecting.is_some()||self.quota.gemini_connecting.is_some()||self.quota.doubao_connecting.is_some()||self.quota.login.is_some(){self.quota.connect_open=true;return;}
        self.quota.note.clear();self.quota.api_key.clear();self.quota.deepseek_key.clear();self.quota.gemini_cookie.clear();self.quota.doubao_cookie.clear();self.quota.source.clear();self.quota.alias.clear();
        self.quota.gemini_email.clear();
        self.quota.relay_manual=id.is_some();
        if let Some(a)=id.as_ref().and_then(|id|self.quota.config.accounts.iter().find(|a|&a.id==id)){
            self.quota.provider=a.provider;self.quota.alias=a.alias.clone();self.quota.source=a.source.to_string_lossy().into();
            if a.provider==Provider::Gemini{self.quota.gemini_email=quota::gemini::address(a).unwrap_or_default();}
        }
        self.quota.connect_target=id;self.quota.connect_open=true;
        // 连接面板要告诉用户「本机有没有 Antigravity 登录态」，读一次系统凭据（毫秒级）。
        self.quota.antigravity_email=quota::antigravity::available();
        self.quota.refresh_login();
    }
    fn quota_remove_account(&mut self,id:&str){
        if let Some(a)=self.quota.config.accounts.iter().find(|a|a.id==id&&(a.provider.api_key()||a.provider==Provider::Doubao)).cloned(){
            if a.provider.api_key(){if let Some(slot)=&a.slot{if let Err(error)=quota::api::queue_cleanup(slot){self.quota.note=error;return;}}}
            let mut candidate=self.quota.config.clone();candidate.accounts.retain(|a|a.id!=id);candidate.colors.remove(id);
            if candidate.selected==id{candidate.selected=candidate.accounts.iter().find(|a|a.enabled).map(|a|a.id.clone()).unwrap_or_default();}
            if let Err(error)=candidate.save_checked(){self.quota.note=error;return;}
            self.quota.config=candidate;self.quota.rows.remove(id);self.quota.stale.remove(id);self.quota.busy.remove(id);self.quota.save_snapshots();
            self.quota.note=match if a.provider==Provider::Doubao{quota::doubao::forget(&a)}else{quota::api::cleanup_pending(&self.quota.config)}{Ok(())=>"账号已移除".into(),Err(error)=>error};return;
        }
        if let Some(a)=self.quota.config.accounts.iter().find(|a|a.id==id){match a.provider{Provider::GrokBot=>quota::grok::forget(a),
            // Antigravity 账号用的是系统凭据（用户的 Antigravity 登录态），删账号不能顺手把凭据清掉。
            Provider::Gemini=>{if a.source.to_string_lossy()!=quota::antigravity::SOURCE{quota::gemini::forget(a)}},Provider::DeepSeek=>quota::deepseek::forget(a),Provider::WorkBuddy=>quota::workbuddy::forget(a),_=>{}}}
        self.quota.config.accounts.retain(|a|a.id!=id);self.quota.config.colors.remove(id);self.quota.rows.remove(id);self.quota.stale.remove(id);self.quota.busy.remove(id);self.quota.save_snapshots();
        if self.quota.config.selected==id{self.quota.config.selected=self.quota.config.accounts.first().map(|a|a.id.clone()).unwrap_or_default();}
    }
    pub(crate) fn quota_settings(&mut self,ui:&mut egui::Ui){
        let before=self.quota.config.clone();
        settings_card(ui,|ui|{
            setting_row(ui,"开启 AI 额度","打开额度页时每 30 秒更新，后台每 3 分钟",&mut self.quota.config.enabled);
            ui.separator();
            if self.quota.config.accounts.iter().any(|a|a.provider==Provider::Gemini&&a.source.to_string_lossy()==quota::antigravity::SOURCE){
                let login=self.quota.antigravity_login.clone();
                ui.horizontal_wrapped(|ui|{
                    ui.label(egui::RichText::new("Antigravity CLI 登录态").size(12.0).strong());
                    let (text,color)=match &login{
                        Some(login) if login.expired()=>(format!("{} · 令牌已过期",login.email),Color32::from_rgb(226,164,44)),
                        Some(login)=>match login.expires_at.and_then(|at|chrono::DateTime::from_timestamp(at,0)).map(|at|at.with_timezone(&chrono::Local).format("%H:%M").to_string()){
                            Some(when)=>(format!("{} · 令牌 {when} 到期",login.email),ui.visuals().weak_text_color()),
                            None=>(format!("{} · 已登录",login.email),ui.visuals().weak_text_color()),
                        },
                        None=>("未检测到登录态".to_string(),Color32::from_rgb(226,164,44)),
                    };
                    ui.label(egui::RichText::new(text).size(11.0).color(color));
                });
                ui.horizontal_wrapped(|ui|{
                    if ui.button("检测").clicked(){self.quota.renew_note=None;self.quota.refresh_login();}
                    if ui.add_enabled(self.quota.renew_rx.is_none()&&login.is_some(),egui::Button::new(if self.quota.renew_rx.is_some(){"续期中…"}else{"立即续期"})).clicked(){self.quota.renew_auto=false;self.quota.renew();}
                    if ui.button("打开 CLI 登录").clicked(){
                        self.quota.renew_note=Some(match quota::antigravity::open_login(){Ok(())=>"已在新窗口启动 Antigravity CLI：按提示完成 Google 登录，然后点「检测」。".into(),Err(error)=>error});
                    }
                });
                if let Some(note)=&self.quota.renew_note{ui.add(egui::Label::new(egui::RichText::new(note).size(11.0).color(crate::settings_theme::color(ui.ctx(),Color32::from_rgb(231,175,108),Color32::from_rgb(151,94,28)))).wrap());}
                ui.add(egui::Label::new(egui::RichText::new("令牌快到期（10 分钟内）会自动续期一次，只把新令牌放进内存，不改动 Antigravity 自己的文件；登录态与检测都只读系统凭据；岛仔不会在后台偷偷调用 CLI。").size(11.0).color(ui.visuals().weak_text_color())).wrap());
                if self.quota.renew_rx.is_some(){ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));}
                ui.separator();
            }
            setting_row(ui,"收起时显示常驻账号","WorkBuddy 积分在前，DeepSeek 余额在最后",&mut self.quota.config.compact);
        });
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui|{
            if ui.button("＋ 添加账号").clicked(){self.quota_open_connection(None);}
            if ui.button("识别本机账号").clicked(){self.quota.note=self.quota.config.discover().message();}
            if ui.add_enabled(self.quota.busy.is_empty(),egui::Button::new("刷新额度")).clicked(){self.pending.push(Cmd::QuotaRefresh);}
        });ui.add_space(8.0);
        let accounts=self.quota.config.accounts.clone();let mut remove=None;let mut connect=None;let mut reorder=None;
        for mut a in accounts{
            let fill=crate::settings_theme::color(ui.ctx(),Color32::from_rgb(35,35,38),Color32::WHITE);
            let row=egui::Frame::new().fill(fill).corner_radius(10).inner_margin(egui::Margin::symmetric(16,12)).show(ui,|ui|{
                ui.set_width((ui.available_width()-1.0).max(0.0));ui.spacing_mut().item_spacing.y=6.0;
                ui.push_id(a.id.clone(),|ui|{
                    ui.horizontal(|ui|{
                        ui.add(egui::Label::new("::").sense(egui::Sense::drag())).on_hover_text("拖动调整账号顺序").dnd_set_drag_payload(a.id.clone());
                        let (rect,_)=ui.allocate_exact_size(egui::vec2(26.0,28.0),egui::Sense::hover());let c=Card::new(ui,rect.min,ui.clip_rect());
                        if a.provider==Provider::DeepSeek{deepseek_badge(&c,if ui.visuals().dark_mode{crate::model::Skin::Classic}else{crate::model::Skin::GlassLight},13.0,14.0,26.0);}else if matches!(a.provider,Provider::ZCode|Provider::Kiro){logo(&c,a.provider,13.0,14.0,26.0);}else{c.circle(13.0,14.0,13.0,crate::paint::mix(accent_colors(&self.quota.config,&a)[0],Color32::BLACK,0.75));logo(&c,a.provider,13.0,14.0,20.0);}
                        ui.label(egui::RichText::new(if a.provider.relay(){"中转站"}else{a.provider.name()}).strong());
                        // 从右往左排：按钮先靠右放好，名称输入框占用剩余宽度，按钮再宽也不会顶出卡片。
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                            if ui.add_enabled(self.quota.api_connecting.is_none()&&self.quota.workbuddy_connecting.is_none()&&self.quota.deepseek_connecting.is_none()&&self.quota.gemini_connecting.is_none()&&self.quota.doubao_connecting.is_none()&&self.quota.login.is_none(),egui::Button::new("移除")).clicked(){remove=Some(a.id.clone());}
                            if ui.selectable_label(self.quota.config.selected==a.id,"当前").clicked(){self.quota.config.selected=a.id.clone();}
                            ui.add_sized([ui.available_width().max(55.0),26.0],egui::TextEdit::singleline(&mut a.alias).hint_text("账号名称")).on_hover_text("直接修改账号名称");
                        });
                    });
                    let result=self.quota.rows.get(&a.id);let snapshot=result.and_then(|r|r.as_ref().ok());let error=self.quota.stale.get(&a.id).or_else(||result.and_then(|r|r.as_ref().err()));
                    if let Some(s)=snapshot{
                        if a.provider.api_key()||a.provider==Provider::Doubao{ui.label(snapshot_details(s));}else if let Some(credits)=&s.credits{ui.horizontal_wrapped(|ui|{ui.label(egui::RichText::new("可用积分").size(11.0).color(ui.visuals().weak_text_color()));ui.label(egui::RichText::new(credits.full()).size(20.0).strong());ui.label(egui::RichText::new(credits.packages.iter().map(|(label,n)|format!("{label} {n}")).collect::<Vec<_>>().join(" · ")).size(11.0).color(ui.visuals().weak_text_color()));});}else if let Some(wallet)=&s.wallet{
                            ui.horizontal_wrapped(|ui|{
                                ui.label(egui::RichText::new(if wallet.available{"可用余额"}else{"余额不可用"}).size(11.0).color(ui.visuals().weak_text_color()));
                                ui.label(egui::RichText::new(wallet.primary().full(wallet.primary().total)).size(20.0).strong());
                                ui.label(egui::RichText::new(format!("充值 {} · 赠送 {}",wallet.primary().full(wallet.primary().topped_up),wallet.primary().full(wallet.primary().granted))).size(11.0).color(ui.visuals().weak_text_color()));
                            });
                        }else if a.provider==Provider::ZCode && s.windows.is_empty(){
                            ui.label("已连接 · 暂无体验额度");
                        }else if let Some(details)=&s.kiro{
                            if details.pools.len()>1{if let Some(window)=s.windows.first(){ui.label(format!("{}：{} 点",window.label,window.value_text()));}}
                            ui.label(egui::RichText::new(kiro_details(s)).size(11.0));
                        }else{
                            let windows=s.ordered();let count=windows.len().max(1);let width=(ui.available_width()-12.0*(count-1) as f32)/count as f32;
                            ui.horizontal_top(|ui|{ui.spacing_mut().item_spacing.x=12.0;for window in windows{ui.allocate_ui_with_layout(egui::vec2(width,30.0),egui::Layout::top_down(egui::Align::Min),|ui|{
                                ui.horizontal(|ui|{ui.label(egui::RichText::new(format!("{}  剩余 {}",window.label,window.value_text())).size(11.0));});
                                ui.add(egui::ProgressBar::new((window.remaining/100.0).clamp(0.0,1.0)).desired_width(width).desired_height(3.0).fill(accent_colors(&self.quota.config,&a)[0]));
                            });}});
                        }
                    }
                    if !a.enabled{ui.label(egui::RichText::new("已暂停监控").size(11.0).color(ui.visuals().weak_text_color()));}
                    else if let Some(error)=error{ui.add(egui::Label::new(egui::RichText::new(error).size(11.0).color(crate::settings_theme::color(ui.ctx(),Color32::from_rgb(231,175,108),Color32::from_rgb(151,94,28)))).wrap());}
                    else if snapshot.is_none(){ui.label(egui::RichText::new(if self.quota.busy.contains(&a.id){"正在查询额度…"}else{"等待更新"}).size(11.0).color(ui.visuals().weak_text_color()));}
                    ui.horizontal_wrapped(|ui|{
                        ui.checkbox(&mut a.enabled,"监控");ui.checkbox(&mut a.pinned,"常驻岛上");
                        if ui.add_enabled(!self.quota.busy.contains(&a.id),egui::Button::new(if error.is_some(){"重试"}else{"刷新"})).clicked(){self.pending.push(Cmd::QuotaRefresh);}
                        if ui.button("重新连接").clicked(){connect=Some(a.id.clone());}
                        if let Some(s)=snapshot{let time=chrono::DateTime::from_timestamp(s.at,0).map(|at|at.with_timezone(&chrono::Local).format(if a.provider==Provider::Claude{"%m-%d %H:%M 本地记录"}else{"%H:%M 更新"}).to_string()).unwrap_or_default();ui.label(egui::RichText::new(time).size(10.0).color(ui.visuals().weak_text_color()));}
                    });
                    if !a.provider.balance(){
                        let mut color=self.quota.config.colors.get(&a.id).copied().unwrap_or_default();color_picker(ui,a.provider,&mut color);
                        if color==Accent::Brand{self.quota.config.colors.remove(&a.id);}else{self.quota.config.colors.insert(a.id.clone(),color);}
                    }
                    if a.provider==Provider::Claude{
                        ui.separator();ui.label(egui::RichText::new("本地读取说明").size(12.0).strong());
                        ui.add(egui::Label::new(egui::RichText::new(CLAUDE_LOCAL_NOTICE).size(12.0).color(crate::settings_theme::color(ui.ctx(),Color32::from_gray(180),Color32::from_gray(75)))).wrap());
                        claude_disclaimer(ui);
                    }
                });
            });
            if self.quota.detail_account.as_deref()==Some(a.id.as_str()){
                row.response.scroll_to_me(Some(egui::Align::Min));self.quota.detail_account=None;
            }
            if row.response.dnd_hover_payload::<String>().is_some(){ui.painter().rect_stroke(row.response.rect,10,egui::Stroke::new(2.0,ui.visuals().selection.bg_fill),egui::StrokeKind::Inside);}
            if let Some(from)=row.response.dnd_release_payload::<String>(){reorder=Some(((*from).clone(),a.id.clone()));}
            if let Some(account)=self.quota.config.accounts.iter_mut().find(|account|account.id==a.id){*account=a;}
            ui.add_space(8.0);
        }
        if let Some((from,to))=reorder{self.quota.config.move_account(&from,&to);}
        if let Some(id)=remove{self.quota_remove_account(&id);}
        if let Some(id)=connect{self.quota_open_connection(Some(id));}
        if !self.quota.connect_open&&!self.quota.note.is_empty(){ui.add_space(8.0);ui.label(egui::RichText::new(&self.quota.note).size(12.0).color(ui.visuals().weak_text_color()));}
        self.quota_connect_window(ui.ctx());
        if before!=self.quota.config{self.quota.config.save();}
    }
    fn quota_connect_window(&mut self,ctx:&egui::Context){
        if !self.quota.connect_open{return;}let mut open=true;
        egui::Window::new(egui::RichText::new(if self.quota.connect_target.is_some(){"重新连接账号"}else{"添加账号"}).size(16.0).color(crate::settings_theme::color(ctx,Color32::from_gray(235),Color32::from_gray(35)))).id(egui::Id::new("quota-connect-window")).open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER,egui::Vec2::ZERO).default_width(410.0).show(ctx,|ui|{
            ui.set_max_width(410.0);ui.style_mut().wrap_mode=Some(egui::TextWrapMode::Wrap);ui.spacing_mut().item_spacing.y=9.0;ui.visuals_mut().override_text_color=Some(crate::settings_theme::color(ctx,Color32::from_gray(225),Color32::from_gray(35)));
            egui::ScrollArea::vertical().max_height(460.0).show(ui,|ui|{
            let previous_provider=self.quota.provider;
            let mut group=connection_group(self.quota.provider);
            ui.add_enabled_ui(self.quota.connect_target.is_none()&&self.quota.api_connecting.is_none()&&self.quota.kiro_connecting.is_none()&&self.quota.workbuddy_connecting.is_none()&&self.quota.deepseek_connecting.is_none()&&self.quota.gemini_connecting.is_none()&&self.quota.doubao_connecting.is_none()&&self.quota.login.is_none(),|ui|{
                settings_control_row(ui,"服务",56.0,|ui|{egui::ComboBox::from_id_salt("quota-provider").selected_text(connection_name(group)).show_ui(ui,|ui|{
                    for p in Provider::ALL.into_iter().filter(|p|connection_group(*p)==*p){ui.selectable_value(&mut group,p,connection_name(p));}
                }).response});
            });
            if group!=connection_group(previous_provider){self.quota.provider=group;}
            ui.add_enabled_ui(self.quota.connect_target.is_none()&&self.quota.api_connecting.is_none()&&self.quota.kiro_connecting.is_none(),|ui|{
                if group==Provider::ZCode{
                    let mut use_key=self.quota.provider==Provider::Glm;
                    ui.horizontal(|ui|{ui.label("连接方式");ui.selectable_value(&mut use_key,false,"本机 ZCode");ui.selectable_value(&mut use_key,true,"套餐 API Key");});
                    self.quota.provider=if use_key{Provider::Glm}else{Provider::ZCode};
                }else if group==Provider::NewApi{
                    ui.collapsing("高级设置",|ui|{
                        settings_control_row(ui,"站点类型",76.0,|ui|{egui::ComboBox::from_id_salt("relay-protocol").selected_text(if self.quota.relay_manual{self.quota.provider.name()}else{"自动识别（推荐）"}).show_ui(ui,|ui|{
                            ui.selectable_value(&mut self.quota.relay_manual,false,"自动识别（推荐）");
                            for p in [Provider::NewApi,Provider::OneApi,Provider::Sub2Api]{if ui.selectable_label(self.quota.relay_manual&&self.quota.provider==p,p.name()).clicked(){self.quota.relay_manual=true;self.quota.provider=p;}}
                        }).response});
                    });
                }
            });
            if previous_provider!=self.quota.provider{
                self.quota.doubao_cookie.clear();
                self.quota.api_key.clear();self.quota.deepseek_key.clear();self.quota.note.clear();
                if !(previous_provider.relay()&&self.quota.provider.relay()){self.quota.source.clear();}
            }
            settings_control_row(ui,"备注",56.0,|ui|{ui.add_sized([ui.available_width(),27.0],egui::TextEdit::singleline(&mut self.quota.alias).vertical_align(egui::Align::Center).hint_text("例如：个人、工作、备用"))});
            if self.quota.provider.api_key(){
                let provider=self.quota.provider;
                ui.label("仅查询额度；API Key 保存在 Windows 凭据管理器，不写入配置或缓存。");
                ui.add_enabled_ui(self.quota.api_connecting.is_none(),|ui|{
                    if provider.relay(){
                        if self.quota.connect_target.is_some(){ui.label(format!("使用已保存的 {} 接口",provider.name()));}
                        else if !self.quota.relay_manual{ui.label("自动识别站点类型");}
                        ui.label("填写站点网站地址和 API Key，读取站点提供的额度。");
                        ui.add_sized([ui.available_width(),27.0],egui::TextEdit::singleline(&mut self.quota.source).hint_text("https://你的站点地址（可含子目录）"));
                    }else if matches!(provider,Provider::MiniMax|Provider::Glm){
                        if !matches!(self.quota.source.as_str(),"cn"|"global"){self.quota.source="cn".into();}
                        ui.horizontal(|ui|{
                            ui.label("账号地区");ui.selectable_value(&mut self.quota.source,"cn".into(),"中国区");ui.selectable_value(&mut self.quota.source,"global".into(),"国际区");
                            if provider==Provider::Glm&&ui.small_button("获取密钥").on_hover_text(if self.quota.source=="global"{"打开国际区控制台，选择 Personal Coding Plan"}else{"打开中国区个人编程套餐页，获取套餐 Key"}).clicked(){
                                let url=if self.quota.source=="global"{"https://z.ai/manage-apikey/apikey-list"}else{"https://bigmodel.cn/coding-plan/personal/overview"};
                                self.pending.push(Cmd::OpenUrl(url.into()));
                            }
                        });
                        ui.label(if provider==Provider::Glm{"请使用 Coding Plan 套餐 Key，非标准 API Key；本机 ZCode 登录无需填写密钥。"}else{"读取 MiniMax Coding Plan；按量 API 余额暂不包含。"});
                    }else{ui.label("查询 OpenCode Go 套餐使用率，不查询 Zen 按量余额。");}
                    ui.add_sized([ui.available_width(),27.0],egui::TextEdit::singleline(&mut self.quota.api_key).password(true).hint_text("粘贴 API Key"));
                    if ui.add_enabled(!self.quota.api_key.trim().is_empty()&&(!provider.relay()||!self.quota.source.trim().is_empty()),egui::Button::new("验证并连接")).clicked(){
                        self.quota.note.clear();let source=self.quota.source.trim().into();let alias=self.quota.alias.clone();let key=std::mem::take(&mut self.quota.api_key);
                        self.quota.api_connecting=Some(if provider.relay()&&self.quota.connect_target.is_none()&&!self.quota.relay_manual{quota::api::Connect::start_auto(source,alias,key)}else{quota::api::Connect::start(provider,source,alias,key)});
                    }
                });
                if self.quota.api_connecting.is_some(){ui.horizontal(|ui|{ui.spinner();ui.label("正在验证额度…");if ui.button("取消").clicked(){self.quota.api_connecting=None;self.quota.note="已取消，未保存密钥".into();}});ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));}
            }else if self.quota.provider==Provider::WorkBuddy{
                ui.label("连接官网个人账号，查看剩余积分。");
                ui.horizontal(|ui|{
                    if ui.button("打开官网用量页").clicked(){self.pending.push(Cmd::OpenUrl("https://www.codebuddy.cn/profile/usage".into()));}
                    if ui.add_enabled(self.quota.workbuddy_connecting.is_none(),egui::Button::new("登录并连接")).clicked(){
                        let expected=self.quota.connect_target.as_ref().and_then(|id|self.quota.config.accounts.iter().find(|a|&a.id==id)).and_then(|a|a.slot.clone());self.quota.note.clear();self.quota.workbuddy_connecting=Some(quota::workbuddy::Connect::start(self.quota.alias.clone(),expected));
                    }
                });
                ui.label(egui::RichText::new("自动识别本机登录；需要授权时打开默认浏览器。登录状态加密保存在本机。").size(12.0).color(ui.visuals().weak_text_color()));
                if self.quota.workbuddy_connecting.is_some(){ui.horizontal(|ui|{ui.spinner();ui.label("请在默认浏览器完成官网登录…");if ui.button("取消").clicked(){self.quota.workbuddy_connecting=None;self.quota.note="已取消 WorkBuddy 连接".into();}});ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));}
            }else if self.quota.provider==Provider::DeepSeek{
                ui.label("查询 API 账户余额，密钥仅保存在本机。");
                ui.add_sized([ui.available_width(),27.0],egui::TextEdit::singleline(&mut self.quota.deepseek_key).password(true).hint_text("粘贴 API Key"));
                if ui.add_enabled(self.quota.deepseek_connecting.is_none()&&!self.quota.deepseek_key.trim().is_empty(),egui::Button::new("验证并连接")).clicked(){self.quota.note.clear();self.quota.deepseek_connecting=Some(quota::deepseek::Connect::start(self.quota.alias.clone(),std::mem::take(&mut self.quota.deepseek_key)));}
                if self.quota.deepseek_connecting.is_some(){ui.spinner();ui.label("正在连接 DeepSeek…");ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));}
            }else if self.quota.provider==Provider::Gemini{
                // 本机 Antigravity 登录态：读的是同一份 Google AI Pro 订阅额度，不用碰浏览器。
                let detected=self.quota.antigravity_email.clone();
                ui.label(egui::RichText::new(match &detected{Some(email)=>format!("本机 Antigravity 已登录 {email}"),None=>"未发现本机 Antigravity 登录态".into()}).size(12.0).color(if detected.is_some(){ui.visuals().weak_text_color()}else{Color32::from_rgb(226,164,44)}));
                ui.horizontal_wrapped(|ui|{
                    if ui.add_enabled(self.quota.antigravity_connecting.is_none(),egui::Button::new("用本机 Antigravity 登录态")).clicked(){self.quota.note.clear();self.quota.antigravity_connecting=Some(quota::antigravity::Connect::start(self.quota.alias.clone()));}
                    if ui.add_enabled(self.quota.antigravity_connecting.is_none(),egui::Button::new("重新检测")).clicked(){self.quota.antigravity_email=quota::antigravity::available();}
                });
                ui.add(egui::Label::new(egui::RichText::new("读 5 小时额度，外加 Gemini 每周与 Gemini 4 Argon 每周：周额度要跑一次本机 Antigravity CLI（约 10 秒）。").size(11.0).color(ui.visuals().weak_text_color())).wrap());
                if self.quota.antigravity_connecting.is_some(){ui.horizontal(|ui|{ui.spinner();ui.label("正在读取本机 Antigravity 额度…");if ui.button("取消").clicked(){self.quota.antigravity_connecting=None;self.quota.note="已取消 Antigravity 连接".into();}});ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));}
                ui.separator();
                ui.label(egui::RichText::new("或者按网页会话连接，读取 Gemini 网页的 5 小时与每周额度。").size(12.0).color(crate::settings_theme::color(ui.ctx(),Color32::from_gray(180),Color32::from_gray(75))));
                ui.horizontal_wrapped(|ui|{
                    if ui.button("打开默认浏览器登录").clicked(){self.pending.push(Cmd::OpenUrl("https://gemini.google.com/usage".into()));}
                    if ui.add_enabled(self.quota.gemini_connecting.is_none(),egui::Button::new("连接已登录的 Chrome")).clicked(){self.quota.note.clear();self.quota.gemini_connecting=Some(quota::gemini::Connect::start(self.quota.alias.clone(),self.quota.gemini_email.clone(),None,self.quota.connect_target.as_ref().and_then(|id|self.quota.config.accounts.iter().find(|a|&a.id==id)).and_then(|a|a.slot.clone())));}
                });
                ui.label(egui::RichText::new("使用已登录的 Chrome 144+，允许连接后验证额度，无需扩展。").size(12.0).color(ui.visuals().weak_text_color()));
                ui.horizontal_wrapped(|ui|{ui.label("首次在 Chrome 开启原生连接：");ui.code("chrome://inspect/#remote-debugging");if ui.small_button("复制地址").clicked(){ui.ctx().copy_text("chrome://inspect/#remote-debugging".into());}});
                ui.add(egui::Label::new(egui::RichText::new("Chrome 会请求浏览器会话访问权限。岛仔仅在连接时读取 Gemini 登录，完成后断开，后续使用本机加密会话刷新。").size(11.0).color(ui.visuals().weak_text_color())).wrap());
                settings_control_row(ui,"Google 邮箱",80.0,|ui|{ui.add_sized([ui.available_width(),27.0],egui::TextEdit::singleline(&mut self.quota.gemini_email).vertical_align(egui::Align::Center).hint_text("可选：指定要连接的 Google 邮箱"))});
                ui.collapsing("高级：手动 Cookie 连接",|ui|{
                    ui.label("在 Gemini 用量页按 F12，打开网络，刷新后找到 executeQuery 请求，复制 Cookie 请求头。");
                    ui.add_sized([ui.available_width(),27.0],egui::TextEdit::singleline(&mut self.quota.gemini_cookie).vertical_align(egui::Align::Center).password(true).hint_text("粘贴 Cookie 请求头"));
                    if ui.add_enabled(self.quota.gemini_connecting.is_none() && !self.quota.gemini_cookie.trim().is_empty()&&!self.quota.gemini_email.trim().is_empty(),egui::Button::new("验证并连接")).clicked(){self.quota.note.clear();self.quota.gemini_connecting=Some(quota::gemini::Connect::start(self.quota.alias.clone(),self.quota.gemini_email.clone(),Some(std::mem::take(&mut self.quota.gemini_cookie)),self.quota.connect_target.as_ref().and_then(|id|self.quota.config.accounts.iter().find(|a|&a.id==id)).and_then(|a|a.slot.clone())));}
                });
                if self.quota.gemini_connecting.is_some(){ui.horizontal(|ui|{ui.spinner();ui.label("正在连接并验证 Gemini 额度…");if ui.button("取消").clicked(){self.quota.gemini_connecting=None;self.quota.note="已取消 Gemini 连接".into();}});ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));}
            }else if self.quota.provider==Provider::Doubao{
                ui.label("查询豆包网页会员的 5 小时、每周及活动额度。");
                let expected=self.quota.connect_target.as_ref().and_then(|id|self.quota.config.accounts.iter().find(|account|&account.id==id)).and_then(|account|account.slot.clone());
                ui.horizontal(|ui|{
                    if ui.add_enabled(self.quota.doubao_connecting.is_none(),egui::Button::new("浏览器登录")).clicked(){
                        self.quota.note.clear();self.quota.doubao_connecting=Some(quota::doubao::Connect::start(self.quota.alias.clone(),quota::doubao::LoginSource::Browser,expected.clone()));
                    }
                    if ui.add_enabled(self.quota.doubao_connecting.is_none(),egui::Button::new("从本机导入")).clicked(){
                        self.quota.note.clear();self.quota.doubao_connecting=Some(quota::doubao::Connect::start(self.quota.alias.clone(),quota::doubao::LoginSource::Local,expected.clone()));
                    }
                    if ui.button("打开豆包额度").clicked(){self.pending.push(Cmd::OpenUrl("https://www.doubao.com/member/quota-management".into()));}
                });
                ui.add(egui::Label::new("无需浏览器扩展，在独立窗口完成豆包官网登录。也可导入本机豆包客户端当前账号；登录状态加密保存在本机。").wrap());
                ui.collapsing("使用 Cookie 连接",|ui|{
                    ui.label("在豆包网页按 F12，打开网络，复制额度请求的完整 Cookie 请求头。");
                    ui.add_sized([ui.available_width(),27.0],egui::TextEdit::singleline(&mut self.quota.doubao_cookie).password(true).hint_text("粘贴 Cookie 请求头"));
                    if ui.add_enabled(self.quota.doubao_connecting.is_none()&&!self.quota.doubao_cookie.trim().is_empty(),egui::Button::new("验证并连接")).clicked(){
                        self.quota.note.clear();self.quota.doubao_connecting=Some(quota::doubao::Connect::start(self.quota.alias.clone(),quota::doubao::LoginSource::Cookie(std::mem::take(&mut self.quota.doubao_cookie)),expected.clone()));
                    }
                });
                if self.quota.doubao_connecting.is_some(){
                    ui.horizontal(|ui|{ui.spinner();ui.label("正在连接豆包…");if ui.button("取消").clicked(){self.quota.doubao_connecting=None;self.quota.doubao_cookie.clear();self.quota.note="已取消，未保存豆包登录".into();}});
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                }
            }else if self.quota.provider==Provider::GrokBot{
                ui.horizontal(|ui|{
                    if ui.add_enabled(self.quota.login.is_none(),egui::Button::new("浏览器登录")).clicked(){self.quota_login(self.quota.alias.clone());}
                    if ui.button("从本机导入").clicked(){self.quota.note=match quota::grok::native_path(){Some(path)=>match self.quota.config.import_grok(&path,&self.quota.alias){Ok(n)=>format!("新增 {n} 个 Grok Bot 账号"),Err(e)=>e},None=>"请先安装 Grok Bot 并登录，或使用浏览器连接".into()};}
                });
                ui.collapsing("导入其他登录文件",|ui|{self.quota_file_form(ui);});
                ui.label(egui::RichText::new("使用 Grok Bot 对应的 Cursor 账号登录，浏览器授权支持自动续期。").size(12.0).color(crate::settings_theme::color(ui.ctx(),Color32::from_gray(180),Color32::from_gray(75))));
            }else if self.quota.provider==Provider::ZCode{
                if ui.button("连接本机 ZCode").clicked(){self.quota.note=match quota::zcode::path().ok_or_else(||"未找到 ZCode 登录目录".to_owned()).and_then(|p|quota::zcode::add(&mut self.quota.config,&p,&self.quota.alias)){Ok(())=>"ZCode 已连接".into(),Err(e)=>e};}
            }else if self.quota.provider==Provider::Kiro{
                if ui.add_enabled(self.quota.kiro_connecting.is_none(),egui::Button::new("连接本机 Kiro")).clicked(){match quota::kiro::path(){Some(path)=>self.quota_start_kiro(path),None=>self.quota.note="未找到 Kiro 登录目录".into()}}
                ui.collapsing("导入其他登录文件",|ui|{self.quota_file_form(ui);});
                ui.label(egui::RichText::new("只读识别 Kiro 登录缓存，核验用户后显示套餐、有效试用和赠送 / 加购点数；超额上限单独显示。不自动续期，登录过期时请在 Kiro 中重新登录。").size(12.0).color(crate::settings_theme::color(ui.ctx(),Color32::from_gray(180),Color32::from_gray(75))));
            }else if self.quota.provider==Provider::Claude{
                ui.add(egui::Label::new(CLAUDE_LOCAL_NOTICE).wrap());
                ui.horizontal(|ui|{
                    if ui.button("读取桌面版本地记录").clicked(){self.quota.note=match self.quota.config.connect_claude_desktop(&self.quota.alias){Ok(_)=>{self.pending.push(Cmd::QuotaConnected);"已连接，数据随 Claude 桌面版使用更新".into()},Err(e)=>e};}
                    if ui.button("启用 Code 本地采集").clicked(){self.quota.note=match self.quota.config.connect_claude_code(&self.quota.alias){Ok(_)=>{self.pending.push(Cmd::QuotaConnected);"本地采集已启用，使用 Claude Code 后更新".into()},Err(e)=>e};}
                });
                for text in ["桌面版自动从本地用量缓存读取重置时间；缓存不可用时显示未知，额度仍以本地记录为准。","Code 本地采集会配置官方状态栏输出，已有状态栏不会被覆盖。仅使用网页版时，暂无可读取的本机记录。"]{ui.add(egui::Label::new(egui::RichText::new(text).size(12.0).color(ui.visuals().weak_text_color())).wrap());}
                ui.collapsing("选择本地用量 JSON",|ui|{self.quota_file_form(ui);});
                claude_disclaimer(ui);
            }else{
                self.quota_file_form(ui);
                ui.label(egui::RichText::new("各账号使用各自的登录文件；移除只取消监控。登录失效时，请在对应工具中重新登录。").size(12.0).color(crate::settings_theme::color(ui.ctx(),Color32::from_gray(180),Color32::from_gray(75))));
            }
            if !self.quota.note.is_empty(){
                ui.add_space(8.0);
                let color=crate::settings_theme::color(ui.ctx(),Color32::from_rgb(112,190,255),Color32::from_rgb(24,94,160));
                egui::Frame::new().fill(color.gamma_multiply(0.12)).stroke(egui::Stroke::new(1.0,color.gamma_multiply(0.4))).corner_radius(8).inner_margin(10.0).show(ui,|ui|{
                    ui.set_min_width((ui.available_width()-1.0).max(0.0));
                    ui.label(egui::RichText::new(format!("●  {}",self.quota.note)).size(13.0).strong().color(color));
                });
            }

                if self.quota.login.is_some(){ui.horizontal(|ui|{ui.spinner();ui.label("等待浏览器登录…");if ui.button("取消登录").clicked(){self.quota.login=None;self.quota.note="登录已取消".into();}});ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));}
                if self.quota.gemini_connecting.is_some(){ui.horizontal(|ui|{ui.spinner();ui.label("正在连接 Gemini…");});ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));}
                if let Some(connect)=&mut self.quota.kiro_connecting{ui.horizontal(|ui|{ui.spinner();ui.label(if connect.cancelled(){"正在结束 Kiro 查询…"}else{"正在核验 Kiro 用户与额度…"});if !connect.cancelled()&&ui.button("取消连接").clicked(){connect.cancel();}});ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));}
            });
        });self.quota.connect_open=open;
        if !open{self.quota_close_connection();}
    }
    fn quota_file_form(&mut self,ui:&mut egui::Ui){
        let local=self.quota.provider==Provider::Claude;
        ui.label(if local{"本地用量 JSON 文件"}else{"登录 JSON 文件"});ui.horizontal(|ui|{ui.add_sized([(ui.available_width()-65.0).max(100.0),27.0],egui::TextEdit::singleline(&mut self.quota.source).vertical_align(egui::Align::Center).hint_text(if local{"选择 Claude 本地用量记录"}else{"选择该账号的登录文件"}));if ui.button("选择…").clicked(){self.pending.push(Cmd::QuotaChooseFile);}});
        if self.quota.provider==Provider::Kiro{
            if ui.add_enabled(self.quota.kiro_connecting.is_none()&&!self.quota.source.trim().is_empty(),egui::Button::new("验证并连接")).clicked(){self.quota_start_kiro(self.quota.source.trim().into());}
        }else if ui.button("添加账号").clicked(){match self.quota.config.add(self.quota.provider,self.quota.alias.clone(),self.quota.source.trim().into()){Ok(())=>{self.quota.alias.clear();self.quota.source.clear();self.quota.note="账号已添加".into();},Err(e)=>self.quota.note=e}}
    }
    #[cfg(feature="record")]
    pub fn quota_demo(&mut self,mode:&str){
        use serde_json::json;
        self.quota=State::default();let reset=chrono::Utc::now().timestamp()+8200;
        for(i,(provider,alias))in [(Provider::Codex,"个人"),(Provider::Codex,"工作"),(Provider::Claude,"Claude")].into_iter().enumerate(){let id=format!("demo-{i}");self.quota.config.accounts.push(Account{id:id.clone(),provider,alias:alias.into(),source:format!("demo-{i}.json").into(),slot:None,enabled:true,pinned:mode=="multi" || i==0});let payload=if provider==Provider::Codex{json!({"plan_type":if i==0{"pro"}else{"plus"},"rate_limit":{"primary_window":if mode=="single" && i==0{serde_json::Value::Null}else{json!({"used_percent":28+i*15,"limit_window_seconds":18000,"reset_at":reset})},"secondary_window":{"used_percent":56,"limit_window_seconds":604800,"reset_at":reset+200000}}})}else{json!({"five_hour":{"utilization":45,"resets_at":"2030-01-01T12:00:00Z"},"seven_day":{"utilization":27}})};self.quota.rows.insert(id,quota::parse(provider,&payload));}
        self.quota.config.selected="demo-0".into();self.show_page(6);if self.settings_open{self.settings_tab=11;}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore="按需导出 27 像素常驻小环的实际渲染"]
    fn island_ring_preview(){
        let dest=std::path::PathBuf::from(std::env::var_os("DAOZAI_ISLAND_RING_PREVIEW").unwrap());std::fs::create_dir_all(&dest).unwrap();
        let account=Account{id:"Gemini-antigravity".into(),provider:Provider::Gemini,alias:"user@example.com".into(),source:"antigravity".into(),slot:Some("antigravity".into()),enabled:true,pinned:true};
        let window=|label:&str,remaining:f32|crate::quota::Window{label:label.into(),seconds:Some(604_800),remaining,reset:Some(4_102_444_800),amount:None};
        let snapshot=Snapshot{windows:vec![window("5 小时",73.0),window("Gemini 每周",49.0),window("Gemini 4 Argon 每周",16.0)],..Default::default()};
        let ctx=egui::Context::default();
        // 默认 8 倍看清结构；传 1 可以看到真机 100% 缩放下的实际像素效果（配合放大查看）。
        let ppp:f32=std::env::var("DAOZAI_ISLAND_RING_PPP").ok().and_then(|v|v.parse().ok()).unwrap_or(8.0);
        ctx.set_pixels_per_point(ppp);
        let size=egui::vec2(80.0,60.0);
        let mut raster=crate::preview_raster::Raster::new((size.x*ppp)as usize,(size.y*ppp)as usize);
        for _ in 0..3{
            let output=ctx.run(egui::RawInput{screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,size)),..Default::default()},|ctx|{
                egui::CentralPanel::default().frame(egui::Frame::new().fill(egui::Color32::from_rgb(24,26,30))).show(ctx,|ui|{
                    let c=Card::new(ui,egui::Pos2::ZERO,ui.max_rect());
                    gemini_ring(&c,Some(&snapshot),40.0,30.0,27.0);let _=&account;
                });
            });
            raster.update_textures(&output.textures_delta);
            let jobs=ctx.tessellate(output.shapes.clone(),output.pixels_per_point);
            raster.buf.fill([0.09,0.10,0.12,1.0]);raster.draw(&jobs,output.pixels_per_point);
            raster.free_textures(&output.textures_delta);
        }
        let mut rgba=Vec::new();raster.to_rgba8(&mut rgba);
        image::save_buffer(dest.join("island-ring.png"),&rgba,(size.x*ppp)as u32,(size.y*ppp)as u32,image::ColorType::Rgba8).unwrap();
    }
    #[test]
    fn kiro_pool_details_include_monthly_reset_and_overage_once(){
        let s=quota::parse(Provider::Kiro,&serde_json::json!({"userInfo":{"userId":"details-user"},"usageBreakdownList":[{"resourceType":"CREDIT","usageLimitWithPrecision":1000,"currentUsageWithPrecision":1080,"currentOveragesWithPrecision":80,"overageCapWithPrecision":200,"nextDateReset":1796083200_i64}],"overageConfiguration":{"overageStatus":"ENABLED"}})).unwrap();
        let details=snapshot_details(&s);
        assert_eq!(details.matches("月度额度").count(),1);
        assert!(details.contains("超额上限余量：120 / 200 点"));
        assert!(details.contains("重置"));
    }
    #[test] fn api_details_include_third_window_tools_and_resets(){
        let w=|label:&str|quota::Window{label:label.into(),seconds:None,remaining:0.0,reset:Some(4102444800),amount:None};
        let snapshot=Snapshot{windows:vec![w("滚动"),w("本周"),w("本月")],extra:vec![w("工具额度")],..Default::default()};
        let detail=snapshot_details(&snapshot);for label in ["滚动","本周","本月","工具额度"]{assert!(detail.contains(label));}assert!(detail.contains("重置"));
    }
    #[test]
    fn claude_local_quota_displays_only_recorded_reset_times(){
        let now=chrono::Local::now();let id="local-fixture".to_string();
        let account=Account{id:id.clone(),provider:Provider::Claude,alias:"离线账号".into(),source:"fixture/config.json".into(),slot:Some("fixture-slot".into()),enabled:true,pinned:true};
        let mut state=State::default();state.config.accounts.push(account.clone());
        let window=quota::Window{label:"5 小时".into(),seconds:Some(18000),remaining:37.5,reset:None,amount:None};
        let snapshot=Snapshot{at:now.timestamp(),windows:vec![window.clone()],..Default::default()};
        state.receive(quota::Event{id:id.clone(),result:Ok(snapshot.clone())});
        assert_eq!(reset_text(window.reset,&now),"重置时间未知");
        assert_eq!(state.rows[&id].as_ref().unwrap().windows[0],window);
        let later=now+chrono::Duration::minutes(11);
        assert_eq!(reset_text(window.reset,&later),"重置时间未知");
        assert_eq!(state.rows[&id].as_ref().unwrap().windows[0].remaining,37.5,"缺少重置时间不能影响额度百分比");
        let actual=quota::Window{reset:Some(now.timestamp()+1200),remaining:20.0,..window.clone()};
        assert_ne!(reset_text(actual.reset,&now),"重置时间未知");
        state.receive(quota::Event{id:id.clone(),result:Ok(Snapshot{windows:vec![actual.clone()],..snapshot})});
        assert_eq!(state.rows[&id].as_ref().unwrap().windows[0],actual,"本地更新仍应正常生效");
    }
    #[test]
    fn claude_local_records_do_not_restore_old_online_snapshots_or_claim_a_new_update(){
        let path=std::env::temp_dir().join(format!("daozai-claude-cache-{}.json",std::process::id()));
        let mut config=Config::default();let id="claude-local".to_string();config.accounts.push(Account{id:id.clone(),provider:Provider::Claude,alias:"Claude".into(),source:"desktop/config.json".into(),slot:Some("fixture".into()),enabled:true,pinned:true});
        let at=chrono::Utc::now().timestamp()-3600;let snapshot=Snapshot{at,windows:vec![quota::Window{label:"5 小时".into(),seconds:Some(18000),remaining:72.0,reset:None,amount:None}],..Default::default()};
        std::fs::write(&path,serde_json::json!({&id:{"provider":"Claude","slot":"fixture","snapshot":snapshot}}).to_string()).unwrap();
        let mut state=State::restore(config.clone(),path.clone());assert!(state.rows.is_empty(),"迁移后不能把旧联网缓存显示为本地数据");
        state.receive(quota::Event{id:id.clone(),result:Ok(snapshot)});assert!(state.stale[&id].contains("本地记录较旧"));
        let restored=State::restore(config,path.clone());assert_eq!(restored.rows[&id].as_ref().unwrap().at,at,"读取缓存不能刷新官方记录时间");std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn claude_verified_history_survives_client_exit_and_island_restart(){
        let path=std::env::temp_dir().join(format!("daozai-claude-exit-{}.json",std::process::id()));let mut config=Config::default();let id="claude-closed".to_string();config.accounts.push(Account{id:id.clone(),provider:Provider::Claude,alias:"Claude".into(),source:"desktop/config.json".into(),slot:Some("verified-owner".into()),enabled:true,pinned:true});
        let at=chrono::Utc::now().timestamp()-60;let snapshot=Snapshot{at,windows:vec![quota::Window{label:"本周".into(),seconds:Some(604800),remaining:16.0,reset:Some(at+86400),amount:None}],..Default::default()};
        let mut state=State::restore(config.clone(),path.clone());state.receive(quota::Event{id:id.clone(),result:Ok(snapshot.clone())});state.receive(quota::Event{id:id.clone(),result:Err("客户端已关闭，等待新记录".into())});assert_eq!(state.rows[&id].as_ref().unwrap().windows,snapshot.windows);assert!(state.stale.contains_key(&id));
        let mut restarted=State::restore(config,path.clone());assert_eq!(restarted.rows[&id].as_ref().unwrap().windows,snapshot.windows);restarted.receive(quota::Event{id:id.clone(),result:Err("等待新记录".into())});assert_eq!(restarted.rows[&id].as_ref().unwrap().windows,snapshot.windows);assert_eq!(restarted.rows[&id].as_ref().unwrap().at,at);std::fs::remove_file(path).unwrap();
    }
    #[test] fn side_amounts_stay_short(){
        for (value,symbol,expected) in [(4.18,"¥","¥4.1"),(999.9,"¥","¥999"),(9999.0,"¥","¥9.99k"),(28560.0,"","2.85万"),(123456.7,"¥","¥12.3万"),
            (1234567.8,"¥","¥123万"),(99999999.0,"¥","¥0.99亿"),(12345678901.0,"","123亿"),(-12.5,"¥","-¥12.5"),(0.0,"","0"),(0.04,"¥","¥0")] {
            assert_eq!(side_amount(value,symbol),expected,"{value}");
        }
        assert!(side_amount(f64::NAN,"¥").contains("--"));
    }
    #[test] fn deepseek_missing_balance_distinguishes_loading_and_failure(){
        let mut state=State::default();assert!(state.balance_loading("test"));assert!(state.balance_text("test").is_none());
        state.rows.insert("test".into(),Err("无法连接".into()));assert!(!state.balance_loading("test"));assert!(state.balance_text("test").is_none());
        state.busy.insert("test".into());assert!(state.balance_loading("test"));
    }
    #[test] fn zcode_badge_has_clear_corners_and_a_visible_edge(){
        let icon=egui_extras::image::load_svg_bytes_with_size(include_bytes!("../assets/ai-icons/zcode.svg"),Some(egui::load::SizeHint::Size(26,26))).unwrap();
        for (x,y) in [(3,3),(22,3),(3,22),(22,22)] {assert_eq!(icon[(x,y)].a(),0);}
        assert!(icon[(13,1)].a()>180);
        assert!(icon.pixels.iter().filter(|p|p.r()>230&&p.a()>240).count()>35);
    }
    #[test] fn relay_key_icons_remain_visible_in_light_skins(){
        let ctx=egui::Context::default();
        for provider in [Provider::NewApi,Provider::OneApi,Provider::Sub2Api]{
            let texture=logo_texture(&ctx,provider,24).unwrap();
            for palette in [crate::skin_paint::CREAM,crate::skin_paint::GLASS_LIGHT]{
                let mut mesh=egui::Mesh::with_texture(texture.id());
                mesh.add_rect_with_uv(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(24.0,24.0)),egui::Rect::from_min_max(egui::Pos2::ZERO,egui::pos2(1.0,1.0)),Color32::WHITE);
                let mut shape=egui::Shape::mesh(mesh);crate::skin_paint::recolor(&mut shape,&palette);
                if let egui::Shape::Mesh(mesh)=shape{assert!(mesh.vertices.iter().all(|v|v.color.r()<80),"浅色皮肤上的钥匙应跟随文字变深");}else{panic!("应绘制钥匙纹理");}
            }
        }
    }
    #[test] fn deepseek_badge_keeps_brand_blue_in_every_skin(){
        use crate::model::Skin;
        for skin in [Skin::Classic,Skin::Cream,Skin::GlassLight,Skin::GlassDark]{
            let image=egui_extras::image::load_svg_bytes_with_size(deepseek_badge_svg(skin).as_bytes(),Some(egui::load::SizeHint::Size(100,100))).unwrap();
            assert!(image.pixels.iter().any(|p|*p==Color32::from_rgb(77,107,254)));
            let id=egui::TextureId::User(0xDEE5);crate::skin_paint::mark_monochrome(id);crate::skin_paint::mark_colored(id);
            let mut mesh=egui::Mesh::with_texture(id);mesh.add_rect_with_uv(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(24.0,24.0)),egui::Rect::from_min_max(egui::Pos2::ZERO,egui::pos2(1.0,1.0)),Color32::WHITE);
            let mut shape=egui::Shape::mesh(mesh);crate::skin_paint::recolor(&mut shape,&crate::skin_paint::CREAM);
            if let egui::Shape::Mesh(mesh)=shape{assert!(mesh.vertices.iter().all(|v|v.color==Color32::WHITE));}else{panic!("徽标应保留纹理");}
        }
    }
    #[test]
    #[ignore="只读验证本机已启用账号冷启动时可恢复额度"]
    fn quota_live_cold_start(){
        let state=State::restore(Config::load(),crate::model::config_dir().join("ai-quota-cache.json"));
        let enabled:Vec<_>=state.config.accounts.iter().filter(|a|a.enabled).collect();
        assert!(!enabled.is_empty());
        for a in enabled {
            assert!(matches!(state.rows.get(&a.id),Some(Ok(_))),"{} 缺少上次记录",a.provider.name());
            assert!(state.stale.contains_key(&a.id));
            println!("{} 已从磁盘恢复",a.provider.name());
        }
    }
    #[test] fn restart_restores_last_balance_only_for_the_bound_account(){
        let path=std::env::temp_dir().join(format!("daozai-quota-restart-{}.json",std::process::id()));
        let mut config=Config::default();
        let id=config.connect_workbuddy("fixture".into(),"测试账号".into());
        let mut state=State::restore(config.clone(),path.clone());
        let snapshot=quota::workbuddy::parse(&serde_json::json!({"code":0,"data":{"Packages":[{"CycleRemainCapacity":2000}]}})).unwrap();
        state.receive(quota::Event{id:id.clone(),result:Ok(snapshot)});
        let mut restored=State::restore(config.clone(),path.clone());
        assert_eq!(restored.balance_text(&id).as_deref(),Some("2k"));
        assert!(restored.stale.contains_key(&id));
        restored.receive(quota::Event{id:id.clone(),result:Err("浏览器未启动".into())});
        assert_eq!(restored.balance_text(&id).as_deref(),Some("2k"));
        config.accounts[0].slot=Some("不同账号".into());
        assert!(State::restore(config,path.clone()).rows.is_empty());
        std::fs::remove_file(path).unwrap();
    }
    #[test] fn deepseek_refresh_failure_keeps_balance_until_success(){
        let mut state=State::default();let id=state.config.connect_deepseek("test".into(),"DeepSeek".into());
        let snapshot=quota::deepseek::parse(&serde_json::json!({"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":"12800.00","granted_balance":"800","topped_up_balance":"12000"}]})).unwrap();
        state.receive(quota::Event{id:id.clone(),result:Ok(snapshot.clone())});state.receive(quota::Event{id:id.clone(),result:Err("连接失败".into())});
        assert_eq!(state.rows[&id].as_ref().unwrap().wallet.as_ref().unwrap().primary().total,12800.0);assert!(state.stale.contains_key(&id));
        state.receive(quota::Event{id:id.clone(),result:Ok(snapshot)});assert!(!state.stale.contains_key(&id));
    }

    use chrono::TimeZone;
    #[test]
    fn quota_logo_matches_screen_pixels_at_each_display_scale(){
        for scale in [1.0,1.25,1.5,1.75,2.0] {
            let ctx=egui::Context::default();egui_extras::install_image_loaders(&ctx);
            for provider in Provider::ALL {for size in [10.0,12.0,20.0,24.0] {
                let raw=egui::RawInput{screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(120.0,80.0))),viewports:std::iter::once((egui::ViewportId::ROOT,egui::ViewportInfo{native_pixels_per_point:Some(scale),..Default::default()})).collect(),..Default::default()};
                let output=ctx.run(raw,|ctx|{egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx,|ui|{
                    let c=Card::new(ui,egui::pos2(0.37,0.21),ui.max_rect());logo(&c,provider,32.25,32.5,size);
                });});
                let mesh=output.shapes.iter().find_map(|s|match &s.shape{egui::Shape::Mesh(mesh)=>Some(mesh),_=>None}).expect("应绘制厂商图标");
                let image=&output.textures_delta.set.iter().find(|(id,_)|*id==mesh.texture_id).expect("不同尺寸应各自生成纹理").1.image;
                let pixels=(size*scale).round() as usize;
                assert_eq!(image.size(),[pixels,pixels],"图标应按屏幕像素生成，厂商={}，缩放={scale}",provider.name());
                for v in &mesh.vertices {
                    assert!((v.pos.x*scale-(v.pos.x*scale).round()).abs()<0.001 && (v.pos.y*scale-(v.pos.y*scale).round()).abs()<0.001,"图标边界应对齐物理像素");
                }
                if let egui::ImageData::Color(image)=image {
                    if provider==Provider::Doubao{
                        assert!(image.pixels.iter().any(|pixel|pixel.a()>150&&pixel.b()>pixel.r().saturating_add(20)),"豆包头像应保留官方原始配色");
                        assert!(image.pixels.iter().any(|pixel|pixel.a()==0),"豆包头像应保留透明底");
                    }else if provider==Provider::MiniMax{
                        let original=egui_extras::image::load_svg_bytes_with_size(include_bytes!("../assets/ai-icons/minimax.svg"),Some(egui::load::SizeHint::Size(pixels as u32,pixels as u32))).unwrap();
                        assert_eq!(image.pixels,original.pixels,"MiniMax 应保留品牌颜色和透明边缘");
                    }else{for pixel in &image.pixels {let expected=if keeps_brand_color(provider){*pixel}else if provider==Provider::DeepSeek{Color32::from_rgba_unmultiplied(77,107,254,pixel.a())}else{Color32::from_white_alpha(pixel.a())};assert_eq!(*pixel,expected,"标识颜色与透明边缘应保持一致");}}
                }
            }}
        }
    }
    #[test]
    fn quota_reset_uses_local_calendar_dates_and_clock_times(){
        let now=chrono::Local.with_ymd_and_hms(2026,10,31,22,0,0).unwrap();
        for (parts,expected) in [
            ((2026,10,31,23,30),"今天 23:30 重置"),
            ((2026,11,1,9,0),"11月1日 09:00 重置"),
            ((2026,11,3,9,0),"11月3日 09:00 重置"),
            ((2027,1,1,9,0),"2027/1/1 09:00 重置"),
        ] {
            let (year,month,day,hour,minute)=parts;
            let at=chrono::Local.with_ymd_and_hms(year,month,day,hour,minute,0).unwrap().timestamp();
            assert_eq!(reset_text(Some(at),&now),expected);
        }
    }
    #[test]
    fn quota_reset_does_not_invent_missing_or_expired_times(){
        let now=chrono::Local.with_ymd_and_hms(2026,10,31,22,0,0).unwrap();
        assert_eq!(reset_text(None,&now),"重置时间未知");
        assert_eq!(reset_text(Some(i64::MAX),&now),"重置时间未知");
        assert_eq!(reset_text(Some(now.timestamp()),&now),"等待刷新");
        assert_eq!(reset_text(Some(now.timestamp()-1),&now),"等待刷新");
    }
    #[test]
    fn island_sized_ring_still_draws_three_arcs(){
        let account=Account{id:"Gemini-antigravity".into(),provider:Provider::Gemini,alias:"user@example.com".into(),source:"antigravity".into(),slot:Some("antigravity".into()),enabled:true,pinned:true};
        let window=|label:&str,remaining:f32|crate::quota::Window{label:label.into(),seconds:Some(604_800),remaining,reset:None,amount:None};
        let snapshot=Snapshot{windows:vec![window("5 小时",88.0),window("Gemini 每周",56.0),window("Gemini 4 Argon 每周",28.0)],..Default::default()};
        let ctx=egui::Context::default();
        let output=ctx.run(egui::RawInput{screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(90.0,90.0))),..Default::default()},|ctx|{
            egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx,|ui|{
                let c=Card::new(ui,egui::Pos2::ZERO,ui.max_rect());
                gemini_ring(&c,Some(&snapshot),45.0,45.0,27.0);
            });
        });
        // 27 像素改用描边弧（Mesh 硬边在这个尺寸会露锯齿），所以按「不同半径的弧」数圈。
        let mut radii:Vec<f32>=output.shapes.iter().filter_map(|shape|match &shape.shape{
            egui::Shape::Path(path) if !path.points.is_empty()=>{
                let r=path.points.iter().map(|p|((p.x-45.0).powi(2)+(p.y-45.0).powi(2)).sqrt()).sum::<f32>()/path.points.len() as f32;
                Some((r*4.0).round()/4.0)
            }
            _=>None,
        }).collect();
        radii.sort_by(f32::total_cmp);radii.dedup();
        let arcs=radii.len();
        assert!(arcs>=3,"27 像素的常驻环也要画三圈，实际弧 {arcs}");
    }
    #[test]
    fn island_ring_geometry_keeps_three_visible_bands(){
        let(width,gap,radii)=crate::quota_visual::ring_layout(27.0,3,true);
        assert_eq!(radii.len(),3);
        assert!(width>=1.0&&gap>0.0);
        for pair in radii.windows(2){assert!(pair[0]-pair[1]>width,"相邻两圈之间要留出肉眼可见的间隙");}
        // 中央星不能压到最内圈：留出的空档至少半个环宽。
        assert!(radii[2]-width/2.0>radii[2]*0.66,"星半径不能顶到内圈");
    }
    #[test]
    fn quota_countdown_rounds_up_and_never_shows_negatives(){        assert_eq!(countdown_label(30.0).as_deref(),Some("30s 后刷新"));
        // 还剩 0.2 秒也算 1 秒：显示 0s 会让人以为已经刷新过了。
        assert_eq!(countdown_label(0.2).as_deref(),Some("1s 后刷新"));
        assert_eq!(countdown_label(0.0).as_deref(),Some("0s 后刷新"));
        assert_eq!(countdown_label(-3.5).as_deref(),Some("0s 后刷新"));
        assert_eq!(countdown_label(f32::NAN),None);
    }
    #[test]
    fn auto_renew_starts_ten_minutes_before_expiry_at_most_once_a_minute(){
        let now=1_760_000_000;
        let login=|expires|quota::antigravity::Login{email:"a@b.c".into(),expires_at:expires};
        assert!(!renew_due(None,None,now),"没登录态不续期");
        assert!(!renew_due(Some(&login(None)),None,now),"读不到到期时间就不猜");
        assert!(!renew_due(Some(&login(Some(now+3600))),None,now),"离到期还早不必惊动");
        assert!(renew_due(Some(&login(Some(now+300))),None,now),"10 分钟内该动手");
        assert!(renew_due(Some(&login(Some(now-30))),None,now),"已经过期更要续");
        assert!(!renew_due(Some(&login(Some(now+300))),Some(std::time::Instant::now()),now),"一分钟内不重复打 OAuth");
    }
    #[test]
    fn renewed_expiry_survives_a_keyring_reread(){
        // 系统凭据里还是旧时间，内存里刚续的必须留着，否则界面又跳回「已过期」并反复续期。
        let mut login=Some(quota::antigravity::Login{email:"a@b.c".into(),expires_at:Some(100)});
        keep_freshest(&mut login,Some(200));
        assert_eq!(login.as_ref().and_then(|login|login.expires_at),Some(200));
        keep_freshest(&mut login,Some(50));
        assert_eq!(login.as_ref().and_then(|login|login.expires_at),Some(200),"旧值不能把内存令牌时间顶回去");
        keep_freshest(&mut None,Some(300));
    }
    #[test]
    fn auto_renew_fires_once_then_waits_a_minute(){
        let mut state=State::default();
        let now=1_760_000_000;
        state.antigravity_login=Some(quota::antigravity::Login{email:"a@b.c".into(),expires_at:Some(now+120)});
        assert!(state.auto_renew_if_due(now),"令牌两分钟后到期，应该自动续");
        assert!(state.renew_auto,"自动续期要能区分出来，提示文案不一样");
        assert!(!state.auto_renew_if_due(now),"一分钟内不重复打 OAuth");
        let mut state=State::default();
        state.antigravity_login=Some(quota::antigravity::Login{email:"a@b.c".into(),expires_at:Some(now+7200)});
        assert!(!state.auto_renew_if_due(now),"还有两小时就别打扰");
    }
}

// ---------------------------------------------------------------- 左右停靠：竖向额度
// 侧边停靠时额度竖排在胶囊里：环形额度一个圆环一行；余额类（DeepSeek、WorkBuddy）图标在上、金额在下。
pub(crate) const SIDE_QUOTA_RING_H:f32=30.0;

/// 侧边竖排的金额：4.1、999、9.99k、99.9万、999万、0.12亿……截断不进位，避免显示得比实际多。
pub(crate) fn side_amount(value:f64,symbol:&str)->String{
    if !value.is_finite(){return format!("{symbol}--");}
    let n=value.abs();
    let (mut v,mut unit)=if n>=100_000_000.0{(n/100_000_000.0,"亿")}else if n>=10_000.0{(n/10_000.0,"万")}else if n>=1000.0{(n/1000.0,"k")}else{(n,"")};
    if unit=="万" && v>=1000.0 {v=n/100_000_000.0;unit="亿";}
    let digits=if unit.is_empty(){if v<100.0{1}else{0}}else if v<10.0{2}else if v<100.0{1}else{0};
    let factor=10f64.powi(digits);
    let text=format!("{:.*}",digits as usize,(v*factor).trunc()/factor);
    let text=if text.contains('.'){text.trim_end_matches('0').trim_end_matches('.').to_string()}else{text};
    format!("{}{symbol}{text}{unit}",if value<0.0 && text!="0"{"-"}else{""})
}
pub(crate) const SIDE_QUOTA_BALANCE_H:f32=38.0;
impl App {
    pub(crate) fn quota_side_items(&self)->Vec<(usize,f32)>{
        self.quota_bar_indices().into_iter().map(|i|{
            let balance=self.quota.config.accounts[i].provider.balance();
            (i,if balance{SIDE_QUOTA_BALANCE_H}else{SIDE_QUOTA_RING_H})
        }).collect()
    }

    pub(crate) fn draw_quota_side(&mut self,c:&Card,cx:f32,mid:f32,height:f32,i:usize){
        let Some(a)=self.quota.config.accounts.get(i).cloned() else{return};
        let snapshot=self.quota.rows.get(&a.id).and_then(|r|r.as_ref().ok()).cloned();
        let summary;
        if a.provider.balance(){
            let icon_y=mid-7.0;
            if a.provider!=Provider::DeepSeek{logo(c,a.provider,cx,icon_y,20.0);}else{deepseek_badge(c,self.settings.skin,cx,icon_y,22.0);}
            let loading=self.quota.balance_loading(&a.id);let stale=self.quota.stale.contains_key(&a.id);
            if stale||(!loading&&snapshot.is_none()){
                c.circle(cx+8.0,icon_y+8.0,3.6,Color32::BLACK);
                c.circle(cx+8.0,icon_y+8.0,2.4,Color32::from_rgb(240,165,84));
            }
            if let Some(text)=self.quota.balance_text(&a.id){
                let wallet=snapshot.as_ref().and_then(|s|s.wallet.as_ref());
                let color=if stale{theme::GRAY}else if wallet.is_some_and(|w|!w.available||w.primary().total<=0.0){Color32::from_rgb(255,166,77)}else{theme::WHITE};
                // 竖向胶囊只有 34 点宽：用更短的写法（最多约 4 位有效字符），放不下再缩小字号。
                let short=snapshot.as_ref().and_then(|s|s.wallet.as_ref().map(|w|side_amount(w.primary().total,w.primary().symbol()))
                    .or_else(||s.credits.as_ref().map(|c|side_amount(c.remaining,""))).or_else(||s.relay.as_ref().map(|q|q.compact().split(" · ").next().unwrap_or("未知").to_owned()))).unwrap_or_else(||text.clone());
                let size=[9.5,8.5,7.5].into_iter().find(|&size|c.text_w(&short,size,W::Bold)<=34.0).unwrap_or(7.5);
                let shown=c.fit(&short,size,W::Bold,34.0);
                c.text(cx,mid+15.0,&shown,size,W::Bold,color,H::Center);
                summary=format!("{} {text}",if a.provider.relay(){"额度"}else{"余额"});
            }else if loading{
                let active=(self.t*3.0) as usize%3;
                for dot in 0..3{c.circle(cx-5.0+dot as f32*5.0,mid+11.0,1.5,theme::GRAY.gamma_multiply(if dot==active{1.0}else{0.35}));}
                c.ui.ctx().request_repaint_after(std::time::Duration::from_millis(160));
                summary="正在更新".into();
            }else{summary="余额暂不可用".into();}
        }else{
            if a.provider==Provider::Gemini&&a.source.to_string_lossy()==quota::antigravity::SOURCE{gemini_ring(c,snapshot.as_ref(),cx,mid,26.0);}else{ring(c,&a,snapshot.as_ref(),cx,mid,26.0,ring_colors(&self.quota.config,&a));}
            let mut same=self.quota.config.accounts.iter().filter(|other|other.provider==a.provider && other.enabled);
            if same.clone().count()>1{let n=same.position(|other|other.id==a.id).unwrap_or(0)+1;c.circle(cx+10.0,mid+9.0,4.5,Color32::from_gray(225));c.text(cx+10.0,mid+11.8,&n.to_string(),6.5,W::Bold,Color32::BLACK,H::Center);}
            summary=snapshot.as_ref().map(|s|s.ordered().iter().map(|w|w.summary()).collect::<Vec<_>>().join(" · ")).unwrap_or_else(||if self.quota.busy.contains(&a.id){"正在更新".into()}else{"额度暂不可用".into()});
        }
        let hit=c.hit(cx-17.0,mid-height/2.0,34.0,height,("quota-side",&a.id));
        if bar_tooltip(hit,format!("{} · {}\n{}",a.provider.name(),a.alias,summary)).clicked(){
            self.quota.config.selected=a.id.clone();self.quota.config.save();self.show_page(6);
        }
    }
}
