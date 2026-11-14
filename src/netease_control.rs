//! 网易云音乐进度跳转：走客户端 CDP 的原生桥，不点界面坐标。
//!
//! 网易云 3.1.x 启动参数带 `--disable-features=...,MediaSessionService`，不注册系统媒体会话，
//! 因此 `TryChangePlaybackPositionAsync` 必然失败；但它前端里保留着完整的跳转命令
//! （`window.legacyNativeCmder` / OrpheusCommand.call，底层是 CEF 的 `window.channel.call`）。
//!
//! 本模块是**渐进增强**：客户端带 `--remote-debugging-port` 启动时才接管跳转；
//! 端口不在则立刻返回 false，由上层照旧提示，不反复等待超时。
//! 可用 `%APPDATA%/Daozai/netease-cdp.json` 覆盖端口与命令：
//! `{"port":9222,"commands":["audioplayer.seek"]}`。
//!
//! 命令与参数经实机验证（2026-11-13，网易云 3.1.41 + CDP 9222）：
//! `legacyNativeCmder.call('audioplayer.seek','','',秒数)` 会让原生层回
//! `Seek(playId, seekId, code, position)` 事件，`code=0` 且 position 等于请求秒数；
//! 前两个参数留空时原生层自动使用当前播放实例。`nm.play.playerSeek` /
//! `nh.player.seek` / `player.setCurrentTime` 三条虽在前端白名单里，实机**没有任何
//! 原生事件**，仅 JS 不抛异常，不能当作成功。
//!
//! 只用 std + 已有的 serde_json/base64 手写 HTTP/WebSocket，不加依赖、不动 Cargo.lock。

use std::io::{Read,Write};
use std::net::{SocketAddr,TcpStream};
use std::sync::Mutex;
use std::time::{Duration,Instant,SystemTime,UNIX_EPOCH};
use base64::Engine;

pub const DEFAULT_PORT:u16=9222;
/// 端口连不上时的冷却，避免拖动时每帧都做一次连接尝试。
const COOLDOWN:Duration=Duration::from_secs(20);
const CONNECT_TIMEOUT:Duration=Duration::from_millis(350);
/// 原生 Seek 回调用 Promise 等待，内部 1.5s 超时；读超时要留得比它长。
const FRAME_TIMEOUT:Duration=Duration::from_millis(2500);
const MAX_FRAME:usize=2*1024*1024;
/// 只保留实机确认生效的命令；旧命令可由配置文件临时加回做兼容试验。
const DEFAULT_COMMANDS:&[&str]=&["audioplayer.seek"];

#[derive(Clone,Debug,PartialEq,Eq)]
pub struct Config{pub port:u16,pub commands:Vec<String>}
impl Default for Config{
    fn default()->Self{Config{port:DEFAULT_PORT,commands:DEFAULT_COMMANDS.iter().map(|s|(*s).to_string()).collect()}}
}

/// 命令名会被拼进 JS 字符串，只放行安全的字符集。
fn sanitize_commands(value:&serde_json::Value)->Option<Vec<String>>{
    let items:Vec<String>=value.as_array()?.iter().filter_map(|item|item.as_str())
        .filter(|name|!name.is_empty() && name.len()<=64
            && name.chars().all(|c|c.is_ascii_alphanumeric()||matches!(c,'.'|'_'|'-')))
        .map(|name|name.to_string()).collect();
    (!items.is_empty()).then_some(items)
}

pub fn config()->Config{
    let mut cfg=Config::default();
    let Ok(bytes)=std::fs::read(crate::model::config_dir().join("netease-cdp.json")) else{return cfg;};
    let Ok(value)=serde_json::from_slice::<serde_json::Value>(&bytes) else{return cfg;};
    if let Some(port)=value["port"].as_u64(){if (1..=65535).contains(&port){cfg.port=port as u16;}}
    if let Some(commands)=sanitize_commands(&value["commands"]){cfg.commands=commands;}
    cfg
}

// ---------------------------------------------------------------
// 快捷方式：把 --remote-debugging-port 写进客户端快捷方式
// ---------------------------------------------------------------
// CEF 的调试端口只能在进程启动时打开，所以必须改启动参数。手工改快捷方式对普通用户太麻烦，
// 这里直接改写 .lnk 的参数字段（二进制格式，LinkFlags 置 HasArguments，其余字节原样保留），
// 幂等且先备份；扫描范围是开始菜单与桌面，按内容识别是否指向 cloudmusic.exe，
// 所以快捷方式被改名、换成英文名也能认出来，也不会误改别的程序。
fn le16(b:&[u8],at:usize)->Option<u16>{Some(u16::from_le_bytes([*b.get(at)?,*b.get(at+1)?]))}
fn le32(b:&[u8],at:usize)->Option<u32>{Some(u32::from_le_bytes([*b.get(at)?,*b.get(at+1)?,*b.get(at+2)?,*b.get(at+3)?]))}

