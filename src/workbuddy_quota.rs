//! WorkBuddy 官方客户端积分汇总，浏览器授权连接。
use super::{Account,Snapshot};
use serde_json::{json,Value};
use std::{io::Read,sync::mpsc,time::Duration};
#[derive(Clone,Debug)]
#[derive(serde::Serialize,serde::Deserialize)]
pub struct Credits {pub remaining:f64,pub packages:Vec<(String,f64)>}
impl Credits {
    pub fn compact(&self)->String{super::deepseek::Balance{currency:"CNY".into(),total:self.remaining,granted:0.0,topped_up:0.0}.compact().trim_start_matches('¥').to_owned()}
    pub fn full(&self)->String{let text=super::deepseek::Balance{currency:"CNY".into(),total:self.remaining,granted:0.0,topped_up:0.0}.full(self.remaining);text.trim_start_matches('¥').trim_end_matches('0').trim_end_matches('.').to_owned()}
}
pub fn parse(value:&Value)->Result<Snapshot,String>{
    if value["code"].as_i64()!=Some(0){return Err("WorkBuddy 积分接口暂未返回结果，请稍后重试".into());}
    let data=&value["data"];let rows=data["Packages"].as_array().ok_or("积分接口缺少资源包数据")?;
    let mut packages=Vec::new();let mut remaining=0.0;
    for row in rows{
        let amount=row["CycleRemainCapacity"].as_f64().or_else(||row["CycleRemainCapacity"].as_str().and_then(|s|s.parse().ok())).filter(|n|n.is_finite()&&*n>=0.0).ok_or("积分余额字段无效")?;
        remaining+=amount;packages.push((row["PackageCode"].as_str().unwrap_or_default().into(),amount));
    }
    if !remaining.is_finite(){return Err("积分余额超出范围".into());}
    if let Some(groups)=data["CreditGroups"].as_array(){
        packages.clear();for group in groups{
            let label=match group["Kind"].as_str(){Some("plan_base")=>"套餐",Some("plan_bonus")=>"加赠",Some("addon")=>"加量包",Some("benefit")=>"赠送",_=>continue};
            if let Some(amount)=group["CycleRemainCapacity"].as_f64().or_else(||group["CycleRemainCapacity"].as_str().and_then(|s|s.parse::<f64>().ok())).filter(|n|n.is_finite()&&*n>0.0){packages.push((label.into(),amount));}
        }
    }else{packages.clear();}
    Ok(Snapshot{plan:data["SubscriptionPackageName"].as_str().unwrap_or_default().into(),credits:Some(Credits{remaining,packages}),at:chrono::Utc::now().timestamp(),..Default::default()})
}
fn valid_origin(origin:&str)->bool{matches!(origin,"https://www.codebuddy.cn"|"https://www.workbuddy.cn")}
fn slot(uid:&str)->String{ring::digest::digest(&ring::digest::SHA256,uid.as_bytes()).as_ref().iter().map(|b|format!("{b:02x}")).collect()}
#[derive(serde::Serialize,serde::Deserialize)]
struct Session { origin:String, ua:String, cookie:String }

