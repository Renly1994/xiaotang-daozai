//! 「其他中转站」：用户只填站点地址和凭据，程序按已知格式自动探测余额接口。
//! 命中后把格式与地址记进账号的 slot，后续按正确方式直接查询，不再重复探测。
use super::{Account,Snapshot,Window};
use serde_json::Value;
use std::{io::Read,sync::mpsc};

/// 凭据类型由用户明确选择，程序不猜。
#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub enum Kind { #[default] Key, Session }
impl Kind {
    pub const ALL:[Self;2]=[Self::Key,Self::Session];
    pub fn name(self)->&'static str{match self{Self::Key=>"API Key",Self::Session=>"网页登录令牌"}}
    fn key(self)->&'static str{match self{Self::Key=>"key",Self::Session=>"session"}}
    fn parse(s:&str)->Self{if s.eq_ignore_ascii_case("session")||s.eq_ignore_ascii_case("cookie"){Self::Session}else{Self::Key}}
}

/// 已知的余额接口格式。new-api 系返回点数，需要按倍率换算。
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum Format { Usage, NewApi, OpenAi, DeepSeek, CookieSite }
impl Format {
    // 优先读账户余额类接口，再看只看单把 Key 的几类接口。
    const ALL:[Self;5]=[Self::NewApi,Self::CookieSite,Self::Usage,Self::OpenAi,Self::DeepSeek];
    fn key(self)->&'static str{match self{Self::Usage=>"usage",Self::NewApi=>"newapi",Self::OpenAi=>"openai",Self::DeepSeek=>"deepseek",Self::CookieSite=>"cookie"}}
    fn parse(s:&str)->Option<Self>{match s{"usage"=>Some(Self::Usage),"newapi"=>Some(Self::NewApi),"openai"=>Some(Self::OpenAi),"deepseek"=>Some(Self::DeepSeek),"cookie"=>Some(Self::CookieSite),_=>None}}
    fn path(self)->&'static str{
        match self{
            Self::Usage=>"/v1/usage?days=30&timezone=UTC",
            Self::NewApi|Self::CookieSite=>"/api/user/self",
            Self::OpenAi=>"/dashboard/billing/subscription",
            Self::DeepSeek=>"/user/balance",
        }
    }
    /// 部分站点用「会话 Cookie + 数字用户 ID」认证，与 Bearer 完全不同。
    fn cookie_auth(self)->bool{self==Self::CookieSite}
}
/// new-api / one-api 默认 500000 点 = 1 美元。
pub const DEFAULT_RATE:f64=500_000.0;