/// 幂等地把 .lnk 的命令行参数段设为指定值；其余段（IDList / LinkInfo / ExtraData）原样保留。
/// 纯字节改写，不用 COM。返回是否成功。
fn patch_shortcut(path:&std::path::Path,arg:&str)->bool{
    let Ok(bytes)=std::fs::read(path) else{return false};
    if bytes.len()<76{return false;}
    let Some(flags)=le32(&bytes,0x14) else{return false};
    let unicode=flags&0x80!=0;
    let mut pos=76usize;
    if flags&0x1!=0{
        let Some(n)=le16(&bytes,pos) else{return false};
        pos+=2+n as usize;
    }
    if flags&0x2!=0{
        let Some(n)=le32(&bytes,pos) else{return false};
        pos+=n as usize;
    }
    if pos>bytes.len(){return false;}
    let fixed_end=pos;
    let mut parts:Vec<(u32,Vec<u8>)>=Vec::new();
    let mut q=pos;
    for bit in [0x4u32,0x8,0x10,0x20,0x40]{
        if flags&bit==0{continue;}
        let Some(count)=le16(&bytes,q) else{return false};
        let width=if unicode{2}else{1};
        let n=2+count as usize*width;
        if q+n>bytes.len(){return false;}
        parts.push((bit,bytes[q..q+n].to_vec()));
        q+=n;
    }
    let extra=&bytes[q..];
    let text:Vec<u8>=if unicode{arg.encode_utf16().flat_map(|c|c.to_le_bytes()).collect()}else{arg.as_bytes().to_vec()};
    let count=(text.len()/if unicode{2}else{1}) as u16;
    let mut newarg=count.to_le_bytes().to_vec();
    newarg.extend_from_slice(&text);
    let mut stringdata=Vec::new();
    for bit in [0x4u32,0x8,0x10,0x20,0x40]{
        if bit==0x20{stringdata.extend_from_slice(&newarg);continue;}
        if let Some((_,data))=parts.iter().find(|(b,_)|*b==bit){stringdata.extend_from_slice(data);}
    }
    let mut out=bytes[..0x14].to_vec();
    out.extend_from_slice(&(flags|0x20).to_le_bytes());
    out.extend_from_slice(&bytes[0x18..fixed_end]);
    out.extend_from_slice(&stringdata);
    out.extend_from_slice(extra);
    if out==bytes{return true;}
    let backup=path.with_extension("lnk.bak-cdp");
    if !backup.exists(){let _=std::fs::copy(path,&backup);}
    std::fs::write(path,&out).is_ok()
}

/// 快捷方式可能所在的目录：开始菜单（用户级/全局）与桌面（用户/公共）。
fn shortcut_dirs()->Vec<std::path::PathBuf>{
    let mut dirs=Vec::new();
    if let Some(appdata)=std::env::var_os("APPDATA"){
        dirs.push(std::path::PathBuf::from(appdata).join("Microsoft/Windows/Start Menu/Programs"));
    }
    dirs.push(std::path::PathBuf::from("C:/ProgramData/Microsoft/Windows/Start Menu/Programs"));
    if let Some(home)=std::env::var_os("USERPROFILE"){
        dirs.push(std::path::PathBuf::from(home).join("Desktop"));
    }
    if let Some(public)=std::env::var_os("PUBLIC"){
        dirs.push(std::path::PathBuf::from(public).join("Desktop"));
    }
    dirs
}

/// `.lnk` 的内容里是否指向网易云客户端。桌面的快捷方式名字不固定（可能被改名或英文名），
/// 只按文件名会把它们漏掉；按内容匹配才不会误改别的快捷方式。LinkInfo 里的路径通常是 ASCII，
/// 另外补 UTF-16 形态（EnvironmentVariableDataBlock）。
fn points_to_cloudmusic(bytes:&[u8])->bool{
    let ascii=b"cloudmusic.exe";
    if bytes.windows(ascii.len()).any(|window|window.eq_ignore_ascii_case(ascii)){return true;}
    for text in ["cloudmusic.exe","CloudMusic.exe"]{
        let wide:Vec<u8>=text.encode_utf16().flat_map(|unit|unit.to_le_bytes()).collect();
        if bytes.windows(wide.len()).any(|window|window==wide){return true;}
    }
    false
}

/// 所有指向网易云的快捷方式：开始菜单、用户桌面、公共桌面。
fn shortcut_paths()->Vec<std::path::PathBuf>{
    let mut found=Vec::new();
    for dir in shortcut_dirs(){collect_shortcuts(&dir,2,&mut found);}
    found
}

/// 安装器经常把快捷方式放进开始菜单的同名子文件夹（`Programs\网易云音乐\网易云音乐.lnk`），
/// 只扫第一层会漏掉它，而用户从「所有应用」点开的恰恰是这一条，所以往下递归两层。
fn collect_shortcuts(dir:&std::path::Path,depth:usize,found:&mut Vec<std::path::PathBuf>){
    let Ok(entries)=std::fs::read_dir(dir) else{return;};
    for entry in entries.flatten(){
        let path=entry.path();
        let Ok(kind)=entry.file_type() else{continue;};
        if kind.is_dir(){
            if depth>1{collect_shortcuts(&path,depth-1,found);}
            continue;
        }
        if !path.extension().and_then(|ext|ext.to_str()).is_some_and(|ext|ext.eq_ignore_ascii_case("lnk")){continue;}
        // 按内容识别；个别 .lnk 读不到内容时按标准文件名兜底。
        let named=path.file_name().and_then(|name|name.to_str())
            .is_some_and(|name|name.eq_ignore_ascii_case("网易云音乐.lnk"));
        if named || std::fs::read(&path).is_ok_and(|bytes|points_to_cloudmusic(&bytes)){found.push(path);}
    }
}

