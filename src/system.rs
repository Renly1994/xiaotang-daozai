//! Windows 状态采样与经用户授权的通知读取。
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{self, Receiver};
use windows::core::{Result, HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, GetLastInputInfo, LASTINPUTINFO, SetActiveWindow, keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus,
};
use windows::UI::Notifications::{NotificationKinds, UserNotification};

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub volume: Option<(u32, bool)>,
    pub audio_endpoint: Option<String>,
    pub caps: bool,
    pub network: bool,
    pub network_kind: NetworkKind,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Observation {
    pub system: Snapshot,
    pub input: Option<crate::input::State>,
    pub fullscreen: Option<crate::display_layout::Bounds>,
}

/// 跨进程输入法查询和 COM 状态采样留在后台，只在状态改变时唤醒界面。
pub fn observe(ctx:eframe::egui::Context,initial:Snapshot,input:Option<crate::input::State>)->Receiver<Observation> {
    let(tx,rx)=mpsc::sync_channel(1);
    std::thread::Builder::new().name("daozai-system".into()).spawn(move||{
        let mut current=Observation{system:initial,input,fullscreen:None};
        let mut delivered=None;
        let mut sampled=crate::poll_clock::PollClock::default();
        let mut input_at=sampled;
        loop {
            let identity=crate::input::State::identity();
            let changed=identity.map(|state|(state.context,state.layout))!=current.input.map(|state|(state.context,state.layout));
            if changed || crate::input::switch_recent() || input_at.ready(std::time::Duration::from_secs(1)){
                current.input=crate::input::State::read();input_at=crate::poll_clock::PollClock::now();
            }
            if sampled.ready(std::time::Duration::from_millis(500)){
                current.system=Snapshot::read();current.fullscreen=fullscreen();sampled=crate::poll_clock::PollClock::now();
            }
            if delivered.as_ref()!=Some(&current) {
                match tx.try_send(current.clone()) {
                    Ok(())=>{delivered=Some(current.clone());ctx.request_repaint();},
                    Err(mpsc::TrySendError::Disconnected(_))=>return,
                    Err(mpsc::TrySendError::Full(_))=>{},
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }).ok();
    rx
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NetworkKind { Wifi, Ethernet, #[default] Other }
impl Snapshot {
    pub fn read() -> Self {
        unsafe {
            let device=audio_device().ok();
            let audio_endpoint=device.as_ref().and_then(|d|d.GetId().ok()).and_then(|id|{
                let text=id.to_string().ok();
                windows::Win32::System::Com::CoTaskMemFree(Some(id.0 as _));
                text
            });
            let volume = device.and_then(|d|d.Activate::<IAudioEndpointVolume>(CLSCTX_ALL,None).ok()).and_then(|a| {
                Some((
                    (a.GetMasterVolumeLevelScalar().ok()? * 100.0).round() as u32,
                    a.GetMute().ok()?.as_bool(),
                ))
            });
            let profile=windows::Networking::Connectivity::NetworkInformation::GetInternetConnectionProfile().ok();
            let network=profile.as_ref().and_then(|p|p.GetNetworkConnectivityLevel().ok()).is_some_and(|v|v==windows::Networking::Connectivity::NetworkConnectivityLevel::InternetAccess);
            let network_kind=profile.as_ref().map(|p| {
                if p.IsWlanConnectionProfile().unwrap_or(false){NetworkKind::Wifi}
                else if p.NetworkAdapter().ok().and_then(|a|a.IanaInterfaceType().ok())==Some(6){NetworkKind::Ethernet}
                else{NetworkKind::Other}
            }).unwrap_or_default();
            Self{volume,audio_endpoint,network,network_kind,caps:GetKeyState(0x14)&1!=0}
        }
    }
    pub fn changes(&self, old: &Self) -> Vec<(String, String)> {
        let mut out = vec![];
        if self.audio_endpoint == old.audio_endpoint && self.volume != old.volume {
            if let Some((v, mute)) = self.volume {
                out.push((
                    "系统音量".into(),
                    if mute {
                        "已静音".into()
                    } else {
                        format!("音量 {v}%")
                    },
                ));
            }
        }
        if self.caps != old.caps {
            out.push((
                "大写锁定".into(),
                if self.caps { "已开启" } else { "已关闭" }.into(),
            ));
        }
        if self.network != old.network || self.network && self.network_kind != old.network_kind {
            let kind=if self.network {self.network_kind}else{old.network_kind};
            let status=if self.network {"网络已连接"}else{"网络已断开"};
            out.push((
                "网络连接".into(),
                match kind {NetworkKind::Wifi=>format!("Wi-Fi · {status}"),NetworkKind::Ethernet=>format!("以太网 · {status}"),NetworkKind::Other=>status.into()},
            ));
        }
        out
    }
}
unsafe fn audio_device() -> Result<IMMDevice> {    thread_local! {static INITIALIZED:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};}
    INITIALIZED.with(|v| {
        if !v.replace(true) {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
    });
    // 每个采样线程复用枚举器，但每次仍查询当前默认设备，保留热插拔响应。
    thread_local! {static ENUMERATOR:std::cell::RefCell<Option<IMMDeviceEnumerator>>=const{std::cell::RefCell::new(None)};}
    ENUMERATOR.with(|slot| {
        let mut slot=slot.borrow_mut();
        if slot.is_none(){*slot=Some(CoCreateInstance(&MMDeviceEnumerator,None,CLSCTX_ALL)?);}
        let result=slot.as_ref().unwrap().GetDefaultAudioEndpoint(eRender,eConsole);
        if result.is_err(){*slot=None;}
        result
    })
}
unsafe fn audio() -> Result<IAudioEndpointVolume> {audio_device()?.Activate(CLSCTX_ALL,None)}
/// 设置主音量（0..100）；同时解除静音，拖动音量条时立刻能听到。
#[cfg(windows)]
pub fn set_volume(percent: u32) {
    unsafe {
        if let Ok(a) = audio() {
            let _ = a.SetMasterVolumeLevelScalar((percent.min(100) as f32) / 100.0, std::ptr::null());
            let _ = a.SetMute(false, std::ptr::null());
        }
    }
}

/// 切换静音，返回切换后的静音状态（拿不到设备时返回 None）。
#[cfg(windows)]
pub fn toggle_mute() -> Option<bool> {
    unsafe {
        let a = audio().ok()?;
        let muted = a.GetMute().ok()?.as_bool();
        a.SetMute(!muted, std::ptr::null()).ok()?;
        Some(!muted)
    }
}

/// 当前会话是否锁屏。查询走 WTS（一次本机 RPC），按 2 秒缓存，避免每帧都查。
pub fn screen_locked() -> bool {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use windows::Win32::System::RemoteDesktop::{
        WTSFreeMemory, WTSQuerySessionInformationW, WTSSessionInfoEx, WTSINFOEXW, WTS_CURRENT_SERVER_HANDLE,
        WTS_CURRENT_SESSION, WTS_SESSIONSTATE_LOCK,
    };
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, locked)) = *cache {
        if at.elapsed() < Duration::from_secs(2) { return locked; }
    }
    let locked = unsafe {
        let mut buf = windows::core::PWSTR::null();
        let mut len = 0u32;
        if WTSQuerySessionInformationW(WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTSSessionInfoEx, &mut buf, &mut len).is_ok() && !buf.is_null() {
            let info = &*(buf.0 as *const WTSINFOEXW);
            let locked = info.Level == 1 && info.Data.WTSInfoExLevel1.SessionFlags == WTS_SESSIONSTATE_LOCK as i32;
            WTSFreeMemory(buf.0 as *mut core::ffi::c_void);
            locked
        } else {
            false
        }
    };
    *cache = Some((Instant::now(), locked));
    locked
}

pub fn idle_seconds() -> f64 {
    unsafe {
        let mut info = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            ..Default::default()
        };
        if GetLastInputInfo(&mut info).as_bool() {
            windows::Win32::System::SystemInformation::GetTickCount().wrapping_sub(info.dwTime)
                as f64
                / 1000.0
        } else {
            0.0
        }
    }
}
pub fn fullscreen() -> Option<crate::display_layout::Bounds> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == windows::Win32::System::Threading::GetCurrentProcessId() {
            return None;
        }
        // 桌面与资源管理器桌面窗口不能当成全屏游戏。
        let mut cls = [0u16; 64];
        let n = GetClassNameW(hwnd, &mut cls);
        let cls = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
        if matches!(cls.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd") {
            return None;
        }
        let mut r = RECT::default();
        if GetWindowRect(hwnd, &mut r).is_err() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info).as_bool()
        {
            return None;
        }
        let covers=r.left <= info.rcMonitor.left
            && r.top <= info.rcMonitor.top
            && r.right >= info.rcMonitor.right
            && r.bottom >= info.rcMonitor.bottom;
        covers.then_some(crate::display_layout::Bounds {left:info.rcMonitor.left,top:info.rcMonitor.top,right:info.rcMonitor.right,bottom:info.rcMonitor.bottom})
    }
}
pub(crate) fn is_browser_process(name:&str)->bool {
    ["chrome.exe","msedge.exe","firefox.exe","brave.exe","vivaldi.exe","opera.exe",
        "arc.exe","zen.exe","chromium.exe","librewolf.exe","floorp.exe","waterfox.exe",
        "duckduckgo.exe","360se.exe","360chrome.exe","qqbrowser.exe","sogouexplorer.exe","quark.exe"]
        .iter().any(|browser|name.eq_ignore_ascii_case(browser))
}

