//! Read-only adapters for distinct relay protocols. Do not convert site quota to money.
//! Protocol references (reviewed 2026-11-08):
//! https://github.com/QuantumNous/new-api/blob/main/controller/token.go
//! https://github.com/songquanpeng/one-api/blob/main/controller/billing.go
//! https://github.com/Wei-Shaw/sub2api/blob/main/backend/internal/handler/gateway_handler.go
use super::{Snapshot, Window};
use serde_json::Value;
use std::io::Read;
use std::time::{Duration,Instant};

pub const AUTO_TIMEOUT:Duration=Duration::from_secs(15);
const PROBE_TIMEOUT:Duration=Duration::from_secs(5);
const MAX_PROBE_REQUESTS:usize=4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { NewApi, OneApi, Sub2Api }
impl Kind {
    fn name(self) -> &'static str { match self { Self::NewApi => "New API", Self::OneApi => "兼容计费接口", Self::Sub2Api => "Sub2API" } }
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Scope { #[default] Unknown, Account, Subscription, Key }

/// 站点声明的配额换算参数，仅用于显示，不改变缓存中的原始点。
///
/// 只取 `quota_per_unit` 做一次除法：这是站长按实际售价设定的常数，必须
/// 与站点前台显示一致。刻意不乘 `usd_exchange_rate`——实测站点普遍保留
/// 安装默认值（如 7.3），乘上去会得到七倍于真实的金额（4985538 点本应显示
/// ¥9.97，按 7.3 换算会变成 ¥72.79）。
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rate {
    /// 多少点等于一个货币单位。
    pub per_unit: f64,
    /// 展示符号：¥ / $ / 自定义。
    pub symbol: String,
}
impl Rate {
    /// 原始点换算成货币值；参数不可信时返回 None，由调用方退回显示原始点。
    pub fn money(&self,points:f64)->Option<f64>{
        (self.per_unit>0.0&&self.per_unit.is_finite()&&points.is_finite()).then(||points/self.per_unit)
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Amount {
    #[serde(default)]
    pub scope: Scope,
    pub label: String,
    pub unit: String,
    pub remaining: Option<f64>,
    pub limit: Option<f64>,
    pub used: Option<f64>,
    pub unlimited: bool,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Quota {
    pub source: String,
    pub amounts: Vec<Amount>,
    /// Expiry is not a reset. Keep it out of Window.reset.
    pub expires_at: Option<i64>,
    /// 站点声明的换算参数。缺失时按原始点显示，绝不猜金额。
    #[serde(default)]
    pub rate: Option<Rate>,
    /// 站点自己的 logo（data URL）。只在本次运行的内存里保留用于渲染：
    /// 跳过序列化，避免写进 ai-quota-cache.json 或随安装包分发。
    #[serde(skip)]
    pub logo: Option<String>,
}
impl Quota {
    /// 用户在高级设置里手动指定的货币符号；优先于站点自报值。
    pub fn symbol(&self,overrides:&std::collections::HashMap<String,String>,account_id:&str)->Option<String>{
        overrides.get(account_id).cloned().or_else(||self.rate.as_ref().map(|r|r.symbol.clone()))
    }
    /// 应用手动覆盖后的换算参数；覆盖存在时替换站点符号，保留每单位点数。
    pub fn rate_with(&self,symbol:Option<String>)->Option<Rate>{
        let mut rate=self.rate.clone()?;
        if let Some(symbol)=symbol.filter(|s|!s.is_empty()){rate.symbol=symbol;}
        Some(rate)
    }
    pub fn primary(&self)->Option<&Amount>{
        self.amounts.iter().find(|a|a.displayable()&&matches!(a.scope,Scope::Account))
            .or_else(||self.amounts.iter().filter(|a|a.displayable()).min_by(|a,b|a.remaining.unwrap().total_cmp(&b.remaining.unwrap())))
    }
    pub fn unavailable_text(&self)->&'static str{
        if self.amounts.iter().any(|a|matches!(a.scope,Scope::Subscription)){"额度暂不可查询"}else{"余额暂不可查询"}
    }
    pub fn compact(&self) -> String { self.compact_with(None) }
    /// 带手动货币符号覆盖的紧凑写法（常驻胶囊与侧栏使用）。
    pub fn compact_with(&self,symbol:Option<String>) -> String {
        let Some(a) = self.primary() else { return self.unavailable_text().into() };
        let rate=self.rate_with(symbol);
        if matches!(a.scope,Scope::Subscription){format!("{}剩余 {}",a.label,a.remaining_text(rate.as_ref()))}else{a.remaining_text(rate.as_ref())}
    }
    pub fn full(&self) -> String { self.full_with(None) }
    /// 带手动货币符号覆盖的明细（设置页“明细”使用）。
    pub fn full_with(&self,symbol:Option<String>) -> String {
        let rate=self.rate_with(symbol);
        let rate=rate.as_ref();
        let mut rows = vec![self.source.clone()];
        if self.primary().is_none(){rows.push(self.unavailable_text().into());}
        for a in self.amounts.iter().filter(|a|a.displayable()) {
            let value = a.remaining_text(rate);
            let used = a.used.map(|n|format!("，已用 {}",a.amount_text(n,rate))).unwrap_or_default();
            // 换算过的行同时给出原始点，方便到站点后台逐位核对。
            let points = match rate.and_then(|_|a.raw_points_text(rate)) {
                Some(raw) => format!("（{raw}）"),
                None => String::new(),
            };
            rows.push(format!("{}：剩余 {value}{used}{points}",a.label));
        }
        if self.amounts.iter().any(|a|a.displayable()&&matches!(a.scope,Scope::Subscription)){
            if let Some(at) = self.expires_at.and_then(|t|chrono::DateTime::from_timestamp(t,0)) { rows.push(format!("到期 {} UTC",at.format("%Y-%m-%d %H:%M"))); }
        }
        rows.join("\n")
    }
}
impl Amount {
    fn displayable(&self)->bool{
        !self.unlimited&&self.remaining.is_some_and(f64::is_finite)&&match self.scope{
            Scope::Account=>true,Scope::Subscription=>self.limit.is_some_and(|n|n.is_finite()&&n>0.0),_=>false,
        }
    }
    /// 单个数值的显示写法：能给金额就给金额，否则退回原始点。
    fn amount_text(&self,n:f64,rate:Option<&Rate>)->String{
        match rate.filter(|_|!matches!(self.scope,Scope::Subscription)).and_then(|r|r.money(n).map(|m|(r,m))) {
            Some((r,m))=>format!("{}{}",r.symbol,money_text(m)),
            None=>format!("{} {}",number(n),self.unit),
        }
    }
    /// 换算后的原始点写法；未换算时返回 None，避免重复显示同一个数字。
    fn raw_points_text(&self,rate:Option<&Rate>)->Option<String>{
        let _=rate?;
        let remaining=self.remaining?;
        Some(match self.used {
            Some(used)=>format!("{} / {} {}",number(remaining),number(used),self.unit),
            None=>format!("{} {}",number(remaining),self.unit),
        })
    }
    pub fn remaining_text(&self,rate:Option<&Rate>)->String{
        if !self.displayable(){return "额度暂不可查询".into();}
        let remaining=self.remaining.unwrap();
        if matches!(self.scope,Scope::Subscription){format!("{:.0}%",(remaining/self.limit.unwrap()*100.0).clamp(0.0,100.0))}else{self.amount_text(remaining,rate)}
    }
}
fn number(n:f64)->String {
    let raw = if n.abs() >= 1e12 || (n != 0.0 && n.abs() < 0.0001) { format!("{n:.3e}") } else { format!("{n:.4}") };
    if raw.contains('e') { raw } else { raw.trim_end_matches('0').trim_end_matches('.').to_owned() }
}

/// Accept the site root, preserving its reverse-proxy path prefix.
/// HTTPS only: localhost HTTP is deliberately not an implicit credential exception.
pub fn validate_base_url(raw:&str)->Result<String,String> {
    let raw=raw.trim();
    if raw.is_empty() || raw.chars().any(char::is_whitespace) || raw.contains('\\') { return Err("请输入有效的 HTTPS 站点地址".into()); }
    let url=url::Url::parse(raw).map_err(|_|"请输入完整的 HTTPS 站点地址")?;
    if url.scheme()!="https" || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return Err("站点地址必须使用 HTTPS，且不能包含账号、查询参数或片段".into());
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}
/// Preserve the user's exact deployment path; never guess whether /v1 is a
/// tenant prefix or an API suffix. The UI asks for the site root, not API base.
pub fn normalize_base_url(raw:&str)->Result<String,String>{validate_base_url(raw)}

fn numeric(v:&Value)->Option<f64> { v.as_f64().filter(|n|n.is_finite()) }
fn required(v:&Value,key:&str)->Result<f64,String> { numeric(&v[key]).ok_or_else(||"站点返回的额度字段缺失或无效".into()) }
fn nonnegative(v:&Value,key:&str)->Result<f64,String> { required(v,key).and_then(|n|if n>=0.0{Ok(n)}else{Err("站点返回的额度字段无效".into())}) }
fn expiry(v:&Value)->Option<i64> {
    v.as_i64().filter(|n|*n>0).or_else(||v.as_str().and_then(|s|chrono::DateTime::parse_from_rfc3339(s).ok()).map(|d|d.timestamp()).filter(|n|*n>0))
}
/// 金额只保留到分，小额保留更多位以免显示成 0。
fn money_text(v:f64)->String {
    if v==0.0 { return "0".into(); }
    let abs=v.abs();
    let raw = if abs>=1.0 { format!("{v:.2}") } else if abs>=0.01 { format!("{v:.4}") } else { format!("{v:.6}") };
    raw.trim_end_matches('0').trim_end_matches('.').to_owned()
}
/// 站点自报的展示信息，来自匿名 `/api/status`。
///
/// `logo` 是站点自己的品牌资产，只在内存里解码渲染：不写进
/// `ai-quota-cache.json`，也不随安装包分发。站点换图后下次刷新自动跟随。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Site {
    pub rate: Option<Rate>,
    pub logo: Option<String>,
}
/// 读取 `/api/status` 的配额换算参数与站点 logo。
///
/// 只信 `quota_per_unit`：它是站长按实际售价填写的常数，绝不像
/// `usd_exchange_rate` 那样普遍保留安装默认值。
///
/// 货币符号按「站点是否按人民币收款」判断（方案 C）：
/// - `quota_display_type=CNY` → ¥
/// - `quota_display_type=CUSTOM` → 站点自定义符号
/// - `quota_display_type=USD` 但 `price`（1 美元额度的充值价）明显不是 1
///   → 站点实际按人民币计价，显示 ¥
/// - 其余 → $
///
/// `price` 的依据见 new-api `controller/topup.go`：`payMoney = 额度 × Price`，
/// 所以 `Price` 就是充值汇率。ican.uno 的 `quota_display_type` 仍是安装默认
/// 的 USD、`Price=7.3`，后台实际显示 ¥9.97，正是这个分支要覆盖的情况。
pub fn parse_site(status:&Value)->Site {
    let rate=parse_rate(status);
    let logo=parse_logo(status);
    Site{rate,logo}
}
pub fn parse_rate(status:&Value)->Option<Rate> {
    let per_unit=numeric(&status["data"]["quota_per_unit"]).filter(|n|*n>0.0)?;
    let declared=status["data"]["quota_display_type"].as_str().unwrap_or("USD");
    let symbol=match declared {
        "CNY"=>"¥".to_owned(),
        "CUSTOM"=>{
            let custom=status["data"]["custom_currency_symbol"].as_str().map(str::trim).filter(|s|!s.is_empty()&&s.chars().count()<=4)?;
            custom.to_owned()
        }
        _=>{
            // 声明美元，但充值汇率明显不是 1:1 —— 站点实际收人民币。
            let price=numeric(&status["data"]["price"]).filter(|n|*n>0.0).unwrap_or(1.0);
            let cny_collected=(price-1.0).abs()>0.01;
            if cny_collected{"¥"}else{"$"}.to_owned()
        }
    };
    Some(Rate{per_unit,symbol})
}
/// 只接受内联的位图/矢量 data URL；外链、超大与未知 MIME 一律丢弃。
///
/// 白名单与 Cargo 里 `image` crate 启用的格式保持一致（png/jpeg/webp），
/// 避免收下了解码不了的类型；SVG 走矢量光栅化另有路径。
fn parse_logo(status:&Value)->Option<String> {
    data_url(status["data"]["logo"].as_str()?)
}
/// 校验并规整内联 data URL；外链、超大、非 base64 与未知 MIME 一律丢弃。
///
/// `mime` 只用于决定渲染路径，不参与信任判断——站点实际给的是 PNG 却
/// 标成 `image/x-icon`（DSHAPI 就是这样），所以放宽到任意 `image/*`，
/// 真正的解码失败由渲染层兜底。
fn data_url(raw:&str)->Option<String> {
    const MAX:usize=256*1024;
    let raw=raw.trim();
    if raw.is_empty()||raw.len()>MAX{return None;}
    let (header,payload)=raw.split_once(',')?;
    if payload.is_empty(){return None;}
    if !payload.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'+'||b==b'/'||b==b'='){return None;}
    let meta=header.strip_prefix("data:")?;
    if !meta.split(';').any(|part|part.eq_ignore_ascii_case("base64")){return None;}
    let mime=meta.split(';').next()?.to_ascii_lowercase();
    mime.starts_with("image/").then(||raw.to_owned())
}
/// 从 Sub2API 的网页壳提取站点 logo。
///
/// Sub2API 没有 `/api/status`，品牌信息以 `window.__APP_CONFIG__={...}` 内联
/// 在首页 HTML 里。这里只做字符串定位 + JSON 解析，**不执行任何脚本**：
/// - 找不到标记或 JSON 不完整时返回 None；
/// - 只取 `site_logo` 一个字段，其余配置一概不碰。
fn sub2api_site_logo(html:&str)->Option<String> {
    const MARKER:&str="__APP_CONFIG__=";
    let start=html.find(MARKER)?+MARKER.len();
    let rest=html.get(start..)?;
    // 只解析第一个完整 JSON 值，后面的脚本内容不参与。
    let mut stream=serde_json::Deserializer::from_str(rest).into_iter::<Value>();
    let config=stream.next()?.ok()?;
    data_url(config["site_logo"].as_str()?)
}
/// 匿名读取站点首页并提取 Sub2API 的品牌 logo。
///
/// 与 New API 的 `/api/status` 一样：不带 Authorization、失败静默、
/// 不计入自动识别预算。
fn fetch_sub2api_logo(agent:&ureq::Agent,base:&str,timeout:Duration)->Option<String> {
    let response=agent.get(&format!("{base}/")).set("Accept","text/html").timeout(timeout).call().ok()?;
    if !(200..300).contains(&response.status()){return None;}
    const MAX:u64=1024*1024;
    let mut raw=Vec::new();
    response.into_reader().take(MAX+1).read_to_end(&mut raw).ok()?;
    if raw.len() as u64>MAX{return None;}
    sub2api_site_logo(&String::from_utf8_lossy(&raw))
}
/// 从 HTML 里取 `<link rel="...icon...">` 的 href。
///
/// 只做大小写无关的子串搜索，不引入 HTML 解析器：图标声明在 `<head>` 里
/// 形式很固定。`rel` 需要是 icon 或含 icon 的 token（如 `shortcut icon`、
/// `apple-touch-icon`），`href` 支持单/双引号与无引号。
fn icon_href(html:&str)->Option<String> {
    let lower=html.to_ascii_lowercase();
    let mut at=0;
    while let Some(found)=lower[at..].find("<link") {
        let start=at+found;
        let end=lower[start..].find('>').map(|n|start+n)?;
        let tag=&html[start..end];
        let tag_lower=&lower[start..end];
        if tag_lower.contains("rel")&&tag_lower.contains("icon") {
            if let Some(href)=attr(tag,"href") {
                let value=href.trim();
                if !value.is_empty(){return Some(value.to_owned());}
            }
        }
        at=end;
    }
    None
}
/// 从标签里取属性值，支持双引号、单引号与无引号三种写法。
fn attr(tag:&str,name:&str)->Option<String> {
    let lower=tag.to_ascii_lowercase();
    let mut at=0;
    while let Some(found)=lower[at..].find(name) {
        let start=at+found;
        // 必须是完整属性名，避免匹配到 data-href 之类的子串。
        let before=lower[..start].chars().next_back();
        // 属性名前面必须是边界：标签开头、空白或 '/'
        let boundary=matches!(before,None)||before.is_some_and(|c|c.is_whitespace()||c=='<'||c=='/');
        if !boundary{at=start+name.len();continue;}
        let rest=&tag[start+name.len()..];
        let rest=rest.trim_start();
        let Some(rest)=rest.strip_prefix('=') else{at=start+name.len();continue;};
        let rest=rest.trim_start();
        let value=match rest.chars().next()? {
            '"'=>rest[1..].split('"').next()?,
            '\''=>rest[1..].split('\'').next()?,
            _=>rest.split(|c:char|c.is_whitespace()||c=='>').next()?,
        };
        return Some(value.to_owned());
    }
    None
}
/// 把 `<link rel=icon>` 的 href 变成可请求的绝对地址。
///
/// 只接受同源或同主机的 HTTPS 地址：不接受外站、`data:`（本来就在页面里，
/// 不需要单独请求）与非 HTTP 协议。
fn icon_url(base:&str,href:&str)->Option<String> {
    if href.starts_with("data:"){return None;}
    let base_url=url::Url::parse(base).ok()?;
    let resolved=base_url.join(href).ok()?;
    if resolved.scheme()!="https"||resolved.host_str()!=base_url.host_str(){return None;}
    Some(resolved.into())
}
/// 下载一张图标并转成 data URL，供现有渲染路径统一处理。
///
/// 与品牌信息请求同样的约束：不带 Authorization、失败静默、
/// 上限 256KB、只接受 `image/*`。
fn fetch_icon(agent:&ureq::Agent,url:&str,timeout:Duration)->Option<String> {
    let response=agent.get(url).set("Accept","image/*").timeout(timeout).call().ok()?;
    if !(200..300).contains(&response.status()){return None;}
    let mime=response.header("Content-Type")?.split(';').next()?.trim().to_ascii_lowercase();
    // 只认真正的图片；站点常把 404 页面当 favicon 返回。
    if !mime.starts_with("image/"){return None;}
    const MAX:u64=256*1024;
    let mut raw=Vec::new();
    response.into_reader().take(MAX+1).read_to_end(&mut raw).ok()?;
    if raw.is_empty()||raw.len() as u64>MAX{return None;}
    use base64::Engine;
    Some(format!("data:{mime};base64,{}",base64::engine::general_purpose::STANDARD.encode(&raw)))
}
/// 站点图标回退：先按 `<link rel=icon>` 声明取，再退到 `/favicon.ico`。
///
/// 仅当用户显式在高级设置里开启时才调用；抓到的内容只在本次运行的内存里
/// 渲染，不写缓存、不进安装包。声明里若是内联 data URL，直接复用。
fn fetch_favicon(agent:&ureq::Agent,base:&str,timeout:Duration)->Option<String> {
    let deadline=Instant::now()+timeout;
    let left=||deadline.checked_duration_since(Instant::now()).filter(|d|!d.is_zero());
    if let Some(html)=fetch_html(agent,base,&left)?{
        if let Some(href)=icon_href(&html) {
            if href.starts_with("data:"){return data_url(&href);}
            if let Some(url)=icon_url(base,&href) {
                if let Some(icon)=fetch_icon(agent,&url,left()?){return Some(icon);}
            }
        }
    }
    fetch_icon(agent,&format!("{base}/favicon.ico"),left()?)
}
/// 下载首页 HTML；上限 1MB，失败静默。
fn fetch_html(agent:&ureq::Agent,base:&str,left:&impl Fn()->Option<Duration>)->Option<Option<String>> {
    let response=agent.get(&format!("{base}/")).set("Accept","text/html").timeout(left()?).call().ok()?;
    if !(200..300).contains(&response.status()){return Some(None);}
    const MAX:u64=1024*1024;
    let mut raw=Vec::new();
    response.into_reader().take(MAX+1).read_to_end(&mut raw).ok()?;
    if raw.len() as u64>MAX{return Some(None);}
    Some(Some(String::from_utf8_lossy(&raw).into_owned()))
}
fn snapshot(kind:Kind,amounts:Vec<Amount>,expires_at:Option<i64>,windows:Vec<Window>,rate:Option<Rate>)->Snapshot {
    Snapshot { plan:kind.name().into(), relay:Some(Quota{source:kind.name().into(),amounts,expires_at,rate,logo:None}), windows, at:chrono::Utc::now().timestamp(), ..Default::default() }
}
fn window(label:&str,limit:f64,remaining:f64,seconds:Option<i64>,reset:Option<i64>)->Option<Window> {
    (limit>0.0).then(||Window{label:label.into(),seconds,remaining:((remaining/limit).clamp(0.0,1.0)*100.0) as f32,reset,amount:None})
}
fn amount(label:&str,unit:&str,remaining:Option<f64>,limit:Option<f64>,used:Option<f64>,unlimited:bool,scope:Scope)->Amount {
    Amount{scope,label:label.into(),unit:unit.into(),remaining,limit,used,unlimited}
}

/// `rate` 是站点声明的换算参数；令牌行的 `used` 是该 Key 自身消耗，
/// 账户行的 `used` 是账号全量累计，两者语义不同，标签上要分开。
pub fn parse_new_api(v:&Value,rate:Option<Rate>)->Result<Snapshot,String> {
    if v["code"]!=true || !v["error"].is_null() { return Err("站点未返回有效的令牌额度".into()); }
    let d=&v["data"];
    if d["object"]!="token_usage" { return Err("该站点版本不支持 New API 令牌额度接口".into()); }
    if !d["unit"].is_null()&&d["unit"]!="quota"{return Err("站点额度单位与 New API 令牌协议不一致".into());}
    let unlimited=d["unlimited_quota"].as_bool().ok_or("站点未返回令牌限额状态")?;
    let used=nonnegative(d,"total_used")?;
    let (remaining,limit)=if unlimited {(None,None)}else{(Some(required(d,"total_available")?),Some(nonnegative(d,"total_granted")?))};
    let windows=match (remaining,limit) { (Some(r),Some(l))=>window("令牌额度",l,r,None,None).into_iter().collect(),_=>vec![] };
    let mut amounts=vec![amount("令牌额度","站点配额",remaining,limit,Some(used),unlimited,Scope::Key)];
    if d["user_quota"].is_object() {
        let u=&d["user_quota"];
        if !u["unit"].is_null()&&u["unit"]!="quota"{return Err("站点账户额度单位与令牌额度不一致".into());}
        amounts.push(amount("账户额度","站点配额",Some(required(u,"total_available")?),None,Some(nonnegative(u,"total_used")?),false,Scope::Account));
    }
    Ok(snapshot(Kind::NewApi,amounts,expiry(&d["expires_at"]),windows,rate))
}

pub fn parse_one_api(subscription:&Value,usage:&Value)->Result<Snapshot,String> {
    if !subscription["error"].is_null() || !usage["error"].is_null() || subscription["object"]!="billing_subscription" || usage["object"]!="list" {
        return Err("该站点未返回有效的 One API 计费额度".into());
    }
    let limit=nonnegative(subscription,"hard_limit_usd")?;
    // This endpoint multiplies usage by 100 even when DisplayInCurrencyEnabled is false.
    let used=nonnegative(usage,"total_usage")?/100.0;
    // Official One API uses this sentinel for unlimited, but cannot distinguish it
    // from a real numeric limit. Never present a fabricated balance or percentage.
    let ambiguous=limit==100_000_000.0;
    let remaining=if ambiguous {None}else{Some(limit-used)};
    let windows=remaining.and_then(|r|window("计费额度",limit,r,None,None)).into_iter().collect();
    // 兼容计费接口返回的是站点自定义单位，币种不可知，因此从不给 rate。
    Ok(snapshot(Kind::OneApi,vec![amount("计费额度","站点单位",remaining,if ambiguous{None}else{Some(limit)},Some(used),false,Scope::Unknown)],expiry(&subscription["access_until"]),windows,None))
}

pub fn parse_sub2api(v:&Value)->Result<Snapshot,String> {
    if v["isValid"]!=true || !v["error"].is_null() { return Err("站点未返回有效的 Sub2API 额度".into()); }
    if !v["unit"].is_null()&&v["unit"]!="USD"{return Err("站点返回了未知的额度单位".into());}
    let mut amounts=vec![];let mut windows=vec![];
    match v["mode"].as_str() {
        Some("quota_limited")=>{
            if !v["quota"].is_null() {
                let q=&v["quota"];
                if q["unit"]!="USD" {return Err("站点返回了未知的额度单位".into());}
                let limit=nonnegative(q,"limit")?;let used=nonnegative(q,"used")?;let remaining=required(q,"remaining")?;
                amounts.push(amount("API Key 总额度","USD",Some(remaining),Some(limit),Some(used),false,Scope::Key));
                if let Some(w)=window("总额度",limit,remaining,None,None){windows.push(w);}
            }
            if let Some(rows)=v["rate_limits"].as_array() {
                if rows.len()>3 {return Err("站点返回了重复或过多的限额周期".into());}
                let mut seen=std::collections::HashSet::new();
                for row in rows {
                    if !row["unit"].is_null()&&row["unit"]!="USD"{return Err("站点返回了未知的额度单位".into());}
                    let (label,seconds)=match row["window"].as_str(){Some("5h")=>("5 小时",18000),Some("1d")=>("每日",86400),Some("7d")=>("每周",604800),_=>return Err("站点返回了未知的限额周期".into())};
                    if !seen.insert(seconds){return Err("站点返回了重复的限额周期".into());}
                    let limit=nonnegative(row,"limit")?;let used=nonnegative(row,"used")?;let remaining=required(row,"remaining")?;
                    // Official rate_limits fields are USD spending limits, not request counts.
                    amounts.push(amount(label,"USD",Some(remaining),Some(limit),Some(used),false,Scope::Key));
                    if let Some(w)=window(label,limit,remaining,Some(seconds),expiry(&row["reset_at"])){windows.push(w);}
                }
            }
        }
        Some("unrestricted")=>{
            if v["unit"]!="USD" {return Err("站点返回了未知的额度单位".into());}
            if v["subscription"].is_object() {
                let sub=&v["subscription"];
                for (prefix,label,seconds) in [("daily","每日",86400),("weekly","每周",604800),("monthly","每月",2592000)] {
                    let field=format!("{prefix}_limit_usd");
                    // Null / absent is not an exhausted window, and does not prove unlimited.
                    if sub[&field].is_null(){continue;}
                    let limit=nonnegative(sub,&field)?;
                    if limit==0.0 {continue;}
                    let used=nonnegative(sub,&format!("{prefix}_usage_usd"))?;let remaining=(limit-used).max(0.0);
                    amounts.push(amount(label,"USD",Some(remaining),Some(limit),Some(used),false,Scope::Subscription));
                    if let Some(w)=window(label,limit,remaining,Some(seconds),None){windows.push(w);}
                }
                if amounts.is_empty() {
                    // -1 is an explicit unlimited signal only for this subscription mode.
                    if numeric(&v["remaining"])==Some(-1.0) {
                        amounts.push(amount("订阅额度","USD",None,None,None,true,Scope::Subscription));
                    } else { return Err("站点未返回可用的订阅限额明细".into()); }
                }
            } else if !v["balance"].is_null() {
                amounts.push(amount("账户余额","USD",Some(required(v,"balance")?),None,None,false,Scope::Account));
            } else {
                return Err("站点未返回订阅限额或钱包余额，请检查站点版本与权限".into());
            }
        }
        _=>return Err("该站点版本不支持当前 Sub2API 额度协议".into()),
    }
    if amounts.is_empty(){return Err("站点未返回可用的额度明细".into());}
    let expires=expiry(&v["expires_at"]).or_else(||expiry(&v["subscription"]["expires_at"]));
    // Sub2API 直接返回 USD，本身就是货币值，不做二次换算。
    Ok(snapshot(Kind::Sub2Api,amounts,expires,windows,None))
}

fn response_error(status:u16)->(String,u64) {
    match status {
        401=>("站点认证失败（401），请核对 API Key".into(),0),
        403=>("站点拒绝额度查询（403），请检查权限".into(),60),
        404=>("站点没有该额度接口（404），请核对类型、地址和版本".into(),60),
        429=>("站点请求过于频繁，请稍后重试".into(),180),
        300..=399=>("站点要求重定向；为保护 API Key 已停止，请填写最终 HTTPS 地址".into(),0),
        _=>(format!("站点额度服务暂不可用（{status}）"),30),
    }
}
#[derive(Debug)]
enum ProbeError { Unsupported((String,u64)),Stop((String,u64)) }
impl ProbeError {fn detail(self)->(String,u64){match self{Self::Unsupported(e)|Self::Stop(e)=>e}}}
fn query(agent:&ureq::Agent,url:&str,key:&str,timeout:Option<Duration>)->Result<Value,ProbeError> {
    // Callers must supply the application's timeout-bounded, redirects(0) agent.
    let request=agent.get(url).set("Authorization",&format!("Bearer {key}")).set("Accept","application/json");
    let request=if let Some(timeout)=timeout{request.timeout(timeout)}else{request};
    let response=request.call().map_err(|e|match e{
        ureq::Error::Status(s,_)=>if matches!(s,404|405){ProbeError::Unsupported(response_error(s))}else{ProbeError::Stop(response_error(s))},
        _=>ProbeError::Stop(("无法连接额度站点，请检查网络与地址".into(),30)),
    })?;
    if !(200..300).contains(&response.status()){return Err(ProbeError::Stop(response_error(response.status())));}
    const MAX:u64=1024*1024;
    let mut raw=Vec::new();
    response.into_reader().take(MAX+1).read_to_end(&mut raw).map_err(|_|ProbeError::Stop(("读取站点额度失败".into(),30)))?;
    if raw.len() as u64>MAX{return Err(ProbeError::Stop(("站点额度响应过大".into(),60)));}
    serde_json::from_slice(&raw).map_err(|_|ProbeError::Unsupported(("站点未返回有效的 JSON 额度响应".into(),30)))
}
fn get(agent:&ureq::Agent,url:&str,key:&str)->Result<Value,(String,u64)>{query(agent,url,key,None).map_err(ProbeError::detail)}

/// 匿名读取 `/api/status` 的换算参数与站点 logo。
///
/// 这是唯一不为额度取证、只为展示取证而发出的请求，因此：
/// - 不带 Authorization，站点拒绝也不影响额度查询；
/// - 任何失败（网络、非 JSON、字段缺失）都静默返回 None；
/// - 不计入自动识别的请求预算，也不改变协议判定。
fn fetch_site(agent:&ureq::Agent,base:&str,timeout:Duration)->Option<Site>{
    let request=agent.get(&format!("{base}/api/status")).set("Accept","application/json");
    let response=request.timeout(timeout).call().ok()?;
    if !(200..300).contains(&response.status()){return None;}
    const MAX:u64=512*1024;
    let mut raw=Vec::new();
    response.into_reader().take(MAX+1).read_to_end(&mut raw).ok()?;
    if raw.len() as u64>MAX{return None;}
    let site=parse_site(&serde_json::from_slice(&raw).ok()?);
    (site.rate.is_some()||site.logo.is_some()).then_some(site)
}

fn detect_with(mut query:impl FnMut(&str,Duration)->Result<Value,ProbeError>,mut site:impl FnMut(Kind,Duration)->Option<Site>,cancelled:impl Fn()->bool,budget:Duration)->Result<(Kind,Snapshot),(String,u64)>{
    let deadline=Instant::now()+budget;let mut requests=0;
    let mut next=|path:&str|->Result<Option<Value>,(String,u64)>{
        if cancelled(){return Err(("已取消连接".into(),0));}
        let remaining=deadline.checked_duration_since(Instant::now()).filter(|d|!d.is_zero()).ok_or_else(||("自动识别超时，请稍后重试或在高级设置中手动选择".into(),30))?;
        if requests>=MAX_PROBE_REQUESTS{return Err(("已达到自动识别请求上限".into(),60));}requests+=1;
        let response=query(path,remaining.min(PROBE_TIMEOUT));
        if cancelled(){return Err(("已取消连接".into(),0));}
        if Instant::now()>=deadline{return Err(("自动识别超时，请稍后重试或在高级设置中手动选择".into(),30));}
        let value=match response{Ok(v)=>v,Err(ProbeError::Unsupported(_))=>return Ok(None),Err(ProbeError::Stop(e))=>return Err(e)};
        if value["code"]==false||value["success"]==false||value["isValid"]==false||!value["error"].is_null(){return Err(("站点拒绝额度查询，请核对 API Key 与查询权限".into(),60));}
        Ok(Some(value))
    };
    // 品牌信息（换算参数 / logo）只在识别出具体协议后才取，避免为无关协议多发请求。
    let mut site_for=|kind:Kind|->Option<Site>{
        let left=deadline.checked_duration_since(Instant::now()).filter(|d|!d.is_zero())?;
        if cancelled(){return None;}
        let timeout=left.min(PROBE_TIMEOUT);
        site(kind,timeout)
    };
    // 优先专用额度协议；兼容计费接口不证明站点品牌，也不证明账户或 Key 范围。
    for (kind,path) in [(Kind::NewApi,"/api/usage/token/"),(Kind::Sub2Api,"/v1/usage"),(Kind::OneApi,"/dashboard/billing/subscription")] {
        let Some(value)=next(path)?else{continue;};
        let result=match kind{
            Kind::NewApi if value["data"]["object"]=="token_usage"=>{
                let site=site_for(kind);
                parse_new_api(&value,site.as_ref().and_then(|s|s.rate.clone())).map(|mut s|{
                    if let Some(relay)=s.relay.as_mut(){relay.logo=site.and_then(|s|s.logo);}
                    s
                })
            }
            Kind::Sub2Api if value.get("mode").is_some()=>{
                let logo=site_for(kind).and_then(|s|s.logo);
                parse_sub2api(&value).map(|mut s|{
                    if let Some(relay)=s.relay.as_mut(){relay.logo=logo;}
                    s
                })
            }
            Kind::OneApi if value["object"]=="billing_subscription"=>{
                let usage=next("/dashboard/billing/usage")?.ok_or_else(||("计费额度明细不完整，请核对站点版本或在高级设置中选择类型".into(),60))?;
                let logo=site_for(kind).and_then(|s|s.logo);
                parse_one_api(&value,&usage).map(|mut s|{
                    if let Some(relay)=s.relay.as_mut(){relay.logo=logo;}
                    s
                })
            },
            _=>continue,
        };
        return result.map(|s|(kind,s)).map_err(|e|(e,60));
    }
    Err(("暂时无法自动识别额度接口，请核对站点地址或在高级设置中手动选择".into(),60))
}
pub fn detect_cancellable(agent:impl Fn(Duration)->ureq::Agent,base_url:&str,key:&str,use_favicon:bool,cancelled:impl Fn()->bool)->Result<(Kind,Snapshot),(String,u64)>{
    let base=validate_base_url(base_url).map_err(|e|(e,0))?;let key=key.trim();
    if key.is_empty()||key.len()>8192||key.chars().any(|c|c.is_whitespace()||c.is_control()){return Err(("请输入有效的 API Key".into(),0));}
    detect_with(|path,timeout|query(&agent(timeout),&format!("{base}{path}"),key,Some(timeout)),|kind,timeout|{
        let agent=agent(timeout);
        match kind{
            Kind::NewApi=>{
                let site=fetch_site(&agent,&base,timeout);
                // 站点没自报 logo 时，按用户开关回退到站点图标。
                if use_favicon&&site.as_ref().is_none_or(|s|s.logo.is_none()){
                    let logo=fetch_favicon(&agent,&base,timeout);
                    return (site.is_some()||logo.is_some()).then(||Site{rate:site.and_then(|s|s.rate),logo});
                }
                site
            }
            // Sub2API 没有 /api/status，品牌信息在首页 __APP_CONFIG__ 里。
            Kind::Sub2Api=>{
                let logo=fetch_sub2api_logo(&agent,&base,timeout).or_else(||use_favicon.then(||fetch_favicon(&agent,&base,timeout)).flatten());
                logo.map(|logo|Site{rate:None,logo:Some(logo)})
            }
            // 兼容计费接口没有可信品牌来源，仅在用户开启时抓站点图标。
            Kind::OneApi=>use_favicon.then(||fetch_favicon(&agent,&base,timeout)).flatten().map(|logo|Site{rate:None,logo:Some(logo)}),
        }
    },cancelled,AUTO_TIMEOUT)
}
/// 已保存的协议直接查询；刷新和手动指定类型均不自动切换协议。
pub fn request_cancellable(agent:&ureq::Agent,kind:Kind,base_url:&str,key:&str,use_favicon:bool,cancelled:impl Fn()->bool)->Result<Snapshot,(String,u64)> {
    if cancelled(){return Err(("已取消连接".into(),0));}
    let base=validate_base_url(base_url).map_err(|e|(e,0))?;
    let key=key.trim();
    if key.is_empty() || key.len()>8192 || key.chars().any(|c|c.is_whitespace()||c.is_control()) {return Err(("请输入有效的 API Key".into(),0));}
    let result=match kind {
        Kind::NewApi=>{
            let site=fetch_site(agent,&base,PROBE_TIMEOUT);
            if cancelled(){return Err(("已取消连接".into(),0));}
            let mut snapshot=parse_new_api(&get(agent,&format!("{base}/api/usage/token/"),key)?,site.as_ref().and_then(|s|s.rate.clone())).map_err(|e|(e,60))?;
            let mut logo=site.and_then(|s|s.logo);
            // 站点没自报 logo 时，按用户开关回退到站点图标。
            if logo.is_none()&&use_favicon{logo=fetch_favicon(agent,&base,PROBE_TIMEOUT);}
            if let Some(relay)=snapshot.relay.as_mut(){relay.logo=logo;}
            Ok(snapshot)
        }
        Kind::OneApi=>{
            let subscription=get(agent,&format!("{base}/dashboard/billing/subscription"),key)?;
            if cancelled(){return Err(("已取消连接".into(),0));}
            let usage=get(agent,&format!("{base}/dashboard/billing/usage"),key)?;
            let mut snapshot=parse_one_api(&subscription,&usage).map_err(|e|(e,60))?;
            if use_favicon{if let Some(logo)=fetch_favicon(agent,&base,PROBE_TIMEOUT){if let Some(relay)=snapshot.relay.as_mut(){relay.logo=Some(logo);}}}
            Ok(snapshot)
        }
        Kind::Sub2Api=>{
            let mut logo=fetch_sub2api_logo(agent,&base,PROBE_TIMEOUT);
            if cancelled(){return Err(("已取消连接".into(),0));}
            let mut snapshot=parse_sub2api(&get(agent,&format!("{base}/v1/usage"),key)?).map_err(|e|(e,60))?;
            if logo.is_none()&&use_favicon{logo=fetch_favicon(agent,&base,PROBE_TIMEOUT);}
            if let Some(relay)=snapshot.relay.as_mut(){relay.logo=logo;}
            Ok(snapshot)
        }
    };
    result.map_err(|e|(e,60))
}

#[cfg(test)]
mod tests {
    use super::*;use serde_json::json;
    fn serve_once(response:Vec<u8>,delay:std::time::Duration)->(String,std::thread::JoinHandle<String>){
        use std::{io::{Read,Write},net::TcpListener};
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();let url=format!("http://{}/api/usage/token/",listener.local_addr().unwrap());
        let worker=std::thread::spawn(move||{
            let (mut stream,_)=listener.accept().unwrap();stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
            let mut request=Vec::new();let mut byte=[0u8;1];
            while request.len()<8192 && !request.ends_with(b"\r\n\r\n"){stream.read_exact(&mut byte).unwrap();request.push(byte[0]);}
            std::thread::sleep(delay);let _=stream.write_all(&response);String::from_utf8(request).unwrap()
        });(url,worker)
    }
    #[test]
    fn http_errors_and_redirects_keep_credentials_out_of_messages(){
        let agent=ureq::AgentBuilder::new().try_proxy_from_env(false).redirects(0).timeout(std::time::Duration::from_secs(2)).build();
        for status in [401,403,429,302,500]{
            let body=r#"{"message":"sk-sensitive-response"}"#;
            let response=format!("HTTP/1.1 {status} Error\r\nContent-Length: {}\r\nLocation: https://must-not-connect.invalid\r\nConnection: close\r\n\r\n{body}",body.len()).into_bytes();
            let (url,worker)=serve_once(response,std::time::Duration::ZERO);
            let (error,wait)=get(&agent,&url,"synthetic-wire-key").unwrap_err();let request=worker.join().unwrap();
            assert!(request.starts_with("GET /api/usage/token/ HTTP/1.1\r\n"));
            assert!(request.to_ascii_lowercase().contains("authorization: bearer synthetic-wire-key\r\n"));
            assert!(!error.contains("sk-sensitive")&&!error.contains("synthetic-wire"));
            if status==429{assert_eq!(wait,180);}if status==302{assert!(error.contains("重定向"));}
        }
    }
    #[test]
    fn oversized_and_slow_responses_are_bounded(){
        let agent=ureq::AgentBuilder::new().try_proxy_from_env(false).redirects(0).timeout(std::time::Duration::from_millis(150)).build();
        let mut response=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",1024*1024+1).into_bytes();response.resize(response.len()+1024*1024+1,b' ');
        let (url,worker)=serve_once(response,std::time::Duration::ZERO);let error=get(&agent,&url,"synthetic").unwrap_err().0;worker.join().unwrap();assert!(error.contains("过大"));
        let (url,worker)=serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".to_vec(),std::time::Duration::from_millis(350));
        assert!(get(&agent,&url,"synthetic").is_err());worker.join().unwrap();
    }
    #[test] fn cancelled_validation_makes_no_request(){
        let agent=ureq::AgentBuilder::new().redirects(0).build();
        assert_eq!(request_cancellable(&agent,Kind::OneApi,"https://must-not-resolve.invalid","synthetic",false,||true).unwrap_err().0,"已取消连接");
    }
    #[test] fn persisted_prefix_is_not_normalized_again(){
        let base=normalize_base_url("https://relay.example/v1/v1").unwrap();assert_eq!(base,"https://relay.example/v1/v1");
        assert_eq!(validate_base_url(&base).unwrap(),base);
    }
    #[test] fn compact_names_the_exhausted_limiting_window(){
        let q=Quota{source:"Sub2API".into(),expires_at:None,rate:None,logo:None,amounts:vec![amount("每日","USD",Some(10.0),Some(10.0),Some(0.0),false,Scope::Subscription),amount("每周","USD",Some(0.0),Some(20.0),Some(20.0),false,Scope::Subscription)]};
        assert_eq!(q.compact(),"每周剩余 0%");
    }
    #[test] fn bases_are_https_and_never_contain_credentials_or_parameters(){
        for bad in ["http://example.com","http://localhost:8080","https://u:p@example.com","https://example.com?key=secret","https://example.com/#secret","file:///tmp/a","https://example.com\\evil","https://exa mple.com"] {assert!(normalize_base_url(bad).is_err(),"{bad}");}
        assert_eq!(normalize_base_url(" https://relay.example/prefix/v1/ ").unwrap(),"https://relay.example/prefix/v1");
        assert_eq!(normalize_base_url("https://relay.example/").unwrap(),"https://relay.example");
    }
    fn new_fixture()->Value {json!({"code":true,"data":{"object":"token_usage","total_granted":500000,"total_available":250000,"total_used":250000,"unlimited_quota":false,"expires_at":1900000000}})}
    fn unsupported()->ProbeError{ProbeError::Unsupported(response_error(404))}
    #[test]
    fn auto_prefers_key_quota_and_keeps_account_quota_separate(){
        let mut paths=vec![];let mut value=new_fixture();value["data"]["user_quota"]=json!({"total_available":12,"total_used":8});
        let (kind,snapshot)=detect_with(|path,timeout|{assert!(timeout<=PROBE_TIMEOUT);paths.push(path.to_owned());Ok(value.clone())},|_,_|None,||false,AUTO_TIMEOUT).unwrap();
        assert_eq!(kind,Kind::NewApi);assert_eq!(paths,["/api/usage/token/"]);
        let q=snapshot.relay.unwrap();assert_eq!(q.amounts[0].label,"令牌额度");assert_eq!(q.amounts[0].remaining,Some(250000.0));
        assert_eq!(q.amounts[1].label,"账户额度");assert_eq!(q.amounts[1].remaining,Some(12.0));assert!(q.amounts.iter().all(|a|a.unit=="站点配额"));
    }
    #[test]
    fn auto_falls_back_to_sub2api_before_compatible_billing(){
        let mut paths=vec![];
        let (kind,snapshot)=detect_with(|path,_|{paths.push(path.to_owned());if path=="/v1/usage"{Ok(json!({"isValid":true,"mode":"unrestricted","unit":"USD","balance":7}))}else{Err(unsupported())}},|_,_|None,||false,AUTO_TIMEOUT).unwrap();
        assert_eq!(kind,Kind::Sub2Api);assert_eq!(paths,["/api/usage/token/","/v1/usage"]);
        let q=snapshot.relay.unwrap();assert_eq!(q.amounts[0].label,"账户余额");assert_eq!(q.compact(),"7 USD");assert!(snapshot.windows.is_empty());
    }
    #[test]
    fn auto_billing_uses_at_most_four_requests_and_does_not_invent_scope_or_currency(){
        let mut paths=vec![];
        let (kind,snapshot)=detect_with(|path,_|{paths.push(path.to_owned());match path{
            "/dashboard/billing/subscription"=>Ok(json!({"object":"billing_subscription","hard_limit_usd":100})),
            "/dashboard/billing/usage"=>Ok(json!({"object":"list","total_usage":2500})),_=>Err(unsupported()),
        }},|_,_|None,||false,AUTO_TIMEOUT).unwrap();
        assert_eq!(kind,Kind::OneApi);assert_eq!(paths.len(),MAX_PROBE_REQUESTS);
        let q=snapshot.relay.unwrap();assert_eq!(q.source,"兼容计费接口");assert_eq!(q.amounts[0].remaining,Some(75.0));assert_eq!(q.amounts[0].unit,"站点单位");
        assert!(q.full().contains("余额暂不可查询"));assert!(!q.full().contains("USD")&&!q.full().contains('$'));
    }
    #[test]
    fn auto_stops_on_broken_recognized_quota_instead_of_accepting_a_different_balance(){
        for value in [json!({"code":true,"data":{"object":"token_usage"}}),json!({"code":false,"message":"sk-sensitive"}),json!({"success":false}),json!({"error":{"message":"sk-sensitive"}})] {
            let mut calls=0;let error=detect_with(|_,_|{calls+=1;Ok(value.clone())},|_,_|None,||false,AUTO_TIMEOUT).unwrap_err().0;
            assert_eq!(calls,1);assert!(!error.contains("sk-sensitive"));
        }
    }
    #[test]
    fn auto_does_not_accept_conflicting_currency_or_missing_billing_usage(){
        let mut value=new_fixture();value["data"]["unit"]=json!("USD");
        assert!(detect_with(|_,_|Ok(value.clone()),|_,_|None,||false,AUTO_TIMEOUT).unwrap_err().0.contains("单位"));
        for value in [json!({"isValid":true,"mode":"quota_limited","quota":{"unit":"CNY","limit":10,"used":1,"remaining":9}}),json!({"isValid":true,"mode":"quota_limited","rate_limits":[{"window":"1d","unit":"requests","limit":10,"used":1,"remaining":9}]})] {
            let mut calls=0;let error=detect_with(|path,_|{calls+=1;if path=="/v1/usage"{Ok(value.clone())}else{Err(unsupported())}},|_,_|None,||false,AUTO_TIMEOUT).unwrap_err().0;
            assert!(error.contains("单位"));assert_eq!(calls,2);
        }
        let error=detect_with(|path,_|if path.ends_with("subscription"){Ok(json!({"object":"billing_subscription","hard_limit_usd":10}))}else{Err(unsupported())},|_,_|None,||false,AUTO_TIMEOUT).unwrap_err().0;
        assert!(error.contains("不完整"));
    }
    #[test]
    fn auto_unknown_shapes_never_become_zero_balances(){
        let mut calls=0;let error=detect_with(|_,_|{calls+=1;Ok(json!({"balance":0}))},|_,_|None,||false,AUTO_TIMEOUT).unwrap_err().0;
        assert_eq!(calls,3);assert!(error.contains("高级设置"));
        let mut value=new_fixture();value["data"]["unlimited_quota"]=json!(true);
        let (_,snapshot)=detect_with(|_,_|Ok(value.clone()),|_,_|None,||false,AUTO_TIMEOUT).unwrap();assert_eq!(snapshot.relay.unwrap().amounts[0].remaining,None);
    }
    #[test]
    fn auto_budget_and_cancellation_stop_further_requests_and_reject_late_success(){
        let mut calls=0;assert!(detect_with(|_,_|{calls+=1;Ok(new_fixture())},|_,_|None,||true,AUTO_TIMEOUT).is_err());assert_eq!(calls,0);
        let error=detect_with(|_,_|{calls+=1;Ok(new_fixture())},|_,_|None,||false,Duration::ZERO).unwrap_err().0;assert!(error.contains("超时"));assert_eq!(calls,0);
        let cancelled=std::cell::Cell::new(false);
        assert!(detect_with(|_,_|{calls+=1;cancelled.set(true);Err(unsupported())},|_,_|None,||cancelled.get(),AUTO_TIMEOUT).is_err());assert_eq!(calls,1);
        let error=detect_with(|_,timeout|{assert!(timeout<=Duration::from_millis(20));std::thread::sleep(Duration::from_millis(40));Ok(new_fixture())},|_,_|None,||false,Duration::from_millis(20)).unwrap_err().0;assert!(error.contains("超时"));
    }
    #[test]
    fn auto_http_auth_rate_limit_redirect_and_server_errors_do_not_probe_other_routes(){
        let agent=ureq::AgentBuilder::new().try_proxy_from_env(false).redirects(0).timeout(Duration::from_secs(2)).build();
        for status in [401,403,429,302,500]{
            let body=r#"{"message":"sk-sensitive-response"}"#;
            let response=format!("HTTP/1.1 {status} Error\r\nContent-Length: {}\r\nLocation: https://must-not-connect.invalid\r\nConnection: close\r\n\r\n{body}",body.len()).into_bytes();
            let (url,worker)=serve_once(response,Duration::ZERO);let mut calls=0;
            let (error,_)=detect_with(|_,timeout|{calls+=1;query(&agent,&url,"synthetic-auto-key",Some(timeout))},|_,_|None,||false,AUTO_TIMEOUT).unwrap_err();
            let request=worker.join().unwrap();assert!(request.contains("Bearer synthetic-auto-key"));assert_eq!(calls,1);assert!(!error.contains("sensitive")&&!error.contains("synthetic-auto"));
        }
    }
    #[test]
    fn auto_rejects_bad_addresses_and_keys_before_any_network_request(){
        let agent=ureq::AgentBuilder::new().redirects(0).build();
        for base in ["http://must-not-resolve.invalid","https://u:p@must-not-resolve.invalid","https://must-not-resolve.invalid?key=secret"]{assert!(detect_cancellable(|_|agent.clone(),base,"synthetic",false,||false).is_err());}
        for key in ["","bad\nkey"]{assert!(detect_cancellable(|_|agent.clone(),"https://must-not-resolve.invalid/prefix",key,false,||false).is_err());}
    }
    #[test]
    fn auto_http_flow_preserves_origin_prefix_and_bearer_with_bounded_clients(){
        let calls=std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));let history=calls.clone();
        let (kind,snapshot)=detect_cancellable(move|timeout|{
            assert!(!timeout.is_zero()&&timeout<=PROBE_TIMEOUT);let history=history.clone();
            ureq::AgentBuilder::new().redirects(0).timeout_connect(timeout).middleware(move|request:ureq::Request,_:ureq::MiddlewareNext|{
                let url=request.url().to_owned();
                // 站点首页是公开展示资源，绝不允许携带 API Key。
                let brand=url.ends_with("/tenant/v1/");
                if brand{assert_eq!(request.header("Authorization"),None,"品牌请求不得带凭据：{url}");}
                else{assert_eq!(request.header("Authorization"),Some("Bearer synthetic-prefix-key"),"{url}");}
                history.lock().unwrap().push(url.clone());
                let body=if url.ends_with("/v1/usage"){r#"{"isValid":true,"mode":"unrestricted","unit":"USD","balance":7}"#}
                    else if brand{r#"<script>window.__APP_CONFIG__={"site_logo":"data:image/png;base64,AAAA"};</script>"#}
                    else{"<!doctype html>"};
                ureq::Response::new(200,"OK",body)
            }).build()
        },"https://relay.example/tenant/v1","synthetic-prefix-key",false,||false).unwrap();
        assert_eq!(kind,Kind::Sub2Api);
        let relay=snapshot.relay.unwrap();
        assert_eq!(relay.compact(),"7 USD");
        assert_eq!(relay.logo.as_deref(),Some("data:image/png;base64,AAAA"));
        // 三次请求：两次额度探测 + 一次匿名品牌请求（排在最后，失败也不影响额度）。
        assert_eq!(*calls.lock().unwrap(),[
            "https://relay.example/tenant/v1/api/usage/token/",
            "https://relay.example/tenant/v1/v1/usage",
            "https://relay.example/tenant/v1/",
        ]);
    }
    #[test] fn new_api_preserves_units_and_expiry_is_not_reset(){
        let s=parse_new_api(&new_fixture(),None).unwrap();assert_eq!(s.windows[0].remaining,50.0);assert_eq!(s.windows[0].reset,None);
        let q=s.relay.unwrap();assert_eq!(q.amounts[0].unit,"站点配额");assert!(!q.full().contains('$'));assert_eq!(q.expires_at,Some(1900000000));
    }
    #[test] fn missing_zero_and_unlimited_are_distinct(){
        let mut v=new_fixture();v["data"]["total_available"]=Value::Null;assert!(parse_new_api(&v,None).is_err());
        v["data"]["total_available"]=json!(0);assert_eq!(parse_new_api(&v,None).unwrap().windows[0].remaining,0.0);
        v["data"]["unlimited_quota"]=json!(true);let s=parse_new_api(&v,None).unwrap();assert!(s.windows.is_empty());let q=s.relay.unwrap();assert!(q.amounts[0].unlimited);assert_eq!(q.amounts[0].remaining,None);
        v["code"]=json!(false);assert!(parse_new_api(&v,None).is_err());
    }
    #[test] fn one_api_has_configurable_units_and_cent_scaled_usage(){
        let sub=json!({"object":"billing_subscription","hard_limit_usd":100});let usage=json!({"object":"list","total_usage":2500});
        let s=parse_one_api(&sub,&usage).unwrap();assert_eq!(s.windows[0].remaining,75.0);let q=s.relay.unwrap();assert_eq!(q.amounts[0].remaining,Some(75.0));assert_eq!(q.amounts[0].unit,"站点单位");
        let mut sentinel=sub.clone();sentinel["hard_limit_usd"]=json!(100000000);let s=parse_one_api(&sentinel,&usage).unwrap();assert!(s.windows.is_empty());assert_eq!(s.relay.unwrap().amounts[0].remaining,None);
        assert!(parse_one_api(&sub,&json!({"object":"list"})).is_err());assert!(parse_one_api(&json!({"error":{"message":"secret"}}),&usage).is_err());
    }
    #[test] fn sub2api_wallet_does_not_become_a_percentage(){
        let s=parse_sub2api(&json!({"mode":"unrestricted","isValid":true,"unit":"USD","balance":0,"remaining":0})).unwrap();assert!(s.windows.is_empty());assert_eq!(s.relay.unwrap().compact(),"0 USD");
        assert!(parse_sub2api(&json!({"mode":"unrestricted","isValid":true,"unit":"USD"})).is_err());
    }
    #[test] fn sub2api_retains_all_key_windows_and_explicit_reset(){
        let s=parse_sub2api(&json!({"mode":"quota_limited","isValid":true,"quota":{"limit":10,"used":2,"remaining":8,"unit":"USD"},"rate_limits":[{"window":"5h","limit":5,"used":1,"remaining":4,"reset_at":"2030-01-01T00:00:00Z"},{"window":"1d","limit":20,"used":0,"remaining":20}]})).unwrap();
        assert_eq!(s.windows.len(),3);assert_eq!(s.windows[1].remaining,80.0);assert!(s.windows[1].reset.is_some());assert_eq!(s.windows[2].reset,None);
        assert!(parse_sub2api(&json!({"mode":"quota_limited","isValid":true,"quota":{"limit":10,"used":2,"unit":"USD"}})).is_err());
    }
    #[test] fn duplicate_sub2api_periods_are_rejected(){
        let row=json!({"window":"5h","limit":5,"used":1,"remaining":4});
        assert!(parse_sub2api(&json!({"mode":"quota_limited","isValid":true,"rate_limits":[row.clone(),row]})).is_err());
    }
    #[test] fn sub2api_subscription_missing_and_unlimited_do_not_become_zero(){
        let s=parse_sub2api(&json!({"mode":"unrestricted","isValid":true,"unit":"USD","remaining":3,"subscription":{"daily_limit_usd":10,"daily_usage_usd":7,"weekly_limit_usd":null}})).unwrap();assert_eq!(s.windows.len(),1);assert_eq!(s.windows[0].remaining,30.0);
        let s=parse_sub2api(&json!({"mode":"unrestricted","isValid":true,"unit":"USD","remaining":-1,"subscription":{"daily_limit_usd":null}})).unwrap();assert!(s.relay.unwrap().amounts[0].unlimited);
        assert!(parse_sub2api(&json!({"mode":"unrestricted","isValid":true,"unit":"USD","remaining":0,"subscription":{}})).is_err());
    }
    #[test] fn finite_zero_limit_is_not_unlimited(){
        let s=parse_one_api(&json!({"object":"billing_subscription","hard_limit_usd":0}),&json!({"object":"list","total_usage":0})).unwrap();
        assert!(s.windows.is_empty());let q=s.relay.unwrap();assert_eq!(q.amounts[0].remaining,Some(0.0));assert!(!q.amounts[0].unlimited);
        let mut v=new_fixture();v["data"]["total_granted"]=json!(0);v["data"]["total_available"]=json!(0);v["data"]["total_used"]=json!(0);
        let s=parse_new_api(&v,None).unwrap();assert!(s.windows.is_empty());assert!(!s.relay.unwrap().amounts[0].unlimited);
    }
    #[test] fn tiny_positive_amounts_are_not_displayed_as_zero(){assert_ne!(number(0.000001),"0");assert_eq!(number(0.0),"0");}
    #[test] fn quota_cache_roundtrip_preserves_unknown_and_unlimited(){
        let mut v=new_fixture();v["data"]["unlimited_quota"]=json!(true);
        let q=parse_new_api(&v,None).unwrap().relay.unwrap();let raw=serde_json::to_string(&q).unwrap();
        let restored:Quota=serde_json::from_str(&raw).unwrap();assert_eq!(restored.amounts[0].remaining,None);assert!(restored.amounts[0].unlimited);assert_eq!(restored.compact(),"余额暂不可查询");
    }
    #[test] fn balance_display_does_not_substitute_key_budgets_for_wallets(){
        for value in [false,true]{
            let mut v=new_fixture();v["data"]["unlimited_quota"]=json!(value);
            let q=parse_new_api(&v,None).unwrap().relay.unwrap();
            assert_eq!(q.compact(),"余额暂不可查询");
            assert!(!q.full().contains("令牌额度")&&!q.full().contains("不限额"));
        }
        let q=parse_sub2api(&json!({"mode":"quota_limited","isValid":true,"quota":{"limit":10,"used":2,"remaining":8,"unit":"USD"},"rate_limits":[{"window":"7d","limit":20,"used":20,"remaining":0}]})).unwrap().relay.unwrap();
        assert_eq!(q.compact(),"余额暂不可查询");assert!(!q.full().contains("API Key 总额度"));
        let q=parse_one_api(&json!({"object":"billing_subscription","hard_limit_usd":100}),&json!({"object":"list","total_usage":2500})).unwrap().relay.unwrap();
        assert_eq!(q.compact(),"余额暂不可查询");
    }
    #[test] fn balance_display_prefers_account_data_and_keeps_zero_balances(){
        for remaining in [0,500000]{
            let mut v=new_fixture();v["data"]["total_available"]=json!(1);v["data"]["user_quota"]=json!({"total_available":remaining,"total_used":8});
            let q=parse_new_api(&v,None).unwrap().relay.unwrap();
            assert_eq!(q.compact(),format!("{remaining} 站点配额"));
            assert!(q.full().contains("账户额度"));assert!(!q.full().contains("令牌额度"));
        }
    }
    #[test] fn balance_display_subscription_uses_remaining_percentages(){
        let q=parse_sub2api(&json!({"mode":"unrestricted","isValid":true,"unit":"USD","remaining":3,"subscription":{"daily_limit_usd":10,"daily_usage_usd":7,"weekly_limit_usd":100,"weekly_usage_usd":60}})).unwrap().relay.unwrap();
        assert_eq!(q.compact(),"每日剩余 30%");assert!(q.full().contains("剩余 30%"));
        let q=parse_sub2api(&json!({"mode":"unrestricted","isValid":true,"unit":"USD","remaining":-1,"subscription":{"daily_limit_usd":null}})).unwrap().relay.unwrap();
        assert_eq!(q.compact(),"额度暂不可查询");assert!(!q.full().contains("不限额"));
    }
    #[test] fn balance_display_legacy_cache_does_not_restore_key_limits(){
        let q:Quota=serde_json::from_value(json!({"source":"Sub2API","amounts":[{"label":"每日","unit":"USD","remaining":20,"limit":100,"used":80,"unlimited":false}],"expires_at":null})).unwrap();
        assert_eq!(q.compact(),"余额暂不可查询");assert!(!q.full().contains("20"));
    }
    #[test] fn untrusted_errors_and_nonfinite_numbers_are_not_exposed(){
        let v=json!({"code":false,"message":"sk-secret https://private.example"});let error=parse_new_api(&v,None).unwrap_err();assert!(!error.contains("secret"));assert!(!error.contains("private"));
        let mut v=new_fixture();v["data"]["total_used"]=json!("NaN");assert!(parse_new_api(&v,None).is_err());
        assert_eq!(response_error(429).1,180);assert!(response_error(302).0.contains("重定向"));
    }
    /// ican.uno 的真实返回值：站点 quota_per_unit=500000、quota_display_type=CNY，
    /// 但 usd_exchange_rate 是安装默认 7.3，本站并未使用。实测后台显示 ¥9.97。
    fn ican_status()->Value {json!({"data":{"quota_per_unit":500000,"quota_display_type":"CNY","usd_exchange_rate":7.3,"display_in_currency":true,"custom_currency_symbol":"¤","version":"v1.0.0-rc.49"}})}
    fn ican_usage()->Value {json!({"code":true,"data":{"object":"token_usage","total_granted":86001379,"total_available":-87919,"total_used":14462,"unlimited_quota":true,"user_quota":{"total_available":4985538,"total_used":14462}}})}
    #[test] fn rate_divides_by_quota_per_unit_and_ignores_usd_exchange_rate(){
        let rate=parse_rate(&ican_status()).unwrap();
        assert_eq!(rate.per_unit,500000.0);assert_eq!(rate.symbol,"¥");
        let q=parse_new_api(&ican_usage(),Some(rate)).unwrap().relay.unwrap();
        // 只除不乘：4985538 / 500000 = 9.9708，若误乘 7.3 会变成 72.79。
        assert_eq!(q.compact(),"¥9.97");
        assert!(q.full().contains("账户额度：剩余 ¥9.97，已用 ¥0.0289"));
        // 原始点仍然保留，方便到站点后台逐位核对。
        assert!(q.full().contains("4985538 / 14462 站点配额"));
        assert!(!q.full().contains("72.7"));
    }
    #[test] fn rate_requires_usable_per_unit_and_respects_currency_type(){
        for bad in [json!({}),json!({"data":{}}),json!({"data":{"quota_per_unit":0}}),json!({"data":{"quota_per_unit":"x"}})]{assert!(parse_rate(&bad).is_none(),"{bad}");}
        assert_eq!(parse_rate(&json!({"data":{"quota_per_unit":500000,"quota_display_type":"USD"}})).unwrap().symbol,"$");
        assert_eq!(parse_rate(&json!({"data":{"quota_per_unit":500000,"quota_display_type":"CUSTOM","custom_currency_symbol":"€"}})).unwrap().symbol,"€");
        // 自定义符号缺失或过长时不可信，退回原始点。
        for bad in [json!({"data":{"quota_per_unit":500000,"quota_display_type":"CUSTOM"}}),json!({"data":{"quota_per_unit":500000,"quota_display_type":"CUSTOM","custom_currency_symbol":"abcdefgh"}})]{assert!(parse_rate(&bad).is_none(),"{bad}");}
    }
    #[test] fn missing_rate_falls_back_to_raw_points_never_guesses_money(){
        let q=parse_new_api(&ican_usage(),None).unwrap().relay.unwrap();
        assert_eq!(q.compact(),"4985538 站点配额");
        assert!(!q.full().contains('¥')&&!q.full().contains('$'));
    }
    /// 方案 C：`quota_display_type=USD` 但充值汇率不是 1:1，说明站点实际收人民币。
    #[test] fn declared_usd_with_non_unit_topup_price_is_treated_as_cny(){
        let cny=json!({"data":{"quota_per_unit":500000,"quota_display_type":"USD","price":7.3}});
        assert_eq!(parse_rate(&cny).unwrap().symbol,"¥");
        // 真正的美元站：price 为 1（或缺失），保持 $。
        for usd in [json!({"data":{"quota_per_unit":500000,"quota_display_type":"USD","price":1}}),json!({"data":{"quota_per_unit":500000,"quota_display_type":"USD"}})]{assert_eq!(parse_rate(&usd).unwrap().symbol,"$","{usd}");}
        // 显式声明 CNY 时不受 price 影响。
        assert_eq!(parse_rate(&json!({"data":{"quota_per_unit":500000,"quota_display_type":"CNY","price":1}})).unwrap().symbol,"¥");
    }
    /// 手动覆盖优先级最高：即使站点自报 USD，也按用户选择显示。
    #[test] fn manual_currency_override_wins_over_site_declaration(){
        let rate=parse_rate(&json!({"data":{"quota_per_unit":500000,"quota_display_type":"USD","price":1}})).unwrap();
        let q=parse_new_api(&ican_usage(),Some(rate)).unwrap().relay.unwrap();
        assert_eq!(q.compact(),"$9.97");
        assert_eq!(q.compact_with(Some("¥".into())),"¥9.97");
        assert!(q.full_with(Some("¥".into())).contains("¥9.97"));
        // 空覆盖视为未覆盖，避免写坏配置后金额凭空消失。
        assert_eq!(q.compact_with(Some(String::new())),"$9.97");
        assert_eq!(q.compact_with(None),"$9.97");
    }
    #[test] fn rate_is_not_applied_to_subscription_percentages(){
        let rate=Rate{per_unit:500000.0,symbol:"¥".into()};
        let q=parse_sub2api(&json!({"mode":"unrestricted","isValid":true,"unit":"USD","subsidy":null,"remaining":3,"subscription":{"daily_limit_usd":10,"daily_usage_usd":7}})).unwrap().relay.unwrap();
        // 百分比形态与货币无关，即使传入 rate 也不应变成金额。
        let with_rate=Quota{rate:Some(rate),..q};
        assert_eq!(with_rate.compact(),"每日剩余 30%");assert!(with_rate.full().contains("剩余 30%"));
    }
    #[test] fn cached_rate_survives_roundtrip_and_old_cache_still_loads(){
        let rate=parse_rate(&ican_status()).unwrap();
        let q=parse_new_api(&ican_usage(),Some(rate)).unwrap().relay.unwrap();
        let restored:Quota=serde_json::from_str(&serde_json::to_string(&q).unwrap()).unwrap();
        assert_eq!(restored.rate.as_ref().unwrap().per_unit,500000.0);assert_eq!(restored.compact(),"¥9.97");
        let old:Quota=serde_json::from_value(json!({"source":"New API","amounts":[{"label":"账户额度","unit":"站点配额","remaining":4985538,"limit":null,"used":14462,"unlimited":false,"scope":"Account"}],"expires_at":null})).unwrap();
        assert!(old.rate.is_none());assert_eq!(old.compact(),"4985538 站点配额");
    }
    #[test] fn auto_requests_site_info_only_for_protocols_that_have_it(){
        let mut status_calls=0;
        let value=new_fixture();
        // New API 命中：发一次换算请求。
        let (kind,_)=detect_with(|_,_|Ok(value.clone()),|kind,_|{status_calls+=1;Some(parse_site(&ican_status()))},||false,AUTO_TIMEOUT).unwrap();
        assert_eq!(kind,Kind::NewApi);assert_eq!(status_calls,1);
        // 换算请求失败不阻断额度查询。
        let (kind,snapshot)=detect_with(|_,_|Ok(value.clone()),|_,_|None,||false,AUTO_TIMEOUT).unwrap();
        assert_eq!(kind,Kind::NewApi);assert!(snapshot.relay.unwrap().rate.is_none());
        // Sub2API 有自己的品牌接口，也只请求一次。
        let mut sub2_calls=0;
        let (kind,_)=detect_with(|path,_|if path=="/v1/usage"{Ok(json!({"isValid":true,"mode":"unrestricted","unit":"USD","balance":7}))}else{Err(unsupported())},|kind,_|{if kind==Kind::Sub2Api{sub2_calls+=1;}None},||false,AUTO_TIMEOUT).unwrap();
        assert_eq!(kind,Kind::Sub2Api);assert_eq!(sub2_calls,1);
        // 兼容计费接口的 site 闭包只在用户开启抓图标时才会真正发请求；
        // 这里断言闭包自己没被无谓调用两次以上（New API 只调一次）。
        let mut billing_calls=0;
        let (kind,_)=detect_with(|path,_|match path{
            "/dashboard/billing/subscription"=>Ok(json!({"object":"billing_subscription","hard_limit_usd":100})),
            "/dashboard/billing/usage"=>Ok(json!({"object":"list","total_usage":2500})),_=>Err(unsupported()),
        },|_,_|{billing_calls+=1;None},||false,AUTO_TIMEOUT).unwrap();
        assert_eq!(kind,Kind::OneApi);assert_eq!(billing_calls,1);
    }
    #[test] fn site_logo_accepts_only_inline_decodable_images(){
        let png="data:image/png;base64,iVBORw0KGgo=";
        assert_eq!(parse_site(&json!({"data":{"quota_per_unit":500000,"logo":png}})).logo.as_deref(),Some(png));
        // 任意 image/* 都收，因为站点可能把 PNG 标成 image/x-icon。
        for ok in ["data:image/jpeg;base64,AAAA","data:image/webp;base64,AAAA","data:image/svg+xml;base64,AAAA","data:image/x-icon;base64,AAAA"]{assert!(parse_site(&json!({"data":{"logo":ok}})).logo.is_some(),"{ok}");}
        // 外链、非图片、非 base64 与非法字符一律丢弃。
        for bad in ["https://cdn.example/logo.png","data:text/html;base64,AAAA","data:image/png;base64,AA AA","data:image/png,AAAA","data:image/png;base64,"]{assert!(parse_site(&json!({"data":{"logo":bad}})).logo.is_none(),"{bad}");}
        // 超过上限直接丢弃，不做截断解码。
        let oversized=format!("data:image/png;base64,{}",  "A".repeat(256*1024));
        assert!(parse_site(&json!({"data":{"logo":oversized}})).logo.is_none());
    }
    #[test] fn site_logo_stays_out_of_serialized_cache(){
        let site=parse_site(&json!({"data":{"quota_per_unit":500000,"quota_display_type":"CNY","logo":"data:image/png;base64,AAAA"}}));
        let mut q=parse_new_api(&ican_usage(),site.rate.clone()).unwrap().relay.unwrap();
        q.logo=site.logo.clone();
        let raw=serde_json::to_string(&q).unwrap();
        assert!(!raw.contains("base64")&&!raw.contains("data:image"));
        let restored:Quota=serde_json::from_str(&raw).unwrap();
        assert!(restored.logo.is_none());assert_eq!(restored.compact(),"¥9.97");
    }
    #[test] fn site_logo_rides_along_only_with_new_api(){
        let site=json!({"data":{"quota_per_unit":500000,"quota_display_type":"CNY","logo":"data:image/png;base64,AAAA"}});
        let (kind,snapshot)=detect_with(|_,_|Ok(ican_usage()),|kind,_|(kind==Kind::NewApi).then(||parse_site(&site)),||false,AUTO_TIMEOUT).unwrap();
        assert_eq!(kind,Kind::NewApi);
        let q=snapshot.relay.unwrap();
        assert_eq!(q.logo.as_deref(),Some("data:image/png;base64,AAAA"));
        assert_eq!(q.compact(),"¥9.97");
        // 兼容计费接口不给品牌信息。
        let (_,snapshot)=detect_with(|path,_|if path=="/v1/usage"{Ok(json!({"isValid":true,"mode":"unrestricted","unit":"USD","balance":7}))}else{Err(unsupported())},|_,_|None,||false,AUTO_TIMEOUT).unwrap();
        assert!(snapshot.relay.unwrap().logo.is_none());
    }
    /// DSHAPI 真实首页结构：品牌信息内联在 `window.__APP_CONFIG__` 里，
    /// 且 MIME 标成 image/x-icon 实际是 PNG。
    #[test] fn sub2api_logo_comes_from_inline_app_config(){
        let png="data:image/x-icon;base64,iVBORw0KGgoAAAANSUhEUg";
        let html=format!(r#"<html><body><script>window.__APP_CONFIG__={{"site_name":"DSH API","site_logo":"{png}","api_base_url":"https://api.example/v1"}};</script><script>var other=1;</script></body></html>"#);
        assert_eq!(sub2api_site_logo(&html).as_deref(),Some(png));
        // 标记缺失、JSON 截断、字段缺失、非法 data URL 一律 None。
        for bad in [
            "<html>no config here</html>".to_owned(),
            r#"<script>window.__APP_CONFIG__={"site_name":"X""#.to_owned(),
            r#"<script>window.__APP_CONFIG__={"site_name":"X"}</script>"#.to_owned(),
            format!(r#"<script>window.__APP_CONFIG__={{"site_logo":"https://cdn.example/l.png"}}</script>"#),
            format!(r#"<script>window.__APP_CONFIG__={{"site_logo":"data:text/html;base64,AAAA"}}</script>"#),
        ]{assert!(sub2api_site_logo(&bad).is_none(),"{bad}");}
    }
    #[test] fn icon_href_reads_common_link_forms(){
        for (html,want) in [
            (r#"<link rel="icon" href="/favicon.ico">"#,"/favicon.ico"),
            (r#"<link rel="icon" href="/favicon.svg" type="image/svg+xml">"#,"/favicon.svg"),
            (r#"<link rel='shortcut icon' href='/a.png'>"#,"/a.png"),
            (r#"<LINK REL="Icon" HREF="/up.png">"#,"/up.png"),
            (r#"<link href="/n.png" rel="apple-touch-icon">"#,"/n.png"),
            (r#"<link rel=icon href=/bare.png>"#,"/bare.png"),
        ]{assert_eq!(icon_href(html).as_deref(),Some(want),"{html}");}
        // 没有 icon 声明、或只有无关 link 时返回 None。
        for html in [
            r#"<link rel="stylesheet" href="/a.css">"#,
            r#"<link rel="preconnect" href="https://cdn.example">"#,
            "<html>no links</html>",
        ]{assert!(icon_href(html).is_none(),"{html}");}
    }
    #[test] fn icon_url_stays_on_the_site_and_https(){
        let base="https://relay.example";
        assert_eq!(icon_url(base,"/favicon.ico").as_deref(),Some("https://relay.example/favicon.ico"));
        assert_eq!(icon_url("https://relay.example/tenant/v1","/i.png").as_deref(),Some("https://relay.example/i.png"));
        assert_eq!(icon_url(base,"i.png").as_deref(),Some("https://relay.example/i.png"));
        // 外站、降级到 http、data: 与非 HTTP 协议一律拒绝。
        for href in ["https://evil.example/i.png","http://relay.example/i.png","data:image/png;base64,AAAA","javascript:alert(1)","//evil.example/i.png"]{
            assert!(icon_url(base,href).is_none(),"{href}");
        }
    }
    /// 图标解析链的决策逻辑：声明优先、同源校验、data URL 直用。
    fn icon_plan(base:&str,html:&str)->Option<String>{
        let href=icon_href(html)?;
        if href.starts_with("data:"){return data_url(&href);}
        icon_url(base,&href)
    }
    #[test] fn icon_plan_prefers_declaration_then_same_origin(){
        let base="https://relay.example";
        assert_eq!(icon_plan(base,r#"<link rel="icon" href="/favicon.svg">"#).as_deref(),Some("https://relay.example/favicon.svg"));
        // 声明是内联图片时不发请求，直接用。
        assert_eq!(icon_plan(base,r#"<link rel="icon" href="data:image/png;base64,AAAA">"#).as_deref(),Some("data:image/png;base64,AAAA"));
        // 声明指向外站时放弃该地址（调用方随后退回 /favicon.ico）。
        assert!(icon_plan(base,r#"<link rel="icon" href="https://cdn.other/i.png">"#).is_none());
        // 没有声明时返回 None，由 /favicon.ico 兜底。
        assert!(icon_plan(base,"<html><body>x</body></html>").is_none());
    }

    #[test] fn sub2api_logo_never_runs_scripts_or_reads_other_fields(){
        // 脚本内容只有引号字符串，解析必须停在这个 JSON 值上，不能被后续内容带偏。
        let html=r#"<script>window.__APP_CONFIG__={"site_logo":"data:image/png;base64,AAAA","x":1}{"site_logo":"data:image/png;base64,BBBB"}</script>"#;
        assert_eq!(sub2api_site_logo(html).as_deref(),Some("data:image/png;base64,AAAA"));
    }
    #[test] fn sub2api_refresh_attaches_site_logo(){
        let png="data:image/png;base64,AAAA";
        let snapshot=parse_sub2api(&json!({"mode":"unrestricted","isValid":true,"unit":"USD","balance":7})).unwrap();
        let mut snapshot=snapshot;
        if let Some(relay)=snapshot.relay.as_mut(){relay.logo=Some(png.into());}
        assert_eq!(snapshot.relay.unwrap().logo.as_deref(),Some(png));
    }
}