/// 站点地址只保存 origin（协议 + 主机 + 可选端口），用户把整条 URL 粘进来也能用。
pub fn origin(raw:&str)->Result<String,String>{
    let raw=raw.trim();
    if raw.is_empty(){return Err("请填写站点地址".into());}
    if raw.len()>2048||raw.chars().any(char::is_whitespace){return Err("站点地址格式不正确".into());}
    let with_scheme=if raw.contains("://"){raw.to_owned()}else{format!("https://{raw}")};
    let url=url::Url::parse(&with_scheme).map_err(|_|"站点地址格式不正确".to_string())?;
    if !matches!(url.scheme(),"http"|"https"){return Err("站点地址只能是 http 或 https".into());}
    let host=url.host_str().filter(|h|!h.is_empty()).ok_or("站点地址缺少域名")?;
    if !url.username().is_empty()||url.password().is_some(){return Err("站点地址不能包含账号密码".into());}
    let port=url.port().map(|p|format!(":{p}")).unwrap_or_default();
    Ok(format!("{}://{host}{port}",url.scheme()))
}
fn secret_value(raw:&str)->Result<String,String>{
    let raw=raw.trim();
    if raw.is_empty()||raw.len()>32768||!raw.is_ascii()||raw.bytes().any(|b|b<32||b==127){return Err("请粘贴有效的凭据".into());}
    let raw=raw.strip_prefix("Bearer ").or_else(||raw.strip_prefix("bearer ")).unwrap_or(raw).trim();
    if raw.is_empty()||raw.chars().any(char::is_whitespace){return Err("凭据不能包含空白字符".into());}
    Ok(raw.into())
}
/// Cookie 型站点要「会话 Cookie + 数字用户 ID」。用户贴整条 cURL 时一并解析出来。
struct CookieCredential {header:String,user:String}
fn cookie_credential(kind:Kind,raw:&str)->Result<CookieCredential,String>{
    let raw=raw.trim();
    if raw.is_empty()||raw.len()>65536||!raw.is_ascii()||raw.bytes().any(|b|b<32&&b!=b'\n'&&b!=b'\r'||b==127){return Err("请粘贴有效的 Cookie 或 cURL".into());}
    if kind==Kind::Key{return Err("该站点需要网页会话，请把凭据类型改为网页登录令牌".into());}
    // 整条 cURL 里能同时拿到 Cookie 和用户 ID 头。
    let user=header_value(raw,"x-modelflare-user");
    // cURL 用 -b/--cookie 传 Cookie，普通文本则可能是 "Cookie: ..." 或裸 Cookie。
    let header=header_value(raw,"Cookie")
        .or_else(||curl_flag(raw,"-b").or_else(||curl_flag(raw,"--cookie")))
        .unwrap_or_else(||raw.strip_prefix("Cookie:").or_else(||raw.strip_prefix("cookie:")).unwrap_or(raw).trim().to_owned());
    let header=header.trim();
    if header.is_empty()||header.len()>16384{return Err("请粘贴有效的 Cookie 请求头".into());}
    if !header.split(';').filter_map(|part|part.trim().split_once('=')).any(|(name,_)|name.eq_ignore_ascii_case("session")){return Err("Cookie 中没有 session，请重新复制".into());}
    Ok(CookieCredential{header:header.into(),user:user.unwrap_or_default()})
}
/// 从 cURL 里取 -b/--cookie 后面的值。Cookie 含空格，必须成对引号整块取出。
fn curl_flag(raw:&str,flag:&str)->Option<String>{
    for quote in ['\'','"']{
        let pattern=format!("{flag} {quote}");
        if let Some(start)=raw.find(&pattern){
            let rest=&raw[start+pattern.len()..];
            if let Some(end)=rest.find(quote){return Some(rest[..end].to_owned());}
        }
    }
    None
}
/// 从 cURL 或请求头文本中取某个头；找不到时返回 None。
fn header_value(raw:&str,name:&str)->Option<String>{
    for line in raw.lines(){
        let line=line.trim();
        let line=line.strip_prefix("-H ").or_else(||line.strip_prefix("--header ")).unwrap_or(line).trim();
        let line=line.trim_matches(|c|c=='\''||c=='"');
        if let Some((key,value))=line.split_once(':'){if key.trim().eq_ignore_ascii_case(name){return Some(value.trim().trim_matches(|c|c=='\''||c=='"').to_owned());}}
    }
    None
}
fn slot(origin:&str,kind:Kind,secret:&str,format:Format)->String{
    let digest=ring::digest::digest(&ring::digest::SHA256,format!("{origin}\n{}\n{secret}",kind.key()).as_bytes());
    let hash=digest.as_ref().iter().map(|b|format!("{b:02x}")).collect::<String>();
    format!("{}:{hash}",format.key())
}
fn parse_slot(slot:&str)->Option<(Format,String)>{
    let(format,hash)=slot.split_once(':')?;
    if hash.len()!=64||!hash.bytes().all(|b|b.is_ascii_hexdigit()){return None;}
    Some((Format::parse(format)?,hash.to_owned()))
}
fn session_path(slot:&str)->Result<std::path::PathBuf,String>{
    let(_,hash)=parse_slot(slot).ok_or("账号信息已损坏，请重新连接")?;
    Ok(crate::model::config_dir().join("credentials").join(format!("other-{hash}.bin")))
}
#[derive(serde::Serialize,serde::Deserialize)]
struct Session {origin:String,kind:String,secret:String,#[serde(default)]rate:f64,#[serde(default)]symbol:String,#[serde(default)]user:String}
fn save(origin:&str,kind:Kind,secret:&str,user:&str,format:Format,rate:f64,symbol:&str)->Result<String,String>{
    let slot=slot(origin,kind,secret,format);let path=session_path(&slot)?;
    let raw=serde_json::to_vec(&Session{origin:origin.into(),kind:kind.key().into(),secret:secret.into(),rate:rate_or_default(rate),symbol:symbol.into(),user:user.into()}).map_err(|_|"无法保存站点凭据")?;
    let encrypted=super::gemini::protect(&raw,true)?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|_|"无法保存站点凭据")?;
    crate::model::atomic_write(&path,&encrypted).map_err(|_|"无法保存站点凭据")?;Ok(slot)
}
fn rate_or_default(rate:f64)->f64{if rate.is_finite()&&rate>0.0{rate}else{DEFAULT_RATE}}
struct Loaded {origin:String,kind:Kind,secret:String,user:String,rate:f64,format:Format,symbol:String}
fn load(account:&Account)->Result<Loaded,String>{
    let slot=account.slot.as_deref().ok_or("请重新连接站点")?;let(format,_)=parse_slot(slot).ok_or("账号信息已损坏，请重新连接")?;
    let path=session_path(slot)?;let sealed=std::fs::read(path).map_err(|_|"未找到已保存的站点凭据，请重新连接")?;
    let raw=super::gemini::protect(&sealed,false)?;
    let session:Session=serde_json::from_slice(&raw).map_err(|_|"站点凭据已损坏，请重新连接")?;
    Ok(Loaded{origin:session.origin,kind:Kind::parse(&session.kind),secret:session.secret,user:session.user,rate:rate_or_default(session.rate),format,symbol:if session.symbol.is_empty(){"¥".into()}else{session.symbol}})
}
pub fn forget(account:&Account){if let Some(path)=account.slot.as_deref().and_then(|s|session_path(s).ok()){let _=std::fs::remove_file(path);}}