fn browser_overlaps_island(browser:RECT,island:RECT,dock:crate::model::Dock)->bool {
    if dock.is_side() {return window_covers_side(browser,island,dock);}
    // 窗口是 CANVAS_W 宽的透明画布，岛体中心固定在画布左侧 WIN_W/2 处（与水平位置、大小无关）。
    let scale=(island.right-island.left) as f32/crate::app::CANVAS_W;
    let center=island.left as f32+crate::app::WIN_W*scale/2.0;
    let half=crate::app::PW*scale/2.0;
    (browser.left as f32)<center+half && (browser.right as f32)>center-half
        && (browser.top as f32)<island.top as f32+crate::app::BAR_H*scale && browser.bottom>island.top
}

/// 左右停靠：画布是 SIDE_W × SIDE_H，岛体贴在画布的左 / 右边、竖直居中。
/// 前台窗口盖住岛所在的那一段屏幕边缘（例如最大化、全屏或贴边摆放）即视为遮挡。
fn window_covers_side(window:RECT,island:RECT,dock:crate::model::Dock)->bool {
    let scale=(island.right-island.left) as f32/crate::side_dock::SIDE_W;
    let cy=island.top as f32+crate::side_dock::SIDE_H*scale/2.0;
    let half=crate::app::PH*scale/2.0;
    let strip=crate::side_dock::SIDE_BAR_W*scale;
    let vertical=(window.top as f32)<cy+half && (window.bottom as f32)>cy-half;
    let horizontal=if dock==crate::model::Dock::Left {(window.left as f32)<island.left as f32+strip && window.right>island.left}
        else {(window.right as f32)>island.right as f32-strip && window.left<island.right};
    vertical && horizontal
}

pub fn browser_at_top(island:HWND,settings:&crate::model::Settings)->Option<bool> {
    unsafe {
        let foreground=GetForegroundWindow();
        if foreground.0.is_null(){return None;}
        let mut pid=0;
        GetWindowThreadProcessId(foreground,Some(&mut pid));
        // 设置、托盘和岛内输入沿用此前的避让状态，避免自身窗口引起反复切换。
        if pid==0 || pid==GetCurrentProcessId(){return None;}
        let mut browser_rect=RECT::default();let mut island_rect=RECT::default();
        if GetWindowRect(foreground,&mut browser_rect).is_err() || GetWindowRect(island,&mut island_rect).is_err(){return None;}
        if !browser_overlaps_island(browser_rect,island_rect,settings.dock){return Some(false);}
        let Ok(process)=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,false,pid) else{return Some(false)};
        let mut path=[0u16;1024];let mut length=path.len() as u32;
        let result=QueryFullProcessImageNameW(process,PROCESS_NAME_WIN32,PWSTR(path.as_mut_ptr()),&mut length);
        let _=CloseHandle(process);
        if result.is_err(){return Some(false);}
        let path=String::from_utf16_lossy(&path[..length as usize]);
        Some(avoidance_matches(&path,settings))
    }
}

pub enum Incoming {
    Revoked,
    Status(String),
    Message(String, String, String, Option<crate::messenger::Target>),
}
#[path="toast_source.rs"]
mod toast_source;

fn notification_identity(n:&UserNotification,number:u32,created:i64,known:Option<crate::messenger::Messenger>)->(String,String){
    use crate::messenger::Messenger;
    let info=n.AppInfo().ok();
    let id=info.as_ref().and_then(|app|app.AppUserModelId().ok()).map(|id|id.to_string()).unwrap_or_default();
    let client=if !id.is_empty(){Messenger::from_notification_id(&id)}
        else{known.or_else(||toast_source::sender(number,created).and_then(|id|Messenger::from_notification_id(&id)))};
    if let Some(client)=client{return (client.name().into(),client.adapter().notification_id.into());}
    let name=info.and_then(|app|app.DisplayInfo().ok()).and_then(|display|display.DisplayName().ok())
        .map(|name|name.to_string()).filter(|name|!name.trim().is_empty()).unwrap_or_else(||"Windows 通知".into());
    let id=if id.is_empty(){name.clone()}else{id};
    (name,id)
}
fn notification_body(n: &UserNotification) -> Result<String> {
    let binding = n
        .Notification()?
        .Visual()?
        .GetBinding(&HSTRING::from("ToastGeneric"))?;
    let texts = binding.GetTextElements()?;
    let mut parts = vec![];
    for i in 0..texts.Size()?.min(8) {
        parts.push(texts.GetAt(i)?.Text()?.to_string().chars().take(300).collect::<String>());
    }
    Ok(parts.join(" · "))
}
/// 停止信号独立于消息队列，断开后结束等待，不再读取通知。
pub struct Notifications {
    incoming: Receiver<Incoming>,
    stop: mpsc::Sender<()>,
    taskbar_mask:std::sync::Arc<std::sync::atomic::AtomicU8>,
}
impl Notifications {
    pub fn try_recv(&self)->std::result::Result<Incoming,mpsc::TryRecvError>{self.incoming.try_recv()}
    pub fn taskbar_settings(&self,settings:&crate::model::Settings){self.taskbar_mask.store(crate::messenger::toast_suppression(settings),std::sync::atomic::Ordering::Relaxed);}
}
impl Drop for Notifications {fn drop(&mut self){let _=self.stop.send(());}}

#[derive(Clone,Copy,PartialEq,Eq)]
enum ToastBody {Skipped,Unavailable,Read(u64)}
#[derive(Clone,Copy,PartialEq,Eq)]
struct ToastVersion { created:i64, client:Option<crate::messenger::Messenger>, body:ToastBody }
impl ToastVersion {
    fn new(created:i64,client:Option<crate::messenger::Messenger>,body:Option<&str>)->Self{
        Self{created,client,body:body.map(|body|{let mut hash=std::collections::hash_map::DefaultHasher::new();body.hash(&mut hash);ToastBody::Read(hash.finish())}).unwrap_or(ToastBody::Skipped)}
    }
}
struct NotificationBaseline { previous:Option<HashMap<u32,ToastVersion>>,started:i64,primed:bool }
impl Default for NotificationBaseline {
    fn default()->Self{
        let started=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()
            .and_then(|time|i64::try_from(time.as_nanos()/100+116444736000000000).ok()).unwrap_or(i64::MAX);
        Self{previous:None,started,primed:false}
    }
}
impl NotificationBaseline {
    fn unchanged_other(&self,id:u32,created:i64)->Option<ToastVersion>{
        self.previous.as_ref()?.get(&id).copied().filter(|old|old.created==created && old.client.is_none() && old.body==ToastBody::Skipped)
    }
    fn changed(&self,id:u32,version:ToastVersion)->bool{
        let Some(previous)=&self.previous else{return version.created>self.started;};
        let Some(old)=previous.get(&id) else{return self.primed || version.created>self.started;};
        if old.created!=version.created{return true;}
        // 未读取、读取失败后的首次成功只建立指纹，避免通道切换和恢复读取时重放旧通知。
        matches!((old.body,version.body),(ToastBody::Read(before),ToastBody::Read(after)) if before!=after)
    }
    fn finish(&mut self,mut current:HashMap<u32,ToastVersion>,complete:bool){
        if !complete{if let Some(previous)=&self.previous{for (&id,&old) in previous{current.entry(id).or_insert(old);}}}
        self.previous=Some(current);self.primed|=complete;
    }
    fn observe(&self,id:u32,created:i64,client:Option<crate::messenger::Messenger>,tracked:bool,body:Result<String>)->(ToastVersion,Option<String>){
        let body=match body{
            Ok(body)=>body,
            Err(_)=>{
                if let Some(old)=self.previous.as_ref().and_then(|items|items.get(&id)).filter(|old|old.created==created && old.client==client){return (*old,None);}
                let version=ToastVersion{created,client,body:ToastBody::Unavailable};
                // 已确认是新通知时仍给通用提醒；恢复正文后只补指纹，不重复弹出。
                let message=self.changed(id,version).then(||if tracked{"有新消息".into()}else{"收到新通知".into()});
                return (version,message);
            }
        };
        let version=ToastVersion::new(created,client,tracked.then_some(body.as_str()));
        let message=self.changed(id,version).then(||if tracked && body.trim().is_empty(){"有新消息".into()}else{body});
        (version,message)
    }
}

