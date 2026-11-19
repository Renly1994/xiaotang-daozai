//! A request stays tied to its live client connection; replies never enter notification history.
use crate::{agent_notice::Provider, model::Settings, paint::{self, Card, H}, theme::W};
use eframe::egui;
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use std::{collections::VecDeque, io::{BufRead,BufReader,Write}, process::{Child,Command,Stdio}, sync::mpsc, time::{Duration,Instant}};

#[derive(Debug,Clone,PartialEq,Eq,Serialize,Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub interactive:bool, pub handoff:u8, pub completion:bool, pub dwell:u8,
    pub expanded:bool, pub content:u8, pub deferred:bool, pub system_toast:bool,
}

fn notice_action(c:&Card,x:f32,y:f32,w:f32,text:&str,id:&str)->bool{
    let (r,b)=paint::capsule(c,x,y,w,21.0,id,crate::theme::BTN,crate::theme::BTN_HOVER);
    b.centered_text(x+w/2.0,y+10.5,text,10.5,W::Semibold,crate::theme::WHITE,H::Center);
    r.clicked()
}
impl Default for Preferences {
    fn default()->Self {Self{interactive:true,handoff:0,completion:true,dwell:5,expanded:false,content:0,deferred:false,system_toast:false}}
}
impl Preferences {
    pub(crate) fn normalized(mut self)->Self {
        self.dwell=self.dwell.clamp(2,15);self.content=self.content.min(2);
        if ![0,5,10,15,30,60,120].contains(&self.handoff){self.handoff=0;} self
    }
    pub(crate) fn ui(&mut self,ui:&mut egui::Ui){
        use crate::extras::{setting_row,settings_control_row_right,settings_segmented};
        setting_row(ui,"岛上处理审批与提问","关闭时由原客户端处理",&mut self.interactive);
        ui.add_enabled_ui(self.interactive,|ui|{
            settings_control_row_right(ui,"自动交回",|ui|{
                let label=|s:u8|if s==0{"不自动交回".to_owned()}else{format!("{s} 秒后交回")};
                egui::ComboBox::from_id_salt("agent-handoff").selected_text(label(self.handoff)).show_ui(ui,|ui|{
                    for s in [0,5,10,15,30,60,120]{ui.selectable_value(&mut self.handoff,s,label(s));}
                });
            }).on_hover_text("无人操作时交回客户端；查看详情或停留在卡片上时暂停。");
            setting_row(ui,"接收延后提问","DeepSeek Harness 已进入限时模式时，可在岛上补答",&mut self.deferred);
        });
        ui.separator();
        setting_row(ui,"任务完成提醒","待处理请求优先，完成提醒排在其后",&mut self.completion);
        ui.add_enabled_ui(self.completion,|ui|{
            settings_control_row_right(ui,"显示方式",|ui|{settings_segmented(ui,&mut self.expanded,&[(false,"轻提示"),(true,"展开结果卡")],80.0);});
            settings_control_row_right(ui,"完成内容",|ui|{
                egui::ComboBox::from_id_salt("agent-content").selected_text(["仅状态","回复摘要","本轮回复"][self.content as usize]).show_ui(ui,|ui|{
                    for (i,label) in ["仅状态","回复摘要","本轮回复"].iter().enumerate(){ui.selectable_value(&mut self.content,i as u8,*label);}
                });
            });
            settings_control_row_right(ui,"停留时间",|ui|{ui.add(egui::Slider::new(&mut self.dwell,2..=15).suffix(" 秒"));});
            setting_row(ui,"同时发送系统提醒","遵循提醒总方式与声音设置，不显示回复正文",&mut self.system_toast);
        });
    }
}