fn amount(v:&Value)->Option<f64>{super::number(v).filter(|n|n.is_finite()&&*n>=0.0)}
/// new-api 对「无限额度」Key 的 hard_limit_usd 恒为 1 亿，与账户真实余额无关。
const UNLIMITED:f64=100_000_000.0;
fn wallet(total:f64,symbol:&str,available:bool,plan:&str)->Snapshot{
    let currency=if symbol=="¥"{"CNY"}else{"USD"};
    let balance=super::deepseek::Balance{currency:currency.into(),total:total.max(0.0),granted:0.0,topped_up:total.max(0.0)};
    Snapshot{plan:plan.into(),wallet:Some(super::deepseek::Wallet{available,balances:vec![balance]}),at:chrono::Utc::now().timestamp(),..Default::default()}
}
fn code_error(value:&Value)->Option<String>{
    let code=value["code"].as_i64()?;
    (code!=0).then(||value["message"].as_str().unwrap_or("站点返回了错误").to_owned())
}
/// sub2api 系 /v1/usage：钱包余额、套餐配额或 5h/1d/7d 限速窗口。
fn parse_usage(value:&Value)->Result<Snapshot,String>{
    let data=if value["data"].is_object(){&value["data"]}else{value};
    let mode=data["mode"].as_str().unwrap_or_default();
    let status=data["status"].as_str().unwrap_or_default();
    let valid=data["isValid"].as_bool().unwrap_or(true);
    let mut windows=Vec::new();
    if let Some(rows)=data["rate_limits"].as_array(){
        for row in rows{
            let key=row["window"].as_str().unwrap_or_default();
            let(label,seconds)=match key{"5h"=>("5 小时",Some(18000i64)),"1d"=>("今日",Some(86400i64)),"7d"=>("本周",Some(604800i64)),other if !other.is_empty()=>(other,None),_=>continue};
            let(Some(used),Some(limit))=(super::number(&row["used"]),super::number(&row["limit"]))else{continue};
            if limit<=0.0{continue;}
            windows.push(Window{label:label.into(),seconds,remaining:(100.0-used/limit*100.0).clamp(0.0,100.0) as f32,reset:super::reset(&row["reset_at"])});
        }
    }
    let total=if mode=="quota_limited"{
        let quota=&data["quota"];
        super::number(&quota["remaining"]).or_else(||super::number(&quota["limit"]).zip(super::number(&quota["used"])).map(|(l,u)|l-u)).unwrap_or(0.0)
    }else if data["subscription"].is_object(){
        let sub=&data["subscription"];
        super::number(&sub["monthly_limit_usd"]).unwrap_or(0.0)-super::number(&sub["monthly_usage_usd"]).unwrap_or(0.0)
    }else{
        super::number(&data["balance"]).ok_or("站点没有返回可用的余额")?
    };
    if !total.is_finite()||total < -0.000_001{return Err("余额字段无效，请稍后刷新".into());}
    let total=total.max(0.0);
    let available=valid && status!="expired" && total>0.000_001;
    let balance=super::deepseek::Balance{currency:"CNY".into(),total,granted:0.0,topped_up:total};
    Ok(Snapshot{plan:data["planName"].as_str().unwrap_or_default().into(),wallet:Some(super::deepseek::Wallet{available,balances:vec![balance]}),windows,at:chrono::Utc::now().timestamp(),..Default::default()})
}
/// new-api 的 hard_limit_usd 是「总额度」（剩余 + 已用），必须减去已用才是余额。
fn openai_total(value:&Value)->Option<f64>{
    let data=if value["data"].is_object(){&value["data"]}else{value};
    amount(&data["hard_limit_usd"]).or_else(||amount(&data["soft_limit_usd"])).or_else(||amount(&data["system_hard_limit_usd"]))
}
/// /dashboard/billing/usage 的 total_usage 是美分。
fn openai_used(value:&Value)->f64{amount(&value["total_usage"]).map(|cents|cents/100.0).unwrap_or(0.0)}
fn parse_openai(subscription:&Value,used:f64,symbol:&str)->Result<Snapshot,String>{
    if let Some(error)=code_error(subscription){return Err(error);}
    let total=openai_total(subscription).ok_or("站点没有返回可用的余额")?;
    if (total-UNLIMITED).abs()<0.5{
        // 无限额度 Key 查不到账户余额，只能提示改用网页会话。
        return Ok(wallet(0.0,symbol,false,"无限额度 · 改用网页登录令牌查看账户余额"));
    }
    let remaining=total-used;
    let available=remaining>0.000_001;
    Ok(wallet(remaining,symbol,available,if available{""}else{"额度已用完"}))
}
/// 按已确定的格式解析单条响应；OpenAI 格式的已用量由调用方另行请求。
pub fn parse(format:Format,rate:f64,symbol:&str,value:&Value)->Result<Snapshot,String>{
    if let Some(error)=code_error(value){return Err(error);}
    let data=if value["data"].is_object(){&value["data"]}else{value};
    match format{
        Format::Usage=>parse_usage(value),
        Format::DeepSeek=>super::deepseek::parse(value),
        Format::NewApi|Format::CookieSite=>{
            // /api/user/self 的 quota 是当前余额点数，按站点倍率换算成金额。
            let quota=amount(&data["quota"]).ok_or("站点没有返回可用的余额")?;
            let plan=data["display_name"].as_str().unwrap_or_default();
            Ok(wallet(quota/rate_or_default(rate),symbol,quota>0.000_001,plan))
        }
        Format::OpenAi=>{
            if openai_total(value).is_some(){return parse_openai(value,0.0,symbol);}
            // 少数实现只给 total_available（美分）。
            let usd=amount(&value["total_available"]).map(|cents|cents/100.0).ok_or("站点没有返回可用的余额")?;
            Ok(wallet(usd,symbol,usd>0.000_001,""))
        }
    }
}
/// 站点货币由用户指定：有些站点声明美元却按人民币收款。
pub fn symbol(raw:&str)->&'static str{if raw.trim()=="$"{"$"}else{"¥"}}
fn response_error(code:u16)->(String,u64,Option<u16>){
    match code{
        401|403=>(String::new(),0,Some(code)),
        429=>("请求过于频繁，稍后自动重试".into(),180,Some(code)),
        _=>(format!("站点暂不可用（{code}）"),60,Some(code))
    }
}
fn get(agent:&ureq::Agent,url:&str,secret:&str,user:&str,cookie:bool)->Result<Value,(String,u64,Option<u16>)>{
    let mut request=agent.get(url).set("Accept","application/json").set("User-Agent","Mozilla/5.0 Daozai");
    request=if cookie{
        // Cookie 型站点：会话 Cookie + 数字用户 ID。
        let request=request.set("Cookie",secret);
        if user.is_empty(){request}else{request.set("X-ModelFlare-User",user)}
    }else{
        // new-api 与各家 API Key 都走 Bearer。
        request.set("Authorization",&format!("Bearer {secret}"))
    };
    let response=request.call().map_err(|e|match e{
        ureq::Error::Status(code,_)=>response_error(code),
        _=>("无法连接该站点，请检查网络或代理".into(),30,None)
    })?;
    let mut body=String::new();response.into_reader().take(1024*1024).read_to_string(&mut body).map_err(|_|("读取站点响应失败".into(),30,None))?;
    serde_json::from_str(&body).map_err(|_|("站点没有返回 JSON 数据".into(),60,None))
}
fn http(agent:&ureq::Agent,loaded:&Loaded,path:&str,cookie:bool)->Result<Value,(String,u64)>{
    let url=format!("{}{}",loaded.origin,path);
    get(agent,&url,&loaded.secret,&loaded.user,cookie).map_err(|(message,wait,code)|{
        let message=if code.is_some_and(|c|c==401||c==403){"凭据无效，或这把凭据没有查看余额的权限".to_owned()}else{message};
        (message,wait)
    })
}
fn fetch_format(agent:&ureq::Agent,loaded:&Loaded,format:Format)->Result<Snapshot,(String,u64)>{
    let value=http(agent,loaded,format.path(),format.cookie_auth())?;
    if format==Format::OpenAi{
        // new-api 的总额度要减去已用量；取不到时不阻断，按已用 0 处理。
        let used=http(agent,loaded,"/dashboard/billing/usage",false).map(|value|openai_used(&value)).unwrap_or(0.0);
        return parse_openai(&value,used,&loaded.symbol).map_err(|e|(e,60));
    }
    // 凭据失效时接口常返回 200 + 业务错误码。
    parse(format,loaded.rate,&loaded.symbol,&value).map_err(|e|(e,60))
}
pub fn fetch(agent:&ureq::Agent,account:&Account)->Result<Snapshot,(String,u64)>{
    let loaded=load(account).map_err(|e|(e,0))?;
    fetch_format(agent,&loaded,loaded.format)
}
/// 依次尝试已知格式，命中即返回；认证错误优先于「未找到」，便于用户定位问题。
fn probe(agent:&ureq::Agent,origin:&str,secret:&str,user:&str,rate:f64,symbol:&str,cookie:bool)->Result<(Format,Snapshot),String>{
    let mut saw_auth=false;let mut saw_other=false;
    for format in Format::ALL{
        // Cookie 型格式只在用户贴了 Cookie 时尝试，避免无意义的请求。
        let use_cookie=format.cookie_auth();
        if use_cookie&&!cookie{continue;}
        if !use_cookie&&cookie{continue;}
        let value=match get(agent,&format!("{origin}{}",format.path()),secret,user,use_cookie){
            Ok(value)=>value,
            Err((_,_,Some(401|403)))=>{saw_auth=true;continue;},
            Err(_)=>{saw_other=true;continue;},
        };
        let used=if format==Format::OpenAi{
            get(agent,&format!("{origin}/dashboard/billing/usage"),secret,user,false).map(|value|openai_used(&value)).unwrap_or(0.0)
        }else{0.0};
        let parsed=if format==Format::OpenAi{parse_openai(&value,used,symbol)}else{parse(format,rate,symbol,&value)};
        match parsed{
            Ok(snapshot)=>return Ok((format,snapshot)),
            Err(_)=>saw_other=true,
        }
    }
    Err(if saw_auth{"凭据无效，或这把凭据没有查看余额的权限".into()}else if saw_other{"该站的接口格式暂不支持，可在高级里填写准确的余额端点".into()}else{"该站没有可识别的余额接口".into()})
}
pub struct Connected {pub slot:String,pub label:String,pub origin:String,pub snapshot:Snapshot}
pub struct Connect {rx:mpsc::Receiver<Result<Connected,String>>}
impl Connect {
    pub fn start(alias:String,address:String,kind:Kind,credential:String,endpoint:String,rate:String,currency:String)->Self{
        let(tx,rx)=mpsc::channel();std::thread::spawn(move||{let result=(||{
            let origin=origin(&address)?;
            let rate=rate.trim().parse::<f64>().ok().filter(|n|n.is_finite()&&*n>0.0).unwrap_or(DEFAULT_RATE);
            let symbol=symbol(&currency).to_owned();
            let agent=super::http_agent();
            // 凭据既可能是 Bearer 令牌，也可能是整条 Cookie/cURL；按内容判定，用户不必记格式。
            let has_cookie=credential.contains(';')&&credential.contains('=');
            let(cookie,secret,user)=if has_cookie{
                let parsed=cookie_credential(kind,&credential)?;
                if parsed.user.is_empty(){return Err("该站点要求用户 ID，请粘贴整条 cURL（含 x-modelflare-user 头）".into());}
                (true,parsed.header,parsed.user)
            }else{(false,secret_value(&credential)?,String::new())};
            let(format,snapshot)=match endpoint.trim(){
                ""=>probe(&agent,&origin,&secret,&user,rate,&symbol,cookie)?,
                custom=>{
                    let url=url::Url::parse(custom).map_err(|_|"自定义端点格式不正确".to_string())?;
                    if !matches!(url.scheme(),"http"|"https"){return Err("自定义端点只能是 http 或 https".into());}
                    let url=format!("{origin}{}",url.path());
                    let value=get(&agent,&url,&secret,&user,cookie).map_err(|(message,_,_)|format!("自定义端点：{message}"))?;
                    (Format::Usage,parse_usage(&value).map_err(|e|format!("自定义端点：{e}"))?)
                }
            };
            let slot=save(&origin,kind,&secret,&user,format,rate,&symbol)?;
            let label=if alias.trim().is_empty(){origin.clone()}else{alias.trim().to_owned()};
            Ok(Connected{slot,label,origin,snapshot})
        })();let _=tx.send(result);});Self{rx}
    }
    pub fn poll(&self)->Option<Result<Connected,String>>{self.rx.try_recv().ok()}
}
#[cfg(test)]
mod tests {
    use super::*;use serde_json::json;
    #[test] fn origin_keeps_only_scheme_host_and_port(){
        assert_eq!(origin("https://api.example.com/path?x=1").unwrap(),"https://api.example.com");
        assert_eq!(origin("api.example.com").unwrap(),"https://api.example.com");
        assert_eq!(origin("http://127.0.0.1:8080/v1").unwrap(),"http://127.0.0.1:8080");
        for bad in ["","ftp://example.com","https://user:pw@example.com","has space"]{assert!(origin(bad).is_err(),"{bad}");}
    }
    #[test] fn credential_strips_bearer_and_rejects_whitespace(){
        assert_eq!(secret_value("sk-abc").unwrap(),"sk-abc");
        assert_eq!(secret_value("Bearer sk-abc").unwrap(),"sk-abc");
        assert_eq!(secret_value("Bearer eyJhbGciOi.payload.sig").unwrap(),"eyJhbGciOi.payload.sig");
        for bad in ["","sk abc","sk-a\r\nX: y"]{assert!(secret_value(bad).is_err());}
    }
    #[test] fn newapi_user_self_quota_is_the_real_balance(){
        let s=parse(Format::NewApi,DEFAULT_RATE,"¥",&json!({"success":true,"data":{"id":8,"quota":5_000_000,"used_quota":0}})).unwrap();
        let b=s.wallet.unwrap().balances[0].clone();assert_eq!(b.currency,"CNY");assert_eq!(b.total,10.0);
        let s=parse(Format::NewApi,100_000.0,"$",&json!({"data":{"quota":1_000_000}})).unwrap();
        assert_eq!(s.wallet.unwrap().balances[0].total,10.0);
        assert!(parse(Format::NewApi,DEFAULT_RATE,"¥",&json!({"data":{}})).is_err());
    }
    /// 回落：hard_limit_usd 是总额度，不是余额，必须减去已用（美分）。
    #[test] fn openai_subtracts_used_and_ignores_the_unlimited_sentinel(){
        let s=parse_openai(&json!({"hard_limit_usd":1000.0}),10.0,"¥").unwrap();
        assert_eq!(s.wallet.unwrap().balances[0].total,990.0);
        // 无限额度的 Key 返回 1 亿，不能当成余额。
        let s=parse_openai(&json!({"hard_limit_usd":1e8}),0.0,"¥").unwrap();
        let wallet=s.wallet.unwrap();assert!(!wallet.available,"无限额度不能显示为可用余额");
        assert_eq!(wallet.balances[0].total,0.0);assert!(s.plan.contains("网页登录令牌"));
        // 额度用尽时明确标为不可用，而不是显示 0 元可用。
        let s=parse_openai(&json!({"hard_limit_usd":5.0}),5.0,"¥").unwrap();
        assert!(!s.wallet.unwrap().available);assert_eq!(s.plan,"额度已用完");
        assert_eq!(openai_used(&json!({"total_usage":1234})),12.34);
        assert_eq!(openai_used(&json!({})),0.0);
    }
    #[test] fn cookie_site_quota_is_the_current_balance(){
        let s=parse(Format::CookieSite,DEFAULT_RATE,"¥",&json!({"success":true,"data":{"id":221946,"display_name":"Sen Tony","quota":125000,"used_quota":0}})).unwrap();
        let wallet=s.wallet.unwrap();assert!(wallet.available);
        let b=wallet.balances[0].clone();assert_eq!(b.currency,"CNY");assert_eq!((b.total*100.0).round()/100.0,0.25);
        assert_eq!(s.plan,"Sen Tony");
        assert!(!parse(Format::CookieSite,DEFAULT_RATE,"¥",&json!({"data":{"quota":0}})).unwrap().wallet.unwrap().available);
    }
    #[test] fn cookie_credential_reads_a_pasted_curl(){
        let curl=r#"curl --url 'https://example.com/api/user/self' \
  -b 'mf_security_device=abc; session=xyz; mf_recaptcha_account=q' \
  -H 'x-interface-language: zh' \
  -H 'x-modelflare-user: 221946'"#;
        let parsed=cookie_credential(Kind::Session,curl).unwrap();
        assert_eq!(parsed.user,"221946");
        assert!(parsed.header.contains("session=xyz"));
        assert!(!parsed.header.to_lowercase().contains("x-modelflare-user"));
        // 直接粘整条 Cookie 也接受，用户 ID 由调用方另行补齐。
        let parsed=cookie_credential(Kind::Session,"Cookie: session=xyz; a=b").unwrap();
        assert!(parsed.user.is_empty());assert_eq!(parsed.header,"session=xyz; a=b");
        assert!(cookie_credential(Kind::Session,"a=b").is_err(),"缺少 session 时应拒绝");
        assert!(cookie_credential(Kind::Key,"session=xyz").is_err(),"API Key 不能当 Cookie 用");
        assert_eq!(header_value("-H 'X-ModelFlare-User: 42'","x-modelflare-user").as_deref(),Some("42"));
        assert!(header_value("no headers here","x-modelflare-user").is_none());
    }
    #[test] fn symbol_defaults_to_cny_and_accepts_dollar(){
        assert_eq!(symbol(""),"¥");assert_eq!(symbol("¥"),"¥");assert_eq!(symbol("$"),"$");
    }
    #[test] fn sub2api_wallet_and_quota_modes(){
        let s=parse(Format::Usage,DEFAULT_RATE,"¥",&json!({"mode":"balance","balance":"7.25"})).unwrap();
        assert_eq!(s.wallet.unwrap().balances[0].total,7.25);
        let s=parse(Format::Usage,DEFAULT_RATE,"¥",&json!({"mode":"quota_limited","isValid":true,"status":"active","quota":{"limit":100,"used":25,"remaining":75},"rate_limits":[{"window":"5h","limit":10,"used":2}]})).unwrap();
        assert_eq!(s.wallet.as_ref().unwrap().balances[0].total,75.0);assert_eq!(s.windows.len(),1);
    }
    #[test] fn deepseek_format_is_supported(){
        let s=parse(Format::DeepSeek,DEFAULT_RATE,"¥",&json!({"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":"1.5","granted_balance":"0","topped_up_balance":"1.5"}]})).unwrap();
        assert_eq!(s.wallet.unwrap().balances[0].total,1.5);
        let s=parse(Format::OpenAi,DEFAULT_RATE,"$",&json!({"total_available":1234})).unwrap();
        assert_eq!(s.wallet.unwrap().balances[0].total,12.34);
    }
    #[test] fn business_error_code_is_reported_without_leaking_json(){
        let error=parse(Format::NewApi,DEFAULT_RATE,"¥",&json!({"code":-1,"message":"token 无效"})).unwrap_err();
        assert_eq!(error,"token 无效");
    }
    #[test] fn slot_roundtrip_and_path_guard(){
        let slot=slot("https://a.example",Kind::Key,"sk-abc",Format::NewApi);
        assert!(slot.starts_with("newapi:"));
        let(format,hash)=parse_slot(&slot).unwrap();assert_eq!(format,Format::NewApi);assert_eq!(hash.len(),64);
        assert_ne!(slot,self::slot("https://a.example",Kind::Key,"sk-abd",Format::NewApi));
        assert!(session_path("../../other").is_err());
        assert!(parse_slot("unknown:abc").is_none());
    }
    #[test] fn every_format_has_an_endpoint(){
        for format in Format::ALL{assert!(format.path().starts_with('/'));}
    }
}