fn channel_status(client:crate::messenger::Messenger)->String{
    use windows::UI::Notifications::{NotificationSetting,ToastNotificationManager};
    let name=client.name();
    match ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(client.adapter().notification_id)).and_then(|notifier|notifier.Setting()){
        Ok(NotificationSetting::Enabled)=>format!("{name} 系统通知通道可用"),
        Ok(NotificationSetting::DisabledForApplication)=>format!("{name} 系统通知已关闭，请在 Windows 通知设置中开启后重新连接"),
        Ok(NotificationSetting::DisabledForUser)=>"Windows 系统通知已关闭，请开启后重新连接".into(),
        Ok(NotificationSetting::DisabledByGroupPolicy)=>"系统策略禁止通知".into(),
        _=>format!("{name} 系统通知通道未就绪，请检查安装和通知设置后重新连接"),
    }
}

/// 必须由窗口线程发起权限请求，后台仅读取新增或更新的通知。
pub fn connect(ctx:eframe::egui::Context,filter:crate::messenger::NotificationFilter) -> Result<Notifications> {
    connect_inner(ctx,filter,crate::messenger::toast_suppression(&crate::model::Settings::default()))
}
pub fn connect_with_settings(ctx:eframe::egui::Context,settings:&crate::model::Settings)->Result<Notifications>{
    connect_inner(ctx,crate::messenger::NotificationFilter::for_settings(settings),crate::messenger::toast_suppression(settings))
}
fn connect_inner(ctx:eframe::egui::Context,filter:crate::messenger::NotificationFilter,mask:u8)->Result<Notifications>{
    let baseline=NotificationBaseline::default();
    let listener = UserNotificationListener::Current()?;
    let request = listener.RequestAccessAsync()?;
    let (tx, rx) = mpsc::sync_channel(32);
    let (stop, stopped)=mpsc::channel();
    let taskbar_mask=std::sync::Arc::new(std::sync::atomic::AtomicU8::new(mask));let worker_mask=taskbar_mask.clone();
    std::thread::Builder::new().name("daozai-notifications".into()).spawn(move || {
        let send=|event|{
            let result=tx.send(event);
            if result.is_ok(){ctx.request_repaint();}
            result.is_ok()
        };
        let run = || -> Result<()> {
            unsafe {windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED)}?;
            struct Apartment;
            impl Drop for Apartment {fn drop(&mut self){unsafe{windows::Win32::System::WinRT::RoUninitialize();}}}
            let _apartment=Apartment;
            if request.get()? != UserNotificationListenerAccessStatus::Allowed {
                send(Incoming::Revoked);
                send(Incoming::Status("Windows 未授予通知读取权限，可在系统设置中调整。".into()));
                return Ok(());
            }
            let listener = UserNotificationListener::Current()?;
            let channels=crate::messenger::Messenger::ALL.into_iter().filter(|client|client.adapter().capture==crate::messenger::Capture::SystemToast)
                .filter(|client|filter.allows(client.adapter().notification_id)).map(channel_status).collect::<Vec<_>>().join("\n");
            let mut baseline=baseline;
            let mut accounts=crate::messenger::ToastAccounts::default();accounts.sample();
            let mut last_status=String::new();
            loop {
                if stopped.try_recv()!=Err(mpsc::TryRecvError::Empty){return Ok(());}
                let (items,count)=match listener.GetAccessStatus().and_then(|access|{
                    if access!=UserNotificationListenerAccessStatus::Allowed{return Ok(None);}
                    let items=listener.GetNotificationsAsync(NotificationKinds::Toast)?.get()?;let count=items.Size()?;
                    Ok(Some((items,count)))
                }){
                    Ok(Some(items))=>items,
                    Ok(None)=>{send(Incoming::Revoked);send(Incoming::Status("通知权限已被关闭".into()));return Ok(());}
                    Err(error)=>{
                        let status=format!("Windows 通知暂时读取失败：{} · 将自动重试",error.code());
                        if status!=last_status{send(Incoming::Status(status.clone()));last_status=status;}
                        if stopped.recv_timeout(std::time::Duration::from_secs(2))!=Err(mpsc::RecvTimeoutError::Timeout){return Ok(());}
                        continue;
                    }
                };
                accounts.sample();
                let mut current=HashMap::new();
                let mut complete=true;
                let mut messages=Vec::new();
                for i in 0..count {
                    let Ok(n)=items.GetAt(i) else{complete=false;continue};
                    let (Ok(number),Ok(created))=(n.Id(),n.CreationTime()) else{complete=false;continue};
                    let created=created.UniversalTime;
                    if let Some(old)=baseline.unchanged_other(number,created){current.insert(number,old);continue;}
                    let known=baseline.previous.as_ref().and_then(|old|old.get(&number)).filter(|old|old.created==created).and_then(|old|old.client);
                    let (app,id)=notification_identity(&n,number,created,known);
                    // 微信只走任务栏事件，即使客户端同时发送系统通知也不读取其正文。
                    let client=crate::messenger::Messenger::from_notification_id(&id);
                    if !filter.allows(&id) || crate::messenger::suppresses_toast(&app,&id,worker_mask.load(std::sync::atomic::Ordering::Relaxed)){current.insert(number,ToastVersion::new(created,client,None));continue;}
                    let tracked=client.is_some();
                    // 已适配客户端同时检查正文变化；其他应用的启动历史不需要读取正文。
                    let body=if tracked || baseline.previous.is_some() || created>baseline.started{notification_body(&n)}else{Ok(String::new())};
                    let (version,body)=baseline.observe(number,created,client,tracked,body);
                    current.insert(number,version);
                    if let Some(body)=body{
                        let target=client.filter(|client|client.adapter().capture==crate::messenger::Capture::SystemToast).and_then(|client|accounts.target(client,number,created));
                        if !body.trim().is_empty(){messages.push((created,app,id,body,target));}
                    }
                }
                baseline.finish(current,complete);
                accounts.retain(|client,id,created|baseline.previous.as_ref().and_then(|items|items.get(&id)).is_some_and(|item|item.created==created && item.client==Some(client)));
                messages.sort_by_key(|item|item.0);
                for (_,app,id,body,target) in messages {
                    if !send(Incoming::Message(app,id,body,target)){return Ok(());}
                }
                let status=match filter{crate::messenger::NotificationFilter::Only(client)=>format!("仅监听 {} · {channels}",client.name()),crate::messenger::NotificationFilter::All=>format!("正在监听 Windows 通知 · 当前可见 {count} 条\n{channels}")};
                if status!=last_status {
                    if !send(Incoming::Status(status.clone())){return Ok(());}
                    last_status=status;
                }
                if stopped.recv_timeout(std::time::Duration::from_secs(1))!=Err(mpsc::RecvTimeoutError::Timeout){return Ok(());}
            }
        };
        if let Err(e) = run() {
            send(Incoming::Status(format!("Windows 暂不允许读取通知：{}。请检查通知访问权限与应用身份。",e.code())));
        }
    }).map_err(|_|windows::core::Error::from_win32())?;
    Ok(Notifications{incoming:rx,stop,taskbar_mask})
}

pub fn pick_files() -> Result<Vec<std::path::PathBuf>> {
    pick_files_for("添加文件到置物架",true)
}

/// 选择一个文件夹（返回 None = 用户取消）。
pub fn pick_folder(title:&str) -> Result<Option<std::path::PathBuf>> {
    unsafe {
        use windows::Win32::UI::Shell::*;
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL)?;
        // 只让选文件夹，且必须是已存在的文件系统路径。
        dialog.SetOptions(FOS_FORCEFILESYSTEM | FOS_PICKFOLDERS | FOS_PATHMUSTEXIST)?;
        dialog.SetTitle(&HSTRING::from(title))?;
        if let Err(e) = dialog.Show(None) {
            if e.code().0 as u32 == 0x800704c7 { return Ok(None); } // 用户取消
            return Err(e);
        }
        let item = dialog.GetResult()?;
        let ptr = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let text = ptr.to_string()?;
        windows::Win32::System::Com::CoTaskMemFree(Some(ptr.0 as _));
        Ok(Some(text.into()))
    }
}