struct Service { child:Child, out:mpsc::Sender<Value>, incoming:mpsc::Receiver<Value>, policy:Value }
impl Drop for Service {fn drop(&mut self){let _=self.child.kill();let _=self.child.wait();}}
impl Service {
    fn start(ctx:&egui::Context)->Option<Self>{
        let script=crate::model::config_dir().join("agent-notices/bridge/channel.cjs");
        if !std::fs::read(&script).is_ok_and(|b|b==include_bytes!("../integrations/agent-notices/bridge/channel.cjs")){return None;}
        let mut cmd=Command::new(crate::agent_notice::node_runtime()?);
        #[cfg(windows)] {use std::os::windows::process::CommandExt;cmd.creation_flags(0x08000000);}
        let mut child=cmd.arg(script).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
        let mut input=child.stdin.take()?;let output=child.stdout.take()?;
        let (out,rx)=mpsc::channel::<Value>();let (tx,incoming)=mpsc::sync_channel(96);
        std::thread::spawn(move||for v in rx{if writeln!(input,"{v}").is_err(){break;}});
        let ctx=ctx.clone();
        std::thread::spawn(move||{for line in BufReader::new(output).lines().map_while(Result::ok){
            if let Ok(v)=serde_json::from_str(&line){if tx.send(v).is_err(){break;}ctx.request_repaint();}
        }ctx.request_repaint();});
        Some(Self{child,out,incoming,policy:Value::Null})
    }
}