fn session_path(id:&str)->Result<std::path::PathBuf,String>{
    if id.len()!=64 || !id.bytes().all(|c|c.is_ascii_hexdigit()){return Err("WorkBuddy 账号标识无效".into());}
    Ok(crate::model::config_dir().join("credentials").join(format!("workbuddy-{id}.bin")))
}
fn save_session(id:&str,session:&Session)->Result<(),String>{
    let path=session_path(id)?;
    let bytes=serde_json::to_vec(session).map_err(|_|"登录会话无效")?;
    let encrypted=super::gemini::protect(&bytes,true)?;
    std::fs::create_dir_all(path.parent().unwrap()).and_then(|_|crate::model::atomic_write(&path,&encrypted)).map_err(|_|"无法保存 WorkBuddy 登录会话".into())
}
pub fn forget(account:&Account){
    if let Some(path)=account.slot.as_deref().and_then(|id|session_path(id).ok()){let _=std::fs::remove_file(path);}
}
const SUMMARY:&str="/billing/meter/get-user-resource-summary";
enum Failure { Transport, Expired, Message(String) }
impl Failure {
    fn message(self)->String{match self{Self::Transport=>"暂时无法连接 WorkBuddy 官网".into(),Self::Expired=>"官网登录已过期，请重新连接 WorkBuddy".into(),Self::Message(s)=>s}}
}
fn api(agent:&ureq::Agent,session:&Session,path:&str)->Result<Value,Failure>{
    let url=format!("{}{path}",session.origin);
    let req=agent.request(if path==SUMMARY{"POST"}else{"GET"},&url)
        .set("User-Agent",&session.ua).set("Cookie",&session.cookie).set("Accept","application/json").set("X-Client-Platform","web");
    let response=if path==SUMMARY{req.set("Content-Type","application/json").send_string("{}")}else{req.call()}.map_err(|e|match e{
        ureq::Error::Status(401,_)=>Failure::Expired,
        ureq::Error::Status(code,_)=>Failure::Message(format!("WorkBuddy 官网返回 {code}，请稍后重试")),
        _=>Failure::Transport
    })?;
    serde_json::from_reader(response.into_reader().take(2*1024*1024)).map_err(|_|Failure::Message("WorkBuddy 官网返回了无效数据".into()))
}
fn verified(agent:&ureq::Agent,s:&Session,id:&str)->Result<Snapshot,Failure>{
    let profile=api(agent,s,"/console/account")?;
    let uid=profile.pointer("/data/uid").and_then(Value::as_str).ok_or_else(||Failure::Message("官网未返回 WorkBuddy 账号信息".into()))?;
    if slot(uid)!=id || profile.pointer("/data/isCurrentOneIdEnterprise")==Some(&Value::Bool(true)){
        return Err(Failure::Message("官网账号与已绑定的个人账号不一致".into()));
    }
    parse(&api(agent,s,SUMMARY)?).map_err(Failure::Message)
}
fn query(agent:&ureq::Agent,s:&Session,id:&str)->Result<Snapshot,Failure>{
    if !valid_origin(&s.origin){return Err(Failure::Message("WorkBuddy 登录来源无效".into()));}
    let result=verified(agent,s,id);
    if !matches!(result,Err(Failure::Transport)){return result;}
    // 仅在本地代理映射到基准测试地址且连接失败时重新解析；TLS 仍校验原官网域名。
    use std::net::{Ipv4Addr,SocketAddr,ToSocketAddrs};
    let host=s.origin.trim_start_matches("https://");
    let mapped=(host,443).to_socket_addrs().ok().is_some_and(|mut addresses|addresses.any(|a|matches!(a,SocketAddr::V4(v) if v.ip().octets()[0]==198 && matches!(v.ip().octets()[1],18|19))));
    if !mapped{return result;}
    let lookup=format!("https://dns.alidns.com/resolve?name={host}&type=A");
    let dns=agent.get(&lookup).call().map_err(|_|Failure::Transport)?;
    let data:Value=serde_json::from_reader(dns.into_reader().take(65536)).map_err(|_|Failure::Transport)?;
    let ips:Vec<_>=data["Answer"].as_array().into_iter().flatten().filter(|r|r["type"]==1).filter_map(|r|r["data"].as_str()?.parse::<Ipv4Addr>().ok()).map(|ip|SocketAddr::from((ip,443))).collect();
    if ips.is_empty(){return result;}
    let endpoint=format!("{host}:443");
    let direct=ureq::AgentBuilder::new().try_proxy_from_env(false).timeout(Duration::from_secs(12)).redirects(0)
        .resolver(move |name:&str|if name==endpoint{Ok(ips.clone())}else{Err(std::io::Error::other("官网地址不匹配"))}).build();
    verified(&direct,s,id)
}
pub fn fetch(agent:&ureq::Agent,account:&Account)->Result<Snapshot,(String,u64)>{
    let id=account.slot.as_deref().ok_or(("请连接 WorkBuddy 浏览器账号".into(),0))?;
    let path=session_path(id).map_err(|e|(e,0))?;
    match std::fs::read(path){
        Ok(bytes)=>{
            let raw=super::gemini::protect(&bytes,false).map_err(|e|(e,30))?;
            let session:Session=serde_json::from_slice(&raw).map_err(|_|("保存的 WorkBuddy 会话无效，请重新连接".into(),0))?;
            match query(agent,&session,id){
                Ok(snapshot)=>return Ok(snapshot),
                Err(Failure::Expired)=>{},
                Err(e)=>return Err((e.message(),30))
            }
        },
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
        Err(_)=>return Err(("无法读取已保存的 WorkBuddy 会话".into(),30))
    }
    browser_snapshot(Some(id)).map(|(_,s)|s).map_err(|e|(e,30))
}
fn bridge(agent:&ureq::Agent,action:&str,args:Value)->Result<Value,String>{
    let response=agent.post("http://127.0.0.1:10086/command").set("Content-Type","application/json").send_string(&json!({"action":action,"args":args,"session":"daozai-workbuddy"}).to_string()).map_err(|_|if action=="list_browser_tabs"{"浏览器扩展未响应，等待恢复连接"}else{"浏览器会话响应超时，稍后重试"})?;
    let mut raw=String::new();response.into_reader().take(2*1024*1024).read_to_string(&mut raw).map_err(|_|"读取浏览器会话失败")?;
    let value:Value=serde_json::from_str(&raw).map_err(|_|"浏览器扩展返回无效数据")?;
    if value["ok"]!=true{return Err("请检查 Kimi 浏览器扩展连接后重试".into());}Ok(value["data"].clone())
}
fn browser_snapshot(expected:Option<&str>)->Result<(String,Snapshot),String>{
    let agent=ureq::AgentBuilder::new().try_proxy_from_env(false).timeout(Duration::from_secs(25)).redirects(0).build();
    let response=bridge(&agent,"list_browser_tabs",json!({}))?;
    let mut tabs=response["tabs"].as_array().ok_or("无法读取浏览器标签页")?.iter().filter(|t|t["url"].as_str().and_then(|s|url::Url::parse(s).ok()).is_some_and(|u|valid_origin(&u.origin().ascii_serialization())&&u.path().starts_with("/profile"))).collect::<Vec<_>>();
    tabs.sort_by_key(|t|!t["active"].as_bool().unwrap_or(false));
    if expected.is_none()&&tabs.len()>1&&tabs[0]["active"]!=true{return Err("请激活要连接的 WorkBuddy 用量标签页后重试".into());}
    if tabs.is_empty(){return Err("等待打开 WorkBuddy 官网用量页".into());}
    let mut other_account=false;
    for tab in tabs{
        bridge(&agent,"attach_tab",json!({"tabId":tab["tabId"]}))?;
        let result=bridge(&agent,"evaluate",json!({"code":"JSON.stringify({uid:sessionStorage.getItem('website-userId'),enterprise:sessionStorage.getItem('profile-enterpriseId'),origin:location.origin,ua:navigator.userAgent})"}))?;
        let identity:Value=serde_json::from_str(result["value"].as_str().unwrap_or_default()).map_err(|_|"无法识别 WorkBuddy 登录账号")?;
        let Some(uid)=identity["uid"].as_str().filter(|s|!s.is_empty())else{continue;};
        let current=slot(uid);if expected.is_some_and(|id|id!=current){other_account=true;continue;}
        if identity["enterprise"].as_str().is_some_and(|s|!s.is_empty()){return Err("请切换到个人账号，企业积分暂不支持".into());}
        if !identity["origin"].as_str().is_some_and(valid_origin){return Err("请在 WorkBuddy 官网连接".into());}
        let origin=identity["origin"].as_str().unwrap();
        let cookies=bridge(&agent,"cdp",json!({"method":"Network.getCookies","params":{"urls":[format!("{origin}{SUMMARY}")]}}))?;
        let cookie=cookies["cookies"].as_array().into_iter().flatten().filter_map(|c|{
            if c["path"].as_str()!=Some("/"){return None;}
            let name=c["name"].as_str()?;
            if !matches!(name,"session"|"session_2"|"cb_login_mode"|"cb_login_realm"){return None;}
            let value=c["value"].as_str()?;if value.contains([';','\r','\n']){return None;}
            Some(format!("{name}={value}"))
        }).collect::<Vec<_>>().join("; ");
        let session=Session{origin:origin.into(),ua:identity["ua"].as_str().ok_or("无法读取浏览器版本")?.into(),cookie};
        let snapshot=query(&super::http_agent(),&session,&current).map_err(Failure::message)?;
        save_session(&current,&session)?;
        return Ok((current,snapshot));
    }
    Err(if other_account{"浏览器当前打开了其他 WorkBuddy 账号，请打开已绑定账号的用量页"}else{"等待 WorkBuddy 用量页恢复登录，请打开官网确认登录状态"}.into())
}
pub struct Connected {pub slot:String,pub label:String,pub snapshot:Snapshot}
pub struct Connect {rx:mpsc::Receiver<Result<Connected,String>>}
impl Connect {
    pub fn start(alias:String,expected:Option<String>)->Self{
        let(tx,rx)=mpsc::channel();std::thread::spawn(move||{let result=(||{
            let (slot,snapshot)=browser_snapshot(expected.as_deref())?;
            Ok(Connected{slot,label:if alias.trim().is_empty(){"WorkBuddy".into()}else{alias.trim().into()},snapshot})
        })();let _=tx.send(result);});Self{rx}
    }
    pub fn poll(&self)->Option<Result<Connected,String>>{self.rx.try_recv().ok()}
}
#[cfg(test)]mod tests{
    use super::*;use serde_json::json;
    #[test]
    #[ignore="显式连接本机浏览器 WorkBuddy 账号并保存配置"]
    fn workbuddy_live_connect_local(){
        let (slot,snapshot)=browser_snapshot(None).expect("官网查询");
        let (_,second)=browser_snapshot(Some(&slot)).expect("账号绑定刷新");assert!(second.credits.is_some());
        if std::env::var("DAOZAI_WORKBUDDY_SAVE_ACCOUNT").as_deref()==Ok("1"){let mut config=super::super::Config::load();config.connect_workbuddy(slot,"WorkBuddy · 个人".into());config.save();}
        println!("官网查询成功，可用积分={}，分类={}，账号绑定刷新已验证",snapshot.credits.as_ref().unwrap().remaining,snapshot.credits.as_ref().unwrap().packages.len());
    }
    #[test]
    #[ignore="只使用已保存的官网会话验证积分，不连接浏览器"]
    fn workbuddy_saved_session_without_browser(){
        let config=super::super::Config::load();
        let a=config.accounts.iter().find(|a|a.enabled&&a.provider==super::super::Provider::WorkBuddy).unwrap();
        let id=a.slot.as_deref().unwrap();
        let bytes=std::fs::read(session_path(id).unwrap()).unwrap();
        let s:Session=serde_json::from_slice(&super::super::gemini::protect(&bytes,false).unwrap()).unwrap();
        let snapshot=query(&super::super::http_agent(),&s,id).map_err(Failure::message).unwrap();
        assert!(snapshot.credits.is_some());
    }
    #[test]fn workbuddy_sums_valid_packages_and_rejects_missing_balance(){
        let s=parse(&json!({"code":0,"data":{"Packages":[{"PackageCode":"base","CycleRemainCapacity":"1200"},{"PackageCode":"gift","CycleRemainCapacity":80}]}})).unwrap();let c=s.credits.unwrap();assert_eq!(c.remaining,1280.0);assert_eq!(c.compact(),"1.28k");assert_eq!(c.full(),"1,280");
        for v in [json!({"code":0}),json!({"code":401}),json!({"code":0,"data":{"Packages":[{}]}}),json!({"code":0,"data":{"Packages":[{"CycleRemainCapacity":-1}]}})]{assert!(parse(&v).is_err());}
        assert_eq!(parse(&json!({"code":0,"data":{"Packages":[]}})).unwrap().credits.unwrap().remaining,0.0);
    }
}