/// 选择一个程序文件（返回 None = 用户取消）。
pub fn pick_exe(title:&str) -> Result<Option<std::path::PathBuf>> {
    unsafe {
        use windows::Win32::UI::Shell::*;
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL)?;
        dialog.SetOptions(FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST)?;
        dialog.SetTitle(&HSTRING::from(title))?;
        // 默认只列 .exe，用户仍可在文件名框里输入其它路径。
        let name: Vec<u16> = "程序 (*.exe)\0".encode_utf16().collect();
        let spec: Vec<u16> = "*.exe\0".encode_utf16().collect();
        let filters = [Common::COMDLG_FILTERSPEC {
            pszName: windows::core::PCWSTR(name.as_ptr()),
            pszSpec: windows::core::PCWSTR(spec.as_ptr()),
        }];
        let _ = dialog.SetFileTypes(&filters);
        if let Err(e) = dialog.Show(None) {
            if e.code().0 as u32 == 0x800704c7 { return Ok(None); }
            return Err(e);
        }
        let item = dialog.GetResult()?;
        let ptr = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let text = ptr.to_string()?;
        windows::Win32::System::Com::CoTaskMemFree(Some(ptr.0 as _));
        Ok(Some(text.into()))
    }
}

// —— 关闭其它应用的 Windows 通知横幅 ——
// Windows 没有公开 API 改别人的通知设置；ShowBanner 是 HKCU 下的内部项，
// 写 0 表示「不弹右下角横幅」，通知仍会进通知中心，岛仔照常读取。
// 原值记在 banner-overrides.json，随时可以一键恢复；每次操作逐条写日志。
const BANNER_ROOT:&str="SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Notifications\\Settings";
const BANNER_VALUE:&str="ShowBanner";
const BANNER_FILE:&str="banner-overrides.json";
const BANNER_LOG:&str="notification-banner.log";

fn banner_wide(text:&str)->Vec<u16>{text.encode_utf16().chain(Some(0)).collect()}
fn banner_log_path()->std::path::PathBuf{crate::model::config_dir().join("diagnostics").join(BANNER_LOG)}

fn banner_log(message:&str){
    use std::io::Write as _;
    let path=banner_log_path();
    if let Some(dir)=path.parent(){let _=std::fs::create_dir_all(dir);}
    // 别让它无限长：超过 256 KB 就从头写。
    if std::fs::metadata(&path).is_ok_and(|meta|meta.len()>256*1024){let _=std::fs::write(&path,b"");}
    if let Ok(mut file)=std::fs::OpenOptions::new().create(true).append(true).open(&path){
        let stamp=chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _=writeln!(file,"{stamp} | {message}");
    }
}

/// 设置页显示的上次结果；找不到就返回 None。
pub fn banner_last_summary()->Option<String>{
    let text=std::fs::read_to_string(banner_log_path()).ok()?;
    text.lines().rev().find(|line|line.contains("| 汇总 |"))
        .and_then(|line|line.split_once("| 汇总 |"))
        .map(|(_,rest)|rest.trim().to_string())
}

pub fn open_banner_log()->std::result::Result<(),String>{
    let path=banner_log_path();
    if !path.exists(){
        if let Some(dir)=path.parent(){let _=std::fs::create_dir_all(dir);}
        let _=std::fs::write(&path,b"");
    }
    std::process::Command::new("explorer.exe").arg(&path).spawn().map(|_|())
        .map_err(|error|format!("打开日志失败：{error}"))
}

fn win32_message(code:u32)->String{
    format!("错误 {code}：{}",std::io::Error::from_raw_os_error(code as i32))
}
fn banner_value_text(value:Option<u32>)->String{
    match value{
        None=>"不存在（默认弹横幅）".to_string(),
        Some(0)=>"0（原本已关）".to_string(),
        Some(1)=>"1（原本弹横幅）".to_string(),
        Some(v)=>v.to_string(),
    }
}