fn invalidate_shortcut_cache(){
    if let Ok(mut guard)=FIXED_CACHE.lock(){*guard=None;}
}

/// 把已存在的快捷方式都补上调试端口参数：开始菜单与桌面一起修。
/// 完全没有快捷方式时返回 false，由自动托管兜底。
fn ensure_shortcut(port:u16)->bool{
    let arg=format!("--remote-debugging-port={}",port);
    let mut ok=false;
    for path in shortcut_paths(){
        if patch_shortcut(&path,&arg){ok=true;}
    }
    if ok{invalidate_shortcut_cache();}
    ok
}

/// 用户手动指定的快捷方式：检查扩展名后幂等写入调试端口参数。
pub fn fix_shortcut(path:&std::path::Path)->Result<String,String>{
    if path.extension().and_then(|e|e.to_str()).map_or(true,|e|!e.eq_ignore_ascii_case("lnk")){
        return Err("请选择 .lnk 快捷方式".to_string());
    }
    // 只接受指向网易云的快捷方式：否则会把调试参数写到别的程序上。
    let bytes=std::fs::read(path).map_err(|error|format!("读取失败：{error}"))?;
    if !points_to_cloudmusic(&bytes){
        return Err("这不是网易云的快捷方式，请重新选择".to_string());
    }
    let arg=format!("--remote-debugging-port={}",config().port);
    if patch_shortcut(path,&arg){
        invalidate_shortcut_cache();
        Ok("已修改网易云快捷方式".to_string())
    }
    else{Err("修改失败，请检查快捷方式文件".to_string())}
}

/// 设置页状态：各处的快捷方式是否都带了调试端口参数。
/// 要遍历桌面文件，结果缓存 2 秒——否则每帧调用都会把整张桌面读一遍。
pub fn shortcut_fixed()->bool{
    if let Ok(guard)=FIXED_CACHE.lock(){
        if let Some((at,value))=*guard{
            if at.elapsed()<Duration::from_secs(2){return value;}
        }
    }
    let needle_u16:Vec<u8>="--remote-debugging-port".encode_utf16().flat_map(|c|c.to_le_bytes()).collect();
    let needle_ascii=b"--remote-debugging-port".to_vec();
    let paths=shortcut_paths();
    let value=!paths.is_empty() && paths.iter().all(|path|std::fs::read(path).is_ok_and(|bytes|{
        bytes.windows(needle_u16.len()).any(|w|w==needle_u16.as_slice())
            || bytes.windows(needle_ascii.len()).any(|w|w==needle_ascii.as_slice())
    }));
    if let Ok(mut guard)=FIXED_CACHE.lock(){*guard=Some((Instant::now(),value));}
    value
}

static FIXED_CACHE:Mutex<Option<(Instant,bool)>>=Mutex::new(None);

/// 一键修复：扫描开始菜单与桌面，给所有指向网易云的快捷方式补上调试端口参数。
/// 返回修好的数量；0 表示没找到快捷方式，交给调用方提示手动选择。
pub fn fix_all()->Result<usize,String>{
    let arg=format!("--remote-debugging-port={}",config().port);
    let mut count=0;
    for path in shortcut_paths(){
        if patch_shortcut(&path,&arg){count+=1;}
    }
    if count>0{invalidate_shortcut_cache();}
    Ok(count)
}

fn valid(seconds:f64)->bool{seconds.is_finite() && (0.0..86400.0).contains(&seconds)}

static UNAVAILABLE_UNTIL:Mutex<Option<Instant>>=Mutex::new(None);
fn cooling_down()->bool{
    UNAVAILABLE_UNTIL.lock().is_ok_and(|guard|(*guard).is_some_and(|until|Instant::now()<until))
}
fn mark_unavailable(){
    if let Ok(mut guard)=UNAVAILABLE_UNTIL.lock(){*guard=Some(Instant::now()+COOLDOWN);}
}

/// 供 smtc::seek 调用。成功时才返回 true；CDP 不在或命令没生效都不吞掉失败。
pub fn seek(seconds:f64)->bool{
    if !valid(seconds)||cooling_down(){return false;}
    let cfg=config();
    // 位置判定用原生 `Seek` 回调带回的 position；没有该字段时才退回时间轴回读，
    // 而这里没有额外的时间轴来源，因此传入 None 表示“只用原生回执判定”。
    match seek_inner(seconds,&cfg,||None,|delay|std::thread::sleep(delay)){
        Outcome::Arrived=>{super::netease_playback::note_seek(seconds);true},
        Outcome::Rejected=>false,
        Outcome::Unavailable=>{mark_unavailable();false},
    }
}

