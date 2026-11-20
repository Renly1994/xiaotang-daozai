//! A request stays tied to its live client connection; replies never enter notification history.
//!
//! 这个文件只保留状态机（队列、倒计时、交回、协议）与桥接子进程管理；
//! 卡片长什么样、多高，全部交给 `crate::agent_ui`。

use crate::agent_ui::{self, Action, Answer, Question, State};
use crate::agent_notice::Provider;
use crate::model::Settings;
use crate::paint::Card;
use eframe::egui;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug,Clone,PartialEq,Eq,Serialize,Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub interactive:bool, pub handoff:u8, pub completion:bool, pub dwell:u8,
    pub expanded:bool, pub content:u8, pub deferred:bool, pub system_toast:bool,
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

struct Request {
    id: String,
    provider: Provider,
    question: bool,
    data: Data,
    answers: Vec<Answer>,
    question_index: usize,
    question_scroll: f32,
    /// 提问的「检查答案」页是否打开。
    reviewing: bool,
    /// 审批正文的滚动偏移（长命令/大段 JSON）。
    scroll: f32,
    shown: Option<f64>,
    details: bool,
    /// 详情展开的时刻，用于入场淡入。
    detail_at: Option<f64>,
    /// 当前交互阶段（等待 / 结果未知）；提交成功后立即移除。
    state: State,
    error: Option<String>,
}
impl Request {
    fn title(&self) -> &str {
        if self.data.deferred {
            "可稍后回答"
        } else if self.question {
            "需要你回答"
        } else {
            "等待你审批"
        }
    }
    fn summary(&self) -> &str {
        if self.question {
            self.data.questions.first().map(|q| q.question.as_str()).unwrap_or("")
        } else {
            &self.data.tool
        }
    }
    fn head<'a>(&'a self, jump: Option<&'a crate::agent_jump::Target>) -> agent_ui::Head<'a> {
        let subtitle = if !self.data.cwd.trim().is_empty() {
            self.data.cwd.as_str()
        } else if !self.data.session_id.trim().is_empty() {
            self.data.session_id.as_str()
        } else {
            ""
        };
        agent_ui::Head { provider: self.provider, kicker: self.title(), subtitle, jump }
    }
    fn jump(&self) -> Option<crate::agent_jump::Target> {
        let target = crate::agent_jump::resolve(self.provider, &self.data.session_id, &self.data.cwd);
        target.available().then_some(target)
    }
}

/// 完成内容为「仅状态」时，结果层里的标题与说明：本机生成，不含任何宿主正文。
/// 桥接侧在 content=0 时压根不采集回复文本（见 integrations/agent-notices/bridge/index.mjs），
/// 所以这里只能诚实说明原因并给出下一步，绝不能凭空造正文。
pub(crate) const NO_BODY_RESULT_TITLE: &str = "本轮未接收回复正文";
pub(crate) const NO_BODY_RESULT_TEXT: &str = "当前「完成内容」设置为「仅状态」：宿主不会把回复正文发给岛仔，所以这里没有内容可显示，不是加载失败。

## 想看到正文
- 打开 设置 → Agent → 完成内容
- 改成「回复摘要」或「本轮回复」
- 下一轮结束后再点「查看回复」

说明：本层由岛仔本机生成，不包含、也不会去读取任何回复内容。";

/// 结果层标题：有正文是「本轮回复」，说明层要一眼看出没有正文。
fn result_kicker(hint: bool) -> &'static str {
    if hint { NO_BODY_RESULT_TITLE } else { "本轮回复" }
}

/// 「查看回复」打开的结果层。
#[derive(Clone)]
pub(crate) struct ResultCard {
    pub provider: Provider,
    pub text: String,
    /// 本机生成的说明（无正文时的占位），不是宿主正文。
    pub hint: bool,
}

/// 收起卡（banner）的只读展示信息。
pub(crate) struct BannerInfo {
    pub subtitle: String,
    pub jump: Option<crate::agent_jump::Target>,
    pub question: bool,
    pub provider: Provider,
}