#[derive(Default,serde::Serialize,serde::Deserialize)]
struct BannerOverrides{entries:std::collections::BTreeMap<String,Option<u32>>}
fn banner_file()->std::path::PathBuf{crate::model::config_dir().join(BANNER_FILE)}
fn banner_load()->BannerOverrides{
    std::fs::read(banner_file()).ok().and_then(|bytes|serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}
fn banner_save(state:&BannerOverrides)->std::result::Result<(),String>{
    let bytes=serde_json::to_vec_pretty(state).map_err(|error|error.to_string())?;
    std::fs::write(banner_file(),bytes).map_err(|error|format!("保存失败：{error}"))
}

/// None = 这个值原本不存在（默认会弹横幅）；Some(0) = 原本就关闭。
fn banner_original(app:&str)->Option<u32>{
    use windows::{core::PCWSTR,Win32::System::Registry::*};
    let path=banner_wide(&format!("{BANNER_ROOT}\\{app}"));
    let name=banner_wide(BANNER_VALUE);
    let mut value=0u32;let mut size=std::mem::size_of::<u32>() as u32;
    let rc=unsafe{RegGetValueW(HKEY_CURRENT_USER,PCWSTR(path.as_ptr()),PCWSTR(name.as_ptr()),
        RRF_RT_REG_DWORD,None,Some((&mut value as *mut u32).cast()),Some(&mut size))};
    (rc.0==0).then_some(value)
}

/// Some(v) 写值；None 删掉该值（回到默认弹横幅）。失败原因带 Windows 错误码与描述。
fn banner_set(app:&str,value:Option<u32>)->std::result::Result<(),String>{
    use windows::{core::PCWSTR,Win32::System::Registry::*};
    let path=banner_wide(&format!("{BANNER_ROOT}\\{app}"));
    let name=banner_wide(BANNER_VALUE);
    let mut key=HKEY::default();
    unsafe{
        let opened=RegCreateKeyExW(HKEY_CURRENT_USER,PCWSTR(path.as_ptr()),0,PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,KEY_QUERY_VALUE|KEY_SET_VALUE,None,&mut key,None);
        if opened.0!=0{return Err(format!("打不开通知设置（{}）",win32_message(opened.0)));}
        let rc=match value{
            Some(v)=>RegSetValueExW(key,PCWSTR(name.as_ptr()),0,REG_DWORD,Some(&v.to_le_bytes())),
            None=>{
                // 值不存在本来就是默认状态，删不掉也无所谓。
                let _=RegDeleteValueW(key,PCWSTR(name.as_ptr()));
                let _ = RegCloseKey(key);
                return Ok(());
            },
        };
        let _ = RegCloseKey(key);
        if rc.0==0{Ok(())}else{Err(format!("写 ShowBanner 失败（{}）",win32_message(rc.0)))}
    }
}

pub fn banner_overridden_count()->usize{banner_load().entries.len()}

/// 只动岛仔应用列表里已勾选的应用；单个失败不影响后面的应用。返回 (成功数, 失败数)。
pub fn close_notification_banners(apps:&[String])->std::result::Result<(usize,usize),String>{
    let mut state=banner_load();
    banner_log(&format!("开始 | 关闭横幅 | 待处理 {} 个",apps.len()));
    let (mut ok,mut fail,mut skip)=(0,0,0);
    for app in apps{
        if app.trim().is_empty(){skip+=1;banner_log("跳过 | 应用名为空");continue;}
        // 第一次记录原值；重复点「关闭」不能把原值覆盖成 0，否则恢复时还原不回去。
        let original=match state.entries.get(app){Some(v)=>*v,None=>banner_original(app)};
        match banner_set(app,Some(0)){
            Ok(())=>{
                state.entries.entry(app.clone()).or_insert(original);
                ok+=1;
                banner_log(&format!("成功 | {app} | {} -> 0",banner_value_text(original)));
            },
            Err(error)=>{fail+=1;banner_log(&format!("失败 | {app} | {error}"));},
        }
    }
    let saved=match banner_save(&state){
        Ok(())=>"状态文件已保存".to_string(),
        Err(error)=>format!("状态文件保存失败：{error}"),
    };
    banner_log(&format!("| 汇总 | 关闭 | 成功={ok} 失败={fail} 跳过={skip} | {saved}"));
    if saved.starts_with("状态文件保存失败"){return Err(saved);}
    Ok((ok,fail))
}

/// 恢复失败的应用继续留在记录里，下次还能重试。返回 (成功数, 失败数)。
pub fn restore_notification_banners()->std::result::Result<(usize,usize),String>{
    let state=banner_load();
    banner_log(&format!("开始 | 恢复横幅 | 待处理 {} 个",state.entries.len()));
    let (mut ok,mut fail)=(0,0);
    let mut remaining=BannerOverrides::default();
    for (app,original) in &state.entries{
        match banner_set(app,*original){
            Ok(())=>{
                ok+=1;
                banner_log(&format!("成功 | {app} | 恢复为 {}",banner_value_text(*original)));
            },
            Err(error)=>{
                fail+=1;
                banner_log(&format!("失败 | {app} | {error}"));
                remaining.entries.insert(app.clone(),*original);
            },
        }
    }
    let saved=match banner_save(&remaining){
        Ok(())=>"状态文件已保存".to_string(),
        Err(error)=>format!("状态文件保存失败：{error}"),
    };
    banner_log(&format!("| 汇总 | 恢复 | 成功={ok} 失败={fail} | {saved}"));
    if saved.starts_with("状态文件保存失败"){return Err(saved);}
    Ok((ok,fail))
}

/// 兜底：直接改注册表不一定立刻生效，给用户一个官方设置入口。
pub fn open_notification_settings()->std::result::Result<(),String>{
    std::process::Command::new("explorer.exe").arg("ms-settings:notifications")
        .spawn().map(|_|()).map_err(|error|format!("打开设置失败：{error}"))
}

pub fn pick_files_for(title:&str,multiple:bool) -> Result<Vec<std::path::PathBuf>> {
    unsafe {
        use windows::Win32::UI::Shell::*;
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL)?;
        // 选 .lnk 时要拿快捷方式本身，不能让对话框把它解析成目标程序。
        let mut options=FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST | FOS_NODEREFERENCELINKS;
        if multiple{options|=FOS_ALLOWMULTISELECT;}
        dialog.SetOptions(options)?;
        dialog.SetTitle(&HSTRING::from(title))?;
        if let Err(e) = dialog.Show(None) {
            if e.code().0 as u32 == 0x800704c7 {
                return Ok(vec![]);
            }
            return Err(e);
        }
        let items = dialog.GetResults()?;
        let mut out = vec![];
        for i in 0..items.GetCount()? {
            let ptr = items.GetItemAt(i)?.GetDisplayName(SIGDN_FILESYSPATH)?;
            let text = ptr.to_string()?;
            windows::Win32::System::Com::CoTaskMemFree(Some(ptr.0 as _));
            out.push(text.into());
        }
        Ok(out)
    }
}
/// 构建资源管理器可识别的文件数据对象。
fn file_data_object(path: &std::path::Path) -> Result<windows::Win32::System::Com::IDataObject> {
    unsafe {
        use windows::Win32::UI::Shell::*;
        let wide: Vec<u16> = path
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let pidl = ILCreateFromPathW(windows::core::PCWSTR(wide.as_ptr()));
        if pidl.is_null() {
            return Err(windows::core::Error::from_win32());
        }
        let result = (|| {
            let mut child = std::ptr::null_mut();
            let folder: IShellFolder = SHBindToParent(pidl, Some(&mut child))?;
            folder.GetUIObjectOf(None, &[child as *const _], None)
        })();
        ILFree(Some(pidl));
        result
    }
}
/// 原生拖出只允许复制，取消拖放不会改变置物架与原文件。
pub fn drag_file(path: &std::path::Path) -> Result<()> {
    unsafe {
        use windows::Win32::System::Ole::{OleInitialize, OleUninitialize, DROPEFFECT_COPY};
        use windows::Win32::UI::Shell::SHDoDragDrop;
        let initialized = OleInitialize(None).is_ok();
        let result = file_data_object(path)
            .and_then(|data| SHDoDragDrop(None, &data, None, DROPEFFECT_COPY).map(|_| ()));
        if initialized {
            OleUninitialize();
        }
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notifications_skip_history_but_detect_reused_ids_and_updated_content(){
        let mut baseline=NotificationBaseline{primed:true,..Default::default()};
        let first=ToastVersion::new(10,Some(crate::messenger::Messenger::Qq),Some("第一条"));
        assert!(!baseline.changed(1,first));
        baseline.previous=Some(HashMap::from([(1,first),(2,ToastVersion::new(10,None,None))]));
        assert!(!baseline.changed(1,first));
        assert!(baseline.changed(1,ToastVersion::new(10,Some(crate::messenger::Messenger::Qq),Some("第二条"))));
        assert!(baseline.changed(1,ToastVersion::new(11,Some(crate::messenger::Messenger::Qq),Some("第一条"))));
        assert!(baseline.unchanged_other(2,10).is_some());
        assert!(baseline.unchanged_other(1,10).is_none());
        assert!(baseline.unchanged_other(2,11).is_none());
        baseline.previous=Some(HashMap::new());assert!(baseline.changed(1,first));
    }
    #[test]
    fn notifications_arriving_during_connection_are_not_treated_as_old_history(){
        let baseline=NotificationBaseline{previous:None,started:100,primed:false};
        let (_,message)=baseline.observe(1,101,Some(crate::messenger::Messenger::Qq),true,Ok("连接期间的新消息".into()));
        assert!(message.is_some(),"连接期间才到达的新通知不能混入启动历史而漏报");
    }
    #[test]
    fn notifications_read_failures_do_not_replay_an_unchanged_qq_message(){
        let client=Some(crate::messenger::Messenger::Qq);
        let original=ToastVersion::new(10,client,Some("同一条消息"));
        let baseline=NotificationBaseline{previous:Some(HashMap::from([(1,original)])),..Default::default()};
        let (version,message)=baseline.observe(1,10,client,true,Err(windows::core::Error::from_win32()));
        assert!(message.is_none(),"临时读取失败不能变成空正文更新而重复提醒");
        assert!(version==original,"读取失败时保留上一轮指纹");
    }
    #[test]
    fn notifications_unreadable_new_toasts_remind_once_and_historical_toasts_stay_silent(){
        let client=Some(crate::messenger::Messenger::Qq);
        for (created,expected) in [(90,false),(110,true)]{
            let mut baseline=NotificationBaseline{previous:None,started:100,primed:false};
            let (version,message)=baseline.observe(1,created,client,true,Err(windows::core::Error::from_win32()));
            assert_eq!(message.is_some(),expected,"历史与连接期间的新通知必须区分");
            baseline.previous=Some(HashMap::from([(1,version)]));
            let (version,message)=baseline.observe(1,created,client,true,Ok("恢复后的正文".into()));
            assert!(message.is_none(),"正文恢复不能重弹同一条通知");
            baseline.previous=Some(HashMap::from([(1,version)]));
            assert!(baseline.observe(1,created,client,true,Ok("下一条消息".into())).1.is_some());
        }
    }
    #[test]
    fn notifications_office_channel_switch_does_not_replay_but_later_body_updates_remind(){
        for client in crate::messenger::Messenger::OFFICE{
            let mut baseline=NotificationBaseline{previous:Some(HashMap::from([(1,ToastVersion::new(10,Some(client),None))])),started:0,primed:true};
            assert!(baseline.unchanged_other(1,10).is_none(),"办公软件的捕获通道会切换，不能永久跳过");
            let (version,message)=baseline.observe(1,10,Some(client),true,Ok("切换前的旧消息".into()));
            assert!(message.is_none());baseline.previous=Some(HashMap::from([(1,version)]));
            assert!(baseline.observe(1,10,Some(client),true,Ok("切换后的新消息".into())).1.is_some());
        }
    }
    #[test]
    fn notifications_partial_snapshots_do_not_replay_old_items_or_drop_new_items(){
        let client=Some(crate::messenger::Messenger::Qq);
        let old=ToastVersion::new(90,client,Some("启动前的旧通知"));let fresh=ToastVersion::new(110,client,Some("连接后的新通知"));
        let mut baseline=NotificationBaseline{previous:None,started:100,primed:false};
        assert!(baseline.changed(2,fresh));baseline.finish(HashMap::from([(2,fresh)]),false);
        assert!(!baseline.changed(1,old),"首次只读到部分条目时，补读到的启动历史不能重放");
        assert!(!baseline.changed(2,fresh));baseline.finish(HashMap::from([(1,old),(2,fresh)]),true);
        baseline.finish(HashMap::from([(2,fresh)]),false);
        assert!(!baseline.changed(1,old),"临时漏读条目不能被当成移除后重新新增");
        let next=ToastVersion::new(120,client,Some("下一条"));assert!(baseline.changed(3,next));
        baseline.finish(HashMap::new(),true);
        assert!(baseline.previous.as_ref().unwrap().is_empty(),"成功读完整空列表后应清理历史，不能永久保留");
    }
    #[test]
    fn notifications_disconnect_signals_stop_without_waiting_for_new_messages(){
        let (_tx,incoming)=mpsc::sync_channel(32);let (stop,stopped)=mpsc::channel();
        drop(Notifications{incoming,stop,taskbar_mask:std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0))});
        assert_eq!(stopped.try_recv(),Ok(()));
    }
    #[test]
    fn browser_minimal_recognizes_browsers_without_matching_electron_apps(){
        for name in ["chrome.exe","MSEDGE.EXE","firefox.exe","brave.exe","zen.exe","QQBrowser.exe","quark.exe","Quark.exe","QUARK.EXE"] {
            assert!(is_browser_process(name),"应识别浏览器 {name}");
        }
        for name in ["Codex.exe","Code.exe","msedgewebview2.exe","explorer.exe","chrome-helper.exe","quark_proxy.exe","quark_pwa_launcher.exe","quark_host_client.exe","quark_swap_util.exe","QuarkUpdaterSetup.exe",""] {
            assert!(!is_browser_process(name),"不能将 {name} 当作浏览器");
        }
    }
    #[test]
    fn browser_minimal_checks_the_actual_top_overlap_on_the_island_monitor(){
        let island=RECT{left:560,top:0,right:1360,bottom:238};
        for browser in [RECT{left:0,top:-8,right:1920,bottom:1200},RECT{left:0,top:0,right:960,bottom:1150},RECT{left:700,top:10,right:1500,bottom:800}] {
            assert!(browser_overlaps_island(browser,island,crate::model::Dock::Top));
        }
        for browser in [RECT{left:100,top:150,right:1700,bottom:1100},RECT{left:1920,top:0,right:3840,bottom:1200},RECT{left:0,top:0,right:600,bottom:1000},RECT{left:0,top:-1200,right:1920,bottom:0}] {
            assert!(!browser_overlaps_island(browser,island,crate::model::Dock::Top));
        }
    }
    #[test]
    fn side_dock_avoidance_checks_the_covered_screen_edge(){
        use crate::model::Dock;
        // 1920×1080 屏幕，100% 缩放：左侧画布 0..500，竖直居中（240..840），岛体中心 y=540。
        let left=RECT{left:0,top:240,right:500,bottom:840};
        let right=RECT{left:1420,top:240,right:1920,bottom:840};
        let maximized=RECT{left:-8,top:-8,right:1928,bottom:1048};
        assert!(browser_overlaps_island(maximized,left,Dock::Left) && browser_overlaps_island(maximized,right,Dock::Right));
        let left_half=RECT{left:0,top:0,right:960,bottom:1040};
        assert!(browser_overlaps_island(left_half,left,Dock::Left) && !browser_overlaps_island(left_half,right,Dock::Right),"贴左半屏只遮住左侧");
        let floating=RECT{left:300,top:100,right:1600,bottom:900};
        assert!(!browser_overlaps_island(floating,left,Dock::Left) && !browser_overlaps_island(floating,right,Dock::Right),"没碰到边缘的窗口不避让");
        let top_strip=RECT{left:0,top:0,right:1920,bottom:300};
        assert!(!browser_overlaps_island(top_strip,left,Dock::Left),"没盖到岛所在的那一段边缘");
    }
    #[test]
    fn network_connection_uses_connected_wording() {
        let old=Snapshot{volume:None,audio_endpoint:None,caps:false,network:false,network_kind:NetworkKind::Other};
        let mut connected=old.clone();connected.network=true;
        assert_eq!(connected.changes(&old),vec![("网络连接".into(),"网络已连接".into())]);
    }
    #[test]
    fn network_switches_transport_and_keeps_the_disconnected_type() {
        let wifi=Snapshot{volume:None,audio_endpoint:None,caps:false,network:true,network_kind:NetworkKind::Wifi};
        let mut cable=wifi.clone();cable.network_kind=NetworkKind::Ethernet;
        assert_eq!(cable.changes(&wifi),vec![("网络连接".into(),"以太网 · 网络已连接".into())]);
        let mut off=cable.clone();off.network=false;off.network_kind=NetworkKind::Other;
        assert_eq!(off.changes(&cable),vec![("网络连接".into(),"以太网 · 网络已断开".into())]);
        assert_eq!(wifi.changes(&off),vec![("网络连接".into(),"Wi-Fi · 网络已连接".into())]);
    }
    #[test]
    fn switching_audio_endpoints_is_not_a_volume_adjustment() {
        let old=Snapshot{volume:Some((20,false)),audio_endpoint:Some("speakers".into()),caps:false,network:true,network_kind:NetworkKind::Other};
        let mut new=old.clone();
        new.audio_endpoint=Some("headphones".into());new.volume=Some((80,false));new.caps=true;
        assert_eq!(new.changes(&old),vec![("大写锁定".into(),"已开启".into())]);
        let mut adjusted=new.clone();adjusted.volume=Some((60,false));
        assert_eq!(adjusted.changes(&new),vec![("系统音量".into(),"音量 60%".into())]);
        adjusted.volume=Some((60,true));
        assert_eq!(adjusted.changes(&new),vec![("系统音量".into(),"已静音".into())]);
        assert!(!old.changes(&new).iter().any(|(title,_)|title=="系统音量"));
    }

    #[test]
    fn dragged_file_exposes_native_file_drop_format() {
        unsafe {
            use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
            use windows::Win32::System::Ole::{OleInitialize, OleUninitialize, CF_HDROP};
            let initialized = OleInitialize(None).is_ok();
            let path =
                std::env::temp_dir().join(format!("daozai-drag-test-{}.txt", std::process::id()));
            std::fs::write(&path, "test").unwrap();
            {
                let data = file_data_object(&path).unwrap();
                let format = FORMATETC {
                    cfFormat: CF_HDROP.0,
                    dwAspect: DVASPECT_CONTENT.0,
                    lindex: -1,
                    tymed: TYMED_HGLOBAL.0 as u32,
                    ..Default::default()
                };
                assert!(data.QueryGetData(&format).is_ok());
            }
            std::fs::remove_file(path).unwrap();
            if initialized {
                OleUninitialize();
            }
        }
    }
}