#[derive(Deserialize,Default)]
#[serde(default)]
struct Data {tool:String,reason:String,cwd:String,operation:String,questions:Vec<Question>,text:String,deferred:bool,#[serde(rename="nativeScope")]native_scope:bool,#[serde(rename="sessionId")]session_id:String}
#[derive(Deserialize)]
struct Question {id:String,question:String, #[serde(default)]detail:String, #[serde(default)]multi:bool, #[serde(default)]options:Vec<Choice>}
#[derive(Deserialize)]
struct Choice {label:String,#[serde(default)]description:String}

/// 提问数据的线协议约束（对齐 island-hud 的 questionsOf）。
/// 返回 None 表示通过；Some 表示应交回客户端，避免脏数据拖垮绘制。
fn question_violation(questions:&[Question])->Option<&'static str>{
    if questions.len()>20 {return Some("题量超过 20");}
    let mut ids=std::collections::HashSet::new();
    for q in questions {
        if !ids.insert(q.id.as_str()) {return Some("题目 id 重复");}
        if q.question.chars().count()>20_000 {return Some("题干超过 20000 字符");}
        if q.options.len()>50 {return Some("选项超过 50");}
        let mut labels=std::collections::HashSet::new();
        for o in &q.options {
            if o.label.trim().is_empty() {return Some("选项为空");}
            if !labels.insert(o.label.as_str()) {return Some("选项重复");}
        }
    }
    None
}
#[derive(Default,Serialize)]
struct Answer {id:String,selected:Vec<String>,custom:String}
struct Request {
    id:String,
    provider:Provider,
    question:bool,
    data:Data,
    answers:Vec<Answer>,
    question_index:usize,
    question_scroll:f32,
    shown:Option<f64>,
    details:bool,
}
impl Request {
    fn title(&self)->&str{if self.data.deferred{"可稍后回答"}else if self.question{"需要你回答"}else{"等待你审批"}}
    fn summary(&self)->&str{if self.question{&self.data.questions[0].question}else{&self.data.tool}}
}
#[derive(Default)]
pub struct Cards {
    service:Option<Service>, retry:Option<Instant>, requests:VecDeque<Request>,
    pub(crate) dsh_ready:bool,
    pub(crate) hovering:bool, pub(crate) result:Option<(Provider,String)>,
    /// 最近一次跳转失败的原因，显示在详情卡上，不做静默失败。
    pub(crate) jump_error:Option<String>,
    /// 测试观察用：记录被交回的请求 id，生产构建不编译。
    #[cfg(test)]
    pub(crate) handed_back:Vec<String>,
}
impl Cards {
    pub(crate) fn active(&self)->Option<(&str,Provider,&str,&str)>{
        self.requests.front().map(|r|(r.id.as_str(),r.provider,r.title(),r.summary()))
    }
    fn remove(&mut self,id:&str){self.requests.retain(|r|r.id!=id);}
    fn reply(&mut self,id:&str,reply:Value){
        if let Some(s)=&self.service{let _=s.out.send(json!({"id":id,"reply":reply}));}
        self.remove(id);
    }
    pub(crate) fn hand_back(&mut self,id:&str){
        #[cfg(test)]
        self.handed_back.push(id.to_owned());
        self.reply(id,json!({"decision":"return"}));
    }
    pub(crate) fn detail(&mut self,id:&str){
        if let Some(r)=self.requests.front_mut().filter(|r|r.id==id){r.details=true;}
    }
    pub(crate) fn close_detail(&mut self,id:&str){
        if let Some(r)=self.requests.front_mut().filter(|r|r.id==id){r.details=false;r.shown=None;}
    }
    pub(crate) fn details_open(&self,id:&str)->bool{
        self.requests.front().is_some_and(|r|r.id==id&&r.details)
    }
    /// 解析当前请求的跳转目标；`None` 表示请求已不在队首。
    pub(crate) fn jump_target(&self,id:&str)->Option<crate::agent_jump::Target>{
        let r=self.requests.iter().find(|r|r.id==id)?;
        Some(crate::agent_jump::resolve(r.provider,&r.data.session_id,&r.data.cwd))
    }
    /// 取走一次跳转失败原因，交给上层用岛上提示显示。
    pub(crate) fn take_jump_error(&mut self)->Option<String>{self.jump_error.take()}
    pub(crate) fn editing(&self)->bool{
        self.requests.front().is_some_and(|r|r.details&&r.question)
    }
    pub(crate) fn detail_size(&self,id:&str)->(f32,f32){
        let Some(r)=self.requests.front().filter(|r|r.id==id&&r.details) else{return (360.0,108.0);};
        if !r.question {
            if r.data.native_scope {return (360.0,152.0);}
            // Keep the approval controls fixed at the bottom while allowing the
            // reason and operation preview enough room to breathe on long Codex
            // and shell requests.
            let reason_lines=((r.data.reason.chars().count()+47)/48).min(2);
            let operation_lines=r.data.operation.lines()
                .map(|line|((line.chars().count()+53)/54).max(1))
                .sum::<usize>().clamp(1,4);
            let height=(160.0+reason_lines as f32*13.0+operation_lines as f32*14.0)
                .clamp(214.0,crate::app::WIN_H-8.0);
            return (360.0,height);
        }
        let q=r.data.questions.get(r.question_index.min(r.data.questions.len().saturating_sub(1)));
        let Some(q)=q else{return (360.0,210.0);};
        let question_lines=((q.question.chars().count() as f32/30.0).ceil() as usize).clamp(1,3);
        let detail_lines=((q.detail.chars().count() as f32/36.0).ceil() as usize).min(2);
        let option_step=if q.options.len()>6{20.0}else{25.0};
        let height=(112.0+question_lines as f32*15.0+detail_lines as f32*14.0+
            q.options.len().min(6) as f32*option_step+56.0)
            .clamp(210.0,crate::app::WIN_H-8.0);
        (360.0,height)
    }
    /// Draw an expanded request inside Daozai's existing notice surface.
    pub(crate) fn draw_details(&mut self,c:&Card,id:&str)->bool{
        let height=self.detail_size(id).1;
        // 跳转目标必须在借用请求前算好，避免与 front_mut 的可变借用冲突。
        let jump=self.jump_target(id).filter(|target|target.available());
        let Some(r)=self.requests.front_mut().filter(|r|r.id==id&&r.details) else{return false;};
        let mut action=None;
        let mut jump_clicked=false;
        let ink=egui::Color32::WHITE;
        let muted=paint::alpha(crate::theme::WHITE,0.62);
        crate::extras::agent_icon(c,26.0,18.0,22.0,r.provider);
        // 标题限宽到跳转按钮左侧，长标题不会盖住按钮。
        c.centered_text(44.0,18.0,&c.fit(&format!("{} · {}",r.provider.label(),r.title()),12.0,W::Semibold,206.0),12.0,W::Semibold,ink,H::Left);
        if let Some(target)=&jump {
            if notice_action(c,258.0,9.0,86.0,target.action_label(),"agent-jump"){jump_clicked=true;}
        }
        c.seg((16.0,37.0),(344.0,37.0),0.6,egui::Color32::from_white_alpha(34));
        if r.question {
            let index=r.question_index.min(r.data.questions.len().saturating_sub(1));
            let q=&r.data.questions[index];
            let a=&mut r.answers[index];
            let total=r.data.questions.len();
            c.centered_text(342.0,52.0,&format!("第 {} / {} 轮",index+1,total),10.0,W::Regular,muted,H::Right);
            let input_y=height-60.0;
            let body_top=62.0;
            let body_height=(input_y-body_top-8.0).max(30.0);
            let body_rect=c.rect(0.0,body_top,360.0,body_height);
            let body_clip=c.clipped(0.0,body_top,360.0,body_height);
            let body=body_clip.shifted(0.0,-r.question_scroll);
            let scroll_delta=c.ui.ctx().input(|i|i.raw_scroll_delta.y);
            if scroll_delta.abs()>0.01 && c.ui.ctx().input(|i|i.pointer.hover_pos()).is_some_and(|p|body_rect.contains(p)) {
                r.question_scroll=(r.question_scroll-scroll_delta).max(0.0);
            }
            let mut y=68.0;
            for line in body.wrap(&q.question,11.0,W::Semibold,324.0,3) {
                body.centered_text(18.0,y,&line,11.0,W::Semibold,ink,H::Left);
                y+=15.0;
            }
            if !q.detail.is_empty() {
                y+=3.0;
                for line in body.wrap(&q.detail,9.5,W::Regular,324.0,2) {
                    body.centered_text(18.0,y,&line,9.5,W::Regular,muted,H::Left);
                    y+=14.0;
                }
            }
            y+=5.0;
            let option_step=if q.options.is_empty(){0.0}else{
                if q.options.len()>6 {20.0} else {25.0}
            };
            for (option_index,o) in q.options.iter().enumerate() {
                let selected=a.selected.iter().any(|value|value==&o.label);
                let (button,b)=paint::capsule(&body,18.0,y,324.0,option_step-4.0,
                    &format!("agent-option-{}-{option_index}",q.id),
                    if selected{egui::Color32::from_rgb(39,94,150)}else{crate::theme::BTN},
                    crate::theme::BTN_HOVER);
                b.centered_text(30.0,y+(option_step-4.0)/2.0,&body.fit(&o.label,10.0,W::Regular,292.0),10.0,W::Regular,ink,H::Left);
                if selected {
                    body.centered_text(334.0,y+(option_step-4.0)/2.0,"✓",10.0,W::Semibold,crate::theme::WHITE,H::Center);
                }
                if button.clicked() {
                    if !q.multi {a.selected.clear();a.custom.clear();}
                    if selected {a.selected.retain(|value|value!=&o.label);} else if q.multi || a.selected.is_empty(){a.selected.push(o.label.clone());}
                }
                y+=option_step;
            }
            let max_scroll=(y-(body_top+body_height-4.0)).max(0.0);
            r.question_scroll=r.question_scroll.min(max_scroll);
            c.fill(18.0,input_y,324.0,26.0,7.0,crate::theme::BTN);
            c.stroke(18.0,input_y,324.0,26.0,7.0,0.6,egui::Color32::from_white_alpha(26));
            let input_rect=c.rect(18.0,input_y,324.0,26.0);
            let edit=egui::TextEdit::singleline(&mut a.custom).id(egui::Id::new(("agent-custom",&q.id)))
                .frame(false).margin(egui::Margin::symmetric(7,2)).hint_text("自定义回答")
                .font(crate::theme::font(10.5,W::Regular)).text_color(ink);
            let mut input_changed=false;
            egui::Area::new(egui::Id::new(("agent-custom-area",&q.id)))
                .order(egui::Order::Foreground)
                .fixed_pos(input_rect.min)
                .show(c.ui.ctx(),|ui|{
                    ui.set_min_size(input_rect.size());
                    let response=ui.add_sized(input_rect.size(),edit);
                    if response.clicked(){response.request_focus();}
                    input_changed=response.changed();
                });
            if input_changed && !q.multi {a.selected.clear();}
            let current_ready=!a.selected.is_empty()||!a.custom.trim().is_empty();
            let all_ready=current_ready && r.answers.iter().enumerate().all(|(i,answer)|i==index || !answer.selected.is_empty() || !answer.custom.trim().is_empty());
            let button_y=height-29.0;
            if index>0 {
                if notice_action(c,18.0,button_y,94.0,"上一轮","agent-detail-prev"){r.question_index-=1;}
            } else if notice_action(c,18.0,button_y,94.0,"回客户端","agent-detail-return"){action=Some(json!({"decision":"return"}));}
            let label=if index+1<total{"下一轮"}else{"提交回答"};
            let enabled=if index+1<total{current_ready}else{all_ready};
            let (next,b)=paint::capsule(c,248.0,button_y,94.0,21.0,"agent-detail-next",
                if enabled{egui::Color32::from_rgb(39,94,150)}else{crate::theme::BTN},crate::theme::BTN_HOVER);
            b.centered_text(295.0,button_y+10.5,label,10.5,W::Semibold,ink,H::Center);
            if enabled&&next.clicked() {
                if index+1<total {r.question_index+=1;r.question_scroll=0.0;} else {action=Some(json!({"decision":"answer","answers":&r.answers}));}
            }
        } else {
            if r.data.native_scope {
                c.centered_text(18.0,55.0,&c.fit("该授权的有效范围由客户端决定，请回客户端确认。",11.0,W::Regular,324.0),11.0,W::Regular,ink,H::Left);
            } else {
                if !r.data.cwd.is_empty(){c.centered_text(18.0,55.0,&c.fit(&r.data.cwd,9.5,W::Regular,324.0),9.5,W::Regular,muted,H::Left);}
                c.centered_text(18.0,78.0,&c.fit(&r.data.tool,12.0,W::Semibold,324.0),12.0,W::Semibold,ink,H::Left);
                let reason_lines=if r.data.reason.is_empty(){Vec::new()}else{c.wrap(&r.data.reason,9.5,W::Regular,324.0,2)};
                for (i,line) in reason_lines.iter().enumerate(){c.centered_text(18.0,98.0+i as f32*13.0,line,9.5,W::Regular,muted,H::Left);}
                let code_top=98.0+reason_lines.len().max(1) as f32*13.0+3.0;
                let code_available=((height-29.0)-code_top-8.0).max(26.0);
                let code_lines=((code_available-12.0)/14.0).floor().max(1.0) as usize;
                let lines=c.wrap(&r.data.operation,9.0,W::Regular,300.0,code_lines);
                let code_height=(lines.len().max(1) as f32*14.0+12.0).min(code_available);
                c.fill(18.0,code_top,324.0,code_height,8.0,egui::Color32::from_rgb(18,18,20));
                c.stroke(18.0,code_top,324.0,code_height,8.0,0.7,egui::Color32::from_white_alpha(28));
                c.centered_text(28.0,code_top+code_height/2.0,"$",11.0,W::Semibold,egui::Color32::from_rgb(196,126,42),H::Center);
                for (i,line) in lines.iter().enumerate(){
                    c.centered_text(40.0,code_top+8.0+i as f32*14.0,line,9.0,W::Regular,muted,H::Left);
                }
            }
            if notice_action(c,18.0,height-29.0,94.0,"回客户端","agent-detail-return"){action=Some(json!({"decision":"return"}));}
            if notice_action(c,118.0,height-29.0,72.0,"拒绝","agent-detail-deny"){action=Some(json!({"decision":"deny"}));}
            if !r.data.native_scope {
                let allowed=!r.data.operation.is_empty()&&!r.data.tool.is_empty();
                let (button,b)=paint::capsule(c,198.0,height-29.0,144.0,21.0,"agent-detail-allow",
                    if allowed{egui::Color32::from_rgb(39,94,150)}else{crate::theme::BTN},
                    crate::theme::BTN_HOVER);
                b.centered_text(270.0,height-18.5,"仅允许这一次",10.5,W::Semibold,ink,H::Center);
                if allowed&&button.clicked(){action=Some(json!({"decision":"allow"}));}
            }
        }
        if let Some(reply)=action {self.reply(id,reply);}
        // 跳转在借用结束后执行；失败要留下原因给卡片显示，不做静默失败。
        if jump_clicked {
            if let Some(target)=&jump {self.jump_error=crate::agent_jump::open(target).err();}
        }
        true
    }
    pub(crate) fn count(&self)->usize{self.requests.len()}
    pub(crate) fn countdown(&mut self,t:f64,seconds:u8,hidden:bool)->Option<u8>{
        let r=self.requests.front_mut()?;
        if seconds==0 || self.hovering || r.details || r.data.deferred || hidden {r.shown=None;return None;}
        let start=*r.shown.get_or_insert(t);Some((f64::from(seconds)-(t-start)).ceil().max(0.0) as u8)
    }
    pub(crate) fn receive(&mut self,v:Value,settings:&Settings)->Option<(Provider,String)>{
        if let Some(ready)=v["dshReady"].as_bool(){self.dsh_ready=ready && Provider::Dsh.enabled(settings);return None;}
        if let Some(id)=v["cancel"].as_str(){self.remove(id);return None;}
        let id=v["id"].as_str()?.to_owned();
        // 来源或类型无法识别时必须交回客户端：静默丢弃会让请求一直挂到宿主超时。
        let provider=match v["provider"].as_str(){
            Some("dsh")=>Provider::Dsh,
            Some("codex")=>Provider::Codex,
            _=>{self.hand_back(&id);return None;}
        };
        let Some(kind)=v["kind"].as_str() else{self.hand_back(&id);return None;};
        if !provider.enabled(settings){self.hand_back(&id);return None;}
        let Ok(data)=serde_json::from_value::<Data>(v["data"].clone()) else {self.hand_back(&id);return None;};
        if kind=="finished" {return settings.agent.completion.then(||(provider,match settings.agent.content{
            1=>data.text.chars().take(1200).collect(),2=>data.text.chars().take(32000).collect(),_=>String::new()}));}
        let bad_question=kind=="question" && (data.questions.is_empty() || question_violation(&data.questions).is_some());
        if !settings.agent.interactive || data.deferred && !settings.agent.deferred || self.requests.len()>=32
            || !["approval","question"].contains(&kind) || bad_question {self.hand_back(&id);return None;}
        self.result=None;
        let answers=data.questions.iter().map(|q|Answer{id:q.id.clone(),..Default::default()}).collect();
        // 同一 id 重发（客户端重试）时先移除旧记录，避免队列出现两条同名请求；
        // 索引必须在移除之后计算，否则 VecDeque 会越界。
        self.remove(&id);
        let insert=if data.deferred{self.requests.len()}else{self.requests.iter().position(|r|r.data.deferred&&!r.details).unwrap_or(self.requests.len())};
        self.requests.insert(insert,Request{id,provider,question:kind=="question",data,answers,question_index:0,question_scroll:0.0,shown:None,details:false});None
    }
    pub fn update(&mut self,settings:&Settings,ctx:&egui::Context)->Vec<(Provider,String)>{
        let [dsh,codex]=Provider::ALL.map(|p|p.enabled(settings));
        if !dsh{self.dsh_ready=false;}
        if !dsh&&!codex {self.service=None;self.requests.clear();self.result=None;self.retry=None;return vec![];}
        if self.service.as_mut().is_some_and(|s|s.child.try_wait().ok().flatten().is_some()){
            self.dsh_ready=false;self.service=None;self.requests.clear();self.retry=Some(Instant::now()+Duration::from_secs(2));
        }
        if self.service.is_none() && self.retry.is_none_or(|t|t<=Instant::now()){
            self.service=Service::start(ctx);self.retry=Some(Instant::now()+Duration::from_secs(2));
        }
        let mut done=vec![];
        if let Some(s)=&mut self.service{
            let policy=json!({"dsh":dsh,"codex":codex,"interactive":settings.agent.interactive,
                "content":settings.agent.content,"completion":settings.agent.completion,"deferred":settings.agent.interactive&&settings.agent.deferred});
            if s.policy!=policy{let _=s.out.send(json!({"policy":policy}));s.policy=policy;}
            let batch:Vec<_>=s.incoming.try_iter().take(96).collect();
            for v in batch{if let Some(result)=self.receive(v,settings){done.push(result);}}
        }
        let retired:Vec<_>=self.requests.iter().filter(|r|!r.provider.enabled(settings)||!settings.agent.interactive||r.data.deferred&&!settings.agent.deferred).map(|r|r.id.clone()).collect();
        for id in retired{self.hand_back(&id);}
        if self.result.as_ref().is_some_and(|(p,_)|!p.enabled(settings)||!settings.agent.completion||settings.agent.content==0){self.result=None;}
        if settings.agent.content==1{if let Some((_,text))=&mut self.result{if let Some((end,_))=text.char_indices().nth(1200){text.truncate(end);}}}
        done
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Settings;

    /// 开启 DSH 来源；notifications 总开关默认已开。
    fn dsh_on()->Settings{
        let mut settings=Settings::default();
        settings.dsh_notifications=true;
        settings
    }

    #[test]
    fn unknown_provider_is_handed_back_instead_of_dropped(){
        let mut cards=Cards::default();
        cards.receive(json!({"id":"unknown","provider":"claude","kind":"approval","data":{"tool":"Bash"}}),&dsh_on());
        assert_eq!(cards.count(),0,"未知来源不应入队");
        assert_eq!(cards.handed_back,vec!["unknown".to_owned()],
            "未知来源必须交回客户端，否则请求会一直挂到宿主超时");
    }

    #[test]
    fn missing_provider_is_handed_back(){
        let mut cards=Cards::default();
        cards.receive(json!({"id":"noprovider","kind":"approval","data":{}}),&dsh_on());
        assert_eq!(cards.handed_back,vec!["noprovider".to_owned()],
            "缺少来源字段同样无法接管，应交回");
    }

    #[test]
    fn missing_kind_is_handed_back(){
        let mut cards=Cards::default();
        cards.receive(json!({"id":"nokind","provider":"dsh","data":{}}),&dsh_on());
        assert_eq!(cards.handed_back,vec!["nokind".to_owned()],
            "缺少事件类型无法判断处理方式，应交回");
    }

    #[test]
    fn known_provider_approval_is_queued_without_hand_back(){
        let mut cards=Cards::default();
        cards.receive(json!({"id":"ok","provider":"dsh","kind":"approval","data":{"tool":"Bash","operation":"ls"}}),&dsh_on());
        assert_eq!(cards.count(),1,"合法审批应入队");
        assert!(cards.handed_back.is_empty(),"合法请求不应回交");
    }

    /// 构造一道题；options 指定选项数量。
    fn question_payload(id:&str,text:&str,options:usize)->Value{
        let options:Vec<_>=(0..options).map(|i|json!({"label":format!("选项{i}")})).collect();
        json!({"id":id,"provider":"dsh","kind":"question","data":{
            "questions":[{"id":"q1","question":text,"options":options}]}})
    }

    #[test]
    fn too_many_options_are_handed_back(){
        let mut cards=Cards::default();
        cards.receive(question_payload("many","选一个",51),&dsh_on());
        assert_eq!(cards.count(),0,"选项超过 50 不应入队");
        assert_eq!(cards.handed_back,vec!["many".to_owned()],"超量选项必须交回，否则会拖慢绘制");
    }

    #[test]
    fn option_limit_boundary_is_accepted(){
        let mut cards=Cards::default();
        cards.receive(question_payload("fifty","选一个",50),&dsh_on());
        assert_eq!(cards.count(),1,"正好 50 个选项应当接受");
        assert!(cards.handed_back.is_empty());
    }

    #[test]
    fn oversized_question_text_is_handed_back(){
        let mut cards=Cards::default();
        let text="题".repeat(20_001);
        cards.receive(question_payload("long",text.as_str(),1),&dsh_on());
        assert_eq!(cards.handed_back,vec!["long".to_owned()],"题干超过 20000 字符应交回");
    }

    #[test]
    fn duplicate_option_labels_are_handed_back(){
        let mut cards=Cards::default();
        cards.receive(json!({"id":"dup","provider":"dsh","kind":"question","data":{
            "questions":[{"id":"q1","question":"选一个","options":[{"label":"A"},{"label":"A"}]}]}}),&dsh_on());
        assert_eq!(cards.handed_back,vec!["dup".to_owned()],"选项 label 重复应交回");
    }

    #[test]
    fn duplicate_question_ids_are_handed_back(){
        let mut cards=Cards::default();
        cards.receive(json!({"id":"dupq","provider":"dsh","kind":"question","data":{
            "questions":[{"id":"q1","question":"一","options":[{"label":"A"}]},
                         {"id":"q1","question":"二","options":[{"label":"B"}]}]}}),&dsh_on());
        assert_eq!(cards.handed_back,vec!["dupq".to_owned()],"题目 id 重复应交回");
    }

    #[test]
    fn blank_option_label_is_handed_back(){
        let mut cards=Cards::default();
        cards.receive(json!({"id":"blank","provider":"dsh","kind":"question","data":{
            "questions":[{"id":"q1","question":"选一个","options":[{"label":"   "}]}]}}),&dsh_on());
        assert_eq!(cards.handed_back,vec!["blank".to_owned()],"空白选项应交回");
    }

    #[test]
    fn duplicate_request_id_replaces_existing_entry(){
        let mut cards=Cards::default();
        let payload=json!({"id":"same","provider":"dsh","kind":"approval","data":{"tool":"A","operation":"a"}});
        cards.receive(payload.clone(),&dsh_on());
        cards.receive(payload,&dsh_on());
        assert_eq!(cards.count(),1,"同一 id 重发不应在队列里出现两条");
    }

    #[test]
    fn jump_target_reads_session_and_cwd(){
        let mut settings=dsh_on();
        settings.codex_notifications=true;
        let mut cards=Cards::default();
        cards.receive(json!({"id":"j","provider":"codex","kind":"approval","data":{
            "tool":"Bash","operation":"ls","sessionId":"thread-1","cwd":"C:\\work"}}),&settings);
        assert_eq!(cards.jump_target("j"),
            Some(crate::agent_jump::Target::DeepLink("codex://threads/thread-1".to_owned())),
            "Codex 有线程 id 时应生成深链");
        assert_eq!(cards.jump_target("missing"),None,"不在队列里的请求没有跳转目标");
    }

    #[test]
    fn jump_target_for_dsh_points_at_its_client(){
        let mut cards=Cards::default();
        cards.receive(json!({"id":"d","provider":"dsh","kind":"approval","data":{
            "tool":"Bash","operation":"ls","sessionId":"s-1","cwd":"C:\\work"}}),&dsh_on());
        assert_eq!(cards.jump_target("d"),
            Some(crate::agent_jump::Target::Client{name:"DeepSeek Harness",cwd:"C:\\work".to_owned()}),
            "DSH 没有深链协议，应指向客户端窗口并保留目录兜底");
    }

    #[test]
    fn long_operation_keeps_detail_card_within_its_width(){
        let mut settings=dsh_on();
        settings.codex_notifications=true;
        let mut cards=Cards::default();
        let operation="{\"file_path\":\"C:\\\\Windows\\\\Temp\\\\x.txt\",\"justification\":\"".to_owned()
            + &"很长的说明文字".repeat(40) + "\"}";
        cards.receive(json!({"id":"wide","provider":"dsh","kind":"approval","data":{
            "tool":"write","operation":operation,"reason":"测试换行宽度"}}),&settings);
        cards.detail("wide");
        let (width,height)=cards.detail_size("wide");
        assert_eq!(width,360.0,"详情卡宽度是固定值，不能被内容撑开");
        assert!(height>0.0 && height<=crate::app::WIN_H-8.0,"高度必须留在窗口内");
    }
}