#[derive(Default)]
pub struct Cards {
    service:Option<Service>, retry:Option<Instant>, requests:VecDeque<Request>,
    pub(crate) dsh_ready:bool,
    pub(crate) hovering:bool,
    /// 完成通知的「查看回复」内容；Some 表示结果层打开。
    pub(crate) result:Option<ResultCard>,
    /// 测量字体用的上下文（`App::new` 里绑定一次）。
    ctx:Option<egui::Context>,
    result_scroll:f32,
    result_at:Option<f64>,
    copied_at:Option<Instant>,
    /// 最近一次跳转失败的原因，显示在详情卡上，不做静默失败。
    pub(crate) jump_error:Option<String>,
    /// 测试观察用：记录被交回的请求 id，生产构建不编译。
    #[cfg(test)]
    pub(crate) handed_back:Vec<String>,
}

impl Cards {
    pub(crate) fn bind_ctx(&mut self,ctx:&egui::Context){self.ctx=Some(ctx.clone());}
    fn measurer(&self)->Option<agent_ui::Measurer<'_>>{self.ctx.as_ref().map(agent_ui::Measurer::new)}
    pub(crate) fn active(&self)->Option<(&str,Provider,&str,&str)>{
        self.requests.front().map(|r|(r.id.as_str(),r.provider,r.title(),r.summary()))
    }
    fn remove(&mut self,id:&str){self.requests.retain(|r|r.id!=id);}
    /// 向桥接发送决定；返回是否真的写进了管道。
    fn send(&self,id:&str,reply:Value)->bool{
        match &self.service{
            Some(s)=>s.out.send(json!({"id":id,"reply":reply})).is_ok(),
            None=>false,
        }
    }
    /// 用户做出的决定：发送成功才移除；失败保留卡片并进入「结果未知」。
    /// 只在请求仍留在队列里时可用；draw_details 已经取走请求，走下面的 resolve()。
    fn decide(&mut self,id:&str,reply:Value){
        if self.send(id,reply){self.remove(id);return;}
        if self.requests.front().is_some_and(|r|r.id==id){
            let r=self.requests.pop_front().expect("刚判空，队首必然存在");
            self.mark_uncertain(r);
        }
    }
    /// 失败兜底：保留请求、标记「结果未知」，并放回队首继续显示（不能让卡片凭空消失）。
    fn mark_uncertain(&mut self,mut r:Request){
        r.state=State::Uncertain;
        r.error=Some("提交失败，结果未知。请回客户端确认后再处理。".into());
        r.details=true;
        if r.detail_at.is_none(){r.detail_at=Some(0.0);}
        self.requests.push_front(r);
    }
    /// 把已经从队列取出的请求发出去：成功即结束（卡片可以消失）；失败放回队首并返回 true
    /// （还要继续显示卡片）。不能用 decide()：那时它在队列里已经不存在，队首查找必然落空。
    fn resolve(&mut self,r:Request,reply:Value)->bool{
        if self.send(&r.id,reply){return false;}
        self.mark_uncertain(r);
        true
    }
    pub(crate) fn hand_back(&mut self,id:&str){
        #[cfg(test)]
        self.handed_back.push(id.to_owned());
        self.send(id,json!({"decision":"return"}));
        self.remove(id);
    }
    pub(crate) fn detail(&mut self,id:&str){
        if let Some(r)=self.requests.front_mut().filter(|r|r.id==id){r.details=true;r.detail_at=None;}
    }
    pub(crate) fn close_detail(&mut self,id:&str){
        if let Some(r)=self.requests.front_mut().filter(|r|r.id==id){r.details=false;r.shown=None;r.detail_at=None;}
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
    /// 是否正在编辑（阻止自动交回、阻止岛体收起）。
    pub(crate) fn editing(&self)->bool{
        self.requests.front().is_some_and(|r|r.details&&r.question)
    }
    /// 详情尺寸：展开时按内容实测，收起时给提示卡的固定尺寸。
    /// 完成通知的实测尺寸（面板高度必须跟卡片一致，否则正文会压到按钮上）。
    pub(crate) fn done_size(&self, provider: Provider, text: &str, compact: bool) -> (f32, f32) {
        let Some(m) = self.measurer() else {
            return (agent_ui::CARD_W, if compact { 44.0 } else { 112.0 });
        };
        let view = agent_ui::DoneView { provider, kicker: "本轮已结束", text, compact, age: 1.0 };
        agent_ui::measure_done(&m, &view)
    }
    pub(crate) fn detail_size(&self, id:&str)->(f32,f32){
        let Some(r)=self.requests.front().filter(|r|r.id==id) else{return (agent_ui::CARD_W,130.0);};
        if !r.details{return (agent_ui::CARD_W,130.0);}
        let Some(m)=self.measurer() else{return (agent_ui::CARD_W,130.0);};
        let jump=r.jump();
        if r.question {
            let mut answers=r.answers.clone();
            let view=agent_ui::QuestionView{
                head:r.head(jump.as_ref()),
                questions:&r.data.questions,
                answers:&mut answers,
                index:r.question_index,
                scroll:r.question_scroll,
                reviewing:r.reviewing,
                queue:self.requests.len(),
                handoff:None,
                state:r.state,
                error:r.error.as_deref(),
                age:1.0,
            };
            agent_ui::measure_question(&m,&view,agent_ui::CARD_W)
        }else{
            let view=agent_ui::ApprovalView{
                head:r.head(jump.as_ref()),
                tool:&r.data.tool,
                reason:&r.data.reason,
                cwd:&r.data.cwd,
                operation:&r.data.operation,
                native_scope:r.data.native_scope,
                queue:self.requests.len(),
                handoff:None,
                state:r.state,
                error:r.error.as_deref(),
                age:1.0,
            };
            agent_ui::measure_approval(&m,&view,agent_ui::CARD_W)
        }
    }
    /// 在现有岛体表面上绘制展开的请求详情；返回是否仍在显示。
    pub(crate) fn draw_details(&mut self,c:&Card,id:&str)->bool{
        let t=c.ui.ctx().input(|i|i.time);
        let Some(index)=self.requests.iter().position(|r|r.id==id) else{return false;};
        let mut r=self.requests.remove(index).expect("队列项刚被定位，必须存在");
        let jump=r.jump();
        let m=agent_ui::Measurer::new(c.ui.ctx());
        if r.state==State::Pending&&c.ui.rect_contains_pointer(c.painter.clip_rect()){self.hovering=true;}
        let at=*r.detail_at.get_or_insert(t);
        let age=(t-at).max(0.0) as f32;
        let queue=self.requests.len()+1;
        let action=if r.question {
            let mut answers=std::mem::take(&mut r.answers);
            let mut view=agent_ui::QuestionView{
                head:r.head(jump.as_ref()),
                questions:&r.data.questions,
                answers:&mut answers,
                index:r.question_index,
                scroll:r.question_scroll,
                reviewing:r.reviewing,
                queue,
                handoff:None,
                state:r.state,
                error:r.error.as_deref(),
                age,
            };
            let out=agent_ui::draw_question(c,&m,&mut view,r.question_scroll,t);
            r.answers=answers;
            if let Some(index)=out.index{r.question_index=index;}
            if let Some(reviewing)=out.reviewing{r.reviewing=reviewing;}
            r.question_scroll=out.scroll;
            out.action
        } else {
            let view=agent_ui::ApprovalView{
                head:r.head(jump.as_ref()),
                tool:&r.data.tool,
                reason:&r.data.reason,
                cwd:&r.data.cwd,
                operation:&r.data.operation,
                native_scope:r.data.native_scope,
                queue,
                handoff:None,
                state:r.state,
                error:r.error.as_deref(),
                age,
            };
            let (action,scroll)=agent_ui::draw_approval(c,&m,&view,r.scroll,t);
            r.scroll=scroll;
            action
        };
        match action {
            Some(Action::Jump)=>{
                if let Some(target)=&jump{self.jump_error=crate::agent_jump::open(target).err();}
            }
            Some(Action::Collapse)=>{r.details=false;r.shown=None;r.detail_at=None;}
            Some(Action::Return)=>{self.hand_back(&r.id);return false;}
            Some(Action::Deny)=>{return self.resolve(r,json!({"decision":"deny"}));}
            Some(Action::Allow)=>{
                // 没有完整操作时不能在岛上允许：保持卡片，让用户回客户端。
                if !r.data.native_scope&&!r.data.operation.trim().is_empty()&&!r.data.tool.trim().is_empty(){
                    return self.resolve(r,json!({"decision":"allow"}));
                }
            }
            Some(Action::Answer)=>{
                let answers=r.answers.clone();
                return self.resolve(r,json!({"decision":"answer","answers":answers}));
            }
            _=>{}
        }
        // 未产生决定：放回队首（期间若被 cancel 移除则丢弃本地副本）。
        if self.requests.iter().all(|other|other.id!=r.id){
            self.requests.push_front(r);
        }
        true
    }
    /// 收起卡需要的展示信息（标题/正文之外的部分）。
    pub(crate) fn banner_info(&self,id:&str)->Option<BannerInfo>{
        let r=self.requests.iter().find(|r|r.id==id)?;
        Some(BannerInfo{subtitle:r.data.cwd.clone(),jump:r.jump(),question:r.question,provider:r.provider})
    }
    pub(crate) fn request_summary(&self,id:&str)->Option<String>{
        self.requests.iter().find(|r|r.id==id).map(|r|r.summary().to_owned())
    }
    /// 测试观察用：当前请求是否停在「检查答案」页。
    #[cfg(test)]
    pub(crate) fn reviewing(&self,id:&str)->bool{self.requests.iter().any(|r|r.id==id&&r.reviewing)}
    pub(crate) fn count(&self)->usize{self.requests.len()}
    /// 测试观察用：结果层当前滚动偏移（每帧必须被 draw_result 写回）。
    #[cfg(test)]
    pub(crate) fn result_scroll(&self)->f32{self.result_scroll}
    pub(crate) fn countdown(&mut self,t:f64,seconds:u8,hidden:bool)->Option<u8>{
        let r=self.requests.front_mut()?;
        if seconds==0 || self.hovering || r.details || r.data.deferred || hidden || r.state.busy() {r.shown=None;return None;}
        let start=*r.shown.get_or_insert(t);Some((f64::from(seconds)-(t-start)).ceil().max(0.0) as u8)
    }
    /// 完成通知「查看回复」：打开结果层（不落盘，只用事件带来的正文）。
    pub(crate) fn open_result(&mut self,provider:Provider,text:String){
        self.result=Some(ResultCard{provider,text,hint:false});
        self.result_scroll=0.0;
        self.result_at=None;
    }
    /// 没有正文时的「查看回复」：打开本机说明层。
    /// 点击必须有可见结果——静默 no-op 会让用户以为按钮坏了。
    pub(crate) fn open_result_no_body(&mut self,provider:Provider){
        self.result=Some(ResultCard{provider,text:NO_BODY_RESULT_TEXT.to_owned(),hint:true});
        self.result_scroll=0.0;
        self.result_at=None;
    }
    pub(crate) fn close_result(&mut self){self.result=None;self.result_scroll=0.0;self.result_at=None;}
    pub(crate) fn result_open(&self)->bool{self.result.is_some()}
    pub(crate) fn result_size(&self)->(f32,f32){
        let Some(result)=&self.result else{return (agent_ui::CARD_W,140.0)};
        let Some(m)=self.measurer() else{return (agent_ui::CARD_W,140.0);};
        let view=agent_ui::ResultView{provider:result.provider,kicker:result_kicker(result.hint),text:&result.text,scroll:self.result_scroll,age:1.0,copied:None,hint:result.hint};
        agent_ui::measure_result(&m,&view)
    }
    pub(crate) fn draw_result(&mut self,c:&Card)->Option<Action>{
        let t=c.ui.ctx().input(|i|i.time);
        let Some(result)=self.result.clone() else{return None;};
        let m=agent_ui::Measurer::new(c.ui.ctx());
        let at=*self.result_at.get_or_insert(t);
        let age=(t-at).max(0.0) as f32;
        let copied=self.copied_at.map(|at|(1.4-at.elapsed().as_secs_f32()).max(0.0));
        if self.copied_at.is_some_and(|at|at.elapsed()>Duration::from_secs_f32(1.4)){self.copied_at=None;}
        let view=agent_ui::ResultView{provider:result.provider,kicker:result_kicker(result.hint),text:&result.text,scroll:self.result_scroll,age,copied,hint:result.hint};
        let (action,scroll)=agent_ui::draw_result(c,&m,&view,t);
        // 与审批/提问详情一致：滚动偏移必须写回状态，否则每帧都从 0 重新开始。
        self.result_scroll=scroll;
        match action {
            Some(Action::CopyResult)=>{
                c.ui.ctx().copy_text(result.text.clone());
                self.copied_at=Some(Instant::now());
            }
            Some(Action::Dismiss)=>{self.close_result();}
            _=>{}
        }
        action
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
        self.close_result();
        let answers=data.questions.iter().map(|q|Answer{id:q.id.clone(),..Default::default()}).collect();
        // 同一 id 重发（客户端重试）时先移除旧记录，避免队列出现两条同名请求；
        // 索引必须在移除之后计算，否则 VecDeque 会越界。
        self.remove(&id);
        let insert=if data.deferred{self.requests.len()}else{self.requests.iter().position(|r|r.data.deferred&&!r.details).unwrap_or(self.requests.len())};
        self.requests.insert(insert,Request{id,provider,question:kind=="question",data,answers,question_index:0,question_scroll:0.0,reviewing:false,scroll:0.0,shown:None,details:false,detail_at:None,state:State::Pending,error:None});None
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
        // content 改成「仅状态」时要立刻关掉已经打开的正文层（隐私）；本机说明层不含正文，关掉反而变成死点击，保留。
        if self.result.as_ref().is_some_and(|r|!r.provider.enabled(settings)||!settings.agent.completion||(settings.agent.content==0&&!r.hint)){self.close_result();}
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
    fn long_operation_keeps_detail_card_within_its_width_and_canvas(){
        let ctx=egui::Context::default();
        crate::theme::load_fonts(&ctx);
        // 测量要用字体，先跑一帧让字体表就绪（真实运行时每帧都在 ctx.run 内）。
        let _=ctx.run(egui::RawInput{screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::Vec2::new(640.0,crate::app::WIN_H))),..Default::default()},|_|{});
        let mut settings=dsh_on();
        settings.codex_notifications=true;
        let mut cards=Cards::default();
        let operation="{\"file_path\":\"C:\\\\Windows\\\\Temp\\\\x.txt\",\"justification\":\"".to_owned()
            + &"很长的说明文字".repeat(40) + "\"}";
        cards.receive(json!({"id":"wide","provider":"dsh","kind":"approval","data":{
            "tool":"write","operation":operation,"reason":"测试换行宽度"}}),&settings);
        cards.detail("wide");
        cards.bind_ctx(&ctx);
        let (width,height)=cards.detail_size("wide");
        assert_eq!(width,agent_ui::CARD_W,"详情卡宽度跟随面板，不能被内容撑开");
        assert!(height>0.0 && height<=crate::app::WIN_H-8.0,"高度必须留在窗口内");
    }

    #[test]
    fn submit_without_a_bridge_keeps_the_card_and_marks_result_unknown(){
        let mut cards=Cards::default();
        cards.receive(json!({"id":"orphan","provider":"dsh","kind":"approval","data":{
            "tool":"Bash","operation":"ls"}}),&dsh_on());
        cards.detail("orphan");
        // 没有 Service（桥接子进程）时不能静默删除：必须保留卡片并进入结果未知。
        assert!(!cards.send("orphan",json!({"decision":"allow"})));
        cards.decide("orphan",json!({"decision":"allow"}));
        assert_eq!(cards.count(),1,"发送失败时卡片必须保留，等待用户回客户端");
        let request=cards.requests.front().unwrap();
        assert_eq!(request.state,State::Uncertain);
        assert!(request.error.is_some(),"结果未知要给出明确提示，不能点了没反应");
    }

    #[test]
    fn hand_back_removes_even_without_a_bridge(){
        let mut cards=Cards::default();
        cards.receive(json!({"id":"gone","provider":"dsh","kind":"approval","data":{"tool":"Bash"}}),&dsh_on());
        cards.hand_back("gone");
        assert_eq!(cards.count(),0,"交回后本机不应继续持有该请求");
    }
}