/// 把正在播放网页媒体的浏览器窗口带到前台。
/// Chrome / Edge 的窗口标题就是当前活动标签的标题，先用媒体标题前几个字匹配；
/// 匹配不到就激活 Z 序最前的浏览器窗口（EnumWindows 按 Z 序返回）。
/// 只能定位到窗口，没法切换窗口里的非活动标签（扩展路线才做得到）。
/// 把正在播放网页媒体的浏览器窗口带到前台。
/// Chrome/Edge 的窗口标题就是当前活动标签标题，用媒体标题前 8 个字匹配窗口；
/// 匹配不到就取 Z 序最前的浏览器窗口。只能定位窗口，切不了窗口内的非活动标签。
pub(crate) fn focus_browser_tab(title:&str)->bool{
    use windows::Win32::Foundation::{BOOL,LPARAM};
    struct Found { hwnd:Option<isize>, matched:bool, needle:String, candidates:usize, detail:String }
    unsafe extern "system" fn visit(hwnd:HWND,param:LPARAM)->BOOL{
        if !IsWindowVisible(hwnd).as_bool(){return BOOL(1);}
        let mut class=[0u16;64];let n=GetClassNameW(hwnd,&mut class);
        if n<=0 || String::from_utf16_lossy(&class[..n as usize])!="Chrome_WidgetWin_1" {return BOOL(1);}
        // 虚拟桌面 / 隐藏窗口不选。
        let mut cloaked=0u32;
        let _=windows::Win32::Graphics::Dwm::DwmGetWindowAttribute(hwnd,
            windows::Win32::Graphics::Dwm::DWMWA_CLOAKED,(&mut cloaked as *mut u32).cast(),4);
        if cloaked!=0{return BOOL(1);}
        let mut pid=0u32;GetWindowThreadProcessId(hwnd,Some(&mut pid));
        let Ok(process)=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,false,pid)else{return BOOL(1)};
        let mut path=[0u16;1024];let mut length=path.len() as u32;
        let ok=QueryFullProcessImageNameW(process,PROCESS_NAME_WIN32,PWSTR(path.as_mut_ptr()),&mut length).is_ok();
        let _=CloseHandle(process);
        if !ok{return BOOL(1);}
        let exe=std::path::Path::new(&String::from_utf16_lossy(&path[..length as usize]))
            .file_name().and_then(|s|s.to_str()).unwrap_or("").to_string();
        // Chrome_WidgetWin_1 是 Electron/CEF 通用类名：WorkBuddy、VS Code、QQ 都是。
        // 不按进程名过滤就会激活当前前台的 Electron 应用，看起来像「点了没反应」。
        if !is_browser_process(&exe){return BOOL(1);}
        let mut text=[0u16;1024];let len=GetWindowTextW(hwnd,&mut text);
        if len<=0{return BOOL(1);}
        let window_title=String::from_utf16_lossy(&text[..len as usize]);
        let state=&mut *(param.0 as *mut Found);
        state.candidates+=1;
        let hit=!state.needle.is_empty() && window_title.contains(state.needle.as_str());
        if state.hwnd.is_none() || (hit && !state.matched){
            state.hwnd=Some(hwnd.0 as isize);state.matched=hit;
            state.detail=format!("{exe} | {window_title}");
        }
        BOOL(1)
    }
    let mut found=Found{hwnd:None,matched:false,needle:title.chars().take(8).collect(),candidates:0,detail:String::new()};
    unsafe{let _=EnumWindows(Some(visit),LPARAM(&mut found as *mut _ as isize));}
    let Some(handle)=found.hwnd else{
        focus_browser_log(&format!("未找到浏览器窗口 | needle={:?} | 候选=0",found.needle));
        return false;
    };
    let hwnd=HWND(handle as *mut _);
    let ok=unsafe{
        if IsIconic(hwnd).as_bool(){let _=ShowWindow(hwnd,SW_RESTORE);}
        if GetForegroundWindow().0==hwnd.0 {true} else {
            // 岛仔主窗口是 WS_EX_NOACTIVATE，自己永远不是前台进程；
            // 把输入队列附到前台线程再抢前台，绕过 Windows 的前台锁。
            let fg=GetForegroundWindow();
            let fg_thread=GetWindowThreadProcessId(fg,None);
            let our=GetCurrentThreadId();
            let attached=fg_thread!=0 && fg_thread!=our && AttachThreadInput(our,fg_thread,true).as_bool();
            let _=BringWindowToTop(hwnd);
            let _=SetActiveWindow(hwnd);
            let mut done=SetForegroundWindow(hwnd).as_bool() && GetForegroundWindow().0==hwnd.0;
            if attached{let _=AttachThreadInput(our,fg_thread,false);}
            if !done {
                // 少数情况前台锁仍不认，模拟一次 ALT 松开再试。
                keybd_event(0x12,0,KEYBD_EVENT_FLAGS(0),0);
                keybd_event(0x12,0,KEYEVENTF_KEYUP,0);
                let _=SetForegroundWindow(hwnd);
                done=GetForegroundWindow().0==hwnd.0;
            }
            done
        }
    };
    focus_browser_log(&format!("needle={:?} | 候选={} | 命中标题={} | 选中={} | 前台={}",
        found.needle,found.candidates,found.matched,found.detail,ok));
    ok
}

