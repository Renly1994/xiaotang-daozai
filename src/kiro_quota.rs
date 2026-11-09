//! Kiro 额度：读取 Kiro 桌面版的 AWS 登录缓存，调用 GetUsageLimits 查询套餐额度。
//! 登录文件只读，不回写、不自动续期；令牌过期时提示在 Kiro 中重新登录。
use super::{Account, Amount, Snapshot, Window};
use serde_json::{json, Value};
use std::{io::Read, path::{Path, PathBuf}};

pub(crate) fn path()->Option<PathBuf>{
    Some(PathBuf::from(std::env::var_os("USERPROFILE")?).join(".aws/sso/cache/kiro-auth-token.json"))
}

struct Login { access_token:String, profile_arn:Option<String>, region:String, slot:String }
fn slot_for(seed:&str)->String{ring::digest::digest(&ring::digest::SHA256,seed.as_bytes()).as_ref().iter().map(|b|format!("{b:02x}")).take(16).collect()}
fn login(root:&Value)->Result<Login,String>{
    let token=root["accessToken"].as_str().filter(|s|!s.trim().is_empty()).ok_or("未找到 Kiro 登录凭据，请先在 Kiro 中登录")?.to_owned();
    let profile_arn=root["profileArn"].as_str().filter(|s|!s.is_empty()).map(str::to_owned);
    let region=root["region"].as_str().filter(|s|!s.is_empty()).unwrap_or("us-east-1").to_owned();
    // profileArn 与企业 SSO 的 clientIdHash 都不随令牌续期变化；只有社交登录缺这两项时才退回令牌哈希。
    let seed=profile_arn.as_deref()
        .or_else(||root["clientIdHash"].as_str().filter(|s|!s.is_empty()))
        .unwrap_or(&token);
    let slot=slot_for(seed);
    Ok(Login{access_token:token,profile_arn,region,slot})
}
fn expired(root:&Value)->bool{
    root["expiresAt"].as_str()
        .and_then(|s|chrono::DateTime::parse_from_rfc3339(s).ok())
        .is_some_and(|d|d.with_timezone(&chrono::Utc)<=chrono::Utc::now())
}
fn endpoint(region:&str)->String{
    match region{
        "us-east-1"=>"https://codewhisperer.us-east-1.amazonaws.com/".into(),
        "eu-central-1"=>"https://q.eu-central-1.amazonaws.com/".into(),
        other=>format!("https://codewhisperer.{other}.amazonaws.com/"),
    }
}
// IAM Identity Center / 企业 SSO 账号没有 profileArn，服务端按令牌识别，传空对象即可。
fn body(profile_arn:Option<&str>)->Value{
    match profile_arn.filter(|arn|!arn.trim().is_empty()){Some(arn)=>json!({"profileArn":arn}),None=>json!({})}
}
pub(crate) fn add(config:&mut super::Config,path:&Path,alias:&str)->Result<(),String>{
    let l=login(&super::login_json(path)?)?;
    if config.accounts.iter().any(|a|a.provider==super::Provider::Kiro&&a.slot.as_ref()==Some(&l.slot)){return Err("这个 Kiro 账号已经添加".into());}
    let next=config.accounts.iter().filter(|a|a.provider==super::Provider::Kiro).count()+1;
    let path=std::fs::canonicalize(path).unwrap_or_else(|_|path.to_owned());
    config.accounts.push(Account{id:format!("Kiro-{}",l.slot),provider:super::Provider::Kiro,
        alias:if alias.trim().is_empty(){format!("Kiro · 账号 {next}")}else{alias.trim().into()},
        source:path,slot:Some(l.slot),enabled:true,pinned:config.accounts.is_empty()});
    if config.selected.is_empty(){config.selected=config.accounts.last().map(|a|a.id.clone()).unwrap_or_default();}
    Ok(())
}
pub(super) fn fetch(agent:&ureq::Agent,a:&Account)->Result<Snapshot,String>{
    let root=super::login_json(&a.source)?;
    let l=login(&root)?;
    if a.slot.as_ref()!=Some(&l.slot){return Err("Kiro 已切换账号，请重新识别本机账号".into());}
    if expired(&root){return Err("Kiro 登录已过期，请打开 Kiro 重新登录后刷新额度".into());}
    let response=agent.post(&endpoint(&l.region))
        .set("Content-Type","application/x-amz-json-1.0")
        .set("X-Amz-Target","AmazonCodeWhispererService.GetUsageLimits")
        .set("Authorization",&format!("Bearer {}",l.access_token))
        .set("Accept","application/json")
        .send_string(&body(l.profile_arn.as_deref()).to_string())
        .map_err(|e|match e{
            ureq::Error::Status(401|403,_)=>"Kiro 授权被拒绝，请在 Kiro 中重新登录".into(),
            ureq::Error::Status(code,_)=>format!("Kiro 额度服务返回 {code}"),
            _=>"无法连接 Kiro 额度服务，请检查网络或代理".into(),
        })?;
    let v:Value=serde_json::from_reader(response.into_reader().take(1024*1024)).map_err(|_|"Kiro 额度响应无效".to_string())?;
    parse(&v)
}
// GetUsageLimits 返回的 currentUsageWithPrecision 是含超额的总用量；bonuses 非空时套餐与赠送积分无法拆分。
pub(super) fn parse(v:&Value)->Result<Snapshot,String>{
    let plan=v["subscriptionInfo"]["subscriptionTitle"].as_str().filter(|s|!s.trim().is_empty()).map(str::to_owned).unwrap_or_else(||"Kiro".into());
    let mut snapshot=Snapshot{plan,at:chrono::Utc::now().timestamp(),..Default::default()};
    let breakdown=v["usageBreakdownList"].as_array().ok_or("Kiro 未返回用量明细")?;
    let credit=breakdown.iter().find(|e|e["resourceType"].as_str()==Some("CREDIT")).ok_or("Kiro 未返回套餐额度")?;
    let limit=super::number(&credit["usageLimitWithPrecision"]).ok_or("Kiro 套餐额度字段无效")?;
    let used=super::number(&credit["currentUsageWithPrecision"]).ok_or("Kiro 用量字段无效")?;
    let overage=super::number(&credit["currentOveragesWithPrecision"]).unwrap_or(0.0).max(0.0);
    let reset=credit["nextDateReset"].as_f64().or_else(||v["nextDateReset"].as_f64())
        .filter(|t|(1_000_000_000.0..=4_102_444_800.0).contains(t)).map(|t|t as i64);
    if limit>0.0{
        let has_bonus=credit["bonuses"].as_array().is_some_and(|b|!b.is_empty());
        let plan_used=if has_bonus{used.min(limit)}else{(used-overage).max(0.0)};
        let left=(limit-plan_used).max(0.0);
        snapshot.windows.push(Window{label:"月度额度".into(),seconds:None,remaining:(left/limit*100.0).clamp(0.0,100.0) as f32,reset,amount:Some(Amount::new(left,limit))});
    }
    let overage_enabled=v["overageConfiguration"]["overageStatus"].as_str().is_some_and(|s|s.eq_ignore_ascii_case("ENABLED"));
    let cap=if overage_enabled{super::number(&credit["overageCapWithPrecision"]).filter(|c|*c>0.0)}else{None};
    if let Some(cap)=cap{
        let left=(cap-overage).max(0.0);
        snapshot.windows.push(Window{label:"超额额度".into(),seconds:None,remaining:(left/cap*100.0).clamp(0.0,100.0) as f32,reset,amount:Some(Amount::new(left,cap))});
    }
    if snapshot.windows.is_empty(){return Err("Kiro 未提供可显示的套餐额度".into());}
    Ok(snapshot)
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn plan_and_overage_map_to_remaining_credits(){
        let s=parse(&json!({"usageBreakdownList":[{"resourceType":"CREDIT","currentUsageWithPrecision":130.5,"usageLimitWithPrecision":1000.0,
            "currentOveragesWithPrecision":30.5,"overageCapWithPrecision":200.0,"nextDateReset":1900000000.0}],"overageConfiguration":{"overageStatus":"ENABLED"}})).unwrap();
        assert_eq!(s.windows.len(),2);
        assert_eq!(s.windows[0].label,"月度额度");assert_eq!(s.windows[0].remaining,90.0);assert_eq!(s.windows[0].reset,Some(1900000000));
        assert_eq!(s.windows[0].amount,Some(Amount::new(900.0,1000.0)));
        assert_eq!(s.windows[0].headline(),"900");assert_eq!(s.windows[0].unit(),"点");
        assert_eq!(s.windows[0].summary(),"月度额度剩余 900 / 1000");
        assert_eq!(s.windows[1].label,"超额额度");assert_eq!(s.windows[1].remaining,84.75);
        assert_eq!(s.windows[1].amount,Some(Amount::new(169.5,200.0)));assert_eq!(s.windows[1].summary(),"超额额度剩余 169.5 / 200");
    }
    #[test]fn amounts_keep_one_decimal_and_never_round_past_the_limit(){
        assert_eq!(Amount::new(904.09,1000.0).full(),"904.1 / 1000");
        assert_eq!(Amount::new(904.09,1000.0).short(),"904");
        assert_eq!(Amount::new(95.91,1000.0).short(),"95.9");
        assert_eq!(Amount::new(999.99,1000.0).full(),"1000 / 1000");
        assert_eq!(Amount::new(-4.0,10.0).full(),"0 / 10");
        assert_eq!(Amount::new(0.0,0.0).full(),"0 / 0");
    }
    #[test]fn bonus_usage_cannot_exceed_the_plan_and_missing_data_is_an_error(){
        let s=parse(&json!({"usageBreakdownList":[{"resourceType":"CREDIT","currentUsageWithPrecision":1200.0,"usageLimitWithPrecision":1000.0,
            "bonuses":[{"credits":1.0}],"nextDateReset":1900000000.0}]})).unwrap();
        assert_eq!(s.windows.len(),1);assert_eq!(s.windows[0].remaining,0.0);
        assert!(parse(&json!({"usageBreakdownList":[]})).is_err());
        assert!(parse(&json!({})).is_err());
    }
    #[test]fn token_expiry_is_detected(){
        assert!(expired(&json!({"accessToken":"a","expiresAt":"2001-01-01T00:00:00Z"})));
        assert!(!expired(&json!({"accessToken":"a","expiresAt":"2099-01-01T00:00:00Z"})));
        assert!(!expired(&json!({"accessToken":"a"})));
    }
    #[test]fn endpoint_follows_the_profile_region(){
        assert_eq!(endpoint("us-east-1"),"https://codewhisperer.us-east-1.amazonaws.com/");
        assert_eq!(endpoint("eu-central-1"),"https://q.eu-central-1.amazonaws.com/");
        assert_eq!(endpoint("ap-northeast-1"),"https://codewhisperer.ap-northeast-1.amazonaws.com/");
    }
    #[test]fn plan_title_and_idc_body(){
        assert_eq!(body(None),json!({}));assert_eq!(body(Some("  ")),json!({}));
        assert_eq!(body(Some("arn:aws:codewhisperer:us-east-1:1:profile/x")),json!({"profileArn":"arn:aws:codewhisperer:us-east-1:1:profile/x"}));
        let s=parse(&json!({"subscriptionInfo":{"subscriptionTitle":"KIRO PRO"},"usageBreakdownList":[{"resourceType":"CREDIT",
            "currentUsageWithPrecision":95.91,"usageLimitWithPrecision":1000.0,"nextDateReset":1796083200.0}]})).unwrap();
        assert_eq!(s.plan,"KIRO PRO");assert!((s.windows[0].remaining-90.409).abs()<0.001);assert_eq!(s.windows[0].reset,Some(1796083200));
        assert_eq!(s.windows[0].summary(),"月度额度剩余 904.1 / 1000");
    }
    #[test]fn slot_survives_token_refresh_when_sso_identity_exists(){
        let first=login(&json!({"accessToken":"old","clientIdHash":"abc","authMethod":"IdC"})).unwrap();
        let second=login(&json!({"accessToken":"new-after-refresh","clientIdHash":"abc","authMethod":"IdC"})).unwrap();
        assert_eq!(first.slot,second.slot);
        let social=login(&json!({"accessToken":"old","profileArn":"arn:x"})).unwrap();
        let social_refreshed=login(&json!({"accessToken":"new","profileArn":"arn:x"})).unwrap();
        assert_eq!(social.slot,social_refreshed.slot);assert_ne!(social.slot,first.slot);
    }
    #[test]
    #[ignore="按需只读核验本机 Kiro 额度，不保存配置"]
    fn kiro_local_probe(){
        let mut config=super::super::Config::default();
        let path=path().expect("需要 USERPROFILE");
        if !path.is_file(){println!("未找到 Kiro 登录文件");return;}
        add(&mut config,&path,"").unwrap();
        let account=config.accounts.iter().find(|a|a.provider==super::super::Provider::Kiro).unwrap();
        match fetch(&super::super::http_agent(),account){
            Ok(s)=>println!("Kiro 套餐={} 周期={:?}",s.plan,s.windows.iter().map(|w|(w.summary(),w.reset)).collect::<Vec<_>>()),
            Err(e)=>println!("Kiro 查询状态：{e}"),
        }
    }
}
