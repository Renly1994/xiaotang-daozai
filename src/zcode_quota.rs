use super::{Account, Snapshot, Window};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::{aead,digest};
use serde_json::Value;
use std::{io::Read,path::{Path,PathBuf}};

pub(crate) fn path()->Option<PathBuf>{
    Some(PathBuf::from(std::env::var_os("USERPROFILE")?).join(".zcode/v2/credentials.json"))
}
fn value(root:&Value,name:&str)->Result<String,String>{
    let raw=root[name].as_str().ok_or("请先在 ZCode 中登录个人账号")?;
    let Some(encoded)=raw.strip_prefix("enc:v1:") else{return Ok(raw.into());};
    let secret=std::env::var("ZCODE_CREDENTIAL_SECRET").ok().filter(|s|!s.trim().is_empty()).unwrap_or_else(||format!(
        "zcode-credential-fallback:win32:{}:{}",std::env::var("USERPROFILE").unwrap_or_default(),std::env::var("USERNAME").unwrap_or_default()));
    let key=digest::digest(&digest::SHA256,secret.trim().as_bytes());
    let error="无法读取当前 Windows 用户的 ZCode 登录，请在客户端重新登录";
    let parts=encoded.split('.').map(|s|URL_SAFE_NO_PAD.decode(s)).collect::<Result<Vec<_>,_>>().map_err(|_|error)?;
    let [iv,tag,cipher]=parts.as_slice() else{return Err(error.into());};
    if tag.len()!=16{return Err(error.into());}
    let mut bytes=cipher.clone();bytes.extend_from_slice(tag);
    let key=aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM,key.as_ref()).map_err(|_|error)?);
    let plain=key.open_in_place(aead::Nonce::try_assume_unique_for_key(iv).map_err(|_|error)?,aead::Aad::empty(),&mut bytes).map_err(|_|error)?;
    String::from_utf8(plain.to_vec()).map_err(|_|error.into())
}
struct Login { family:String, slot:String, subject:String, jwt:String }
fn login(root:&Value)->Result<Login,String>{
    let family=value(root,"oauth:active_provider")?;
    if !matches!(family.as_str(),"zai"|"bigmodel"){return Err("暂不支持这个 ZCode 登录区域".into());}
    let jwt=value(root,"zcodejwttoken")?;
    let payload=jwt.split('.').nth(1).and_then(|s|URL_SAFE_NO_PAD.decode(s).ok()).and_then(|s|serde_json::from_slice::<Value>(&s).ok()).ok_or("ZCode 账号标识无效")?;
    let id=payload["sub"].as_str().or_else(||payload["user_id"].as_str()).ok_or("ZCode 未返回账号标识")?;
    let slot=digest::digest(&digest::SHA256,format!("{family}/{id}").as_bytes()).as_ref().iter().map(|b|format!("{b:02x}")).collect();
    Ok(Login{family,slot,subject:id.to_owned(),jwt})
}
pub(crate) fn add(config:&mut super::Config,path:&Path,alias:&str)->Result<(),String>{
    let l=login(&super::login_json(path)?)?;
    if config.accounts.iter().any(|a|a.provider==super::Provider::ZCode&&a.slot.as_ref()==Some(&l.slot)){return Ok(());}
    config.accounts.push(Account{id:format!("ZCode-{}",l.slot),provider:super::Provider::ZCode,alias:if alias.trim().is_empty(){"智谱 ZCode".into()}else{alias.trim().into()},source:path.to_owned(),slot:Some(l.slot),enabled:true,pinned:false});Ok(())
}
fn read(agent:&ureq::Agent,url:&str,auth:&str,device_mid:Option<&str>)->Result<Value,String>{
    let mut request=agent.get(url).set("Authorization",auth).set("Accept","application/json");
    if let Some(mid)=device_mid{request=request.set("X-Device-Mid",mid);}
    let response=request.call().map_err(|e|match e{
        ureq::Error::Status(401|403,_)=>"ZCode 授权被拒绝，请在客户端确认登录".into(),
        ureq::Error::Status(code,_)=>format!("ZCode 用量服务返回 {code}"),
        _=>"无法连接 ZCode 用量服务".into(),
    })?;
    serde_json::from_reader(response.into_reader().take(1024*1024)).map_err(|_|"ZCode 用量响应无效".into())
}
fn success(v:&Value)->bool{ matches!(v["code"].as_i64(),Some(0|200)) || (v["code"].is_null()&&v["success"]==true) }
fn coding(v:&Value)->Result<Snapshot,String>{
    let mut snapshot=Snapshot{plan:"Coding Plan".into(),at:chrono::Utc::now().timestamp(),..Default::default()};
    for row in v["data"]["limits"].as_array().into_iter().flatten(){
        let Some(used)=super::number(&row["percentage"]).filter(|v|(0.0..=100.0).contains(v)) else{continue;};
        let multiplier=match row["unit"].as_i64(){Some(1)=>Some(86400),Some(3)=>Some(3600),Some(5)=>Some(60),Some(6)=>Some(604800),_=>None};
        let seconds=row["number"].as_i64().filter(|n|*n>0).zip(multiplier).and_then(|(n,m)|n.checked_mul(m));
        let kind=row["type"].as_str().unwrap_or_default();
        let label=if kind=="TIME_LIMIT"{"工具调用".into()}else{super::cadence(seconds)};
        let reset=row["nextResetTime"].as_i64().filter(|x|*x>0).map(|x|if x>10_000_000_000{x/1000}else{x});
        let window=Window{label,seconds,remaining:(100.0-used) as f32,reset};
        match kind{"TOKENS_LIMIT"|"CREDIT_LIMIT"=>snapshot.windows.push(window),"TIME_LIMIT"=>snapshot.extra.push(window),_=>{}}
    }
    if snapshot.windows.is_empty(){std::mem::swap(&mut snapshot.windows,&mut snapshot.extra);}
    if snapshot.windows.is_empty(){return Err("ZCode 未提供可显示的套餐比例".into());}
    Ok(snapshot)
}
fn start(v:&Value)->Result<Snapshot,String>{
    let mut snapshot=Snapshot{plan:"Start Plan".into(),at:chrono::Utc::now().timestamp(),..Default::default()};
    let buckets=v["data"]["balances"].as_array().ok_or("ZCode 响应缺少体验额度明细")?;
    if buckets.is_empty(){snapshot.plan="暂无体验额度".into();return Ok(snapshot);}
    for bucket in buckets{
        let Some(total)=super::number(&bucket["total_units"]).filter(|n|*n>0.0) else{continue;};
        let Some(left)=super::number(&bucket["remaining_units"]).filter(|n|*n>=0.0) else{continue;};
        let reset=super::number(&bucket["expires_at"]).map(|n|n as i64);
        if reset.is_some_and(|t|t<=snapshot.at){continue;}
        snapshot.windows.push(Window{label:bucket["show_name"].as_str().filter(|s|!s.is_empty()).unwrap_or("体验额度").into(),seconds:None,remaining:(left/total*100.0).clamp(0.0,100.0) as f32,reset});
    }
    if snapshot.windows.is_empty(){return Err("当前 ZCode 账号暂无有效体验额度".into());}
    Ok(snapshot)
}
const APIKEY_SLOT:&str="apikey";
#[cfg(windows)]
fn credential(slot:&str)->Result<keyring::Entry,String>{keyring::Entry::new("Daozai.ZCode",slot).map_err(|e|format!("无法打开 Windows 凭据管理器：{e}"))}
#[cfg(windows)]
fn load_key(slot:&str)->Result<String,String>{credential(slot)?.get_password().map_err(|_|"没有保存的 API 密钥".into())}
#[cfg(not(windows))]
fn load_key(_slot:&str)->Result<String,String>{Err("仅 Windows 支持保存 API 密钥".into())}
/// 编码套餐直连：credentials.json 缺少 Coding Plan api-key、且体验额度为空时，
/// 用智谱开放平台 API 密钥查 open.bigmodel.cn（密钥存 Windows 凭据管理器，不写配置文件）。
fn fetch_keyring(agent:&ureq::Agent)->Result<Snapshot,String>{
    let key=load_key(APIKEY_SLOT)?;
    let stamp=|t:chrono::DateTime<chrono::Utc>|t.to_rfc3339_opts(chrono::SecondsFormat::Secs,true).replace(':',"%3A");
    let url=format!("https://open.bigmodel.cn/api/monitor/usage/quota/limit?startTime={}&endTime={}",stamp(chrono::Utc::now()-std::time::Duration::from_secs(86400)),stamp(chrono::Utc::now()));
    let v=read(agent,&url,&key,None)?;
    finalize_keyring(&v)
}
/// 网关对业务错误返回 HTTP 200 + success:false，这里统一映射为用户可读错误。
fn finalize_keyring(v:&Value)->Result<Snapshot,String>{
    if !success(&v){let message=v["msg"].as_str().or_else(||v["message"].as_str()).unwrap_or("额度服务返回错误");return Err(format!("ZCode API 密钥查询失败：{message}"));}
    let mut snapshot=coding(v)?;
    if snapshot.plan=="Coding Plan"{if let Some(level)=v["data"]["level"].as_str().filter(|s|!s.trim().is_empty()){snapshot.plan=level.to_owned();}}
    Ok(snapshot)
}
/// API 密钥独立授权：账号只存凭据管理器引用，配置文件不落明文。
pub(crate) fn connect_keyring(config:&mut super::Config,key:&str,alias:&str)->Result<(),String>{connect_keyring_slot(config,APIKEY_SLOT,key,alias)}
fn connect_keyring_slot(config:&mut super::Config,slot:&str,key:&str,alias:&str)->Result<(),String>{
    let trimmed=key.trim();
    if trimmed.len()<8{return Err("请输入完整的 API 密钥".into());}
    #[cfg(windows)]{credential(slot)?.set_password(trimmed).map_err(|e|format!("无法安全保存密钥：{e}"))?;}
    #[cfg(not(windows))] let _=(slot,trimmed);
    let existing=config.accounts.iter_mut().find(|a|a.provider==super::Provider::ZCode&&a.slot.as_deref()==Some(slot));
    if let Some(a)=existing{
        a.enabled=true;a.source=PathBuf::new();if !alias.trim().is_empty(){a.alias=alias.trim().into();}
        config.selected=a.id.clone();return Ok(());
    }
    let id=format!("ZCode-{slot}");let pinned=config.accounts.is_empty();if pinned{config.selected=id.clone();}
    let alias=if alias.trim().is_empty(){"ZCode · API 密钥".into()}else{alias.trim().into()};
    config.selected=id.clone();config.accounts.push(Account{id,provider:super::Provider::ZCode,alias,source:PathBuf::new(),slot:Some(slot.into()),enabled:true,pinned});Ok(())
}
pub(crate) fn forget(a:&Account){
    #[cfg(windows)]
    if a.slot.as_deref()==Some(APIKEY_SLOT){if let Ok(entry)=credential(APIKEY_SLOT){let _=entry.delete_credential();}}
    #[cfg(not(windows))] {let _=a;}
}
pub(super) fn fetch(agent:&ureq::Agent,a:&Account)->Result<Snapshot,String>{
    if a.source.as_os_str().is_empty(){return fetch_keyring(agent);}
    let root=super::login_json(&a.source)?;let l=login(&root)?;
    if a.slot.as_ref()!=Some(&l.slot){return Err("ZCode 已切换账号，请重新识别本机账号".into());}
    let key=format!("account-provider:coding-plan:account:{}-individual-coding-plan:account:{}:api-key",l.family,l.subject);
    let mut no_coding_plan=false;
    if root.get(&key).is_some(){
        let url=if l.family=="zai"{"https://api.z.ai/api/monitor/usage/quota/limit"}else{"https://bigmodel.cn/api/monitor/usage/quota/limit"};
        let v=read(agent,url,&value(&root,&key)?,None)?;
        if success(&v){return coding(&v);}
        let msg=v["msg"].as_str().or_else(||v["message"].as_str()).unwrap_or("").to_lowercase();
        if !(msg.contains("不存在coding plan")||msg.contains("不存在 coding plan")||msg.contains("no coding plan")){return Err(format!("ZCode 编程套餐查询失败（{}）",v["code"]));}
        no_coding_plan=true;
    }
    let mut url=url::Url::parse("https://zcode.z.ai/api/v1/zcode-plan/billing/balance").unwrap();
    // 客户端按版本发布运行时，官网使用 app_version 判断体验套餐能力。
    if let Some(runtime)=a.source.parent().map(|p|p.join("runtime/provider/windows-x86_64")){
        let versions=std::fs::read_dir(runtime).into_iter().flatten().flatten().filter_map(|dir|{
            let name=dir.file_name().into_string().ok()?;
            let parts=name.split('.').map(str::parse::<u32>).collect::<Result<Vec<_>,_>>().ok()?;
            (parts.len()==3).then_some((parts,name))
        });
        if let Some((_,v))=versions.max(){url.query_pairs_mut().append_pair("app_version",&v);}
    }
    let telemetry=a.source.with_file_name("telemetry-state.json");
    let device=super::login_json(&telemetry).ok();
    let mid=device.as_ref().and_then(|v|v["deviceMid"].as_str()).filter(|s|!s.is_empty() && s.bytes().all(|b|b.is_ascii_graphic())).ok_or("请打开 ZCode，等待客户端初始化设备信息")?;
    let v=read(agent,url.as_str(),&format!("Bearer {}",l.jwt),Some(mid)).map_err(|e|if no_coding_plan{format!("未开通 Coding Plan；体验套餐查询失败：{e}")}else{e})?;
    if !success(&v){return Err(format!("ZCode 体验额度暂不可用（{}）",v["code"]));}
    start(&v)
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn empty_entitlements_are_valid_but_missing_data_is_an_error(){
        assert!(start(&serde_json::json!({"data":{"balances":[]}})).unwrap().windows.is_empty());
        assert!(start(&serde_json::json!({"data":{}})).is_err());
    }
    #[test]fn zcode_distinguishes_used_percentage_and_remaining_units(){
        let c=coding(&serde_json::json!({"data":{"limits":[{"type":"TOKENS_LIMIT","percentage":31,"unit":3,"number":5,"nextResetTime":1900000000000i64}]}})).unwrap();
        assert_eq!(c.windows[0].remaining,69.0);assert_eq!(c.windows[0].reset,Some(1900000000));
        let s=start(&serde_json::json!({"data":{"balances":[{"total_units":"800","remaining_units":"200","available_units":"100"}]}})).unwrap();assert_eq!(s.windows[0].remaining,25.0);
    }
    #[cfg(windows)]
    #[test]fn api_key_connect_round_trips_through_windows_credentials(){
        use crate::quota::{Config,Provider};
        // 测试使用独立槽位，绝不触碰真实用户的 APIKEY_SLOT 凭据。
        const TEST_SLOT:&str="unittest";
        struct Cleanup;impl Drop for Cleanup{fn drop(&mut self){let account=Account{id:"x".into(),provider:Provider::ZCode,alias:String::new(),source:PathBuf::new(),slot:Some(TEST_SLOT.into()),enabled:true,pinned:false};forget(&account);}}
        let _cleanup=Cleanup;
        let mut config=Config::default();
        connect_keyring_slot(&mut config,TEST_SLOT,"synthetic-zcode-key-0001","备用").unwrap();
        assert_eq!(config.accounts.len(),1);assert!(config.accounts[0].source.as_os_str().is_empty());assert_eq!(config.accounts[0].alias,"备用");assert_eq!(config.selected,config.accounts[0].id);
        assert_eq!(load_key(TEST_SLOT).unwrap(),"synthetic-zcode-key-0001");
        config.accounts[0].enabled=false;
        connect_keyring_slot(&mut config,TEST_SLOT,"synthetic-zcode-key-0002","").unwrap();
        assert_eq!(config.accounts.len(),1,"重复连接不能新增账号");assert!(config.accounts[0].enabled,"重连应恢复监控");assert_eq!(config.accounts[0].alias,"备用");
        assert_eq!(load_key(TEST_SLOT).unwrap(),"synthetic-zcode-key-0002");
        assert!(!config.json().to_string().contains("synthetic-zcode-key"),"密钥不能写入配置文件");
        assert!(connect_keyring_slot(&mut config,TEST_SLOT,"短","").is_err());
        assert!(load_key("unittest-empty-slot").is_err(),"凭据失效时读取必须报错而非返回空值");
    }
    #[test]fn keyring_response_maps_periods_resets_and_credential_failure(){
        // 脱敏后的线上真实结构：HTTP 200 + CREDIT_LIMIT + unit/number + nextResetTime(毫秒)。
        let v=serde_json::json!({"code":200,"msg":"操作成功","success":true,"data":{"level":"lite","limits":[
            {"type":"CREDIT_LIMIT","unit":3,"number":5,"usage":2000,"currentValue":930,"percentage":46,"nextResetTime":1793892721234i64},
            {"type":"CREDIT_LIMIT","unit":6,"number":1,"percentage":10,"nextResetTime":1794366124964i64}]}});
        let s=finalize_keyring(&v).unwrap();
        assert_eq!(s.windows.len(),2);assert_eq!(s.rings().len(),2,"两个周期都应画出额度环");
        assert_eq!(s.rings()[0].label,"5 小时");assert_eq!(s.rings()[0].seconds,Some(18000));assert_eq!(s.rings()[0].remaining,54.0);assert_eq!(s.rings()[0].reset,Some(1793892721));
        assert_eq!(s.rings()[1].label,"本周");assert_eq!(s.rings()[1].seconds,Some(604800));assert_eq!(s.rings()[1].remaining,90.0);assert_eq!(s.rings()[1].reset,Some(1794366124));
        assert_eq!(s.plan,"lite","data.level 应回填为套餐名");
        // 凭据失效：网关返回 code 401 + success:false，必须转成可读错误而非空额度。
        let expired=finalize_keyring(&serde_json::json!({"code":401,"msg":"token expired or incorrect","success":false})).unwrap_err();
        assert!(expired.contains("API 密钥查询失败")&&expired.contains("token expired"),"凭据失效要保留服务端原因：{expired}");
        let no_msg=finalize_keyring(&serde_json::json!({"code":500,"success":false})).unwrap_err();
        assert!(no_msg.contains("额度服务返回错误"));
    }
}