fn focus_browser_log(message:&str){
    use std::io::Write as _;
    let path=crate::model::config_dir().join("diagnostics").join("focus-browser.log");
    if let Some(dir)=path.parent(){let _=std::fs::create_dir_all(dir);}
    if std::fs::metadata(&path).is_ok_and(|meta|meta.len()>256*1024){let _=std::fs::write(&path,b"");}
    if let Ok(mut file)=std::fs::OpenOptions::new().create(true).append(true).open(&path){
        let stamp=chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _=writeln!(file,"{stamp} | {message}");
    }
}

fn avoidance_matches(path:&str,settings:&crate::model::Settings)->bool {
    settings.browser_minimal && (crate::avoidance::contains_app(&settings.avoid_apps,path)
        || settings.avoid_browsers && std::path::Path::new(path).file_name().and_then(|name|name.to_str()).is_some_and(is_browser_process))
}

pub(crate) fn avoidance_windows()->std::result::Result<Vec<crate::avoidance::Candidate>,String> {
    use windows::Win32::Foundation::{BOOL,LPARAM};
    unsafe extern "system" fn visit(hwnd:HWND,param:LPARAM)->BOOL {
        if !IsWindowVisible(hwnd).as_bool() || GetWindowLongPtrW(hwnd,GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0!=0 {return BOOL(1);}
        let mut cloaked=0u32;
        let _=windows::Win32::Graphics::Dwm::DwmGetWindowAttribute(hwnd,windows::Win32::Graphics::Dwm::DWMWA_CLOAKED,(&mut cloaked as *mut u32).cast(),4);
        if cloaked!=0{return BOOL(1);}
        let mut title=[0u16;1024];let len=GetWindowTextW(hwnd,&mut title);
        if len==0{return BOOL(1);}
        let mut pid=0;GetWindowThreadProcessId(hwnd,Some(&mut pid));
        if pid==GetCurrentProcessId(){return BOOL(1);}
        let Ok(process)=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,false,pid)else{return BOOL(1)};
        let mut path=[0u16;32768];let mut length=path.len() as u32;
        let result=QueryFullProcessImageNameW(process,PROCESS_NAME_WIN32,PWSTR(path.as_mut_ptr()),&mut length);
        let _=CloseHandle(process);
        if result.is_err(){return BOOL(1);}
        let path=String::from_utf16_lossy(&path[..length as usize]);
        let items=&mut *(param.0 as *mut Vec<crate::avoidance::Candidate>);
        if items.iter().any(|a|crate::avoidance::same_app(&a.path,&path)){return BOOL(1);}
        let filename=std::path::Path::new(&path).file_name().and_then(|s|s.to_str()).unwrap_or("");
        let name=executable_description(&path).unwrap_or_else(||filename.trim_end_matches(".exe").to_string());
        items.push(crate::avoidance::Candidate{browser:is_browser_process(filename),path,name,title:String::from_utf16_lossy(&title[..len as usize])});
        BOOL(1)
    }
    let mut items:Vec<crate::avoidance::Candidate>=Vec::new();
    unsafe{EnumWindows(Some(visit),LPARAM(&mut items as *mut _ as isize))}.map_err(|_|"无法读取应用列表，请刷新重试".to_string())?;
    items.sort_by_key(|a|a.name.to_lowercase());
    Ok(items)
}

fn executable_description(path:&str)->Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW,GetFileVersionInfoW,VerQueryValueW};
    let path:Vec<u16>=path.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let size=GetFileVersionInfoSizeW(PCWSTR(path.as_ptr()),None);if size==0{return None;}
        let mut bytes=vec![0u8;size as usize];GetFileVersionInfoW(PCWSTR(path.as_ptr()),0,size,bytes.as_mut_ptr().cast()).ok()?;
        let mut ptr=std::ptr::null_mut();let mut len=0;
        if !VerQueryValueW(bytes.as_ptr().cast(),windows::core::w!("\\VarFileInfo\\Translation"),&mut ptr,&mut len).as_bool() || len<4{return None;}
        let translation=std::slice::from_raw_parts(ptr as *const u16,2);
        let key:Vec<u16>=format!("\\StringFileInfo\\{:04x}{:04x}\\FileDescription",translation[0],translation[1]).encode_utf16().chain(Some(0)).collect();
        if !VerQueryValueW(bytes.as_ptr().cast(),PCWSTR(key.as_ptr()),&mut ptr,&mut len).as_bool() || len<=1{return None;}
        let name=String::from_utf16_lossy(std::slice::from_raw_parts(ptr as *const u16,len as usize)).trim_end_matches('\0').trim().to_string();
        (!name.is_empty()).then_some(name)
    }
}