/// 实机诊断入口：连上 CDP 后执行任意表达式，返回 `result.result.value`。
/// 用来确认桥是否存在、试各条命令的实参格式，不参与正常播放流程
/// （正式构建里只被 `#[cfg(test)]` 的手动探针调用）。
#[allow(dead_code)]
pub fn evaluate(port:u16,expression:&str)->Option<serde_json::Value>{
    let mut ws=connect_page(port)?;
    let reply=ws.request(1,expression)?;
    Some(reply["result"]["result"]["value"].clone())
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
enum Outcome{Arrived,Rejected,Unavailable}

/// 位置回读与等待都注入，测试用假 CDP 服务即可覆盖全流程，不依赖真实播放器。
fn seek_inner(seconds:f64,cfg:&Config,
    mut position:impl FnMut()->Option<f64>,mut pause:impl FnMut(Duration))->Outcome{
    if !valid(seconds)||cfg.commands.is_empty(){return Outcome::Rejected;}
    let mut ws=match connect_page(cfg.port){Some(ws)=>ws,None=>return Outcome::Unavailable};
    for (index,command) in cfg.commands.iter().enumerate(){
        let reply=match ws.request(index as u64+1,&expression(command,seconds)){
            Some(reply)=>reply,None=>return Outcome::Rejected,
        };
        // 桥返回 ok:false（无此全局对象 / 抛异常 / 原生 code!=0 / 超时）就换下一条。
        if !accepts(&reply){continue;}
        // 原生 Seek 回调带回的 position 是权威回执，优先用它判定；
        // netease_timeline 依赖硬编码内存偏移，部分客户端实例上一直读不到（实测为 None）。
        if let Some(pos)=reply["result"]["result"]["value"]["position"].as_f64(){
            if (pos-seconds).abs()<=3.0{return Outcome::Arrived;}
            continue; // code=0 但跳错位置，试下一条命令
        }
        // 旧命令没有 position 字段时才退回时间轴回读。
        let started=Instant::now();
        for _ in 0..6 {
            pause(Duration::from_millis(80));
            if position().is_some_and(|current|arrived(current,seconds,started.elapsed().as_secs_f64())){
                return Outcome::Arrived;
            }
        }
        // 位置没动：可能命令存在但实参格式不对，继续试下一条。
    }
    Outcome::Rejected
}

/// 跳转是否生效以播放器实际位置为准（与 QQ 音乐回退同一套判据）。
fn arrived(current:f64,target:f64,elapsed:f64)->bool{
    current.is_finite() && current>=target-2.0 && current<=target+elapsed+2.0
}

fn accepts(reply:&serde_json::Value)->bool{
    reply.get("exceptionDetails").is_none()
        && reply["result"]["result"]["value"]["ok"].as_bool()==Some(true)
}

/// 用 Promise 等原生 `Seek` 回调：`code=0` 且 position≈目标才是真跳转。
/// `audioplayer.seek` 的实参是 (playId, seekId, value)，前两个留空即可。
/// Promise 内部自带 1.5s 超时，配合 CDP 的 `awaitPromise` 不会永久挂住。
fn expression(command:&str,seconds:f64)->String{
    let cmd=serde_json::to_string(command).unwrap_or_else(|_|"\"\"".to_string());
    let secs=format!("{:.3}",seconds);
    let args=if command=="audioplayer.seek"{format!("'','',{secs}")}else{secs.clone()};
    let call=format!("const p=c.call({cmd},{args});",cmd=cmd,args=args);
    [
        "(()=>{try{",
        "const c=window.legacyNativeCmder;",
        "if(!c||typeof c.call!=='function')return Promise.resolve({ok:false,why:'no-bridge'});",
        "return new Promise((resolve)=>{",
        "let done=false;let cb=()=>{};",
        "const finish=(v)=>{if(done)return;done=true;try{c.removeRegisterCall('Seek','audioplayer',cb)}catch(e){}resolve(v)};",
        "cb=(playId,seekId,code,position)=>finish({ok:code===0,code,position});",
        "try{c.appendRegisterCall('Seek','audioplayer',cb);",
        call.as_str(),
        "if(p&&typeof p.catch==='function')p.catch(()=>{});",
        "setTimeout(()=>finish({ok:false,why:'timeout'}),1500);",
        "}catch(e){finish({ok:false,why:String(e)});}",
        "});",
        "}catch(e){return Promise.resolve({ok:false,why:String(e)});}})()",
    ].concat()
}

// ── 端点发现 ────────────────────────────────────────────────────────────────

fn http_get(port:u16,path:&str)->Option<String>{
    let addr=SocketAddr::from(([127,0,0,1],port));
    let mut stream=TcpStream::connect_timeout(&addr,CONNECT_TIMEOUT).ok()?;
    stream.set_read_timeout(Some(FRAME_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(FRAME_TIMEOUT)).ok()?;
    write!(stream,"GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").ok()?;
    let mut buffer=Vec::new();let mut chunk=[0u8;4096];
    let header_end=loop{
        if let Some(index)=find_header_end(&buffer){break index;}
        let count=stream.read(&mut chunk).ok()?;
        if count==0||buffer.len()>65536{return None;}
        buffer.extend_from_slice(&chunk[..count]);
    };
    let head=String::from_utf8_lossy(&buffer[..header_end]).to_ascii_lowercase();
    if !head.starts_with("http/1.1 200")&&!head.starts_with("http/1.0 200"){return None;}
    let length=head.lines().find_map(|line|line.strip_prefix("content-length:"))
        .and_then(|value|value.trim().parse::<usize>().ok()).unwrap_or(0);
    while buffer.len()<header_end+length{
        let count=stream.read(&mut chunk).ok()?;
        if count==0{return None;}
        buffer.extend_from_slice(&chunk[..count]);
    }
    String::from_utf8(buffer[header_end..header_end+length].to_vec()).ok()
}

fn find_header_end(buffer:&[u8])->Option<usize>{
    buffer.windows(4).position(|window|window==b"\r\n\r\n").map(|index|index+4)
}

/// 只认本机回环的 page，避免连到别人开的调试端口。
fn pick_target(body:&str,port:u16)->Option<String>{
    let list=serde_json::from_str::<serde_json::Value>(body).ok()?.as_array()?.clone();
    let local=[format!("127.0.0.1:{port}"),format!("localhost:{port}")];
    let mut fallback=None;
    for item in &list{
        if item["type"].as_str()!=Some("page"){continue;}
        let Some(url)=item["webSocketDebuggerUrl"].as_str() else{continue;};
        let host=url.strip_prefix("ws://").and_then(|rest|rest.split('/').next()).unwrap_or("");
        if !local.iter().any(|allowed|host==allowed){continue;}
        let page_url=item["url"].as_str().unwrap_or("").to_ascii_lowercase();
        let player=["orpheus","cloudmusic","music.163","163.com"].iter().any(|key|page_url.contains(key));
        if player{return Some(url.to_string());}
        if fallback.is_none(){fallback=Some(url.to_string());}
    }
    fallback
}

fn connect_page(port:u16)->Option<Ws>{
    let body=http_get(port,"/json/list")?;
    let url=pick_target(&body,port)?;
    Ws::connect(&url)
}

// ── 极简 WebSocket 客户端（RFC 6455 客户端帧必须掩码）────────────────────────

fn encode_frame(opcode:u8,payload:&[u8],mask:Option<[u8;4]>)->Vec<u8>{
    let mut frame=Vec::with_capacity(payload.len()+14);
    frame.push(0x80|opcode);
    let flag=if mask.is_some(){0x80}else{0};
    let length=payload.len();
    if length<126{frame.push(flag|length as u8);}
    else if length<=u16::MAX as usize{frame.push(flag|126);frame.extend_from_slice(&(length as u16).to_be_bytes());}
    else{frame.push(flag|127);frame.extend_from_slice(&(length as u64).to_be_bytes());}
    if let Some(key)=mask{
        frame.extend_from_slice(&key);
        for (index,byte) in payload.iter().enumerate(){frame.push(byte^key[index%4]);}
    }else{frame.extend_from_slice(payload);}
    frame
}

/// 返回 (opcode, fin, payload, 消耗字节数)；数据不足时返回 None 让调用方继续读。
fn decode_frame(data:&[u8])->Option<(u8,bool,Vec<u8>,usize)>{
    if data.len()<2{return None;}
    let fin=data[0]&0x80!=0;let opcode=data[0]&0x0f;
    let masked=data[1]&0x80!=0;
    let mut length=(data[1]&0x7f) as usize;let mut offset=2;
    if length==126{
        if data.len()<4{return None;}
        length=u16::from_be_bytes([data[2],data[3]]) as usize;offset=4;
    }else if length==127{
        if data.len()<10{return None;}
        let mut bytes=[0u8;8];bytes.copy_from_slice(&data[2..10]);
        length=u64::from_be_bytes(bytes) as usize;offset=10;
    }
    if length>MAX_FRAME{return None;}
    let key=if masked{
        if data.len()<offset+4{return None;}
        let mut key=[0u8;4];key.copy_from_slice(&data[offset..offset+4]);offset+=4;key
    }else{[0u8;4]};
    if data.len()<offset+length{return None;}
    let mut payload=data[offset..offset+length].to_vec();
    if masked{for (index,byte) in payload.iter_mut().enumerate(){*byte^=key[index%4];}}
    Some((opcode,fin,payload,offset+length))
}

fn mask_key()->[u8;4]{
    let seed=SystemTime::now().duration_since(UNIX_EPOCH).map(|d|d.as_nanos() as u64).unwrap_or(0)
        ^(std::process::id() as u64).wrapping_mul(0x9E3779B97F4A7C15);
    let mut key=[0u8;4];
    for (index,chunk) in key.chunks_mut(1).enumerate(){
        chunk[0]=(seed.rotate_left((index*8+13) as u32)>>24) as u8;
    }
    key
}

struct Ws{stream:TcpStream,buffer:Vec<u8>}
impl Ws{
    fn connect(url:&str)->Option<Self>{
        let rest=url.strip_prefix("ws://")?;
        let (authority,tail)=rest.split_once('/')?;
        let path=format!("/{tail}");
        let port=authority.rsplit_once(':').and_then(|(_,value)|value.parse::<u16>().ok())?;
        let addr=SocketAddr::from(([127,0,0,1],port));
        let mut stream=TcpStream::connect_timeout(&addr,CONNECT_TIMEOUT).ok()?;
        stream.set_read_timeout(Some(FRAME_TIMEOUT)).ok()?;
        stream.set_write_timeout(Some(FRAME_TIMEOUT)).ok()?;
        // CDP 只校验 key 是合法的 16 字节 base64，不校验随机性。
        let key=base64::engine::general_purpose::STANDARD.encode(mask_key().repeat(4));
        write!(stream,"GET {path} HTTP/1.1\r\nHost: {authority}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").ok()?;
        let mut buffer=Vec::new();let mut chunk=[0u8;1024];
        loop{
            if find_header_end(&buffer).is_some(){break;}
            let count=stream.read(&mut chunk).ok()?;
            if count==0||buffer.len()>16384{return None;}
            buffer.extend_from_slice(&chunk[..count]);
        }
        let end=find_header_end(&buffer)?;
        let head=String::from_utf8_lossy(&buffer[..end]).to_ascii_lowercase();
        // CEF 72 回的是 "101 WebSocket Protocol Handshake"，新版 Chrome 回
        // "101 Switching Protocols"，只认状态码 101，别绑死原因短语。
        if !head.starts_with("http/1.1 101")&&!head.starts_with("http/1.0 101"){return None;}
        buffer.drain(..end);
        Some(Ws{stream,buffer})
    }

    fn send_text(&mut self,text:&str)->Option<()>{
        let frame=encode_frame(0x1,text.as_bytes(),Some(mask_key()));
        self.stream.write_all(&frame).ok()
    }

    fn recv_text(&mut self)->Option<String>{
        let mut accumulated=Vec::new();
        loop{
            if let Some((opcode,fin,payload,used))=decode_frame(&self.buffer){
                self.buffer.drain(..used);
                match opcode{
                    0x1|0x0=>{
                        accumulated.extend_from_slice(&payload);
                        if fin&&!accumulated.is_empty(){return String::from_utf8(accumulated).ok();}
                    },
                    0x9=>{
                        let pong=encode_frame(0xA,&payload,None);
                        let _=self.stream.write_all(&pong);
                    },
                    0x8=>return None,
                    _=>{},
                }
                continue;
            }
            let mut chunk=[0u8;8192];
            let count=self.stream.read(&mut chunk).ok()?;
            if count==0||self.buffer.len()>MAX_FRAME{return None;}
            self.buffer.extend_from_slice(&chunk[..count]);
        }
    }

    /// CDP 会先推事件消息，按 id 过滤出本请求的响应。
    fn request(&mut self,id:u64,expression:&str)->Option<serde_json::Value>{
        // 注入表达式返回 Promise（等原生 Seek 回调）；CDP 必须 awaitPromise 才能拿到结果。
        let message=serde_json::json!({"id":id,"method":"Runtime.evaluate","params":{
            "expression":expression,"returnByValue":true,"awaitPromise":true,"userGesture":true}});
        self.send_text(&message.to_string())?;
        let deadline=Instant::now()+FRAME_TIMEOUT;
        while Instant::now()<deadline{
            let text=self.recv_text()?;
            let Ok(value)=serde_json::from_str::<serde_json::Value>(&text)else{continue;};
            if value["id"].as_u64()==Some(id){return Some(value);}
        }
        None
    }
}

#[cfg(test)]
mod tests{
    use super::*;

    /// 桌面快捷方式名字不固定，只能按内容判断；这里锁住三种常见存储形态和误判。
    #[test]
    fn cloudmusic_shortcut_target_detection(){
        assert!(points_to_cloudmusic(b"\x1c\x00\x00\x00cloudmusic.exe\x00"));
        assert!(points_to_cloudmusic(r"D:\software\CloudMusic\cloudmusic.exe".as_bytes()));
        assert!(points_to_cloudmusic(r"C:\X\CloudMusic.EXE".as_bytes()));
        let wide:Vec<u8>=r"C:\Program Files\CloudMusic.exe".encode_utf16().flat_map(|unit|unit.to_le_bytes()).collect();
        assert!(points_to_cloudmusic(&wide));
        assert!(!points_to_cloudmusic(r"C:\Program Files\Other\player.exe".as_bytes()));
        assert!(!points_to_cloudmusic(b""));
    }
    use std::{net::TcpListener,sync::atomic::{AtomicUsize,Ordering},thread};

    fn server_frame(stream:&mut TcpStream)->Option<String>{
        let mut buffer=Vec::new();let mut chunk=[0u8;4096];
        loop{
            if let Some((opcode,_,payload,_))=decode_frame(&buffer){
                if opcode==0x1{return String::from_utf8(payload).ok();}
                return None;
            }
            let count=stream.read(&mut chunk).ok()?;
            if count==0{return None;}
            buffer.extend_from_slice(&chunk[..count]);
        }
    }

    /// 假 CDP：回 /json/list → 完成 WS 握手 → 对每条 Runtime.evaluate 回一条结果。
    /// 返回收集到的表达式，供断言命令顺序与实参。
    fn mock_cdp(respond:impl Fn(&str,usize)->serde_json::Value+Send+'static)
        ->(Config,thread::JoinHandle<Vec<String>>){
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();
        let port=listener.local_addr().unwrap().port();
        let handle=thread::spawn(move||{
            let mut seen=Vec::new();
            let (mut http,_)=listener.accept().unwrap();
            http.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut buffer=Vec::new();let mut chunk=[0u8;4096];
            while find_header_end(&buffer).is_none(){
                let count=http.read(&mut chunk).unwrap();if count==0{break;}
                buffer.extend_from_slice(&chunk[..count]);
            }
            let body=serde_json::json!([
                {"type":"page","url":"https://music.163.com/other","webSocketDebuggerUrl":format!("ws://example.com:{port}/fake")},
                {"type":"page","url":"orpheus://player/index.html","webSocketDebuggerUrl":format!("ws://127.0.0.1:{port}/devtools/page/abc")}
            ]).to_string();
            write!(http,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            drop(http);
            let (mut ws,_)=listener.accept().unwrap();
            ws.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut handshake=Vec::new();
            while find_header_end(&handshake).is_none(){
                let count=ws.read(&mut chunk).unwrap();if count==0{break;}
                handshake.extend_from_slice(&chunk[..count]);
            }
            write!(ws,"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n").unwrap();
            while let Some(text)=server_frame(&mut ws){
                let Ok(message)=serde_json::from_str::<serde_json::Value>(&text)else{break;};
                let id=message["id"].as_u64().unwrap_or(0);
                let expr=message["params"]["expression"].as_str().unwrap_or("").to_string();
                let value=respond(&expr,seen.len());
                seen.push(expr);
                let reply=serde_json::json!({"id":id,"result":{"result":{"type":"object","value":value}}});
                let frame=encode_frame(0x1,reply.to_string().as_bytes(),None);
                if ws.write_all(&frame).is_err(){break;}
            }
            seen
        });
        (Config{port,commands:vec!["nm.play.playerSeek".into(),"nh.player.seek".into()],..Config::default()},handle)
    }

    #[test]
    fn invalid_positions_never_touch_network(){
        for seconds in [f64::NAN,f64::INFINITY,-1.0,86400.0]{assert!(!valid(seconds));}
        assert!(valid(0.0));assert!(valid(12.5));
        assert_eq!(seek_inner(f64::NAN,&Config::default(),||None,|_|{}),Outcome::Rejected);
        assert_eq!(seek_inner(1.0,&Config{port:1,commands:vec![],..Config::default()},||None,|_|{}),Outcome::Rejected);
    }

    #[test]
    fn target_must_be_loopback_player_page(){
        let body=serde_json::json!([
            {"type":"page","url":"https://example.com","webSocketDebuggerUrl":"ws://example.com:9222/devtools/page/x"},
            {"type":"page","url":"orpheus://player","webSocketDebuggerUrl":"ws://127.0.0.1:9222/devtools/page/y"}
        ]).to_string();
        assert_eq!(pick_target(&body,9222).as_deref(),Some("ws://127.0.0.1:9222/devtools/page/y"));
        assert!(pick_target(&body,9223).is_none());
        assert!(pick_target("not json",9222).is_none());
        let only_other=serde_json::json!([{"type":"page","url":"about:blank","webSocketDebuggerUrl":"ws://127.0.0.1:9222/devtools/page/z"}]).to_string();
        assert_eq!(pick_target(&only_other,9222).as_deref(),Some("ws://127.0.0.1:9222/devtools/page/z"));
    }

    #[test]
    fn frames_mask_payload_and_handle_extended_length(){
        let key=[1u8,2,3,4];
        let payload=vec![b'x';300];
        let frame=encode_frame(0x1,&payload,Some(key));
        assert_eq!(frame[0],0x81);assert_eq!(frame[1]&0x7f,126);
        assert_eq!(u16::from_be_bytes([frame[2],frame[3]]),300);
        let (opcode,fin,decoded,used)=decode_frame(&frame).unwrap();
        assert_eq!((opcode,fin,used),(0x1,true,frame.len()));
        assert_eq!(decoded,payload);
        let pong=encode_frame(0xA,b"ping",None);
        assert_eq!(decode_frame(&pong).unwrap().0,0xA);
        assert!(decode_frame(&frame[..10]).is_none());
    }

    #[test]
    fn expression_uses_native_seek_and_waits_for_callback(){
        let expr=expression("audioplayer.seek",12.5);
        assert!(expr.contains("window.legacyNativeCmder"));
        assert!(expr.contains("c.call(\"audioplayer.seek\",'','',12.500)"));
        assert!(expr.contains("appendRegisterCall('Seek','audioplayer',cb)"));
        assert!(expr.contains("ok:code===0"));
        assert!(expr.contains("setTimeout"));
        // 旧命令仍可用两参数形式，方便通过配置文件做兼容试验。
        let legacy=expression("nm.play.playerSeek",30.0);
        assert!(legacy.contains("c.call(\"nm.play.playerSeek\",30.000)"));
        assert!(!accepts(&serde_json::json!({"exceptionDetails":{},"result":{}})));
        assert!(accepts(&serde_json::json!({"result":{"result":{"value":{"ok":true}}}})));
        assert!(!accepts(&serde_json::json!({"result":{"result":{"value":{"ok":false}}}})));
    }

    #[test]
    fn native_seek_callback_confirms_without_timeline(){
        // 实机关键路径：原生回执自带 position，时间轴完全读不到也应判成功。
        let (mut cfg,handle)=mock_cdp(|_,_|serde_json::json!({"ok":true,"code":0,"position":30.0}));
        cfg.commands.truncate(1);
        assert_eq!(seek_inner(30.0,&cfg,||None,|_|{}),Outcome::Arrived);
        assert_eq!(handle.join().unwrap().len(),1);
    }

    #[test]
    fn native_seek_wrong_position_is_rejected(){
        let (cfg,handle)=mock_cdp(|_,_|serde_json::json!({"ok":true,"code":0,"position":3.0}));
        assert_eq!(seek_inner(30.0,&cfg,||None,|_|{}),Outcome::Rejected);
        assert_eq!(handle.join().unwrap().len(),2);
    }

    #[test]
    fn command_override_rejects_injection(){
        let safe=serde_json::json!(["nh.player.seek","nm.play.playerSeek"]);
        assert_eq!(sanitize_commands(&safe).unwrap().len(),2);
        let bad=serde_json::json!(["nm.play.playerSeek\");alert(1);//"]);
        assert!(sanitize_commands(&bad).is_none());
        assert!(sanitize_commands(&serde_json::json!([])).is_none());
    }

    #[test]
    fn seek_confirms_real_position_over_cdp(){
        let (mut cfg,handle)=mock_cdp(|_,_|serde_json::json!({"ok":true}));
        cfg.commands.truncate(1);
        let step=AtomicUsize::new(0);
        let outcome=seek_inner(12.5,&cfg,
            ||{let n=step.fetch_add(1,Ordering::Relaxed);Some(if n==0{5.0}else{12.5})},|_|{});
        assert_eq!(outcome,Outcome::Arrived);
        let seen=handle.join().unwrap();
        assert_eq!(seen.len(),1);
        assert!(seen[0].contains("nm.play.playerSeek"));
        assert!(seen[0].contains("12.500"));
    }

    #[test]
    fn rejected_command_falls_through_to_next(){
        let (cfg,handle)=mock_cdp(|_,index|if index==0{
            serde_json::json!({"ok":false,"why":"no-bridge"})
        }else{serde_json::json!({"ok":true})});
        let step=AtomicUsize::new(0);
        let outcome=seek_inner(30.0,&cfg,
            ||{let n=step.fetch_add(1,Ordering::Relaxed);Some(if n<1{5.0}else{30.0})},|_|{});
        assert_eq!(outcome,Outcome::Arrived);
        let seen=handle.join().unwrap();
        assert_eq!(seen.len(),2);
        assert!(seen[1].contains("nh.player.seek"));
    }

    #[test]
    fn unchanged_position_is_a_failure_not_a_guess(){
        let (cfg,handle)=mock_cdp(|_,_|serde_json::json!({"ok":true}));
        assert_eq!(seek_inner(90.0,&cfg,||Some(5.0),|_|{}),Outcome::Rejected);
        assert_eq!(handle.join().unwrap().len(),2);
    }

    #[test]
    fn closed_port_is_unavailable_and_fast(){
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();
        let port=listener.local_addr().unwrap().port();drop(listener);
        let started=Instant::now();
        assert_eq!(seek_inner(10.0,&Config{port,..Config::default()},||None,|_|{}),Outcome::Unavailable);
        assert!(started.elapsed()<Duration::from_secs(2),"端口不通时应快速失败");
    }

    /// 实机只读探针：客户端带 `--remote-debugging-port` 重启后手动运行，
    /// `cargo test --locked --release --lib live_cdp_bridge_probe -- --ignored --nocapture`。
    #[test]
    #[ignore="需要网易云带 --remote-debugging-port 启动"]
    fn live_cdp_bridge_probe(){
        let cfg=config();
        let probe="(()=>({bridge:typeof window.legacyNativeCmder,call:typeof window.legacyNativeCmder?.call,channel:typeof window.channel,href:location.href}))()";
        let value=evaluate(cfg.port,probe)
            .expect("CDP 端口不可用：客户端需带 --remote-debugging-port 重启");
        println!("网易云 CDP 探测：{value}");
        assert_eq!(value["call"].as_str(),Some("function"));
    }

    /// 实机验证上层入口：smtc::seek 兜底调用的就是 `seek()`，必须返回 true。
    #[test]
    #[ignore="手动实机验证：会真实改变播放进度"]
    fn live_cdp_seek_public(){
        assert!(seek(45.0),"CDP 跳转应由原生 Seek 回执确认成功");
    }

    /// 手动实机验证：逐条试命令并打印原生回执，确认哪条命令真的能跳。
    /// 会真实改变播放进度（统一跳到 20 秒），只在明确需要验证时运行。
    #[test]
    #[ignore="手动实机验证：会真实改变播放进度"]
    fn live_cdp_seek_probe(){
        let cfg=config();
        for command in &cfg.commands{
            let before=super::super::netease_timeline::timeline();
            let reply=evaluate(cfg.port,&expression(command,20.0));
            thread::sleep(Duration::from_millis(700));
            let after=super::super::netease_timeline::timeline();
            println!("{command}: before={before:?} after={after:?} reply={reply:?}");
        }
    }
}