#[cfg(test)]
mod avoidance_tests {
    use super::*;
    #[test]
    fn avoidance_respects_master_browser_switch_and_custom_app_identity() {
        let mut settings=crate::model::Settings::default();
        settings.avoid_apps=vec![(r"D:\Apps\Editor.exe".into(),"编辑器".into())];
        assert!(!avoidance_matches(r"D:\Apps\Editor.exe",&settings));
        settings.browser_minimal=true;
        assert!(avoidance_matches(r"D:\Quark\quark.exe",&settings));
        assert!(avoidance_matches(r"d:\apps\EDITOR.EXE",&settings));
        assert!(!avoidance_matches(r"C:\Other\Editor.exe",&settings));
        settings.avoid_browsers=false;
        assert!(!avoidance_matches(r"D:\Quark\quark.exe",&settings));
        assert!(avoidance_matches(r"D:\Apps\Editor.exe",&settings));
        settings.avoid_apps.clear();
        assert!(!avoidance_matches(r"D:\Apps\Editor.exe",&settings));
    }
    #[test]
    fn avoidance_window_list_excludes_this_process_and_deduplicates_apps() {
        let entries=avoidance_windows().unwrap();
        let current=std::env::current_exe().unwrap().to_string_lossy().into_owned();
        for (index,app) in entries.iter().enumerate() {
            assert!(!crate::avoidance::same_app(&app.path,&current));
            assert!(!app.name.is_empty() && !app.title.is_empty());
            assert!(!entries[..index].iter().any(|previous|crate::avoidance::same_app(&previous.path,&app.path)));
        }
        let notepad=std::path::Path::new(r"C:\Windows\System32\notepad.exe");
        if notepad.exists(){assert!(executable_description(notepad.to_str().unwrap()).is_some());}
    }
}

// —— 自定义构建补回：手机连接通道（Phone Link）——
const PHONE_CHANNELS: &[(&str,&str)] = &[
    ("YourPhoneMessages_","短信"),
    ("YourPhoneCalling","通话"),
    ("YourPhoneDeviceStatus","手机状态"),
    ("com.apple.mobilephone","电话"),
    ("com.tencent.xin","微信"),
    ("com.tencent.mqq","QQ"),
    ("com.alipay.iphoneclient","支付宝"),
    ("com.google.Gmail","Gmail"),
];
/// 手机连接 Toast 的来源判定。
///
/// WinRT 只报主 AUMID（`!App`），拿不到通道信息，所以只能看正文：
/// 正文格式是「来源 · 内容…」，但短信用**手机号**当来源（`+8610… · 【xx】验证码…`），
/// 电话是「号码 · 传入呼叫 / 未接来电」，普通 App 才是「微信 · 张三 · 内容」。
///
/// 判定顺序：名单命中 → 来电标记 → 短信特征 → 用首段当标签兜底。
/// 名单放在 `%APPDATA%\Daozai\phone-apps.json`，加新 App 不用改代码重编译。
/// 返回 (标签, 是否允许上岛)；None = 无法识别，由调用方按「连接」放行。
pub(crate) fn phone_app(body:&str,apps:&crate::model::PhoneApps)->Option<(String,bool)> {
    let head=body.split('·').next().unwrap_or("").trim();
    if head.is_empty() {return None;}
    // 1. 名单优先：微信 / QQ / 支付宝 / Gmail 等。
    if let Some(hit)=apps.lookup(head){return Some(hit);}
    // 2. 来电：号码当来源，正文带这两个词。
    if body.contains("传入呼叫")||body.contains("未接来电"){return Some(("电话".to_string(),true));}
    // 3. 短信：来源是手机号，或正文有【签名】/验证码字样。
    let digits=head.chars().all(|c|c.is_ascii_digit()||matches!(c,'+'|'-'|' '|'('|')'));
    if digits||body.contains("验证码")||body.contains('【')||body.contains("短信"){
        return Some(("短信".to_string(),true));
    }
    // 4. 兜底：短名字直接当标签（例如 WorkBuddy），过长才交给调用方。
    if head.chars().count()<=12 {return Some((head.to_string(),true));}
    None
}

pub(crate) fn phone_channel(id:&str)->bool {
    // 只要是手机连接（Phone Link）发出来的就归手机通道：
    // WinRT 有时给主 AUMID（`!App`），有时给子通道（`YourPhoneMessages_…` /
    // `YourPhoneNotifications_<uuid>`），开机后通道注册晚一步还会先走汇总通道。
    // 白名单只用来起标签，是否上岛/静音交给 phone-apps.json 按正文判断。
    id.starts_with("Microsoft.YourPhone_")
}
pub(crate) fn phone_label(id:&str)->&'static str {
    if id.ends_with("!App"){return "连接";}
    PHONE_CHANNELS.iter().find(|(key,_)|id.contains(*key)).map(|(_,label)|*label).unwrap_or("手机")
}

#[cfg(test)]
mod phone_channel_tests {
    use super::{phone_app,phone_channel,phone_label};
    #[test]
    fn 只放行手机连接白名单通道(){
        // 现在只要是手机连接的 AUMID 都算手机通道（白名单只管标签）；
        // 真正决定「上不上岛 / 静不静音」的是正文 + phone-apps.json。
        assert!(phone_channel("Microsoft.YourPhone_8wekyb3d8bbwe!YourPhoneNotifications_com.tencent.xin"));
        assert!(phone_channel("Microsoft.YourPhone_8wekyb3d8bbwe!YourPhoneMessages_9d62fe0c"));
        assert!(phone_channel("Microsoft.YourPhone_8wekyb3d8bbwe!App"));
        // 开机后通道还没注册时，Win11 会先走汇总通道，这个也必须算手机通知。
        assert!(phone_channel("Microsoft.YourPhone_8wekyb3d8bbwe!YourPhoneNotifications_9d62fe0c-7488-42c8-be15-0fbe4a087b7f"));
        assert_eq!(phone_label("Microsoft.YourPhone_x!App"),"连接");
        assert!(phone_channel("Microsoft.YourPhone_8wekyb3d8bbwe!YourPhoneNotifications_com.valvesoftware.Steam"));
        assert!(!phone_channel("QQ"));
        assert!(!phone_channel("mail.test"));
        assert_eq!(phone_label("Microsoft.YourPhone_x!YourPhoneNotifications_com.tencent.xin"),"微信");
        assert_eq!(phone_label("Microsoft.YourPhone_x!YourPhoneNotifications_com.google.Gmail"),"Gmail");
    }
    #[test]
    fn 从正文首段识别手机应用与黑名单(){
        let apps=crate::model::PhoneApps::builtin();
        assert_eq!(crate::system::phone_app("QQ · 小号 · 这是一条测试正文内容",&apps),Some(("QQ".to_string(),true)));
        assert_eq!(crate::system::phone_app("微信 · 某人 · 晚上好",&apps),Some(("微信".to_string(),true)));
        assert_eq!(crate::system::phone_app("来电 · 138xxxx",&apps),Some(("电话".to_string(),true)));
        assert_eq!(crate::system::phone_app("Steam · 夏季特惠",&apps),Some(("连接".to_string(),false)));
        assert_eq!(crate::system::phone_app("",&apps),None);
        assert_eq!(crate::system::phone_app("一条很长的普通消息内容里没有应用名",&apps),None);
        // 名单是外部配置：加新 App 只需往 JSON 里加一条，不改代码。
        let mut custom=crate::model::PhoneApps::builtin();
        custom.items.push(("小红书".to_string(),"小红书".to_string(),true));
        assert_eq!(crate::system::phone_app("小红书 · 有人回复了你",&custom),Some(("小红书".to_string(),true)));
    }
    #[test]
    fn 短信与来电按正文特征识别(){
        let apps=crate::model::PhoneApps::builtin();
        // 短信用手机号当来源：认不出号码也要归到「短信」，不能退化成「连接」。
        assert_eq!(phone_app("+86106826974003139 · 【清枫北京科技】您的验证码：4342，如非本人操作",&apps),Some(("短信".to_string(),true)));
        assert_eq!(phone_app("10691234 · 您的验证码 8899",&apps),Some(("短信".to_string(),true)));
        // 响铃与未接都先归到「电话」，再由正文区分「来电中 / 未接来电」。
        assert_eq!(phone_app("130 8646 4686 · 传入呼叫",&apps),Some(("电话".to_string(),true)));
        assert_eq!(phone_app("电话 · 130 8646 4686 · 未接来电",&apps),Some(("电话".to_string(),true)));
        // 名单外的短名字直接用名字当标签。
        assert_eq!(phone_app("WorkBuddy · 任务已完成",&apps),Some(("WorkBuddy".to_string(),true)));
    }

    #[test]
    fn only_real_browsers_pass_the_chromium_window_class_filter(){
        // Chrome_WidgetWin_1 是 Electron/CEF 通用类名：WorkBuddy、VS Code、QQ 都用它。
        // 只按类名挑窗口会激活前台的 Electron 应用，看起来像「点了没反应」。
        assert!(super::is_browser_process("chrome.exe"));
        assert!(super::is_browser_process("msedge.exe"));
        assert!(!super::is_browser_process("WorkBuddy.exe"));
        assert!(!super::is_browser_process("Code.exe"));
        assert!(!super::is_browser_process("QQ.exe"));
        assert!(!super::is_browser_process(""));
    }
}
